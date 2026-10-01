"""Command line: ``python -m bench <command> [experiment.json]``.

Commands:
  plan        render graphs, runtime plans and the attempt order without running anything
  check-key   confirm OPENAI_API_KEY can reach the experiment's model (prints only a status)
  smoke       isolation checks, a diagnostic run, the full pipeline with short limits, and a
              scoring-fidelity check (defaults to experiments/smoke.json)
  run         run every attempt, then evaluate and report
  eval        (re)evaluate and score existing attempts
  report      rebuild summary.json and summary.md
  cleanup     remove this experiment's containers and networks
"""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import signal
import socket
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from . import ROOT, config, evaluate, images, report, smoke
from .attempt import Attempt, run_files
from .util import (
    docker,
    load_secret_file,
    log,
    read_json,
    require_secret,
    set_log_file,
    write_json,
)

RESULTS = Path(os.environ.get("ZSBENCH_RESULTS", "/results"))
CACHE = Path(os.environ.get("ZSBENCH_CACHE", "/cache"))
# One key file per provider, mounted read-only by scripts/zsbench (whichever exist).
SECRET_FILES = (os.environ.get("ZSBENCH_SECRET_FILE", "/run/secrets/openai.env"), "/run/secrets/anthropic.env", "/run/secrets/openrouter.env")
GIB = 1 << 30


def _results(exp: config.Experiment) -> Path:
    path = RESULTS / exp.id
    path.mkdir(parents=True, exist_ok=True)
    set_log_file(path / "run.log")
    return path


def _provenance(exp: config.Experiment) -> dict:
    runner_image = docker("inspect", "-f", "{{.Image}}", socket.gethostname(), check=False).strip() or "unknown"
    return {
        "vcs_ref": os.environ.get("ZSBENCH_VCS_REF", "unknown"),
        "vcs_dirty": os.environ.get("ZSBENCH_VCS_DIRTY", "unknown"),
        "runner_image": runner_image,
        "experiment_digest": exp.digest(),
    }


def cmd_plan(exp: config.Experiment) -> None:
    results = _results(exp)
    for arm in sorted(set(exp.raw["order"])):
        for name, value in run_files(exp, arm).items():
            write_json(results / "plan" / arm / name, value)
    log(f"experiment {exp.id} digest {exp.digest()[:16]}")
    for spec in exp.attempts():
        log(f"  attempt {spec.label}")
    log(f"rendered graphs, runtime plans and input under {results / 'plan'}")


def cmd_check_key(exp: config.Experiment) -> None:
    require_secret(exp.secret_env)
    key = os.environ[exp.secret_env]
    if exp.provider == "anthropic":
        request = urllib.request.Request(f"https://api.anthropic.com/v1/models/{exp.model}", headers={"x-api-key": key, "anthropic-version": "2023-06-01"})
    elif exp.provider == "openrouter":  # the key's own record; the model is checked by the smoke test's run
        request = urllib.request.Request("https://openrouter.ai/api/v1/key", headers={"Authorization": f"Bearer {key}"})
    else:
        request = urllib.request.Request(f"https://api.openai.com/v1/models/{exp.model}", headers={"Authorization": f"Bearer {key}"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            print(f"OK: key accepted and {exp.model} is available (HTTP {response.status})")
    except urllib.error.HTTPError as error:
        sys.exit(f"FAILED: HTTP {error.code} for {exp.model} (401 = key rejected, 404 = model unavailable to this key)")


def _memory_bytes(value: str) -> int:
    units = {"k": 1 << 10, "m": 1 << 20, "g": GIB}
    return int(float(value[:-1]) * units[value[-1].lower()]) if value[-1].lower() in units else int(value)


def _preflight(exp: config.Experiment, results: Path) -> None:
    running = docker("ps", "-q", "--filter", f"label=zsbench.experiment={exp.id}").split()
    if running:
        sys.exit(f"{len(running)} container(s) of experiment {exp.id} are still running (another runner, or a crashed one). Run `cleanup` first.")
    res = exp.resources
    cpus = float(docker("info", "--format", "{{.NCPU}}").strip())
    memory = int(docker("info", "--format", "{{.MemTotal}}").strip())
    if cpus < float(res["cpus"]):
        sys.exit(f"host has {cpus:g} CPUs; each attempt needs {res['cpus']}")
    if res["concurrency"] * float(res["cpus"]) > cpus:
        sys.exit(f"{res['concurrency']} concurrent attempts x {res['cpus']} CPUs oversubscribes this {cpus:g}-CPU host; lower resources.concurrency in the experiment file")
    needed = res["concurrency"] * _memory_bytes(res["memory"]) + 4 * GIB
    if needed > memory:
        sys.exit(f"host has {memory / GIB:.1f} GiB; {res['concurrency']} x {res['memory']} plus 4 GiB headroom needs {needed / GIB:.1f} GiB")
    free = shutil.disk_usage(results).free
    if free < 60 * GIB:
        sys.exit(f"only {free / GIB:.1f} GiB free under {results}; need at least 60 GiB")


def _prepare(exp: config.Experiment, results: Path, allow_mixed: bool) -> tuple[str, str, dict]:
    provenance = _provenance(exp)
    manifest_path = results / "manifest.json"
    if manifest_path.exists() and any((results / "attempts").glob("*/attempt.json")):
        previous = read_json(manifest_path).get("provenance") or {}
        if previous.get("experiment_digest") != provenance["experiment_digest"] and not allow_mixed:
            sys.exit("results already hold attempts from a different experiment digest (code, prompts or config changed). Use a new results directory, or --allow-mixed.")
    _preflight(exp, results)
    proxy_image = images.build_gateway(CACHE) if exp.harness == "claude" else images.build_proxy(CACHE, exp.api_host)
    agent_image, info = images.build_agent(exp, CACHE)
    history = read_json(manifest_path).get("invocations", []) if manifest_path.exists() else []
    write_json(manifest_path, {
        "experiment": exp.raw,
        "provenance": provenance,
        "invocations": [*history, {"at": time.time(), **provenance}],
        "pins": config.pins(),
        "agent_image": agent_image,
        "agent_image_id": docker("image", "inspect", agent_image, "--format", "{{.Id}}").strip(),
        "task_image": info["task_image"],
        "task_image_id": docker("image", "inspect", exp.task_image, "--format", "{{.Id}}").strip(),
        "eval_image": f"{exp.task_image.split(':')[0]}:{evaluate.eval_image_tag(exp)}",
        "codex_config": info.get("codex_config"),
        "harness_config": info.get("harness_config"),
        "task_adjustments": info["task_adjustments"],
        "proxy_image": proxy_image,
        "prompts": {name: config.prompt(name) for name in ("builder", "checker", "task")},
        "runner": {"python": platform.python_version()},
        "host": {"docker": docker("version", "--format", "{{.Server.Version}}").strip(), "cpus": docker("info", "--format", "{{.NCPU}}").strip(), "memory_bytes": docker("info", "--format", "{{.MemTotal}}").strip()},
    })
    return agent_image, proxy_image, provenance


def _run_attempts(exp: config.Experiment, results: Path, agent_image: str, proxy_image: str, provenance: dict, keep: bool) -> bool:
    """Run every attempt; returns True if a stop was requested."""
    attempts = [Attempt(exp, spec, results, agent_image, proxy_image, provenance, keep) for spec in exp.attempts()]
    stopped = False

    def stop(*_: object) -> None:
        # Only set flags: a signal can land while the main thread holds the log lock.
        nonlocal stopped
        stopped = True
        for attempt in attempts:
            attempt.request_stop()

    previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        with ThreadPoolExecutor(max_workers=exp.resources["concurrency"]) as pool:
            futures = {pool.submit(a.run): a for a in attempts}  # submitted in the pre-registered order
            announced = False
            for future in as_completed(futures):
                if stopped and not announced:
                    log("stop requested: running attempts are force-stopped, queued attempts are skipped")
                    announced = True
                try:
                    meta = future.result()
                except Exception as error:  # an attempt records its own errors; this is the runner failing around it
                    log(f"[{futures[future].spec.label}] RUNNER ERROR {type(error).__name__}: {error}")
                    continue
                log(f"[{meta['label']}] {meta['state']} after {meta.get('wall_seconds', 0) / 60:.1f} min")
    finally:
        # Outside the attempt phase a stop ends the runner at once (see _exit_on_signal).
        for sig, handler in previous.items():
            signal.signal(sig, handler)
    return stopped


def cmd_run(exp: config.Experiment, keep: bool, skip_eval: bool, allow_mixed: bool) -> dict:
    require_secret(exp.secret_env)
    _refuse_while_another_runner_runs("run")
    results = _results(exp)
    agent_image, proxy_image, provenance = _prepare(exp, results, allow_mixed)
    stopped = _run_attempts(exp, results, agent_image, proxy_image, provenance, keep)
    if stopped:
        log("stop requested: skipping evaluation (resume with `run`, or score what exists with `eval`)")
    elif not skip_eval:
        evaluate.evaluate(exp, results, CACHE)
    summary = report.build(exp, results)
    summary["stopped"] = stopped
    log(f"summary written to {results / 'summary.md'}")
    _guard_secrets(results, summary)
    return summary


def _guard_secrets(results: Path, summary: dict) -> None:
    marker = results / "DO-NOT-PUBLISH.txt"
    if summary["secrets"]["literal_key_hits"]:
        marker.write_text("The API key appears in these artifacts:\n" + "\n".join(summary["secrets"]["literal_key_hits"]) + "\n")
        sys.exit(f"SECRET LEAK: the API key appears in results; see {marker}")
    marker.unlink(missing_ok=True)


def _record_invocation(results: Path, exp: config.Experiment, command: str) -> None:
    """Re-scoring runs the current code on old attempts: record which code did it."""
    manifest_path = results / "manifest.json"
    if manifest_path.exists():
        manifest = read_json(manifest_path)
        manifest["invocations"] = [*manifest.get("invocations", []), {"at": time.time(), "command": command, **_provenance(exp)}]
        write_json(manifest_path, manifest)


def cmd_eval(exp: config.Experiment, force: bool) -> None:
    _refuse_while_another_runner_runs("eval")
    results = _results(exp)
    _record_invocation(results, exp, "eval")
    evaluate.evaluate(exp, results, CACHE, force=force)
    cmd_report(exp)


def cmd_report(exp: config.Experiment) -> None:
    results = _results(exp)
    _record_invocation(results, exp, "report")
    summary = report.build(exp, results)
    print((results / "summary.md").read_text())
    _guard_secrets(results, summary)


def cmd_smoke(exp: config.Experiment) -> None:
    require_secret(exp.secret_env)
    results = _results(exp)
    if any((results / "attempts").glob("*/attempt.json")):
        sys.exit(f"{results} already holds a smoke test; move it aside to run the smoke test again")
    agent_image, proxy_image, _ = _prepare(exp, results, allow_mixed=False)
    s = smoke.Smoke(exp, results, CACHE, agent_image, proxy_image)
    s.isolation()
    s.diagnostic_run()
    summary = cmd_run(exp, keep=False, skip_eval=False, allow_mixed=False)
    if summary["stopped"]:
        sys.exit("smoke stopped before the pipeline finished; nothing was checked after the stop")
    smoke.pipeline_checks(s, summary)
    s.scoring_fidelity()
    outcome = s.summary()
    failed = [name for name, c in outcome["checks"].items() if not c["ok"]]
    log(f"smoke {'PASSED' if not failed else 'FAILED'}: {len(outcome['checks']) - len(failed)}/{len(outcome['checks'])} checks passed" + (f"; failed: {failed}" if failed else ""))
    if failed:
        sys.exit(1)


def _refuse_while_another_runner_runs(command: str) -> None:
    """`cleanup` would destroy a live runner's attempts, and `eval` would compete with them for CPUs."""
    own = socket.gethostname()
    others = [c for c in docker("ps", "-q", "--filter", "label=zsbench.runner=1").split() if not own.startswith(c) and not c.startswith(own)]
    if others:
        sys.exit(f"another zsbench runner is running ({', '.join(others)}); stop it before `{command}`")


def cmd_cleanup(exp: config.Experiment) -> None:
    _refuse_while_another_runner_runs("cleanup")
    for container in docker("ps", "-aq", "--filter", f"label=zsbench.experiment={exp.id}").split():
        docker("rm", "-f", container, check=False)
    for network in docker("network", "ls", "-q", "--filter", f"label=zsbench.experiment={exp.id}").split():
        docker("network", "rm", network, check=False)
    log(f"removed containers and networks for {exp.id}")


def _hand_back_results() -> None:
    """The runner runs as root; give the results back to the invoking host user."""
    uid, gid = os.environ.get("ZSBENCH_HOST_UID"), os.environ.get("ZSBENCH_HOST_GID")
    if not (uid and gid and RESULTS.exists()):
        return
    for path in [RESULTS, *RESULTS.rglob("*")]:
        try:
            os.lchown(path, int(uid), int(gid))
        except OSError:
            pass


def _exit_on_signal(signum: int, _frame: object) -> None:
    raise SystemExit(128 + signum)  # unwinds through `finally`, so results are still handed back


def main() -> None:
    parser = argparse.ArgumentParser(prog="bench", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["plan", "check-key", "smoke", "run", "eval", "report", "cleanup"])
    parser.add_argument("experiment", nargs="?", help="experiment JSON (default: experiments/smoke.json for smoke)")
    parser.add_argument("--keep-containers", action="store_true", help="leave attempt containers for inspection")
    parser.add_argument("--skip-eval", action="store_true", help="run attempts without evaluating")
    parser.add_argument("--force", action="store_true", help="re-evaluate archives that already have results")
    parser.add_argument("--allow-mixed", action="store_true", help="resume even if the experiment digest changed")
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, _exit_on_signal)
    for path in SECRET_FILES:
        load_secret_file(path)
    default = ROOT / "experiments" / ("smoke.json" if args.command == "smoke" else "luna-xhigh-svgbob.json")
    exp = config.load(args.experiment or default)
    try:
        if args.command == "plan":
            cmd_plan(exp)
        elif args.command == "check-key":
            cmd_check_key(exp)
        elif args.command == "smoke":
            cmd_smoke(exp)
        elif args.command == "run":
            cmd_run(exp, args.keep_containers, args.skip_eval, args.allow_mixed)
        elif args.command == "eval":
            cmd_eval(exp, args.force)
        elif args.command == "report":
            cmd_report(exp)
        elif args.command == "cleanup":
            cmd_cleanup(exp)
    finally:
        _hand_back_results()


if __name__ == "__main__":
    main()

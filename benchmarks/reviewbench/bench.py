#!/usr/bin/env python3
"""Local ReviewBench harness for the Zeroshot review graph.

Runs the reviewer image on benchmark pull requests the way the benchmark does
(one fresh container per pull request, the contract's mounts and variables, a
30-minute limit, a runner-sized container, network limited to the model
hosts), then scores the findings with ReviewBench's own judging pipeline and
the leaderboard's judge model. ANTHROPIC_API_KEY and OPENAI_API_KEY must be set.

    bench.py pick --set full --exclude test --count 50   # seeded sample of pull requests
    bench.py prepare --set full --keys KEY...            # frozen checkouts, diffs, metadata
    bench.py eval --exp NAME --keys KEY... --rounds 1    # run, judge, score and audit
    bench.py run --exp NAME --keys KEY... --rounds 1     # or the steps one by one
    bench.py judge --exp NAME --rounds 1
    bench.py score --exp NAME
    bench.py audit --exp NAME
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import hashlib
import json
import os
import random
import shutil
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORK = Path(os.environ.get("RB_WORK", Path.home() / "dev" / "reviewbench-work"))
BENCH = WORK / "ReviewBench"
PRS = WORK / "prs"
RUNS = WORK / "runs"
TMP = WORK / "tmp"
NETWORK = "rb-internal"
PROXY = os.environ.get("RB_PROXY", "http://rb-egress-proxy:3128")
PROXY_LOG = Path(os.environ.get("RB_PROXY_LOG", WORK / "proxy-log" / "egress.jsonl"))
MIRROR = "https://github.com/review-bench/{}.git"
JUDGE_MODEL = "claude-sonnet-5"
AGENT = "zeroshot-code-review"
PR_LIMIT_SECONDS = 1800
# Reviewer container limits: about ReviewBench's runner, a standard GitHub-hosted runner
# (4 CPUs, 16 GB) that it shares with its driver.
CONTAINER_CPUS = os.environ.get("RB_CPUS", "4")
CONTAINER_MEMORY = os.environ.get("RB_MEMORY", "14g")
SEED = 20261029
LOCK = threading.Lock()


def log(message: str, path: Path | None = None) -> None:
    line = f"{datetime.datetime.now().strftime('%H:%M:%S')} {message}"
    with LOCK:
        print(line, flush=True)
        if path:
            with path.open("a") as handle:
                handle.write(line + "\n")


def manifest(set_name: str) -> list[dict]:
    path = BENCH / ("corpus/test/test.json" if set_name == "test" else "corpus/manifest.json")
    return json.loads(path.read_text())


def key_of(entry: dict) -> str:
    return f"{entry['nwo'].replace('/', '_')}_{entry['pr_number']}-{entry['head'][:8]}"


def entries_by_key() -> dict[str, dict]:
    return {key_of(e): e for e in manifest("full")}


def git(*args: str, cwd: Path | None = None, check: bool = True) -> str:
    env = dict(os.environ, GIT_LFS_SKIP_SMUDGE="1", GIT_TERMINAL_PROMPT="0", TMPDIR=str(TMP))
    result = subprocess.run(["git", *args], cwd=cwd, env=env, capture_output=True, text=True)
    if check and result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {result.stderr.strip()[:500]}")
    return result.stdout.strip()


# --- credentials -------------------------------------------------------------

API_KEYS = ("ANTHROPIC_API_KEY", "OPENAI_API_KEY")


def require_keys(*names: str) -> None:
    """Keys come from the environment and reach a container by name only, never on a command line."""
    missing = [name for name in names if not os.environ.get(name)]
    if missing:
        sys.exit(f"set {' and '.join(missing)} in the environment")


# --- prepare -----------------------------------------------------------------


def prepare(entry: dict) -> Path:
    key = key_of(entry)
    dest = PRS / key
    if (dest / "ready").exists():
        return dest
    stage = PRS / f".{key}.tmp"
    shutil.rmtree(stage, ignore_errors=True)
    (stage / "pr").mkdir(parents=True)
    url = MIRROR.format(entry["nwo"].replace("/", "_"))
    base, head = entry["base"], entry["head"]

    probe = stage / "probe"
    git("init", "-q", str(probe))
    git("fetch", "-q", "--no-tags", "--filter=blob:none", url, base, head, cwd=probe)
    merge_base = git("merge-base", base, head, cwd=probe)
    depth = max(int(git("rev-list", "--count", f"{merge_base}..{head}", cwd=probe)),
                int(git("rev-list", "--count", f"{merge_base}..{base}", cwd=probe))) + 2

    # A frozen checkout like the benchmark's: history only down to the merge
    # base, no remotes, no record of where it came from.
    repo = stage / "repo"
    git("init", "-q", str(repo))
    git("fetch", "-q", "--no-tags", f"--depth={depth}", url, base, head, cwd=repo)
    for _ in range(4):
        if git("merge-base", base, head, cwd=repo, check=False):
            break
        depth *= 4
        git("fetch", "-q", "--no-tags", f"--depth={depth}", url, base, head, cwd=repo)
    else:
        git("fetch", "-q", "--no-tags", "--unshallow", url, base, head, cwd=repo)
    git("checkout", "-q", "--detach", head, cwd=repo)
    for leftover in ("FETCH_HEAD", "logs"):
        target = repo / ".git" / leftover
        shutil.rmtree(target, ignore_errors=True) if target.is_dir() else target.unlink(missing_ok=True)
    diff = subprocess.run(["git", "diff", f"{base}...{head}"], cwd=repo, capture_output=True, check=True).stdout
    (stage / "pr" / "diff.patch").write_bytes(diff)
    pr = {"repo": entry["repo"], "pr_number": int(entry["pr_number"]), "base": base, "head": head,
          "nwo": entry["nwo"], "title": entry["title"], "body": entry["body"]}
    (stage / "pr" / "pr.json").write_text(json.dumps(pr, indent=2))
    shutil.rmtree(probe)
    size = sum(f.stat().st_size for f in repo.rglob("*") if f.is_file())
    (stage / "meta.json").write_text(json.dumps({"key": key, "depth": depth, "repo_bytes": size,
                                                 "diff_lines": diff.count(b"\n")}, indent=2))
    shutil.rmtree(dest, ignore_errors=True)
    stage.rename(dest)
    (dest / "ready").touch()
    return dest


# --- run ---------------------------------------------------------------------


def validate(path: Path, entry: dict) -> str | None:
    try:
        document = json.loads(path.read_text())
    except (OSError, ValueError):
        return "no valid JSON at RB_OUT"
    if document.get("pr", {}).get("head") != entry["head"]:
        return "pr.head is not RB_HEAD"
    if int(document.get("pr", {}).get("pr_number", -1)) != int(entry["pr_number"]):
        return "pr.pr_number is not RB_PR_NUMBER"
    if not isinstance(document.get("findings"), list):
        return "findings is not an array"
    for f in document["findings"]:
        if not (isinstance(f.get("file"), str) and isinstance(f.get("start_line"), int)
                and isinstance(f.get("end_line"), int) and isinstance(f.get("message"), str)) \
                or f["file"].startswith(("./", "/")):
            return "a finding misses file, start_line, end_line or message"
    return None


def run_pr(exp: Path, round_no: int, entry: dict, config: Path, image: str, force: bool) -> dict:
    """Run one pull request the way the benchmark's runner does, then collect what the audit needs."""
    key = key_of(entry)
    round_dir = exp / f"round-{round_no}"
    run_dir = round_dir / "prs" / key
    done = run_dir / "done.json"
    if done.exists() and not force:
        return json.loads(done.read_text())
    template = prepare(entry)
    shutil.rmtree(run_dir, ignore_errors=True)
    (run_dir / "out").mkdir(parents=True)
    subprocess.run(["cp", "-a", str(template / "repo"), str(run_dir / "repo")], check=True)
    container = f"rb-{exp.name}-r{round_no}-{hashlib.sha1(key.encode()).hexdigest()[:8]}"
    # The contract's mounts and variables, plus what the runner adds: the proxy and git's safe directory.
    variables = {
        "RB_NWO": entry["nwo"], "RB_PR_NUMBER": str(entry["pr_number"]), "RB_BASE": entry["base"], "RB_HEAD": entry["head"],
        "RB_AGENT": AGENT, "RB_DIFF": "/work/pr/diff.patch", "RB_PR_JSON": "/work/pr/pr.json",
        "RB_OUT": "/work/out/findings.json", "RB_ATTEMPT": "1",
        "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "safe.directory", "GIT_CONFIG_VALUE_0": "*",
        "HTTPS_PROXY": PROXY, "HTTP_PROXY": PROXY, "https_proxy": PROXY, "http_proxy": PROXY, "NODE_USE_ENV_PROXY": "1",
    }
    command = ["docker", "run", "--name", container, "--network", NETWORK,
               "--memory", CONTAINER_MEMORY, "--pids-limit", "8192", "--cpus", CONTAINER_CPUS,
               "-v", f"{run_dir / 'repo'}:/work/repo", "-v", f"{template / 'pr'}:/work/pr:ro",
               "-v", f"{run_dir / 'out'}:/work/out"]
    for variable, value in variables.items():
        command += ["-e", f"{variable}={value}"]
    for variable in API_KEYS:
        command += ["-e", variable]
    command.append(image)
    subprocess.run(["docker", "rm", "-f", container], capture_output=True)
    started = time.time()
    status = "ok"
    with (run_dir / "container.log").open("w") as output:
        try:
            exit_code = subprocess.run(command, stdout=output, stderr=subprocess.STDOUT,
                                       timeout=PR_LIMIT_SECONDS).returncode
        except subprocess.TimeoutExpired:
            subprocess.run(["docker", "kill", container], capture_output=True)
            exit_code, status = None, "timeout"
    seconds = round(time.time() - started, 1)
    # From outside the container: keep its home (the agents' transcripts) for the audit, then remove it.
    subprocess.run(["docker", "cp", f"{container}:/root", str(run_dir / "home")], capture_output=True)
    subprocess.run(["docker", "rm", "-f", container], capture_output=True)
    findings_path = run_dir / "out" / "findings.json"
    problem = None if status == "timeout" else (f"exit code {exit_code}" if exit_code else validate(findings_path, entry))
    if problem:
        status = "failed"
    summary = adapter_summary(run_dir)
    result = {"key": key, "status": status, "problem": problem, "seconds": seconds, "exit": exit_code,
              "source": summary.get("source"), "graph_status": summary.get("graph_status"),
              "completed_nodes": summary.get("completed_nodes"), "findings": None}
    if status == "ok":
        (round_dir / "findings").mkdir(parents=True, exist_ok=True)
        shutil.copy2(findings_path, round_dir / "findings" / f"{key}.json")
        result["findings"] = len(json.loads(findings_path.read_text())["findings"])
    # The reviewer ran as root, so files it left in the checkout may be root's: remove it from a container.
    subprocess.run(["docker", "run", "--rm", "--network", "none", "-v", f"{run_dir}:/run", "--entrypoint", "rm",
                    image, "-rf", "/run/repo"], capture_output=True)
    shutil.rmtree(run_dir / "repo", ignore_errors=True)
    done.write_text(json.dumps(result, indent=2))
    return result


def adapter_summary(run_dir: Path) -> dict:
    """The adapter's closing summary line, from the container's log."""
    path = run_dir / "container.log"
    lines = path.read_text(errors="replace").splitlines() if path.exists() else []
    for line in reversed(lines):
        if "] summary {" in line:
            try:
                return json.loads(line.split("] summary ", 1)[1])
            except ValueError:
                break
    return {}


HOST_GATE = threading.Lock()


def wait_for_host(bench_log: Path | None, max_io_percent: float = 35.0, min_free_gb: float = 6.0) -> None:
    """Hold a new run while the shared host is too loaded to run it reliably.

    Under heavy memory and I/O pressure, Zeroshot cannot confirm that provider
    processes exited and fails whole runs, so such runs measure the host, not
    the review.
    """
    import re
    warned = False
    with HOST_GATE:
        while True:
            io = float(re.search(r"avg60=([\d.]+)", Path("/proc/pressure/io").read_text()).group(1))
            free = int(re.search(r"MemAvailable:\s+(\d+)", Path("/proc/meminfo").read_text()).group(1)) / 1e6
            if io <= max_io_percent and free >= min_free_gb:
                if warned:
                    log(f"host ready (I/O pressure {io:.0f}%, {free:.1f} GB free)", bench_log)
                return
            if not warned:
                log(f"HOST BUSY: I/O pressure {io:.0f}%, {free:.1f} GB free; holding new runs", bench_log)
                warned = True
            time.sleep(30)


def config_identity(config: Path) -> dict:
    return {name: hashlib.sha256((config / name).read_bytes()).hexdigest()
            for name in ("review.graph.json", "review.runtime.json")}


def cmd_run(args) -> None:
    exp = RUNS / args.exp
    exp.mkdir(parents=True, exist_ok=True)
    require_keys(*API_KEYS)
    config = HERE / "config"
    record = {"exp": args.exp, "config_sha256": config_identity(config),
              "image": args.image, "image_id": subprocess.run(["docker", "image", "inspect", "-f", "{{.Id}}", args.image],
                                                              capture_output=True, text=True).stdout.strip(),
              "keys": args.keys, "rounds": args.rounds, "judge_model": JUDGE_MODEL,
              "container": {"cpus": CONTAINER_CPUS, "memory": CONTAINER_MEMORY}}
    experiment = exp / "experiment.json"
    if experiment.exists():
        previous = json.loads(experiment.read_text())
        if previous["config_sha256"] != record["config_sha256"] and not args.force:
            sys.exit(f"{args.exp} was started with a different config; use a new experiment name")
        record["keys"] = sorted(set(previous.get("keys", [])) | set(args.keys))
        record["rounds"] = sorted(set(previous.get("rounds", [])) | set(args.rounds))
    experiment.write_text(json.dumps(record, indent=2))
    lookup = entries_by_key()
    jobs = [(r, lookup[k]) for r in args.rounds for k in args.keys]
    bench_log = exp / "bench.log"
    log(f"{args.exp}: {len(jobs)} runs, {args.parallel} at a time", bench_log)

    def work(job):
        round_no, entry = job
        wait_for_host(bench_log)
        log(f"start r{round_no} {key_of(entry)}", bench_log)
        try:
            result = run_pr(exp, round_no, entry, config, args.image, args.force)
        except Exception as error:
            result = {"key": key_of(entry), "status": "error", "problem": str(error)[:300]}
        log(f"end   r{round_no} {result['key']}: {result['status']} {result.get('findings')} findings "
            f"{result.get('seconds')}s source={result.get('source')} {result.get('problem') or ''}", bench_log)
        return result

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        results = list(pool.map(work, jobs))
    failed = [r for r in results if r["status"] != "ok"]
    log(f"{args.exp}: {len(results) - len(failed)} ok, {len(failed)} not ok", bench_log)


# --- judge -------------------------------------------------------------------


def cmd_judge(args) -> None:
    exp = RUNS / args.exp
    require_keys("ANTHROPIC_API_KEY")
    pi_dir = WORK / "judge-pi"
    pi_dir.mkdir(parents=True, exist_ok=True)
    # The judge's SDK sends budget-based thinking to a model it does not know unless the model is
    # marked adaptive; this model accepts only adaptive thinking on an API key.
    model = {"id": JUDGE_MODEL, "reasoning": True, "contextWindow": 1000000, "maxTokens": 64000,
             "compat": {"forceAdaptiveThinking": True}}
    (pi_dir / "models.json").write_text(json.dumps({"providers": {"anthropic": {
        "apiKey": "ANTHROPIC_API_KEY", "models": [model]}}}, indent=2))
    for round_no in args.rounds:
        round_dir = exp / f"round-{round_no}"
        output = round_dir / "scoring" / "results.json"
        if output.exists() and not args.force:
            log(f"r{round_no}: already judged")
            continue
        output.parent.mkdir(parents=True, exist_ok=True)
        command = ["npm", "run", "judge", "--", "--candidate", str(round_dir / "findings"),
                   "--provider", "anthropic", "--model", JUDGE_MODEL, "--output", str(output),
                   "--repo-dir", str(WORK / "judge-repos"), "--concurrency", str(args.concurrency)]
        # A failed pull request (for example a malformed matcher reply) is retried
        # from the judge's own checkpoint; finished pull requests are not re-judged.
        # If one keeps failing, the last attempt scores the rest and the round
        # records which pull requests are missing.
        attempts = 5
        for attempt in range(1, attempts + 1):
            strict = attempt < attempts
            env = dict(os.environ, PI_CODING_AGENT_DIR=str(pi_dir), REVIEW_BENCH_JUDGE_TIMEOUT_MS="2400000", TMPDIR=str(TMP))
            for name in ("ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_BASE_URL"):
                env.pop(name, None)
            log(f"r{round_no}: judging (attempt {attempt}{'' if strict else ', not strict'})")
            with (round_dir / "scoring" / f"judge-{attempt}.log").open("w") as handle:
                code = subprocess.run(command + ([] if strict else ["--no-strict"]), cwd=BENCH, env=env,
                                      stdout=handle, stderr=subprocess.STDOUT).returncode
            log(f"r{round_no}: judge exit {code}")
            if code == 0:
                if not strict:
                    scored = json.loads(output.read_text()).get("pr_count")
                    log(f"r{round_no}: NOT STRICT: scored {scored} of {len(list((round_dir / 'findings').glob('*.json')))} pull requests")
                break


# --- score -------------------------------------------------------------------


def f_beta(p, r, beta=1.0):
    if p is None or r is None or p + r == 0:
        return None
    return (1 + beta * beta) * p * r / (beta * beta * p + r)


def round_metrics(results: dict) -> dict:
    out = {}
    for agg in ("macro", "micro"):
        block = results[agg]
        views = {"all": block["overall"], "high": block["by_severity"].get("high")}
        for view, m in views.items():
            if not m:
                continue
            gp, gr, ap, ar = (m.get(k) for k in ("grounded_precision", "grounded_recall", "augmented_precision", "augmented_recall"))
            out[f"{agg}.{view}"] = {
                "grounded_precision": gp, "grounded_recall": gr, "grounded_f1": f_beta(gp, gr),
                "augmented_precision": ap, "augmented_recall": ar, "augmented_f1": f_beta(ap, ar),
                "golden_tp": m.get("golden_tp_count"), "covered": m.get("golden_covered_count"),
                "candidates": m.get("candidate_count"), "novel_tp": m.get("novel_tp_count"),
            }
    return out


def mean_sd(values):
    values = [v for v in values if v is not None]
    if not values:
        return None, None
    return statistics.mean(values), (statistics.stdev(values) if len(values) > 1 else 0.0)


def cmd_score(args) -> None:
    exp = RUNS / args.exp
    per_round, durations, counts, statuses = {}, [], [], {}
    for round_dir in sorted(exp.glob("round-*")):
        results = round_dir / "scoring" / "results.json"
        for done in round_dir.glob("prs/*/done.json"):
            d = json.loads(done.read_text())
            statuses[d["status"]] = statuses.get(d["status"], 0) + 1
            if d.get("seconds"):
                durations.append(d["seconds"])
            if d.get("findings") is not None:
                counts.append(d["findings"])
        if results.exists():
            data = json.loads(results.read_text())
            per_round[round_dir.name] = {"prs": data.get("pr_count"), **round_metrics(data)}
    summary = {"exp": args.exp, "rounds": per_round, "statuses": statuses,
               "seconds_per_pr": mean_sd(durations), "findings_per_pr": mean_sd(counts), "mean": {}}
    for view in sorted({v for r in per_round.values() for v in r if "." in v}):
        summary["mean"][view] = {
            metric: mean_sd([r[view].get(metric) for r in per_round.values() if view in r])
            for metric in ("grounded_precision", "grounded_recall", "grounded_f1",
                           "augmented_precision", "augmented_recall", "augmented_f1")
        }
    (exp / "summary.json").write_text(json.dumps(summary, indent=2))

    def pct(pair):
        m, s = pair if isinstance(pair, (list, tuple)) else (pair, None)
        return "n/a" if m is None else (f"{100 * m:.1f}" + (f" ± {100 * s:.1f}" if s else ""))

    lines = [f"# {args.exp}", "", f"Runs: {statuses}; findings/PR {summary['findings_per_pr'][0] or 0:.1f}; "
             f"seconds/PR {summary['seconds_per_pr'][0] or 0:.0f}", "",
             "| View | Grounded P | Grounded R | Grounded F1 | Aug P | Aug R | Aug F1 |", "|---|---|---|---|---|---|---|"]
    for view, metrics in summary["mean"].items():
        lines.append(f"| {view} | " + " | ".join(pct(metrics[m]) for m in (
            "grounded_precision", "grounded_recall", "grounded_f1", "augmented_precision", "augmented_recall", "augmented_f1")) + " |")
    (exp / "summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


# --- audit -------------------------------------------------------------------

SUSPICIOUS_TOOLS = ("websearch", "web_search", "webfetch", "web_fetch", "browser", "remotetrigger", "mcp__")
SUSPICIOUS_TEXT = ("api.github.com", "github.com/{nwo}/pull", "github.com/{nwo}/issues", "review-bench")


def cmd_audit(args) -> None:
    exp = RUNS / args.exp
    hosts: dict[tuple, int] = {}
    if PROXY_LOG.exists():
        for line in PROXY_LOG.read_text().splitlines():
            event = json.loads(line)
            if event.get("method"):
                k = (event["decision"], event.get("host") or event.get("target"))
                hosts[k] = hosts.get(k, 0) + 1
    lookup = entries_by_key()
    hits = []
    for transcript in list(exp.glob("round-*/prs/*/home/.claude/projects/**/*.jsonl")) + \
            list(exp.glob("round-*/prs/*/home/.codex/sessions/**/*.jsonl")):
        key = transcript.relative_to(exp).parts[2]
        nwo = lookup.get(key, {}).get("nwo", "")
        for number, line in enumerate(transcript.read_text(errors="replace").splitlines()):
            if not any(marker in line for marker in ('"custom_tool_call"', '"function_call"', '"tool_use"')):
                continue
            lowered = line.lower()
            for word in SUSPICIOUS_TOOLS:
                if f'"name":"{word}' in lowered.replace(" ", "") or f'"type":"{word}' in lowered.replace(" ", ""):
                    hits.append((str(transcript.relative_to(exp)), number, word))
            for pattern in SUSPICIOUS_TEXT:
                needle = pattern.format(nwo=nwo).lower()
                if nwo and needle in lowered:
                    hits.append((str(transcript.relative_to(exp)), number, needle))
    # Harness health: every agent must have been able to run commands.
    health = {"codex_sessions": 0, "codex_not_full_access": 0, "codex_sandbox_errors": 0,
              "claude_sessions": 0, "claude_permission_denials": 0}
    for transcript in exp.glob("round-*/prs/*/home/.codex/sessions/**/*.jsonl"):
        health["codex_sessions"] += 1
        text = transcript.read_text(errors="replace")
        if '"danger-full-access"' not in text:
            health["codex_not_full_access"] += 1
        if "bwrap: No permissions" in text:
            health["codex_sandbox_errors"] += 1
    for transcript in exp.glob("round-*/prs/*/home/.claude/projects/**/*.jsonl"):
        health["claude_sessions"] += 1
        text = transcript.read_text(errors="replace")
        health["claude_permission_denials"] += text.count("requested permissions") + text.count("haven't granted")
    report = {"proxy_hosts": [{"decision": d, "host": h, "count": c} for (d, h), c in sorted(hosts.items())],
              "health": health, "transcript_hits": hits[:200], "transcript_hit_count": len(hits)}
    (exp / "audit.json").write_text(json.dumps(report, indent=2))
    print(json.dumps({"proxy_hosts": report["proxy_hosts"], "health": health,
                      "transcript_hit_count": len(hits)}, indent=2))
    for hit in hits[:20]:
        print("HIT", *hit)


# --- eval (run, judge, score, audit) ------------------------------------------


def lock_experiment(name: str):
    """Refuse to run two harness processes on one experiment."""
    import fcntl
    (RUNS / name).mkdir(parents=True, exist_ok=True)
    handle = (RUNS / name / ".lock").open("w")
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit(f"{name} is already being run by another process")
    return handle


def cmd_eval(args) -> None:
    """Run each round, judging the previous round while the next one runs."""
    _lock = lock_experiment(args.exp)  # held until the process exits
    judges: list[threading.Thread] = []
    for round_no in args.rounds:
        run_args = argparse.Namespace(exp=args.exp, keys=args.keys, rounds=[round_no],
                                      parallel=args.parallel, image=args.image, force=False)
        cmd_run(run_args)
        judge_args = argparse.Namespace(exp=args.exp, rounds=[round_no], concurrency=args.judge_concurrency, force=False)
        thread = threading.Thread(target=cmd_judge, args=(judge_args,))
        thread.start()
        judges.append(thread)
    for thread in judges:
        thread.join()
    for command in (cmd_score, cmd_audit):
        command(argparse.Namespace(exp=args.exp))
    log(f"{args.exp}: EVAL DONE")


# --- repair --------------------------------------------------------------------

PROVIDER_ERRORS = ("at capacity", "rate limit", "overloaded", "Internal server error", "502", "503", "504",
                   "temporarily unavailable", "connection reset", "stream disconnected",
                   "cleanup could not be confirmed")
def infra_failures(exp: Path) -> list[tuple[int, str]]:
    """Runs whose graph failed because a model provider failed, not because of the review."""
    out = []
    for done in exp.glob("round-*/prs/*/done.json"):
        result = json.loads(done.read_text())
        if result.get("status") == "ok" and result.get("graph_status") == "succeeded":
            continue
        log_path = done.parent / "container.log"
        text = log_path.read_text(errors="replace") if log_path.exists() else ""
        if result.get("status") != "ok" or any(marker.lower() in text.lower() for marker in PROVIDER_ERRORS):
            out.append((int(done.parent.parent.parent.name.split("-")[1]), result["key"]))
    return sorted(out)


def cmd_repair(args) -> None:
    """Re-run provider-failed runs with the experiment's own config, then re-judge their rounds."""
    exp = RUNS / args.exp
    _lock = lock_experiment(args.exp)  # held until the process exits
    failures = infra_failures(exp)
    log(f"{args.exp}: {len(failures)} provider-failed runs: {failures}")
    if not failures:
        return
    config = HERE / "config"
    lookup = entries_by_key()
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
        results = list(pool.map(lambda job: run_pr(exp, job[0], lookup[job[1]], config, args.image, True), failures))
    for result in results:
        log(f"repaired {result['key']}: {result['status']} graph={result.get('graph_status')} {result.get('findings')} findings")
    with (exp / "repairs.jsonl").open("a") as handle:
        for (round_no, key), result in zip(failures, results):
            handle.write(json.dumps({"round": round_no, "key": key, "graph_status": result.get("graph_status")}) + "\n")
    rounds = sorted({round_no for round_no, _ in failures})
    for round_no in rounds:
        for stale in (exp / f"round-{round_no}" / "scoring").glob("results*"):
            stale.unlink()
    cmd_judge(argparse.Namespace(exp=args.exp, rounds=rounds, concurrency=args.judge_concurrency, force=True))
    for command in (cmd_score, cmd_audit):
        command(argparse.Namespace(exp=args.exp))


# --- main --------------------------------------------------------------------


def cmd_pick(args) -> None:
    excluded = {key_of(e) for e in manifest(args.exclude)} if args.exclude else set()
    keys = sorted(key_of(e) for e in manifest(args.set) if key_of(e) not in excluded)
    print(" ".join(random.Random(args.seed).sample(keys, args.count)))


def cmd_prepare(args) -> None:
    lookup = entries_by_key()
    keys = args.keys or [key_of(e) for e in manifest(args.set)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        for key, result in zip(keys, pool.map(lambda k: prepare(lookup[k]), keys)):
            meta = json.loads((result / "meta.json").read_text())
            log(f"prepared {key}: depth {meta['depth']}, {meta['repo_bytes'] // 1_000_000} MB, {meta['diff_lines']} diff lines")


def main() -> None:
    TMP.mkdir(parents=True, exist_ok=True)
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("pick")
    p.add_argument("--set", default="test", choices=["test", "full"])
    p.add_argument("--count", type=int, default=3)
    p.add_argument("--seed", type=int, default=SEED)
    p.add_argument("--exclude", choices=["test"], help="leave out this set's pull requests")
    p = sub.add_parser("prepare")
    p.add_argument("--set", default="test", choices=["test", "full"])
    p.add_argument("--keys", nargs="*")
    p = sub.add_parser("run")
    p.add_argument("--exp", required=True)
    p.add_argument("--keys", nargs="+", required=True)
    p.add_argument("--rounds", nargs="+", type=int, default=[1, 2, 3])
    p.add_argument("--parallel", type=int, default=3)
    p.add_argument("--image", default="rb-reviewer:dev")
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("judge")
    p.add_argument("--exp", required=True)
    p.add_argument("--rounds", nargs="+", type=int, default=[1, 2, 3])
    p.add_argument("--concurrency", type=int, default=4)
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("score")
    p.add_argument("--exp", required=True)
    p = sub.add_parser("audit")
    p.add_argument("--exp", required=True)
    p = sub.add_parser("repair")
    p.add_argument("--exp", required=True)
    p.add_argument("--parallel", type=int, default=4)
    p.add_argument("--judge-concurrency", type=int, default=4)
    p.add_argument("--image", default="rb-reviewer:dev")
    p = sub.add_parser("eval")
    p.add_argument("--exp", required=True)
    p.add_argument("--keys", nargs="+", required=True)
    p.add_argument("--rounds", nargs="+", type=int, default=[1, 2, 3])
    p.add_argument("--parallel", type=int, default=4)
    p.add_argument("--judge-concurrency", type=int, default=4)
    p.add_argument("--image", default="rb-reviewer:dev")
    args = parser.parse_args()
    {"pick": cmd_pick, "prepare": cmd_prepare, "run": cmd_run, "judge": cmd_judge,
     "score": cmd_score, "audit": cmd_audit, "eval": cmd_eval, "repair": cmd_repair}[args.command](args)


if __name__ == "__main__":
    main()

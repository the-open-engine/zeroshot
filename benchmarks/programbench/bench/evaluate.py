"""Official ProgramBench evaluation and leaderboard-identical scoring.

Everything that feeds a score is pinned: the eval image by digest, the hidden tests by Hugging
Face revision, ProgramBench by version, pytest-rerunfailures by version (bench/pbeval.py), and the
leaderboard's ignore list by commit.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from .config import Experiment, pins
from .util import download, log, read_json, sha256_file, without_secrets, write_json

# Eval error codes that are outcomes of the submission itself: its tree could not be committed,
# its compile.sh failed or timed out, or it produced no usable ./executable. The leaderboard scores
# them 0 on every test, and so do we.
# A test branch whose test run hit ProgramBench's time limit is also the submission's outcome: its
# tests kept running (chroma's did whenever pytest diffed a large mismatched output), no results
# file was written, and the branch's tests count as not passed, as on the leaderboard. So do the
# tests of a branch whose pytest could not load an installed plugin: the evaluation image's own
# packages fail before any test or conftest runs (dust: libtmux's plugin under pytest 9), the same
# for every submission.
# Any other error code, and any other test-branch error, is an evaluation infrastructure failure.
SUBMISSION_OUTCOMES = frozenset({"compile_failed", "copy_executable_failed", "hash_executable_failed", "no_executable_hash", "seed_git_failed"})


def infrastructure_error(score: dict[str, Any]) -> str | None:
    """Why an archive's evaluation cannot be trusted, or None."""
    branch_errors = score.get("test_branch_errors") or {}
    explained = score.get("missing_results_explained", score.get("test_runs_timed_out"))
    no_results = explained and all({_error_code(e) for e in errors} == {"results_read_failed"} for errors in branch_errors.values())
    if branch_errors and not no_results:
        return f"test branch errors {sorted(branch_errors)}"
    code = score.get("error_code")
    if code:
        return None if code in SUBMISSION_OUTCOMES else code
    if score.get("score") is not None and score.get("rerun_plugin_pinned") is False:
        return "pinned pytest-rerunfailures was not active"
    return None


def _error_code(error: Any) -> str | None:
    return error.get("error_code") if isinstance(error, dict) else str(error)


def _timed_out(entry: dict[str, Any]) -> bool:
    """A step that ProgramBench stopped at its time limit (``Command timed out after <n>s``)."""
    return entry.get("returncode") == -1 and "timed out after" in str(entry.get("exception_info") or "")


def _ran_tests(entry: dict[str, Any]) -> bool:
    """Whether a test run's output shows a pytest session (its header or plugin line) or a summary."""
    output = str(entry.get("output") or "")
    return any(mark in output for mark in ("test session starts", "plugins: ")) or re.search(r"\b\d+ (passed|failed|errors?)\b", output) is not None


def _plugin_load_failed(entry: dict[str, Any]) -> bool:
    """A test run in which pytest failed while loading an installed plugin, before any test ran."""
    return "load_setuptools_entrypoints" in str(entry.get("output") or "") and not _ran_tests(entry)


def _failed_reads_follow(raw: dict[str, Any], cause: Any) -> bool:
    entries = [e for e in raw.get("log") or [] if isinstance(e, dict)]
    failed_reads = [i for i, e in enumerate(entries) if e.get("step") == "results_read" and e.get("returncode") != 0]
    return bool(failed_reads) and all(i > 0 and entries[i - 1].get("step") == "run_tests" and cause(entries[i - 1]) for i in failed_reads)


def test_runs_timed_out(raw: dict[str, Any]) -> bool:
    """Every failed read of a branch's results file directly follows that branch's test run hitting
    ProgramBench's time limit (each attempt logs its steps together, in order)."""
    return _failed_reads_follow(raw, _timed_out)


def missing_results_explained(raw: dict[str, Any]) -> bool:
    """Every failed read of a results file follows a test run that the leaderboard also scores as
    tests not passed: one stopped at the time limit, or one whose pytest could not load a plugin."""
    return _failed_reads_follow(raw, lambda entry: _timed_out(entry) or _plugin_load_failed(entry))


def archive_id(path: Path) -> str:
    """Content hash of one archive: a re-run attempt writes different bytes under the same label,
    and unlike file metadata the hash survives copying the results elsewhere."""
    return sha256_file(path)


def rerun_plugin_active(raw: dict[str, Any]) -> bool | None:
    """Whether the pinned plugin was installed (exit 0) and loaded by every pytest that ran tests;
    None if no test run ran tests (a run stopped at ProgramBench's time limit keeps no output, and
    a branch can have no tests to run or fail before pytest starts its session)."""
    log_entries = [e for e in raw.get("log") or [] if isinstance(e, dict)]
    installs = [e for e in log_entries if e.get("step") == "install_rerunfailures"]
    runs = [e for e in log_entries if e.get("step") == "run_tests" and not _timed_out(e) and _ran_tests(e)]
    if not runs:
        return None
    installed = any(e.get("returncode") == 0 and "pytest-rerunfailures==16.4" in str(e.get("command")) for e in installs)
    return installed and all("rerunfailures-16.4" in str(e.get("output")) for e in runs)


def leaderboard_ignores(cache: Path) -> dict[str, list[str]]:
    pin = pins()["leaderboard_ignores"]
    return json.loads(download(pin["url"], pin["sha256"], cache / "downloads" / "ignored_tests.json").read_text())


def eval_image_tag(exp: Experiment) -> str:
    """``task_cleanroom_v6@sha256:...`` so programbench runs the exact image the agent used."""
    from programbench.constants import image_name_from_instance_id

    repository, _, tag_and_digest = exp.task_image.partition(":")
    if repository != image_name_from_instance_id(exp.instance_id) or "@sha256:" not in tag_and_digest:
        raise ValueError(f"task image {exp.task_image} does not match instance {exp.instance_id}")
    return tag_and_digest


def score_eval(eval_json: Path, instance_id: str, ignores: dict[str, list[str]]) -> dict[str, Any]:
    """Score exactly like the leaderboard: ProgramBench's active-branch/ignored-test filtering, then
    the registry's ignore map (``programbench.submission`` + ``compile_leaderboard.py``)."""
    from programbench.submission import (
        benchmark_instances,
        score_from_tests,
        test_results_map,
    )

    instance = benchmark_instances()[instance_id]
    tests = test_results_map(eval_json, instance)
    ignore = set(ignores.get(instance_id, []))
    kept = {name: passed for name, passed in tests.items() if name not in ignore}
    raw = json.loads(eval_json.read_text())
    entries = Counter(f"{t.get('branch')}/{t.get('name')}" for t in raw.get("test_results") or [])
    return {
        "score": score_from_tests(tests, ignore),
        "passed": sum(kept.values()),
        "scored_tests": len(kept),
        "duplicate_result_entries": sum(n - 1 for n in entries.values() if n > 1),
        "rerun_plugin_pinned": rerun_plugin_active(raw),
        "missing_results_explained": missing_results_explained(raw),
        "error_code": raw.get("error_code"),
        "error_details": str(raw.get("error_details") or "")[:2000] or None,
        "test_branch_errors": raw.get("test_branch_errors") or {},
        "test_runs_timed_out": test_runs_timed_out(raw),
        "executable_hash": raw.get("executable_hash"),
        "tests": kept,
    }


def targets(results: Path, rounds: list[int] | None = None) -> list[tuple[str, Path]]:
    """Every archive to score: each attempt's final workspace and its per-round snapshots, or,
    with a pre-registered schedule of ``rounds``, only the final and those rounds' builds (every
    snapshot is still archived and can be scored later)."""
    items = []
    for attempt in sorted((results / "attempts").glob("*")):
        if not attempt.is_dir() or "." in attempt.name:  # skip NN-arm.discarded-<ts>
            continue
        if (attempt / "submission.tar.gz").exists():
            items.append((f"{attempt.name}__final", attempt / "submission.tar.gz"))
        for snap in sorted((attempt / "snapshots").glob("*.tar.gz"), key=_snapshot_order):
            name = snap.name.removesuffix(".tar.gz")
            if rounds is None or name in {f"build-{r}" for r in rounds}:
                items.append((f"{attempt.name}__{name}", snap))
    return items


def _snapshot_order(path: Path) -> tuple[int, str]:
    """build-2 before build-10: order snapshots by their round number, then by name."""
    kind, _, rest = path.name.removesuffix(".tar.gz").partition("-")
    number = rest.split(".", 1)[0]
    return (int(number) if number.isdigit() else 0, kind + rest)


def content_key(archive: Path) -> str:
    """The workspace inside an archive, ignoring timestamps and owners: archives with the same
    files, contents, modes and link targets compile and test identically."""
    entries = []
    with tarfile.open(archive) as tar:
        for member in tar:
            name = member.name.removeprefix("./")
            if member.isfile():
                data = tar.extractfile(member)
                entries.append((name, "file", member.mode & 0o7777, hashlib.sha256(data.read()).hexdigest() if data else ""))
            elif member.issym() or member.islnk():
                entries.append((name, "link", member.mode & 0o7777, member.linkname))
            else:
                entries.append((name, member.type.decode(errors="replace"), member.mode & 0o7777, ""))
    return hashlib.sha256(json.dumps(sorted(entries)).encode()).hexdigest()


def _eval_json(exp: Experiment, results: Path, label: str) -> Path:
    return results / "evals" / label / exp.instance_id / f"{exp.instance_id}.eval.json"


def evaluate(exp: Experiment, results: Path, cache: Path, force: bool = False) -> dict[str, Any]:
    """Score every archive, evaluating each distinct workspace once: a check snapshot usually holds
    the same code as the build before it, and a final the same as the last snapshot."""
    evals = results / "evals"
    schedule = exp.raw["eval"].get("rounds")
    items = targets(results, schedule)
    groups: dict[str, list[str]] = defaultdict(list)
    for label, archive in items:
        instance_dir = evals / label / exp.instance_id
        instance_dir.mkdir(parents=True, exist_ok=True)
        link = instance_dir / "submission.tar.gz"
        if link.exists() and not os.path.samefile(archive, link):
            if archive_id(link) != archive_id(archive):  # the attempt was re-run: drop its old results
                for stale in instance_dir.glob("*.eval.json"):
                    stale.unlink()
            link.unlink()  # re-linked below; a copied results tree loses its hard links
        if not link.exists():
            os.link(archive, link)
        groups[content_key(archive)].append(label)
    # One representative per distinct workspace: one already evaluated if there is one.
    representative = {}
    for labels in groups.values():
        rep = labels[0] if force else next((label for label in labels if _eval_json(exp, results, label).exists()), labels[0])
        representative.update(dict.fromkeys(labels, rep))
    write_json(evals / "representatives.json", representative)
    reps = sorted(set(representative.values()))
    pending = [evals / rep for rep in reps if force or not _eval_json(exp, results, rep).exists()]
    run_record: dict[str, Any] = {"archives": len(items), "distinct": len(reps), "pending": len(pending)}
    if pending:
        run_record["returncode"] = _programbench_eval(exp, results, pending, force)
    _share_results(exp, results, representative, overwrite=force)
    ignores = leaderboard_ignores(cache)
    scores = _scores(exp, results, ignores, representative)
    # Evaluation is deterministic for a given workspace, so an infrastructure failure is retried
    # once; a failure that persists makes the archive's run ineligible (report._eligibility).
    retry = sorted({representative[label] for label, score in scores.items() if score.get("error_code") == "not_evaluated" or (score.get("score") is not None and infrastructure_error(score))})
    if retry:
        run_record["retried"] = {rep: infrastructure_error(scores[rep]) or scores[rep].get("error_code") for rep in retry}
        run_record["retry_returncode"] = _programbench_eval(exp, results, [evals / rep for rep in retry], force=True)
        _share_results(exp, results, representative, overwrite=True, only=set(retry))
        scores = _scores(exp, results, ignores, representative)
    write_json(results / "scores.json", {k: {kk: vv for kk, vv in v.items() if kk != "tests"} for k, v in scores.items()})
    write_json(results / "scores-per-test.json", {k: v.get("tests", {}) for k, v in scores.items()})
    history = results / "eval-runs.json"
    runs = read_json(history) if history.exists() else []
    write_json(history, [*runs, run_record])
    return scores


def _programbench_eval(exp: Experiment, results: Path, run_dirs: list[Path], force: bool) -> int | str:
    cfg = exp.raw["eval"]
    log(f"evaluating {len(run_dirs)} archive(s) with programbench eval")
    args = [
        sys.executable, "-m", "bench.pbeval", "eval", *map(str, run_dirs),
        "--workers", str(cfg["workers"]), "--docker-cpus", str(cfg["docker_cpus"]),
        "--image-tag", eval_image_tag(exp),
    ]
    if force:
        args.append("--force")
    env = without_secrets(dict(os.environ))
    env["PROGRAMBENCH_HF_REVISION"] = pins()["programbench_tests"]["revision"]
    with (results / "programbench-eval.log").open("ab") as out:
        try:
            returncode: int | str = subprocess.run(args, stdout=out, stderr=subprocess.STDOUT, env=env, check=False, timeout=int(cfg.get("timeout_seconds", 14400))).returncode
        except subprocess.TimeoutExpired:
            returncode = "timeout"
    log(f"programbench eval finished: {returncode}")
    return returncode


def _share_results(exp: Experiment, results: Path, representative: dict[str, str], overwrite: bool, only: set[str] | None = None) -> None:
    """Give every archive its representative's evaluation (identical workspaces)."""
    for label, rep in representative.items():
        if label == rep or (only is not None and rep not in only):
            continue
        source, target = _eval_json(exp, results, rep), _eval_json(exp, results, label)
        if source.exists() and (overwrite or not target.exists()):
            shutil.copyfile(source, target)


def _scores(exp: Experiment, results: Path, ignores: dict[str, list[str]], representative: dict[str, str] | None = None) -> dict[str, Any]:
    scores: dict[str, Any] = {}
    for label, archive in targets(results, exp.raw["eval"].get("rounds")):
        eval_json = _eval_json(exp, results, label)
        try:
            scores[label] = score_eval(eval_json, exp.instance_id, ignores) if eval_json.exists() else {"score": None, "error_code": "not_evaluated"}
        except ValueError as error:  # e.g. eval.json cut short when the evaluation was killed
            scores[label] = {"score": None, "error_code": "not_evaluated", "error_details": f"unreadable eval.json: {str(error)[:300]}"}
        scores[label]["archive_id"] = archive_id(archive)
        if representative and representative.get(label, label) != label:
            scores[label]["evaluated_as"] = representative[label]
    return scores


def load_scores(results: Path) -> dict[str, Any]:
    path = results / "scores.json"
    return read_json(path) if path.exists() else {}

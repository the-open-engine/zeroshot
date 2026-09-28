"""Summaries: per-attempt table, per-arm aggregates, and the pre-registered H1 decision."""

from __future__ import annotations

import math
import os
import statistics
import tarfile
from collections import Counter
from pathlib import Path
from typing import Any

from . import accounting, audit
from .config import Experiment, code_digest
from .evaluate import archive_id, infrastructure_error, load_scores
from .util import read_json, secret_values, write_json

REQUIRED_LOOP_RUNS = 5
# Build 1 must end normally, or at its node time limit (it used its full budget, as a single-arm
# build that hits the same limit does); a crash or malformed response cuts the baseline short.
BUILD_1_OUTCOMES = ("verified", "timeout")


def _surfaces(record: dict[str, Any], key: str) -> Any:
    """Home-surface probe results; attempts before the Claude harness recorded them as codex_*."""
    return record.get(key, record.get(f"codex_{key}"))


def home_changes(meta: dict[str, Any]) -> list[str]:
    """Instruction and hook files in the agent's home (~/.codex or ~/.claude) that differ from the
    attempt's start, at any snapshot or at the end."""
    if _surfaces(meta, "home_surfaces") is None:
        return ["not checked at the start"]
    start = set(_surfaces(meta, "home_surfaces"))
    snapshots = {label: s for label, s in (meta.get("snapshots") or {}).items() if isinstance(s, dict)}
    missing = [f"not checked after {label}" for label, s in snapshots.items() if _surfaces(s, "home_surfaces") is None]
    if _surfaces(meta, "home_surfaces_end") is None:
        missing.append("not checked at the end")
    seen = [*(_surfaces(s, "home_surfaces") or [] for s in snapshots.values()), _surfaces(meta, "home_surfaces_end") or []]
    return sorted({line for lines in seen for line in lines} - start) + missing


codex_home_changes = home_changes


def _pct(value: float | None) -> str:
    return "—" if value is None else f"{100 * value:.1f}%"


def _archived_codex_config(trajectories: Path) -> str | None:
    if not trajectories.exists():
        return None
    with tarfile.open(trajectories) as tar:
        for member in tar:
            if member.name.endswith(".codex/config.toml") and member.isfile():
                data = tar.extractfile(member)
                return data.read().decode() if data else None
    return None


def _eligibility(record: dict[str, Any], expected_tests: int | None) -> list[str]:
    """Reasons a loop run cannot enter the H1 analysis (empty = eligible)."""
    reasons = []
    if record["state"] != "complete":
        reasons.append(f"state {record['state']}")
    final, first = record["rounds"].get("final") or {}, record["rounds"].get("build-1") or {}
    for name, score in (("final", final), ("build-1", first)):
        if score.get("score") is None:
            reasons.append(f"{name} not scored")
        elif expected_tests and score.get("scored_tests") != expected_tests:
            reasons.append(f"{name} scored {score.get('scored_tests')} of {expected_tests} tests")
        if score.get("score") is not None and infrastructure_error(score):
            reasons.append(f"{name} evaluation failed: {infrastructure_error(score)}")
        if record.get("reference_sha256") and score.get("executable_hash") == record["reference_sha256"]:
            reasons.append(f"{name} built executable is the reference")
    first_build = (record.get("build_outcomes") or [None])[0]
    if first_build not in BUILD_1_OUTCOMES:
        reasons.append(f"build 1 ended {first_build}")
    if "error" in ((record.get("snapshots") or {}).get("build-1") or {}):
        reasons.append("build-1 snapshot failed")
    commands = record["commands"]
    for rule in audit.DISQUALIFYING:
        if (commands.get("rule_counts") or {}).get(rule):
            reasons.append(f"audit: {rule}")
    if commands.get("web_search_calls"):
        reasons.append("audit: web search")
    if commands.get("agents_md_loaded"):
        reasons.append("the agent loaded instructions (AGENTS.md or CLAUDE.md) left by a node")
    if record.get("home_changes"):
        reasons.append(f"a node left files for later agent sessions: {', '.join(record['home_changes'])[:200]}")
    if ((commands.get("rule_counts_by_round") or {}).get("build.round1") or {}).get("harness_internals"):
        reasons.append("audit: builder read harness internals in round 1")
    if record.get("reference_copies_in_final") or record.get("reference_copies_in_first_build"):
        reasons.append("a scored archive contains the reference binary")
    if record.get("codex_config_unchanged") is False:
        reasons.append("Codex config was modified during the run")
    if record.get("harness_unchanged") is not True:
        reasons.append("harness binaries were modified during the run" if record.get("harness_unchanged") is False else "harness integrity not checked")
    return reasons


def build(exp: Experiment, results: Path) -> dict[str, Any]:
    scores = load_scores(results)
    manifest = read_json(results / "manifest.json") if (results / "manifest.json").exists() else {}
    counts = Counter(v.get("scored_tests") for v in scores.values() if v.get("scored_tests"))
    expected_tests = counts.most_common(1)[0][0] if counts else None
    attempts = []
    for directory in sorted((results / "attempts").glob("*")):
        if not directory.is_dir() or "." in directory.name or not (directory / "attempt.json").exists():
            continue
        try:
            attempts.append(_attempt_record(exp, directory, scores, manifest, expected_tests))
        except Exception as error:  # one unreadable artifact must not sink the whole summary
            meta = read_json(directory / "attempt.json")
            attempts.append(_unreadable_record(meta, f"{type(error).__name__}: {error}"))
    summary = {
        "experiment": exp.id,
        "description": exp.raw.get("description"),
        "model": exp.model,
        "effort": exp.effort,
        "expected_scored_tests": expected_tests,
        "attempts": attempts,
        "discarded_attempts": _discarded(results, exp.pricing),
        "arms": _arms(attempts),
        "h1_decision": _decision(attempts, expected_tests, exp),
        "baseline_check": _baseline_check(attempts),
        "secrets": _secrets(results),
        "egress": _egress(exp, results),
        "provenance": manifest.get("provenance"),
        "scored_by": _scoring_code(),
    }
    write_json(results / "summary.json", summary)
    (results / "summary.md").write_text(markdown(summary))
    return summary


def _attempt_record(exp: Experiment, directory: Path, scores: dict[str, Any], manifest: dict[str, Any], expected_tests: int | None) -> dict[str, Any]:
    meta = read_json(directory / "attempt.json")
    label = meta["label"]
    rounds = {k.split("__", 1)[1]: v for k, v in scores.items() if k.startswith(f"{label}__")}
    for name, score in rounds.items():
        archive = directory / "submission.tar.gz" if name == "final" else directory / "snapshots" / f"{name}.tar.gz"
        if score.get("archive_id") and (not archive.exists() or archive_id(archive) != score["archive_id"]):
            rounds[name] = {"score": None, "error_code": "stale_score", "error_details": "scored a different archive; run `eval`"}
    usage = accounting.usage(directory)
    costs = {node: round(accounting.cost(t, exp.pricing), 4) for node, t in usage["nodes"].items()}
    first_build_cost = None if usage["first_build_round"] is None else round(accounting.cost(usage["first_build_round"], exp.pricing), 4)
    snapshots = {k: v for k, v in (meta.get("snapshots") or {}).items() if isinstance(v, dict)}
    snapshot_order = sorted(snapshots, key=lambda k: snapshots[k].get("started_at") or 0)
    archived_config = _archived_codex_config(directory / "trajectories.tar.gz")
    record = {
        "label": label,
        "arm": meta["arm"],
        "state": meta.get("state"),
        "error": meta.get("error"),
        "terminal": meta.get("terminal"),
        "force_stopped": meta.get("force_stopped"),
        "builds": meta.get("builds"),
        "checks": meta.get("checks"),
        "build_outcomes": [b.get("code") or b.get("status") for b in meta.get("build_outcomes", [])],
        "verdicts": [v.get("verdict") or f"{v.get('status')}:{v.get('code')}" for v in meta.get("verdicts", [])],
        "wall_seconds": meta.get("wall_seconds"),
        "rounds": rounds,
        "score_final": (rounds.get("final") or {}).get("score"),
        "score_first_build": (rounds.get("build-1") or {}).get("score"),
        "snapshots": snapshots,
        "tokens": usage,
        "cost_usd": costs,
        "cost_first_build_usd": first_build_cost,
        "commands": audit.command_audit(directory / "trajectories.tar.gz"),
        "checker_edits": audit.checker_edits(directory, snapshot_order),
        "reference_sha256": meta.get("reference_sha256"),
        "harness_unchanged": meta.get("harness_unchanged"),
        "home_changes": home_changes(meta),
        "reference_available_after_build_1": bool(snapshots["build-1"]["reference_at"]) if "reference_at" in snapshots.get("build-1", {}) else None,
        "reference_copies_in_final": audit.reference_copies(directory / "submission.tar.gz", meta.get("reference_sha256")),
        "reference_copies_in_first_build": audit.reference_copies(directory / "snapshots" / "build-1.tar.gz", meta.get("reference_sha256")),
        "codex_config_unchanged": None if archived_config is None or not manifest.get("codex_config") else archived_config == manifest["codex_config"],
        "gateway": _gateway_record(directory) if exp.harness == "claude" else None,
    }
    if record["gateway"]:
        costs["transcripts_total"] = costs.get("total")
        costs["total"] = record["gateway"]["cost_usd"]  # billed: includes requests no transcript records
    final, first = rounds.get("final") or {}, rounds.get("build-1") or {}
    if final.get("passed") is not None and first.get("passed") is not None:
        record["gain_tests"] = final["passed"] - first["passed"]
    record["ineligible_reasons"] = _eligibility(record, expected_tests) if record["arm"] == "loop" else []
    return record


def _egress(exp: Experiment, results: Path) -> dict[str, Any]:
    logs = "\n".join((d / "proxy.log").read_text() for d in sorted((results / "attempts").glob("*")) if (d / "proxy.log").exists())
    if exp.harness != "claude":
        return audit.proxy_audit(logs)
    summary = audit.gateway_audit(logs)
    summary.pop("request_ids")
    return summary


def _gateway_record(directory: Path) -> dict[str, Any] | None:
    """What the attempt's model gateway saw, and whether every model response in the transcripts
    went through it (a response the gateway never saw would mean another route to the API)."""
    log = directory / "proxy.log"
    if not log.exists():
        return None
    gateway = audit.gateway_audit(log.read_text())
    seen = set(gateway.pop("request_ids"))
    transcripts = {
        str(r["requestId"])
        for records in accounting.claude_transcripts(directory / "trajectories.tar.gz").values()
        for r in records
        if r.get("type") == "assistant" and r.get("requestId") and (r.get("message") or {}).get("model") != "<synthetic>"
    }
    gateway["transcript_requests_not_seen"] = sorted(transcripts - seen)
    gateway["requests_not_in_transcripts"] = len(seen - transcripts)
    return gateway


def _unreadable_record(meta: dict[str, Any], error: str) -> dict[str, Any]:
    """A minimal record for an attempt whose artifacts could not be read; never H1-eligible."""
    return {
        "label": meta.get("label"), "arm": meta.get("arm"), "state": meta.get("state"), "error": meta.get("error"),
        "report_error": error, "terminal": meta.get("terminal"), "force_stopped": meta.get("force_stopped"),
        "builds": meta.get("builds"), "checks": meta.get("checks"), "build_outcomes": [], "verdicts": [],
        "wall_seconds": meta.get("wall_seconds"), "rounds": {}, "score_final": None, "score_first_build": None,
        "snapshots": {}, "tokens": {"nodes": {}, "sessions": []}, "cost_usd": {}, "cost_first_build_usd": None,
        "commands": {}, "checker_edits": [], "reference_copies_in_final": [], "codex_config_unchanged": None,
        "ineligible_reasons": [f"artifacts unreadable: {error}"] if meta.get("arm") == "loop" else [],
    }


def _discarded(results: Path, pricing: dict[str, Any]) -> list[dict[str, Any]]:
    """Attempts replaced by a re-run (``NN-arm.discarded-<time>``): kept, reported, never scored."""
    out = []
    for directory in sorted((results / "attempts").glob("*.discarded-*")):
        path = directory / "attempt.json"
        meta = read_json(path) if path.exists() else {}
        out.append({
            "directory": directory.name,
            "label": meta.get("label"),
            "state": meta.get("state"),
            "error": meta.get("error"),
            "force_stopped": meta.get("force_stopped"),
            "wall_seconds": meta.get("wall_seconds"),
            "cost_usd": _total_cost(accounting.usage(directory), pricing),
        })
    return out


def _total_cost(usage: dict[str, Any], pricing: dict[str, Any]) -> float | None:
    """None when no transcript was recovered: unknown, not free."""
    total = usage["nodes"].get("total")
    return None if total is None else round(accounting.cost(total, pricing), 4)


def _scoring_code() -> dict[str, Any]:
    """The code that produced this summary, which can differ from the code that ran the attempts."""
    return {"vcs_ref": os.environ.get("ZSBENCH_VCS_REF", "unknown"), "vcs_dirty": os.environ.get("ZSBENCH_VCS_DIRTY", "unknown"), "code_digest": code_digest()}


def _secrets(results: Path) -> dict[str, Any]:
    """Scan with the key when it is available; otherwise keep the last scan that had it."""
    previous = results / "summary.json"
    if not secret_values() and previous.exists():
        last = (read_json(previous).get("secrets") or {})
        if last.get("checked_literal_key"):
            return {**last, "note": "carried over from the last scan made with the key"}
    return audit.secret_scan(results)


def _arms(attempts: list[dict[str, Any]]) -> dict[str, Any]:
    arms = {}
    for arm in sorted({a["arm"] for a in attempts}):
        group = [a for a in attempts if a["arm"] == arm]
        finals = [a["score_final"] for a in group if a["score_final"] is not None]
        costs = [a["cost_usd"]["total"] for a in group if "total" in a["cost_usd"]]
        arms[arm] = {
            "runs": len(group),
            "complete": sum(1 for a in group if a["state"] == "complete"),
            "scored": len(finals),
            "final_scores": finals,
            "median_final": statistics.median(finals) if finals else None,
            "mean_final": statistics.fmean(finals) if finals else None,
            "collapses_below_5pct": sum(1 for s in finals if s < 0.05),
            "costed": len(costs),
            "mean_cost_usd": statistics.fmean(costs) if costs else None,
            "total_cost_usd": round(sum(costs), 4),
        }
    return arms


def _decision(attempts: list[dict[str, Any]], expected_tests: int | None, exp: Experiment) -> dict[str, Any] | None:
    """Pre-registered H1 rule, in whole tests (N = scored tests per run)."""
    loops = [a for a in attempts if a["arm"] == "loop"]
    if not loops:
        return None
    eligible = [a for a in loops if not a["ineligible_reasons"]]
    gains = [a["gain_tests"] for a in eligible]
    result: dict[str, Any] = {
        "rule": exp.raw.get("decision_rule"),
        "eligible_runs": len(eligible),
        "required_runs": REQUIRED_LOOP_RUNS,
        "ineligible": {a["label"]: a["ineligible_reasons"] for a in loops if a["ineligible_reasons"]},
        "gains_tests": gains,
        "scored_tests": expected_tests,
    }
    if len(eligible) < REQUIRED_LOOP_RUNS or not expected_tests:
        result["verdict"] = f"inconclusive (only {len(eligible)} of {REQUIRED_LOOP_RUNS} loop runs eligible)"
        return result
    n = expected_tests
    median_gain = statistics.median(gains)
    result.update({
        "gains_pp": [round(100 * g / n, 2) for g in gains],
        "median_gain_pp": round(100 * median_gain / n, 2),
        "mean_gain_pp": round(100 * statistics.fmean(gains) / n, 2),
        "sign_test_one_sided_p": round(0.5 ** len(gains), 4) if all(g > 0 for g in gains) else None,
    })
    if all(g >= math.ceil(0.01 * n) for g in gains) and median_gain * 100 >= 5 * n:
        result["verdict"] = "supported"
    elif median_gain * 100 < 2 * n:
        result["verdict"] = "not supported"
    else:
        result["verdict"] = "inconclusive"
    return result


def baseline_lines(b: dict[str, Any]) -> list[str]:
    """Single-arm finals against the loop's first builds; an experiment without a single arm has none."""
    if not b["single_final_scores"]:
        return []
    return ["", f"Baseline check: single-arm finals mean {_pct(b['single_final_mean'])} vs loop first builds mean {_pct(b['loop_first_build_mean'])}."]


def _baseline_check(attempts: list[dict[str, Any]]) -> dict[str, Any]:
    """The loop's first build should look like a single-arm run (same prompt, same node)."""
    singles = [a["score_final"] for a in attempts if a["arm"] == "single" and a["score_final"] is not None]
    first_builds = [a["score_first_build"] for a in attempts if a["arm"] == "loop" and a["score_first_build"] is not None]
    return {
        "single_final_mean": statistics.fmean(singles) if singles else None,
        "loop_first_build_mean": statistics.fmean(first_builds) if first_builds else None,
        "single_final_scores": singles,
        "loop_first_build_scores": first_builds,
    }


def learning_curve(attempts: list[dict[str, Any]]) -> list[str]:
    """Hidden tests passed after each scored build round of every loop run (markdown table)."""
    loops = [a for a in attempts if a["arm"] == "loop"]
    numbers = sorted({int(k.split("-", 1)[1]) for a in loops for k in a["rounds"] if k.startswith("build-") and k.split("-", 1)[1].isdigit()})
    if len(numbers) < 2:
        return []
    lines = ["", "Hidden tests passed after each scored build round:", "", "| Run | " + " | ".join(f"R{n}" for n in numbers) + " | Final |", "|---|" + "---|" * (len(numbers) + 1)]
    for a in loops:
        cells = [str((a["rounds"].get(f"build-{n}") or {}).get("passed", "—")) for n in numbers]
        lines.append(f"| {a['label']} | " + " | ".join(cells) + f" | {(a['rounds'].get('final') or {}).get('passed', '—')} |")
    return lines


def markdown(summary: dict[str, Any]) -> str:
    lines = [f"# {summary['experiment']}", "", summary.get("description") or "", "", f"Model `{summary['model']}` at effort `{summary['effort']}`; {summary.get('expected_scored_tests')} scored hidden tests per run.", ""]
    lines += ["| Run | Arm | State | Rounds | Verdicts | First build | Final | Gain | Wall (min) | Cost (USD) | Flags |", "|---|---|---|---|---|---|---|---|---|---|---|"]
    for a in summary["attempts"]:
        flags = [k for k, v in (a["commands"].get("rule_counts") or {}).items() if v]
        if a["commands"].get("web_search_calls"):
            flags.append("web_search")
        if any(r["workspace_changes"] for r in a["checker_edits"]):
            flags.append("checker_edited_sources")
        if a.get("force_stopped") and (a.get("terminal") or {}).get("reason") == "force_stopped":
            flags.append(f"force-stopped ({a['force_stopped']})")
        if a.get("error"):
            flags.append(f"error: {a['error'][:80]}")
        if a.get("reference_sha256") and any((v or {}).get("executable_hash") == a["reference_sha256"] for v in a["rounds"].values()):
            flags.append("built executable is the reference")
        if a.get("harness_unchanged") is False:
            flags.append("harness modified")
        if a.get("home_changes") or a["commands"].get("agents_md_loaded"):
            flags.append("instructions left for later sessions")
        if (a.get("build_outcomes") or [None])[0] == "timeout":
            flags.append("build 1 timed out")
        if a.get("reference_available_after_build_1") is False:
            flags.append("reference gone after build 1")
        if any(s.get("usage_decreased") for s in a["tokens"].get("sessions", [])):
            flags.append("usage decreased in a thread")
        for name in ("build-1", "final"):
            score = (a["rounds"].get(name) or {})
            if score.get("error_code") or score.get("test_branch_errors"):
                flags.append(f"{name}: {infrastructure_error(score) or score.get('error_code') or 'test run timed out (its tests count as failed)'}")
        if a.get("ineligible_reasons"):
            flags.append("H1-ineligible")
        cost = a["cost_usd"].get("total")
        gain = a.get("gain_tests")
        n = summary.get("expected_scored_tests") or 0
        lines.append(
            f"| {a['label']} | {a['arm']} | {a['state']} | {a['builds'] or 0} | {', '.join(v or '—' for v in a['verdicts']) or '—'} | "
            f"{_pct(a['score_first_build'])} | {_pct(a['score_final'])} | {'—' if gain is None or not n else f'{gain:+d} tests ({100 * gain / n:+.1f} pp)'} | "
            f"{(a['wall_seconds'] or 0) / 60:.0f} | {'—' if cost is None else f'{cost:.2f}'} | {', '.join(flags) or '—'} |"
        )
    lines += ["", "| Arm | Runs | Complete | Scored | Median final | Mean final | Collapses | Mean cost |", "|---|---|---|---|---|---|---|---|"]
    for arm, s in summary["arms"].items():
        mean_cost = "—" if s["mean_cost_usd"] is None else f"${s['mean_cost_usd']:.2f} (n={s['costed']})"
        lines.append(f"| {arm} | {s['runs']} | {s['complete']} | {s['scored']} | {_pct(s['median_final'])} | {_pct(s['mean_final'])} | {s['collapses_below_5pct']} | {mean_cost} |")
    for d in summary.get("discarded_attempts") or []:
        cost = "unknown cost" if d["cost_usd"] is None else f"${d['cost_usd']:.2f}"
        lines.append(f"- Discarded and re-run: {d['directory']} ({d['state']}; {d.get('error') or d.get('force_stopped') or 'no error recorded'}; {cost})")
    d = summary.get("h1_decision")
    if d:
        lines += ["", f"**H1 (pre-registered): {d['verdict']}.** Eligible loop runs: {d['eligible_runs']}/{d['required_runs']}."]
        if "median_gain_pp" in d:
            lines.append(f"Per-run gains: {d['gains_pp']} pp; median {d['median_gain_pp']} pp; mean {d['mean_gain_pp']} pp; one-sided sign test p = {d['sign_test_one_sided_p']}.")
        for label, reasons in d["ineligible"].items():
            lines.append(f"- {label} ineligible: {'; '.join(reasons)}")
    lines += baseline_lines(summary["baseline_check"])
    lines += learning_curve(summary["attempts"])
    p, scored = summary.get("provenance") or {}, summary.get("scored_by") or {}
    lines += ["", f"Attempts ran at commit {p.get('vcs_ref', 'unknown')} (dirty={p.get('vcs_dirty', 'unknown')}); scored at {scored.get('vcs_ref', 'unknown')} (dirty={scored.get('vcs_dirty', 'unknown')})."]
    s = summary["secrets"]
    lines += ["", f"Secret scan: literal key checked={s['checked_literal_key']}, hits={len(s['literal_key_hits'])}{' — DO NOT PUBLISH' if s['literal_key_hits'] else ''}."]
    if summary.get("egress"):
        egress = summary["egress"]
        if "established" in egress:
            lines.append(f"Egress: connected {egress['established']}; refused {egress['refused']}.")
        else:
            lines.append(f"Model gateway (the only route out): {egress['requests']}; models {egress['models']}; refused {egress['refused'] or 'none'}; billed ${egress['cost_usd']}.")
    return "\n".join(lines) + "\n"

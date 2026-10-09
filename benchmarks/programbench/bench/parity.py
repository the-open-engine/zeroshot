"""Cost parity (each study experiment's ``decision_rule.cost_parity``): Luna's loop runs compared with
Sol's single workers on the same task at the single workers' mean model cost.

C is the mean cost of the Sol single workers (a single-arm experiment, or a loop experiment's first
builds: round 1 of a loop is the single worker's build node). A Luna loop run's parity workspace is the
workspace after the last build whose cumulative cost (builds 1..k and checks 1..k-1, from the Codex
transcripts at Luna's prices) does not exceed C; build 1 if build 1 alone costs more; the final
workspace if the run stops first. Parity workspaces are scored under ``parity/`` in the Luna results,
reusing an existing evaluation of an identical workspace, so the pre-registered schedule's scores and
summary stay as they are. Descriptive sensitivity: C at Sol's launch prices.

The prompt study (README) uses the same comparison with Luna's own single sessions in place of Sol's
single workers: C is then at Luna's prices, and there is no launch-price sensitivity.
"""

from __future__ import annotations

import hashlib
import random
import shutil
import statistics
import tarfile
from pathlib import Path
from typing import Any

from . import accounting, audit, evaluate
from .config import Experiment
from .util import log, read_json, write_json

RESAMPLES, SEED = 10_000, 20260924
# Sol's launch prices per million tokens; cache writes at 1.25 times input, as in the promotional prices.
LAUNCH_PRICES = {"usd_per_million_tokens": {"input": 5.0, "cached_input": 0.5, "cache_write": 6.25, "output": 30.0}}
LAUNCH_PRICED_MODEL = "gpt-5.6-sol"


def cumulative_costs(trajectories: Path, pricing: dict[str, Any]) -> list[float]:
    """Model cost of a loop run up to and including each build: builds 1..k and checks 1..k-1."""
    records = sorted(accounting.sessions(trajectories), key=lambda s: Path(s["file"]).name)
    builds = [r for s in records if s["node"] == "build" for r in s["rounds"]]
    checks = [r for s in records if s["node"] == "check" for r in s["rounds"]]
    total, out = 0.0, []
    for k, build in enumerate(builds):
        total += accounting.cost(build, pricing) + (accounting.cost(checks[k - 1], pricing) if 0 < k <= len(checks) else 0.0)
        out.append(total)
    return out


def parity_workspace(cumulative: list[float], budget: float) -> str:
    """The snapshot a loop run is compared through at ``budget`` (see the module docstring)."""
    if not cumulative or all(c <= budget for c in cumulative):
        return "final"
    within = [k + 1 for k, c in enumerate(cumulative) if c <= budget]
    return f"build-{within[-1] if within else 1}"


def _percentile(ordered: list[float], q: float) -> float:
    """Linear interpolation between closest ranks (numpy's default percentile)."""
    position = (len(ordered) - 1) * q / 100
    low = int(position)
    high = min(low + 1, len(ordered) - 1)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def bootstrap_interval(values: list[float]) -> tuple[float, float]:
    """95% percentile bootstrap interval of the mean, runs resampled as units (standard library only:
    the runner image does not install numpy)."""
    rng = random.Random(SEED)
    n = len(values)
    means = sorted(sum(values[rng.randrange(n)] for _ in range(n)) / n for _ in range(RESAMPLES))
    return _percentile(means, 2.5), _percentile(means, 97.5)


def verdict(luna: list[float], sol: list[float], names: tuple[str, str] = ("Luna", "Sol")) -> dict[str, Any]:
    luna_ci, sol_ci = bootstrap_interval(luna), bootstrap_interval(sol)
    luna_mean, sol_mean = statistics.fmean(luna), statistics.fmean(sol)
    if luna_mean > sol_mean:
        text = f"{names[0]} outperforms robustly (intervals do not overlap)" if luna_ci[0] > sol_ci[1] else f"{names[0]} outperforms (higher mean; intervals overlap)"
    elif sol_mean > luna_mean:
        text = f"{names[1]} is ahead robustly (intervals do not overlap)" if sol_ci[0] > luna_ci[1] else f"{names[1]} is ahead (higher mean; intervals overlap)"
    else:
        text = "tie"
    return {"luna_mean": luna_mean, "luna_ci": luna_ci, "sol_mean": sol_mean, "sol_ci": sol_ci, "verdict": text}


def _disqualified(record: dict[str, Any]) -> list[str]:
    """The rule's run-level conditions that the summary records."""
    reasons = []
    if record.get("state") != "complete":
        reasons.append(f"state {record.get('state')}")
    commands = record.get("commands") or {}
    for rule in audit.DISQUALIFYING:
        if (commands.get("rule_counts") or {}).get(rule):
            reasons.append(f"audit: {rule}")
    if commands.get("web_search_calls"):
        reasons.append("audit: web search")
    if record.get("harness_unchanged") is not True:
        reasons.append("harness files changed or unchecked")
    if record.get("codex_config_unchanged") is False:
        reasons.append("Codex config changed")
    return reasons


def _score_problem(score: dict[str, Any], scored_tests: int | None, reference_sha256: str | None) -> str | None:
    if score.get("score") is None:
        return "not scored"
    if scored_tests and score.get("scored_tests") != scored_tests:
        return f"scored {score.get('scored_tests')} of {scored_tests} tests"
    if evaluate.infrastructure_error(score):
        return f"evaluation failed: {evaluate.infrastructure_error(score)}"
    if reference_sha256 and score.get("executable_hash") == reference_sha256:
        return "the workspace builds the reference executable"
    return None


def _contains(archive: Path, digest: str | None) -> bool:
    if not digest:
        return False
    with tarfile.open(archive) as tar:
        for member in tar:
            data = tar.extractfile(member) if member.isfile() else None
            if data is not None and hashlib.sha256(data.read()).hexdigest() == digest:
                return True
    return False


def sol_runs(results: Path) -> list[dict[str, Any]]:
    """Sol's single workers: a single-arm experiment's runs, or a loop experiment's first builds."""
    summary = read_json(results / "summary.json")
    singles = [a for a in summary["attempts"] if a["arm"] == "single"]
    use_first_builds = not singles
    runs = []
    for a in singles or [a for a in summary["attempts"] if a["arm"] == "loop"]:
        label = "build-1" if use_first_builds else "final"
        tokens = (a.get("tokens") or {}).get("first_build_round") if use_first_builds else ((a.get("tokens") or {}).get("nodes") or {}).get("total")
        score = (a.get("rounds") or {}).get(label) or {}
        cost = a.get("cost_first_build_usd") if use_first_builds else (a.get("cost_usd") or {}).get("total")
        problems = _disqualified(a)
        problem = _score_problem(score, summary.get("expected_scored_tests"), a.get("reference_sha256"))
        if problem:
            problems.append(problem)
        if a.get("reference_copies_in_first_build") if use_first_builds else a.get("reference_copies_in_final"):
            problems.append("the workspace contains the reference executable")
        runs.append({
            "run": a["label"], "workspace": label, "passed": score.get("passed"), "scored_tests": score.get("scored_tests"),
            "cost_usd": cost, "cost_launch_usd": accounting.cost(tokens, LAUNCH_PRICES) if tokens else None, "excluded": problems,
        })
    return runs


def _parity_eval(luna: Experiment, results: Path, cache: Path, wanted: dict[str, Path]) -> dict[str, dict[str, Any]]:
    """Score each wanted archive (label -> archive) under parity/, reusing identical evaluated workspaces."""
    evaluated: dict[str, Path] = {}
    for label in sorted((results / "evals").glob("*")):
        eval_json = label / luna.instance_id / f"{luna.instance_id}.eval.json"
        link = label / luna.instance_id / "submission.tar.gz"
        if eval_json.exists() and link.exists():
            evaluated.setdefault(evaluate.content_key(link), eval_json)
    root = results / "parity" / "evals"
    pending = []
    for label, archive in wanted.items():
        instance_dir = root / label / luna.instance_id
        instance_dir.mkdir(parents=True, exist_ok=True)
        link = instance_dir / "submission.tar.gz"
        if not link.exists():
            shutil.copyfile(archive, link)
        target = instance_dir / f"{luna.instance_id}.eval.json"
        if target.exists():
            continue
        same = evaluated.get(evaluate.content_key(link))
        if same is not None:
            shutil.copyfile(same, target)
        else:
            pending.append(root / label)
    if pending:
        evaluate._programbench_eval(luna, results, pending, force=False)
    ignores = evaluate.leaderboard_ignores(cache)
    scores = {}
    for label in wanted:
        target = root / label / luna.instance_id / f"{luna.instance_id}.eval.json"
        try:
            scores[label] = evaluate.score_eval(target, luna.instance_id, ignores) if target.exists() else {"score": None, "error_code": "not_evaluated"}
        except ValueError as error:
            scores[label] = {"score": None, "error_code": "not_evaluated", "error_details": str(error)[:300]}
        scores[label].pop("tests", None)
    return scores


def compare(luna: Experiment, sol: Experiment, luna_results: Path, sol_results: Path, cache: Path) -> dict[str, Any]:
    if luna.instance_id != sol.instance_id:
        raise ValueError(f"{luna.id} and {sol.id} are different tasks")
    sol_list = sol_runs(sol_results)
    included = [r for r in sol_list if not r["excluded"]]
    if not included:
        raise ValueError(f"no Sol single worker of {sol.id} enters the comparison")
    budgets = {"promotional": statistics.fmean(r["cost_usd"] for r in included)}
    same_model = luna.model == sol.model  # the prompt study: Luna's loop against Luna's single sessions
    names = ("The loop", "The single session") if same_model else ("Luna", "Sol")
    if sol.model == LAUNCH_PRICED_MODEL and all(r["cost_launch_usd"] is not None for r in included):
        budgets["launch"] = statistics.fmean(r["cost_launch_usd"] for r in included)
    summary = read_json(luna_results / "summary.json")
    loops = [a for a in summary["attempts"] if a["arm"] == "loop"]
    plans, wanted = {}, {}
    for a in loops:
        attempt_dir = luna_results / "attempts" / a["label"]
        cumulative = cumulative_costs(attempt_dir / "trajectories.tar.gz", luna.pricing)
        plans[a["label"]] = {"cumulative_usd": [round(c, 4) for c in cumulative]}
        for name, budget in budgets.items():
            workspace = parity_workspace(cumulative, budget)
            archive = attempt_dir / ("submission.tar.gz" if workspace == "final" else f"snapshots/{workspace}.tar.gz")
            plans[a["label"]][name] = workspace
            wanted[f"{a['label']}__{workspace}"] = archive
    log(f"[parity] {luna.id} at C=${budgets['promotional']:.2f} from {len(included)} Sol single workers of {sol.id}")
    scores = _parity_eval(luna, luna_results, cache, wanted)
    expected = summary.get("expected_scored_tests")
    outcome: dict[str, Any] = {"luna": luna.id, "sol": sol.id, "task": luna.instance_id, "scored_tests": expected, "sol_runs": sol_list,
                               "budgets_usd": budgets, "launch_prices": LAUNCH_PRICES["usd_per_million_tokens"], "luna_runs": [], "comparisons": {},
                               "labels": ["Loop", "Single session"] if same_model else ["Luna loop", "Sol single worker"]}
    for a in loops:
        plan = plans[a["label"]]
        run: dict[str, Any] = {"run": a["label"], "cumulative_usd": plan["cumulative_usd"], "excluded": _disqualified(a)}
        for name in budgets:
            label = f"{a['label']}__{plan[name]}"
            score, problems = scores[label], []
            problem = _score_problem(score, expected, a.get("reference_sha256"))
            if problem:
                problems.append(problem)
            if _contains(wanted[label], a.get("reference_sha256")):
                problems.append("the workspace contains the reference executable")
            k = len(plan["cumulative_usd"]) if plan[name] == "final" else int(plan[name].split("-")[1])
            spent = plan["cumulative_usd"][k - 1] if plan["cumulative_usd"] else None
            run[name] = {"workspace": plan[name], "rounds": k, "cost_usd": spent, "passed": score.get("passed"), "excluded": problems}
        outcome["luna_runs"].append(run)
    sol_pct = [100 * r["passed"] / expected for r in included]
    for name in budgets:
        luna_pct = [100 * r[name]["passed"] / expected for r in outcome["luna_runs"] if not r["excluded"] and not r[name]["excluded"]]
        outcome["comparisons"][name] = {"budget_usd": budgets[name], "luna_runs": len(luna_pct), "sol_runs": len(sol_pct),
                                        **(verdict(luna_pct, sol_pct, names) if luna_pct else {"verdict": "no eligible loop runs"})}
    write_json(luna_results / "parity" / f"{sol.id}.json", outcome)
    (luna_results / "parity" / f"{sol.id}.md").write_text(render(outcome))
    return outcome


def render(o: dict[str, Any]) -> str:
    loop, single = o.get("labels", ["Luna loop", "Sol single worker"])
    lines = [f"# Cost parity: {o['luna']} against {o['sol']}", "", f"Task {o['task']}, {o['scored_tests']} scored tests.", ""]
    for name, c in o["comparisons"].items():
        lines.append(f"## At {name} prices: C = ${c['budget_usd']:.2f} per run")
        if "luna_mean" in c:
            lines += ["", f"- {loop} at parity: {c['luna_mean']:.1f}% (95% CI {c['luna_ci'][0]:.1f} to {c['luna_ci'][1]:.1f}), {c['luna_runs']} runs",
                      f"- {single}: {c['sol_mean']:.1f}% (95% CI {c['sol_ci'][0]:.1f} to {c['sol_ci'][1]:.1f}), {c['sol_runs']} runs",
                      f"- {c['verdict']}", ""]
        else:
            lines += ["", f"- {c['verdict']}", ""]
    lines += ["| Single run | Workspace | Passed | Cost | Excluded |", "|---|---|---|---|---|"]
    for r in o["sol_runs"]:
        cost = "-" if r["cost_usd"] is None else f"${r['cost_usd']:.2f}"
        lines.append(f"| {r['run']} | {r['workspace']} | {r['passed']} | {cost} | {', '.join(r['excluded']) or '-'} |")
    lines += ["", "| Loop run | Budget | Workspace | Passed | Cost to there | Excluded |", "|---|---|---|---|---|---|"]
    for r in o["luna_runs"]:
        for name in o["comparisons"]:
            p = r[name]
            cost = "-" if p["cost_usd"] is None else f"${p['cost_usd']:.2f}"
            lines.append(f"| {r['run']} | {name} | {p['workspace']} | {p['passed']} | {cost} | {', '.join(r['excluded'] + p['excluded']) or '-'} |")
    return "\n".join(lines) + "\n"

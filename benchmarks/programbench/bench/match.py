"""Pinned performance, the dual of the cost-parity comparison: the model cost at which GPT-5.6 Luna's
loop runs on a task first reach, on average, the mean score of GPT-5.6 Sol's single workers.

At a budget x each loop run delivers its last build whose cumulative cost (builds 1..k and checks
1..k-1, as in parity.py) is within x; before build 1 it delivers nothing and scores 0. The cost to
match is the smallest x at which the mean score of the eligible runs' deliverables reaches Sol's mean
score; budgets are searched where some run completes a build. Builds are scored only as far as the
search needs, in blocks, under parity/ (reusing identical evaluated workspaces), after the parity
comparison has fixed the eligible runs, Sol's mean score and Sol's mean cost.
"""

from __future__ import annotations

import statistics
from pathlib import Path
from typing import Any

from . import parity
from .config import Experiment
from .util import log, read_json, write_json

# Builds are scored up to these rounds, one block at a time, until the search no longer needs more.
BLOCKS = (4, 8, 14, 22, 32, 50)


def first_match(runs: dict[str, list[float]], scores: dict[tuple[str, int], float], target: float) -> tuple[str, float, Any]:
    """Search budgets in increasing order. ("reached", budget, deliverables) at the first budget whose
    deliverables' mean score reaches ``target``; ("unknown", budget, missing builds) at the first budget
    with an unscored deliverable; ("never", last budget, deliverables) if no budget reaches it."""
    budgets = sorted({c for costs in runs.values() for c in costs})
    deliverables: dict[str, int] = {}
    for budget in budgets:
        deliverables = {run: sum(1 for c in costs if c <= budget) for run, costs in runs.items()}
        missing = sorted((run, k) for run, k in deliverables.items() if k >= 1 and (run, k) not in scores)
        if missing:
            return "unknown", budget, missing
        if statistics.fmean(scores[(run, k)] if k >= 1 else 0.0 for run, k in deliverables.items()) >= target:
            return "reached", budget, deliverables
    return "never", budgets[-1] if budgets else 0.0, deliverables


def _label(run: str, k: int, builds: int) -> str:
    return f"{run}__final" if k == builds else f"{run}__build-{k}"


def match(luna: Experiment, sol: Experiment, luna_results: Path, cache: Path) -> dict[str, Any]:
    comparison = read_json(luna_results / "parity" / f"{sol.id}.json")
    expected = comparison["scored_tests"]
    sol_runs = [r for r in comparison["sol_runs"] if not r["excluded"]]
    target = statistics.fmean(100 * r["passed"] / expected for r in sol_runs)
    sol_cost = statistics.fmean(r["cost_usd"] for r in sol_runs)
    runs = {r["run"]: r["cumulative_usd"] for r in comparison["luna_runs"] if not r["excluded"] and r["cumulative_usd"]}
    scores: dict[tuple[str, int], float] = {}
    measured = read_json(luna_results / "scores.json")
    for run, costs in runs.items():
        for k in range(1, len(costs) + 1):
            s = measured.get(f"{run}__{'final' if k == len(costs) else f'build-{k}'}")
            if s and s.get("passed") is not None:
                scores[(run, k)] = 100 * s["passed"] / expected
    for r in comparison["luna_runs"]:
        for name in comparison["comparisons"]:
            p = r[name]
            if r["run"] in runs and p.get("passed") is not None:
                scores.setdefault((r["run"], p["rounds"]), 100 * p["passed"] / expected)
    blocks_scored = []
    while True:
        kind, budget, info = first_match(runs, scores, target)
        if kind != "unknown":
            break
        need = max(k for _, k in info)
        limit = next((b for b in BLOCKS if b >= need), max(len(c) for c in runs.values()))
        wanted = {}
        for run, costs in runs.items():
            for k in range(1, min(limit, len(costs)) + 1):
                if (run, k) not in scores:
                    archive = luna_results / "attempts" / run / ("submission.tar.gz" if k == len(costs) else f"snapshots/build-{k}.tar.gz")
                    if archive.exists():
                        wanted[_label(run, k, len(costs))] = archive
        if not wanted:
            raise RuntimeError(f"{luna.id}: builds {info[:3]} are needed but have no archive")
        log(f"[match] {luna.id}: scoring {len(wanted)} builds up to round {limit} (Sol's mean {target:.1f}%)")
        got = parity._parity_eval(luna, luna_results, cache, wanted)
        blocks_scored.append({"up_to_round": limit, "archives": len(wanted)})
        for run, costs in runs.items():
            for k in range(1, min(limit, len(costs)) + 1):
                s = got.get(_label(run, k, len(costs)))
                if s is not None and s.get("passed") is not None:
                    scores[(run, k)] = 100 * s["passed"] / expected
                elif s is not None:
                    scores[(run, k)] = 0.0  # not scorable: no usable executable, as on the leaderboard
    outcome = {
        "luna": luna.id, "sol": sol.id, "task": luna.instance_id, "scored_tests": expected,
        "sol_mean_pct": target, "sol_mean_cost_usd": sol_cost, "outcome": kind,
        "luna_cost_to_match_usd": budget if kind == "reached" else None,
        "luna_spent_without_match_usd": budget if kind == "never" else None,
        "cost_ratio": budget / sol_cost if kind == "reached" else None,
        "deliverables": info if kind != "unknown" else None,
        "blocks_scored": blocks_scored,
        "runs": {run: {"cumulative_usd": costs, "pass_pct": {str(k): round(v, 3) for (r, k), v in sorted(scores.items()) if r == run}} for run, costs in runs.items()},
    }
    write_json(luna_results / "parity" / f"{sol.id}.match.json", outcome)
    return outcome

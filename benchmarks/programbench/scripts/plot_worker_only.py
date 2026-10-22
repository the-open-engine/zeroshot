"""Worker-only study: the loop without its checker, told "Continue." (figures/worker-loop/) or told to
review its own work (figures/self-review/), against Sol's single shot, Luna's single shot and the review
loop, all through the same Sol single workers (figures/cost-parity/ and figures/prompt-study/ for the loop
and the single shot), from the JSONs that `bench parity` and `bench match` write.

    python3 scripts/plot_worker_only.py figures/worker-loop/*.json figures/self-review/*.json [--out-dir DIR]

Writes to figures/ (or DIR):
- worker-only-equal-cost.png: per task, the five arms at Sol's single-shot cost (means of 5 runs with
  95% bootstrap intervals; rows by the self-review loop's lead over the review loop);
- worker-only-equal-score.png: per task, each loop's cost to reach Sol's mean score as a share of
  Sol's cost.

Needs matplotlib; fonts as in plotstyle.py.
"""
import json
import math
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker
from matplotlib import font_manager
from matplotlib.lines import Line2D

import plot_parity
from bench.parity import bootstrap_interval
from plotstyle import CANVAS, HAIRLINE, INK, INK_2, MUTED, RUST, load_fonts

LIGHT_RUST = "#DB8373"
SUFFIXES = {"worker-loop": "-worker-loop", "self-review": "-self-review"}
SERIES = {  # key -> (legend, color, marker)
    "sol": ("GPT-5.6 Sol (xhigh), single shot", INK, "o"),
    "single": ("GPT-5.6 Luna (xhigh), single shot", LIGHT_RUST, "o"),
    "worker-loop": ("GPT-5.6 Luna (xhigh), builder alone told \"Continue.\", at Sol's cost", MUTED, "s"),
    "self-review": ("GPT-5.6 Luna (xhigh), builder alone told to review its work, at Sol's cost", INK_2, "^"),
    "loop": ("GPT-5.6 Luna (xhigh), review loop at Sol's cost", RUST, "D"),
}


def task_of(o: dict) -> str:
    task = o["luna"].removeprefix("luna-xhigh-")
    for suffix in (*SUFFIXES.values(), "-v1"):
        task = task.removesuffix(suffix)
    return task


def rows_from(paths: list[Path], figures: Path) -> list[dict]:
    """One row per task, from the worker-loop and self-review comparison files of that task."""
    by_task: dict[str, dict] = {}
    for path in paths:
        o = json.loads(path.read_text())
        variant = next(k for k, s in SUFFIXES.items() if o["luna"].endswith(s))
        by_task.setdefault(task_of(o), {})[variant] = (o, json.loads(path.with_name(path.name.removesuffix(".json") + ".match.json").read_text()))
    rows = []
    for task, variants in sorted(by_task.items()):
        if set(variants) != set(SUFFIXES):
            raise ValueError(f"{task}: both worker-only variants are needed, found {sorted(variants)}")
        loop = json.loads((figures / "cost-parity" / f"luna-xhigh-{task}-v1.json").read_text())
        loop_match = json.loads((figures / "cost-parity" / f"luna-xhigh-{task}-v1.match.json").read_text())
        shots = json.loads((figures / "prompt-study" / f"luna-xhigh-{task}-v1.match.json").read_text())
        single = [r["pass_pct"]["1"] for r in shots["runs"].values()]
        c = loop["comparisons"]["promotional"]
        row = {"task": task, "budget": c["budget_usd"], "sol": (c["sol_mean"], c["sol_ci"]), "loop": (c["luna_mean"], c["luna_ci"]),
               "single": (statistics.fmean(single), bootstrap_interval(single)),
               "loop_ratio": loop_match["cost_ratio"] if loop_match["outcome"] == "reached" else math.inf}
        for variant, (parity, match) in variants.items():
            if parity["sol"] != loop["sol"]:
                raise ValueError(f"{task}: {variant} compared with {parity['sol']}, the loop with {loop['sol']}")
            v = parity["comparisons"]["promotional"]
            row[variant] = (v["luna_mean"], v["luna_ci"])
            row[f"{variant}_ratio"] = match["cost_ratio"] if match["outcome"] == "reached" else math.inf
            runs = [r["promotional"] for r in parity["luna_runs"] if not r["excluded"] and not r["promotional"]["excluded"]]
            row[f"{variant}_rounds"] = statistics.fmean(r["rounds"] for r in runs)
            row[f"{variant}_spent"] = statistics.fmean(r["cost_usd"] for r in runs)
        rows.append(row)
    rows.sort(key=lambda r: -(r["self-review"][0] - r["loop"][0]))
    return rows


def equal_cost(rows: list[dict], out: Path) -> Path:
    n = len(rows)
    wins = {key: sum(r[key][0] > r["sol"][0] for r in rows) for key in ("single", "worker-loop", "self-review", "loop")}
    above_single = {key: sum(r[key][0] > r["single"][0] for r in rows) for key in ("worker-loop", "self-review")}
    title = (f"Without a checker, the builder beats Sol's single shot on {wins['self-review']} of {n}\nsampled ProgramBench tasks when told to review its work\n"
             f"and on {wins['worker-loop']} when told \"Continue.\"; the review loop on {wins['loop']}")
    descriptions = dict(plot_parity.TASKS)
    keys = list(SERIES)
    spacing = 0.17
    fig, ax = plt.subplots(figsize=(9.6, 1.05 * n + 2.8), dpi=200)
    offsets = {key: spacing * ((len(keys) - 1) / 2 - i) for i, key in enumerate(keys)}
    for i, r in enumerate(rows):
        y = n - 1 - i
        for key in keys:
            (mean, ci), (_, color, marker), dy = r[key], SERIES[key], offsets[key]
            ax.plot(ci, [y + dy] * 2, color=color, lw=2.2, solid_capstyle="round", alpha=0.85, zorder=2)
            ax.scatter([mean], [y + dy], s=46, color=color, marker=marker, edgecolor=CANVAS, linewidth=1.1, zorder=3)
            ax.annotate(f"{mean:.1f}%", (ci[1], y + dy), xytext=(6, 0), textcoords="offset points", va="center", fontsize=7.8, color=color, fontweight="medium")
    ax.set_yticks(range(n))
    ax.set_yticklabels([f"{r['task']}\n{descriptions.get(r['task'], '')}" for r in reversed(rows)], fontsize=9.5, linespacing=1.3)
    ax.set_ylim(-0.6, n - 0.4)
    ax.set_xlim(0, 100)
    ax.xaxis.set_major_locator(matplotlib.ticker.MultipleLocator(10))
    ax.xaxis.set_major_formatter(matplotlib.ticker.FormatStrFormatter("%d%%"))
    ax.set_xlabel("Hidden tests passed (mean of 5 runs, 95% bootstrap interval)", labelpad=8)
    ax.spines[["top", "right", "left"]].set_visible(False)
    ax.tick_params(axis="y", length=0)
    ax.grid(axis="x", color=HAIRLINE, lw=0.9)
    ax.set_axisbelow(True)
    for label in ax.get_xticklabels():
        label.set_fontproperties(font_manager.FontProperties(family="DejaVu Sans Mono", size=9))
    handles = [Line2D([], [], color=color, marker=marker, lw=2.2, ms=6.5, mec=CANVAS, label=legend) for legend, color, marker in SERIES.values()]
    ax.legend(handles=handles, loc="lower left", frameon=False, fontsize=9.0, bbox_to_anchor=(0.0, 1.0), ncol=1, handlelength=2.2)
    ax.set_title(title, loc="left", fontfamily=["Fraunces", "DejaVu Serif"], fontweight="semibold", fontsize=15.5, color=INK, pad=100, linespacing=1.15)
    fig.tight_layout()
    path = out / "worker-only-equal-cost.png"
    fig.savefig(path)
    print(f"above Luna's single shot: told Continue. on {above_single['worker-loop']} of {n} tasks, told to review on {above_single['self-review']}")
    return path


def _median_ratio(ratios: list[float]) -> str:
    median = statistics.median(ratios)
    return "not reached" if math.isinf(median) else f"{100 * median:.0f}%"


def equal_score(rows: list[dict], out: Path) -> Path:
    n = len(rows)
    bars = (("loop_ratio", 0.27, RUST, "review loop"), ("self-review_ratio", 0.0, INK_2, "builder alone, told to review its work"), ("worker-loop_ratio", -0.27, MUTED, "builder alone, told \"Continue.\""))
    rows = sorted(rows, key=lambda r: (min(r[k] for k, *_ in bars), r["task"]))
    finite = [r[k] for r in rows for k, *_ in bars if not math.isinf(r[k])]
    right = max(1.2, max(finite, default=1.0) * 1.1)
    title = (f"The loop reaches Sol's score for {_median_ratio([r['loop_ratio'] for r in rows])} of Sol's cost with its checker,\n"
             f"{_median_ratio([r['self-review_ratio'] for r in rows])} with the builder reviewing itself, {_median_ratio([r['worker-loop_ratio'] for r in rows])} told \"Continue.\"\n"
             f"(medians over {n} sampled ProgramBench tasks)")
    fig, ax = plt.subplots(figsize=(8.6, 0.78 * n + 2.4), dpi=200)
    ax.axvline(1, color=INK, lw=1, ls=(0, (4, 3)), zorder=1)
    ax.text(1 + right * 0.01, n - 0.3, "Sol's cost", fontsize=8.6, color=INK_2, va="bottom")
    box = {"facecolor": CANVAS, "edgecolor": "none", "pad": 0.6}
    for i, r in enumerate(rows):
        y = n - 1 - i
        for key, dy, color, _ in bars:
            ratio = r[key]
            if math.isinf(ratio):
                ax.text(right * 0.007, y + dy, "not reached", va="center", fontsize=7.8, color=MUTED, bbox=box, zorder=3)
                continue
            ax.barh(y + dy, ratio, color=color, height=0.24, zorder=2)
            ax.text(ratio + right * 0.012, y + dy, f"{100 * ratio:.0f}%", va="center", fontsize=7.8, color=color, bbox=box, zorder=3)
    ax.set_yticks(range(n), [r["task"] for r in reversed(rows)], fontsize=9)
    ax.tick_params(axis="y", length=0)
    ax.set_xlim(0, right)
    ax.set_ylim(-0.8, n + 0.3)
    ax.xaxis.set_major_locator(matplotlib.ticker.MultipleLocator(0.5 if right <= 3 else 1.0))
    ax.xaxis.set_major_formatter(matplotlib.ticker.FuncFormatter(lambda v, _: f"{100 * v:.0f}%"))
    ax.set_xlabel("The loop's cost to reach Sol's mean score, as a share of Sol's cost", labelpad=8)
    ax.spines[["top", "right", "left"]].set_visible(False)
    ax.legend(handles=[Line2D([], [], color=color, lw=8, label=label) for _, _, color, label in bars], loc="lower right", frameon=False, fontsize=8.8)
    ax.set_title(title, loc="left", fontsize=13.5, pad=12, fontfamily=["Fraunces", "DejaVu Serif"], fontweight="semibold", color=INK)
    fig.tight_layout()
    path = out / "worker-only-equal-score.png"
    fig.savefig(path)
    return path


def summary(rows: list[dict]) -> str:
    lines = []
    for key, label in (("worker-loop", "told Continue."), ("self-review", "told to review its work")):
        d = [r[key][0] - r["loop"][0] for r in rows]
        lines.append(f"builder alone, {label}: vs the review loop at Sol's cost higher on {sum(x > 0 for x in d)}, lower on {sum(x < 0 for x in d)} of {len(rows)} tasks, mean {statistics.fmean(d):+.1f} points; "
                     f"vs Sol higher on {sum(r[key][0] > r['sol'][0] for r in rows)}; vs Luna's single shot higher on {sum(r[key][0] > r['single'][0] for r in rows)}; "
                     f"mean over tasks {statistics.fmean(r[key][0] for r in rows):.1f}%; rounds within Sol's budget {statistics.fmean(r[f'{key}_rounds'] for r in rows):.1f}; "
                     f"spent of the budget {statistics.fmean(r[f'{key}_spent'] / r['budget'] for r in rows):.0%}; cost to reach Sol's score median {_median_ratio([r[f'{key}_ratio'] for r in rows])}, reached on {sum(not math.isinf(r[f'{key}_ratio']) for r in rows)}")
    lines.append(f"means over tasks: Sol {statistics.fmean(r['sol'][0] for r in rows):.1f}, Luna single shot {statistics.fmean(r['single'][0] for r in rows):.1f}, review loop at Sol's cost {statistics.fmean(r['loop'][0] for r in rows):.1f}")
    return "\n".join(lines)


def main(paths: list[Path], out_dir: Path | None = None) -> list[Path]:
    root = Path(__file__).resolve().parent.parent
    rows = rows_from(sorted(p for p in paths if p.name.startswith("luna-xhigh-") and not p.name.endswith(".match.json")), root / "figures")
    load_fonts()
    plt.rcParams.update({
        "font.family": ["Spline Sans", "DejaVu Sans"], "font.size": 10.5,
        "figure.facecolor": CANVAS, "axes.facecolor": CANVAS, "savefig.facecolor": CANVAS,
        "text.color": INK, "axes.edgecolor": MUTED, "axes.linewidth": 0.8, "axes.labelcolor": INK_2, "axes.labelweight": "medium",
        "xtick.color": MUTED, "ytick.color": MUTED, "xtick.labelcolor": INK_2, "ytick.labelcolor": INK_2, "legend.labelcolor": INK_2,
    })
    out = out_dir or root / "figures"
    print(summary(rows))
    return [equal_cost(rows, out), equal_score(rows, out)]


if __name__ == "__main__":
    args = sys.argv[1:]
    out_dir = Path(args.pop(args.index("--out-dir") + 1)) if "--out-dir" in args else None
    args = [a for a in args if a != "--out-dir"]
    for path in main([Path(p) for p in args], out_dir):
        print(path)

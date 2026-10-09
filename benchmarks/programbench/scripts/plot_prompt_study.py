"""Prompt study: GPT-5.6 Luna's loop against the same model prompted with both roles in one session,
from the JSONs that `bench parity` and `bench match` write (`<loop>.json` and `<loop>.match.json`).

    python3 scripts/plot_prompt_study.py figures/prompt-study/*.json [--out-dir DIR]

Writes two figures to figures/ (or DIR):
- prompt-study-equal-cost.png: per task, the sessions against the loop at the sessions' mean cost
  (means with 95% bootstrap intervals; rows by the loop's lead);
- prompt-study-equal-score.png: per task, the loop's cost to reach the sessions' mean score, as a
  share of their mean cost; tasks the loop never gets there give the gap at equal cost instead.

Needs matplotlib; fonts as in plot_pass_rate.py.
"""
import json
import math
import statistics
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker

import plot_parity
from plot_pass_rate import CANVAS, INK, INK_2, MUTED, RUST, load_fonts

LEGEND = ("GPT-5.6 Luna (xhigh), one prompted session", "GPT-5.6 Luna (xhigh), loop at the same cost")
TITLE = "Zeroshot's loop outperforms one session\nprompted with both roles at equal cost\nin {share}% of {n} sampled ProgramBench tasks"


def task_of(o: dict) -> str:
    return o["luna"].removeprefix("luna-xhigh-").rsplit("-v", 1)[0]  # luna-xhigh-<task>-v<N>


def equal_score(pairs: list[tuple[dict, dict]], out: Path) -> Path:
    rows = []
    for parity, match in pairs:
        reached = match["outcome"] == "reached"
        c = parity["comparisons"]["promotional"]
        rows.append({"task": task_of(match), "ratio": match["cost_ratio"] if reached else math.inf, "delta": c["luna_mean"] - c["sol_mean"]})
    rows.sort(key=lambda r: (r["ratio"], -r["delta"]))
    n = len(rows)
    median = statistics.median(r["ratio"] for r in rows)
    longest = max([r["ratio"] for r in rows if not math.isinf(r["ratio"])] or [1.0])
    right = max(1.2, longest * 1.1)
    fig, ax = plt.subplots(figsize=(8.6, 0.34 * n + 2.0), dpi=200)
    ax.axvline(1, color=INK, lw=1, ls=(0, (4, 3)), zorder=1)
    ax.text(1 + right * 0.01, n - 0.45, "the sessions' cost", fontsize=8.6, color=INK_2, va="bottom")
    box = {"facecolor": CANVAS, "edgecolor": "none", "pad": 0.8}
    for i, r in enumerate(rows):
        y = n - 1 - i
        if math.isinf(r["ratio"]):
            ax.text(right * 0.007, y, f"not reached ({r['delta']:+.1f} points at equal cost)".replace("-", "−"), va="center", fontsize=8.6, color=MUTED, bbox=box, zorder=3)
            continue
        color = RUST if r["ratio"] < 1 else MUTED
        ax.barh(y, r["ratio"], color=color, height=0.62, zorder=2)
        ax.text(r["ratio"] + right * 0.014, y, f"{100 * r['ratio']:.0f}%", va="center", fontsize=8.6, color=RUST if r["ratio"] < 1 else INK_2, bbox=box, zorder=3)
    ax.set_yticks(range(n), [r["task"] for r in reversed(rows)], fontsize=9)
    ax.tick_params(axis="y", length=0)
    ax.set_xlim(0, right)
    ax.set_ylim(-1.0, n + 0.3)
    ax.xaxis.set_major_locator(matplotlib.ticker.MultipleLocator(0.5 if right <= 3 else 1.0))
    ax.xaxis.set_major_formatter(matplotlib.ticker.FuncFormatter(lambda v, _: f"{100 * v:.0f}%"))
    ax.set_xlabel("The loop's cost to reach the sessions' mean score, as a share of their mean cost")
    ax.spines[["top", "right", "left"]].set_visible(False)
    if math.isinf(median):
        title = f"On most of {n} sampled ProgramBench tasks, the loop never reaches\nthe score of one session prompted with both roles"
    else:
        title = f"The loop reaches the score of one session prompted with both roles\nfor {100 * median:.0f}% of its cost (median of {n} sampled ProgramBench tasks)"
    ax.set_title(title, loc="left", fontsize=14, pad=12, fontfamily=["Fraunces", "DejaVu Serif"], fontweight="semibold", color=INK)
    fig.tight_layout()
    path = out / "prompt-study-equal-score.png"
    fig.savefig(path)
    return path


def main(paths: list[Path], out_dir: Path | None = None) -> list[Path]:
    out = out_dir or Path(__file__).resolve().parent.parent / "figures"
    parity_paths = [p for p in paths if not p.name.endswith(".match.json")]
    pairs = [(json.loads(p.read_text()), json.loads(p.with_name(p.name.removesuffix(".json") + ".match.json").read_text())) for p in parity_paths]
    first = plot_parity.main(parity_paths, out, legend=LEGEND, title=TITLE, name="prompt-study-equal-cost.png")
    load_fonts()
    return [first, equal_score(pairs, out)]


if __name__ == "__main__":
    args = sys.argv[1:]
    out_dir = Path(args.pop(args.index("--out-dir") + 1)) if "--out-dir" in args else None
    args = [a for a in args if a != "--out-dir"]
    for path in main([Path(p) for p in args], out_dir):
        print(path)

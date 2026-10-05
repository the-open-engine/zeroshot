"""Cost-parity study: per task, GPT-5.6 Sol's single workers against GPT-5.6 Luna's loop at the same
model cost (means with 95% bootstrap intervals), from the JSONs that `bench parity` writes.

    python3 scripts/plot_parity.py figures/cost-parity/*.json

Writes figures/cost-parity-promotional.png and figures/cost-parity-launch.png (Sol's prices that
set the budget). Needs matplotlib; fonts as in plot_pass_rate.py.
"""
import json
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker
from matplotlib import font_manager
from matplotlib.lines import Line2D

from plot_pass_rate import CANVAS, HAIRLINE, INK, INK_2, MUTED, RUST, load_fonts

TASKS = [("svgbob", "Rust: ASCII diagrams to SVG"), ("ditaa", "Java: ASCII diagrams to PNG"), ("calcurse", "C: terminal calendar"),
         ("revive", "Go: linter for Go"), ("fasttext", "C++: text classification")]


def main(paths: list[Path], prices: str = "promotional") -> Path:
    data = {}
    for p in paths:
        o = json.loads(p.read_text())
        task = next(t for t, _ in TASKS if t in o["luna"])
        data[task] = o
    load_fonts()
    plt.rcParams.update({
        "font.family": ["Spline Sans", "DejaVu Sans"], "font.size": 10.5,
        "figure.facecolor": CANVAS, "axes.facecolor": CANVAS, "savefig.facecolor": CANVAS,
        "text.color": INK, "axes.edgecolor": MUTED, "axes.linewidth": 0.8, "axes.labelcolor": INK_2, "axes.labelweight": "medium",
        "xtick.color": MUTED, "ytick.color": MUTED, "xtick.labelcolor": INK_2, "ytick.labelcolor": INK_2,
    })
    tasks = [(t, d) for t, d in TASKS if t in data]
    fig, ax = plt.subplots(figsize=(9.6, 1.0 * len(tasks) + 2.0), dpi=200)
    for i, (task, desc) in enumerate(tasks):
        c = data[task]["comparisons"][prices]
        y = len(tasks) - 1 - i
        for mean, ci, color, dy, marker in ((c["sol_mean"], c["sol_ci"], INK, -0.14, "o"), (c["luna_mean"], c["luna_ci"], RUST, 0.14, "D")):
            ax.plot(ci, [y + dy] * 2, color=color, lw=2.4, solid_capstyle="round", alpha=0.85, zorder=2)
            ax.scatter([mean], [y + dy], s=58, color=color, marker=marker, edgecolor=CANVAS, linewidth=1.2, zorder=3)
            ax.annotate(f"{mean:.1f}%", (ci[1], y + dy), xytext=(7, 0), textcoords="offset points", va="center", fontsize=9, color=color, fontweight="medium")
        # The loop rounds Luna's runs needed to reach the budget (their parity workspaces), on average.
        rounds = [r[prices]["rounds"] for r in data[task]["luna_runs"] if not r["excluded"] and not r[prices]["excluded"]]
        ax.annotate(f"{sum(rounds) / len(rounds):.0f}", (c["luna_mean"], y + 0.14), xytext=(0, 7), textcoords="offset points",
                    ha="center", va="bottom", fontsize=8.4, color=RUST, fontweight="medium")
    ax.set_yticks(range(len(tasks)))
    ax.set_yticklabels([f"{t}\n{d}" for t, d in reversed(tasks)], fontsize=9.5, linespacing=1.3)
    ax.set_ylim(-0.5, len(tasks) - 0.45)
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
    handles = [Line2D([], [], color=INK, marker="o", lw=2.4, ms=7, mec=CANVAS, label="GPT-5.6 Sol (xhigh), single worker"),
               Line2D([], [], color=RUST, marker="D", lw=2.4, ms=6.5, mec=CANVAS, label="GPT-5.6 Luna (xhigh), loop at the same model cost")]
    ax.legend(handles=handles, loc="lower left", frameon=False, fontsize=9.3, bbox_to_anchor=(0.0, 1.0), ncol=2, handlelength=2.2, columnspacing=1.6)
    ax.set_title("A small model with a review loop can outperform\na single-shot large model at cost parity", loc="left", fontfamily=["Fraunces", "DejaVu Serif"],
                 fontweight="semibold", fontsize=16, color=INK, pad=30, linespacing=1.15)
    fig.text(0.5, 0.012, f"Number above Luna's marker: mean loop rounds Luna needed to reach the cost of Sol's single worker on that task (Sol's {prices} prices).",
             ha="center", va="bottom", fontsize=8.6, color=MUTED)
    fig.tight_layout(rect=(0, 0.03, 1, 1))
    out = Path(__file__).resolve().parent.parent / "figures" / f"cost-parity-{prices}.png"
    fig.savefig(out)
    return out


if __name__ == "__main__":
    for prices in ("promotional", "launch"):
        print(main([Path(p) for p in sys.argv[1:]], prices))

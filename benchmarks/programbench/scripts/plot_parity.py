"""Cost-parity study: per task, GPT-5.6 Sol's single workers against GPT-5.6 Luna's loop at the same
model cost (means with 95% bootstrap intervals), from the JSONs that `bench parity` writes. The title
gives the share of tasks where the loop's mean is higher.

    python3 scripts/plot_parity.py figures/cost-parity/*.json [--out-dir DIR] [--order lead|luna]

Writes figures/cost-parity.png, or the same name in DIR. With more than six tasks, rows are sorted
by Luna's lead, or with --order luna by Luna's score. Needs matplotlib; fonts as in plot_pass_rate.py.
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
         ("revive", "Go: linter for Go"), ("fasttext", "C++: text classification"),
         # The expansion (README, cost-parity study), in the order they were drawn.
         ("parqeye", "Rust: Parquet file viewer"), ("tree-sitter", "Rust: parser generator CLI"), ("igrep", "Rust: interactive grep"),
         ("marmite", "Rust: static site generator"), ("datasurgeon", "Rust: extracts data from text"),
         ("crowbook", "Rust: Markdown books to HTML, PDF, EPUB"), ("gittype", "Rust: typing game on source code"),
         ("keifu", "Rust: Git commit graph TUI"), ("dust", "Rust: disk usage tree"), ("direnv", "Go: per-directory environments"),
         ("gdu", "Go: disk usage analyzer"), ("dstask", "Go: Git-backed task manager"),
         ("ascii-image-converter", "Go: images to ASCII art"), ("xz", "C: XZ compression tools"), ("samtools", "C: sequencing alignment tools")]


LEGEND = ("GPT-5.6 Sol (xhigh), single worker", "GPT-5.6 Luna (xhigh), loop at the same model cost")
TITLE = "A small model with a review loop outperforms\na single-shot large model at cost parity\nin {share}% of {n} sampled ProgramBench tasks"


def main(paths: list[Path], out_dir: Path | None = None, order: str = "lead", legend: tuple[str, str] = LEGEND, title: str = TITLE,
         name: str = "cost-parity.png") -> Path:
    """``legend`` names the single-worker and loop arms; ``title`` gets the loop's leads as ``share`` (percent) or
    ``count``, and the task count ``n``."""
    prices = "promotional"  # the single workers' prices, which set each task's budget
    data = {}
    for p in paths:
        o = json.loads(p.read_text())
        task = o["luna"].removeprefix("luna-xhigh-").rsplit("-v", 1)[0]  # luna-xhigh-<task>-v<N>
        data[task] = o
    load_fonts()
    plt.rcParams.update({
        "font.family": ["Spline Sans", "DejaVu Sans"], "font.size": 10.5,
        "figure.facecolor": CANVAS, "axes.facecolor": CANVAS, "savefig.facecolor": CANVAS,
        "text.color": INK, "axes.edgecolor": MUTED, "axes.linewidth": 0.8, "axes.labelcolor": INK_2, "axes.labelweight": "medium",
        "xtick.color": MUTED, "ytick.color": MUTED, "xtick.labelcolor": INK_2, "ytick.labelcolor": INK_2,
    })
    tasks = [(t, d) for t, d in TASKS if t in data]
    many = len(tasks) > 6
    if many:  # a forest plot: Luna's largest lead on top, or (order="luna") Luna's highest score
        key = {t: data[t]["comparisons"][prices]["luna_mean"] - (0 if order == "luna" else data[t]["comparisons"][prices]["sol_mean"]) for t, _ in tasks}
        tasks.sort(key=lambda item: -key[item[0]])
    fig, ax = plt.subplots(figsize=(9.6, (0.62 if many else 1.0) * len(tasks) + 2.0), dpi=200)
    for i, (task, desc) in enumerate(tasks):
        c = data[task]["comparisons"][prices]
        y = len(tasks) - 1 - i
        for mean, ci, color, dy, marker in ((c["sol_mean"], c["sol_ci"], INK, -0.14, "o"), (c["luna_mean"], c["luna_ci"], RUST, 0.14, "D")):
            ax.plot(ci, [y + dy] * 2, color=color, lw=2.4, solid_capstyle="round", alpha=0.85, zorder=2)
            ax.scatter([mean], [y + dy], s=58, color=color, marker=marker, edgecolor=CANVAS, linewidth=1.2, zorder=3)
            ax.annotate(f"{mean:.1f}%", (ci[1], y + dy), xytext=(7, 0), textcoords="offset points", va="center", fontsize=9, color=color, fontweight="medium")
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
    handles = [Line2D([], [], color=INK, marker="o", lw=2.4, ms=7, mec=CANVAS, label=legend[0]),
               Line2D([], [], color=RUST, marker="D", lw=2.4, ms=6.5, mec=CANVAS, label=legend[1])]
    ax.legend(handles=handles, loc="lower left", frameon=False, fontsize=9.3, bbox_to_anchor=(0.0, 1.0), ncol=2, handlelength=2.2, columnspacing=1.6)
    count = sum(data[t]["comparisons"][prices]["luna_mean"] > data[t]["comparisons"][prices]["sol_mean"] for t, _ in tasks)
    share = round(100 * count / len(tasks))
    ax.set_title(title.format(share=share, count=count, n=len(tasks)), loc="left", fontfamily=["Fraunces", "DejaVu Serif"],
                 fontweight="semibold", fontsize=16, color=INK, pad=30, linespacing=1.15)
    fig.tight_layout()
    out = (out_dir or Path(__file__).resolve().parent.parent / "figures") / name
    fig.savefig(out)
    return out


if __name__ == "__main__":
    args = sys.argv[1:]
    out_dir = Path(args.pop(args.index("--out-dir") + 1)) if "--out-dir" in args else None
    order = args.pop(args.index("--order") + 1) if "--order" in args else "lead"
    args = [a for a in args if a not in ("--out-dir", "--order")]
    print(main([Path(p) for p in args], out_dir, order))

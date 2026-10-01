"""Plot a loop experiment's hidden-test pass rate per scored round in The Open Engine style.

    python3 scripts/plot_pass_rate.py figures/luna-xhigh-svgbob-v3.json

Draws the mean of the loop runs with a 95% percentile bootstrap interval (runs resampled as units),
the single-worker level (the loop runs' mean first build) and published reference results. Writes
<data>-pass-rate.png next to the data file. Needs matplotlib and numpy; Fraunces and Spline Sans are
fetched once from Google Fonts into ~/.cache/zsbench/fonts (the default fonts are used offline).
"""

import json
import re
import sys
import urllib.request
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker
import numpy as np
from matplotlib import font_manager

# docs/brand/README.md: cream canvas, ink text, rust as the only accent (one rule, never a fill).
CANVAS, INK, INK_2, MUTED, HAIRLINE, RUST = "#FAF7F1", "#171411", "#4A433A", "#847A6C", "#E4DED3", "#C2240C"
FONTS = {"Fraunces": "Fraunces:wght@600", "SplineSans": "Spline+Sans:wght@400;500"}
FONT_CACHE = Path.home() / ".cache" / "zsbench" / "fonts"
RESAMPLES, SEED = 10_000, 20260924


def load_fonts() -> None:
    FONT_CACHE.mkdir(parents=True, exist_ok=True)
    for name, spec in FONTS.items():
        try:
            if not any(FONT_CACHE.glob(f"{name}-*.ttf")):
                css = urllib.request.urlopen(f"https://fonts.googleapis.com/css2?family={spec}", timeout=30).read().decode()
                for weight, url in re.findall(r"font-weight:\s*(\d+);\s*src:\s*url\((https://[^)]+\.ttf)\)", css):
                    urllib.request.urlretrieve(url, FONT_CACHE / f"{name}-{weight}.ttf")
        except OSError as error:
            print(f"warning: could not fetch {name} ({error}); using the default font", file=sys.stderr)
    for font in FONT_CACHE.glob("*.ttf"):
        font_manager.fontManager.addfont(str(font))


def main(data_path: Path) -> Path:
    data = json.loads(data_path.read_text())
    total, rounds = data["scored_tests"], data["rounds"]
    passed = np.array(list(data["loop_runs"].values()), dtype=float)
    rate = 100 * passed / total

    rng = np.random.default_rng(SEED)
    boot = rate[rng.integers(0, len(passed), size=(RESAMPLES, len(passed)))].mean(axis=1)
    lo, hi = np.percentile(boot, [2.5, 97.5], axis=0)
    mean = rate.mean(axis=0)

    load_fonts()
    plt.rcParams.update({
        "font.family": ["Spline Sans", "DejaVu Sans"],
        "font.size": 10.5,
        "figure.facecolor": CANVAS,
        "axes.facecolor": CANVAS,
        "savefig.facecolor": CANVAS,
        "text.color": INK,
        "axes.edgecolor": MUTED,
        "axes.linewidth": 0.8,
        "axes.labelcolor": INK_2,
        "axes.labelweight": "medium",
        "xtick.color": MUTED,
        "ytick.color": MUTED,
        "xtick.labelcolor": INK_2,
        "ytick.labelcolor": INK_2,
        "legend.labelcolor": INK_2,
    })
    fig, ax = plt.subplots(figsize=(9, 5.2), dpi=200)
    ax.fill_between(rounds, lo, hi, color=INK, alpha=0.09, lw=0, zorder=2, label="95% bootstrap CI of the mean")
    ax.plot(rounds, mean, color=INK, lw=2.2, marker="o", ms=5.5, mec=CANVAS, mew=1.2, zorder=3, label=f"Mean of {len(passed)} loop runs")

    short = rounds[-1] <= 12  # label every round; long runs get room for the labels at the right end
    x_lo, x_hi = (0.5, rounds[-1] + 0.5) if short else (0, rounds[-1] * 1.03)
    span = x_hi - x_lo
    single = rate[:, 0].mean()  # a single worker is the loop's first build
    lines = [(single, f"Single worker, {single:.1f}%", INK_2, "right", data.get("single_label_below", False))]
    for ref in data["references"]:
        pct = 100 * ref["passed"] / total
        prefix = "Best published" if ref.get("best") else "Published"
        text = f"{prefix}: {ref['model']}\n({ref['effort']}, {ref['harness']}), {pct:.1f}%"
        lines.append((pct, text, RUST if ref.get("best") else MUTED, ref.get("label_side", "right"), ref.get("label_below", False)))
    for y, text, color, side, below in lines:
        ax.axhline(y, color=color, lw=1.5, ls=(0, (1.2, 2.4)), dash_capstyle="round", zorder=1)
        x, ha = (x_hi - span * 0.3 / 51.5, "right") if side == "right" else (x_lo + span * 0.9 / 51.5, "left")
        ax.text(x, y - 0.55 if below else y + 0.55, text, ha=ha, va="top" if below else "bottom", ma=ha, linespacing=1.25, fontsize=9.5, fontweight="medium", color=color)

    low = min(lo.min(), *(y for y, *_ in lines)) - 4
    high = max(hi.max(), *(y for y, *_ in lines)) + 7  # room for the legend above the highest line
    ax.set_ylim(5 * np.floor(low / 5), 5 * np.ceil(high / 5) - 3)
    ax.set_xlim(x_lo, x_hi)
    ax.set_xticks(rounds if short else sorted({1, *(r for r in rounds if r % 5 == 0)}))
    ax.yaxis.set_major_locator(matplotlib.ticker.MultipleLocator(5))
    ax.yaxis.set_major_formatter(matplotlib.ticker.FormatStrFormatter("%d%%"))
    ax.set_xlabel("Build round (one build followed by one independent check)", labelpad=8)
    ax.set_ylabel(f"Hidden tests passed (of {total})", labelpad=8)
    ax.spines[["top", "right"]].set_visible(False)
    ax.grid(axis="y", color=HAIRLINE, lw=0.9)
    ax.set_axisbelow(True)
    ax.tick_params(length=4, width=0.8)
    for label in ax.get_xticklabels() + ax.get_yticklabels():
        label.set_fontproperties(font_manager.FontProperties(family="DejaVu Sans Mono", size=9))
    ax.set_title(data["title"], loc="left", fontfamily=["Fraunces", "DejaVu Serif"], fontweight="semibold", fontsize=17, color=INK, pad=16)
    ax.legend(loc="upper left", frameon=False, fontsize=9.5, handlelength=2.4)
    fig.tight_layout()
    out = data_path.with_name(f"{data_path.stem}-pass-rate.png")
    fig.savefig(out)
    return out


if __name__ == "__main__":
    print(main(Path(sys.argv[1])))

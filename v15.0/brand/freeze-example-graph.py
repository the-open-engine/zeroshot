"""Freeze zeroshot.sh's #one-graph drawing into standalone SVG sources.

Usage, from the repository root: python3 docs/brand/freeze-example-graph.py <zeroshot-landing-page checkout> .
"""
import html as htmllib
import re
import sys
from pathlib import Path

site = Path(sys.argv[1])
repo = Path(sys.argv[2])
html = (site / "index.html").read_text()
css = (site / "styles.css").read_text()
PAD = 16

# The loop's own rules, from the comment that opens them to the end of the generated run.
start = css.index("/* The loop under the line.")
end = css.index("/* loop-run:end */")
skip = ("ink-pending", "is-inking", "cue", ".loop {", ".loop--tall", "is-playing", "@media print")
rules = []
for line in re.sub(r"/\*.*?\*/", "", css[start:end], flags=re.S).splitlines():
    s = line.strip()
    if not s:
        continue
    if any(k in s for k in skip):
        continue
    rules.append(s)

# The site's reduced-motion still: no dot, the passing ticks shown.
still = [
    l.strip()
    for block in re.findall(r"@media \(prefers-reduced-motion: reduce\) \{(.*?)\n\}", css, re.S)
    for l in block.splitlines()
    if l.strip().startswith(".loop ")
]

def var(theme_block, name):
    return re.search(rf"--{name}:\s*([^;]+);", theme_block).group(1).strip()

light = css[css.index(":root {"):]
# zeroshot.sh is light-only now. The docs and README still offer dark mode, so without a dark block
# on the page, keep the dark colours from the previous freeze.
if ':root[data-theme="dark"] {' in css:
    dark = css[css.index(':root[data-theme="dark"] {'):]
else:
    previous = (repo / "docs/brand/diagram-sources/example-graph-wide.svg").read_text()
    dark = re.search(r":root\[data-theme=dark\]\{[^}]*\}", previous).group(0).replace("}", ";}")
names = ["paper", "marker-ink", "ink-2", "rust", "green"]
style = "\n".join(
    [
        ":root{" + ";".join(f"--{n}:{var(light, n)}" for n in names) + ";--hand:Caveat,cursive;background:var(--paper)}",
        ":root[data-theme=dark]{" + ";".join(f"--{n}:{var(dark, n)}" for n in names) + "}",
        '@font-face{font-family:Caveat;font-weight:600;src:url(../assets/caveat.ttf) format("truetype")}',
        *rules,
        "@media (prefers-reduced-motion: reduce){" + "".join(still) + "}",
    ]
)

for layout in ("wide", "tall"):
    i = html.index(f'<svg class="loop loop--{layout}"')
    e = html.index("</svg>", i) + len("</svg>")
    svg = html[i:e]
    head_end = svg.index(">") + 1
    head, body = svg[:head_end], svg[head_end:]
    x, y, w, h = map(float, re.search(r'viewBox="([^"]+)"', head).group(1).split())
    box = f"{x - PAD:g} {y - PAD:g} {w + 2 * PAD:g} {h + 2 * PAD:g}"
    head = re.sub(r'viewBox="[^"]+"', f'viewBox="{box}"', head)
    head = head.replace("<svg ", '<svg xmlns="http://www.w3.org/2000/svg" ', 1)
    out = head + "<style><![CDATA[" + style + "]]></style>" + body + "\n"
    # XML knows only five named entities; write the HTML ones (&rsquo;) as characters.
    xml = {"amp", "lt", "gt", "quot", "apos"}
    out = re.sub(r"&(\w+);", lambda m: m.group(0) if m.group(1) in xml else htmllib.unescape(m.group(0)), out)
    dest = repo / "docs/brand/diagram-sources" / f"example-graph-{layout}.svg"
    dest.write_text(out)
    print(dest, len(out), box)
    # The docs home draws the graph inline, as zeroshot.sh does, so it uses the page's fonts and
    # colour scheme: keep the loop's rules and drop the standalone file's :root theme and font.
    inline = head + "<style>" + "\n".join(rules + ["@media (prefers-reduced-motion: reduce){" + "".join(still) + "}"]) + "</style>" + body + "\n"
    inline = re.sub(r"&(\w+);", lambda m: m.group(0) if m.group(1) in xml else htmllib.unescape(m.group(0)), inline)
    dest = repo / "docs/overrides/partials" / f"example-graph-{layout}.svg"
    dest.write_text(inline)
    print(dest, len(inline))

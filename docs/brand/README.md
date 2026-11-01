# Zeroshot README banners

Editable HTML/CSS sources and rendered light/dark desktop PNGs, plus a portrait
mobile PNG. The repository README no longer shows a banner: it said the same thing as the text
under it and took a phone's whole first screen. The files are kept for reuse.
The design follows [The Open Engine](https://theopenengine.com) and
[Zeroshot](https://zeroshot.sh), inspected on October 2, 2026: warm paper,
handwritten headings and diagrams, yellow notes, and the company mascot.

## Files

- `zeroshot-hero-light.html`, `zeroshot-hero-dark.html` and `zeroshot-hero-mobile.html`: source
  layouts.
- `hero.css`: shared layout and theme tokens.
- `zeroshot-hero-light.png` and `zeroshot-hero-dark.png`: 2560 × 800 exports
  from a 1280 × 400 canvas at 2× scale.
- `zeroshot-hero-mobile.png`: 1122 × 1402 portrait banner, exported from a 561 × 701 canvas at
  2× scale, selected by the README below 641px viewport width, before the desktop theme sources.
  It uses the warm-paper design in both themes.
- `social/starred-by.html`: the "Starred by engineers at" strip under the README badges. Its
  companies come from the self-reported GitHub company field of zeroshot stargazers, each profile
  checked by hand, so the copy says "engineers at", never "used by". Marks are Simple Icons (CC0).
  Simple Icons later removed Microsoft and Amazon over those companies' trademark terms; the strip
  keeps their v11 marks on purpose. `starred-by-light.png` and
  `starred-by-dark.png` are its transparent 2× exports; re-render steps are below.
- `assets/`: local fonts, font licenses, and the website mascot. Rendering needs
  no network requests.

## Design

Reenie Beanie is the handwriting face; Fraunces 600 is the company wordmark.
The light palette uses paper `#FAF7F1`, pen ink `#26221C`, and rust `#C2240C`.
Dark mode uses `#171411`, cream text, and the documentation theme's lighter rust
`#ED6B55` for readable accents. Notes retain yellow paper and dark ink in both themes.
The banners carry the README headline and the "graphs are enforced" note. They draw no graph:
a fixed loop on the banner would sell the built-in template instead of the team's own graph.

## Re-render

From the repository root, install rendering tools outside the product dependencies:

```sh
npm install --prefix /tmp/open-engine-banner-tools playwright@1.61.0
/tmp/open-engine-banner-tools/node_modules/.bin/playwright install chromium
NODE_PATH=/tmp/open-engine-banner-tools/node_modules node <<'JS'
const { chromium } = require('playwright');
const { resolve } = require('node:path');
const { pathToFileURL } = require('node:url');
(async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({
      viewport: { width: 1280, height: 400 },
      deviceScaleFactor: 2,
    });
    for (const [name, width, height] of [['light', 1280, 400], ['dark', 1280, 400], ['mobile', 561, 701]]) {
      await page.setViewportSize({ width, height });
      const stem = resolve(`docs/brand/zeroshot-hero-${name}`);
      await page.goto(pathToFileURL(`${stem}.html`).href);
      await page.evaluate(() => document.fonts.ready);
      const ready = await page.evaluate(() =>
        [...document.fonts].every(font => font.status === 'loaded') &&
        [...document.images].every(image => image.complete && image.naturalWidth > 0)
      );
      if (!ready) throw new Error('Banner assets failed to load');
      await page.locator('.hero').screenshot({ path: `${stem}.png` });
    }
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
JS
```

Review both exports at full resolution and README display width before committing.

## Asset provenance

- `assets/oec-mascot.png`: unchanged copy of
  `company/content/brand/logos/open-engine-mascot-website.png`, originally from
  <https://theopenengine.com/landing/oec-mascot.png>.
- Fonts: Google Fonts' Fraunces 600 (optical-size variable) and Reenie Beanie 400,
  downloaded October 2, 2026. Their SIL Open Font Licenses are bundled beside them.
  Upstream: [Fraunces](https://github.com/google/fonts/tree/main/ofl/fraunces),
  [Reenie Beanie](https://github.com/google/fonts/tree/main/ofl/reeniebeanie).

## Other assets

`zeroshot-og.html` / `zeroshot-og.png` and `zeroshot-seal.svg` retain the earlier
engraved-seal design. They are separate from the README banners. `social/` contains
the existing README social links, and `oec-crow.png` / `oec-favicon.svg` are used by
the documentation theme.

## Workflow diagrams

The workflow diagram shows both reviewers, both repair paths, and optional Git delivery.
`zeroshot-workflow-mobile.svg` is the same drawing laid out for phones; the documentation
overview shows it below 720px.
Editable SVG labels and shapes live in `diagram-sources/`. Run the exporter after
editing them:

```sh
python3 -m venv /tmp/open-engine-diagram-tools
/tmp/open-engine-diagram-tools/bin/pip install fonttools==4.63.0
/tmp/open-engine-diagram-tools/bin/python docs/brand/outline-diagrams.py
```

Exports land in `docs/assets/`. Text becomes vector paths, retaining accessible
labels, so GitHub needs no external fonts. Each SVG supports light and dark mode.
Review both themes after changes, including the mobile layouts where supplied.
Body text uses [Spline Sans](https://github.com/google/fonts/tree/main/ofl/splinesans),
with its SIL Open Font License bundled in `assets/`.

The animated version is authored in the [shared assets repository](https://github.com/the-open-engine/assets/tree/main/compositions/zeroshot-readme). Its timeline illustrates a review rejection and
a delivery failure; both repairs return to both reviewers.

The 22-second GIF exports are `docs/assets/zeroshot-demo.gif` (light) and
`docs/assets/zeroshot-demo-dark.gif` (dark), rendered at 1600 × 940 and 15 fps.
The documentation retains these workflow assets; the README uses the product-page graphs below.

## Social buttons

`social/buttons.html` is the source for the README's "Get help on Discord" and zeroshot.sh
buttons, plus an X button the README no longer shows. The Discord button is the only filled rust
button, so it reads as the primary action; the other buttons stay ink pills with rust icons.
The README also dropped the theopenengine.com and LinkedIn buttons; their PNGs remain in `social/`. Exports are
70 px tall (35 px at 2×) and are displayed at `height="30"`.

Re-render both themes with the Playwright install above:

```sh
NODE_PATH=/tmp/open-engine-banner-tools/node_modules node <<'JS'
const { chromium } = require('playwright');
const { resolve } = require('node:path');
const { pathToFileURL } = require('node:url');
(async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({ viewport: { width: 480, height: 160 }, deviceScaleFactor: 2 });
    for (const theme of ['light', 'dark']) {
      await page.goto(pathToFileURL(resolve('docs/brand/social/buttons.html')).href);
      await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
      await page.evaluate(() => document.fonts.ready);
      for (const id of ['discord-cta', 'zeroshot-site', 'x']) {
        await page.locator(`#${id}`).screenshot({
          path: resolve(`docs/brand/social/${id}-${theme}.png`),
          omitBackground: true,
        });
      }
    }
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
JS
```

### Starred-by strip

With the same tools installed, render both themes of `social/starred-by.html`:

```sh
NODE_PATH=/tmp/open-engine-banner-tools/node_modules node <<'JS'
const { chromium } = require('playwright');
const { resolve } = require('node:path');
const { pathToFileURL } = require('node:url');
(async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 300 }, deviceScaleFactor: 2 });
    const source = resolve('docs/brand/social/starred-by.html');
    for (const theme of ['light', 'dark']) {
      await page.goto(pathToFileURL(source).href);
      await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
      await page.evaluate(() => document.fonts.ready);
      const ready = await page.evaluate(() => [...document.fonts].every(font => font.status === 'loaded'));
      if (!ready) throw new Error('Strip fonts failed to load');
      await page.locator('.strip').screenshot({
        path: `docs/brand/social/starred-by-${theme}.png`,
        omitBackground: true,
      });
    }
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
JS
```

## Product-page graph exports

The README no longer shows these exports: its example graph is a text block, so it stays sharp at
any width, works in both themes, and is readable by agents that fetch the README as plain text.
The exports below are kept for reuse. They come from <https://zeroshot.sh/#topology>, captured on
October 19, 2026.
The source panels are `graph-panel-feature` (Ship a feature) and `graph-panel-danger` (A risky
change), each with desktop and mobile SVG layouts. These are illustrative custom graphs, not
recordings or built-in templates. The README links back to the interactive examples.

`docs/assets/product-graph-feature-{wide,mobile}.gif` preserves the feature graph's 16-second
animation: review rejection, repair, end-to-end rejection, repair, then delivery. The exports
have 800 frames at 50 fps and loop continuously. Desktop is 1280px wide; mobile is 780px wide.
The desktop export includes the page's key (writes, judges, built in, boots the stack); the
page's mobile layout has no key.
The risk-routing graph is static on the source page and exported as a PNG.

The four `product-graph-{feature,danger}-{wide,mobile}.png` files are static exports.

Exports retain the page's paths, labels, role icons, Caveat 600 font, and warm-paper palette.
Sixteen SVG units of padding protect icons at the edges. Both themes use the page's paper
background. PNGs are rendered at 2× scale from 960px desktop and 390px mobile display widths.

The feature animation's SVG sources are frozen in `diagram-sources/product-graph-feature-*.svg`.
Their animation rules come from the page stylesheet `assets/index-CvFDPx6b.css`; the source
keyframes and timing are unchanged. Caveat and its SIL Open Font License are in `assets/`,
from [Google Fonts](https://github.com/google/fonts/tree/main/ofl/caveat). Rendering is offline.

With the Playwright install described above and FFmpeg on PATH, reproduce the GIFs from the
repository root:

```sh
NODE_PATH=/tmp/open-engine-banner-tools/node_modules node docs/brand/render-product-graphs.cjs
```

The renderer pauses the source CSS animations and seeks every frame explicitly. Inspect the
review rejection, end-to-end rejection, final delivery, and loop boundary in both sizes.
When updating the drawings, refresh them from the source page and preserve its animation rules.
The source page owns changes to the graph drawings.

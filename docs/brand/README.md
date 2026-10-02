# Zeroshot README banners

Editable HTML/CSS sources and rendered light/dark PNGs for the repository README.
The design follows [The Open Engine](https://theopenengine.com) and
[Zeroshot](https://zeroshot.sh), inspected on October 2, 2026: warm paper,
handwritten headings and diagrams, yellow notes, and the company mascot.

## Files

- `zeroshot-hero-light.html` and `zeroshot-hero-dark.html`: source layouts.
- `hero.css`: shared layout and theme tokens.
- `zeroshot-hero-light.png` and `zeroshot-hero-dark.png`: 2560 × 800 exports
  from a 1280 × 400 canvas at 2× scale.
- `assets/`: local fonts, font licenses, and the website mascot. Rendering needs
  no network requests.

## Design

Reenie Beanie is the handwriting face; Fraunces 600 is the company wordmark.
The light palette uses paper `#FAF7F1`, pen ink `#26221C`, and rust `#C2240C`.
Dark mode uses `#171411`, cream text, and the documentation theme's lighter rust
`#ED6B55` for readable accents. Notes retain yellow paper and dark ink in both themes.
The diagrams illustrate each product's feedback loop, rather than runtime status.

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
    for (const theme of ['light', 'dark']) {
      const stem = resolve(`docs/brand/zeroshot-hero-${theme}`);
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

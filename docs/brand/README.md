# Zeroshot brand assets

Zeroshot's repo/social visuals, part of the shared **The Open Engine** system (sibling to Opcore). Fraunces wordmark with a rust period (`Zeroshot.`), the engraved **guilloché seal** (a verification variant of the family: an executor-verifier lemniscate whose two lobes cross on the rust verdict node), engineering-plate registration ticks, and the `№ 001` serial.

## Files

| File                                                  | What                                                                | Size      |
| ----------------------------------------------------- | ------------------------------------------------------------------- | --------- |
| `zeroshot-hero-light.png` / `zeroshot-hero-dark.png`  | README hero (`<picture>`, light/dark)                               | 2560×640  |
| `zeroshot-og.png`                                     | Social / OpenGraph card (dark)                                      | 2400×1260 |
| `zeroshot-seal.svg`                                   | Standalone seal                                                     | n/a       |
| `zeroshot-hero-{light,dark}.html`, `zeroshot-og.html` | Reproducible sources (real Fraunces via Google Fonts + inline seal) | n/a       |

## Tokens

Rust `#C2240C`, the single accent, **semantic only** (the period, the verdict/PASS mark, one rule; never decoration or fill) · cream `#FAF7F1` · ink `#171411` · OG dark `#14110E`. Type: **Fraunces** (wordmark + headlines), **Spline Sans** (body), system mono (labels, `№`).

## Re-render

The `.html` files are the source of truth. Render to PNG with headless Chrome at 2× device scale, e.g. via the bundled Puppeteer:

```js
const puppeteer = require('puppeteer');
const page = await (await puppeteer.launch()).newPage();
await page.setViewport({ width: 1280, height: 320, deviceScaleFactor: 2 }); // OG: 1200x630
await page.goto('file://.../zeroshot-hero-light.html', { waitUntil: 'networkidle0' });
await page.evaluate(() => document.fonts.ready);
await (await page.$('.hero')).screenshot({ path: 'zeroshot-hero-light.png' }); // OG: '.og'
```

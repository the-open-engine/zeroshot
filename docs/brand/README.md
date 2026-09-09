# Zeroshot brand assets

These repository and social assets use **The Open Engine** visual system. The Fraunces wordmark ends
with a rust period (`Zeroshot.`). In the engraved **guilloché seal**, an executor-verifier lemniscate
crosses at the rust verdict node; engineering-plate registration ticks and the `№ 001` serial
complete the mark.

## Files

| File                                                  | What                                                           | Size            |
| ----------------------------------------------------- | -------------------------------------------------------------- | --------------- |
| `oec-crow.png`, `oec-favicon.svg`                     | Company crow for the docs header and browser icon              | 908×968 / 64×64 |
| `zeroshot-hero-light.png` / `zeroshot-hero-dark.png`  | README hero (`<picture>`, light/dark)                          | 2560×640        |
| `zeroshot-og.png`                                     | Social / OpenGraph card (dark)                                 | 2400×1260       |
| `zeroshot-seal.svg`                                   | Standalone seal                                                | n/a             |
| `zeroshot-hero-{light,dark}.html`, `zeroshot-og.html` | Reproducible sources (Fraunces via Google Fonts + inline seal) | n/a             |

## Tokens

Rust `#C2240C` is the only accent. Reserve it for the period, the verdict/PASS mark, or one rule;
never use it as decoration or fill. The remaining colors are cream `#FAF7F1`, ink `#171411`, and OG
dark `#14110E`.

Set the wordmark and headlines in **Fraunces**, body copy in **Spline Sans**, and labels or `№` in
the system monospace face.

## Re-render

Edit the `.html` source files, then render PNGs with headless Chrome at 2× device scale. Puppeteer is
not a project dependency; install it on demand (`npx puppeteer browsers install chrome`) or run the
snippet below with a one-off `npx -p puppeteer node`:

```js
const puppeteer = require('puppeteer');

async function render() {
  const browser = await puppeteer.launch();
  try {
    const page = await browser.newPage();
    await page.setViewport({ width: 1280, height: 320, deviceScaleFactor: 2 }); // OG: 1200x630
    await page.goto('file://.../zeroshot-hero-light.html', { waitUntil: 'networkidle0' });
    await page.evaluate(() => document.fonts.ready);
    const hero = await page.$('.hero'); // OG: '.og'
    if (hero === null) throw new Error('render target not found');
    await hero.screenshot({ path: 'zeroshot-hero-light.png' });
  } finally {
    await browser.close();
  }
}

render().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
```

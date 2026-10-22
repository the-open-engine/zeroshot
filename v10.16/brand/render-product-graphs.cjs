// Run from the repository root with Playwright on NODE_PATH and ffmpeg on PATH.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const { mkdtempSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { resolve, join } = require('node:path');
const { pathToFileURL } = require('node:url');

async function render() {
  const browser = await chromium.launch();
  const scratch = mkdtempSync(join(tmpdir(), 'zeroshot-graphs-'));
  try {
    for (const mode of ['wide', 'mobile']) {
      const width = mode === 'wide' ? 1280 : 780;
      const page = await browser.newPage({
        viewport: { width, height: 2000 },
        deviceScaleFactor: 1,
        reducedMotion: 'no-preference',
      });
      const source = resolve(__dirname, `diagram-sources/product-graph-feature-${mode}.svg`);
      await page.goto(pathToFileURL(source).href);
      await page.evaluate(async () => {
        document.documentElement.style.width = '100%';
        document.documentElement.style.height = 'auto';
        await document.fonts.ready;
        if (![...document.fonts].every((font) => font.status === 'loaded')) {
          throw new Error('Graph font failed to load');
        }
        const animations = document.getAnimations();
        if (animations.length !== 18) throw new Error('Expected 18 source animations');
        animations.forEach((animation) => {
          animation.pause();
          animation.currentTime = 0;
        });
      });
      const graph = page.locator('svg.custom-diagram');
      for (let frame = 0; frame < 240; frame++) {
        await page.evaluate(
          (time) => {
            document.getAnimations().forEach((animation) => {
              animation.currentTime = time;
            });
          },
          (frame * 1000) / 15
        );
        await graph.screenshot({
          path: join(scratch, `${String(frame).padStart(4, '0')}.png`),
          animations: 'allow',
        });
      }
      execFileSync(
        'ffmpeg',
        [
          '-y',
          '-v',
          'error',
          '-framerate',
          '15',
          '-i',
          join(scratch, '%04d.png'),
          '-filter_complex',
          '[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle',
          '-loop',
          '0',
          resolve(__dirname, `../assets/product-graph-feature-${mode}.gif`),
        ],
        { stdio: 'inherit' }
      );
      await page.close();
    }
  } finally {
    await browser.close();
    rmSync(scratch, { recursive: true, force: true });
  }
}

render().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

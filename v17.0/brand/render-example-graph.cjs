// Run from the repository root with Playwright on NODE_PATH and ffmpeg on PATH.
// Renders zeroshot.sh's example graph (issue to merge) as looping GIFs and reduced-motion PNGs.
const { chromium } = require('playwright');
const { execFileSync } = require('node:child_process');
const { mkdtempSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { resolve, join } = require('node:path');
const { pathToFileURL } = require('node:url');

// GIF delays are stored in hundredths of a second, so 50 fps (20 ms) plays evenly.
const FPS = 50;
const LOOP_MS = 31800;
const FRAMES = Math.round((LOOP_MS / 1000) * FPS);
// Display widths in CSS pixels, rendered at 2x: the README column and a phone.
const LAYOUTS = { wide: 880, tall: 400 };
const THEMES = ['light', 'dark'];

async function open(browser, layout, theme, reducedMotion) {
  const source = resolve(__dirname, `diagram-sources/example-graph-${layout}.svg`);
  const page = await browser.newPage({
    viewport: { width: LAYOUTS[layout], height: 2000 },
    deviceScaleFactor: 2,
    reducedMotion,
  });
  await page.goto(pathToFileURL(source).href);
  const height = await page.evaluate(
    async ({ width, theme }) => {
      const svg = document.documentElement;
      const [, , w, h] = svg.getAttribute('viewBox').split(/\s+/).map(Number);
      svg.dataset.theme = theme;
      svg.setAttribute('width', width);
      svg.setAttribute('height', Math.round((width * h) / w));
      await document.fonts.ready;
      if (![...document.fonts].every((font) => font.status === 'loaded')) {
        throw new Error('Graph font failed to load');
      }
      return Math.round((width * h) / w);
    },
    { width: LAYOUTS[layout], theme }
  );
  await page.setViewportSize({ width: LAYOUTS[layout], height });
  return page;
}

async function renderGif(browser, layout, theme) {
  const page = await open(browser, layout, theme, 'no-preference');
  const scratch = mkdtempSync(join(tmpdir(), `zeroshot-example-graph-${layout}-${theme}-`));
  try {
    await page.evaluate((loopMs) => {
      const animations = document.getAnimations();
      if (animations.length === 0) throw new Error('No source animations');
      for (const animation of animations) {
        const { duration } = animation.effect.getTiming();
        if (duration !== loopMs) throw new Error(`Animation runs ${duration} ms, not ${loopMs}`);
        animation.pause();
        animation.currentTime = 0;
      }
    }, LOOP_MS);
    for (let frame = 0; frame < FRAMES; frame++) {
      await page.evaluate(
        (time) => {
          document.getAnimations().forEach((animation) => {
            animation.currentTime = time;
          });
        },
        (frame * 1000) / FPS
      );
      await page.screenshot({
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
        String(FPS),
        '-i',
        join(scratch, '%04d.png'),
        '-filter_complex',
        '[0:v]split[a][b];[a]palettegen=stats_mode=full[p];[b][p]paletteuse=dither=none:diff_mode=rectangle',
        '-loop',
        '0',
        resolve(__dirname, `../assets/example-graph-${layout}-${theme}.gif`),
      ],
      { stdio: 'inherit' }
    );
  } finally {
    await page.close();
    rmSync(scratch, { recursive: true, force: true });
  }
}

async function renderStill(browser, layout, theme) {
  const page = await open(browser, layout, theme, 'reduce');
  await page.screenshot({
    path: resolve(__dirname, `../assets/example-graph-${layout}-${theme}.png`),
  });
  await page.close();
}

async function render() {
  const browser = await chromium.launch();
  try {
    const jobs = [];
    for (const layout of Object.keys(LAYOUTS)) {
      for (const theme of THEMES) {
        jobs.push(renderStill(browser, layout, theme), renderGif(browser, layout, theme));
      }
    }
    await Promise.all(jobs);
  } finally {
    await browser.close();
  }
}

render().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

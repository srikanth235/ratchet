// Renders the four app screens of design/v02.html at the Mac app's window size (1080 x 692) into design/v02/ref-<screen>.png,
// so the CI snapshots of the native app can be compared with them pixel by pixel (native.yml, design-diff).
//   node design/render-reference.mjs            (needs `playwright`; on a runner: npx playwright install chromium first)
// The page loads Instrument Sans / Serif and JetBrains Mono from Google Fonts, so it needs network.
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const require = createRequire(import.meta.url);
let chromium;
try { ({ chromium } = require('playwright')); } catch { ({ chromium } = require('playwright-core')); }

const here = path.dirname(fileURLToPath(import.meta.url));
const WIDTH = 1080, HEIGHT = 692;
const screens = { s1: 'overview', s2: 'connectors', s3: 'new-connections', s4: 'new-environment' };

const browser = await chromium.launch(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {});
const page = await browser.newPage({ viewport: { width: WIDTH, height: HEIGHT }, deviceScaleFactor: 1 });
await page.goto('file://' + path.join(here, 'v02.html'), { waitUntil: 'networkidle' });
await page.evaluate(() => document.fonts.ready);
// One window per image: no outer padding, square corners, the app's height.
await page.addStyleTag({ content: `.screen{width:${WIDTH}px;padding:0} .win{border-radius:0;border:0;min-height:${HEIGHT}px}` });
for (const [id, name] of Object.entries(screens)) {
  const box = await page.locator(`#${id} .win`).boundingBox();
  const out = path.join(here, 'v02', `ref-${name}.png`);
  await page.screenshot({
    path: out, fullPage: true,
    clip: { x: box.x, y: box.y, width: WIDTH, height: HEIGHT },
  });
  console.log(`${name}: window ${Math.round(box.width)}x${Math.round(box.height)} -> ${out}`);
}
await browser.close();

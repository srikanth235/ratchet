// Launches the BUILT app (run `pnpm build` first) under Electron + Playwright and screenshots every screen.
// Usage (headless Linux):  xvfb-run -a node scripts/screenshots.mjs <outDir>
// Needs `playwright` resolvable (global install is fine: set PLAYWRIGHT_MODULE=/path/to/node_modules/playwright).
import { createRequire } from 'node:module';
import { mkdirSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { _electron: electron } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const electronBin = require('electron');

const outDir = resolve(process.argv[2] ?? join(tmpdir(), 'ratchet-shots'));
mkdirSync(outDir, { recursive: true });
const userData = mkdtempSync(join(tmpdir(), 'ratchet-userdata-'));

const app = await electron.launch({
  executablePath: electronBin,
  args: [resolve(here, '..'), '--no-sandbox', `--user-data-dir=${userData}`],
});
const page = await app.firstWindow();
page.on('pageerror', (e) => console.error('pageerror:', e.message));
page.on('console', (m) => m.type() === 'error' && console.error('console.error:', m.text()));
await page.setViewportSize({ width: 1440, height: 960 });
await page.waitForSelector('.shell');

const shot = async (name) => {
  await page.waitForTimeout(700);
  await page.screenshot({ path: join(outDir, `${name}.png`) });
  console.log('wrote', join(outDir, `${name}.png`));
};
const nav = (label) => page.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: label }).click();

await page.waitForSelector('.featured');
await shot('01-optimizations');

await page.getByRole('button', { name: /New optimization/ }).click();
await page.waitForSelector('.yaml');
await page.waitForSelector('text=Valid config');
await shot('02-new-optimization');

await nav('AWS sandbox');
await page.waitForSelector('.plan');
await shot('03-aws-sandbox');

await nav('Live agents');
await page.waitForSelector('.agent-card');
await page.waitForSelector('.log__line');
await shot('04-live-agents');

await nav('Benchmark history');
await page.waitForSelector('.rt-stair');
await shot('05-benchmark-history');

await nav('Result');
await page.waitForSelector('.headline');
await shot('06-result');

await nav('Settings');
await page.waitForSelector('.secret-row');
await shot('07-settings');

// End-to-end: create + start through the real IPC, land on Live agents.
await nav('Optimizations');
await page.getByRole('button', { name: /New optimization/ }).click();
await page.waitForSelector('text=Valid config');
await page.getByRole('button', { name: 'Run baseline & start' }).click();
await page.waitForSelector('.agent-card');
await page.waitForTimeout(9000);
await shot('08-new-run-live');

await app.close();

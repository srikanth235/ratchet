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
  // The seeded dev engine: screens with data, no cloud, no network.
  env: { ...process.env, RATCHET_ENGINE: 'dev', RATCHET_DEV_TICK_MS: '3600000', RATCHET_USER_DATA_DIR: userData },
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
await page.getByRole('heading', { name: 'Repo', level: 2 }).waitFor();
await shot('02-new-optimization-repo');
await page.getByRole('group', { name: 'Demos' }).getByRole('button').first().click();
await page.getByRole('button', { name: 'Continue to review' }).click();
await page.getByRole('heading', { name: 'Review', level: 2 }).waitFor();
await shot('03-new-optimization-review');

await nav('Environments');
await page.getByRole('heading', { name: 'Environments', level: 1 }).waitFor();
await shot('04-environments');

await nav('Agents');
await page.waitForSelector('.agent-card');
await page.waitForSelector('.log__line');
await shot('05-agents');

await nav('Benchmarks');
await page.waitForSelector('.rt-stair');
await shot('06-benchmark-history');
await page.getByRole('radio', { name: 'Templates' }).click();
await page.getByRole('region', { name: 'Evaluator templates' }).waitFor();
await shot('07-benchmark-templates');

await nav('Result');
await page.waitForSelector('.headline');
await shot('08-result');

await nav('Settings');
await page.getByTestId('row-agents').waitFor();
await shot('09-settings');

await app.close();

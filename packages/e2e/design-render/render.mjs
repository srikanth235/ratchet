// Render the `design/*.dc.html` files in standalone Chromium and screenshot them at 1440px wide.
//   node design-render/render.mjs [name…]          → artifacts/design/<screen>.png
// `buildDesignPage(file)` / `openDesign(page, screen)` are reused by check.mjs for computed-style extraction.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');

const here = dirname(fileURLToPath(import.meta.url));
export const E2E_ROOT = join(here, '..');
export const DESIGN_DIR = join(E2E_ROOT, '..', '..', 'design');
export const OUT_DIR = join(E2E_ROOT, 'artifacts', 'design');
const HTML_DIR = join(OUT_DIR, 'html');

/** design file → screen name (matches the app screenshot names in artifacts/app). */
export const SCREENS = {
  optimizations: 'Main.dc.html',
  'new-optimization': 'NewOptimization.dc.html',
  sandbox: 'Environment.dc.html',
  'live-agents': 'Live.dc.html',
  benchmarks: 'Benchmarks.dc.html',
  result: 'Result.dc.html',
};

/** @fontsource files used by the app, inlined as data: URIs so the shim output is self-contained and offline. */
function fontFaceCss() {
  const desktopRequire = createRequire(join(E2E_ROOT, '..', 'desktop', 'package.json'));
  const faces = [
    ['Instrument Serif', 'instrument-serif', 400, 'normal'],
    ['Instrument Serif', 'instrument-serif', 400, 'italic'],
    ['Instrument Sans', 'instrument-sans', 400, 'normal'],
    ['Instrument Sans', 'instrument-sans', 500, 'normal'],
    ['Instrument Sans', 'instrument-sans', 600, 'normal'],
    ['JetBrains Mono', 'jetbrains-mono', 400, 'normal'],
    ['JetBrains Mono', 'jetbrains-mono', 500, 'normal'],
  ];
  return faces
    .map(([family, pkg, weight, style]) => {
      const file = desktopRequire.resolve(`@fontsource/${pkg}/files/${pkg}-latin-${weight}-${style}.woff2`);
      const b64 = readFileSync(file).toString('base64');
      return `@font-face{font-family:'${family}';font-style:${style};font-weight:${weight};font-display:block;src:url(data:font/woff2;base64,${b64}) format('woff2')}`;
    })
    .join('\n');
}

/** Write a standalone html file for a design: the shim replaces `<script src="./support.js">`. */
export function buildDesignPage(screen) {
  const file = SCREENS[screen];
  const source = readFileSync(join(DESIGN_DIR, file), 'utf8');
  const shim = readFileSync(join(here, 'dc-shim.js'), 'utf8');
  const embedded = JSON.stringify(source).replace(/</g, '\\u003c');
  const replacement = `<style>${fontFaceCss()}</style><script>window.__DC_SOURCE__=${embedded};</script><script>${shim.replace(/<\/script/gi, '<\\/script')}</script>`;
  if (!/<script src="\.\/support\.js"><\/script>/.test(source)) throw new Error(`${file}: no support.js script tag`);
  const html = source.replace(/<script src="\.\/support\.js"><\/script>/, () => replacement);
  mkdirSync(HTML_DIR, { recursive: true });
  const out = join(HTML_DIR, `${screen}.html`);
  writeFileSync(out, html);
  return out;
}

export async function openDesign(browser, screen, { width = 1440, height = 1000 } = {}) {
  const page = await browser.newPage({ viewport: { width, height } });
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(pathToFileURL(buildDesignPage(screen)).href);
  await page.waitForSelector('html[data-dc-rendered], html[data-dc-error]');
  const err = await page.getAttribute('html', 'data-dc-error');
  if (err) throw new Error(`${screen}: ${err}`);
  await page.evaluate(() => document.fonts.ready);
  return { page, errors };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const wanted = process.argv.slice(2);
  const screens = Object.keys(SCREENS).filter((s) => wanted.length === 0 || wanted.includes(s));
  mkdirSync(OUT_DIR, { recursive: true });
  const browser = await chromium.launch({ args: ['--no-sandbox'] });
  for (const screen of screens) {
    const { page, errors } = await openDesign(browser, screen);
    await page.screenshot({ path: join(OUT_DIR, `${screen}.png`), fullPage: true });
    const dims = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.scrollHeight]);
    console.log(`${screen}: ${dims.join('x')}${errors.length ? ` errors: ${errors.join('; ')}` : ''}`);
    await page.close();
  }
  await browser.close();
}

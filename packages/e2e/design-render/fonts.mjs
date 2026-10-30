// Which font files Chromium actually used (CSS.getPlatformFontsForNode) for representative elements, in the design and the app.
//   xvfb-run -a node design-render/fonts.mjs   → artifacts/design-check/fonts.json
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { ARTIFACTS, chromium, goTo, launchApp } from '../harness/launch.mjs';
import { openDesign } from './render.mjs';

const PROBES = [
  ['Page title (serif)', 'h1', 'main h1'],
  ['Hero number (serif)', '.serif', 'main .hero-num, main .stat-card__num, main [data-testid=featured-best]'],
  ['UI text (sans)', 'p', 'main .page-sub'],
  ['Metric / SHA (mono)', '.mono', 'main .mono'],
];

async function fontsOf(page, sel) {
  const client = await page.context().newCDPSession(page);
  await client.send('DOM.enable');
  await client.send('CSS.enable');
  const { root } = await client.send('DOM.getDocument', { depth: 0 });
  let nodeId = 0;
  for (const s of sel.split(', ')) {
    ({ nodeId } = await client.send('DOM.querySelector', { nodeId: root.nodeId, selector: s }));
    if (nodeId) break;
  }
  if (!nodeId) return null;
  const { fonts } = await client.send('CSS.getPlatformFontsForNode', { nodeId });
  return fonts.map((f) => `${f.familyName} (${f.isCustomFont ? 'webfont' : 'system'}, ${f.glyphCount} glyphs)`).join('; ');
}

const out = { design: {}, app: {} };
const browser = await chromium.launch({ args: ['--no-sandbox'] });
const { page: d } = await openDesign(browser, 'optimizations');
for (const [name, ds] of PROBES) out.design[name] = await fontsOf(d, ds);
await browser.close();
const { win, close } = await launchApp({ engine: 'dev' });
try {
  await win.waitForTimeout(2500);
  for (const [name, , as] of PROBES) out.app[name] = await fontsOf(win, as);
} finally {
  await close();
}
mkdirSync(join(ARTIFACTS, 'design-check'), { recursive: true });
writeFileSync(join(ARTIFACTS, 'design-check', 'fonts.json'), JSON.stringify(out, null, 1));
console.log(JSON.stringify(out, null, 1));

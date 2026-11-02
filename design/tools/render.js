// Renders each screen of design/v03.html to design/v03/<n>-<name>.png. Usage: node design/tools/render.js [sN ...]
const path = require('path'); const fs = require('fs'); const { chromium } = require('playwright');
const D = path.join(__dirname, '..');
const slug = s => s.toLowerCase().replace(/[·]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
(async () => {
  const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1500, height: 1000 } });
  const errs = []; p.on('pageerror', e => errs.push(e.message));
  await p.goto('file://' + path.join(D, 'v03.html')); await p.waitForTimeout(800);
  const want = process.argv.slice(2);
  const secs = await p.$$eval('section.screen', els => els.map(e => [e.id, e.dataset.name]));
  fs.mkdirSync(path.join(D, 'v03'), { recursive: true });
  for (const f of fs.readdirSync(path.join(D, 'v03'))) if (f.endsWith('.png') && !want.length) fs.unlinkSync(path.join(D, 'v03', f));
  for (const [id, name] of secs) {
    if (want.length && !want.includes(id)) continue;
    const file = id.slice(1) + '-' + slug(name) + '.png';
    await (await p.$('#' + id)).screenshot({ path: path.join(D, 'v03', file) });
  }
  console.log(errs.length ? errs : 'ok', secs.length + ' screens'); await b.close();
})();

// Renders each screen of design/v03.html to design/v03/<n>-<name>.png. Usage: node design/tools/render.js
const path = require('path'); const { chromium } = require('playwright');
const D = path.join(__dirname, '..');
const files = {s1:'1-home',s2:'2-run',s3:'3-optimization',s4:'4-customize',s5:'5-connectors',s6:'6-settings',s7:'7-agents',s8:'8-new-conversation',s9:'9-overview',s10:'10-connectors-discover'};
(async () => {
  const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1500, height: 1000 } });
  const errs = []; p.on('pageerror', e => errs.push(e.message));
  await p.goto('file://' + path.join(D, 'v03.html')); await p.waitForTimeout(800);
  for (const id of await p.$$eval('section[id^=s], [id^=s].screen, div[id^=s]', els => els.map(e => e.id).filter(i => /^s\d+$/.test(i)))) {
    const name = files[id] || id; await (await p.$('#' + id)).screenshot({ path: path.join(D, 'v03', name + '.png') });
  }
  console.log(errs.length ? errs : 'ok'); await b.close();
})();

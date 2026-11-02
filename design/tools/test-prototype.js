// Click test of design/v03-prototype.html: every annotated control is clicked, no page errors, routes land on their screen,
// and no interactive element is left without a route. Usage: node design/tools/test-prototype.js
const path = require('path'); const { chromium } = require('playwright');
const D = path.join(__dirname, '..');
const INTERACTIVE = '.btn,.chip,.nv,.run,.seg2 span,.tabs span,.stabs span,.plus,.more,.ibtn,.send,.tog,.add,.arr,.link,.sugg span,.pillst,.field,.tr[data-go],tr[data-go]';
const ANNOT = '[data-go],[data-toast],[data-pop],[data-pane],[data-swap],.tog,.add,.seg2 span,.tabs span';
(async () => {
  const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1500, height: 1000 } });
  const errs = [], fails = []; let clicks = 0;
  p.on('pageerror', e => errs.push(e.message)); p.on('console', m => { if (m.type() === 'error' && !/fonts|net::|Failed to load/i.test(m.text())) errs.push(m.text()); });
  await p.goto('file://' + path.join(D, 'v03-prototype.html')); await p.waitForTimeout(500);
  const ids = await p.$$eval('section.screen', e => e.map(s => s.id));
  const cur = () => p.$eval('.screen.cur', e => e.id);
  for (const id of ids) {
    await p.evaluate(i => window.__go(i), id);
    // 1. nothing interactive without a route
    const bare = await p.$$eval('#' + id + ' ' + INTERACTIVE.split(',').join(',#' + id + ' '), (els, A) => els.filter(e => !e.matches(A) && !e.closest('.seg2,.tabs,[aria-hidden=true]')).map(e => e.className + ':' + e.textContent.trim().slice(0, 20)), ANNOT);
    bare.forEach(x => fails.push(`${id}: no route on ${x}`));
    // 2. unlabelled icon-only buttons
    const unl = await p.$$eval('#' + id + ' button', els => els.filter(e => !e.textContent.trim() && !e.getAttribute('aria-label')).length);
    if (unl) fails.push(`${id}: ${unl} icon-only buttons without aria-label`);
    // 3. click everything annotated
    const n = await p.$$eval('#' + id + ' ' + ANNOT.split(',').join(',#' + id + ' '), e => e.length);
    for (let i = 0; i < n; i++) {
      await p.evaluate(i2 => window.__go(document.querySelector('.screen.cur').id), 0);
      await p.evaluate(s => window.__go(s), id);
      const info = await p.evaluate(([sid, k, sel]) => { const e = document.querySelectorAll('#' + sid + ' ' + sel.split(',').join(',#' + sid + ' '))[k]; if (!e) return null;
        const inSet = !!e.closest('.setwin'); const r = e.getBoundingClientRect(); const vis = r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden';
        return { go: (e.disabled || (/^s(6|21)$/.test(sid) && !inSet)) ? null : (e.dataset.go || null), vis, inSet, label: (e.getAttribute('aria-label') || e.textContent || e.className).trim().slice(0, 30) }; }, [id, i, ANNOT]);
      if (!info || !info.vis) continue;
      await p.evaluate(([sid, k, sel]) => document.querySelectorAll('#' + sid + ' ' + sel.split(',').join(',#' + sid + ' '))[k].click(), [id, i, ANNOT]); clicks++;
      if (info.go) { const c = await cur(); if (c !== info.go) fails.push(`${id}: "${info.label}" -> ${c}, expected ${info.go}`); }
    }
  }
  // settings window: ⌘, opens, Esc returns, tabs switch panes, click outside closes
  await p.evaluate(() => window.__go('s1')); await p.keyboard.press('Control+,'); if (await cur() !== 's6') fails.push('Ctrl+, did not open Settings');
  await p.keyboard.press('Escape'); if (await cur() !== 's1') fails.push('Esc did not return from Settings');
  await p.evaluate(() => window.__go('s6'));
  for (const t of ['Defaults', 'Notifications', 'This Mac', 'General']) { await p.click(`#s6 .stabs span[data-pane="${t}"]`); const on = await p.$eval('#s6 .pane.on', e => e.dataset.panel); if (on !== t) fails.push('settings tab ' + t + ' -> ' + on); }
  await p.mouse.click(1400, 700); if (await cur() === 's6') fails.push('click outside Settings did not close it');
  // popover chip toggles
  await p.evaluate(() => window.__go('s1')); await p.click('#s1 .chip[data-pop="p1"]').catch(() => {});
  // keyboard: Enter on a focused nav link
  await p.evaluate(() => window.__go('s9')); await p.focus('#s9 .nv[data-go="s4"]'); await p.keyboard.press('Enter'); if (await cur() !== 's4') fails.push('Enter on nav link did not route');
  console.log(`${ids.length} screens, ${clicks} clicks`); if (errs.length) console.log('page errors:', errs); if (fails.length) console.log('FAILS:\n' + fails.join('\n'));
  await b.close(); process.exit(errs.length || fails.length ? 1 : 0);
})();

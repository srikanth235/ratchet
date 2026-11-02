// Static audit of design/v03.html against the #10 §6 checks (WCAG 2.1 AA): text contrast, non-text contrast of control
// borders, text size >= 11 px, target size, labelled icon buttons, focus ring present. Usage: node design/tools/audit.js
const path = require('path'); const { chromium } = require('playwright');
(async () => {
  const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1500, height: 1000 } });
  await p.goto('file://' + path.join(__dirname, '..', 'v03.html')); await p.waitForTimeout(500);
  const res = await p.evaluate(() => {
    const out = { small: [], text: [], border: [], target: [], label: [], noicon: [], focus: 0, texts: 0, ctl: 0 };
    const parse = c => { const m = c.match(/rgba?\(([^)]+)\)/); if (!m) return null; const v = m[1].split(/[ ,\/]+/).filter(Boolean).map(Number); return { r: v[0], g: v[1], b: v[2], a: v.length > 3 ? v[3] : 1 }; };
    const over = (f, bg) => ({ r: f.r * f.a + bg.r * (1 - f.a), g: f.g * f.a + bg.g * (1 - f.a), b: f.b * f.a + bg.b * (1 - f.a), a: 1 });
    const lum = c => { const f = v => { v /= 255; return v <= .04045 ? v / 12.92 : Math.pow((v + .055) / 1.055, 2.4); }; return .2126 * f(c.r) + .7152 * f(c.g) + .0722 * f(c.b); };
    const cr = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((m, n) => n - m); return (x + .05) / (y + .05); };
    const bgOf = el => { const stack = []; for (let e = el; e; e = e.parentElement) { const cs = getComputedStyle(e); if (cs.backgroundImage !== 'none') return null; const c = parse(cs.backgroundColor); if (c && c.a > 0) { stack.push(c); if (c.a === 1) break; } } let base = { r: 255, g: 255, b: 255, a: 1 }; for (const c of stack.reverse()) base = over(c, base); return base; };
    for (const s of document.querySelectorAll('section.screen')) {
      out.focus += s.querySelectorAll('.focus').length;
      const walker = document.createTreeWalker(s, NodeFilter.SHOW_TEXT);
      for (let n; n = walker.nextNode();) {
        const t = n.textContent.trim(); if (!t) continue; const el = n.parentElement; if (el.closest('.sr,title,desc,style,script')) continue;
        const cs = getComputedStyle(el); if (cs.visibility === 'hidden' || cs.display === 'none') continue; const r = el.getBoundingClientRect(); if (!r.width) continue;
        out.texts++; const fs = parseFloat(cs.fontSize);
        if (fs < 10.99) out.small.push(`${s.id} ${fs}px "${t.slice(0, 24)}"`);
        const isSvg = el instanceof SVGElement; const fg = parse(isSvg ? cs.fill : cs.color); const bg = bgOf(el); if (!fg || !bg) continue;
        const f = over(fg, bg); const ratio = cr(f, bg); const big = fs >= 24 || (fs >= 18.66 && parseInt(cs.fontWeight) >= 700); const need = big ? 3 : 4.5;
        if (el.closest('.dis,[disabled],.stale')) continue;
        if (ratio < need) out.text.push(`${s.id} ${ratio.toFixed(2)}:1 (<${need}) "${t.slice(0, 24)}" ${cs.color}`);
      }
      for (const el of s.querySelectorAll('.btn:not(.pri):not(.ghost):not(.dis),.chip,.field,.sel,.seg2,.tog:not(.on),.ibtn,.plus,.add:not(.done),.arr,.composer')) {
        const cs = getComputedStyle(el); const bc = parse(cs.borderTopColor); if (!bc || parseFloat(cs.borderTopWidth) < 1) { out.border.push(`${s.id} no border ${el.className}`); continue; }
        const bg = bgOf(el.parentElement) || bgOf(el); if (!bg) continue; out.ctl++; const ratio = cr(over(bc, bg), bg); if (ratio < 2.95) out.border.push(`${s.id} ${ratio.toFixed(2)}:1 ${el.className.split(' ')[0]} "${el.textContent.trim().slice(0, 16)}"`);
      }
      for (const el of s.querySelectorAll('.btn,.ibtn,.plus,.add,.arr,.more,.chip,.tog,.send')) {
        const r = el.getBoundingClientRect(); if (!r.width) continue; const min = el.matches('.arr') ? 28 : 24;
        if (r.width < min || r.height < min) out.target.push(`${s.id} ${Math.round(r.width)}x${Math.round(r.height)} ${el.className.split(' ')[0]}`);
      }
      for (const el of s.querySelectorAll('button,[role=switch]')) if (!el.textContent.trim() && !el.getAttribute('aria-label')) out.label.push(`${s.id} ${el.className}`);
      for (const el of s.querySelectorAll('.stt')) if (!el.querySelector('svg')) out.noicon.push(s.id);
    }
    return out;
  });
  const show = (k, a) => console.log(`${a.length ? '🔴' : '✅'} ${k}: ${a.length}${a.length ? '\n  ' + [...new Set(a)].slice(0, 25).join('\n  ') : ''}`);
  console.log(`${res.texts} text runs, ${res.ctl} control borders checked, ${res.focus} focus rings drawn`);
  show('text below 11 px', res.small); show('text contrast (4.5:1, 3:1 large)', res.text); show('control border contrast (3:1)', res.border);
  show('target size (24 px, arrows 28 px)', res.target); show('icon-only buttons without a label', res.label); show('status without a shape', res.noicon);
  console.log(res.focus ? '✅ focus rings drawn on focused controls' : '🔴 no focus ring drawn');
  await b.close(); process.exit(res.small.length + res.text.length + res.border.length + res.label.length + res.noicon.length + (res.focus ? 0 : 1) ? 1 : 0);
})();

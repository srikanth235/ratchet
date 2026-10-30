// Design conformance: measure computed styles in the rendered design and in the running app, compare, write JSON.
//   xvfb-run -a node design-render/check.mjs <label>        → artifacts/design-check/<label>.json
//   RATCHET_MAIN=…/out-before/main/index.js  to measure another build of the app.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { ARTIFACTS, goTo, launchApp, openNewOptimization } from '../harness/launch.mjs';
import { chromium } from '../harness/launch.mjs';
import { openDesign } from './render.mjs';
import { SCREEN_SPECS, SHELL, STRUCTURE } from './specs.mjs';

const label = process.argv[2] ?? 'after';

/** Runs in the page: find an element by locator and read the requested properties. */
function measure({ loc, props }) {
  const at = loc.indexOf('@');
  let sel = loc;
  let text = '';
  let closest = '';
  if (at >= 0) {
    sel = loc.slice(0, at);
    const rest = loc.slice(at + 1);
    const gt = rest.lastIndexOf('>');
    text = gt >= 0 ? rest.slice(0, gt) : rest;
    closest = gt >= 0 ? rest.slice(gt + 1) : '';
  }
  const own = (el) =>
    Array.from(el.childNodes).filter((n) => n.nodeType === 3).map((n) => n.textContent).join('').replace(/\s+/g, ' ').trim();
  const matches = (el) => {
    if (!text) return true;
    const o = own(el);
    if (text.startsWith('^')) return new RegExp(text, 'i').test(o);
    return o.toLowerCase().includes(text.toLowerCase());
  };
  let el = null;
  // A locator may list alternatives separated by ", " at top level; try each in order.
  for (const s of splitTop(sel)) {
    const found = Array.from(document.querySelectorAll(s)).find(matches);
    if (found) { el = found; break; }
  }
  if (el && closest) el = el.closest(closest);
  if (!el) return { found: false };
  const cs = getComputedStyle(el);
  const r = el.getBoundingClientRect();
  const out = { found: true, tag: el.tagName.toLowerCase(), cls: (el.getAttribute('class') || '').slice(0, 40), values: {} };
  for (const p of props) {
    if (p === 'w') out.values[p] = String(r.width);
    else if (p === 'h') out.values[p] = String(r.height);
    else if (p === 'pad') out.values[p] = [cs.paddingTop, cs.paddingRight, cs.paddingBottom, cs.paddingLeft].join(' ');
    else out.values[p] = cs[p];
  }
  return out;

  function splitTop(s) {
    const parts = [];
    let depth = 0, cur = '';
    for (const ch of s) {
      if (ch === '(' || ch === '[') depth++;
      if (ch === ')' || ch === ']') depth--;
      if (ch === ',' && depth === 0) { parts.push(cur.trim()); cur = ''; } else cur += ch;
    }
    parts.push(cur.trim());
    return parts.filter(Boolean);
  }
}

// ---------------------------------------------------------------- comparison (node side)

const hex2 = (n) => Math.round(n).toString(16).padStart(2, '0');
function parseColor(s) {
  const cs = /color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)/.exec(s);
  if (cs) return [cs[1] * 255, cs[2] * 255, cs[3] * 255, cs[4] === undefined ? 1 : Number(cs[4])];
  const m = /rgba?\(\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)/.exec(s);
  if (!m) return null;
  let a = m[4] === undefined ? 1 : m[4].endsWith('%') ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
  return [Number(m[1]), Number(m[2]), Number(m[3]), a];
}
const fmtColor = (c) => (c[3] === 0 ? 'transparent' : `#${hex2(c[0])}${hex2(c[1])}${hex2(c[2])}${c[3] < 1 ? `/${+c[3].toFixed(2)}` : ''}`.toUpperCase());
function normColors(s) {
  return s.replace(/rgba?\([^)]*\)/g, (m) => {
    const c = parseColor(m);
    return c ? fmtColor(c) : m;
  });
}
const px = (s) => (s === 'normal' ? NaN : parseFloat(s));
const colorProps = /(color|backgroundColor)$/i;
const TOL = { fontSize: 0.5, lineHeight: 1, letterSpacing: 0.2, w: 1.5, h: 1.5, borderRadius: 0.6, borderTopWidth: 0.3, rowGap: 0.6, columnGap: 0.6 };

function display(prop, v) {
  if (v === undefined) return '—';
  if (colorProps.test(prop)) {
    const c = parseColor(v);
    return c ? fmtColor(c) : v;
  }
  if (prop === 'fontFamily') return v.split(',')[0].replace(/["']/g, '').trim();
  if (prop === 'boxShadow' || prop === 'backgroundImage') return v === 'none' ? 'none' : normColors(v).replace(/\s+/g, ' ');
  if (prop === 'pad') return v.split(' ').map((x) => `${+parseFloat(x).toFixed(1)}`).join(' ');
  if (['w', 'h'].includes(prop)) return `${+parseFloat(v).toFixed(1)}`;
  if (/^-?[\d.]+px$/.test(v)) return `${+parseFloat(v).toFixed(2)}px`;
  return v;
}

function equal(prop, d, a) {
  if (d === undefined || a === undefined) return false;
  if (colorProps.test(prop)) {
    const x = parseColor(d), y = parseColor(a);
    if (!x || !y) return d === a;
    if (x[3] === 0 && y[3] === 0) return true;
    return Math.abs(x[0] - y[0]) <= 2 && Math.abs(x[1] - y[1]) <= 2 && Math.abs(x[2] - y[2]) <= 2 && Math.abs(x[3] - y[3]) <= 0.02;
  }
  if (prop === 'fontFamily') return display(prop, d).toLowerCase() === display(prop, a).toLowerCase();
  if (prop === 'fontWeight') return Number(d === 'normal' ? 400 : d === 'bold' ? 700 : d) === Number(a === 'normal' ? 400 : a === 'bold' ? 700 : a);
  if (prop === 'pad') {
    const x = d.split(' ').map(parseFloat), y = a.split(' ').map(parseFloat);
    return x.every((v, i) => Math.abs(v - y[i]) <= 1);
  }
  if (prop === 'lineHeight' || prop === 'letterSpacing') {
    if (d === 'normal' || a === 'normal') return (d === 'normal' || parseFloat(d) === 0) && (a === 'normal' || parseFloat(a) === 0);
  }
  if (prop in TOL || prop === 'w' || prop === 'h') return Math.abs(px(d) - px(a)) <= (TOL[prop] ?? 0.6);
  if (prop === 'boxShadow' || prop === 'backgroundImage') return display(prop, d) === display(prop, a);
  return d === a;
}

// ---------------------------------------------------------------- run

async function runChecks(dPage, aPage, checks, group) {
  const rows = [];
  for (const [name, dLoc, aLoc, props] of checks) {
    if (process.env.CHECK_TRACE) console.log('check', name);
    const d = await dPage.evaluate(measure, { loc: dLoc, props });
    const a = await aPage.evaluate(measure, { loc: aLoc ?? dLoc, props });
    if (!d.found) { rows.push({ group, check: name, prop: '(element)', design: 'NOT FOUND IN DESIGN', app: a.found ? 'found' : 'missing', pass: false, note: `design locator ${dLoc}` }); continue; }
    if (!a.found) { rows.push({ group, check: name, prop: '(element)', design: 'present', app: 'missing', pass: false, note: `app locator ${aLoc ?? dLoc}` }); continue; }
    for (const p of props) {
      rows.push({ group, check: name, prop: p, design: display(p, d.values[p]), app: display(p, a.values[p]), pass: equal(p, d.values[p], a.values[p]) });
    }
  }
  return rows;
}

async function runStructure(dPage, aPage, items) {
  const rows = [];
  for (const [name, dLoc, aLoc] of items) {
    const d = (await dPage.evaluate(measure, { loc: dLoc, props: [] })).found;
    const a = (await aPage.evaluate(measure, { loc: aLoc ?? dLoc, props: [] })).found;
    rows.push({ check: name, design: d, app: a });
  }
  return rows;
}

const APP_ROUTE = {
  optimizations: (win) => goTo(win, 'Optimizations'),
  'new-optimization': (win) => openNewOptimization(win),
  sandbox: (win) => goTo(win, 'Environments'),
  'live-agents': (win) => goTo(win, 'Agents'),
  benchmarks: (win) => goTo(win, 'Benchmarks'),
  result: (win) => goTo(win, 'Result'),
};

const browser = await chromium.launch({ args: ['--no-sandbox'] });
const { win: aPage, close } = await launchApp();
const out = { label, at: new Date().toISOString(), screens: {} };
try {
  await aPage.waitForTimeout(1500);
  for (const screen of Object.keys(SCREEN_SPECS)) {
    const { page: dPage } = await openDesign(browser, screen);
    await APP_ROUTE[screen](aPage);
    await aPage.waitForTimeout(1500);
    const checks = [...(screen === 'optimizations' ? SHELL.map(([n, ...r]) => [`Sidebar · ${n}`, ...r]) : []), ...SCREEN_SPECS[screen]];
    const rows = await runChecks(dPage, aPage, checks, screen);
    const structure = await runStructure(dPage, aPage, STRUCTURE[screen] ?? []);
    out.screens[screen] = { rows, structure };
    const pass = rows.filter((r) => r.pass).length;
    console.log(`${screen}: ${pass}/${rows.length} style checks pass, structure ${structure.filter((s) => s.app).length}/${structure.length}`);
    await dPage.close();
  }
} finally {
  await close();
  await browser.close();
}
const dir = join(ARTIFACTS, 'design-check');
mkdirSync(dir, { recursive: true });
writeFileSync(join(dir, `${label}.json`), JSON.stringify(out, null, 1));

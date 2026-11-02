#!/usr/bin/env python3
"""Generates design/v03.html (v0.3.1 mockups) from components, so the sidebar, toolbar and status shapes are written once.
Run: python3 design/tools/build-v03.py   then   python3 design/tools/build-prototype.py   and   node design/tools/render.js
Styles live in design/tools/v03.css (Ink/Palette tokens mirror DesignKit/Tokens.swift)."""
import os, itertools

D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
CSS = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "v03.css")).read()

# ---------------------------------------------------------------- icons
P = {
 "home": '<path d="M2.5 7.5 8 3l5.5 4.5M4 6.8V13h8V6.8"/>',
 "gauge": '<path d="M2.5 11.5a5.5 5.5 0 1 1 11 0"/><path d="M8 11.5l2.8-3.6"/>',
 "list": '<path d="M3 4h10M3 8h10M3 12h6"/>',
 "bag": '<rect x="2.5" y="4.5" width="11" height="8" rx="2"/><path d="M6 4.5V3.5h4v1"/>',
 "plus": '<path d="M8 3v10M3 8h10"/>',
 "search": '<circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5 14 14"/>',
 "gear": '<circle cx="8" cy="8" r="2.2"/><path d="M8 1.8v1.7M8 12.5v1.7M1.8 8h1.7M12.5 8h1.7M3.6 3.6l1.2 1.2M11.2 11.2l1.2 1.2M12.4 3.6l-1.2 1.2M4.8 11.2l-1.2 1.2"/>',
 "chev": '<path d="M4 6l4 4 4-4"/>',
 "left": '<path d="M10 3 5 8l5 5"/>',
 "right": '<path d="M6 3l5 5-5 5"/>',
 "up": '<path d="M4 10l4-4 4 4"/>',
 "down": '<path d="M4 6l4 4 4-4"/>',
 "x": '<path d="M4 4l8 8M12 4l-8 8"/>',
 "check": '<path d="M3.5 8.5l3 3 6-7"/>',
 "info": '<circle cx="8" cy="8" r="6"/><path d="M8 7.2v3.6M8 5.2v.1"/>',
 "warn": '<path d="M8 2.2 14.2 13H1.8z"/><path d="M8 6.5v3.2M8 11.4v.1"/>',
 "send": '<path d="M8 13V3M3.5 7.5 8 3l4.5 4.5"/>',
 "stop": '<rect x="4" y="4" width="8" height="8" rx="1.5"/>',
 "undo": '<path d="M5 3 2.5 5.5 5 8M3 5.5h6a4 4 0 0 1 0 8H6"/>',
 "bell": '<path d="M4 11V7a4 4 0 0 1 8 0v4l1 1.2H3zM6.5 13.8h3"/>',
 "mac": '<rect x="2" y="3" width="12" height="8" rx="1.5"/><path d="M5.5 13.5h5M8 11v2.5"/>',
 "sliders": '<path d="M3 5h6M11 5h2M3 11h2M7 11h6"/><circle cx="10" cy="5" r="1.5"/><circle cx="6" cy="11" r="1.5"/>',
 "pause": '<path d="M5.5 3.5v9M10.5 3.5v9"/>',
 "grip": '<path d="M6 4h.01M10 4h.01M6 8h.01M10 8h.01M6 12h.01M10 12h.01" stroke-width="2"/>',
 "pr": '<circle cx="4.5" cy="4" r="1.6"/><circle cx="4.5" cy="12" r="1.6"/><circle cx="11.5" cy="12" r="1.6"/><path d="M4.5 5.6v4.8M11.5 10.4V7a2.5 2.5 0 0 0-2.5-2.5H7.5"/>',
}
def ic(n, s=16, cls=""):
    return f'<svg class="{cls}" width="{s}" height="{s}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{P[n]}</svg>'

_g = itertools.count()
def mark(w=26):
    h = round(w * 369 / 600); gid = f"brassM{next(_g)}"
    return (f'<svg width="{w}" height="{h}" viewBox="218 338 600 369" role="img" aria-label="Ratchet"><defs><linearGradient id="{gid}" x1="0" y1="0" x2="1" y2="1">'
     f'<stop offset="0" style="stop-color:var(--markHi)"/><stop offset=".6" style="stop-color:var(--markMid)"/><stop offset="1" style="stop-color:var(--markLo)"/></linearGradient></defs>'
     '<g style="fill:var(--markSlate)"><path d="M397.8 434.8L397.8 466.2A31.5 31.5 0 0 1 366.4 434.8Z" fill-opacity="0.12"/><path d="M407.7 434.8L407.7 466.2A31.5 31.5 0 0 0 439.2 434.8Z" fill-opacity="0.18"/>'
     '<path d="M361.6 424.9L439.2 424.9A77.6 77.6 0 0 0 361.6 347.3Z" fill-opacity="0.28"/><path d="M351.7 471.0L227.9 471.0A123.8 123.8 0 0 1 351.7 347.3Z" fill-opacity="0.42"/>'
     f'<path d="M444.0 480.9L444.0 697.0A216.1 216.1 0 0 1 227.9 480.9Z" fill-opacity="0.6"/></g><path d="M453.9 342.5L453.9 697.0A354.5 354.5 0 0 0 808.4 342.5Z" fill="url(#{gid})"/></svg>')

# status = shape + word (never colour alone)
ST = {
 "ok":   '<circle cx="7" cy="7" r="6"/><path d="M4.3 7.2l1.9 1.9 3.5-3.9"/>',
 "run":  '<circle cx="7" cy="7" r="6" opacity=".3"/><path class="arc" d="M7 1a6 6 0 0 1 6 6"/>',
 "wait": '<circle cx="7" cy="7" r="6"/><path d="M7 1a6 6 0 0 1 0 12z" fill="currentColor"/>',
 "bad":  '<circle cx="7" cy="7" r="6"/><path d="M4.8 4.8l4.4 4.4M9.2 4.8l-4.4 4.4"/>',
 "off":  '<circle cx="7" cy="7" r="6"/><path d="M4.5 7h5"/>',
 "q":    '<circle cx="7" cy="7" r="6" stroke-dasharray="2.2 2.2"/>',
}
def st(kind, label="", cls=""):
    svg = f'<svg class="stt-ic" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{ST[kind]}</svg>'
    return f'<span class="stt stt-{kind} {cls}">{svg}<span>{label}</span></span>' if label else f'<span class="stt stt-{kind} {cls}">{svg}</span>'

def A(go=None, toast=None, pop=None, pane=None, swap=None):
    s = ""
    if go: s += f' data-go="{go}"'
    if toast: s += f' data-toast="{toast}"'
    if pop: s += f' data-pop="{pop}"'
    if pane: s += f' data-pane="{pane}"'
    if swap: s += f' data-swap="{swap}"'
    return s

def btn(label, cls="", icon=None, aria=None, **a):
    al = f' aria-label="{aria}" title="{aria}"' if aria else ""
    return f'<button type="button" class="btn {cls}"{al}{A(**a)}>{ic(icon,14) if icon else ""}{label}</button>'
def ibtn(icon, label, cls="", **a):
    return f'<button type="button" class="ibtn {cls}" aria-label="{label}" title="{label}"{A(**a)}>{ic(icon,16)}</button>'
def more(label, **a):
    a.setdefault("toast", "Make default · Rename · Remove")
    return (f'<button type="button" class="more" aria-label="{label}" title="{label}"{A(**a)}>'
            '<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"><circle cx="3.5" cy="8" r="1.3"/><circle cx="8" cy="8" r="1.3"/><circle cx="12.5" cy="8" r="1.3"/></svg></button>')
def chip(label, pop, open_=False, extra=""):
    return (f'<button type="button" class="chip{" open" if open_ else ""}" aria-haspopup="dialog" aria-expanded="{str(open_).lower()}" data-pop="{pop}"{extra}>{label}{ic("chev",12)}</button>')
def tog(on=False, label="", **a):
    return f'<button type="button" class="tog{" on" if on else ""}" role="switch" aria-checked="{str(on).lower()}" aria-label="{label}"></button>'
def seg(items, cur, label="", gos=None):
    out = f'<span class="seg2" role="tablist" aria-label="{label}">'
    for i, t in enumerate(items):
        go = f' data-go="{gos[i]}"' if gos and gos[i] else ""
        out += f'<span role="tab" tabindex="0" aria-selected="{str(t==cur).lower()}" class="{"on" if t==cur else ""}"{go}>{t}</span>'
    return out + "</span>"

# ---------------------------------------------------------------- chrome
RUNS = [
 ("14", "Checkout p95", "run", "running · 1 h 12 m left", "s2"),
 ("31", "Search relevance", "wait", "PR #311 open · +5.9%", "s2"),
 ("9",  "Forecast accuracy", "ok", "merged · −13%", "s2"),
 ("6",  "Bundle size", "off", "no win · within noise", "s2"),
 ("2",  "CLI cold start", "bad", "failed · AWS session expired", "s13"),
 ("15", "Checkout p95", "q", "queued · 02:00", "s3"),
]
NAV = [("Home", "home", "s1"), ("Overview", "gauge", "s9"), ("Optimizations", "list", "s3"), ("Customize", "bag", "s4")]

def side(nav, run=None, empty=False, loading=False):
    h = f'<nav class="side" aria-label="Sidebar"><div class="lights" aria-hidden="true"><i></i><i></i><i></i></div><div class="brand">{mark(26)}<span class="serif" style="font-size:22px">Ratchet</span></div>'
    for n, i, go in NAV:
        cur = ' aria-current="page"' if n == nav else ""
        h += f'<div class="nv{" on" if n==nav else ""}" role="link" tabindex="0" data-go="{go}"{cur}>{ic(i,16)}<span>{n}</span></div>'
    h += '<div class="sec">Recent runs</div>'
    if empty:
        h += '<div class="empty-side">No runs yet.<br>Runs appear here.</div>'
    elif loading:
        for _ in range(5):
            h += '<div class="run" aria-hidden="true"><div class="skel" style="width:14px;height:14px;border-radius:7px"></div><div style="flex:1"><div class="skel" style="height:11px;width:70%"></div><div class="skel" style="height:11px;width:50%;margin-top:6px"></div></div></div>'
    else:
        for num, name, k, meta, go in RUNS:
            on = run == num
            lab = f"Run #{num} {name}, {meta.replace(' · ', ', ')}"
            h += (f'<div class="run{" on" if on else ""}" role="link" tabindex="0" aria-label="{lab}"{" aria-current=\"true\"" if on else ""} data-go="{go}">'
                  f'{st(k)}<div style="flex:1;min-width:0"><div class="ttl"><span class="num">#{num}</span>{name}</div><div class="meta">{meta}</div></div></div>')
    return h + "</nav>"

def toolbar(title, new="sec", search="Search", status=True, newgo="s1"):
    s = ""
    if status:
        s += f'<span class="pillst" role="button" tabindex="0" title="Sandbox on · acme-prod · $1.20 so far" data-toast="Sandbox on · acme-prod · $1.20 so far">{st("ok")}Sandbox on · $1.20</span>'
    s += f'<span class="field" role="search" tabindex="0" data-toast="Search: ⌘K" title="Search (⌘K)">{ic("search",14)}<span>{search}</span><kbd>⌘K</kbd></span>'
    s += ibtn("gear", "Settings (⌘,)", go="s6")
    cls = {"pri": "pri", "sec": "", "dis": "dis"}[new]
    dis = " disabled" if new == "dis" else ""
    s += f'<button type="button" class="btn {cls}" title="New Optimization (⌘N)"{dis}{A(go=newgo)}>{ic("plus",14)}New Optimization</button>'
    return f'<header class="tb" role="toolbar" aria-label="Window toolbar"><span class="tbt">{title}</span>{s}</header>'

def banner(kind, text, actions="", dismiss=True):
    icon = "warn" if kind in ("bad", "wait") else "info"
    x = ibtn("x", "Dismiss", "sq x", toast="Dismissed") if dismiss else ""
    return f'<div class="banner {kind}" role="alert">{ic(icon,16)}<span>{text}</span>{actions}{x}</div>'

def window(sidebar, tb, main, banners="", extra="", cls=""):
    return (f'<div class="win {cls}">{sidebar}<div class="col">{tb}<div class="banners">{banners}</div>'
            f'<main class="main">{main}</main></div>{extra}</div>')

def screen(sid, name, body, theme="dark"):
    return f'<section class="screen" id="{sid}" data-name="{name}" data-theme="{theme}" aria-label="{name}">{body}</section>\n'

def page(inner, w=1000, mid=False, cls="", style=""):
    return f'<div class="page{" mid" if mid else ""} {cls}" style="--w:{w}px;{style}">{inner}</div>'

# ---------------------------------------------------------------- shared blocks
def agents_pop(pid, open_=True, style="", up=False, left=False, lft=False):
    rows = [("1", "Claude Code · your login"), ("2", "Codex · ci key"), ("3", "Grok · your login")]
    r = "".join(f'<div class="po"><span class="n">{n}</span><span class="t">{t}</span>'
                f'{ibtn("up", "Move "+t.split(" ·")[0]+" up", "sq", toast="Moved up")}{ibtn("down", "Move "+t.split(" ·")[0]+" down", "sq", toast="Moved down")}</div>' for n, t in rows)
    return (f'<div class="pop{" open" if open_ else ""}{" up" if up else ""}{" left" if left else ""}{" lft" if lft else ""}" id="{pid}" role="dialog" aria-label="Change agents" style="{style}">'
            f'<h3>Change agents</h3>{r}<div class="pf"><span class="s">Where</span>{seg(["This Mac","Cloud"],"This Mac","Where agents run")}<span class="sp"></span></div>'
            '<div class="hint">If one is out of quota or signed out, the next one takes over.</div>'
            f'<div class="pf"><span class="sp"></span>{btn("Done","sm",pop=pid)}</div></div>')

ORDER = "Claude Code → Codex → Grok · on this Mac"

def story_line():
    return '<div class="storyline">Value per year: <b>$12.5k delivered + $2.9k pending · 68×</b></div>'

CHART = '''<figure style="margin:0"><svg viewBox="-4 -14 732 240" width="100%" style="display:block;overflow:visible" role="img" aria-labelledby="chT chD">
<title id="chT">Value delivered versus spend, 90 days</title><desc id="chD">Delivered value rose in three steps: CLI cold start to $1.2k in August, Forecast accuracy to $6.5k in September, Checkout p95 to $12.5k in October. A further $2.9k is pending if PR 311 merges. Spend stayed under $184.</desc>
<defs><linearGradient id="ovA" x1="0" y1="0" x2="0" y2="1"><stop offset="0" style="stop-color:var(--brass);stop-opacity:.22"/><stop offset="1" style="stop-color:var(--brass);stop-opacity:0"/></linearGradient></defs>
<g font-size="11" font-family="JetBrains Mono,monospace" style="fill:var(--textFaint)"><line x1="0" x2="680" y1="200" y2="200" stroke="var(--hairlineStrong)"/><text x="690" y="204">$0</text><line x1="0" x2="680" y1="137.5" y2="137.5" stroke="var(--hairline)"/><text x="690" y="141.5">$5k</text><line x1="0" x2="680" y1="75" y2="75" stroke="var(--hairline)"/><text x="690" y="79">$10k</text><line x1="0" x2="680" y1="12.5" y2="12.5" stroke="var(--hairline)"/><text x="690" y="16.5">$15k</text></g>
<g font-size="11" style="fill:var(--textFaint)"><text x="0" y="222">Aug 2</text><text x="226.7" y="222" text-anchor="middle">Sep 1</text><text x="453.3" y="222" text-anchor="middle">Oct 1</text><text x="680" y="222" text-anchor="end">Oct 31</text></g>
<path d="M0 200 H75.6 V185 H241.8 V103.8 H513.8 V43.8 H680 V200 Z" fill="url(#ovA)"/>
<path d="M0 200 H75.6 V185 H241.8 V103.8 H513.8 V43.8 H680" fill="none" style="stroke:var(--brass)" stroke-width="2"/>
<path d="M657.3 43.8 V7.5 H680" fill="none" style="stroke:var(--brassText)" stroke-width="1.5" stroke-dasharray="4 4"/>
<text x="649" y="11.5" font-size="12.5" text-anchor="end" style="fill:var(--brassText)">+$2.9k pending if PR #311 merges</text>
<polyline points="0,200 75.6,199.6 241.8,199.1 415.6,198.6 513.8,198.2 680,197.7" fill="none" style="stroke:var(--bad)" stroke-width="1.5"/>
<text x="676" y="186" font-size="12.5" text-anchor="end" style="fill:var(--bad)">Spent $184</text>
<g font-size="12.5" style="fill:var(--textMuted)"><circle cx="75.6" cy="185" r="3.5" style="fill:var(--brass)"/><text x="83.6" y="177">CLI cold start</text><circle cx="241.8" cy="103.8" r="3.5" style="fill:var(--brass)"/><text x="249.8" y="95.8">Forecast accuracy</text><circle cx="513.8" cy="43.8" r="3.5" style="fill:var(--brass)"/><text x="521.8" y="35.8">Checkout p95</text></g></svg>
<figcaption class="sr">Value delivered rose in three steps to $12.5k; $2.9k is pending; spend $184.</figcaption></figure>'''

def ov_table():
    def r(name, repo, stk, stl, sub, best, spent, val, ret, pend=False):
        v = f'<span class="faint">{val}</span>' if pend else val
        rr = (f'<span class="faint">{ret} pending</span>' if pend else ret)
        return (f'<tr><td>{name}<div class="s">{repo}</div></td><td>{st(stk, stl)}<div class="s" style="padding-left:22px">{sub}</div></td><td>{best}</td>'
                f'<td class="r mono">{spent}</td><td class="r mono">{v}</td><td class="r mono brasst">{rr}</td></tr>')
    rows = (r("Forecast accuracy","acme-org/demand-model","ok","Merged","Weekly",'<span class="up mono">−13%</span> <span class="s">WAPE 18.2 → 15.9%</span>',"$64","$6.5k","102×")
          + r("Checkout p95","acme-org/shop-api","ok","Merged","Nightly · running now",'<span class="up mono">−30%</span> <span class="s">412 → 288 ms</span>',"$38","$4.8k","126×")
          + r("Search relevance","acme-org/search-svc","wait","PR #311 open","Nightly",'<span class="up mono">+5.9%</span> <span class="s">nDCG 0.612 → 0.648</span>',"$22","$2.9k","132×",True)
          + r("CLI cold start","srikanth235/acme-cli","ok","Merged","Once",'<span class="up mono">−39%</span> <span class="s">840 → 510 ms</span>',"$41","$1.2k","29×")
          + r("Bundle size","acme-org/web-app","off","No win","On push to main",'<span class="s">no win yet · within noise (±2%)</span>',"$19",'<span class="s">—</span>','<span class="s">—</span>'))
    return ('<div><h2>Return by optimization</h2><table><thead><tr><th style="width:24%">Optimization</th><th>Status</th><th>Best result</th><th class="r">Spent</th><th class="r">Value / yr</th><th class="r">Return</th></tr></thead>'
            f'<tbody>{rows}</tbody></table><div class="s" style="margin-top:8px">Estimated from each optimization\'s Value row. <button type="button" class="link" data-go="s3">Edit value</button></div></div>')

# ---------------------------------------------------------------- screens
def home(theme="dark", focus=True):
    strip = ('<div class="strip"><div><div class="l">Spent · 90 days</div><div class="v serif">$184</div></div>'
             '<div><div class="l">Value per year</div><div class="v serif">$12.5k <small style="font-size:14px;color:var(--textMuted);font-family:\'Instrument Sans\',sans-serif">delivered + $2.9k pending · 68×</small></div></div></div>'
             f'<div class="s" style="display:flex;gap:8px;align-items:center;margin-top:-16px">3 optimizations · 2 PRs this week · {st("bad","1 run failed")} <button type="button" class="link" data-go="s13">CLI cold start</button></div>')
    chips = (chip("acme-org/shop-api","p1") + chip("acme-prod · AWS","p2") + chip("Claude Code (2 logins)","pAg",open_=True)
             + chip("$50 · 2 h per run","p4") + chip("Nightly 02:00","p5")
             + f'<button type="button" class="send" aria-label="Send (⌘↩)" title="Send (⌘↩)" data-go="s8">{ic("send",16)}</button>')
    comp = (f'<div class="composer{" focus" if focus else ""}" style="position:relative" role="group" aria-label="Describe an optimization"><div class="ph">Make checkout p95 faster in shop-api. Run it every night.</div><div class="chips">{chips}</div>'
            + agents_pop("pAg", True, "top:calc(100% + 8px);left:252px", left=True) + '</div>')
    inner = (strip + '<div style="margin-top:8px;text-align:center"><div class="serif" style="font-size:34px">What should get better?</div>'
             '<div class="sub" style="font-size:14px">Describe it once. Ratchet runs it and opens a pull request with what it proves.</div></div>'
             + comp + '<div class="sugg"><span data-go="s8" role="button" tabindex="0">Cut CLI cold start</span><span data-go="s8" role="button" tabindex="0">Shrink the bundle</span><span data-go="s8" role="button" tabindex="0">Speed up the test suite</span><span data-go="s8" role="button" tabindex="0">Improve forecast accuracy</span><span data-go="s8" role="button" tabindex="0">Try a demo</span></div>')
    return window(side("Home"), toolbar("Home", "sec"), page(inner, 820, True))

def overview(theme="dark", mode="ok"):
    hero = ('<div class="hero3"><div><div class="s">Spent</div><div class="serif big">$184</div><div class="bar"><i style="width:37%"></i></div><div class="s" style="margin-top:8px">of $500 cap · AWS $121 · agents $63</div></div>'
            '<div><div class="s">Delivered</div><div class="serif big">$12.5k<small> / yr</small></div><div class="s" style="margin-top:20px">3 merged wins · +$2.9k pending in 1 PR</div></div>'
            '<div><div class="s">Return</div><div class="serif big brasst">68×</div><div class="s" style="margin-top:20px">per $1 spent, yearly · paid back in 6 days</div></div></div>')
    right = ('<div style="display:flex;flex-direction:column;gap:20px"><div><h2>Pending value</h2><div class="card"><div class="serif" style="font-size:30px">$2.9k<small class="muted" style="font-size:14px"> / yr</small></div>'
             f'<div class="s" style="margin-top:4px">Search relevance · +5.9% nDCG, verified. Counts once PR #311 merges.</div><div style="margin-top:12px">{btn("Open PR #311","sm",icon="pr",toast="Opens PR #311 on GitHub")}</div></div></div>'
             '<div><h2>Next 30 days</h2><div class="kv compact" style="grid-template-columns:1fr auto"><div class="k">Scheduled runs</div><div class="mono">62</div><div class="k">Expected spend</div><div class="mono">~$140</div><div class="k">Hard cap</div><div class="mono">$500</div></div></div>'
             f'<div><h2>Sandbox</h2><div class="kv compact" style="grid-template-columns:1fr auto"><div class="k">{st("ok","On · acme-prod")}</div><div class="mono">$1.20</div></div><div style="margin-top:8px">{btn("End Now","sm",toast="Ends the sandbox after the run finishes")}</div></div></div>')
    inner = (f'<div class="head" style="display:flex;align-items:center;gap:12px"><h1 class="serif">Overview</h1><span class="sp"></span>{seg(["30 days","90 days","All time"],"90 days","Range")}</div>'
             + hero + story_line()
             + f'<div style="display:grid;grid-template-columns:1fr 296px;gap:40px"><div><h2>Value delivered vs spend</h2>{CHART}</div>{right}</div>' + ov_table())
    return inner

def overview_win(theme="dark"):
    return window(side("Overview"), toolbar("Overview", "pri"), page(overview(theme), 1000))

def overview_empty():
    inner = ('<h1 class="serif">Overview</h1><div style="flex:1;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:12px;text-align:center;padding-bottom:80px">'
             f'{mark(72)}<div class="serif" style="font-size:30px;margin-top:8px">No wins yet</div><div class="muted">Start your first optimization.</div>'
             f'<div style="margin-top:8px">{btn("New Optimization","pri",icon="plus",go="s1")}</div><div class="s">Or <button type="button" class="link" data-go="s8">try a demo</button> first.</div></div>')
    return window(side("Overview", empty=True), toolbar("Overview", "sec", status=False), page(inner, 1000))

def overview_down():
    b = banner("bad", "<b>Ratchet service isn't running.</b> Data below may be out of date · last updated 14:02.",
               btn("Start Service", "sm", toast="Starting the background service…") + btn("Details","sm ghost",toast="launchctl: ratchetd not loaded"), dismiss=False)
    return window(side("Overview"), toolbar("Overview", "dis"), page(f'<div class="stale" aria-disabled="true" style="display:flex;flex-direction:column;gap:32px">{overview()}</div>', 1000), banners=b)

def overview_loading():
    sk = lambda w, h=14, m=0: f'<div class="skel" style="width:{w};height:{h}px;margin-top:{m}px"></div>'
    cell = lambda: f'<div>{sk("30%",12)}{sk("60%",52,8)}{sk("100%",4,16)}{sk("70%",12,10)}</div>'
    inner = (f'<div style="display:flex;align-items:center;gap:12px"><h1 class="serif">Overview</h1><span class="sp"></span><span class="stt stt-run" role="status">{st("run")}<span>Loading your optimizations…</span></span></div>'
             f'<div class="hero3" aria-busy="true">{cell()}{cell()}{cell()}</div>'
             f'<div style="display:grid;grid-template-columns:1fr 296px;gap:40px"><div>{sk("100%",220)}</div><div>{sk("100%",120)}{sk("100%",80,16)}</div></div>'
             f'<div>{sk("100%",14)}{sk("100%",14,16)}{sk("100%",14,16)}{sk("100%",14,16)}</div>')
    return window(side("Overview", loading=True), toolbar("Overview", "dis"), page(inner, 1000))

# ---- Run
def run_win(theme="dark", variant="main"):
    hdr = ('<div class="rh"><div><div class="serif" style="font-size:28px;line-height:1.2">Checkout p95</div>'
           f'<div class="sub" style="margin-top:4px">acme-org/shop-api · run #14 · started 00:48 · 1 h 12 m left · $38 of $50</div></div>'
           f'<div class="acts">{st("run","Running")}'
           + btn("Stop", "bad", icon="stop", go="s19", aria="Stop this run")
           + btn("Pause Schedule", "", icon="pause", swap="Pause Schedule|Resume Schedule", toast="Schedule paused · no new runs until you resume")
           + btn("Open PR #313", "", icon="pr", toast="Opens PR #313 on GitHub") + '</div></div>')
    views = f'<div style="display:flex;align-items:center;gap:12px;position:relative">{seg(["Overview","By Agent","Everything"], "By Agent" if variant=="agent" else "Overview", "Log view", ["s2","s20","s2"])}'
    if variant == "main":
        views += '<span class="tip" style="left:96px;top:36px">Agents, one lane each</span>'
    views += '<span class="s">What the run did, step by step</span></div>' if variant != "agent" else '<span class="s">One lane per agent</span></div>'
    if variant == "agent":
        def lane(t, state, body, act=""):
            return f'<div class="lane"><div class="lh">{st(state[0], state[1])}<b>{t}</b><span class="sp"></span>{act}</div>{body}</div>'
        body = ('<div style="display:flex;flex-direction:column;gap:12px">'
            + lane("Claude Code · your login", ("run", "Working"), '<div class="muted">Candidate 3 · <i>Skip the audit write on the hot path</i> · blocked, touched a protected path</div><div class="s">Candidate 4 on the bench · best so far 288 ms (−30%)</div>', btn("Pause Agent", "sm", icon="pause", swap="Pause Agent|Resume Agent", toast="Agent paused"))
            + lane("Claude Code · team key", ("ok", "Idle"), '<div class="muted">Candidate 1 · <i>Pool size 20 → 48, reuse connections</i> · locked in, −16.5%</div>', btn("Pause Agent", "sm", icon="pause", swap="Pause Agent|Resume Agent", toast="Agent paused"))
            + '<div class="lane"><div class="lh">' + st("q", "Not started") + '<b>Grok · your login</b></div><div class="emptyst">No activity yet. Grok starts when a slot is free.</div></div></div>')
    else:
        ev = lambda t, b: f'<div class="ev"><div class="t">{t}</div><div class="b">{b}</div></div>'
        body = ('<div style="display:flex;flex-direction:column;gap:4px">'
          + ev("00:48", "<b>Environment</b> · full stack passed every check and was locked on <span class=\"mono\">ratchet/env</span> · $0.59/h")
          + ev("00:57", "<b>Baseline</b> · 412 ms p95 over 9 runs · noise ±2.1%, so a win must beat 4.2%")
          + ev("01:10", '<div class="agentmsg"><b>Claude Code · team key</b> proposes <i>Pool size 20 → 48, reuse connections</i> · 2 files</div>')
          + ev("01:22", f'<div class="card verdict"><div class="sealed">{mark(16)} Evaluator verdict · sealed</div><div style="display:flex;align-items:baseline;gap:12px;margin-top:8px"><span class="serif" style="font-size:30px">−16.5%</span><span class="muted">412 → 344 ms p95 · confirmed on a fresh host · <b style="color:var(--ok);font-weight:500">locked in</b></span></div><div class="s" style="margin-top:4px">Holdout passed · guardrails: error rate 0.02%, memory +3 MB</div></div>')
          + ev("01:31", '<div class="agentmsg"><b>Claude Code · your login</b> proposes <i>Skip the audit write on the hot path</i></div>')
          + ev("01:33", f'<div class="card" style="border-color:var(--bad)"><div class="sealed" style="color:var(--bad)">{st("bad","Blocked before measuring")}</div><div class="muted" style="margin-top:8px">Touched a protected path: <span class="mono">bench/latency.js</span>. Agents can\'t change the evaluator.</div></div>')
          + ev("02:00", f'{st("run")} 2 candidates on the bench · best so far <b>288 ms</b> (−30%)')
          # steering timeline with read receipt
          + ev("02:05", f'<div class="note"><b>You</b> · “focus on database queries” <span class="rcpt">{st("wait","Queued · read at the next candidate")}</span> {btn("Undo","sm ghost",icon="undo",toast="Note withdrawn")}</div>')
          + ev("02:09", f'<div class="note"><b>You</b> · “skip the cache layer, it is owned by another team” <span class="rcpt">{st("ok","Read by Claude Code at 02:11")}</span></div>')
          + '</div>')
    comp = ('<div class="composer steer" style="margin-top:auto"><div class="ph" style="font-size:14px">Leave a note to steer the next candidate, e.g. “focus on database queries”. Notes are read between candidates.</div>'
            f'<div class="chips"><span class="s">Return adds a line · ⌘↩ sends</span><button type="button" class="send" aria-label="Send note (⌘↩)" title="Send note (⌘↩)" data-toast="Note queued · read at the next candidate">{ic("send",16)}</button></div></div>')
    inner = hdr + views + body
    pg = page(inner, 880, cls="runpg")
    # composer sits at the bottom, inside the same column width
    pg = pg[:-6] + comp + "</div>"
    extra = ""
    if variant == "stop":
        extra = ('<div class="overlay"><div class="dialog" role="alertdialog" aria-labelledby="dg"><h3 id="dg">Stop Checkout p95?</h3><p>Wins found so far are kept.</p>'
                 f'<div class="bs">{btn("Cancel","",go="s2")}{btn("Stop","bad",go="s3",toast="Stopped · wins so far are kept")}</div></div></div>')
        hdr_variant = "main"
    return window(side("Overview" if False else "Optimizations", run="14"), toolbar("#14 Checkout p95", "pri"), pg, extra=extra)

# ---- Optimization
def optimization_win():
    def row(k, v, tag, saved=False, popid=None, pop_html="", label=""):
        t = f'<span class="tagc{" cus" if tag=="Custom" else ""}">{"Using default" if tag=="Default" else tag}</span>' if tag else ""
        sv = f'<span class="saved">{ic("check",12)}Saved</span>' if saved else ""
        e = f'<div class="rowedit" style="position:relative">{sv}{t}{btn("Edit","sm",pop=popid or "x", aria=f"Edit {label}") if popid else btn("Edit","sm",toast=f"Edit {label} inline", aria=f"Edit {label}")}{pop_html}</div>'
        return f'<div class="k">{k}</div><div>{v}</div>{e}'
    pop = agents_pop("pOpAg", True, "top:-24px;right:calc(100% + 8px)", lft=True)
    kv = ('<div class="kv rows">'
      + row("Measures", 'p95 of <span class="mono">POST /checkout</span> · lower is better', "Custom", saved=True, label="measure")
      + row("Agents", ORDER, "Custom", popid="pOpAg", pop_html=pop, label="agents")
      + row("Cloud", "acme-prod · us-east-1", "Default", label="cloud")
      + row("Each run", "up to <b>$50</b> and <b>2 h</b> · stops at either limit", "Default", label="limits")
      + row("Schedule", "every night at 02:00 · next run in 3 h", "Custom", label="schedule")
      + row("Value", "ms saved × 48k checkouts a day → <b>$4.8k</b> / yr", "Custom", label="value")
      + row("Output", "pull request with verified wins · auto-merge off", "Default", label="output")
      + row("Notify", "PR opened · run failed · budget 80% · connector expiring", "Default", label="notifications")
      + '</div>')
    runs = ('<div><h2>Runs</h2><table><tbody>'
      f'<tr data-go="s2" style="cursor:pointer"><td>{st("run","#14 · tonight")}</td><td class="s">running · 1 h 12 m left</td><td class="r mono">$38</td><td class="r"><span class="up mono">−30%</span></td></tr>'
      f'<tr data-go="s2" style="cursor:pointer"><td>{st("wait","#13 · yesterday")}</td><td class="s">PR #307 open</td><td class="r mono">$44</td><td class="r"><span class="up mono">−8%</span></td></tr>'
      f'<tr data-go="s2" style="cursor:pointer"><td>{st("off","#12 · Oct 29")}</td><td class="s">no win · nothing beat the noise</td><td class="r mono">$31</td><td class="r s">—</td></tr></tbody></table></div>')
    arch = ('<div style="display:flex;align-items:center;gap:16px;border-top:1px solid var(--hairline);padding-top:16px"><div><b>Archive optimization</b><div class="s">Stops the schedule and keeps its history and wins.</div></div><span class="sp"></span>'
            + btn("Archive…", "", toast="Archive Checkout p95 · nightly? History is kept.") + '</div>')
    hd = ('<div class="rh"><div><div class="s">Optimization · acme-org/shop-api · main</div><div class="serif" style="font-size:28px;line-height:1.2">Checkout p95 · nightly</div></div>'
          f'<div class="acts">{st("run","Running")}{btn("Run Now","",icon="send",go="s2",toast="Run started")}{btn("Pause Schedule","",icon="pause",swap="Pause Schedule|Resume Schedule",toast="Schedule paused")}</div></div>')
    leg = '<div class="s" style="margin-top:-16px">Defaults come from Settings → Defaults for new optimizations. “Custom” rows win over them.</div>'
    inner = hd + leg + kv + runs + arch
    return window(side("Optimizations", run="14" if False else None), toolbar("Checkout p95 · nightly", "pri"), page(inner, 860))

# ---- Customize
def customize(tab, mode, body, search, add, w=960):
    tabs = "".join(f'<span role="tab" tabindex="0" aria-selected="{str(t==tab).lower()}" class="{"on" if t==tab else ""}" data-go="{go}">{t}</span>'
                   for t, go in (("Templates", "s4"), ("Connectors", "s5"), ("Agents", "s7")))
    head = f'<div style="display:flex;align-items:center;gap:12px"><h1 class="serif">Customize</h1></div>'
    row2 = f'<div style="display:flex;align-items:flex-end;gap:12px"><div class="tabs" role="tablist" aria-label="Customize" style="flex:1">{tabs}</div>{btn(add,"",toast="Add a "+add[4:-1].lower()+"…")}</div>'
    row3 = ""
    if tab != "Agents":
        gos = ("s4" if tab == "Templates" else "s5", "s10" if tab == "Connectors" else "s4")
        row3 = f'<div>{seg(["Yours","Discover"], mode, "Show", gos)}</div>'
    inner = head + row2 + (row3 or "") + body
    return window(side("Customize"), toolbar("Customize", "sec" if tab == "Templates" else "pri", search=search), page(inner, w))

def templates():
    def tile(ico, n, d):
        return (f'<div class="tile"><span class="ico" aria-hidden="true">{ico}</span><div><div class="n">{n}</div><div class="d">{d}</div></div>'
                f'<button type="button" class="plus" aria-label="Add template: {n}" title="Add {n} to Yours" data-toast="{n} added to Yours">{ic("plus",14)}</button></div>')
    hero = ('<div class="hero"><div style="flex:1"><div class="s">Featured template</div><div class="serif" style="font-size:28px;margin-top:4px">Measure your API\'s p95</div>'
            '<div class="muted" style="margin-top:8px">A load test on a separate host, a hidden query mix as the holdout, and an error-rate guardrail.</div>'
            f'<div style="margin-top:16px;display:flex;gap:12px;align-items:center">{btn("Use Template","pri",go="s8")}<span class="s">Opens a chat with this template chosen.</span></div></div>{mark(150)}</div>')
    tiles = ('<div><h2>For your repos</h2><div class="grid2c">' + tile("⏱","CLI cold start","Wall time of repeated invocations with hyperfine; hidden invocations as holdout.")
        + tile("⧉","Bundle size","Build seconds and bundle KB for web apps.") + tile("✓","Test suite time","Faster CI without skipping tests: every test must still pass.")
        + tile("∿","Forecast accuracy","WAPE, MAPE and bias, backtested on the most recent periods.") + tile("◎","LLM eval","Score and cost per call on a hidden eval set.")
        + tile("▤","Memory","Peak RSS under a hidden workload.") + '</div></div>')
    return customize("Templates", "Discover", hero + tiles, "Search templates", "Add Template…")

def crow(logo, name, desc, k, label, dflt=False, fix=None, err=None, focus=False):
    d = '<span class="dflt">default</span>' if dflt else ""
    fx = ""
    if fix:
        fx = btn(fix[0], "sm" + (" focus" if focus else ""), toast=fix[1])
    e = f'<div class="errline">{ic("warn",14)}{err}</div>' if err else ""
    return (f'<div class="row"><span class="logo" aria-hidden="true">{logo}</span><div style="flex:1;min-width:0"><div class="n">{name}{d}</div><div class="d">{desc}</div>{e}</div>'
            f'{st(k,label)}<span style="width:96px;display:flex;justify-content:flex-end">{fx}</span>{more("More actions for "+name)}</div>')

def connectors(fail=False):
    g = lambda t, d: f'<div class="grp" style="margin-top:20px"><b>{t}</b> {d}</div>'
    awsfix = ("Sign In", "Opens AWS sign-in in your browser")
    body = (g("GitHub", "where optimizations read code and open pull requests").replace("margin-top:20px", "margin-top:0")
      + crow("GH","srikanth235","GitHub · Personal · signed in · 42 repos","ok","Connected",True)
      + (crow("GH","acme-org","GitHub · App · 18 repos selected","wait","Needs permission",fix=("Reconnect","Opens GitHub to approve the new permission"),err="Can't open pull requests: “contents: write” was removed. Reconnect to restore it.") if fail
         else crow("GH","acme-org","GitHub · App · 18 repos selected","ok","Connected"))
      + g("Cloud", "where the evaluator and environments run")
      + crow("AWS","acme-prod","Amazon Web Services · SSO · us-east-1 · cap $500 / month · $412 left","ok","Connected",True)
      + crow("AWS","personal-dev","Amazon Web Services · ~/.aws profile · us-west-2 · cap $100 / month","ok","Connected")
      + crow("AWS","acme-staging","Amazon Web Services · SSO · eu-west-1 · used by “CLI cold start”","bad","Expired",fix=awsfix,
             err="Session expired Oct 31. “CLI cold start” failed on it." if fail else None, focus=fail)
      + g("Notifications", "where results go")
      + (crow("#","Slack · #perf-wins","Slack · pull requests opened and failed runs","bad","Failing",fix=("Fix","Re-send test message"),err="Webhook returned 404 — the channel was archived. Pick another channel.") if fail
         else crow("#","Slack · #perf-wins","Slack · pull requests opened and failed runs","ok","Connected")))
    inner = customize("Connectors", "Yours", body, "Search connectors", "Add Connector…")
    return inner

def connectors_fail():
    body = connectors(True)
    return body

def agents():
    g = lambda t, d, extra="": f'<div class="grp" style="margin-top:20px"><b>{t}</b> {d}{extra}</div>'
    order = ('<div class="row" style="min-height:56px"><span class="logo" aria-hidden="true">→</span><div style="flex:1"><div class="n">Order</div><div class="d">' + ORDER + '</div></div>'
             + '<div style="position:relative">' + btn("Edit Order","sm",pop="pAgPg") + agents_pop("pAgPg", False, "top:calc(100% + 8px);right:0") + '</div></div>')
    body = (order
      + g("On this Mac", "your own logins · free while the Mac is awake", f' <button type="button" class="link" data-toast="Found Claude Code, Codex, Grok">Re-detect</button>')
      + crow("CC","Claude Code · your login","Claude Code · ~/.local/bin/claude · v2.1.314 · 6 optimizations use it","ok","Signed in",True)
      + crow("CX","Codex · your login","Codex · /opt/homebrew/bin/codex · v0.166","off","Signed out",fix=("Sign In","Opens codex login in Terminal"))
      + crow("GK","Grok · your login","Grok · ~/.grok/bin/grok · v1.0.70 · 1 optimization uses it","ok","Signed in")
      + g("API keys", "run on this Mac or unattended in the cloud · billed to the key")
      + crow("CC","Claude Code · team key","Anthropic key ····4f2a · this month $61 across 3 optimizations","ok","Valid")
      + crow("CX","Codex · ci key","OpenAI key ····91c0 · not used yet","ok","Valid")
      + crow("GK","Grok · xAI key","xAI key · none yet","off","No key",fix=("Add Key…","Paste an xAI key"))
      + g("Instructions", "what every agent reads before it starts")
      + '<div class="row"><span class="logo" aria-hidden="true">MD</span><div style="flex:1"><div class="n">RATCHET.md in each repo</div><div class="d">Goal, constraints and what not to touch; agents also see protected paths and past verdicts</div></div>'
      + btn("View Default","sm",toast="Opens the default RATCHET.md") + '</div>')
    return customize("Agents", "Yours", body, "Search agents", "Add Agent…")

def discover():
    def cc(bg, init, size, n, d, by, done, verified=True):
        vf = (f'<svg class="vf" role="img" aria-label="Verified by Ratchet" width="14" height="14" viewBox="0 0 16 16" fill="none" stroke-width="1.3"><title>Verified by Ratchet</title><path d="M8 1.5l1.6 1.2 2-.1.6 1.9 1.6 1.2-.6 1.9.6 1.9-1.6 1.2-.6 1.9-2-.1L8 14.5l-1.6-1.2-2 .1-.6-1.9-1.6-1.2.6-1.9-.6-1.9 1.6-1.2.6-1.9 2 .1z"/><path d="M5.6 8.2l1.6 1.6 3.2-3.4"/></svg>') if verified else ""
        b = (f'<button type="button" class="add done" aria-label="{n} added. Remove" title="Added · click to remove" data-add="1">{ic("check",14)}</button>' if done
             else f'<button type="button" class="add" aria-label="Add {n}" title="Add {n}" data-add="1">{ic("plus",14)}</button>')
        return (f'<div class="cc"><span class="lg"><i style="background:{bg}">{init}</i></span><div style="flex:1;min-width:0"><div class="n">{n} {vf}</div><div class="d">{d}</div><div class="by">by {by}</div></div>{b}</div>')
    logos = ""
    for mt, sz, inner, bg, fs in ((0,74,50,"var(--vendorDatadog)",17),(30,88,59,"#E2662A",21),(-10,112,76,"#2C6CE5",26),(36,80,54,"#362D59",19),(-6,96,65,"#1A7F64",23),(22,78,53,"#C4462B",18)):
        t = {"var(--vendorDatadog)":"Dd","#E2662A":"Gf","#2C6CE5":"Hc","#362D59":"Se","#1A7F64":"NR","#C4462B":"Pm"}[bg]
        logos += f'<div style="margin-top:{mt}px"><span class="lg" style="width:{sz}px;height:{sz}px"><i style="width:{inner}px;height:{inner}px;background:{bg};font-size:{fs}px">{t}</i></span></div>'
    hero = ('<div class="cur-hero">'
      f'<button type="button" class="arr" style="left:20px" aria-label="Previous collection" data-toast="Previous collection: Cloud providers">{ic("left",16)}</button>'
      f'<button type="button" class="arr" style="right:20px" aria-label="Next collection" data-toast="Next collection: Cloud providers">{ic("right",16)}</button>'
      '<div class="ey">Curated by Ratchet</div><div class="tt">Measure against production</div><div class="sb">Pull real traffic shapes and baselines from your observability stack, so wins hold up after merge.</div>'
      f'<div class="logos">{logos}</div>{btn("Explore","",toast="Observability connectors: Datadog, Grafana, Honeycomb, Sentry, New Relic, Prometheus")}'
      '<div class="dots"><i></i><i class="on"></i><i></i></div></div>')
    grid = ('<div><div style="display:flex;align-items:baseline;margin-bottom:12px"><b>Top connectors</b><span class="cnt">24</span><button type="button" class="link sp" data-toast="All 24 connectors">Show all →</button></div><div class="cgrid">'
      + cc("#24292F","GH",0,"GitHub","Read code and open pull requests with wins","Ratchet",True)
      + cc("#94551A","AWS",0,"Amazon Web Services","Run the evaluator and environments in your account","Ratchet",True)
      + cc("#3A6FD8","GCP",0,"Google Cloud","Run the evaluator and environments in your GCP project","Ratchet",False)
      + cc("#1F6FC0","Az",0,"Microsoft Azure","Run the evaluator in an Azure subscription","Ratchet",False)
      + cc("#C2491D","GL",0,"GitLab","Read code and open merge requests","Ratchet",False)
      + cc("var(--vendorDatadog)","Dd",0,"Datadog","Baselines and traffic shape from production APM","Datadog",False)
      + cc("#4A154B","#",0,"Slack","Post opened pull requests and failed runs","Slack",True)
      + cc("#4F57C9","LN",0,"Linear","Link wins to issues; file one when a run fails","Linear",False) + '</div></div>')
    return customize("Connectors", "Discover", hero + grid, "Search connectors", "Add Connector…")

# ---- New optimization (creation conversation)
def newconv(state="ok"):
    steps = (f'<div class="step">{st("ok")} Read acme-org/shop-api · main · no ratchet.yaml yet</div>'
             f'<div class="step">{st("ok")} Picked the <b style="color:var(--text);font-weight:500">HTTP latency</b> template: k6 load test, hidden query mix as the holdout, error-rate guardrail</div>')
    if state == "draft":
        steps += f'<div class="step">{st("run")} Setup agent is drafting the environment…</div>'
    elif state == "ok":
        steps += f'<div class="step">{st("ok")} Setup agent drafted a <b style="color:var(--text);font-weight:500">full-stack</b> environment · passes every check · $0.59/h</div>'
    else:
        steps += f'<div class="step">{st("bad")} Environment check failed: load generator would cost $1.40/h, over the $1/h limit</div>'
    def kvrow(k, v, aria, popid=None, upd=False, pop=""):
        u = f'<span class="upd">{ic("check",12)}Updated {btn("Undo","sm ghost",icon="undo",toast="Change undone")}</span>' if upd else ""
        b = btn(f"Change {aria}", "sm", pop=popid, aria=f"Change {aria}") if popid else btn(f"Change {aria}", "sm", toast=f"Change {aria}", aria=f"Change {aria}")
        return f'<div class="k">{k}</div><div>{v}{u}</div><div style="position:relative">{b}{pop}</div>'
    pop = agents_pop("pDraftAg", state == "ok", "top:-24px;right:calc(100% + 8px)", lft=True)
    err = ""
    if state == "err":
        err = (f'<div class="errbox" role="alert">{ic("warn",16)}<div style="flex:1"><b>Couldn\'t lock the environment</b><div class="muted">A smaller setup (API + Postgres, no load generator host) fits under $1/h but measures less.</div></div>'
               f'{btn("Use Smaller Setup","sm",toast="Redrafting with the smaller setup…")}{btn("Try Again","sm",toast="Retrying the check")}</div>')
    kv = ('<div class="kv">'
      + kvrow("Measures", 'p95 of <span class="mono">POST /checkout</span> · lower is better', "measure")
      + kvrow("Agents", ORDER, "agents", "pDraftAg", False, pop)
      + kvrow("Cloud", "acme-prod · us-east-1", "cloud")
      + kvrow("Each run", "up to $50 and 2 h", "limits")
      + kvrow("Schedule", "every night at 02:00 · first run now", "schedule", upd=(state == "ok"))
      + kvrow("Output", "a pull request with verified wins · auto-merge off", "output")
      + '<div class="k">Config</div><div class="mono sm" style="color:var(--textMuted)">ratchet.yaml · bench/latency.js · infra/ratchet/main.tf · RATCHET.md → branch <b>ratchet/config</b></div><div></div></div>')
    dis = state == "err"
    ft = ('<div class="ft"><span class="s">Runs at once from <span class="mono">ratchet/config</span>. A PR adopts it into main later.</span><span class="sp"></span>'
          + btn("Start Over", "ghost", icon="undo", go="s1")
          + btn("Create and Run Once · ≈$38", "dis" if dis else "", toast="Created · first run started now")
          + btn("Create Optimization", "pri" + (" dis" if dis else ""), go="s3", toast="Created · runs tonight at 02:00") + '</div>')
    card = (f'<div class="def"><div class="hd">{mark(18)}<b>Checkout p95 · nightly</b><span class="sp"></span><span class="s">about $1.20 environment + agents per run</span></div>{err}{kv}{ft}</div>')
    lead = {"ok": "Here's the optimization. Create it and it runs at once, then every night, and opens a pull request when something is proven faster.",
            "err": "The environment needs a smaller setup. Pick a fix below, or say what to change.", "draft": ""}[state]
    leadh = f'<div style="margin:12px 0 12px;color:var(--textMuted)">{lead}</div>' if lead else ""
    attrib = "" if state == "draft" else (f'<div class="attrib">Written by <b>Claude Code</b> · <b>Opus</b> {btn("Retry","sm ghost",icon="undo",toast="Regenerating the draft…")}{btn("Copy","sm ghost",toast="Copied")}</div>')
    reply = (f'<div class="msg-r"><div class="who">{mark(22)}</div><div style="flex:1">'
             f'<div class="notice">{ic("info",14)}<span>First time: your prompt and repo name go to Claude Code on this Mac. Nothing else.</span><span class="sp"></span>{ibtn("x","Dismiss","sq",toast="Dismissed")}</div>'
             + steps + leadh + ("" if state == "draft" else card) + attrib + '</div></div>')
    action = (btn("Stop", "sm", icon="stop", aria="Stop drafting", toast="Stopped drafting") if state == "draft"
              else f'<button type="button" class="send" aria-label="Send (⌘↩)" title="Send (⌘↩)" data-toast="Sent">{ic("send",16)}</button>')
    comp = (f'<div class="composer steer" style="margin-top:auto"><div class="ph" style="font-size:14px">Change anything in plain words, e.g. “use Grok too” or “only weekdays”.</div>'
            f'<div class="chips"><span class="s">⌘↩ sends</span><span class="sp"></span>{action}</div></div>')
    banners = ""
    inner = ('<div class="msg-u">Make checkout p95 faster in shop-api. Run it every night, $50 a run.</div>' + reply)
    pg = f'<div class="page mid" style="--w:900px;gap:20px">{inner}{comp}</div>'
    return window(side("Home"), toolbar("New optimization", "sec"), pg, banners=banners)

# ---- Settings (Settings scene, ⌘,)
def settings(active="General"):
    tabs = [("General", "sliders"), ("Defaults", "sliders"), ("Notifications", "bell"), ("This Mac", "mac")]
    st_ = "".join(f'<span role="tab" tabindex="0" aria-selected="{str(n==active).lower()}" class="{"on" if n==active else ""}" data-pane="{n}">{ic(i,18)}{n}</span>' for n, i in tabs)
    S = lambda t, d, ctl: f'<div class="set"><div><div class="t">{t}</div><div class="d">{d}</div></div>{ctl}</div>'
    sel = lambda v: f'<span class="sel" role="button" tabindex="0" data-toast="Pick a value">{v}{ic("chev",12)}</span>'
    panes = {
     "General": ('<div class="grph">Ratchet</div>'
        + S("Launch at login", "Keeps the menu bar item and notifications running after a restart.", tog(True, "Launch at login"))
        + S("Appearance", "Follows your Mac.", seg(["System","Light","Dark"],"System","Appearance"))
        + S("Ask about a run", "Answer questions about a run with your default agent. Counts toward that run's spend. Never changes a verdict.", tog(True, "Ask about a run"))
        + S("Run log opens in", "Overview hides agent chatter. Everything shows every event.", seg(["Overview","By Agent","Everything"],"Overview","Run log")) ),
     "Defaults": ('<div class="grph">Default for new optimizations</div>'
        + S("Each run", "Stops on its own at either limit. Change it per optimization.", sel("$50 · 2 h"))
        + S("Schedule", "When a new optimization runs if you don't say.", sel("Once"))
        + S("Agents run on", "This Mac uses your own logins and is free while the Mac is awake. Cloud runs unattended with API keys.", seg(["This Mac","Cloud"],"This Mac","Where agents run"))
        + S("Auto-merge verified wins", "Off: a pull request waits for you. On: it merges once CI passes.", tog(False, "Auto-merge verified wins")) ),
     "Notifications": ('<div class="grph">Default for new optimizations</div>'
        + S("PR opened", "A verified win has a pull request.", tog(True, "PR opened"))
        + S("Run failed", "A run stopped on an error.", tog(True, "Run failed"))
        + S("Budget 80% used", "A run or month is close to its cap.", tog(True, "Budget"))
        + S("Connector expiring", "A sign-in is about to expire.", tog(True, "Connector expiring")) ),
     "This Mac": ('<div class="grph">Background service</div>'
        + S("Service", f'{st("ok","Running")} · version 0.3.1', btn("Stop…", "sm", toast="Stop the service? Runs pause.", aria="Stop the background service"))
        + S("Command-line tool", f'{st("ok","Installed")} · ~/.local/bin/ratchet', btn("Reinstall", "sm", toast="Reinstalled to ~/.local/bin"))
        + '<div class="grph">Advanced</div>'
        + f'<div class="set" style="flex-direction:column;align-items:stretch;gap:0"><div style="display:flex;gap:16px;align-items:center"><div><div class="t">Demo mode</div><div class="d">Shows sample data. Real runs pause.</div></div>{tog(False,"Demo mode")}</div>'
        + f'<div class="confirm" role="alertdialog"><b>Switch to Demo mode?</b><span class="muted">Real runs pause until you switch back.</span><div class="bs">{btn("Switch","sm bad",toast="Demo mode on")}{btn("Cancel","sm",toast="Cancelled")}</div></div></div>'),
    }
    pane_html = "".join(f'<div class="pane{" on" if n==active else ""}" data-panel="{n}">{h}</div>' for n, h in panes.items())
    return (f'<div class="setwin" role="dialog" aria-label="Settings"><div class="tbar"><div class="lights" aria-hidden="true"><i></i><i></i><i></i></div>'
            f'<div class="stabs" role="tablist" aria-label="Settings">{st_}</div></div>{pane_html}</div>')

def settings_screen(active):
    base = window(side("Overview"), toolbar("Overview", "pri"), page(overview(), 1000), cls="inactive")
    return '<div style="position:relative">' + base + settings(active) + "</div>"

# ---------------------------------------------------------------- assemble
S = []
S.append(screen("s1", "Home", home()))
S.append(screen("s9", "Overview", overview_win()))
S.append(screen("s2", "Run", run_win()))
S.append(screen("s3", "Optimization", optimization_win()))
S.append(screen("s4", "Templates", templates()))
S.append(screen("s5", "Connectors", connectors()))
S.append(screen("s6", "Settings", settings_screen("General")))
S.append(screen("s7", "Agents", agents()))
S.append(screen("s8", "New optimization", newconv("ok")))
S.append(screen("s10", "Connectors · Discover", discover()))
S.append(screen("s11", "Empty · first launch", overview_empty()))
S.append(screen("s12", "Daemon down", overview_down()))
S.append(screen("s13", "Failed connector", connectors(True)))
S.append(screen("s14", "Loading", overview_loading()))
S.append(screen("s15", "Home · light", home(focus=False), "light"))
S.append(screen("s16", "Overview · light", overview_win(), "light"))
S.append(screen("s17", "Run · light", run_win(), "light"))
S.append(screen("s18", "New optimization · error", newconv("err")))
S.append(screen("s19", "Run · stop", run_win(variant="stop")))
S.append(screen("s20", "Run · by agent", run_win(variant="agent")))
S.append(screen("s21", "Settings · This Mac", settings_screen("This Mac")))
S.append(screen("s22", "New optimization · drafting", newconv("draft")))

html = ('<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Ratchet v0.3.1 · mockups</title>'
        '<link href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600&family=Instrument+Serif&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">'
        f'<style>\n{CSS}</style></head><body>\n' + "".join(S) + "</body></html>\n")
open(os.path.join(D, "v03.html"), "w").write(html)
print("v03.html", len(html), "bytes,", len(S), "screens")

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
 "spark": '<path d="M8 2.2l1.5 3.8 3.8 1.5-3.8 1.5L8 12.8 6.5 9 2.7 7.5 6.5 6z"/>',
 "bars": '<path d="M3.5 13V8.5M8 13V3.5M12.5 13V6"/>',
 "link": '<path d="M6.8 9.2a2.5 2.5 0 0 0 3.5 0l2-2a2.5 2.5 0 0 0-3.5-3.5l-.8.8M9.2 6.8a2.5 2.5 0 0 0-3.5 0l-2 2a2.5 2.5 0 0 0 3.5 3.5l.8-.8"/>',
 "refresh": '<path d="M13 8a5 5 0 1 1-1.5-3.5M13 2.8v2.6h-2.6"/>',
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
 ("Checkout p95", "run", "Running · 1 h 12 m left", "s2"),
 ("Search relevance", "wait", "PR #311 open · +5.9%", "s2"),
 ("Forecast accuracy", "ok", "Merged · −13%", "s2"),
 ("Bundle size", "off", "No win · within noise", "s2"),
 ("CLI cold start", "bad", "Failed · AWS session expired", "s13"),
]
# The app's pages (Pages.swift), in order. No Customize group; Templates live on Benchmarks.
NAV = [("Home", "home", "s1"), ("Overview", "gauge", "s9"), ("Optimizations", "list", "s23"),
       ("Agents", "spark", "s7"), ("Benchmarks", "bars", "s4"), ("Connectors", "link", "s5")]

def side(nav, run=None, empty=False, loading=False):
    """One selection style for pages and runs (the system pill). A run is told apart by its status shape."""
    h = '<nav class="side" aria-label="Sidebar"><div class="lights" aria-hidden="true"><i></i><i></i><i></i></div>'
    for n, i, go in NAV:
        on = n == nav and not run
        cur = ' aria-current="page"' if on else ""
        h += f'<div class="nv{" on" if on else ""}" role="link" tabindex="0" data-go="{go}"{cur}>{ic(i,16)}<span>{n}</span></div>'
    h += '<div class="sec">Recent runs</div>'
    if empty:
        h += '<div class="empty-side">No runs yet.<br>Runs appear here.</div>'
    elif loading:
        for _ in range(5):
            h += '<div class="run" aria-hidden="true"><div class="skel" style="width:14px;height:14px;border-radius:7px"></div><div style="flex:1"><div class="skel" style="height:11px;width:70%"></div><div class="skel" style="height:11px;width:50%;margin-top:6px"></div></div></div>'
    else:
        for name, k, meta, go in RUNS:
            on = run == name
            lab = f"{name}, {meta.replace(' · ', ', ')}"
            h += (f'<div class="run{" on" if on else ""}" role="link" tabindex="0" aria-label="{lab}"{" aria-current=\"true\"" if on else ""} data-go="{go}">'
                  f'{st(k)}<div style="flex:1;min-width:0"><div class="ttl">{name}</div><div class="meta">{meta}</div></div></div>')
    return h + "</nav>"

def toolbar(title, sub="", search=None, actions="", new="sec", status=True, newgo="s1"):
    """Native toolbar: title and subtitle, then the sandbox pill and the one New button, then (after a gap) the page's own actions
    and its page-scoped search. No border, no global search, no Settings button (⌘, opens it)."""
    t = f'<div class="tbt"><b>{title}</b>' + (f'<span>{sub}</span>' if sub else "") + '</div>'
    g = ""
    if status:
        g += f'<span class="pillst" role="button" tabindex="0" title="Sandbox on · acme-prod · $1.20 so far" data-toast="Sandbox on · acme-prod · $1.20 so far · End Now…">{st("ok")}Sandbox on · $1.20</span>'
    dis = " disabled" if new == "dis" else ""
    g += f'<button type="button" class="btn tbnew" title="New Optimization (⌘N)"{dis}{A(go=newgo)}>{ic("plus",14)}New Optimization</button>'
    s = f'<span class="tbg">{g}</span>'
    if actions:
        s += f'<span class="tbgap" aria-hidden="true"></span><span class="tbg">{actions}</span>'
    if search:
        s += f'<span class="field" role="search" tabindex="0" data-toast="Search this page" title="{search}">{ic("search",14)}<span>{search}</span></span>'
    return f'<header class="tb" role="toolbar" aria-label="Window toolbar">{t}{s}</header>'

def banner(kind, text, actions="", dismiss=True):
    icon = "warn" if kind in ("bad", "wait") else "info"
    x = ibtn("x", "Dismiss", "sq x", toast="Dismissed") if dismiss else ""
    return f'<div class="banner {kind}" role="alert">{ic(icon,16)}<span>{text}</span>{actions}{x}</div>'

def window(sidebar, tb, main, banners="", extra="", cls=""):
    return (f'<div class="win {cls}">{sidebar}<div class="col">{tb}<div class="banners">{banners}</div>'
            f'<main class="main">{main}</main></div>{extra}</div>')

def screen(sid, name, body, theme="dark", style=""):
    return f'<section class="screen" id="{sid}" data-name="{name}" data-theme="{theme}" aria-label="{name}"{f" style=\"{style}\"" if style else ""}>{body}</section>\n'

def page(inner, w=1000, mid=False, cls="", style=""):
    return f'<div class="page{" mid" if mid else ""} {cls}" style="--w:{w}px;{style}">{inner}</div>'

# ---------------------------------------------------------------- shared blocks
def agents_pop(pid, open_=True, style="", up=False, left=False, lft=False, where=True):
    rows = [("1", "Claude Code · your login"), ("2", "Codex · ci key"), ("3", "Grok · your login")]
    r = "".join(f'<div class="po"><span class="n">{n}</span><span class="t">{t}</span>'
                f'{ibtn("up", "Move "+t.split(" ·")[0]+" up", "sq", toast="Moved up")}{ibtn("down", "Move "+t.split(" ·")[0]+" down", "sq", toast="Moved down")}</div>' for n, t in rows)
    return (f'<div class="pop{" open" if open_ else ""}{" up" if up else ""}{" left" if left else ""}{" lft" if lft else ""}" id="{pid}" role="dialog" aria-label="Change agents" style="{style}">'
            f'<h3>Change agents</h3>{r}' + (f'<div class="pf"><span class="s">Where</span>{seg(["This Mac","Cloud"],"This Mac","Where agents run")}<span class="sp"></span></div>' if where else "") +
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
          + r("Checkout p95","acme-org/shop-api","run","Running","1 h 12 m left",'<span class="up mono">−30%</span> <span class="s">412 → 288 ms</span>',"$38","$4.8k","126×")
          + r("Search relevance","acme-org/search-svc","wait","PR #311 open","Nightly",'<span class="up mono">+5.9%</span> <span class="s">nDCG 0.612 → 0.648</span>',"$22","$2.9k","132×",True)
          + r("CLI cold start","srikanth235/acme-cli","bad","Failed","AWS session expired",'<span class="up mono">−39%</span> <span class="s">840 → 510 ms</span>',"$41","$1.2k","29×")
          + r("Bundle size","acme-org/web-app","off","No win","On push to main",'<span class="s">no win yet · within noise (±2%)</span>',"$19",'<span class="s">—</span>','<span class="s">—</span>'))
    return ('<div><h2>Return by optimization</h2><table><thead><tr><th style="width:24%">Optimization</th><th>Status</th><th>Best result</th><th class="r">Spent</th><th class="r">Value / yr</th><th class="r">Return</th></tr></thead>'
            f'<tbody>{rows}</tbody></table><div class="s" style="margin-top:8px">Estimated from each optimization\'s Value row. <button type="button" class="link" data-go="s3">Edit value</button></div></div>')

# ---------------------------------------------------------------- screens
def page_head(title, sub="", right=""):
    s = f'<div class="s" style="margin-top:4px">{sub}</div>' if sub else ""
    return f'<div class="head" style="display:flex;align-items:flex-start;gap:12px"><div><h1 class="serif">{title}</h1>{s}</div><span class="sp"></span>{right}</div>'

def home(theme="dark", focus=True):
    strip = ('<div class="strip"><div><div class="l">Spent · 90 days</div><div class="v serif">$184</div></div>'
             '<div><div class="l">Value per year</div><div class="v serif">$12.5k <small style="font-size:14px;color:var(--textMuted);font-family:\'Instrument Sans\',sans-serif">delivered + $2.9k pending · 68×</small></div></div></div>'
             f'<div class="s" style="display:flex;gap:8px;align-items:center;margin-top:-16px">3 optimizations · 2 PRs this week · {st("bad","1 run failed")} <button type="button" class="link" data-go="s13">CLI cold start</button></div>')
    chips = (chip("acme-org/shop-api","p1") + chip("acme-prod · AWS","p2") + chip("Claude Code → Codex → Grok","pAg",open_=True)
             + chip("$50 · 2 h per run","p4") + chip("Every night","p5")
             + f'<button type="button" class="send" aria-label="Send (⌘↩)" title="Send (⌘↩)" data-go="s8">{ic("send",16)}</button>')
    comp = (f'<div class="composer{" focus" if focus else ""}" style="position:relative" role="group" aria-label="Describe an optimization"><div class="ph">Make checkout p95 faster in shop-api. Run it every night.</div><div class="chips">{chips}</div>'
            + agents_pop("pAg", True, "top:calc(100% + 8px);left:304px", left=True) + '</div>')
    sugg = "".join(f'<span data-go="s8" role="button" tabindex="0">{t}</span>' for t in ("Cut CLI cold start", "Shrink the web bundle", "Improve forecast accuracy", "Try a demo"))
    inner = (strip + '<div style="margin-top:8px;text-align:center"><div class="serif" style="font-size:34px">What should get better?</div>'
             '<div class="sub" style="font-size:14px">Describe it once. Ratchet runs it and opens a pull request with what it proves.</div></div>'
             + comp + f'<div class="sugg" style="margin-top:{"168px" if False else "0"}">{sugg}</div>'
             + f'<div class="notice" style="margin:0">{ic("info",14)}<span>First time: your messages, the repo name and the files Claude Code reads go to Anthropic. Ratchet sends nothing else.</span><span class="sp"></span>{ibtn("x","Dismiss the first-use notice","sq",toast="Dismissed")}</div>')
    return window(side("Home"), toolbar("Home", "Describe what to optimize"), page(inner, 820, True))

def overview(theme="dark", mode="ok"):
    hero = ('<div class="hero3"><div><div class="s">Spent</div><div class="serif big">$184</div><div class="bar"><i style="width:37%"></i></div><div class="s" style="margin-top:8px">of $500 cap · AWS $121 · agents $63</div></div>'
            '<div><div class="s">Delivered</div><div class="serif big">$12.5k<small> / yr</small></div><div class="s" style="margin-top:20px">3 merged wins · +$2.9k pending in 1 PR</div></div>'
            '<div><div class="s">Return</div><div class="serif big brasst">68×</div><div class="s" style="margin-top:20px">per $1 spent, yearly · paid back in 6 days</div></div></div>')
    right = ('<div style="display:flex;flex-direction:column;gap:20px"><div><h2>Pending value</h2><div class="card"><div class="serif" style="font-size:30px">$2.9k<small class="muted" style="font-size:14px"> / yr</small></div>'
             f'<div class="s" style="margin-top:4px">1 verified win in 1 open pull request. Counts once it merges.</div><div style="margin-top:12px">{btn("Open PR #311","sm",icon="pr",toast="Opens PR #311 on GitHub")}</div></div></div>'
             '<div><h2>Next 30 days</h2><div class="kv compact" style="grid-template-columns:1fr auto"><div class="k">Scheduled runs</div><div class="mono">62</div><div class="k">Expected spend</div><div class="mono">~$140</div><div class="k">Hard cap</div><div class="mono">$500</div></div></div></div>')
    # the period picker is a page control, not a toolbar item
    inner = (page_head("Overview", "", seg(["30 days","90 days","All time"],"90 days","Range"))
             + hero + story_line()
             + f'<div style="display:grid;grid-template-columns:1fr 296px;gap:40px"><div><h2>Value delivered vs spend</h2>{CHART}</div>{right}</div>' + ov_table())
    return inner

def overview_win(theme="dark"):
    return window(side("Overview"), toolbar("Overview", "90 days", search="Search optimizations"), page(overview(theme), 1000))

def overview_empty():
    inner = ('<h1 class="serif">Overview</h1><div style="flex:1;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:12px;text-align:center;padding-bottom:80px">'
             f'{mark(72)}<div class="serif" style="font-size:30px;margin-top:8px">No wins yet</div><div class="muted">Start your first optimization.</div>'
             f'<div style="margin-top:8px">{btn("New Optimization","pri",icon="plus",go="s1")}</div><div class="s">Or <button type="button" class="link" data-go="s8">try a demo</button> first.</div></div>')
    return window(side("Overview", empty=True), toolbar("Overview", "", status=False), page(inner, 1000))

def overview_down():
    b = banner("bad", "<b>Ratchet service isn't running.</b> Data below may be out of date · last updated 14:02.",
               btn("Start Service", "sm", toast="Starting the background service…") + btn("Details","sm ghost",toast="launchctl: ratchetd not loaded"), dismiss=False)
    return window(side("Overview"), toolbar("Overview", "90 days", new="dis"), page(f'<div class="stale" aria-disabled="true" style="display:flex;flex-direction:column;gap:32px">{overview()}</div>', 1000), banners=b)

def overview_loading():
    sk = lambda w, h=14, m=0: f'<div class="skel" style="width:{w};height:{h}px;margin-top:{m}px"></div>'
    cell = lambda: f'<div>{sk("30%",12)}{sk("60%",52,8)}{sk("100%",4,16)}{sk("70%",12,10)}</div>'
    inner = (f'<div style="display:flex;align-items:center;gap:12px"><h1 class="serif">Overview</h1><span class="sp"></span><span class="stt stt-run" role="status">{st("run")}<span>Loading your optimizations…</span></span></div>'
             f'<div class="hero3" aria-busy="true">{cell()}{cell()}{cell()}</div>'
             f'<div style="display:grid;grid-template-columns:1fr 296px;gap:40px"><div>{sk("100%",220)}</div><div>{sk("100%",120)}{sk("100%",80,16)}</div></div>'
             f'<div>{sk("100%",14)}{sk("100%",14,16)}{sk("100%",14,16)}{sk("100%",14,16)}</div>')
    return window(side("Overview", loading=True), toolbar("Overview", "", new="dis"), page(inner, 1000))

# ---- Optimizations (the list)
def optimizations_win():
    def r(name, repo, stk, stl, sub, best, spent, val, go="s2", pend=False):
        v = f'<span class="faint">{val}</span>' if pend else val
        return (f'<tr data-go="{go}" class="rowlink"><td><button type="button" class="tname" data-go="{go}">{name}</button><div class="s">{repo}</div></td>'
                f'<td>{st(stk, stl)}<div class="s" style="padding-left:22px">{sub}</div></td><td>{best}</td>'
                f'<td class="r mono">{spent}</td><td class="r mono">{v}</td></tr>')
    rows = (r("Checkout p95","acme-org/shop-api","run","Running","1 h 12 m left",'<span class="up mono">−30%</span> <span class="s">412 → 288 ms</span>','$38 <span class="faint">/ 50</span>',"$4.8k")
          + r("Search relevance","acme-org/search-svc","wait","PR #311 open","Nightly",'<span class="up mono">+5.9%</span> <span class="s">nDCG 0.612 → 0.648</span>','$22 <span class="faint">/ 80</span>',"+$2.9k pending",pend=True)
          + r("Forecast accuracy","acme-org/demand-model","ok","Merged","Weekly",'<span class="up mono">−13%</span> <span class="s">WAPE 18.2 → 15.9%</span>',"$64","$6.5k")
          + r("CLI cold start","srikanth235/acme-cli","bad","Failed","AWS session expired",'<span class="up mono">−39%</span> <span class="s">840 → 510 ms</span>',"$41","$1.2k","s13")
          + r("Bundle size","acme-org/web-app","off","No win","On push to main",'<span class="s">nothing beat the noise (±2%)</span>',"$19",'<span class="s">—</span>'))
    tbl = ('<table class="otable"><thead><tr><th style="width:30%">Optimization</th><th style="width:20%">Status</th><th>Result</th><th class="r">Spent</th><th class="r">Value / yr</th></tr></thead>'
           f'<tbody>{rows}</tbody></table>')
    inner = page_head("Optimizations", "Every run, live or done. Open one to see its agents and result.", seg(["All","Running","Done"],"All","Show")) + tbl
    return window(side("Optimizations"), toolbar("Optimizations", "5 optimizations", search="Search optimizations"), page(inner, 1000))

# ---- optimization header (Run and Settings tabs)
def opt_head(tab):
    hdr = (f'<div><button type="button" class="link" style="text-decoration:none" data-go="s23">‹ Optimizations</button></div>'
           '<div class="rh"><div><div class="serif" style="font-size:32px;line-height:1.15">Checkout p95</div>'
           '<div class="sub" style="margin-top:4px">acme-org/shop-api · main · run #14 · started 00:48 · 1 h 12 m left · $38 of $50</div></div></div>'
           f'<div class="acts2">{st("run","Running")}'
           + btn("Stop…", "bad", go="s19", aria="Stop this run")
           + btn("Pause Schedule", "", swap="Pause Schedule|Resume Schedule", toast="Schedule paused · no new runs until you resume")
           + btn("Open PR #313", "", icon="pr", toast="Opens PR #313 on GitHub")
           + more("More actions for Checkout p95", toast="Run Now · Duplicate · Archive…") + '</div>'
           + f'<div>{seg(["Run","Settings"], tab, "Optimization", ["s2","s3"])}</div>')
    return hdr

# ---- Run: one timeline, the steering composer docked below the scrolling content (never over it)
def run_win(theme="dark", variant="main"):
    hero = ('<div class="hero3"><div><div class="s">Best so far</div><div class="serif big">288 ms</div><div class="s" style="margin-top:12px">−30% · baseline 412 ms · noise ±2.1%</div></div>'
            '<div><div class="s">Spent</div><div class="serif big">$38</div><div class="bar"><i style="width:76%"></i></div><div class="s" style="margin-top:8px">of $50 budget</div></div>'
            '<div><div class="s">Time left</div><div class="serif big">1 h 12 m</div><div class="s" style="margin-top:12px">of 2 h limit</div></div></div>')
    views = (f'<div style="display:flex;align-items:center;gap:12px"><h2 style="margin:0">Log</h2>{seg(["Overview","By Agent","Everything"], "By Agent" if variant=="agent" else "Overview", "Log view", ["s2","s20","s2"])}'
             + ('<span class="s">One lane per agent</span>' if variant == "agent" else '<span class="s">What the run did, step by step</span>') + '</div>')
    if variant == "agent":
        def lane(t, state, body, act=""):
            return f'<div class="lane"><div class="lh">{st(state[0], state[1])}<b>{t}</b><span class="sp"></span>{act}</div>{body}</div>'
        body = ('<div style="display:flex;flex-direction:column;gap:12px">'
            + lane("Claude Code · your login", ("run", "Working"), '<div class="muted">Candidate 3 · <i>Skip the audit write on the hot path</i> · blocked, touched a protected path</div><div class="s">Candidate 4 on the bench · best so far 288 ms (−30%)</div>', btn("Pause Agent", "sm", icon="pause", swap="Pause Agent|Resume Agent", toast="Agent paused"))
            + lane("Claude Code · team key", ("ok", "Idle"), '<div class="muted">Candidate 1 · <i>Pool size 20 → 48, reuse connections</i> · locked in, −16.5%</div>', btn("Pause Agent", "sm", icon="pause", swap="Pause Agent|Resume Agent", toast="Agent paused"))
            + '<div class="lane"><div class="lh">' + st("q", "Not started") + '<b>Grok · your login</b></div><div class="emptyst">No activity yet. Grok starts when a slot is free.</div></div></div>')
    else:
        ev = lambda t, b: f'<div class="ev"><div class="t">{t}</div><div class="b">{b}</div></div>'
        body = ('<div class="tl">'
          + ev("00:48", "<b>Environment</b> · full stack passed every check and was locked on <span class=\"mono\">ratchet/env</span> · $0.59/h")
          + ev("00:57", "<b>Baseline</b> · 412 ms p95 over 9 runs · noise ±2.1%, so a win must beat 4.2%")
          + ev("01:10", '<div class="agentmsg"><b>Claude Code · team key</b> proposes <i>Pool size 20 → 48, reuse connections</i> · 2 files</div>')
          + ev("01:22", f'<div class="card verdict"><div class="sealed">{mark(16)} Verdict · sealed</div><div style="display:flex;align-items:baseline;gap:12px;margin-top:8px;flex-wrap:wrap"><span class="serif" style="font-size:30px">−16.5%</span><span class="muted">412 → 344 ms p95 · confirmed on a fresh host · <b style="color:var(--ok);font-weight:500">locked in</b></span></div><div class="s" style="margin-top:4px">Holdout passed · error rate 0.02% · memory +3 MB</div></div>')
          + ev("01:31", '<div class="agentmsg"><b>Claude Code · your login</b> proposes <i>Skip the audit write on the hot path</i></div>')
          + ev("01:33", f'<div class="card" style="border-color:var(--bad)"><div class="sealed" style="color:var(--bad)">{st("bad","Blocked before measuring")}</div><div class="muted" style="margin-top:8px">Touched a protected path: <span class="mono">bench/latency.js</span>. Agents can\'t change the evaluator.</div></div>')
          + ev("02:00", f'{st("run")} 2 candidates on the bench · best so far <b>288 ms</b> (−30%)')
          + ev("02:05", f'<div class="note"><b>You</b> · “focus on database queries” <span class="rcpt">{st("wait","Queued · read at the next candidate")}</span> {btn("Undo","sm ghost",icon="undo",toast="Note withdrawn")}</div>')
          + ev("02:09", f'<div class="note"><b>You</b> · “skip the cache layer, it is owned by another team” <span class="rcpt">{st("ok","Read by Claude Code at 02:11")}</span></div>')
          + '</div>')
    comp = ('<div class="dock"><div class="composer steer"><div class="ph" style="font-size:14px">Leave a note to steer the next candidate, e.g. “focus on database queries”. Notes are read between candidates.</div>'
            f'<div class="chips"><span class="s">Return adds a line · ⌘↩ sends</span><button type="button" class="send" aria-label="Send note (⌘↩)" title="Send note (⌘↩)" data-toast="Note queued · read at the next candidate">{ic("send",16)}</button></div></div></div>')
    inner = f'<div class="scrl">{opt_head("Run")}{hero}{views}{body}</div>' + comp
    pg = page(inner, 1000, cls="runpg")
    extra = ""
    if variant == "stop":
        extra = ('<div class="overlay"><div class="dialog" role="alertdialog" aria-labelledby="dg"><h3 id="dg">Stop Checkout p95?</h3><p>Wins found so far are kept.</p>'
                 f'<div class="bs">{btn("Cancel","",go="s2")}{btn("Stop","bad",go="s23",toast="Stopped · wins so far are kept")}</div></div></div>')
    return window(side("Optimizations", run="Checkout p95"), toolbar("Optimizations", "Checkout p95"), pg, extra=extra, cls="fix")

# ---- Optimization settings (Settings tab)
def optimization_win():
    def row(k, v, tag, note="", saved=False, popid=None, pop_html="", label="", fixed=False):
        t = f'<span class="tagc{" cus" if tag=="Custom" else ""}">{"Using default" if tag=="Default" else tag}</span>' if tag else ""
        sv = f'<span class="saved">{ic("check",12)}Saved</span>' if saved else ""
        n = f'<div class="s" style="width:100%">{note}</div>' if note else ""
        if fixed:
            e = '<div></div>'
        else:
            e = f'<div class="rowedit" style="position:relative">{sv}{t}{btn("Edit","sm",pop=popid or "x", aria=f"Edit {label}") if popid else btn("Edit","sm",toast=f"Edit {label} inline", aria=f"Edit {label}")}{pop_html}</div>'
        return f'<div class="k">{k}</div><div style="flex-direction:column;align-items:flex-start;gap:2px"><div>{v}</div>{n}</div>{e}'
    pop = agents_pop("pOpAg", False, "top:-24px;right:calc(100% + 8px)", lft=True)
    fx = "Fixed when it was created."
    kv = ('<div class="kv rows">'
      + row("Goal", "Make checkout p95 faster", None, fx, fixed=True)
      + row("Repository", "acme-org/shop-api · main", None, fx, fixed=True)
      + row("Agents", ORDER, "Custom", popid="pOpAg", pop_html=pop, label="agents")
      + row("Each run", "up to <b>$50</b> and <b>2 h</b>", None, fx, fixed=True)
      + row("Schedule", "every night at 02:00 · next run in 3 h", "Custom", saved=True, label="schedule")
      + row("Value", "ms saved × 48k checkouts a day → <b>$4.8k</b> / yr", "Custom", label="value")
      + row("Environment", "Checked before every run. Falls back to a smaller setup if a check fails.", "Default", "Nothing waits for you.", label="environment")
      + row("Output", "pull request with verified wins · auto-merge off", "Default", label="output")
      + row("Notify", "PR opened · run failed · budget 80% · connector expiring", "Default", label="notifications")
      + '</div>')
    arch = ('<div style="display:flex;align-items:center;gap:16px;border-top:1px solid var(--hairline);padding-top:16px"><div><b>Archive optimization</b><div class="s">Stops the schedule and keeps its history and wins.</div></div><span class="sp"></span>'
            + btn("Archive…", "", toast="Archive Checkout p95? History is kept.") + '</div>')
    leg = '<div class="s">Defaults come from Settings → General. “Custom” rows win over them.</div>'
    inner = opt_head("Settings") + leg + kv + arch
    return window(side("Optimizations", run="Checkout p95"), toolbar("Optimizations", "Checkout p95"), page(inner, 1000))

# ---- Benchmarks (history, templates, demos)
def benchmarks():
    def hrow(n, o, k, res, noise, ev, d, base=False):
        r = f'<span class="s">{res}</span>' if base else res
        return (f'<tr><td><b style="font-weight:600">{n}</b><div class="s">{o}</div></td><td>{r}</td><td class="r mono">{noise}</td><td class="r mono">{ev}</td><td class="r s">{d}</td></tr>')
    up = lambda p, t: f'<span class="up mono">{p}</span> <span class="s">{t}</span>'
    hist = ('<div><h2>History</h2><table><thead><tr><th style="width:34%">Benchmark</th><th>Result</th><th class="r">Noise</th><th class="r">Evals</th><th class="r">Started</th></tr></thead><tbody>'
      + hrow("Open-loop load test","Checkout p95","", up("−30%","412 → 288 ms"),"±2.1%","11","Oct 31")
      + hrow("Relevance eval","Search relevance","", up("+5.9%","nDCG 0.612 → 0.648"),"±0.8%","9","Oct 28")
      + hrow("Rolling backtest","Forecast accuracy","", up("−13%","WAPE 18.2 → 15.9%"),"±1.2%","21","Sep 21")
      + hrow("Cold start","CLI cold start","", up("−39%","840 → 510 ms"),"±3%","14","Aug 22")
      + hrow("Bundle size","Bundle size","", "baseline 1840 KB","±2%","8","Oct 11", True)
      + '</tbody></table></div>')
    def tile(n, cat, d, meas, label="Use Template…"):
        return (f'<div class="tile" style="flex-direction:column"><div style="display:flex;gap:8px;align-items:baseline"><b style="font-weight:600">{n}</b><span class="sp s">{cat}</span></div>'
                f'<div class="muted sm">{d}</div><div class="s">{meas}</div><div style="margin-top:auto;padding-top:4px">{btn(label,"sm",go="s8",toast="Opens a chat with this template chosen")}</div></div>')
    tpl = ('<div><h2>Templates</h2><div class="grid2c">'
      + tile("HTTP latency","Service","p95 of an endpoint under load, with a hidden query mix as the holdout.","Measures p95 latency · separate host")
      + tile("CLI cold start","Tool","Wall time of repeated invocations; hidden invocations as holdout.","Measures wall time · this Mac")
      + tile("Bundle size","Web","Build seconds and bundle KB.","Measures bundle KB · this Mac")
      + tile("Test suite time","CI","Faster CI without skipping tests: every test must still pass.","Measures suite seconds · this Mac")
      + tile("Forecast accuracy","Data","WAPE, MAPE and bias, backtested on recent periods.","Measures WAPE · this Mac")
      + tile("LLM eval","AI","Score and cost per call on a hidden eval set.","Measures score · separate host")
      + '</div></div>')
    demo = ('<div><h2>Try a demo</h2><div class="grid2c">'
      + tile("Demo: slow function","Demo","A small repo with a slow function. No accounts needed.","Measures runtime · this Mac","Try a Demo…")
      + tile("Demo: slow endpoint","Demo","A toy API with a slow route. No accounts needed.","Measures p95 latency · this Mac","Try a Demo…")
      + '</div></div>')
    inner = page_head("Benchmarks", "What each optimization was measured on, and templates to start from.") + hist + tpl + demo
    return window(side("Benchmarks"), toolbar("Benchmarks", "5 runs"), page(inner, 1000))

# ---- Connectors
def crow(logo, name, desc, k, label, dflt=False, fix=None, err=None, focus=False, badge=""):
    d = '<span class="s" style="margin-right:12px">Default</span>' if dflt else ""
    fx = ""
    if fix:
        fx = btn(fix[0], "sm" + (" focus" if focus else ""), toast=fix[1])
    e = f'<div class="errline">{ic("warn",14)}{err}</div>' if err else ""
    return (f'<div class="row"><span class="logo" aria-hidden="true">{logo}</span><div style="flex:1;min-width:0"><div class="n">{name}{badge}</div><div class="d">{desc}</div>{e}</div>'
            f'{d}{st(k,label)}<span style="width:96px;display:flex;justify-content:flex-end">{fx}</span>{more("More actions for "+name)}</div>')

def connectors_page(tab, body):
    gos = ("s5", "s10")
    inner = page_head("Connectors", "Accounts each optimization can use. Secrets stay in your keychain.") + f'<div>{seg(["Yours","Discover"], tab, "Show", gos)}</div>' + f'<div class="stack">{body}</div>'
    acts = ibtn("refresh", "Refresh connectors", toast="Checking every connector…") + ibtn("plus", "Add Connector", toast="Add Connector…")
    return window(side("Connectors"), toolbar("Connectors", "8 connectors", search="Search connectors", actions=acts), page(inner, 1000))

def connectors(fail=False):
    g = lambda t: f'<div class="grp" style="margin-top:20px"><b>{t}</b></div>'
    awsfix = ("Sign In", "Opens AWS sign-in in your browser")
    body = (g("GitHub").replace("margin-top:20px", "margin-top:0")
      + crow("GH","srikanth235","Personal · 42 repos","ok","Connected",True)
      + (crow("GH","acme-org","GitHub App · 18 repos","wait","Needs permission",fix=("Reconnect","Opens GitHub to approve the new permission"),err="Can't open pull requests: “contents: write” was removed. Reconnect to restore it.") if fail
         else crow("GH","acme-org","GitHub App · 18 repos","ok","Connected"))
      + g("AWS")
      + crow("AWS","acme-prod","SSO · us-east-1 · cap $500 / month","ok","Connected",True)
      + crow("AWS","personal-dev","~/.aws profile · us-west-2 · cap $100 / month","ok","Connected")
      + crow("AWS","acme-staging","SSO · eu-west-1","bad","Expired",fix=awsfix,
             err="The AWS session is expired. Sign in again before a long run." + (" “CLI cold start” failed on it." if fail else ""), focus=fail))
    return connectors_page("Yours", body)

def discover():
    def d(n, t):
        return (f'<div class="row"><div style="flex:1;min-width:0"><div class="n">{n}</div><div class="d">{t}</div></div>'
                f'<button type="button" class="plus" style="width:32px;height:32px" aria-label="Add {n}" title="Add {n}" data-toast="Add {n}…">{ic("plus",14)}</button></div>')
    body = ('<div><h2 style="color:var(--text)">Add an account or a key</h2>'
      + d("GitHub", "Ratchet pushes candidate branches and opens the pull request as you.")
      + d("AWS", "The evaluator runs in a sandbox in your account, removed after each run.")
      + d("Claude Code API key", "An Anthropic key can also run in the cloud, with your laptop closed.")
      + d("Codex API key", "An OpenAI key can also run in the cloud, with your laptop closed.")
      + d("Grok API key", "A xAI key can also run in the cloud, with your laptop closed.") + '</div>')
    return connectors_page("Discover", body)

# ---- Agents
def agents():
    g = lambda t, d, extra="": f'<div class="grp" style="margin-top:20px"><b>{t}</b> {d}{extra}</div>'
    order = ('<div class="row" style="min-height:56px"><span class="logo" aria-hidden="true">→</span><div style="flex:1"><div class="n">Order</div><div class="d">Claude Code · on this Mac</div><div class="d">If one is out of quota or signed out, the next one takes over.</div></div>'
             + '<div style="position:relative">' + btn("Edit Order","sm",pop="pAgPg") + agents_pop("pAgPg", False, "top:calc(100% + 8px);right:0", where=False) + '</div></div>')
    redet = ("Re-detect", "Looks for the agent CLIs on this Mac again")
    body = (order
      + g("On this Mac", "your own logins · free while the Mac is awake", ' <button type="button" class="link" data-toast="Found Claude Code">Re-detect</button>')
      + crow("CC","Claude Code · your login","v2.1.314","ok","Signed in",badge='<span class="dflt">default</span>')
      + crow("CX","Codex · your login","Not found on this Mac","q","Not installed",fix=redet)
      + crow("GK","Grok · your login","Not found on this Mac","q","Not installed",fix=redet)
      + g("API keys", "run on this Mac or unattended in the cloud · billed to the key")
      + crow("CC","Claude Code · team key","API key ····4f2a · also runs in the cloud · billed to the key","ok","Valid")
      + crow("CX","Codex · ci key","API key ····91c0 · not used yet","ok","Valid")
      + crow("GK","Grok · xAI key","No key yet","off","No key",fix=("Add Key…","Paste an xAI key")))
    inner = page_head("Agents", "The agent CLIs Ratchet drives on this Mac, and the keys it can use.", btn("Add Agent…","",toast="Add Agent…")) + body
    return window(side("Agents"), toolbar("Agents", "Agent CLIs on this Mac"), page(inner, 1000))

# ---- New Optimization (creation conversation): three steps, then the draft card
def newconv(state="ok", pop_open=False):
    S = lambda k, t: f'<div class="step">{st(k)} {t}</div>'
    if state == "draft":
        steps = S("ok", "Read the repo") + S("run", "Pick a benchmark") + S("q", "Draft the environment")
    elif state == "ok":
        steps = S("ok", "Read the repo · acme-org/shop-api") + S("ok", "Picked HTTP latency · k6, hidden query mix as holdout") + S("ok", "Environment passes every check · $0.59/h")
    else:
        steps = S("ok", "Read the repo · acme-org/shop-api") + S("ok", "Picked HTTP latency · k6, hidden query mix as holdout") + S("bad", "Environment check failed · $1.40/h, over the $1/h limit")
    steps = f'<div class="steps" role="list" aria-label="Setup steps">{steps}</div>'
    def kvrow(k, v, aria, popid=None, upd=False, pop=""):
        u = f'<span class="upd">{ic("check",12)}Updated {btn("Undo","sm ghost",icon="undo",toast="Change undone")}</span>' if upd else ""
        b = (btn("Change", "sm chg ghost", pop=popid, aria=f"Change {aria}") if popid else btn("Change", "sm chg ghost", toast=f"Change {aria}", aria=f"Change {aria}"))
        return f'<div class="k">{k}</div><div>{v}{u}</div><div style="position:relative">{b}{pop}</div>'
    pop = agents_pop("pDraftAg", pop_open, "top:-24px;right:calc(100% + 8px)", lft=True)
    err = ""
    if state == "err":
        err = (f'<div class="errbox" role="alert">{ic("warn",16)}<div style="flex:1"><b>Couldn\'t lock the environment</b><div class="muted">A smaller setup (API + Postgres, no load generator host) fits under $1/h but measures less.</div></div>'
               f'{btn("Use Smaller Setup","sm",toast="Redrafting with the smaller setup…")}{btn("Try Again","sm",toast="Retrying the check")}</div>')
    kv = ('<div class="kv">'
      + kvrow("Goal", "Make checkout p95 faster", "goal")
      + kvrow("Benchmark", 'p95 of <span class="mono">POST /checkout</span> · lower is better', "benchmark")
      + kvrow("Agents", ORDER, "agents", "pDraftAg", False, pop)
      + kvrow("Repository", "acme-org/shop-api · main", "repository")
      + kvrow("Cloud", "acme-prod · us-east-1", "cloud")
      + kvrow("Schedule", "every night at 02:00 · first run now", "schedule", upd=(state == "ok"))
      + kvrow("Budget", "up to $50 and 2 h per run", "budget")
      + '</div>')
    dis = state == "err"
    note = '<div class="note2">Runs at once from <span class="mono">ratchet/config</span>. A PR adopts it into main later.</div>'
    ft = ('<div class="ft">' + btn("Start Over", "ghost", icon="undo", go="s1") + '<span class="sp"></span>'
          + btn("Create and Run Once · ≈$38", "dis" if dis else "", toast="Created · first run started now")
          + btn("Create Optimization", "pri" + (" dis" if dis else ""), go="s23", toast="Created · runs tonight at 02:00") + '</div>')
    card = (f'<div class="def"><div class="hd">{mark(18)}<b>Checkout p95 · nightly</b><span class="sp"></span><span class="s">≈ $1.20 per run</span></div>{err}{kv}{note}{ft}</div>')
    lead = {"ok": "Here's the optimization. Create it and it runs at once, then every night.",
            "err": "The environment needs a smaller setup. Pick a fix below, or say what to change.", "draft": ""}[state]
    leadh = f'<div style="margin:12px 0 12px;color:var(--textMuted)">{lead}</div>' if lead else ""
    attrib = "" if state == "draft" else (f'<div class="attrib">Written by <b>Claude Code</b> · <b>Opus</b> {btn("Retry","sm ghost",icon="undo",toast="Regenerating the draft…")}{btn("Copy","sm ghost",toast="Copied")}</div>')
    reply = (f'<div class="msg-r"><div class="who">{mark(22)}</div><div style="flex:1;min-width:0">'
             f'<div class="notice">{ic("info",14)}<span>First time: your prompt and repo name go to Claude Code on this Mac. Nothing else.</span><span class="sp"></span>{ibtn("x","Dismiss","sq",toast="Dismissed")}</div>'
             + steps + leadh + ("" if state == "draft" else card) + attrib + '</div></div>')
    action = (btn("Stop", "sm", icon="stop", aria="Stop drafting", toast="Stopped drafting") if state == "draft"
              else f'<button type="button" class="send" aria-label="Send (⌘↩)" title="Send (⌘↩)" data-toast="Sent">{ic("send",16)}</button>')
    comp = (f'<div class="dock"><div class="composer steer"><div class="ph" style="font-size:14px">Change anything in plain words, e.g. “use Grok too” or “only weekdays”.</div>'
            f'<div class="chips"><span class="s">⌘↩ sends</span><span class="sp"></span>{action}</div></div></div>')
    inner = f'<div class="scrl" style="gap:20px"><div class="msg-u">Make checkout p95 faster in shop-api. Run it every night, $50 a run.</div>{reply}</div>' + comp
    pg = f'<div class="page mid runpg" style="--w:900px;gap:20px;padding-top:20px">{inner}</div>'
    return window(side("Home"), toolbar("New Optimization", "Claude Code"), pg, cls="fix2")

# ---- Menu bar window (native window, brand content)
def menubar():
    b = lambda l, c="", **a: btn(l, c, **a)
    inner = (f'<div class="mbwin" role="dialog" aria-label="Ratchet menu bar"><div class="mbh">{mark(26)}<span class="serif" style="font-size:22px">Ratchet</span><span class="sp" style="color:var(--wait);font-size:12.5px">1 needs you</span></div>'
      '<div class="mbs"><div style="display:flex;align-items:baseline"><b>Checkout p95</b><span class="sp up mono">−30%</span></div><div class="bar" style="margin-top:8px"><i style="width:76%"></i></div><div class="s" style="margin-top:8px">1 h 12 m left · $38 spent</div></div>'
      f'<div class="mbs" style="display:flex;gap:12px;align-items:flex-start">{st("bad")}<div style="flex:1"><b>Sign in to acme-staging</b><div class="s">CLI cold start failed · AWS session expired</div></div>{btn("Sign In","sm",go="s5")}</div>'
      f'<div class="mbs"><span class="stt stt-ok">{st("ok")}<span>Sandbox on · acme-prod · $1.20 so far · off at 14:32</span></span><div class="s" style="margin-top:4px">Keeping this Mac awake while agents run.</div></div>'
      f'<div class="mbf">{b("Open Ratchet","",go="s1")}{b("New Optimization","pri",go="s8")}<button type="button" class="link sp" data-toast="Quit Ratchet">Quit</button></div></div>')
    return f'<div class="desk"><div class="mbar" aria-hidden="true">{mark(16)}</div>{inner}</div>'

# ---- Settings (Settings scene, ⌘,): the app's four tabs
def settings(active="General"):
    tabs = [("General", "sliders"), ("Notifications", "bell"), ("This Mac", "mac"), ("Advanced", "gear")]
    st_ = "".join(f'<span role="tab" tabindex="0" aria-selected="{str(n==active).lower()}" class="{"on" if n==active else ""}" data-pane="{n}">{ic(i,18)}{n}</span>' for n, i in tabs)
    S = lambda t, d, ctl: f'<div class="set"><div><div class="t">{t}</div>{f"<div class=d>{d}</div>" if d else ""}</div>{ctl}</div>'
    G = lambda body, note="": f'<div class="sgrp">{body}</div>' + (f'<div class="snote">{note}</div>' if note else "")
    panes = {
     "General": ('<div class="grph">Menu bar</div>'
        + G(S("Show Ratchet in the menu bar", "", tog(True, "Show Ratchet in the menu bar")), "Running optimizations, what needs you and the sandbox, one click away. Closing the window keeps Ratchet running while this is on.")
        + '<div class="grph">Default for new optimizations</div>'
        + G(S("Agents", "", '<span class="muted" style="margin-left:auto">Automatic</span>') + f'<div class="set">{btn("Change on the Agents Page","sm",go="s7")}</div>', "A new optimization starts from these. Each optimization shows “Using default” until you change it there.")),
     "Notifications": ('<div class="grph">Tell me when</div>'
        + G(S("Wins ready to merge", "A verified win is ready to become a pull request.", tog(True, "Wins ready to merge"))
          + S("Budget limit reached", "An optimization stopped because it spent its budget.", tog(True, "Budget limit reached"))
          + S("Run finished or failed", "An optimization finished, or a run failed.", tog(True, "Run finished or failed"))
          + S("Connector expired", "A GitHub, AWS or agent sign-in needs renewing.", tog(True, "Connector expired")), "Posted only while Ratchet is in the background.")),
     "This Mac": ('<div class="grph">Background service</div>'
        + G(S("Status", "", f'<span class="muted" style="margin-left:auto">Running</span>'), "Ratchet is managing the background service in this copy.")
        + '<div class="grph">Login item</div>'
        + G(S("Launch at login", "", tog(False, "Launch at login")), "Start Ratchet when you log in, with only the menu bar item, so notifications keep arriving after a restart.")
        + '<div class="grph">Command-line tool</div>'
        + G(S("ratchet", "", '<span class="muted" style="margin-left:auto">Not installed</span>') + f'<div class="set">{btn("Install","sm",toast="Installed to ~/.local/bin")}{btn("Install for All Users…","sm",toast="Asks for your password")}</div>',
            "Adds a ratchet command to your Terminal, for scripts and agents. It goes in ~/.local/bin. “All users” puts one in /usr/local/bin and asks for your password.")),
     "Advanced": ('<div class="grph">Data</div>'
        + G(S("Source", "", '<span class="muted" style="margin-left:auto">Live data</span>') + S("Optimizations", "", '<span class="mono" style="margin-left:auto">5</span>'))
        + G(S("Advanced (development)", "", btn("Show", "sm", toast="Development options"))) ),
    }
    pane_html = "".join(f'<div class="pane{" on" if n==active else ""}" data-panel="{n}">{h}</div>' for n, h in panes.items())
    return (f'<div class="setwin" role="dialog" aria-label="Settings"><div class="tbar"><div class="lights" aria-hidden="true"><i></i><i></i><i></i></div>'
            f'<div class="stabs" role="tablist" aria-label="Settings">{st_}</div></div>{pane_html}</div>')

def settings_screen(active):
    base = window(side("Overview"), toolbar("Overview", "90 days", search="Search optimizations"), page(overview(), 1000), cls="inactive")
    return '<div style="position:relative">' + base + settings(active) + "</div>"

# ---------------------------------------------------------------- assemble
S = []
S.append(screen("s1", "Home", home()))
S.append(screen("s9", "Overview", overview_win()))
S.append(screen("s23", "Optimizations", optimizations_win()))
S.append(screen("s2", "Run", run_win()))
S.append(screen("s3", "Optimization settings", optimization_win()))
S.append(screen("s4", "Benchmarks", benchmarks()))
S.append(screen("s5", "Connectors", connectors()))
S.append(screen("s6", "Settings", settings_screen("General")))
S.append(screen("s7", "Agents", agents()))
S.append(screen("s8", "New Optimization", newconv("ok")))
S.append(screen("s24", "New Optimization · agents", newconv("ok", True)))
S.append(screen("s10", "Connectors · Discover", discover()))
S.append(screen("s11", "Empty · first launch", overview_empty()))
S.append(screen("s12", "Daemon down", overview_down()))
S.append(screen("s13", "Failed connector", connectors(True)))
S.append(screen("s14", "Loading", overview_loading()))
S.append(screen("s15", "Home · light", home(focus=False), "light"))
S.append(screen("s16", "Overview · light", overview_win(), "light"))
S.append(screen("s17", "Run · light", run_win(), "light"))
S.append(screen("s18", "New Optimization · error", newconv("err")))
S.append(screen("s19", "Run · stop", run_win(variant="stop")))
S.append(screen("s20", "Run · by agent", run_win(variant="agent")))
S.append(screen("s21", "Settings · This Mac", settings_screen("This Mac")))
S.append(screen("s22", "New Optimization · drafting", newconv("draft")))
S.append(screen("s25", "Menu bar", menubar(), "dark", "width:420px"))

html = ('<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Ratchet v0.3.1 · mockups</title>'
        '<link href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600&family=Instrument+Serif&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">'
        f'<style>\n{CSS}</style></head><body>\n' + "".join(S) + "</body></html>\n")
open(os.path.join(D, "v03.html"), "w").write(html)
print("v03.html", len(html), "bytes,", len(S), "screens")

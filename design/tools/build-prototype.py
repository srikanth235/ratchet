#!/usr/bin/env python3
"""Regenerates design/v03-prototype.html (the clickable prototype) from design/v03.html.
Routes are declarative: v03.html marks every interactive element with data-go (screen), data-toast, data-pop (popover),
data-pane (settings tab) or data-swap (label toggle). Run build-v03.py first. Test: node design/tools/test-prototype.js"""
import os, re
D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
src = open(os.path.join(D, "v03.html")).read()
style = re.search(r"<style>(.*?)</style>", src, re.S).group(1)
secs = re.findall(r'<section class="screen" id="(s\d+)" data-name="([^"]*)"', src)
body = "".join(re.findall(r'<section class="screen".*?</section>\n', src, re.S))
chips = "".join(f'<button type="button" class="pj" data-go="{i}">{n}</button>' for i, n in secs)
extra = """<style>
html,body{background:var(--stage)}
body{padding-block:16px;padding-left:max(16px,env(safe-area-inset-left,0px));padding-right:max(16px,env(safe-area-inset-right,0px))}
.pbar{display:flex;flex-wrap:wrap;gap:8px;align-items:center;max-width:1440px;margin:0 auto 12px}
.pbar .nm{font:500 14px 'Instrument Sans',sans-serif;color:var(--textMuted);margin-right:12px}
.pj,.hs{font:12.5px 'Instrument Sans',sans-serif;background:none;border:1px solid var(--controlBorder);color:var(--textMuted);border-radius:999px;padding:4px 12px;cursor:pointer}
.pj.on{color:var(--brassInk);background:var(--brass500);border-color:var(--brass500)}
.hs{margin-left:auto}.hs[aria-pressed="true"]{color:var(--brass300);border-color:var(--brass300)}
.stage{width:1440px;margin:0 auto}
.screen{display:none;padding:0!important;width:1440px}.screen.cur{display:block}
.hot [data-go],.hot [data-toast],.hot [data-pop],.hot [data-pane],.hot [data-swap],.hot .tog,.hot .add{outline:1px dashed rgba(217,178,111,.7);outline-offset:1px}
.toast{position:fixed;left:50%;bottom:calc(24px + env(safe-area-inset-bottom,0px));transform:translateX(-50%);background:var(--night700);border:1px solid var(--controlBorder);color:var(--snow);padding:8px 16px;border-radius:8px;font:12.5px 'Instrument Sans',sans-serif;opacity:0;transition:opacity .2s;pointer-events:none;z-index:99}
.toast.show{opacity:1}
[data-go],[data-toast],[data-pop],[data-pane],[data-swap]{cursor:pointer}
:focus-visible{outline:2px solid var(--focus,var(--brass300));outline-offset:2px}
@media (prefers-reduced-motion:reduce){.toast{transition:none}}
</style>"""
js = r"""<script>
(function(){
let cur='s1',prev='s9';
const ICO=d=>'<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">'+d+'</svg>';
const CHECK=ICO('<path d="M3.5 8.5l3 3 6-7"/>'),PLUS=ICO('<path d="M8 3v10M3 8h10"/>');
const stage=document.querySelector('.stage');
function fit(){const w=Math.min(1,(document.documentElement.clientWidth-32)/1440);stage.style.zoom=w;document.querySelector('.pbar').style.maxWidth=(1440*w)+'px'}
addEventListener('resize',fit);fit();
const isSet=s=>s==='s6'||s==='s21';
function go(s){if(!document.getElementById(s))return;if(!isSet(s))prev=s;else if(!isSet(cur))prev=cur;cur=s;
 document.querySelectorAll('.screen').forEach(e=>e.classList.toggle('cur',e.id===s));
 document.querySelectorAll('.pj').forEach(b=>b.classList.toggle('on',b.dataset.go===s));
 scrollTo(0,0)}
let tt;function toast(m){const t=document.querySelector('.toast');t.textContent=m;t.classList.add('show');clearTimeout(tt);tt=setTimeout(()=>t.classList.remove('show'),1800)}
function pane(btn){const w=btn.closest('.setwin');w.querySelectorAll('.stabs span').forEach(o=>{o.classList.toggle('on',o===btn);o.setAttribute('aria-selected',o===btn)});
 w.querySelectorAll('.pane').forEach(p=>p.classList.toggle('on',p.dataset.panel===btn.dataset.pane))}
function swap(b){const [a,c]=b.dataset.swap.split('|');const n=[...b.childNodes].reverse().find(x=>x.nodeType===3);if(!n)return;
 const now=n.textContent.trim()===a;n.textContent=now?c:a;toast(now?'Paused':'Resumed')}
function click(el){
 let x;
 if(x=el.closest('.hs')){const on=x.getAttribute('aria-pressed')!=='true';x.setAttribute('aria-pressed',on);document.body.classList.toggle('hot',on);return}
 if(el.closest('.pbar')){x=el.closest('[data-go]');if(x)go(x.dataset.go);return}
 if(isSet(cur)&&el.closest('.screen')&&!el.closest('.setwin')){return go(prev)}
 if(x=el.closest('[data-pop]')){const p=document.getElementById(x.dataset.pop);if(p){const o=!p.classList.contains('open');
   document.querySelectorAll('.pop.open').forEach(q=>{if(q!==p)q.classList.remove('open')});p.classList.toggle('open',o);
   document.querySelectorAll('[data-pop="'+x.dataset.pop+'"].chip').forEach(c=>{c.classList.toggle('open',o);c.setAttribute('aria-expanded',o)})}return}
 if(!el.closest('.pop')&&document.querySelector('.screen.cur .pop.open')){document.querySelectorAll('.screen.cur .pop.open').forEach(q=>q.classList.remove('open'));document.querySelectorAll('.screen.cur .chip.open').forEach(c=>c.classList.remove('open'))}
 if(x=el.closest('.seg2 span,.tabs span')){x.parentElement.querySelectorAll(':scope>.on').forEach(o=>{o.classList.remove('on');o.setAttribute('aria-selected','false')});x.classList.add('on');x.setAttribute('aria-selected','true')}
 if(x=el.closest('.stabs span[data-pane]'))return pane(x);
 if(x=el.closest('.tog')){const on=!x.classList.contains('on');x.classList.toggle('on',on);x.setAttribute('aria-checked',on);return}
 if(x=el.closest('.add')){const on=!x.classList.contains('done');x.classList.toggle('done',on);x.innerHTML=on?CHECK:PLUS;x.setAttribute('aria-label',(on?'Remove ':'Add ')+x.closest('.cc').querySelector('.n').textContent.trim());
   const n=x.closest('.cc').querySelector('.n').textContent.trim();return toast(on?n+' added · pick it per optimization':n+' removed')}
 if(x=el.closest('[data-swap]'))return swap(x);
 if(x=el.closest('[data-toast],[data-go]')){if(x.dataset.toast)toast(x.dataset.toast);if(x.dataset.go)go(x.dataset.go)}
}
document.addEventListener('click',ev=>click(ev.target));
document.addEventListener('keydown',e=>{
 if(e.key==='Escape'){if(isSet(cur))go(prev);else document.querySelectorAll('.pop.open').forEach(q=>q.classList.remove('open'))}
 if((e.metaKey||e.ctrlKey)&&e.key===','){e.preventDefault();go('s6')}
 if((e.key==='Enter'||e.key===' ')&&e.target.matches('[role=link],[role=tab],[role=button],[role=search]')){e.preventDefault();click(e.target)}
});
window.__go=go;
go('s1');
})();
</script>"""
out = f"""<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Ratchet v0.3.1 · clickable prototype</title>
<link href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600&family=Instrument+Serif&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>{style}</style>
{extra}</head><body>
<div class="pbar"><span class="nm">Ratchet v0.3.1 · clickable prototype</span>{chips}<button type="button" class="hs" aria-pressed="false">Show hotspots</button></div>
<div class="stage">{body}</div>
<div class="toast" role="status" aria-live="polite"></div>
{js}</body></html>
"""
open(os.path.join(D, "v03-prototype.html"), "w").write(out)
print(len(out), [i for i, _ in secs])

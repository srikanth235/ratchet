import os
D=os.path.join(os.path.dirname(os.path.abspath(__file__)),"..")
import re,sys
src=open(os.path.join(D,'v03.html')).read()
head=re.search(r'<head>(.*?)</head>',src,re.S).group(1)
head=re.sub(r'<title>.*?</title>','',head)
head=re.sub(r'<meta[^>]*>','',head)
body=re.search(r'<body>(.*)</body>',src,re.S).group(1)
secs=re.findall(r'<section class="screen" id="(s\d+)">.*?</section>',body,re.S)
body=''.join(re.findall(r'<section class="screen" id="s\d+">.*?</section>',body,re.S))
names={'s1':'Home','s2':'Run','s3':'Optimization','s4':'Templates','s5':'Connectors','s6':'Settings','s7':'Agents','s8':'New optimization','s9':'Overview','s10':'Connectors · Discover'}
chips=''.join(f'<button type="button" class="pj" data-go="{s}">{names[s]}</button>' for s in secs)
extra='''<style>
:root{color-scheme:dark}
html,body{background:var(--bg)}
body{padding-block:16px;padding-left:max(16px,env(safe-area-inset-left,0px));padding-right:max(16px,env(safe-area-inset-right,0px))}
.pbar{display:flex;flex-wrap:wrap;gap:6px;align-items:center;max-width:1440px;margin:0 auto 12px}
.pbar .nm{font:500 13px 'Instrument Sans',sans-serif;color:var(--mu);margin-right:10px}
.pj,.hs{font:12.5px 'Instrument Sans',sans-serif;background:none;border:1px solid var(--line2);color:var(--mu);border-radius:999px;padding:5px 11px;cursor:pointer}
.pj.on{color:#16110A;background:var(--br);border-color:var(--br)}
.hs{margin-left:auto}.hs[aria-pressed="true"]{color:var(--br2);border-color:var(--br2)}
.stage{width:1440px;margin:0 auto}
.screen{display:none;padding:0!important;width:1440px}.screen.cur{display:block}
.nv,.run,.btn,.tabs span,.seg2 span,.send,.sugg span,.tog,.mnav .i,.link,.edit,.chip,.tile .plus,.sel,.composer,.add,.arr,.cur-hero .btn{cursor:pointer}
.nv:hover,.run:hover{background:rgba(148,163,190,.07)}
.btn:hover,.sugg span:hover,.chip:hover{border-color:var(--br2)}
.hot .nv,.hot .run,.hot .btn,.hot .tabs span,.hot .seg2 span,.hot .send,.hot .sugg span,.hot .tog,.hot .mnav .i,.hot .link,.hot .edit,.hot .composer{outline:1px dashed rgba(217,178,111,.7);outline-offset:1px}
.toast{position:fixed;left:50%;bottom:calc(24px + env(safe-area-inset-bottom,0px));transform:translateX(-50%);background:#1A2233;border:1px solid var(--line2);color:var(--tx);padding:10px 16px;border-radius:10px;font:13px 'Instrument Sans',sans-serif;opacity:0;transition:opacity .2s;pointer-events:none}
.toast.show{opacity:1}
.setnv{margin-top:6px;padding:7px 10px;border-radius:8px;color:var(--mu);cursor:pointer;font-size:13px}.setnv:hover{background:rgba(148,163,190,.07)}
:focus-visible{outline:2px solid var(--br2);outline-offset:2px}
@media (prefers-reduced-motion:reduce){.toast{transition:none}}
</style>'''
js=r'''<script>
(function(){
const N=%s;
let cur='s9',prev='s9';
const stage=document.querySelector('.stage');
function fit(){const w=Math.min(1,(document.documentElement.clientWidth-32)/1440);stage.style.zoom=w;document.querySelector('.pbar').style.maxWidth=(1440*w)+'px'}
addEventListener('resize',fit);fit();
function go(s){if(!document.getElementById(s))return;if(s!=='s6'&&cur!=='s6')prev=cur;else if(s==='s6'&&cur!=='s6')prev=cur;cur=s;
 document.querySelectorAll('.screen').forEach(e=>e.classList.toggle('cur',e.id===s));
 document.querySelectorAll('.pj').forEach(b=>b.classList.toggle('on',b.dataset.go===s));
 scrollTo(0,0)}
let tt;function toast(m){const t=document.querySelector('.toast');t.textContent=m;t.classList.add('show');clearTimeout(tt);tt=setTimeout(()=>t.classList.remove('show'),1800)}
// settings entry in each sidebar footer
document.querySelectorAll('.side').forEach(sd=>{const f=[...sd.children].find(c=>/Sandbox on/.test(c.textContent));const d=document.createElement('div');d.className='setnv';d.textContent='⚙ Settings';d.tabIndex=0;d.dataset.go='s6';(f?sd.insertBefore(d,f):sd.appendChild(d))});
const nav={'New optimization':'s1','Search':'s1','Overview':'s9','Optimizations':'s3','Customize':'s4'};
const tabs={'Templates':'s4','Connectors':'s5','Agents':'s7','Agents on this Mac':'s7'};
const t=e=>e.textContent.trim();
document.addEventListener('click',ev=>{
 const el=ev.target;let x;
 if(x=el.closest('[data-go]'))return go(x.dataset.go);
 if(x=el.closest('.hs')){const on=x.getAttribute('aria-pressed')!=='true';x.setAttribute('aria-pressed',on);document.body.classList.toggle('hot',on);return}
 if(cur==='s6'&&el.closest('#s6')&&!el.closest('.modal')&&!el.closest('.side'))return go(prev);
 if(x=el.closest('.nv')){const k=t(x.querySelector('span')||x);if(k==='Search')toast('Search: ⌘K');return go(nav[k]||'s1')}
 if(x=el.closest('.run')){return go(/queued/.test(x.textContent)?'s3':'s2')}
 if(x=el.closest('.tabs span,.seg2 span,.mnav .i')){const g=x.parentElement;g.querySelectorAll(':scope>.on').forEach(o=>o.classList.remove('on'));x.classList.add('on');const k=t(x);if((cur==='s5'||cur==='s10')&&(k==='Yours'||k==='Discover'))return go(k==='Yours'?'s5':'s10');if(tabs[k])go(tabs[k]);return}
 if(x=el.closest('.add')){const on=!x.classList.contains('done');x.classList.toggle('done',on);x.textContent=on?'✓':'+';const n=x.parentElement.querySelector('.n').textContent.trim();return toast(on?n+' added · pick it per optimization':n+' removed')}
 if(x=el.closest('.arr')){return toast('Next collection: Cloud providers')}
 if(x=el.closest('.tog')){x.classList.toggle('on');return}
 if(x=el.closest('.btn')){const k=t(x);
  if(k==='Create optimization'){toast('Created · first run tonight at 02:00');return go('s3')}
  if(k==='Run once now'||k==='Run now'){toast('Run started');return go('s2')}
  if(k==='Use template')return go('s8');
  if(k==='New optimization')return go('s1');
  if(/^Open PR/.test(k))return toast('Opens '+k.slice(5)+' on GitHub');
  if(k==='Pause'){x.textContent='Resume';return toast('Paused · no runs until you resume')}
  if(k==='Resume'){x.textContent='Pause';return toast('Resumed')}
  if(k==='Sign in')return toast('Opens codex login in Terminal');
  if(/^\+ Add/.test(k))return toast('Add a connector: GitHub, cloud, notifications or custom MCP');
  if(k==='Explore')return toast('Observability connectors: Datadog, Grafana, Honeycomb, Sentry, New Relic, Prometheus');
  return toast(k)}
 if(cur==='s1'&&(x=el.closest('.send,.sugg span,.composer')))return go('s8');
 if(x=el.closest('.edit'))return toast('Edit inline, or say it in plain words below');
 if(x=el.closest('.link')){const k=t(x);return toast(k==='Re-detect'?'Found Claude Code, Codex, Grok':'Opens PR #312 on GitHub')}
});
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&cur==='s6')go(prev);if((e.metaKey||e.ctrlKey)&&e.key===',' ){e.preventDefault();go('s6')}
 if(e.key==='Enter'&&e.target.matches('.setnv'))go('s6')});
go('s9');
})();
</script>'''%repr(names).replace("'",'"')
out=f'''<title>Ratchet v0.3</title>
<meta name="claude:viewport-fit-sides" content="cover">
{head}
{extra}
<div class="pbar"><span class="nm">Ratchet v0.3 · clickable prototype</span>{chips}<button type="button" class="hs" aria-pressed="false">Show hotspots</button></div>
<div class="stage">{body}</div>
<div class="toast" role="status" aria-live="polite"></div>
{js}'''
open(os.path.join(D,'v03-prototype.html'),'w').write(out)
print(len(out),secs)

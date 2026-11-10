// Shared pieces of the studio mock-ups: icons, fake pictures, the chrome every page repeats,
// and mount(), which turns a table of frames into a switcher. Reference only.
document.body.insertAdjacentHTML('afterbegin',String.raw`<svg width="0" height="0" style="position:absolute"><defs>
<symbol id="layers" viewBox="0 0 24 24"><path d="M12 3 3 8l9 5 9-5-9-5ZM3 13l9 5 9-5M3 18l9 5 9-5" transform="translate(0 -1.5)"/></symbol>
<symbol id="plus" viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></symbol>
<symbol id="sliders" viewBox="0 0 24 24"><path d="M6 4v8m0 4v4M12 4v3m0 4v9M18 4v10m0 4v2M4 14h4m2-5h4m2 7h4"/></symbol>
<symbol id="users" viewBox="0 0 24 24"><circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.6-3.6 3.2-5.5 6.5-5.5s5.9 1.900 6.500 5.500M16 4.800a3.500 3.500 0 0 1 0 6.400M18.500 14.800c1.700.800 2.700 2.500 3 5.200"/></symbol>
<symbol id="rewind" viewBox="0 0 24 24"><path d="M11 6 3 12l8 6V6ZM21 6l-8 6 8 6V6Z"/></symbol>
<symbol id="wand" viewBox="0 0 24 24"><path d="m4 20 11-11m2-5v3m-1.500-1.500h3M20 11v2m-1-1h2M9 4v2M8 5h2"/></symbol>
<symbol id="gear" viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M19 5l-2 2M7 17l-2 2"/></symbol>
<symbol id="mic" viewBox="0 0 24 24"><rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/></symbol>
<symbol id="vol" viewBox="0 0 24 24"><path d="M4 9v6h4l5 4V5L8 9H4ZM16.500 8.500a5 5 0 0 1 0 7M19 6a9 9 0 0 1 0 12"/></symbol>
<symbol id="mon" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></symbol>
<symbol id="win" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 9h18M7 6.500h.01M10 6.500h.01"/></symbol>
<symbol id="cam" viewBox="0 0 24 24"><rect x="3" y="6" width="13" height="12" rx="2"/><path d="m16 10 5-3v10l-5-3"/></symbol>
<symbol id="phones" viewBox="0 0 24 24"><path d="M4 14a8 8 0 0 1 16 0v4a2 2 0 0 1-2 2h-1v-6h3M4 14h3v6H6a2 2 0 0 1-2-2v-4Z"/></symbol>
<symbol id="eye" viewBox="0 0 24 24"><path d="M2 12s3.500-7 10-7 10 7 10 7-3.500 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></symbol>
<symbol id="lock" viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/></symbol>
<symbol id="clip" viewBox="0 0 24 24"><circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M8.500 8 20 19M8.500 16 20 5"/></symbol>
<symbol id="search" viewBox="0 0 24 24"><circle cx="11" cy="11" r="6.500"/><path d="m16 16 5 5"/></symbol>
<symbol id="undo" viewBox="0 0 24 24"><path d="M9 7 4 12l5 5M4 12h10a6 6 0 0 1 0 12h-2" transform="translate(0 -3)"/></symbol>
<symbol id="shield" viewBox="0 0 24 24"><path d="M12 3 4 6v6c0 5 3.500 8 8 9 4.500-1 8-4 8-9V6l-8-3Z"/></symbol>
<symbol id="globe" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3.500 3 14.500 0 18M12 3c-3 3.500-3 14.500 0 18"/></symbol>
<symbol id="type" viewBox="0 0 24 24"><path d="M5 6V4h14v2M12 4v16M9 20h6"/></symbol>
<symbol id="img" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="m4 18 5-4 4 3 3-2 4 3"/></symbol>
<symbol id="crop" viewBox="0 0 24 24"><path d="M6 2v16h16M2 6h16v16"/></symbol>
<symbol id="link" viewBox="0 0 24 24"><path d="M10 14a4 4 0 0 0 6 0l3-3a4 4 0 0 0-6-6l-1 1M14 10a4 4 0 0 0-6 0l-3 3a4 4 0 0 0 6 6l1-1"/></symbol>
<symbol id="cue" viewBox="0 0 24 24"><rect x="3" y="5" width="8" height="14" rx="1.500"/><rect x="13" y="5" width="8" height="14" rx="1.500"/></symbol>
<symbol id="flag" viewBox="0 0 24 24"><path d="M5 21V4h12l-2 4 2 4H5"/></symbol>
<symbol id="eyeoff" viewBox="0 0 24 24"><path d="M3 3l18 18M10 6a9 9 0 0 1 2-.200c6.500 0 10 6.200 10 6.200a17 17 0 0 1-3 3.800M6 7.500A17 17 0 0 0 2 12s3.500 7 10 7a9 9 0 0 0 4-1"/></symbol>
<symbol id="film" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M8 4v16M16 4v16M3 9h5M3 15h5M16 9h5M16 15h5"/></symbol>
</defs></svg>
<template id="t-game"><div class="pic game"><div class="sun"></div><div class="grid"></div><div class="hp"></div><div class="map"></div><div class="ammo">24 / 120</div><div class="cam"></div><div class="l3">KIXELATED<small>Ranked grind, day 12</small></div></div></template>
<template id="t-chat"><div class="pic chat"><div class="cam"></div><div class="col"></div></div></template>
<template id="t-brb"><div class="pic brb"><b>BRB</b></div></template>
<template id="t-soon"><div class="pic soon"><b>STARTING<br>SOON</b></div></template>
<template id="t-duo"><div class="pic duo"><div class="cam a"></div><div class="cam b"></div></div></template>
<template id="t-close"><div class="pic close"><div class="obj"></div></div></template>

<template id="t-sell"><div class="pic close port"><div class="obj"></div><div class="price">$42</div><div class="cam"></div><div class="l3">LOT 14<small>Vintage rangefinder, tested</small></div></div></template>
<template id="t-scenes">
  <div class="scenes">
    <div class="pad" data-s="soon"><div class="th" data-pic="soon"></div><div class="nm">Starting soon <kbd>F1</kbd></div></div>
    <div class="pad" data-s="game"><div class="th" data-pic="game"></div><div class="nm">Gameplay <kbd>F2</kbd></div></div>
    <div class="pad" data-s="chat"><div class="th" data-pic="chat"></div><div class="nm">Just chatting <kbd>F3</kbd></div></div>
    <div class="pad" data-s="duo"><div class="th" data-pic="duo"></div><div class="nm">Guest duo <kbd>F4</kbd></div></div>
    <div class="pad" data-s="brb"><div class="th" data-pic="brb"></div><div class="nm">BRB <kbd>F5</kbd></div></div>
    <div class="pad add"><svg class="i"><use href="#plus"/></svg></div>
    <div class="grow"></div>
    <div class="tr"><div class="chip">Fade · 300 ms ▾</div></div>
  </div>
</template>

<template id="t-mix">
  <div class="mix">
    <div class="chs">
      <div class="ch"><svg class="i"><use href="#mic"/></svg><span class="n">Desk mic</span><div class="led hot" style="--v:92%"></div><div class="mute"><svg class="i sm"><use href="#mic"/></svg></div></div>
      <div class="ch"><div class="ic" style="width:18px;height:18px;border-radius:5px;background:#e2574c"></div><span class="n">Neon Drift</span><div class="led" style="--v:64%"></div><div class="mute"><svg class="i sm"><use href="#vol"/></svg></div></div>
      <div class="ch"><div class="ic" style="width:18px;height:18px;border-radius:5px;background:#1db954"></div><span class="n">Music</span><div class="led" style="--v:38%"></div><div class="mute"><svg class="i sm"><use href="#vol"/></svg></div></div>
      <div class="ch dim"><div class="ic" style="width:18px;height:18px;border-radius:5px;background:#5865f2"></div><span class="n">Voice chat</span><div class="led" style="--v:0%"></div><div class="mute off"><svg class="i sm"><use href="#vol"/></svg></div></div>
    </div>
    <div class="master"><span class="cap">Stream</span><div class="vled"></div><svg class="i dim"><use href="#phones"/></svg></div>
  </div>
</template>

<template id="t-chatpanel">
  <div class="side">
    <div class="tabs"><span class="on">Chat</span><span>Activity</span><span>Rundown</span></div>
    <div class="msgs">
      <div><b style="color:#ff9f43">mossy_owl</b> that clutch was unreal</div>
      <div><b style="color:#54a0ff">dana</b> is the cam covering the minimap for anyone else?</div>
      <div class="ev"><b>pixelfern</b> followed · <span class="dim">2 s ago</span></div>
      <div><b style="color:#5f27cd">quietriver</b> gg</div>
      <div><b style="color:#1dd1a1">hopper</b> what sens do you play on</div>
      <div><b style="color:#ff6b6b">tin_kettle</b> clip that!!</div>
      <div class="ev" style="border-color:var(--air)"><b>lowtide</b> gifted 5 subs</div>
      <div><b style="color:#feca57">marrow</b> first time here, this stream looks so clean</div>
      <div><b style="color:#48dbfb">ibis</b> song name?</div>
      <div><b style="color:#ff9ff3">vellum</b> can you show the loadout again</div>
    </div>
    <div class="input sm"><svg class="i"><use href="#lock"/></svg>Private panel, never on stream</div>
  </div>
</template>

<template id="t-inspector">
  <div class="side">
    <div class="tabs"><span class="on">Facecam</span><span>Animate</span></div>
    <div class="sec"><span class="cap">Place</span>
      <div class="f"><span><em>X</em>1498</span><span><em>Y</em>650</span><span><em>W</em>365</span><span><em>H</em>365</span></div>
      <div class="row"><span class="grow">Keep out of the way</span><span class="dim" style="font-size:11px">moves off what matters</span><i class="tog on"></i></div>
    </div>
    <div class="sec"><span class="cap">Shape</span>
      <div class="shape"><span><i></i></span><span><i style="border-radius:6px"></i></span><span class="on"><i style="border-radius:50%;background:var(--sel)"></i></span><span><i style="border-radius:50% 50% 4px 4px"></i></span></div>
      <div class="sl"><span class="n">Border</span><div class="bar" style="--v:30%"></div></div>
      <div class="sl"><span class="n">Shadow</span><div class="bar" style="--v:55%"></div></div>
    </div>
    <div class="sec"><span class="cap">Look</span>
      <div class="row"><span class="grow">Remove background</span><i class="tog"></i></div>
      <div class="row"><span class="grow">Color grade</span><span class="chip">Warm film ▾</span></div>
      <div class="sl"><span class="n">Brightness</span><div class="bar" style="--v:52%"></div></div>
      <div class="row dim"><svg class="i sm"><use href="#plus"/></svg>Add a filter</div>
    </div>
    <div class="sec" style="border:0"><span class="cap">Shown in</span>
      <div class="row"><svg class="i sm"><use href="#link"/></svg><span class="grow">Linked in 3 scenes</span><span class="chip">Make independent</span></div>
    </div>
  </div>
</template>

<template id="t-layers">
  <div class="drawer">
    <div class="tabs"><span class="on">Layers</span><span>Gameplay</span></div>
    <div style="padding:8px 0;flex:1">
      <div class="ly dim"><span class="cap">Always on top</span></div>
      <div class="ly"><div class="ic" style="background:#3a3a46"><svg class="i sm"><use href="#globe"/></svg></div><div class="grow">Alerts<small>Web page</small></div><svg class="i sm dim"><use href="#eye"/></svg></div>
      <div class="ly dim" style="margin-top:6px"><span class="cap">This scene</span></div>
      <div class="ly"><div class="ic" style="background:#3a3a46"><svg class="i sm"><use href="#type"/></svg></div><div class="grow">Lower third<small>Overlay</small></div><svg class="i sm dim"><use href="#eye"/></svg></div>
      <div class="ly on"><div class="ic" style="background:#3b6cf6"><svg class="i sm"><use href="#cam"/></svg></div><div class="grow">Facecam<small>USB camera · linked ×3</small></div><svg class="i sm"><use href="#eye"/></svg></div>
      <div class="ly"><div class="ic" style="background:#e2574c;font:700 13px var(--disp)">N</div><div class="grow">Neon Drift<small>Window · follows the game</small></div><svg class="i sm dim"><use href="#lock"/></svg></div>
      <div class="ly"><div class="ic" style="background:#3a3a46"><svg class="i sm"><use href="#img"/></svg></div><div class="grow">Backdrop<small>Image</small></div><svg class="i sm dim"><use href="#eye"/></svg></div>
    </div>
    <div class="input sm" style="color:var(--text)"><svg class="i"><use href="#plus"/></svg>Add a layer <span class="grow"></span><kbd>A</kbd></div>
  </div>
</template>
<div class="switch" id="sw"></div>
<div class="fit mock" id="fit"></div>
<p class="note" id="note"></p>`);

const T=id=>document.getElementById('t-'+id).innerHTML;
const pic=k=>T(k);
const I=(n,c='')=>`<svg class="i ${c}"><use href="#${n}"/></svg>`;
const NAV=p=>`<div class="nav">${['LIVE','BUILD','MIX','REVIEW'].map(n=>`<span class="${n==p?'on':''}">${n}</span>`).join('')}</div>`;
function topbar(page,o={}){return `<div class="top">
  <div class="logo"></div>${o.compact?'':'<div class="show">Friday Night Ranked <span class="dim">▾</span></div>'}
  ${o.compact?'':`<div class="ib" style="margin-left:6px">${I('undo')}</div><div class="ib" style="transform:scaleX(-1)">${I('undo')}</div>`}
  <div class="grow"></div>${o.nav===false?'':NAV(page)}<div class="grow"></div>
    <div class="pill"><i class="dot"></i>${o.compact?'':'Excellent'}</div>
  ${o.compact?'':`<div class="ib b"><i class="dot" style="background:var(--air)"></i></div>`}
  ${o.end?'<div class="onair end">END STREAM</div>':'<div class="onair">LIVE <b>01:24:07</b></div>'}
  ${o.compact?'':`<div class="ib">${I('gear')}</div><div class="wc"><i></i><i></i><i></i></div>`}
</div>`}
function stage(o={}){
  const k=o.pic||'game', cue=o.cue;
  return `<div class="stagewrap">${o.extra||''}<div class="stage"><div class="tally ${cue?'cue':''} ${o.port?'port':''}"><span class="tag ${cue?'cue':''}">${cue?'OFF AIR':'ON AIR'}</span>${pic(k)}${o.sel?'<div class="guide"></div><div class="selbox"><i></i><i></i><i></i><i></i></div><div class="ctx"><span>'+I('crop','sm')+'Crop</span><span>Fit</span><span>Fill</span><span class="on">Shape</span><span>'+I('wand','sm')+'Cut out</span><span>⋯</span></div>':''}</div></div></div>`;
}
const quick=`<div style="display:flex;flex-direction:column;gap:6px;justify-content:center;padding:0 14px;border-left:1px solid var(--line)"><div class="q panic sm">${I('shield')}BRB</div><div class="row" style="gap:6px"><div class="ib b">${I('clip')}</div><div class="ib b">${I('eyeoff')}</div></div></div>`;
const scenes=(studio)=>T('scenes').replace('<div class="tr"><div class="chip">Fade · 300 ms ▾</div></div>',`<div class="ib b ${studio?'cue':''}">${I('cue')}</div>`);
const dock=(studio)=>`<div class="dock">${scenes(studio)}${quick}${T('mix')}</div>`;
const layersCol=()=>`<div class="side l"><div class="tabs"><span class="on">Layers</span><span>Add</span></div>${T('layers').replace('class="drawer"','style="display:contents"').replace(/<div class="tabs">.*?<\/div>/s,'')}</div>`;
const snav=p=>`<div class="snav">${[['LIVE','cam'],['BUILD','layers'],['MIX','sliders'],['REVIEW','film']].map(([n,i])=>`<span class="${n==p?'on':''}">${I(i)}${n}</span>`).join('')}</div>`;

function air(f,s){const p=f.querySelector(`.pad[data-s=${s}]`);if(p){p.classList.add('air');p.insertAdjacentHTML('afterbegin','<span class="tg">ON AIR</span>')}}
function cue(f,s){const p=f.querySelector(`.pad[data-s=${s}]`);if(p){p.classList.add('cued');p.insertAdjacentHTML('afterbegin','<span class="tg">NEXT</span>')}}
// V maps a frame id to {label, cls, note, html(), after(frame)}. `#shot:<id>` shows one frame bare, for a screenshot.
function mount(V){
  const fitEl=document.getElementById('fit'),sw=document.getElementById('sw'),note=document.getElementById('note');
  for(const [k,v] of Object.entries(V)){
    const f=document.createElement('div');f.className='frame '+v.cls;f.dataset.v=k;f.innerHTML=v.html();
    f.querySelectorAll('[data-pic]').forEach(e=>e.innerHTML=pic(e.dataset.pic));
    v.after&&v.after(f);fitEl.appendChild(f);
    const b=document.createElement('button');b.textContent=v.label;b.onclick=()=>show(k);b.dataset.v=k;sw.appendChild(b);
  }
  function show(k){document.querySelectorAll('.frame').forEach(f=>f.classList.toggle('on',f.dataset.v==k));
    sw.querySelectorAll('button').forEach(b=>b.setAttribute('aria-pressed',b.dataset.v==k));note.innerHTML=V[k].note}
  function fit(){const s=fitEl.clientWidth/1440;fitEl.style.height=(900*s)+'px';document.querySelectorAll('.frame').forEach(f=>f.style.transform='scale('+s+')')}
  const h=location.hash.replace('#','');if(h.startsWith('shot:'))document.body.classList.add('shot');
  const k=h.replace('shot:','');show(V[k]?k:Object.keys(V)[0]);fit();addEventListener('resize',fit);
}

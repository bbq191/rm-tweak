const $=(s,r=document)=>r.querySelector(s);
const fmtB=n=>n>1048576?(n/1048576).toFixed(1)+' MB':n>1024?(n/1024).toFixed(0)+' KB':n+' B';
// 小徽章：renderManage 的「基石与模块」列表用。
const badge=(t,ok)=>`<span class="badge ${ok?'on':'off'}">${t}</span>`;
/* 停一会儿再继续：用在"先弹出一条状态文字，再触发会重画掉这条文字的动作"这种场景——不等的话状态
   文字刚显示就被紧跟着的重画冲掉，用户根本来不及看见（点重转/生成笔记本弹出消耗那次踩过的坑）。 */
const wait=ms=>new Promise(res=>setTimeout(res,ms));
/* 防双击：按钮点击后立即禁用，异步操作完成（不管成功失败）再解禁。很多按钮的异步操作是删除/
   落库这类不该被同一次操作重复触发两遍的动作——不加这一层，手指点快了或者网络慢的时候网络请求
   还没回来就能再点一次，2026-09-18 真机反馈"优化过程中点击删除"这类并发操作会撞在一起。母版库
   列表 stagingList 的 btn() 助手（2026-09-19 起）也走这个——原来自己手写了一遍禁用/复位逻辑，
   跟这里是同一件事的第二份实现，改成在这层之外只叠列表特有的按钮文案/本地忙态记账。已经自己
   一开始就手动 disabled=true 的按钮（超限/未安装这类"根本点不了"，不是"点了在跑"）不需要套这层。*/
const guardClick=(el,fn)=>{el.onclick=async()=>{if(el.disabled)return;el.disabled=true;try{await fn()}finally{el.disabled=false}}};
/* 轻量 DOM 构建 helper：`el('div',{class:'small',style:'...'},[child1,child2])`。`attrs` 里
   `class`/其余属性走 `setAttribute`，`style` 走 `style.cssText`，`text`/`html` 分别设
   `textContent`/`innerHTML`；`children` 接单个节点/字符串或数组。不是要把全站手写 DOM 都机械
   替换一遍——只在改动到的地方（stagingList 这类同类节点最密集的函数）顺手用，别的地方不动
   （2026-09-19 代码质量审计范围说明）。*/
const el=(tag,attrs,children)=>{const n=document.createElement(tag);
  if(attrs)for(const k in attrs){const v=attrs[k];if(k==='style')n.style.cssText=v;else if(k==='text')n.textContent=v;else if(k==='html')n.innerHTML=v;else n.setAttribute(k,v)}
  if(children!=null)for(const c of [].concat(children))n.appendChild(typeof c==='string'?document.createTextNode(c):c);
  return n};
/* 结构化步数进度展示：`prog={done,total}` 有数据画真百分比，没有画不确定态滚动条（浏览器原生
   `<progress>` 不带 value/max 渲染成不确定态动画）——从 stagingList 原地实现抽出来，同样适用于
   任何"耗时不短、有时有分步数据有时没有"的忙态展示（`container` 是要挂这块的父节点，自己占
   一整行）。 */
const renderStepProgress=(container,{label,prog,msg})=>{
  const bar=el('progress');
  if(prog){bar.max=prog.total;bar.value=prog.done}
  const text=el('div',{class:'small',text:prog?`${label} ${prog.done}/${prog.total}（${Math.round(prog.done/prog.total*100)}%）${msg?' · '+msg:''}`:label});
  const wrap=el('div',{style:'flex-basis:100%;margin-top:.2em'},[bar,text]);
  container.appendChild(wrap);
  return wrap};
/* 全局 toast：系统里不允许用浏览器原生 alert（打断操作、要点掉才能继续，风格跟页面其它地方的行内
   小字状态提示完全不一致），全站原来散落的 26 处 alert() 统一改走这个（2026-09-19 用户明确要求）。
   #toasthost 惰性建：第一次调用 toast() 时才挂进 body，不用改 index.html。kind 决定配色（跟徽章
   同一套 --ok/--bad/--warn 变量，见 style.css），缺省 'bad'——历史上这堆 alert() 十有八九是报错。
   点一下提前关掉；到时自动淡出+移除。 */
const toastHost=(()=>{let el=document.getElementById('toasthost');if(!el){el=document.createElement('div');el.id='toasthost';document.body.appendChild(el)}return el})();
const toast=(msg,kind='bad',ms=4200)=>{if(!msg)return;const t=document.createElement('div');t.className='toast '+kind;t.textContent=msg;toastHost.appendChild(t);
  requestAnimationFrame(()=>t.classList.add('show'));
  const kill=()=>{t.classList.remove('show');setTimeout(()=>t.remove(),200)};
  t.onclick=kill;setTimeout(kill,ms)};
/* 自定义确认框：浏览器原生 confirm() 跟已经禁掉的 alert() 是同一类问题——阻塞整个页面、样式跟
   站内其它地方完全脱节，全站原来散落的 9 处 confirm() 统一改走这个（2026-09-19 用户反馈"母版库
   删除确认还是 alert"——严格说原来用的是 confirm() 不是 alert()，但对用户来说是同一类"浏览器
   弹出个原生对话框"体验，touch 一次改到底，不分是 alert 还是 confirm）。返回 `Promise<boolean>`，
   调用方需要 `await`（跟原来 `if(confirm(msg))` 同步调用不一样，全部改成
   `if(await confirmDialog(msg))`）；点"是"/`Enter`/取消按钮外没有对应处理，点遮罩/`Esc`/"否"
   都算取消，跟原生 confirm() 的"确定/取消"行为对齐。 */
const confirmDialog=(msg)=>new Promise(resolve=>{
  const yesBtn=el('button',{class:'btn pri',text:T('common.yes')});
  const noBtn=el('button',{class:'btn',text:T('common.no')});
  const overlay=el('div',{class:'confirm-overlay'},[el('div',{class:'confirm-box'},[el('div',{class:'confirm-msg',text:msg}),el('div',{class:'confirm-actions'},[noBtn,yesBtn])])]);
  const close=v=>{document.removeEventListener('keydown',onKey);overlay.remove();resolve(v)};
  const onKey=e=>{if(e.key==='Escape')close(false);else if(e.key==='Enter')close(true)};
  yesBtn.onclick=()=>close(true);
  noBtn.onclick=()=>close(false);
  overlay.onclick=e=>{if(e.target===overlay)close(false)};
  document.addEventListener('keydown',onKey);
  document.body.appendChild(overlay);
  yesBtn.focus();
});
/* 轻量记忆：per-viewer 便利态，隐私窗口/禁用 storage 时静默回默认 */
const LS={get(k,d){try{const v=localStorage.getItem('shelf.'+k);return v==null?d:v}catch{return d}},set(k,v){try{localStorage.setItem('shelf.'+k,v)}catch{}}};
const onUsb=/^10\.11\.99\./.test(location.hostname);
/* i18n 架子（2026-09-09 审计新增；2026-09-10 补全正文全覆盖）。登录页/改密码页（`src/ui.rs` 的
   `login_page`/`password_page`）不在这次范围内——那两页是 Rust 端独立 `format!` 拼字符串，未登录态
   不跑这份 JS、读不到 `LS`（localStorage）里的语言选择，要做需要另一套"未登录态也能传语言"的机制
   （比如写 cookie 给 Rust 端读），跟这里的做法不是一回事，留给以后真有需要再做。`I18N` 在启动 IIFE
   里异步填充，填充完成之前 `T()` 兜底显示 key 本身（不留空白，也不会悄悄掩盖翻译缺口）。
   ⚠️ `T()` 只能在"渲染/交互时才执行"的函数体里调用——`I18N` 是异步填充的，如果把 `T()` 调用塞进
   模块顶层 `const 模板字符串="..."` 这种脚本解析时就立即求值一次、以后不会重新求值的地方，结果会被
   烤死成 key 兜底文本，永远显示不出真翻译，还不报错（`GUIDE` 改成零参数函数
   就是为了避开这个坑，见各自定义处）。 */
let I18N={};
/* vars：可选的 {占位符名: 值} 插值表，key 对应的文案里用 {占位符名} 占位，逐个字符串替换（次数少，
   用 split/join 够用，不必上正则）。零参数调用（`T('xxx')`）行为跟以前完全一样。 */
const T=(key,vars)=>{let s=I18N[key]||key;if(vars)for(const k in vars)s=s.split('{'+k+'}').join(vars[k]);return s};
const currentLang=()=>LS.get('lang',(navigator.language||'').toLowerCase().startsWith('en')?'en-US':'zh-CN');
/* 格式白名单：服务端 rmsvc_core::formats 注入（同一份，网页 accept + 选中即拦 = 服务端上传门） */
const EXT=__EXTS__, dot=l=>l.map(e=>'.'+e);
const BOOK_EXT=dot(EXT.book), FONT_EXT=dot(EXT.font), DICT_EXT=dot(EXT.dict), IMG_EXT=dot(EXT.image);
const up=l=>l.map(e=>e.toUpperCase()).join(' / ');
// 2026-09-18 起母版库只收 EPUB/PDF（BOOK_EXT===EXT.native，见 rmsvc_core::formats 头注）——原来
// 这里有个 FMT_TIERS() 分两档（原生/仅 KOReader）拼文案，两档收成一档后不再需要，删掉。
$('#logout').onclick=e=>{e.preventDefault();fetch('/logout',{method:'POST'}).then(()=>location.href='/login')};
// 徽章的完整解释（渲染自检失败原因、优化档位差异…）以前只写进 title——触屏摸不到 hover，只看得见
// 图标+数字，看不见"为什么/该怎么办"（2026-09-09 审计发现）。这里全局委托一个点击处理：任何带
// title 的徽章点一下就 alert 出完整内容，不用逐个改模板字符串；desktop 上点了也只是多一次确认，
// 不冲突。`.badge[title]` 的 `cursor` 在 style.css 里配套改成 help，给一个"这能点"的视觉提示。
document.addEventListener('click',e=>{const b=e.target.closest('.badge[title]');if(b&&b.title)toast(b.title,'info',6500)});
// 响应不是合法 JSON（网关自身 502/504、反代错误页…）时，以前直接把裸状态码当 message 弹给用户
// （"HTTP 502"），技术术语没翻译成人话（2026-09-09 审计发现）。改成一句人话+状态码放在括号里，
// 报障时还能带出这个号。
async function j(url,opt){const r=await fetch(url,opt);if(r.status===401){location.href='/login?next='+encodeURIComponent(location.pathname);return {ok:false,message:T('common.needLogin')}}if(r.status===403){location.href='/password';return {ok:false,message:T('common.needChangePassword')}}
  const httpErr=T('common.httpErr',{status:r.status});
  let d;try{d=await r.json()}catch{d={ok:false,message:httpErr}}if(!r.ok&&d.ok!==false)d={ok:false,message:d.message||httpErr};return d}
const postJ=async(url,body)=>{const r=await j(url,{method:'POST',body:JSON.stringify(body)});if(r.ok===false)toast(r.message||T('common.failed'));return r};

/* 上传区 HTML（拖放框 + 隐藏 input + 队列 + 按钮），一处生成、各页复用；uploader() 认这个 .up 容器 */
const upHtml=(icon,label,ext,btn)=>`<div class="up"><div class="drop"><span class="big">${icon}</span>${label}</div><input type="file" multiple hidden accept="${ext.join(',')}"><ul class="q"></ul><div class="row"><button class="btn pri go">${btn}</button></div></div>`;

/* 通用上传器：逐文件一请求，进度条，逐项回执；失败项可重传，队列可逐项删/清空，顶部总进度。box=.up 容器 */
function uploader(box,urlOf,queryOf,okExt,onFinish,dedupeApi){
  const list=$('ul.q',box), input=$('input[type=file]',box), drop=$('.drop',box), go=$('.go',box);
  let files=[], sum=null;
  const clr=document.createElement('button');clr.type='button';clr.className='btn';clr.textContent=T('common.clear');clr.onclick=()=>{files=[];render()};go.after(clr);
  const summary=()=>{if(!sum){sum=document.createElement('div');sum.className='small';sum.style.margin='.3em 0';list.parentNode.insertBefore(sum,list)}
    const ok=files.filter(f=>f.st==='ok').length,bad=files.filter(f=>f.st==='bad').length;
    sum.innerHTML=files.length?T('common.uploadSummary',{ok,total:files.length,badPart:bad?T('common.uploadBadPart',{bad}):''}):'';};
  const render=()=>{list.innerHTML='';files.forEach(f=>{const li=document.createElement('li');li.dataset.k=f.k;li.className=f.st||'';
      li.innerHTML=`<div class="name">${f.file.name} <span class="small">${fmtB(f.file.size)}</span> <button class="btn x" type="button" title="${T('common.remove')}" aria-label="${T('common.remove')}">×</button></div><progress value="${f.st==='ok'?100:0}" max="100"></progress><div class="msg">${f.msg||T('common.waitingUpload')}</div>`;
      li.querySelector('.x').onclick=()=>{files=files.filter(x=>x.k!==f.k);render()};list.appendChild(li)});summary()};
  const add=fl=>{for(const f of fl){const rej=okExt&&!okExt.some(e=>f.name.toLowerCase().endsWith(e));
      files.push({file:f,k:Math.random().toString(36).slice(2),rej,st:rej?'bad':'',msg:rej?T('common.rejectedExt',{ext:okExt.join(' / ')}):''})}render()};
  input.onchange=()=>{add(input.files);input.value=''};
  drop.ondragover=e=>{e.preventDefault();drop.classList.add('hi')};drop.ondragleave=()=>drop.classList.remove('hi');
  drop.ondrop=e=>{e.preventDefault();drop.classList.remove('hi');add(e.dataTransfer.files)};
  drop.onclick=()=>input.click();
  go.onclick=async()=>{go.disabled=true;clr.disabled=true;
    // 母版库上传口传 dedupeApi（`/api/books/staging`）：先查一次现有条目，同名同大小＝上一轮已经
    // 成功落地，跳过重传——不然 unique_path 同名不覆盖会把它再落一份 1_x（2026-09-13，跟 shelf
    // push CLI 那次同一个 gap，见书架白皮书 §04；只有母版库这个上传口传这个参数，字体/壁纸/词典
    // 那几个 uploader() 调用点不传，行为不变）。查询失败（网络/未登录）就当没查到，照常全部传。
    let existing=null;
    if(dedupeApi){try{const d=await j(dedupeApi);existing=new Set((d.items||[]).map(it=>it.name+'|'+it.bytes))}catch{existing=null}}
    for(const f of files){if(f.st==='ok'||f.rej)continue;          // 成功项跳过；格式不收项不上传；失败项允许重传
      const li=list.querySelector(`li[data-k="${f.k}"]`);if(!li)continue;const pg=$('progress',li),msg=$('.msg',li);
      if(existing&&existing.has(f.file.name+'|'+f.file.size)){f.st='ok';li.className='ok';pg.value=100;f.msg=T('common.alreadyStaged');msg.textContent=f.msg;summary();continue}
      f.st='';li.className='';pg.value=0;msg.textContent=T('common.uploading');
      await new Promise(res=>{const x=new XMLHttpRequest();const q=queryOf();x.open('POST',urlOf()+(q?'?'+new URLSearchParams(q):''));
        x.upload.onprogress=e=>{if(e.lengthComputable)pg.value=e.loaded/e.total*100};
        x.onload=()=>{if(x.status===401){location.href='/login';return}let d;try{d=JSON.parse(x.responseText)}catch{d={ok:false,message:'HTTP '+x.status}}
          const it=(d.items&&d.items[0])||d;f.st=it.ok?'ok':'bad';f.msg=(it.message||(it.ok?T('common.done'):T('common.failed')))+(d.note&&it.ok?' · '+d.note:'');li.className=f.st;msg.textContent=f.msg;pg.value=100;summary();
          // 同一批里排了两份同名同大小：这份传完了要马上补进快照，下一份循环到时才躲得开——只查一次
          // 快照、循环里不更新的话，两份会一起溜过去（都不在最初那份快照里），2026-09-13 真机踩到。
          if(existing&&it.ok)existing.add(f.file.name+'|'+f.file.size);
          res()};
        x.onerror=()=>{f.st='bad';f.msg=T('common.networkError');li.className='bad';msg.textContent=f.msg;summary();res()};
        const fd=new FormData();fd.append('file',f.file);x.send(fd)})}
    go.disabled=false;clr.disabled=false;if(onFinish)onFinish()};
  return {clear(){files=[];render()}};
}

/* 二级标签：面板都由外层 render/refresh 预先填好，切换只显隐。`:scope >` 限定只找 sec 的**直接
   子元素**（2026-09-10 「其他」tab 把 KOReader 的 render() 原样嵌进自己某个 subpanel 里，KOReader
   自己内部还有一层字体/词典 subnav——不加 `:scope >` 的话外层这次 querySelectorAll('.subpanel')
   会把 KOReader 自己那两个内层 subpanel 也扫进来，外层按钮数对不上内层+外层 panel 总数，点哪个都
   错位。对现有的非嵌套调用点（没有内层 subnav 的场景）结果完全一样，不是破坏性改动）。 */
function subtabs(sec){const nav=sec.querySelector(':scope > .subnav');if(!nav)return;const btns=[...nav.children],panels=[...sec.querySelectorAll(':scope > .subpanel')];
  btns.forEach((b,i)=>b.onclick=()=>{btns.forEach(x=>x.classList.remove('on'));panels.forEach(p=>p.classList.remove('on'));b.classList.add('on');if(panels[i])panels[i].classList.add('on')});}

/* 列表渲染骨架：每项一行「左：名字等 ｜ 右：徽章/大小/按钮」；row(it,left,right,li) 填内容。字体/词典/壁纸共用 */
// emptyMsg 可选：不给就用通用的"（空）"，母版库/浏览页/回收站这几处早就有各自的引导式空状态文案，
// 这里字体/词典/壁纸列表原来共用的"（空）"完全没有引导，跟其它页面不一致（2026-09-09 审计发现）——
// 各调用点按自己的场景传一句"去哪里做什么"。
function fillList(ul,items,row,emptyMsg){ul.innerHTML='';if(!items.length){ul.innerHTML=`<li class="small">${emptyMsg||T('list.empty')}</li>`;return}
  items.forEach(it=>{const li=document.createElement('li');const left=document.createElement('span'),right=document.createElement('span');
    right.className='small';right.style.cssText='display:flex;align-items:center;gap:.4em;flex-wrap:wrap';row(it,left,right,li);li.append(left,right);ul.appendChild(li)})}
/* 删除按钮：confirmDialog → DELETE → 刷新 */
function delBtn(msg,url,refresh){const d=document.createElement('button');d.className='btn';d.textContent=T('action.delete');
  guardClick(d,async()=>{if(await confirmDialog(msg)){const r=await j(url,{method:'DELETE'});if(r.ok===false)toast(r.message);refresh()}});return d}
const cjkBadge=p=>p==null?'':`<span class="badge ${p>=80?'on':(p>=8?'':'off')}" title="${T('common.cjkCoverageTitle')}">${T('common.cjkCoverage',{pct:p})}</span>`;

/* 决策辅助：不替用户分类（闲书/研读机器判不准），讲清母版库三步走 + 两读器各擅长；拿不准先投一个，母版还在 */
const GUIDE=()=>`<details class="cmp"><summary>${T('transfer.guide.summary')}</summary>
<dl class="help">
<dt>${T('transfer.guide.steps.dt')}</dt><dd>${T('transfer.guide.steps.dd')}</dd>
<dt>${T('transfer.guide.format.dt')}</dt><dd>${T('transfer.guide.format.dd',{native:up(EXT.native)})}</dd>
<dt>${T('transfer.guide.native.dt')}</dt><dd>${T('transfer.guide.native.dd')}</dd>
<dt>${T('transfer.guide.koreader.dt')}</dt><dd>${T('transfer.guide.koreader.dd')}</dd>
<dt>${T('transfer.guide.unsure.dt')}</dt><dd>${T('transfer.guide.unsure.dd')}</dd>
</dl></details>`;

/* 母版库列表。按格式门控按钮：EPUB→优化(未优化时)/加入 xochitl/加入 KO；PDF→加入 xochitl/加入 KO；其它→只能加入 KO。
   CBZ 漫画只能加入 KOReader（不加入 xochitl）；超体积门（nativeLimit 字节）的书灰掉「加入 xochitl」。「加入 KOReader」按 koInstalled 门控。
   2026-09-19 用户明确要求去掉两样东西：① 优化分档位（不再有 mode 选择，永远跑完整清洗+优化）；
   ② 投完自动从母版库删除（母版永远保留，用户自己删）——opts 里原来的 `mode()`/`clear()` 两个
   参数已删，跟着一起删的还有 xFolder 那个"书库/批注/自定义"三选一预设（见 renderTransfer）。
   opts: {items,q,fmt,st,xFolder(),kFolder(),koInstalled,nativeLimit,refresh()} */
function stagingList(ul,opts){
  ul.innerHTML='';
  const q=(opts.q||'').toLowerCase();
  const fmtOf=it=>it.format==='cbz'?'other':it.format;   // 筛选里 CBZ 归「其它」
  const items=(opts.items||[]).filter(it=>(!q||it.name.toLowerCase().includes(q))&&(!opts.fmt||fmtOf(it)===opts.fmt)&&(!opts.st||(opts.st==='1')===!!it.optimized));
  if(!items.length){ul.innerHTML='<li class="small">'+(opts.items&&opts.items.length?T('transfer.staging.emptyFiltered'):T('transfer.staging.emptyAll'))+'</li>';return}
  items.forEach(it=>{const li=document.createElement('li');li.style.flexWrap='wrap';
    const fmt=it.format==='epub'?'EPUB':it.format==='pdf'?'PDF':(it.name.includes('.')?it.name.split('.').pop().toUpperCase():T('transfer.staging.fmtOther'));
    const st=it.format!=='epub'?`<span class="badge">${T('transfer.staging.badge.asIs')}</span>`:it.level==='full'?`<span class="badge on">${T('transfer.staging.badge.optimized')}</span>`:it.level==='core'?`<span class="badge" title="${T('transfer.staging.badge.optimizedUncleanTitle')}">${T('transfer.staging.badge.optimizedUnclean')}</span>`:it.level==='old'?`<span class="badge" title="${T('transfer.staging.badge.oldOptimizedTitle')}">${T('transfer.staging.badge.oldOptimized')}</span>`:`<span class="badge">${T('transfer.staging.badge.notOptimized')}</span>`;
    const hint=it.format==='pdf'?' · '+T('transfer.staging.hint.pdf'):it.format==='cbz'?' · '+T('transfer.staging.hint.comic'):it.format==='other'?' · '+T('transfer.staging.hint.other'):'';
    // 落库记录徽章；落库时间早于母版 mtime（之后又优化过）→ 标「旧」，提示可重投
    const dv=it.delivered||{},stale=t=>t&&it.mtime&&t<it.mtime;
    const dl=(dv.native?` <span class="badge on" title="${stale(dv.native)?T('transfer.staging.delivered.native.staleTitle'):T('transfer.staging.delivered.native.title')}">${T('transfer.staging.delivered.native.badge')}${stale(dv.native)?T('transfer.staging.staleSuffix'):''}</span>`:'')+(dv.koreader?` <span class="badge on" title="${stale(dv.koreader)?T('transfer.staging.delivered.koreader.staleTitle'):T('transfer.staging.delivered.koreader.title')}">${T('transfer.staging.delivered.koreader.badge')}${stale(dv.koreader)?T('transfer.staging.staleSuffix'):''}</span>`:'');
    // 渲染自检徽章（投原生后 book-serve 等 xochitl 渲染完核对页数；warn＝整章渲染失败的典型症状）
    const rc=dv.render,rb=!rc?'':rc.status==='ok'?` <span class="badge on" title="${T('transfer.staging.render.okTitle',{pages:rc.pages,expected:rc.expected})}">${T('transfer.staging.render.okBadge',{pages:rc.pages})}</span>`:rc.status==='warn'?` <span class="badge off" title="${T('transfer.staging.render.warnTitle',{pages:rc.pages,expected:rc.expected})}">${T('transfer.staging.render.warnBadge',{pages:rc.pages,expected:rc.expected})}</span>`:rc.status==='pending'?` <span class="badge" title="${T('transfer.staging.render.pendingTitle')}">${T('transfer.staging.render.pendingBadge')}</span>`:` <span class="badge" title="${T('transfer.staging.render.noneTitle')}">${T('transfer.staging.render.noneBadge')}</span>`;
    // 异步优化/落库状态：进行中（忙锁，服务端权威——不是这次点击本地临时禁用那种，跨刷新/跨设备都准，
    // 优化跟落库共用同一把忙锁所以徽章文案不分是哪个在跑）；完成后短暂展示一次失败（不常驻，避免列表
    // 被历史结果占满；失败保留到下次点同一操作前，方便看清原因；成功不额外提示——优化有「已优化」徽章、
    // 落库有「已投原生」徽章，各自已经是"成功了"的证明，2026-09-19 把落库也接进同一套异步管线时补）。
    const oc=dv.optimize,dc=dv.deliver;
    // "卡在 pending 但 busy 已经变 false"＝上次处理被服务/设备重启打断，状态没能写成
    // ok/failed（2026-09-19 真机撞过：进程被摔炸后忙锁在内存里、重启即清零，但 sidecar
    // 里的 pending 记录留在磁盘上）——不是"还在跑"，之前这种状态界面上什么都不显示，跟"点了
    // 没反应"没区别；现在单独标出来，别再跟"真的在跑"混在一起看。
    const stalePending=k=>k&&k.status==='pending'&&!it.busy;
    // 「加入 KOReader」是 koreader-serve 一次同步阻塞调用（本地文件复制，不走 book-serve 忙锁——
    // 两个服务各自独立进程，book-serve 的 `it.busy` 天然管不到它），server 端没有等价的忙态可查。
    // 2026-09-19 用户反馈"加入 KOReader 进度条也不刷新"：根因是这个操作从没接进过忙态系统，之前
    // 按钮点击只有本地"文字+省略号"，跟其余按钮的进度条待遇完全脱节。这里补一个纯前端的本地忙态
    // （`opts.localBusy`，按钮点击时加入、操作完成后移除，见下面按钮定义），跟服务端 `it.busy`
    // 合并成同一个 `busy` 判据，统一走下面的禁用/进度条逻辑——不假装有真实进度（本地复制没有
    // 天然的分步骤可报），但至少不再是"点了没反应"。
    const localBusy=!!(opts.localBusy&&opts.localBusy.has(it.name));
    const busy=it.busy||localBusy;
    const ob=busy?` <span class="badge" title="${T('transfer.staging.processing.title')}">${T('transfer.staging.processing.badge')}</span>`
      :(stalePending(oc)||stalePending(dc)?` <span class="badge off" title="${T('transfer.staging.stalePending.title')}">${T('transfer.staging.stalePending.badge')}</span>`:'')
      +(oc&&oc.status==='failed'?` <span class="badge off" title="${oc.message}">${T('transfer.staging.optimizeFailed.badge')}</span>`:'')
      +(dc&&dc.status==='failed'?` <span class="badge off" title="${dc.message}">${T('transfer.staging.deliverFailed.badge')}</span>`:'');
    li.innerHTML=`<span><b>${it.name}</b> <span class="badge">${fmt}</span> ${st}${dl}${rb}${ob} <span class="small">${fmtB(it.bytes)}${hint}</span></span>`;
    // 报错/进度原因以前只写进徽章的 title（hover 才看得到，触屏设备摸不到）——补一行可见小字，跟
    // 下面"禁用态按钮原因"同一套做法（2026-09-19 用户反馈"报错最下方显示错误原因"）。
    // 优先级：投递失败 ＞ 优化失败 ＞ 处理被打断（真在跑走下面单独的进度条分支，不进这条文字级联）。
    const statusMsg=dc&&dc.status==='failed'?T('transfer.staging.deliverFailedPrefix')+dc.message
      :oc&&oc.status==='failed'?T('transfer.staging.optimizeFailedPrefix')+oc.message
      :(stalePending(oc)||stalePending(dc))?T('transfer.staging.stalePending.title'):'';
    if(statusMsg){const line=document.createElement('div');line.className='small';line.style.cssText='flex-basis:100%;margin-top:.2em;color:var(--bad)';line.textContent=statusMsg;li.appendChild(line)}
    // 真在跑：进度条取代"正在处理中，请稍候"这句静态文案（2026-09-19 用户反馈——干等的文字没意义，
    // 能看见走到哪一步/大概多久才有用）。漫画拆分卷落库、普通 EPUB 优化都有结构化步数
    // （`dc.progress`/`oc.progress`，形状同为 `{done,total}`，见 book-serve `try_deliver_split`/
    // `spawn_optimize`）→ 真百分比进度条；普通整本落库（没有拆分）后端没法给出精确进度（耗时来自
    // 单次上传，没有天然的"第几步"）→ 不确定时长的滚动进度条，至少比一句不会变化的静态文字更能
    // 传达"真的在动、不是卡死了"（`renderStepProgress` 定义见文件顶部，跟笔记「推送本章」共用）。
    if(busy){
      const dcPending=dc&&dc.status==='pending',ocPending=oc&&oc.status==='pending';
      const label=localBusy?T('transfer.staging.progress.addingKoreader'):dcPending?T('transfer.staging.progress.delivering'):ocPending?T('transfer.staging.progress.optimizing'):T('transfer.staging.progress.working');
      const prog=(dcPending&&dc.progress)||(ocPending&&oc.progress)||null;
      const msg=dcPending?dc.message:ocPending?oc.message:'';
      renderStepProgress(li,{label,prog,msg});
    }
    const right=document.createElement('span');right.style.cssText='display:flex;gap:.4em;flex-wrap:wrap;align-items:center';
    // 禁用态按钮的原因（超限/未安装）以前只写进 title——触屏设备摸不到 hover，等于完全看不到为什么点
    // 不了、该去哪解决（2026-09-09 审计发现）。现在禁用时额外补一行可见小字，跟 title 内容一样，
    // `flex-basis:100%` 让它在 `right`（flex-wrap 容器）里独占一行，不挤在按钮同一行。busy 引起的
    // 禁用不再重复这行小字——上面已经有整行的进度条，三个按钮各自重复一遍"正在处理中"没有信息量。
    // trackLocal=true：这个按钮的操作没有服务端忙态可查（目前只有「加入 KOReader」），点击时先把
    // 这条目名记进 `opts.localBusy` 并立即用 `opts.render`（不重新拉数据、只重画）把进度条画出来，
    // 操作结束（无论成不成功）都要清掉，再走一次真正的 `opts.refresh` 落地最终状态。禁用/复位本身
    // 委托给全局 `guardClick`（2026-09-19 代码质量审计——这里以前自己又手写了一遍同一件事）；
    // 这层只叠列表特有的文案/记账/收尾。
    const btn=(t,pri,fn,dis,title,cls,trackLocal)=>{const b=document.createElement('button');b.className='btn'+(pri?' pri':'')+(cls?' '+cls:'');b.textContent=t;if(dis){b.disabled=true;b.title=title||''}else guardClick(b,async()=>{b.textContent=t+'…';
      if(trackLocal&&opts.localBusy){opts.localBusy.add(it.name);if(opts.render)opts.render()}
      try{await fn()}finally{if(trackLocal&&opts.localBusy)opts.localBusy.delete(it.name)}
      if(opts.refresh)opts.refresh()});right.appendChild(b);
      if(dis&&title){const hint=document.createElement('span');hint.className='small';hint.style.cssText='flex-basis:100%';hint.textContent=title;right.appendChild(hint)}};
    // busy（it.busy 服务端忙锁 或 localBusy 本地忙态）：这条目正有一个异步操作在跑（优化/落库真机
    // 实测都能到分钟级，KOReader 加入虽快但也不该看着像没反应）——删除/落库/加入 KOReader/再次优化
    // 全部先禁掉，防止并发冲突（2026-09-18 真机反馈：点了优化又点删除）；服务端忙锁完成后 SSE 推
    // 事件、列表自动刷新解禁，本地忙态在 `fn()` resolve 后立即清。
    if(it.format==='epub'&&!it.optimized)btn(T('transfer.staging.btn.optimize'),false,()=>postJ('/api/books/staging/optimize',{name:it.name}),busy,'');
    // 体积门：超过 xochitl /upload 上限的书灰掉按钮（服务端同样拦），提示走电脑分卷。EPUB 例外——
    // 超限的 EPUB 漫画服务端会按卷拆分投递（2026-09-18，见 book-serve::Staging::try_deliver_split），
    // 按钮不能提前灰掉，得让服务端判过是不是漫画才知道能不能救；PDF 没有这条救援路径，继续照原样灰。
    const tooBig=opts.nativeLimit&&it.bytes>opts.nativeLimit&&it.format!=='epub';
    // 落库改异步同 optimize（2026-09-19：超限漫画按卷拆分要挨个建包+上传，真机能到分钟级，之前同步
    // 阻塞的体验跟优化改异步前一样像卡死）；点了立即回"已开始"，不再 alert 最终结果——完成状态跟优化
    // 一样靠徽章看（成功＝「已加入 xochitl」时间戳徽章出现，失败＝「上次加入失败」徽章，见上面 ob 那段）。
    // 2026-09-19 去掉「投完自动删除」：母版永远保留，不再传 keep（服务端也已删这个参数）。
    if(it.format==='epub'||it.format==='pdf')btn(T('transfer.staging.btn.deliverNative'),true,()=>postJ('/api/books/staging/deliver',{name:it.name,folder:opts.xFolder()}),busy||tooBig,busy?'':T('transfer.staging.btn.tooBigTitle',{limit:fmtB(opts.nativeLimit)}));
    btn(T('transfer.staging.btn.addKoreader'),true,async()=>{const r=await postJ('/api/koreader/books/adopt',{name:it.name,folder:opts.kFolder()});if(r.ok!==false)await postJ('/api/books/staging/mark',{name:it.name,target:'koreader'})},busy||!opts.koInstalled,busy?'':T('transfer.staging.btn.koNotInstalled'),undefined,true);
    // 单行最多 5 徽章+4 按钮时，"删除"（销毁）跟"优化"（编辑）视觉权重完全一样，只靠文案区分
    // （2026-09-09 审计发现）；`.btn-bad` 早就存在但只用在转写失败按钮上，这里补上，破坏性操作至少
    // 颜色上跟常规操作分开。
    btn(T('action.delete'),false,async()=>{if(await confirmDialog(T('transfer.staging.confirmDeleteOne',{name:it.name})))await postJ('/api/books/staging/delete',{name:it.name})},busy,'','btn-bad');
    li.appendChild(right);ul.appendChild(li)});
}

/* 「传书」固定 tab = 三层架构入口：入库（所有内容源汇入）｜母版库（可选优化 → 选去向落库）。放第一位。
   读器页（xochitl / KOReader）不再有任何传书入口，只管各自的字体 / 词典。 */
function renderTransfer(sec){sec.innerHTML=`
  <div class="subnav"><button class="on">${T('transfer.subnav.intake')}</button><button>${T('transfer.subnav.library')}</button></div>
  <div class="subpanel on">
    <div class="card"><h2>${T('transfer.intake.title')}</h2>
      <p class="lead">${T('transfer.intake.lead')}</p>
      ${GUIDE()}
      ${onUsb?'':`<p class="opt-note">${T('transfer.intake.usbHint')}</p>`}
    </div>
    <!-- 入库来源各自独立成卡（2026-09-10 用户要求：原来挤在同一张卡里用 h3 分隔，看着像
         "上传"下面附带子步骤，实际是几条互不依赖、各走各的入库路径，拆卡片才是"独立功能来源"
         该有的视觉分量，跟「系统增强」/「实验室」那种并排卡片同一个语言。2026-09-18：原第三张
         「电脑 shelf push」卡片随 host 整条线退役一并删除——用户明确表态以后不再使用 PC 端，
         入库只剩「上传」+「抓网文」两条路，都走网页本身，不用任何 host CLI）。 -->
    <div class="card"><h3 style="margin-top:0">${T('transfer.upload.title')}</h3>
      ${upHtml('⬆',T('transfer.upload.dropLabel',{native:up(EXT.native)}),BOOK_EXT,T('transfer.upload.btn'))}
      <p class="small">${T('transfer.upload.hint')}</p>
    </div>
    <div class="card"><h3 style="margin-top:0">${T('transfer.fetchArticle.title')}</h3>
      <div class="row"><input type="text" id="arturl" placeholder="${T('transfer.fetchArticle.urlPlaceholder')}" style="flex:1;min-width:12em"><button class="btn" id="artgo">${T('transfer.fetchArticle.btn')}</button></div>
      <label class="toggle"><input type="checkbox" id="artopt"> ${T('transfer.fetchArticle.optimizeToggle')}</label>
      <div class="small" id="artmsg" style="margin-top:.3em"></div>
      <p class="small">${T('transfer.fetchArticle.hint')}</p>
      <p class="small">${T('transfer.fetchArticle.optimizeHint')}</p>
    </div>
  </div>
  <div class="subpanel">
    <div class="card"><h3 style="margin-top:0">${T('transfer.staging.title')} <span class="small" id="stgcap"></span></h3>
      <div class="row"><label class="small" for="folder">${T('transfer.staging.folder.label')}</label><input type="text" id="folder" list="xodirs" placeholder="${T('transfer.staging.folder.placeholder')}" style="max-width:10em"><datalist id="xodirs"></datalist>
        <label class="small" for="kfolder">${T('transfer.staging.kfolder.label')}</label><input type="text" id="kfolder" list="kodirs" placeholder="${T('transfer.staging.kfolder.placeholder')}" style="max-width:9em"><datalist id="kodirs"></datalist></div>
      <details class="cmp"><summary>${T('transfer.staging.optDetailsSummary')}</summary><p class="small">${T('transfer.staging.optNote')}</p></details>
      <div class="row"><input type="text" id="stgq" placeholder="${T('transfer.staging.searchPlaceholder')}" aria-label="${T('transfer.staging.searchAria')}" style="flex:1;min-width:8em"><select id="stgfmt" aria-label="${T('transfer.staging.fmtFilterAria')}" style="max-width:8em"><option value="">${T('transfer.staging.fmtAll')}</option><option value="epub">EPUB</option><option value="pdf">PDF</option><option value="other">${T('transfer.staging.fmtOther')}</option></select><select id="stgst" aria-label="${T('transfer.staging.stFilterAria')}" style="max-width:8em"><option value="">${T('transfer.staging.stAll')}</option><option value="0">${T('transfer.staging.stRaw')}</option><option value="1">${T('transfer.staging.stDone')}</option></select><button class="btn" id="stgpurge" title="${T('transfer.staging.purgeTitle')}">${T('transfer.staging.purgeBtn')}</button></div>
      <div class="small" id="stgfree" style="margin:-.3em 0 .4em"></div>
      <ul class="list" id="stglist"></ul>
    </div>
  </div>`;
  let koInstalled=false,items=[],nativeLimit=0;
  // 「加入 KOReader」没有服务端忙态可查（见 stagingList 内注释）——本地忙态集合，per-render 存活，
  // 不落 LS（纯瞬时 UI 态，刷新页面/切 tab 就该清空，不是需要记住的用户设置）。
  const localBusy=new Set();
  const g=id=>$('#'+id,sec);
  // 落库设置记在本机（per-viewer 便利态）。2026-09-19：原来的「书库/批注/自定义」三选一预设
  // （`folderPreset`）删掉，改跟 KOReader 目录同一个模式——自由输入框 + datalist 真实候选
  // （见下面 refresh() 里的 xodirs 填充），留空＝落配置缺省的书库文件夹。
  [['folder','folder',''],['kfolder','kfolder','']].forEach(([id,k,d])=>{g(id).value=LS.get(k,d);['input','change'].forEach(ev=>g(id).addEventListener(ev,()=>LS.set(k,g(id).value)))});
  const render=()=>stagingList(g('stglist'),{items,q:g('stgq').value,fmt:g('stgfmt').value,st:g('stgst').value,xFolder:()=>g('folder').value.trim(),kFolder:()=>g('kfolder').value.trim(),koInstalled,nativeLimit,localBusy,render:()=>render(),refresh:()=>refresh()});
  ['stgq','stgfmt','stgst'].forEach(id=>['input','change'].forEach(ev=>g(id).addEventListener(ev,render)));
  const refresh=async()=>{const [d,s,k,kb]=await Promise.all([j('/api/books/staging'),j('/api/books/status'),j('/api/koreader/status'),j('/api/koreader/books')]);
    nativeLimit=(s.ok&&s.nativeUploadLimitBytes)||0;koInstalled=!!(k.ok&&k.installed);
    // xochitl/KOReader 现有目录 → 下拉候选（免手打错，跟真实文件夹保持一致，不是写死的预设）
    g('xodirs').innerHTML=(s.ok?s.xochitlFolders||[]:[]).map(n=>`<option value="${n}">`).join('');
    g('kodirs').innerHTML=(kb.items||[]).filter(x=>x.kind==='dir').map(x=>`<option value="${x.name}">`).join('');
    if(d.ok===false){g('stglist').innerHTML=`<li class="small" style="color:var(--bad)">${T('transfer.staging.unavailable',{msg:d.message||T('transfer.staging.notOpen')})}</li>`;g('stgcap').textContent='';return}
    items=d.items||[];const tot=items.reduce((a,b)=>a+b.bytes,0);g('stgcap').textContent=items.length?T('transfer.staging.capSummary',{count:items.length,size:fmtB(tot)}):'';
    const fr=d.freeBytes;const low=fr!=null&&fr<300*1048576;g('stgfree').style.color=low?'var(--bad)':'';g('stgfree').textContent=fr!=null?T('transfer.staging.freeSpace',{free:fmtB(fr),lowWarn:low?T('transfer.staging.lowWarn'):''}):'';
    render()};
  guardClick(g('stgpurge'),async()=>{const done=items.filter(it=>it.delivered&&(it.delivered.native||it.delivered.koreader));if(!done.length){toast(T('transfer.staging.noneToPurge'),'warn');return}
    if(!await confirmDialog(T('transfer.staging.confirmPurge',{count:done.length})))return;
    for(const it of done)await postJ('/api/books/staging/delete',{name:it.name});refresh()});
  uploader($('.up',sec),()=>'/api/books/staging',()=>({}),BOOK_EXT,()=>refresh(),'/api/books/staging');   // 书籍格式原样入库；选中即按 BOOK_EXT 拦；传 dedupeApi 防重传出重复
  const am=g('artmsg'),au=g('arturl'),ag=g('artgo'),ao=g('artopt');
  // 「同步优化」记在本机（per-viewer 便利态，跟 folder/kfolder 那几个一个规矩）；缺省开——网文正文
  // 没有任何 CSS（article.rs 属性白名单本来就不留 class/style），不经优化会在设备上按默认段距渲染出大片
  // 留空（真机反馈），默认帮用户把这一步做了，不想要（比如想快点抓完自己再调）可以关掉。
  ao.checked=LS.get('artopt','1')==='1';ao.onchange=()=>LS.set('artopt',ao.checked?'1':'0');
  ag.onclick=async()=>{const url=au.value.trim();if(!url){am.textContent=T('transfer.fetchArticle.needUrl');return}ag.disabled=true;am.style.color='';am.textContent=T('transfer.fetchArticle.fetching');
    const r=await j('/api/books/staging/fetch-article',{method:'POST',body:JSON.stringify({url,optimize:ao.checked})});ag.disabled=false;
    am.style.color=r.ok===false?'var(--bad)':'var(--ok)';am.textContent=r.ok===false?('✗ '+(r.message||T('transfer.fetchArticle.failed'))):('✓ '+r.message);if(r.ok!==false){au.value='';refresh()}};
  refresh();sec.refresh=refresh;subtabs(sec);}

/* 服务 tab（按注册表出现）。key = 注册的服务名。service→seg（AREA）不再在这里手搓一份——
   那正是 gateway/src/manage.rs::MODULES 表已声明的唯一事实源，这里改成初始化时从
   GET /api/manage 现读，见下面 init() 里的 AREA 变量：手搓的映射会跟 MODULES 改名/新增
   悄悄脱节，SSE 事件的 area 就对不上、对应 tab 的事件驱动刷新会静默失效。 */
const TABS={
 'note-serve':{titleKey:'tab.notes',title:'笔记',render:renderNotes},
 'font-serve':{title:'xochitl',render(sec){assetTab(sec,'/api/fonts',{
   title:T('assets.fonts.title'),
   hint:T('assets.fonts.hint'),
   header:`<div id="fbchain" class="opt-note" style="display:none"></div>
      <div class="row"><label class="toggle"><input type="checkbox" id="embold" checked> ${T('assets.fonts.emboldenLabel')}</label> <span class="small">${T('assets.fonts.emboldenHint')}</span></div>`,
   icon:'🔤',label:T('assets.fonts.dropLabel'),accept:FONT_EXT,btn:T('assets.fonts.btn'),listTitle:T('assets.fonts.listTitle'),
   onRender:async(sec,refresh,fl)=>{
     // 中文缺字回退链：覆盖率≥8% 的中文字体，按覆盖率降序
     const cjk=(fl.items||[]).filter(it=>((it.extra||{}).cjkPct||0)>=8).sort((a,b)=>(b.extra.cjkPct||0)-(a.extra.cjkPct||0));
     const fb=$('#fbchain',sec);fb.style.display='';fb.innerHTML=cjk.length?T('assets.fonts.fallbackChain',{chain:cjk.map(it=>`${it.name} <span class="small">${it.extra.cjkPct}%</span>`).join(' → ')}):T('assets.fonts.noCjkWarn');
     const fst=await j('/api/fonts/status');const eb=$('#embold',sec);if(fst.ok){eb.checked=!!fst.emboldenCjkFallback;eb.onchange=async()=>{const r=await j('/api/fonts/config',{method:'PUT',body:JSON.stringify({emboldenCjkFallback:eb.checked})});if(r.ok===false){toast(r.message);eb.checked=!eb.checked}}}},
   row:(it,left,right,refresh)=>{const ex=it.extra||{};
     left.innerHTML=`${it.name}${ex.names&&ex.names.cn&&ex.names.cn!==it.name?' <span class="small">'+ex.names.cn+'</span>':''}${ex.files&&ex.files.length>1?' <span class="small">×'+ex.files.length+'</span>':''}`;
     right.insertAdjacentHTML('beforeend',cjkBadge(ex.cjkPct)+(ex.fontconfigRef?`<span title="${T('assets.fonts.fallbackRefTitle')}">⚠</span>`:''));
     right.appendChild(delBtn(T('assets.fonts.deleteConfirm',{name:it.name,filesNote:ex.files&&ex.files.length>1?T('assets.fonts.filesNote',{count:ex.files.length}):'',suffix:ex.fontconfigRef?T('assets.fonts.deleteSuffixFallback'):T('assets.fonts.deleteSuffixNormal')}),'/api/fonts/'+encodeURIComponent(it.name),refresh))}})}},
 'koreader-serve':{title:'KOReader',render(sec){sec.innerHTML=`
  <div class="subnav"><button class="on">${T('koreader.subnav.fonts')}</button><button>${T('koreader.subnav.dicts')}</button></div>
  <div class="subpanel on">
    <div class="card"><h2>${T('koreader.title')}</h2><div class="kv small" id="ks" style="margin-top:.5em">${T('common.loading')}</div>
      <details class="cmp"><summary>${T('koreader.installGuide.summary')}</summary>
      <dl class="help">
        <dt>${T('koreader.installGuide.install.dt')}</dt><dd>${T('koreader.installGuide.install.dd')}</dd>
        <dt>${T('koreader.installGuide.tuned.dt')}</dt><dd>${T('koreader.installGuide.tuned.dd')}</dd>
        <dt>${T('koreader.installGuide.books.dt')}</dt><dd>${T('koreader.installGuide.books.dd')}</dd>
        <dt>${T('koreader.installGuide.restart.dt')}</dt><dd>${T('koreader.installGuide.restart.dd')}</dd>
      </dl></details></div>
    <div class="card"><h3 style="margin-top:0">${T('koreader.fonts.title')}</h3><p class="small">${T('koreader.fonts.hint')}</p>
    ${upHtml('🔤',T('assets.fonts.dropLabel'),FONT_EXT,T('assets.fonts.btn'))}
    <h3>${T('assets.fonts.listTitle')}</h3><ul class="list" id="kf"></ul></div>
  </div>
  <div class="subpanel">
    <div class="card"><h3 style="margin-top:0">${T('koreader.dicts.title')}</h3><p class="small">${T('koreader.dicts.hint',{ext:DICT_EXT.join(' / ')})}</p>
    <label class="field" for="dictname">${T('koreader.dicts.nameLabel')}</label><input type="text" id="dictname" placeholder="${T('koreader.dicts.namePlaceholder')}">
    ${upHtml('📖',T('koreader.dicts.dropLabel'),DICT_EXT,T('koreader.dicts.btn'))}
    <h3>${T('koreader.dicts.installedTitle')}</h3><ul class="list" id="kd"></ul></div>
  </div>`;
  const ups=sec.querySelectorAll('.up');
  uploader(ups[0],()=>'/api/koreader/fonts',()=>({}),FONT_EXT,()=>refresh());
  uploader(ups[1],()=>'/api/koreader/dicts',()=>({name:$('#dictname',sec).value.trim()}),DICT_EXT,()=>refresh());
  const refresh=async()=>{
    const [s,f,dc]=await Promise.all([j('/api/koreader/status'),j('/api/koreader/fonts'),j('/api/koreader/dicts')]);
    $('#ks',sec).innerHTML=s.ok?`<b>${T('koreader.status.installed')}</b><span>${s.installed?T('common.yes'):T('common.no')} ${s.version?'('+s.version+')':''}</span><b>${T('koreader.status.running')}</b><span>${s.running?T('koreader.status.runningYes'):T('common.no')}</span><b>${T('koreader.status.installedCount')}</b><span>${T('koreader.status.countLabel',{fonts:s.fonts,dicts:s.dicts||0})}</span>`:`<span>${s.message}</span>`;
    fillList($('#kf',sec),f.items||[],(it,left,right)=>{left.textContent=it.name;right.insertAdjacentHTML('beforeend',cjkBadge(it.cjkPct)+`<span>${fmtB(it.bytes)}</span>`);right.appendChild(delBtn(T('koreader.fonts.deleteConfirm',{name:it.name}),'/api/koreader/fonts/'+encodeURIComponent(it.name),refresh))},T('koreader.fonts.emptyHint'));
    fillList($('#kd',sec),dc.items||[],(it,left,right)=>{left.textContent='📖 '+it.name;right.textContent=T('koreader.dicts.countSuffix',{count:it.ifo})},T('koreader.dicts.emptyHint'))};
  refresh();sec.refresh=refresh;subtabs(sec)}},
 'wallpaper-serve':{titleKey:'tab.wallpaper',title:'壁纸',render(sec){assetTab(sec,'/api/wallpapers',{
   hint:T('wallpaper.hint'),
   header:`<label class="field">${T('wallpaper.rotateLabel')}</label><div class="row"><select id="wpmode" style="max-width:12em"><option value="sequential">${T('wallpaper.mode.sequential')}</option><option value="random">${T('wallpaper.mode.random')}</option><option value="fixed">${T('wallpaper.mode.fixed')}</option></select><span id="wpst" class="small"></span></div>`,
   icon:'🖼',label:T('wallpaper.dropLabel'),accept:IMG_EXT,btn:T('wallpaper.btn'),
   onRender:async(sec,refresh)=>{const st=await j('/api/wallpapers/status');const sel=$('#wpmode',sec);if(st.ok){sel.value=st.mode;const nv=st.native||{};$('#wpst',sec).textContent=T('wallpaper.status',{current:st.current||T('wallpaper.none'),nativeState:nv.enabled?T('wallpaper.nativeEnabled'):T('wallpaper.nativeDisabled'),restartNote:nv.restartPending?T('wallpaper.restartNote'):''})}
     sel.onchange=async()=>{const r=await j('/api/wallpapers/mode',{method:'PUT',body:JSON.stringify({mode:sel.value})});if(r.ok===false){toast(r.message||T('wallpaper.switchFailed'));return}refresh()}},
   row:(it,left,right,refresh)=>{const cur=(it.extra||{}).current;
     // alt="" 原来把这张图当装饰性处理，但壁纸缩略图本身就是内容（"这张壁纸长什么样"），屏幕阅读器
     // 会整个跳过（2026-09-09 审计发现）；文件名本身当描述最直接，跟右边视觉上显示的文字一致。
     left.innerHTML=`<img src="/api/wallpapers/${encodeURIComponent(it.name)}" alt="${T('wallpaper.thumbAlt',{name:it.name})}" style="height:3.4em;border-radius:.3em;border:1px solid var(--line);margin-right:.6em;vertical-align:middle">${it.name}`;
     right.insertAdjacentHTML('beforeend',`<span>${fmtB(it.bytes)}</span>`+(cur?`<span class="badge on">${T('wallpaper.current')}</span>`:''));
     if(!cur){const b=document.createElement('button');b.className='btn';b.textContent=T('wallpaper.use');guardClick(b,async()=>{const r=await j('/api/wallpapers/current',{method:'PUT',body:JSON.stringify({name:it.name})});if(r.ok===false){toast(r.message||T('wallpaper.setFailed'));return}refresh()});right.appendChild(b);
       right.appendChild(delBtn(T('wallpaper.deleteConfirm',{name:it.name}),'/api/wallpapers/'+encodeURIComponent(it.name),refresh))}}})}}
};

/* 资产页模板（字体 / 壁纸）：说明 + 可选头部 + 上传区 + 列表。o: {title?,hint,header?,icon,label,accept,btn,listTitle?,onRender?(sec,refresh,data),row(it,left,right,refresh)} */
function assetTab(sec,api,o){sec.innerHTML=`<div class="card">${o.title?`<h2>${o.title}</h2>`:''}<p class="${o.title?'small':'lead'}">${o.hint}</p>${o.header||''}
  ${upHtml(o.icon,o.label,o.accept,o.btn)}
  <h3>${o.listTitle||T('assets.installedDefault')}</h3><ul class="list" id="al"></ul></div>`;
  const refresh=async()=>{const d=await j(api);fillList($('#al',sec),d.items||[],(it,left,right)=>o.row(it,left,right,refresh),T('assets.emptyHint',{btn:o.btn}));if(o.onRender)o.onRender(sec,refresh,d)};
  uploader($('.up',sec),()=>api,()=>({}),o.accept,refresh);
  refresh();sec.refresh=refresh}

/* 「其他」顶层 tab（2026-09-10 用户重排首层标签：传书/笔记/其他/管理）：xochitl(font-serve)/
   KOReader(koreader-serve)/壁纸(wallpaper-serve) 三个原来各自独立的顶层 tab 降一级，包进这个
   tab 当二级子标签——三个服务各自的 render() 原样复用，不重写内容，只是换个挂载点（KOReader 自己
   内部还有一层字体/词典 subnav，三级嵌套，subtabs() 已经改成 `:scope >` 限定直接子元素，不会互相
   干扰，见 subtabs() 头注）。只装了其中一部分时，subnav 只列已装的那几个（笔记 tab 本身不在这
   里——note-serve 单独占「其他」前面那个固定位置，不受这条影响）。 */
function renderOther(sec,svcs){
  const items=[{name:'font-serve',icon:'🔤',label:'xochitl'},{name:'koreader-serve',icon:'📖',label:'KOReader'},{name:'wallpaper-serve',icon:'🖼️',label:T('tab.wallpaper')}]
    .filter(it=>svcs.some(s=>s.name===it.name));
  sec.innerHTML=`<div class="subnav">${items.map((it,i)=>`<button${i===0?' class="on"':''}>${it.icon} ${it.label}</button>`).join('')}</div>
    ${items.map((it,i)=>`<div class="subpanel${i===0?' on':''}" id="other-${it.name}"></div>`).join('')}`;
  items.forEach(it=>TABS[it.name].render($('#other-'+it.name,sec)));
  sec.refresh=()=>items.forEach(it=>{const c=$('#other-'+it.name,sec);if(c&&c.refresh)c.refresh()});
  subtabs(sec);
}

/* 「笔记」tab（note-serve 注册；数据来自 ink-serve 条目库）：按书→按章列条目，左裁图右文本，改即存。
   设备只负责写、不负责改：这里就是"改"的地方（e-ink 上打字太痛苦）。三期（2026-09-08）砍掉了"分区"——
   AI 触发早就是按条目单发（勾「问AI」+ 填问题+点提问，调 mind-serve 拼"书名+章节+勾画原文+转写文本+问题"
   发模型，二期，白皮书 §03n），分区兼职的笔记本排版分组也不要了，条目一律按页序平铺，格式=Entry.style。 */
// 顶层常量只放 i18n key 名，不放翻译好的文字——真正的 T() 查找挪到调用点（渲染时执行），见 T() 头注的硬性规则。
const STYLE_NAMES={body:'notes.style.body',bullet:'notes.style.bullet',numbered:'notes.style.numbered',checkbox:'notes.style.checkbox'};
/* archived 显示"已删除（可恢复）"而不是单纯"已删除"：这是软删终态，回收站里随时能点「恢复」，
   跟「清空回收站」那个真正不可逆的操作不该用同一个不加限定的"删除"措辞（2026-09-09 审计修：原文案
   会让用户误以为回收站里这条已经彻底没了）。 */
const STATUS_NAMES={mined:'notes.status.mined',pending:'notes.status.pending',draft:'notes.status.draft',reviewed:'notes.status.reviewed',skipped:'notes.status.skipped',revoked:'notes.status.revoked',archived:'notes.status.archived'};
/* Obsidian 官方图标是紫色多面体"石头"，不是随便一个链接符号——真机反馈"能否用它自己的图标"，用一个
   简化的多面体 SVG（不是官方 logo 的精确描边，商标图形不该随手照抄，这个形状+配色足够让人一眼认出
   "这是 Obsidian"）替掉原来占位的 🔗。设备笔记本用 📓 emoji 就够直观，不用特别做图标。 */
const OBSIDIAN_ICON='<svg viewBox="0 0 24 24" width="13" height="13" style="vertical-align:-2px;flex:none" aria-hidden="true"><path d="M12 2 19 7.5 17 15 12 22 7 15 5 7.5Z" fill="#8b6cef"/></svg>';
// 混了 emoji/SVG 和文字，值改成惰性函数（调用时才 T()）而不是 key 字符串——跟上面两个字典处理方式
// 不同，是因为这里图标本身要拼进结果里，不是"整串都是 key 对应的文字"。
const DEST_ICON={notebook:()=>'📓 '+T('notes.dest.notebook'),obsidian:()=>OBSIDIAN_ICON+' '+T('notes.dest.obsidian'),both:()=>'📓'+OBSIDIAN_ICON+' '+T('notes.dest.both')};
const DEST_ORDER=['notebook','obsidian','both'];
/* 落设备笔记本/落 Obsidian/两处都要（三期，白皮书 §03n 之后）：缺省 both；第二轮反馈把下拉换成
   条目卡片里的循环图标按钮（`DEST_ICON`/`DEST_ORDER`，见 renderBook）。 */
/* 「浏览」（新批注先落这，点了才转笔记）与「整理」（真被要求转笔记的才在这核对）拆两个子视图，见二期设计（白皮书 §03n）。 */
function renderNotes(sec){sec.innerHTML=`
  <div class="card"><h2>${T('notes.title')}</h2>
    <p class="lead">${T('notes.lead')}</p>
    <div class="row"><span class="small">${T('notes.bookLabel')}</span><select id="nbook" style="flex:1;min-width:10em"></select><button class="btn" id="nrescan" title="${T('notes.rescanTitle')}">${T('notes.rescanBtn')}</button></div>
    <div class="row small" id="nsum"></div>
  </div>
  <div class="subnav" id="nsubnav"><button class="on">${T('notes.subnav.browse')}</button><button>${T('notes.subnav.organize')}</button><button>${T('notes.subnav.trash')}</button><button hidden>${T('notes.subnav.import')}</button></div>
  <div class="subpanel on" id="nbrowse"></div>
  <div class="subpanel" id="norganize">
    <div class="subnav" id="nexporttabs"><button class="on" data-etab="pending">${T('notes.export.pending')}</button><button data-etab="synced">${T('notes.export.synced')}</button></div>
    <div class="subnav" id="nchaptertabs"></div>
    <div id="nchapterbody"></div>
  </div>
  <div class="subpanel" id="ntrashpanel">
    <div class="card">
      <p class="lead">${T('notes.trash.lead')}</p>
      <div class="row"><button class="btn" id="npurge" title="${T('notes.trash.purgeTitle')}">${T('notes.trash.purgeBtn')}</button><button class="btn" id="nrestoreall" title="${T('notes.trash.restoreAllTitle')}">${T('notes.trash.restoreAllBtn')}</button><span class="small" id="ntrashsum"></span></div>
      <div id="ntrashlist"></div>
    </div>
  </div>
  <div class="subpanel" id="nimport" hidden>
    <div class="card">
      <p class="lead">${T('notes.import.lead')}</p>
      <div class="row"><span class="small">${T('notes.import.docNameLabel')}</span><input type="text" id="nimporttitle" placeholder="${T('notes.import.docNamePlaceholder')}" style="flex:1;min-width:12em"></div>
      <div class="row"><input type="file" id="nimportfile" accept=".md,.markdown"></div>
      <div class="row small" id="nimportfilename"></div>
      <div class="row"><button class="btn" id="nimportbtn">${T('notes.import.btn')}</button><span class="small" id="nimportstat"></span></div>
    </div>
  </div>`;
  const sel=$('#nbook',sec),chaptertabs=$('#nchaptertabs',sec),chapterbody=$('#nchapterbody',sec),browse=$('#nbrowse',sec),sum=$('#nsum',sec);let book=null;
  // 「推送本章」/「重新转写」/「提问」点完显示结果文案、停留 3s 再让用户看清（见下面三处 wait(3000)）——
  // 但这三个动作本身会让 ink-serve 发 `entries` 事件，笔记 tab 正开着时 SSE 会立刻调 `sec.refresh`
  // 整段重画，比 3s 计时器快得多，文案实际上一闪就被这个"我以为没关系的"刷新冲掉了（真机反馈"重复
  // 推送的提示看不清，一闪而过"，2026-09-17；上一轮把 1.5s 延到 3s 完全没解决，根子根本不在计时器
  // 长短）。这里挡一下：显示文案的同时记一个"暂停到几点"的时间戳，`refresh()` 起手先看这个时间戳，
  // 没过就直接跳过这次 SSE 触发的重画——不会漏刷新，三处调用点末尾自己的 `wait(3000)` 之后本来就会
  // 主动重画一次，只是不再被 SSE 抢跑。
  let holdRefreshUntil=0;
  const cropUrl=(uuid,f)=>`/api/ink/books/${encodeURIComponent(uuid)}/crops/${encodeURIComponent(f)}`;
  // 2026-09-16 截图走查发现：`e.ink` 有值但 `e.ink.crop` 是空串（ink-serve 自渲染裁图失败/写盘失败时
  // 会发生，见 ingest.rs 的 render_ink/write_atomic 错误分支，只记服务端日志、条目照常落盘）此前被
  // cropHtml 误判成"纯勾画没有手写"（notes.noCrop），实际上这条明明有手写，只是裁图暂时没生成——
  // 两种情况分开提示，别让用户误以为手写没被识别到。
  const cropHtml=e=>e.ink&&e.ink.crop?`<img src="${cropUrl(book.uuid,e.ink.crop)}" alt="${T('notes.cropAlt')}">`:`<div class="empty">${T(e.ink?'notes.cropMissing':'notes.noCrop')}</div>`;
  const patch=async(id,body)=>{const r=await postJ(`/api/ink/books/${encodeURIComponent(book.uuid)}/entries/${encodeURIComponent(id)}`,body);if(r.ok===false)toast(r.message||T('notes.saveFailed'))};
  /* 编辑区文本失焦才存（`onchange`），但点旁边的按钮（重转/去处/问AI…）会先让文本框失焦触发保存，
     两件事几乎同时各发一个 HTTP 请求，谁先到服务端不一定——按钮那次的收尾动作会拉新数据整页重画，
     如果保存请求还没落地，重画拿到的还是旧文本，编辑就跟着"消失"了（用户反馈"改了内容点重转不存"）。
     用一个 pendingText 记住"还没确认存上"的最新值，任何会拉新数据重画的动作之前先 flush 一遍，
     保证读到的一定是最新的。 */
  const pendingText=new Map();
  const flushPendingText=async()=>{if(!book||!pendingText.size)return;const items=[...pendingText];pendingText.clear();for(const[id,val]of items)await patch(id,{text:val})};
  /* 每章"设备笔记本/Obsidian md 是不是已经跟当前条目内容同步"（整理区第三轮反馈）：一次性取整本书
     的同步状态，章头徽章、「整理」列表默认收起已同步章节、回收站显示这条大概去哪了，三处共用同一份，
     不用各自发请求。`refreshSync()` 在 loadBook 里、以及每次生成/导出动作之后调用刷新。 */
  let syncMap=new Map();
  const refreshSync=async()=>{if(!book){syncMap=new Map();return}const r=await j(`/api/notes/books/${encodeURIComponent(book.uuid)}/sync`);syncMap=new Map((r.chapters||[]).map(c=>[c.chapter,c]))};
  /* 「保存并刷新」这条 5 步链（flush 未落地的改字 → 重取整本书 → 重取同步状态 → 重画指定的几个
     子视图）原来在 triage/archiveEntry/restore/去处切换/转写/问 AI 七处各自逐字重复（2026-09-09
     审计发现），任何一处漏改都容易造成"某个动作之后画面没更新"这类不容易被发现的 bug——收成一个
     辅助函数，调用方只需要说清楚"这次要重画哪几个子视图"。 */
  const reloadBook=async(...views)=>{await flushPendingText();book=await j(`/api/ink/books/${encodeURIComponent(book.uuid)}`);await refreshSync();views.forEach(fn=>fn())};
  /* s 是章节级同步状态（notebookNeeded/Synced、obsidianNeeded/Synced），本身只精确到"整章"，不到
     "这一条"（`fingerprint_chapter` 把整章活条目内容拼一起算一个哈希，见白皮书 §03aa）。章头调用不传
     `only`，如实显示整章的聚合状态；贴在每条笔记行上时传 `only=该条自己的 destination`，把跟这条本身
     无关的那个去处的徽章过滤掉——不然一章里别的条目要笔记本，会让只选了 Obsidian 的那条也显示"笔记本
     未同步"，真机反馈"我只导了 Obsidian，实际显示两者都有"就是这个问题，见白皮书 §03ab。 */
  const syncBadges=(s,only)=>{if(!s)return'';
    const wantsNb=!only||only==='notebook'||only==='both',wantsOb=!only||only==='obsidian'||only==='both';
    // aria-label 跟 title 重复一份（2026-09-09 审计修）：图标本身 aria-hidden，屏幕阅读器原来只能
    // 念出 ✓/… 两个符号，丢了"这是关于设备笔记本/Obsidian"的语境。
    // `xxxNeeded` 是"现在这一刻还有没有条目要这个去处"，条目的 destination 一改就可能立刻翻成 false——
    // 徽章原来只看这个字段，导致真机 bug（2026-09-17 反馈）：一章先推过笔记本，再把（唯一）那条条目的
    // 去处切成 Obsidian，笔记本徽章直接消失，像是"刚推过的笔记本状态丢了"，但设备上的笔记本文档其实
    // 完好无损，只是网页不再显示它存在过。改成"现在要 或者 历史上推过"（`xxxGeneratedAt`/`xxxExportedAt`
    // 是 `notebooks.rs`/`export_state.rs` 记的历史事实，不会因为条目 destination 改了就清空）都显示，
    // ✓/… 仍然只看 `xxxSynced`（是否跟当前条目内容一致）——这样"曾经推过但现在没有条目要了"会诚实地
    // 显示成"…"（不是当前状态的镜像，见标题文案），而不是整个徽章凭空消失。
    const nb=wantsNb&&(s.notebookNeeded||s.notebookGeneratedAt!=null)?`<span class="badge${s.notebookSynced?' on':''}" title="${T('notes.sync.notebook',{state:s.notebookSynced?T('notes.sync.synced'):T('notes.sync.notebookPending')})}" aria-label="${T('notes.sync.notebook',{state:s.notebookSynced?T('notes.sync.synced'):T('notes.sync.notSynced')})}">📓${s.notebookSynced?'✓':'…'}</span>`:'';
    const ob=wantsOb&&(s.obsidianNeeded||s.obsidianExportedAt!=null)?`<span class="badge${s.obsidianSynced?' on':''}" title="${T('notes.sync.obsidian',{state:s.obsidianSynced?T('notes.sync.synced'):T('notes.sync.obsidianPending')})}" aria-label="${T('notes.sync.obsidian',{state:s.obsidianSynced?T('notes.sync.synced'):T('notes.sync.notSynced')})}">${OBSIDIAN_ICON}${s.obsidianSynced?'✓':'…'}</span>`:'';
    return nb+ob};
  const trashList=$('#ntrashlist',sec),trashSum=$('#ntrashsum',sec);
  const TRASH_STATUSES=['skipped','revoked','archived'];
  /* 回收站（点 3）：不是一个只会清空的黑盒按钮——列出「不需要」「不要了」「已撤销」的条目实际内容，
     清空前能看清要丢的是什么。数据不用额外接口：GET /books/{uuid} 本来就带全部条目（含终态的）。 */
  /* 「恢复」（回收站点 3）：Skipped/Revoked/Archived 都能恢复，落点由服务端按条目已有内容倒推
     （见 notecore::model::Entry::restore）——书里已经把笔画擦了也能恢复，找回的是条目库里已经存好
     的裁图/校对文本，不代表设备原页面的笔迹会重新出现（这条限制在页面文案里说清楚，不是网页能力）。 */
  const restoreOne=async id=>{const r=await postJ(`/api/ink/books/${encodeURIComponent(book.uuid)}/entries/${encodeURIComponent(id)}/restore`,{});if(r.ok===false)return false;return true};
  const renderTrash=()=>{if(!book){trashList.innerHTML=`<p class="small">${T('notes.pickBookFirst')}</p>`;trashSum.textContent='';return}trashList.innerHTML='';
    const items=(book.entries||[]).filter(e=>TRASH_STATUSES.includes(e.status)).sort((a,b)=>b.updated-a.updated);
    trashSum.textContent=items.length?T('notes.trash.count',{count:items.length}):T('notes.trash.empty');
    if(!items.length){trashList.innerHTML=`<p class="small">${T('notes.trash.noneHint')}</p>`;return}
    items.forEach(e=>{const row=document.createElement('div');row.className='trash-item';
      const text=e.text||(e.drafts&&e.drafts[0]&&e.drafts[0].text)||(e.quote&&e.quote.text)||T('notes.noTextContent');
      const dv=e.destination||'both';
      const chSync=e.chapter!=null?syncMap.get(e.chapter):null;
      // 这条本身去哪（配置的目的地）+ 它所在章节目前的生成/导出状态（章节维度，不是这条自己确认被
      // 收进去了没——归档/撤销后这条已经不在活条目集合里，没法再逆推"当初有没有被打进那次生成"，
      // 只能诚实地给"这一章大致是什么状态"这个参考信息，用户反馈"回收站该显示导出到哪里"）。
      row.innerHTML=`<span class="badge">${T(STATUS_NAMES[e.status])||e.status}</span><span class="badge">${DEST_ICON[dv]()}</span>${syncBadges(chSync,dv)}
        <div class="txt">p.${e.page_index+1}${e.chapter_title?' · '+e.chapter_title:''}<br><span class="q">${text}</span>${e.status==='revoked'?`<br><span class="small">${T('notes.trash.revokedHint')}</span>`:''}</div>
        <button class="btn" data-restore>${T('notes.trash.restoreBtn')}</button>`;
      guardClick(row.querySelector('[data-restore]'),async()=>{if(!(await restoreOne(e.id)))return;await reloadBook(renderTrash,renderBrowse,renderBook)});
      trashList.appendChild(row)})};
  /* 「导入 md 文档」：单篇 markdown → 一份新设备笔记本文档，独立于条目库（不经浏览/整理/回收站那条
     状态机，见 note-serve::publish::import_markdown）。用户明确要求别塞进「整理」——那边是审阅真被
     要求转笔记的条目，跟"拿一份现成 .md 文件直接生成一份新笔记"是两件不同的事，各自一个入口。
     2026-09-10 从"文本框打字"改成"选一个 .md 文件"：文件内容用 FileReader 在浏览器里读成字符串，
     继续走现有的 JSON POST（{title,markdown}）——后端 import_markdown() 本来就是吃一个纯字符串，
     用户角度"选/拖文件"和"打字"的体验差异已经达到了，没必要为了这层不可见的传输差异去碰
     note-serve 的路由/multipart 解析，多一层没必要的风险面。 */
  const importTitle=$('#nimporttitle',sec),importFile=$('#nimportfile',sec),importFilename=$('#nimportfilename',sec),importBtn=$('#nimportbtn',sec),importStat=$('#nimportstat',sec);
  let importFileContent='';
  const renderImport=()=>{const ready=!!book;importTitle.disabled=importFile.disabled=importBtn.disabled=!ready;
    importStat.textContent=ready?'':T('notes.pickBookFirstShort')};
  importFile.onchange=async()=>{
    const f=importFile.files[0];
    if(!f){importFileContent='';importFilename.textContent='';return}
    importFileContent=await f.text();
    importFilename.textContent=`${f.name}（${fmtB(f.size)}）`;
    if(!importTitle.value.trim())importTitle.value=f.name.replace(/\.(md|markdown)$/i,''); // 顺手拿文件名当默认标题，仍可编辑
  };
  importBtn.onclick=async()=>{if(!book)return;
    const title=importTitle.value.trim()||(importFile.files[0]?importFile.files[0].name.replace(/\.(md|markdown)$/i,''):'');
    const markdown=importFileContent;
    if(!title){toast(T('notes.import.needTitle'),'warn');return}
    if(!markdown.trim()){toast(T('notes.import.needFile'),'warn');return}
    importBtn.disabled=true;importStat.textContent=T('notes.import.generating');
    const r=await postJ(`/api/notes/books/${encodeURIComponent(book.uuid)}/import-md`,{title,markdown}); // 失败 postJ 已经 alert 过
    importBtn.disabled=false;
    if(r.ok===false){importStat.textContent='';return}
    importStat.textContent=T('notes.import.done',{name:r.visibleName});importFile.value='';importFileContent='';importFilename.textContent=''};
  guardClick($('#nrestoreall',sec),async()=>{if(!book)return;
    const items=(book.entries||[]).filter(e=>TRASH_STATUSES.includes(e.status));
    if(!items.length){toast(T('notes.trash.noneToRestore'),'warn');return}
    if(!await confirmDialog(T('notes.trash.confirmRestoreAll',{count:items.length})))return;
    for(const e of items)await restoreOne(e.id);
    await reloadBook(renderTrash,renderBrowse,renderBook)});
  /* 浏览态动作：Mined→Pending（转入笔记）/ Mined→Skipped（不需要），见 ink-serve::triage。三个子视图都要重画（条目跨视图搬家）。 */
  const triage=async(id,action)=>{const r=await postJ(`/api/ink/books/${encodeURIComponent(book.uuid)}/entries/${encodeURIComponent(id)}/${action}`,{});if(r.ok===false)return;
    await reloadBook(renderBrowse,renderBook,renderTrash)};
  const updateSummary=()=>{if(!book){sum.textContent='';return}const es=book.entries||[];
    const c=st=>es.filter(e=>e.status===st).length;
    sum.textContent=T('notes.summary',{mined:c('mined'),pending:c('pending'),draft:c('draft'),reviewed:c('reviewed')})};
  // 全站唯一一处"按钮文案暗示有代价、却没有二次确认"（2026-09-09 审计发现）：清掉页记录会强制整本
  // 重新摄取。实际数据风险不大（已校对文本/条目不会被覆盖，见 notecore::ingest 的增量规则），但操作
  // 本身不常用、容易误触，补一句说清楚"安全在哪"的确认。
  guardClick($('#nrescan',sec),async()=>{if(!book)return;if(!await confirmDialog(T('notes.confirmRescan')))return;await flushPendingText();await postJ(`/api/ink/books/${encodeURIComponent(book.uuid)}/rescan`,{});refresh()});
  guardClick($('#npurge',sec),async()=>{if(!book)return;
    const items=(book.entries||[]).filter(e=>TRASH_STATUSES.includes(e.status));
    if(!items.length){toast(T('notes.trash.noneToPurge'),'warn');return}
    if(!await confirmDialog(T('notes.trash.confirmPurge',{count:items.length})))return;
    await flushPendingText();
    const r=await j(`/api/ink/books/${encodeURIComponent(book.uuid)}/purge`,{method:'POST'});
    if(r.ok===false){toast(r.message||T('notes.trash.purgeFailed'));return}
    book=await j(`/api/ink/books/${encodeURIComponent(book.uuid)}`);renderTrash();refresh()});
  /* 「不要了」（三期）：转 Archived，两处投影都摘掉，条目库里软删留痕（真删靠「回收站」清空）。 */
  const archiveEntry=async(id)=>{if(!await confirmDialog(T('notes.confirmArchive')))return;
    const r=await postJ(`/api/ink/books/${encodeURIComponent(book.uuid)}/entries/${encodeURIComponent(id)}/archive`,{});if(r.ok===false)return;
    await reloadBook(renderBook,renderTrash)};
  /* 浏览：按页分组、只列 Mined（待决定的），最近变更的页在前；转入笔记/不需要两个按钮直接调 triage。 */
  const renderBrowse=()=>{if(!book){browse.innerHTML=`<div class="card"><p class="small">${T('notes.pickBookFirst')}</p></div>`;return}browse.innerHTML='';updateSummary();
    const mined=(book.entries||[]).filter(e=>e.status==='mined');
    if(!mined.length){browse.innerHTML=`<div class="card"><p class="small">${T('notes.browse.empty')}</p></div>`;return}
    const groups=new Map();mined.forEach(e=>{const k=e.page_index;if(!groups.has(k))groups.set(k,[]);groups.get(k).push(e)});
    const recency=k=>Math.max(...groups.get(k).map(e=>e.updated));
    [...groups.keys()].sort((a,b)=>recency(b)-recency(a)).forEach(k=>{const es=groups.get(k).sort((a,b)=>(a.ink?a.ink.bbox[1]:0)-(b.ink?b.ink.bbox[1]:0));
      const card=document.createElement('div');card.className='card';card.innerHTML=`<h3 style="margin-top:0">${T('notes.pageHeading',{page:k+1})}${es[0].chapter_title?' · '+es[0].chapter_title:''} <span class="small">${T('notes.entryCount',{count:es.length})}</span></h3>`;
      es.forEach(e=>{const row=document.createElement('div');row.className='entry';
        row.innerHTML=`<div class="entry-body">
          <div class="entry-crop">${cropHtml(e)}</div>
          <div class="entry-main">
            ${e.quote?`<div class="entry-quote">「${e.quote.text}」</div>`:''}
            <div class="entry-ops"><div class="grp"><button class="btn pri" data-a="request">${T('notes.browse.request')}</button><button class="btn" data-a="skip">${T('notes.browse.skip')}</button></div></div>
          </div></div>`;
        row.querySelector('[data-a="request"]').onclick=()=>triage(e.id,'request');
        row.querySelector('[data-a="skip"]').onclick=()=>triage(e.id,'skip');
        card.appendChild(row)});
      browse.appendChild(card)})};
  /* 整理：只列真被要求转笔记的（Pending/Draft/Reviewed）——Mined 在「浏览」决定，Skipped/Revoked/Archived 去「回收站」。
     每条卡片三块视觉分区（点 4）：左手写裁图 ｜ 中转写/校对文本 ｜ 下问 AI 区，宽屏并排、手机堆叠（style.css .entry-*）。
     **第二轮反馈（2026-09-08）改动**：① 样式不再是下拉——`text` 一存，服务端就按行首 `-`/`1.`/`口`/`##`
     标记自动判样式（`notecore::model::Entry::apply_marked_text`），这里只显示一个只读徽章。② 去处
     （设备笔记本/Obsidian/都要）从下拉换成紧凑图标循环按钮，点一下切下一态。③ 每条一个勾选框；④ 转写
     失败的条目标红、按钮文案变「重转失败」——失败清单查一次 `/api/transcribe/status` 按条目 id 对上。
     **第三轮反馈（2026-09-08）改动**：生成笔记本/导出 md 挪回章头直接按钮（不再要求先勾选——这两个
     操作本来就是整章一起投影，选中哪几条对结果没有过滤作用，硬要求先勾选只是绕远路），批量勾选工具栏
     收窄成只剩真正逐条起作用的重转/不要了；章头新增 📓/Obsidian 同步徽章，全同步的章节默认从列表收起
     （"生成完成后是不是应该移出列表"），有「显示已同步的章节」开关能翻出来；回收站每条显示去处徽章 +
     所在章节的同步状态（"回收站该显示导出到哪里"），见 `refreshSync()`/`syncBadges()`/白皮书 §03x。
     **第四轮反馈（同一天）**：生成笔记本/导出 md 这两个按钮又被指出跟条目已有的「去处」字段重复——
     去处早就决定了这一章该不该落笔记本、该不该落 Obsidian，合并成一个按钮，内部按去处该做哪样做哪样，
     不用用户自己对着两个按钮再选一遍"点哪个"，见白皮书 §03y。
     **第五轮反馈（同一天）**：与其用一个「显示已同步的章节」复选框过滤一条长列表，改成「未导出/已导出」
     两个顶层 tab（复用 `fullySynced` 判据分组，两者互斥且穷尽，没有第三态），tab 下面章节做成第二层
     可点标签，点哪章只显示哪一章的内容——比上一轮"章节默认折叠"更彻底：折叠只是不用看见内容，滚动
     那条轴还在；这样任意时刻屏幕上最多一章的内容，滚动本身消失。旧的 `expandedChapters`（折叠/展开）
     整个被 `exportTab`/`selectedChapter` 两个状态取代。同步状态目前只精确到整章，做不到"这条笔记本身
     导出过没"，退而求其次把章节级同步徽章也贴一份到每条笔记行上。顺带把「同步本章」改名「推送本章」
     （"同步"暗示双向，这个按钮其实只单向推）、「重转」改名「重新转写」（跟「浏览」视图里语义完全不同
     的「转入笔记」共享"转"字，容易混），见白皮书 §03z。
     **第七轮反馈（同一天）**：① 批量勾选（每条复选框/「全选本章」/顶部隐藏工具栏）整段删除——那两个
     操作（重新转写/不要了）现在每条自己就有独立按钮，勾选层是纯粹的重复入口，跟这条线一贯"有独立
     按钮就别再叠一层批量选择"的取舍一致。② tab 归属判据从"整章内容是否跟最近一次投影完全匹配"
     （`fullySynced`）改成"这一章有没有被推送过"（`everExported`，只看 `notebookGeneratedAt`/
     `obsidianExportedAt` 是否非空）——原判据是两个布尔值的组合，编辑任意一条笔记的内容/落点都可能
     让整章的指纹对不上，"已导出"章节因为一次小编辑弹回"未导出"，用户反馈"多条数据的组合判断，一
     改落点就变成未导出"。改判存在性之后，推送过一次就稳定留在「已导出」，不再随内容变化在两个 tab
     间跳；`fullySynced`/`syncBadges` 没有被替换掉——它们继续管"章头/每条笔记的 ✓/… 徽章"，"这一章
     还有没有新改动没推送"这条信息没有丢，只是从"决定进哪个 tab"降级成"已导出 tab 内的一个提示"，
     见白皮书 §03ad。 */
  const exportTabsEl=$('#nexporttabs',sec);
  /* 双层导出状态视图（第五轮反馈，用户反馈"1章10条笔记，10章就100条，手机划几分钟才到底"）：
     顶层「未导出/已导出」两个 tab（第七轮反馈改成按 everExported 分组，见上），tab 下再按章节列第
     二层可点标签，点哪章只显示哪一章的内容——任意时刻屏幕上最多一章的内容，不靠折叠/滚动去缓解。
     exportTab 跨换书保留（比照原复选框状态本来也不随换书重置），selectedChapter 换书清空（章节 key
     按书算）。 */
  let exportTab='pending',selectedChapter=null;
  const renderBook=async(opts={})=>{if(!book){chaptertabs.innerHTML='';chapterbody.innerHTML=`<p class="small">${T('notes.pickBookFirst')}</p>`;return}updateSummary();
    const advance=!!opts.advance;
    const trst=await j('/api/transcribe/status');
    const failedIds=new Set((trst.failures||[]).filter(f=>f.book===book.uuid).map(f=>f.id));
    // 这份判据是 notes/crates/notecore/src/model.rs::Status::is_live_for_projection() 的镜像
    // （2026-09-09 单一事实源化：Rust 侧 project.rs/export.rs 都改成调那个方法了，前端这份因为
    // 跨语言/跨仓库做不到直接复用，改状态机时两边都要看一眼，别只改 Rust 那边）。
    const live=(book.entries||[]).filter(e=>['pending','draft','reviewed'].includes(e.status));
    const groups=new Map();live.forEach(e=>{const k=e.chapter==null?-1:e.chapter;if(!groups.has(k))groups.set(k,[]);groups.get(k).push(e)});
    const sortedKeys=[...groups.keys()].sort((a,b)=>a-b);
    // fullySynced：整章内容是否跟最近一次投影完全匹配——只用来算"✓/…"徽章，不再决定 tab 归属
    // （第七轮反馈：这两件事拆开，见上面大注释）。everExported：这一章有没有被推送过至少一次，
    // 决定 tab 归属，推送过就稳定留在「已导出」，不会因为后续编辑内容/落点又弹回「未导出」。
    const fullySynced=k=>{const s=k>=0?syncMap.get(k):null;return !!(s&&s.notebookSynced&&s.obsidianSynced)};
    const everExported=k=>{const s=k>=0?syncMap.get(k):null;return !!(s&&(s.notebookGeneratedAt!=null||s.obsidianExportedAt!=null))};
    const pendingKeys=sortedKeys.filter(k=>!everExported(k));
    const syncedKeys=sortedKeys.filter(k=>everExported(k));
    /* 选中章节的归属判定：默认"跟随"——只要这一章还有活条目，不管它现在算未导出还是已导出，都继续
       显示它，只是把 tab 高亮切到它现在所在的那边（真机反馈：点了条目自己的去处按钮后画面跳到了别
       的章节，读起来像数据错乱——其实是没有跟随，被"选中章节必须在当前 tab 可见列表里"这条校验当成
       "消失"处理了，随手选中了列表里第一个不相干的章节）。只有两种情况允许真的换到别的章节：显式点
       了顶层 tab 按钮（点击处理器会先把 selectedChapter 置空，走下面的兜底分支）、或显式要求"推送完
       这章就跳下一个待处理的"（`advance`，只有「推送本章」成功后传 true，是那个按钮特有的"处理完
       继续下一条"工作流，不该套用到编辑动作上）。 */
    if(selectedChapter!=null&&groups.has(selectedChapter)&&!advance){
      exportTab=everExported(selectedChapter)?'synced':'pending';
    }else{
      const pick=exportTab==='pending'?pendingKeys:syncedKeys;
      selectedChapter=pick.length?pick[0]:null;
    }
    exportTabsEl.querySelectorAll('button').forEach(b=>b.classList.toggle('on',b.dataset.etab===exportTab));
    const visibleKeys=exportTab==='pending'?pendingKeys:syncedKeys;
    chaptertabs.innerHTML='';
    visibleKeys.forEach(k=>{const b=document.createElement('button');b.className=k===selectedChapter?'on':'';
      b.textContent=k<0?T('notes.unfiledChapter'):T('notes.chapterHeading',{n:k+1});b.onclick=()=>{selectedChapter=k;renderBook()};chaptertabs.appendChild(b)});
    chapterbody.innerHTML='';
    if(selectedChapter==null){
      // 2026-09-09 审计修：pendingKeys 为空有两种完全不同的原因——"这本书压根没有条目被转入笔记过"
      // （sortedKeys 本身是空的）vs"都推送完了"（sortedKeys 非空但全部落进了已导出）。原文案不分这
      // 两种情况一律显示庆祝 emoji，容易让"还什么都没做"的用户误以为自己已经完成了操作。
      const emptyMsg=exportTab==='pending'
        ?(sortedKeys.length?T('notes.export.allPushed'):T('notes.export.noEntries'))
        :T('notes.export.nonePushedYet');
      chapterbody.innerHTML=`<p class="small">${emptyMsg}</p>`;
      return}
    const k=selectedChapter,es=groups.get(k).sort((a,b)=>a.page_index-b.page_index||(a.ink?a.ink.bbox[1]:0)-(b.ink?b.ink.bbox[1]:0));
    const s=k>=0?syncMap.get(k):null;
    const card=document.createElement('div');card.className='card';
    card.innerHTML=`<h3 style="margin-top:0">${k<0?T('notes.unfiledChapterParen'):T('notes.chapterHeadingTitled',{n:k+1,title:es[0].chapter_title||''})} <span class="small">${T('notes.entryCount',{count:es.length})}</span></h3>${k>=0?`<div class="row"><button class="btn pri" data-sync title="${T('notes.pushChapterTitle')}">${T('notes.pushChapterBtn')}</button>${syncBadges(s)}<span class="small" data-genmsg></span></div>`:''}<div data-body></div>`;
    const body=card.querySelector('[data-body]');
    if(k>=0){
      const syncBtn=card.querySelector('[data-sync]'),msg=card.querySelector('[data-genmsg]'),row=card.querySelector('.row');
      // 直接章头按钮，不用先勾选条目——生成/导出本来就是整章一起投影（条目挑不挑没用，见白皮书
      // §03x"是不是重复了"）；批量勾选整层第七轮反馈已经整段删掉，重新转写/不要了现在各自逐条一个
      // 独立按钮，见上面模块注释。
      // **合并成一个按钮（第四轮反馈）、改名「推送本章」（第五轮反馈）**：去处（Entry.destination）
      // 本来就已经决定了这一章该不该生成笔记本、该不该导出 md——分两个按钮让用户自己再选一遍"点哪个"
      // 是重复劳动，一个按钮内部按当前去处该做哪样做哪样：没有条目要那个去处，对应那步自然是 Empty
      // （后端已有这个语义，见 export::ExportOutcome/publish::ChapterOutcome），前端只是不重复提示
      // "没做"；改名"推送"是因为"同步"暗示双向/拉取，这个按钮其实只单向推。
      syncBtn.onclick=async()=>{syncBtn.disabled=true;msg.textContent='';holdRefreshUntil=Date.now()+15000;
        // 服务端没有天然的分步数据（耗时来自生成笔记本+导出 md 两次整章调用，不是可数的"第几步"）——
        // 跟 stagingList 普通整本落库同一处境，共用同一套不确定态滚动条（2026-09-19 代码质量审计，
        // 原来这里只有一句不会变的静态文字"推送中…"）。
        const prog=renderStepProgress(row,{label:T('notes.pushing'),prog:null,msg:''});
        const gr=await j(`/api/notes/books/${encodeURIComponent(book.uuid)}/chapters/${k}/generate`,{method:'POST'});
        const er=await j(`/api/notes/books/${encodeURIComponent(book.uuid)}/chapters/${k}/export`,{method:'POST'});
        prog.remove();
        syncBtn.disabled=false;
        const gc=(gr.chapters&&gr.chapters[0])||{};
        const parts=[];
        if(gr.ok===false)parts.push('✗ '+T('notes.push.notebook')+'：'+(gr.message||T('common.failed')));
        else if(gc.status==='failed')parts.push('✗ '+T('notes.push.notebook')+'：'+gc.error);
        else if(gc.status==='generated')parts.push('✓ '+T('notes.push.notebookUpdated'));
        if(er.ok===false)parts.push('✗ md：'+(er.message||T('common.failed')));
        else if(er.status==='written'){parts.push('✓ '+T('notes.push.mdExported'));window.open(`/api/notes/books/${encodeURIComponent(book.uuid)}/chapters/${k}/export.md`,'_blank')}
        msg.textContent=parts.length?parts.join(' · '):T('notes.push.noChange');
        holdRefreshUntil=Date.now()+3000; // 结果文案刚显示出来，从这一刻起再保 3s，不管上面两次请求实际花了多久
        await wait(3000);await refreshSync();renderBook({advance:true})};
    }
    es.forEach(e=>{const failed=failedIds.has(e.id);const row=document.createElement('div');row.className='entry'+(failed?' entry-failed':'');
      const draft=(e.drafts&&e.drafts[0])?e.drafts[0].text:'';
      const dv=e.destination||'both';
      row.innerHTML=`
        <div class="entry-head">
          <span>p.${e.page_index+1}${e.subhead?' · '+e.subhead:''}</span>
          <span class="badge">${T(STYLE_NAMES[e.style])||e.style}</span>
          ${syncBadges(s,dv)}
          <span class="badge ${e.status==='reviewed'?'on':''}" style="margin-left:auto">${T(STATUS_NAMES[e.status])||e.status}</span>
        </div>
        <div class="entry-body">
          <div class="entry-crop">${cropHtml(e)}</div>
          <div class="entry-main">
            ${e.quote?`<div class="entry-quote">「${e.quote.text}」</div>`:''}
            <textarea class="entry-text" rows="2" placeholder="${draft?T('notes.draftPlaceholder',{draft}):T('notes.waitingTranscribe')}">${e.text||draft}</textarea>
            <div class="small">${T('notes.styleHint')}</div>
            <div class="entry-ops">
              <div class="grp"><button class="btn" data-dest title="${T('notes.dest.switchTitle')}">${DEST_ICON[dv]()} <span aria-hidden="true" style="opacity:.55">⟳</span></button></div>
              <div class="grp">${(e.ink&&e.ink.crop)?`<button class="btn${failed?' btn-bad':''}" data-transcribe title="${T('notes.retranscribeTitle')}">${failed?T('notes.transcribeFailed'):(draft?T('notes.retranscribe'):T('notes.transcribe'))}</button>`:''}<button class="btn" data-archive title="${T('notes.archiveTitle')}">${T('notes.archiveBtn')}</button></div>
            </div>
            <div class="small" data-txstat></div>
            <div class="entry-ask">
              <div class="row"><label class="toggle"><input type="checkbox" data-ask ${e.ask_ai?'checked':''}> ${T('notes.askAi')}</label>
                <input type="text" data-question placeholder="${T('notes.questionPlaceholder')}" value="${e.question?e.question.replace(/"/g,'&quot;'):''}" style="flex:1;min-width:9em" ${e.ask_ai?'':'disabled'}>
                <button class="btn pri" data-askbtn ${e.ask_ai&&e.question?'':'disabled'}>${T('notes.askBtn')}</button></div>
              <div class="small" data-askstat></div>
              ${e.answer?`<div class="entry-answer"><b>${T('notes.aiAnswer')}</b>（${T('notes.askedLabel',{brief:e.answer.brief})}）<br>${e.answer.text}</div>`:''}
            </div>
          </div>
        </div>`;
      const ta=row.querySelector('textarea');
      ta.oninput=ev=>pendingText.set(e.id,ev.target.value);   // 还没失焦确认，先记住最新值，别的动作重画前会先冲掉
      ta.onchange=ev=>{pendingText.delete(e.id);patch(e.id,{text:ev.target.value})};
      guardClick(row.querySelector('[data-dest]'),async()=>{const next=DEST_ORDER[(DEST_ORDER.indexOf(dv)+1)%3];await patch(e.id,{destination:next});await reloadBook(renderBook)});
      row.querySelector('[data-archive]').onclick=()=>archiveEntry(e.id);
      /* 点「重新转写」/「提问」弹出状态和这次调用的消耗（用户反馈"应该弹出状态及当前消耗"，2026-09-08
         第三轮）：先显文字状态（转写中…/提问中…），拿到结果显示"✓ 完成 · token 入X 出Y"或错误，
         停留一小会儿让用户真的看得到（不然紧接着的整页重画会立刻把这条状态盖掉，等于白显示）——
         最初给的 1.5s 真机反馈"闪一下就没了"根本来不及读，2026-09-16 延长到 3s（「推送本章」
         那条同款状态提示也一起延长，三处是同一个模式）。 */
      const tb=row.querySelector('[data-transcribe]'),txStat=row.querySelector('[data-txstat]');
      if(tb)tb.onclick=async()=>{tb.disabled=true;txStat.textContent=T('notes.transcribing');holdRefreshUntil=Date.now()+15000;
        const r=await j(`/api/transcribe/books/${encodeURIComponent(book.uuid)}/entries/${encodeURIComponent(e.id)}`,{method:'POST'});
        tb.disabled=false;
        txStat.textContent=r.ok===false?('✗ '+(r.message||T('notes.transcribeFailed'))):T('notes.transcribeDone',{promptTokens:r.promptTokens||0,completionTokens:r.completionTokens||0});
        holdRefreshUntil=Date.now()+3000;
        await wait(3000);await reloadBook(renderBook)};
      /* 「问AI」勾选框 + 问题 + 提问按钮：改即存（ink-serve），点提问才真的调 mind-serve。 */
      const askBox=row.querySelector('[data-ask]'),qInput=row.querySelector('[data-question]'),askBtn=row.querySelector('[data-askbtn]'),askStat=row.querySelector('[data-askstat]');
      const syncAskUi=()=>{qInput.disabled=!askBox.checked;askBtn.disabled=!(askBox.checked&&qInput.value.trim())};
      askBox.onchange=()=>{patch(e.id,{askAi:askBox.checked});syncAskUi()};
      qInput.onchange=()=>{patch(e.id,{question:qInput.value});syncAskUi()};
      askBtn.onclick=async()=>{askBtn.disabled=true;askStat.textContent=T('notes.asking');holdRefreshUntil=Date.now()+15000;
        const r=await j(`/api/mind/books/${encodeURIComponent(book.uuid)}/entries/${encodeURIComponent(e.id)}/ask`,{method:'POST'});
        askBtn.disabled=false;
        if(r.ok===false){askStat.textContent='✗ '+(r.message||T('notes.askFailed'));holdRefreshUntil=0}
        else{askStat.textContent=T('notes.askDone',{promptTokens:r.promptTokens||0,completionTokens:r.completionTokens||0});holdRefreshUntil=Date.now()+3000;await wait(3000);await reloadBook(renderBook)}};
      body.appendChild(row)});
    chapterbody.appendChild(card)};
  exportTabsEl.querySelectorAll('button').forEach(b=>b.onclick=()=>{if(exportTab===b.dataset.etab)return;exportTab=b.dataset.etab;selectedChapter=null;renderBook()});
  const loadBook=async()=>{await flushPendingText();selectedChapter=null;if(!sel.value){book=null;await refreshSync();renderBrowse();await renderBook();renderTrash();renderImport();return}book=await j(`/api/ink/books/${encodeURIComponent(sel.value)}`);await refreshSync();renderBrowse();await renderBook();renderTrash();renderImport()};
  sel.onchange=loadBook;
  /* 「导入 md 文档」子标签的显示/隐藏跟着「管理→实验室」的 notesImportMdEnabled 开关走——用
     hidden 属性而不是从 DOM 移除（subtabs() 是纯位置下标配对，移除会让后面的子标签全部错位）。
     没有 SSE 推送这个开关的变化，靠 sec.refresh（笔记 tab 每次从别的 tab 切回来都会调，见文件
     末尾 IIFE 里 addTab 的点击处理）顺带每次重新拉一次状态，跟这个 app 里"tab 记脏、切回时刷新"
     的既有设计一致。 */
  const importNavBtn=$('#nsubnav',sec).children[3],importPanel=$('#nimport',sec);
  const syncImportVisible=async()=>{
    const r=await j('/api/enhance/status');
    const show=r.ok!==false&&!!r.notesImportMdEnabled;
    if(!show&&importNavBtn.classList.contains('on'))$('#nsubnav',sec).children[0].click(); // 正停在「导入」时先切回「浏览」，避免 hidden+on 类同时存在
    importNavBtn.hidden=!show;importPanel.hidden=!show;
  };
  const refresh=async()=>{if(Date.now()<holdRefreshUntil)return; // 正显示着结果提示，别被 SSE 抢跑冲掉（见 holdRefreshUntil 声明处注释）
    const d=await j('/api/ink/books');const cur=sel.value;sel.innerHTML=(d.items||[]).map(b=>`<option value="${b.uuid}">${b.title}（${b.entries}）</option>`).join('')||`<option value="">${T('notes.noBooks')}</option>`;
    if(cur&&[...sel.options].some(o=>o.value===cur))sel.value=cur;await loadBook();await syncImportVisible()};
  refresh();sec.refresh=refresh;subtabs(sec)}

// 只放 key 名，DashScope/OpenAI/Gemini/DeepSeek 本身是厂商专名不翻，括注里的中文说明才走 T()（同样是
// 顶层 const 只存 key、真正查找挪到调用点的规则，见 T() 头注）。
const PROVIDER_NAMES={dashscope:'models.provider.dashscope',openai:'models.provider.openai',gemini:'models.provider.gemini',deepseek:'models.provider.deepseek'};
/* 模型管理卡片（点 2「彻底重做」，「管理」tab 用，transcribe/mind 共用同一套 UI，2026-09-08 第二轮反馈；
   2026-09-08 又一轮反馈：厂家/模型拆成两级下拉，别把七八个不同厂家的模型糊在一个框里选）：
   第一级「厂家」下拉（DashScope/OpenAI/Gemini/DeepSeek/自定义），第二级「模型」下拉只列选中厂家的
   模型——选厂家会自动定位到该厂家的第一个模型（服务端 `preset` 立即原子切换，不用再点一次确认）；
   选"自定义"隐藏模型下拉、露出手填 model/baseUrl。key 按厂商分开存、脱敏后只剩「删除」，没配 key
   时才给输入框——不允许在已有 key 时直接改写覆盖，逼着"先删再填"；切换预置不会丢别的厂商已存的 key
   （服务端按 provider 分格）。每个模型自己的用量+花费一行（`usageByModel`，花费＝用户自填单价×token，
   没填单价不显示金额，见 config.rs 模块文档"花费不做官方定价表"——第三方定价常变，不猜）；`showAuto`
   给 transcribe 用，多一个"合书自动转写"开关（模型服务级别的设置，原来在「整理」页的折叠层已经去掉，
   见 renderNotes）。返回一个 refresh 函数，挂到 tab 的 sec.refresh 上，切回这个 tab 时数据不过期。 */
function mountModelPanel(root,seg,title,icon,showAuto){
  const card=document.createElement('div');card.className='card';card.style.cssText='width:100%;margin:0';
  card.innerHTML=`<h3 style="margin-top:0">${icon} ${title}</h3>
    <div class="row">
      <div style="flex:1;min-width:11em"><label class="field">${T('models.vendorLabel')}</label><select data-vendor style="width:100%"></select></div>
      <div style="flex:1;min-width:11em" data-modelbox><label class="field">${T('models.modelLabel')}</label><select data-preset style="width:100%"></select></div>
    </div>
    <div class="row" data-custom hidden>
      <input type="text" data-model placeholder="${T('models.customModelPlaceholder')}" style="max-width:11em">
      <input type="text" data-url placeholder="${T('models.customUrlPlaceholder')}" style="flex:1;min-width:12em">
      <button class="btn" data-savecustom>${T('models.saveCustomBtn')}</button>
    </div>
    <label class="field">${T('models.apiKeyLabel')}</label>
    <div class="row" data-keyrow></div>
    ${showAuto?`<div class="row"><label class="toggle"><input type="checkbox" data-auto> ${T('models.autoTranscribeToggle')}</label></div>`:''}
    <label class="field">${T('models.priceLabel')}</label>
    <div class="row" data-pricerow>
      <input type="number" step="0.001" min="0" data-pricein placeholder="${T('models.priceInPlaceholder')}" style="max-width:7em">
      <input type="number" step="0.001" min="0" data-priceout placeholder="${T('models.priceOutPlaceholder')}" style="max-width:7em">
      <button class="btn" data-pricesave>${T('models.savePriceBtn')}</button>
    </div>
    <label class="field">${T('models.usageLabel')}</label>
    <div class="tblwrap" data-usagewrap><table class="cmp"><thead><tr><th>${T('models.usage.colModel')}</th><th>${T('models.usage.colCalls')}</th><th>${T('models.usage.colTokens')}</th><th>${T('models.usage.colCost')}</th></tr></thead><tbody data-usagebody></tbody></table></div>
    <div class="small" data-stat style="margin-top:.3em;overflow-wrap:anywhere"></div>`;
  root.appendChild(card);
  const vendorSel=card.querySelector('[data-vendor]'),modelBox=card.querySelector('[data-modelbox]'),presetSel=card.querySelector('[data-preset]'),customBox=card.querySelector('[data-custom]'),modelInp=card.querySelector('[data-model]'),urlInp=card.querySelector('[data-url]'),keyRow=card.querySelector('[data-keyrow]'),stat=card.querySelector('[data-stat]'),autoBox=card.querySelector('[data-auto]'),priceIn=card.querySelector('[data-pricein]'),priceOut=card.querySelector('[data-priceout]'),usageBody=card.querySelector('[data-usagebody]');
  const put=body=>j(`/api/${seg}/config`,{method:'PUT',body:JSON.stringify(body)});
  const fmtCost=c=>c==null?T('models.noPrice'):'¥'+c.toFixed(4);
  let presets=[];
  const modelsOf=v=>presets.filter(p=>p.provider===v);
  const refresh=async()=>{
    const st=await j(`/api/${seg}/status`);
    if(st.ok===false){stat.textContent=T('models.notReady',{msg:st.message||T('models.notReadyDefault')});vendorSel.disabled=true;keyRow.innerHTML='';usageBody.innerHTML='';return}
    const c=st.config||{};
    presets=c.presets||[];
    vendorSel.disabled=false;
    const vendors=[...new Set(presets.map(p=>p.provider))];
    vendorSel.innerHTML=vendors.map(v=>`<option value="${v}">${T(PROVIDER_NAMES[v])||v}</option>`).join('')+`<option value="custom">${T('models.customVendor')}</option>`;
    const activeVendor=c.activePreset==='custom'?'custom':(presets.find(p=>p.id===c.activePreset)||{}).provider||'custom';
    vendorSel.value=activeVendor;
    const isCustom=activeVendor==='custom';
    customBox.hidden=!isCustom;modelBox.hidden=isCustom;
    if(isCustom){modelInp.value=c.model||'';urlInp.value=c.baseUrl||''}
    else{presetSel.innerHTML=modelsOf(activeVendor).map(p=>`<option value="${p.id}">${p.label}</option>`).join('');presetSel.value=c.activePreset}
    keyRow.innerHTML=c.hasKey
      ?`<span class="small">${T('models.keySaved',{key:c.keyMasked||'••••'})}</span><button class="btn" data-delkey>${T('action.delete')}</button>`
      :`<input type="password" placeholder="${T('models.keyInputPlaceholder')}" data-keyinput style="flex:1;min-width:11em" autocomplete="off"><button class="btn pri" data-savekey>${T('models.saveKeyBtn')}</button>`;
    if(autoBox){autoBox.checked=!!c.auto;autoBox.onchange=async()=>{const r=await put({auto:autoBox.checked});if(r.ok===false){toast(r.message||T('common.failed'));autoBox.checked=!autoBox.checked}}}
    const price=c.price||{inputPer1k:0,outputPer1k:0};
    priceIn.value=price.inputPer1k||'';priceOut.value=price.outputPer1k||'';
    const rows=st.usageByModel||[];
    usageBody.innerHTML=rows.length?rows.map(m=>`<tr${m.active?' style="font-weight:600"':''}><td>${m.label}${m.active?` <span class="badge on">${T('models.usage.active')}</span>`:''}</td><td>${m.calls}${m.failed?` <span style="color:var(--bad)">${T('models.usage.failedCount',{n:m.failed})}</span>`:''}</td><td>${m.promptTokens}/${m.completionTokens}</td><td>${fmtCost(m.costEstimate)}</td></tr>`).join(''):`<tr><td colspan="4" class="small">${T('models.usage.none')}</td></tr>`;
    stat.textContent=rows.find(m=>m.active&&m.lastError)?.lastError?T('models.lastError',{err:rows.find(m=>m.active).lastError}):'';
    const delBtn=keyRow.querySelector('[data-delkey]'),saveBtn=keyRow.querySelector('[data-savekey]');
    if(delBtn)guardClick(delBtn,async()=>{if(!await confirmDialog(T('models.confirmDeleteKey',{title})))return;const r=await put({clearKey:true});if(r.ok===false)toast(r.message||T('models.deleteFailed'));refresh()});
    if(saveBtn)guardClick(saveBtn,async()=>{const v=keyRow.querySelector('[data-keyinput]').value.trim();if(!v)return;const r=await put({apiKey:v});if(r.ok===false)toast(r.message||T('common.failed'));refresh()});
  };
  /* 选厂家：不是自定义就直接定位到该厂家第一个模型并原子切换（不用再点一次「确认」）；选自定义只切
     UI（露出手填框），真正生效要等用户填完点「保存自定义」——避免半吊子状态被当成已保存的配置发出去。 */
  vendorSel.onchange=async()=>{const v=vendorSel.value;customBox.hidden=v!=='custom';modelBox.hidden=v==='custom';
    if(v==='custom')return;
    const first=modelsOf(v)[0];if(!first)return;
    const r=await put({preset:first.id});if(r.ok===false)toast(r.message||T('common.failed'));refresh()};
  presetSel.onchange=async()=>{const r=await put({preset:presetSel.value});if(r.ok===false)toast(r.message||T('common.failed'));refresh()};
  guardClick(card.querySelector('[data-savecustom]'),async()=>{const r=await put({preset:'custom',model:modelInp.value.trim(),baseUrl:urlInp.value.trim()});if(r.ok===false)toast(r.message||T('common.failed'));refresh()});
  guardClick(card.querySelector('[data-pricesave]'),async()=>{const r=await put({price:{input:parseFloat(priceIn.value)||0,output:parseFloat(priceOut.value)||0}});if(r.ok===false)toast(r.message||T('common.failed'));refresh()});
  refresh();
  return refresh;
}

/* 电池刺客（battop）——2026-09-10 拆成两处：①「实验室」卡片只留开关+说明（mountBattopToggleCard，
   checkbox 直接对应 systemd start/stop，不是 reading-qol.json 那种纯 JSON 开关）；②「管理→电池
   刺客」二级 tab（renderBattopDetail，运行时才出现，见 renderManage 里的可见性同步逻辑）放真正的
   数据——耗电情况（按应用/按进程）+ 唤醒源两个三级子标签，数据源是 battop 常驻聚合的 summary.json
   （4 个时间窗：今日/7天/30天/全部），网页不重新聚合，只管排版，跟以前设备端 battery-audit.sh/
   FINDINGS.md 那份报告对标的思路一样，只是这次是持续聚合不是一次性跑分析脚本。 */
const fmtMs=ms=>ms>=3600000?T('battop.hours',{n:(ms/3600000).toFixed(1)}):ms>=60000?T('battop.minutes',{n:(ms/60000).toFixed(1)}):T('battop.seconds',{n:(ms/1000).toFixed(0)});
// 顶层常量只放 key 名（label 字段），真正的 T() 查找挪到 renderBattopWindowed 里（渲染时执行），见 T() 头注。
const BATTOP_WINDOWS=[{key:'today',label:'battop.window.today'},{key:'7d',label:'battop.window.7d'},{key:'30d',label:'battop.window.30d'},{key:'all',label:'battop.window.all'}];
const battopTopList=items=>items&&items.length
  ?`<ul class="list">${items.map(it=>`<li><span>${it.name}</span><span class="small">${fmtMs(it.ms)} · ${it.pct}%</span></li>`).join('')}</ul>`
  :`<p class="small">${T('battop.noData')}</p>`;
/* 时间窗 subnav+subpanel 骨架，耗电情况/唤醒源两处共用——contentFn(windowData)→这个窗口要显示的 HTML。 */
/* activeIdx：重画时保留原来选中的时间窗（比如耗电情况的"按应用/按进程"下拉切换只想换列表内容，
   不想把用户刚选的"7天"弹回"今日"），不传就默认第一个。每个时间窗的内容包一层 `.card`——跟这个
   app 别处"subnav 切换、每块内容各自一张卡"的样子统一（KOReader 字体/词典两个子标签各自一张卡
   是同一个规矩，battop 详情页之前漏了这层，2026-09-10 用户指出补上）。 */
function renderBattopWindowed(container,windowsData,contentFn,activeIdx=0){
  container.innerHTML=`<div class="subnav">${BATTOP_WINDOWS.map((x,i)=>`<button${i===activeIdx?' class="on"':''}>${T(x.label)}</button>`).join('')}</div>
    ${BATTOP_WINDOWS.map((x,i)=>`<div class="subpanel${i===activeIdx?' on':''}"><div class="card">${contentFn(windowsData[x.key]||{})}</div></div>`).join('')}`;
  subtabs(container);
}
/* 当前激活的时间窗下标——重画前先读一遍，喂给上面的 activeIdx。 */
function battopActiveWindowIdx(container){
  const nav=container.querySelector(':scope > .subnav');
  if(!nav)return 0;
  const i=[...nav.children].findIndex(b=>b.classList.contains('on'));
  return i<0?0:i;
}

function mountBattopToggleCard(container){
  container.innerHTML=`<h3 style="margin-top:0">${T('battop.title')}</h3>
    <p class="small">${T('battop.toggle.desc')}</p>
    <label class="toggle"><input type="checkbox" data-box disabled> ${T('battop.toggle.label')}</label>
    <p class="small" data-note></p>`;
  const box=container.querySelector('[data-box]'),note=container.querySelector('[data-note]');
  let installed=false;
  const refresh=async()=>{const r=await j('/api/enhance/status');if(r.ok===false)return;
    const st=r.battop||{};installed=!!st.installed;
    box.checked=!!st.running;box.disabled=!installed;
    note.textContent=installed?'':T('battop.toggle.notInstalled')};
  box.onchange=async()=>{if(!installed)return;const want=box.checked;box.disabled=true;
    const r=await j(`/api/enhance/battop/${want?'start':'stop'}`,{method:'POST'});
    if(r.ok===false){toast(r.message||T('common.failed'));box.checked=!want}
    box.disabled=false;await refresh()};
  refresh();
  return refresh;
}

/* 「管理→电池刺客」二级 tab 内容：耗电情况（按应用+按进程）/ 唤醒源，各自内部再按时间窗切换
   （三级嵌套：管理 subnav → 电池刺客 subpanel → 这里的 subnav → 耗电情况/唤醒源 subpanel →
   renderBattopWindowed 自己的 subnav → 时间窗 subpanel——subtabs() 的 `:scope >` 收紧保证每层
   只认自己的直接子元素，见 subtabs() 头注）。 */
function renderBattopDetail(sec){
  sec.innerHTML=`<div class="subnav"><button class="on">${T('battop.subnav.usage')}</button><button>${T('battop.subnav.wake')}</button></div>
    <div class="subpanel on" data-usage></div>
    <div class="subpanel" data-wake></div>`;
  const usageEl=sec.querySelector('[data-usage]'),wakeEl=sec.querySelector('[data-wake]');
  const refresh=async()=>{
    const r=await j('/api/enhance/battop/summary');
    if(r.ok===false||!r.available){
      const msg=`<div class="card"><p class="small">${T('battop.noSamplesYet')}</p></div>`;
      usageEl.innerHTML=msg;wakeEl.innerHTML=msg;return;
    }
    const w=r.summary.windows||{};
    /* 每个子标签开头一张说明卡（标题+一句话说明），跟「系统增强」/KOReader 那些卡片同一个
       视觉语言；「按应用/按进程」下拉放这张卡里——下拉要跨时间窗持续存在，不能放进
       renderBattopWindowed 生成的、每次切时间窗都可能重画的内容里（2026-09-10 用户要求统一
       风格顺手理清楚这条边界）。 */
    if(!usageEl.querySelector('[data-metric]')){
      usageEl.innerHTML=`<div class="card"><h3 style="margin-top:0">${T('battop.usage.title')}</h3>
        <p class="small">${T('battop.usage.desc')}</p>
        <div class="row"><label class="small" for="battopMetric">${T('battop.usage.metricLabel')}</label>
        <select id="battopMetric" data-metric><option value="app">${T('battop.usage.byApp')}</option><option value="proc">${T('battop.usage.byProcess')}</option></select></div></div>
        <div data-usagewin></div>`;
    }
    const metricSel=usageEl.querySelector('[data-metric]'),usageWinEl=usageEl.querySelector('[data-usagewin]');
    const renderUsage=()=>{
      const label=metricSel.value==='app'?T('battop.usage.byApp'):T('battop.usage.byProcess');
      renderBattopWindowed(usageWinEl,w,d=>`<p class="small">${T('battop.usage.statLine',{discharge:d.discharge||0,mah:d.mah||0,ma:d.ma||0,samples:d.samples||0})}</p>
        <h4 style="margin:.6em 0 .2em">${T('battop.usage.rankHeading',{metric:label})}</h4>${battopTopList(metricSel.value==='app'?d.app:d.proc)}`,
        battopActiveWindowIdx(usageWinEl));
    };
    metricSel.onchange=renderUsage;
    renderUsage();
    if(!wakeEl.querySelector('[data-wakewin]')){
      wakeEl.innerHTML=`<div class="card"><h3 style="margin-top:0">${T('battop.wake.title')}</h3>
        <p class="small">${T('battop.wake.desc')}</p></div>
        <div data-wakewin></div>`;
    }
    renderBattopWindowed(wakeEl.querySelector('[data-wakewin]'),w,d=>`<p class="small">${T('battop.wake.samples',{n:d.samples||0})}</p>
      <h4 style="margin:.6em 0 .2em">${T('battop.wake.rankHeading')}</h4>${battopTopList(d.wake)}`,
      battopActiveWindowIdx(wakeEl.querySelector('[data-wakewin]')));
  };
  refresh();sec.refresh=refresh;subtabs(sec);
}


/* 管理台/引导（固定 tab，始终在——它是网关自身页面，不由服务注册表驱动） */
/* 「管理」二级 tab（2026-09-09 起三个，2026-09-10 加到五个）：① 基石与模块（原来就有的引导/开关/
   卸载）② 模型管理（原来挂在这页最下面，现在单独一屏，不用跟基石列表一起滚）③ 系统增强（只留真正
   "系统级"的开关，CJK 画线吸附）④ 电池刺客（`battop.running` 时才出现，放在「实验室」前面——用户
   要求顺序）⑤ 实验室（还在打磨/覆盖面没到日常好用程度的功能：CJK 手写笔迹优化开关+电池刺客开关
   本身+导入md文档可见性开关）。
   shelf push 命令那张卡片已经搬到「传书」页「入库」子页——那才是它真正归属的地方（用户反馈）。 */
function renderManage(sec){sec.innerHTML=`
  <div class="subnav"><button class="on">${T('manage.subnav.foundation')}</button><button>${T('manage.subnav.models')}</button><button>${T('manage.subnav.enhance')}</button><button hidden>${T('manage.subnav.battop')}</button><button>${T('manage.subnav.lab')}</button></div>
  <div class="subpanel on">
    <div class="card"><h2>${T('manage.foundation.title')}</h2><p class="lead">${T('manage.foundation.lead')}</p>
      <div class="kv small" id="found">${T('manage.foundation.checking')}</div>
      <p class="small">${T('manage.foundation.links')}</p></div>
    <div class="card"><h2>${T('manage.modules.title')}</h2>
      <p class="lead">${T('manage.modules.lead')}</p>
      <details class="cmp"><summary>${T('manage.modules.helpSummary')}</summary>
        <dl class="help">
          <dt>${T('manage.modules.help.states.dt')}</dt>
          <dd>${T('manage.modules.help.states.dd')}</dd>
          <dt>${T('manage.modules.help.toggle.dt')}</dt>
          <dd>${T('manage.modules.help.toggle.dd')}</dd>
          <dt>${T('manage.modules.help.perf.dt')}</dt>
          <dd>${T('manage.modules.help.perf.dd')}</dd>
          <dt>${T('manage.modules.help.uninstall.dt')}</dt>
          <dd>${T('manage.modules.help.uninstall.dd')}</dd>
          <dt>${T('manage.modules.help.install.dt')}</dt>
          <dd>${T('manage.modules.help.install.dd')}</dd>
        </dl></details>
      <div class="row"><button class="btn" id="allon">${T('manage.modules.allOn')}</button><button class="btn" id="alloff">${T('manage.modules.allOff')}</button></div>
      <ul class="list" id="mods"></ul></div>
  </div>
  <!-- 意图卡（h2+lead）单独一张、跟下面的模型卡是兄弟不是父子（2026-09-10 用户要求跟「管理」页
       其它子标签统一风格——「传书·入库」「引导·基石」都是这个样子：一张说明卡起头，后面各功能
       各自一张卡平铺；改之前这里是说明卡把 #modelcards 包在里面，卡中卡，跟别处不一样）。 -->
  <div class="subpanel">
    <div class="card"><h2>${T('manage.models.title')}</h2><p class="lead">${T('manage.models.lead')}</p></div>
    <div id="modelcards" style="display:flex;flex-direction:column;gap:1em"></div>
  </div>
  <div class="subpanel">
    <div class="card"><h3 style="margin-top:0">${T('manage.enhance.hlSnap.title')}</h3>
      <p class="small">${T('manage.enhance.hlSnap.desc')}</p>
      <label class="toggle"><input type="checkbox" id="erHlSnap"> ${T('manage.enhance.hlSnap.toggle')}</label></div>
  </div>
  <div class="subpanel" id="battopDetail" hidden></div>
  <div class="subpanel">
    <div class="card"><h3 style="margin-top:0">${T('manage.lab.hwStroke.title')}</h3>
      <p class="small">${T('manage.lab.hwStroke.desc')}</p>
      <label class="toggle"><input type="checkbox" id="labHwStroke"> ${T('manage.lab.hwStroke.toggle')}</label></div>
    <div class="card" id="labBattopCard"></div>
    <div class="card"><h3 style="margin-top:0">${T('manage.lab.importMd.title')}</h3>
      <p class="small">${T('manage.lab.importMd.desc')}</p>
      <label class="toggle"><input type="checkbox" id="labImportMd"> ${T('manage.lab.importMd.toggle')}</label></div>
  </div>`;
  const mvRefresh=mountModelPanel($('#modelcards',sec),'transcribe',T('manage.models.visionTitle'),'👁',true);
  const mtRefresh=mountModelPanel($('#modelcards',sec),'mind',T('manage.models.textTitle'),'✎');
  const refresh=async()=>{
    const f=await j('/api/foundation');$('#found',sec).innerHTML=f.ok===false?`<span>${f.message}</span>`:
      `<b>xovi</b><span>${badge(f.xovi?T('common.installed'):T('common.notInstalled'),f.xovi)}</span><b>appload</b><span>${badge(f.appload?T('common.installed'):T('common.notInstalled'),f.appload)}</span><b>qt-resource-rebuilder</b><span>${badge(f.qrr?T('common.installed'):T('common.notInstalled'),f.qrr)}</span><b>KOReader</b><span>${badge(f.koreader?T('common.installed'):T('common.notInstalled'),f.koreader)}</span><b>WeRead</b><span>${badge(f.weread?T('common.installed'):T('common.notInstalled'),f.weread)}</span>`;
    const d=await j('/api/manage');const ul=$('#mods',sec);ul.innerHTML='';(d.modules||[]).forEach(m=>{const li=document.createElement('li');li.style.flexWrap='wrap';
      let state,cls;if(!m.installable){state=T('manage.modules.state.notLaunched');cls=''}else if(!m.installed){state=T('common.notInstalled');cls='off'}else if(m.running){state=T('manage.modules.state.on');cls='on'}else{state=T('manage.modules.state.installedOff');cls=''}
      const label=T('manage.modules.label.'+m.seg)||m.label; // seg 缺对应 key 时兜底用后端 Rust 侧的中文 label，不留空
      const left=document.createElement('span');left.innerHTML=`${label} <span class="small">${m.service}</span> <span class="badge ${cls}">${state}</span>`;
      const right=document.createElement('span');right.style.cssText='display:flex;gap:.4em;align-items:center';
      if(m.installable&&m.installed){
        const t=document.createElement('button');t.className='btn';t.textContent=m.running?T('manage.modules.turnOff'):T('manage.modules.turnOn');
        guardClick(t,async()=>{const r=await j('/api/manage/'+m.seg+'/'+(m.running?'stop':'start'),{method:'POST'});if(r.ok===false)toast(r.message);setTimeout(refresh,600)});right.appendChild(t);
        const u=document.createElement('button');u.className='btn';u.textContent=T('manage.modules.uninstallBtn');
        guardClick(u,async()=>{if(await confirmDialog(T('manage.modules.confirmUninstall',{label}))){const r=await j('/api/manage/'+m.seg+'/uninstall',{method:'POST'});if(r.ok===false)toast(r.message);else toast(T('manage.modules.uninstalled',{label}),'ok');setTimeout(()=>location.reload(),800)}});right.appendChild(u);
      }else if(m.installable){const g=document.createElement('span');g.className='small';g.innerHTML=T('manage.modules.installCmd',{only:m.only});right.appendChild(g)}
      li.append(left,right);ul.appendChild(li)});};
  guardClick($('#allon',sec),async()=>{const d=await j('/api/manage');for(const m of (d.modules||[]))if(m.installable&&m.installed&&!m.running)await j('/api/manage/'+m.seg+'/start',{method:'POST'});refresh()});
  guardClick($('#alloff',sec),async()=>{if(!await confirmDialog(T('manage.modules.confirmAllOff')))return;const d=await j('/api/manage');for(const m of (d.modules||[]))if(m.installable&&m.installed&&m.running)await j('/api/manage/'+m.seg+'/stop',{method:'POST'});refresh()});
  /* 系统增强/实验室（Track 3，2026-09-09；实验室 2026-09-10 加）：CJK 画线吸附/CJK 手写笔迹优化/
     导入md文档可见性都是真开关（写 reading-qol.json，走同一个 /api/enhance/qol）。battop 拆两处：
     「实验室」卡片只留开关+说明（mountBattopToggleCard），详细数据挪到本函数下面新增的第 5 个
     二级 tab「电池刺客」（renderBattopDetail）——这个 tab 本身「运行才出现」，规则/实现都照抄
     「笔记」tab「导入 md 文档」子标签那套 hidden 属性+点走再隐藏的写法（见 renderNotes 里
     syncImportVisible 的注释，这里不重复讲一遍）。 */
  const hlBox=$('#erHlSnap',sec),hwBox=$('#labHwStroke',sec),importMdBox=$('#labImportMd',sec);
  const battopToggleRefresh=mountBattopToggleCard($('#labBattopCard',sec));
  const manageNav=sec.querySelector(':scope > .subnav');
  const battopNavBtn=manageNav.children[3],battopPanel=$('#battopDetail',sec);
  renderBattopDetail(battopPanel);
  const erRefresh=async()=>{const r=await j('/api/enhance/status');if(r.ok===false)return;
    hlBox.checked=!!r.hlSnapCjk;
    hwBox.checked=!!r.hwStrokeEnabled;
    importMdBox.checked=!!r.notesImportMdEnabled;
    await battopToggleRefresh();
    const running=!!(r.battop&&r.battop.running);
    if(!running&&battopNavBtn.classList.contains('on'))manageNav.children[0].click();
    battopNavBtn.hidden=!running;battopPanel.hidden=!running;
    if(running&&battopPanel.refresh)battopPanel.refresh()};
  hlBox.onchange=async()=>{const want=hlBox.checked;hlBox.disabled=true;
    const r=await j('/api/enhance/qol',{method:'PUT',body:JSON.stringify({hlSnapCjk:want})});
    hlBox.disabled=false;if(r.ok===false){toast(r.message||T('common.saveFailed'));hlBox.checked=!want}};
  hwBox.onchange=async()=>{const want=hwBox.checked;hwBox.disabled=true;
    const r=await j('/api/enhance/qol',{method:'PUT',body:JSON.stringify({hwStrokeEnabled:want})});
    hwBox.disabled=false;if(r.ok===false){toast(r.message||T('common.saveFailed'));hwBox.checked=!want}};
  importMdBox.onchange=async()=>{const want=importMdBox.checked;importMdBox.disabled=true;
    const r=await j('/api/enhance/qol',{method:'PUT',body:JSON.stringify({notesImportMdEnabled:want})});
    importMdBox.disabled=false;if(r.ok===false){toast(r.message||T('common.saveFailed'));importMdBox.checked=!want}};
  refresh();erRefresh();sec.refresh=()=>{refresh();mvRefresh();mtRefresh();erRefresh()};subtabs(sec);}

(async()=>{
  // 语言包先拿到手：下面 addTab 用得到 T()，晚拿会让顶层导航先短暂显示 key 本身再跳成文字。
  // 拿不到（离线/服务重启中）静默留空对象——T() 兜底显示 key，不是白屏，也不阻塞页面其余部分。
  const lang=currentLang();
  try{I18N=await(await fetch(`/ui/locales/${lang}.json`)).json()}catch{I18N={}}
  document.title=T('app.title');$('#applogo').textContent=T('app.title');
  $('#navpw').textContent=T('nav.changePassword');$('#navca').textContent=T('nav.caCert');$('#logout').textContent=T('nav.signOut');
  $('#mainloading').textContent=T('main.loading');
  const langsel=$('#langsel');langsel.value=lang;
  langsel.onchange=()=>{LS.set('lang',langsel.value);location.reload()};

  const d=await j('/api/services');
  // service→seg：跟 gateway/src/manage.rs::MODULES 保持同一份事实源，不再在前端手搓映射（拿不到就退回
  // 用服务名本身当 area，跟下面两处 `AREA[s.name]||s.name` 的 fallback 语义一致，不阻塞页面渲染）。
  let AREA={};
  try{AREA=Object.fromEntries((await j('/api/manage')).modules.map(m=>[m.service,m.seg]))}catch{}
  const svcs=(d.services||[]).filter(s=>s.ui&&TABS[s.name]).sort((a,b)=>a.ui.order-b.ui.order);
  /* 首层标签顺序（2026-09-10 用户重排）：传书 / 笔记 / 其他 / 管理。笔记单独占位，xochitl(font-serve)/
     KOReader(koreader-serve)/壁纸(wallpaper-serve)——目前 svcs 里唯三除笔记外还带 ui.order 的候选——
     一律降一级包进「其他」（见 renderOther）。 */
  const noteSvc=svcs.filter(s=>s.name==='note-serve');
  const otherSvcs=svcs.filter(s=>s.name!=='note-serve');
  $('#hdr').textContent=location.host;
  const nav=$('#tabs'),main=$('#main');main.innerHTML='';
  const secByArea={};const dirty=new Set();
  const addTab=(title,render,first,area)=>{const b=document.createElement('button');b.textContent=title;const sec=document.createElement('section');sec.area=area;secByArea[area]=sec;
    b.onclick=()=>{[...nav.children].forEach(x=>x.classList.remove('on'));[...main.children].forEach(x=>x.classList.remove('on'));b.classList.add('on');sec.classList.add('on');dirty.delete(area);if(sec.refresh)sec.refresh()};
    nav.appendChild(b);main.appendChild(sec);render(sec);if(first)b.onclick();return sec};
  addTab(T('tab.transfer'),renderTransfer,true,'books');          // 总入口（入库｜母版库），固定第一位（book-serve 不在时列表里提示去管理页开）
  noteSvc.forEach((s)=>addTab(TABS[s.name].titleKey?T(TABS[s.name].titleKey):TABS[s.name].title,TABS[s.name].render,false,AREA[s.name]||s.name));
  if(otherSvcs.length){
    const otherSec=addTab(T('tab.other'),(sec)=>renderOther(sec,otherSvcs),false,'other');
    // fonts/koreader/wallpapers 各自的 SSE 事件原来路由到各自独立顶层 section，现在都嵌进了同一个
    // 「其他」section——三个 area 名都指向同一个 otherSec，事件到了随便哪个都触发它的合并 refresh
    // （renderOther 里 sec.refresh 会把三块子面板一起刷一遍，不逐个精确匹配，简单可靠）。
    otherSvcs.forEach(s=>{secByArea[AREA[s.name]||s.name]=otherSec});
  }
  addTab(T('tab.manage'),renderManage,false,'manage');            // 固定管理台，始终可进
  /* 事件推送（SSE，零轮询）：服务在变更处发事件 → 网关 /api/events 汇聚 → 这里只刷对应 tab；不在前台的 tab 记脏，切过去时刷。
     manage 事件（服务启停）：tab 集合变了就整页重载，否则只刷管理台。断线（WiFi 掉/设备休眠醒来）EventSource 自动重连。 */
  const svcKey=svcs.map(s=>s.name).join(',');
  const dot=document.createElement('span');dot.id='live';dot.title=T('common.eventStream');dot.textContent='●';dot.style.cssText='margin-left:.5em;font-size:.8em;color:var(--bad)';$('#hdr').appendChild(dot);
  const es=new EventSource('/api/events');
  es.onopen=()=>{dot.style.color='var(--ok)';dot.title=T('common.eventStreamConnected')};
  es.onerror=()=>{dot.style.color='var(--bad)';dot.title=T('common.eventStreamReconnecting')};
  es.onmessage=async(e)=>{let ev;try{ev=JSON.parse(e.data)}catch{return}
    if(ev.area==='manage'){const d=await j('/api/services');const k=(d.services||[]).filter(s=>s.ui&&TABS[s.name]).map(s=>s.name).join(',');if(k!==svcKey){location.reload();return}}
    const sec=secByArea[ev.area];if(!sec)return;
    if(sec.classList.contains('on')){if(sec.refresh)sec.refresh()}else dirty.add(ev.area)};
})();

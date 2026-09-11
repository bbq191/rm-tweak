const $=(s,r=document)=>r.querySelector(s);
/* HTML 转义：**所有外部数据**（文件名、字体内部名、书里的划线/手写转写文本、AI 回答、服务端错误文案）插进
   innerHTML/insertAdjacentHTML/属性值之前必须过它。文件名允许含 `<`（rmsvc_core::fs::plain_name 只拒 `/` `\` 和
   开头的 `.`），抓取的网文标题、字体 name 表、OCR/大模型输出也都是外部内容——不转义就是存储型 XSS。
   textContent/el({text}) 天然安全，不需要它。 */
const esc=s=>String(s==null?'':s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const fmtB=n=>n>1048576?(n/1048576).toFixed(1)+' MB':n>1024?(n/1024).toFixed(0)+' KB':n+' B';
// 小徽章：renderManage 的「基石与模块」列表用。
const badge=(t,ok)=>`<span class="badge ${ok?'on':'off'}">${t}</span>`;
/* 母版库一本书是不是"已经不用管了"——给 stagingList 的"隐藏已完成"开关用（2026-09-19 用户反馈
   母版库列表太长）。正在处理/失败态都不算"完成"（还需要用户看见），格式不是 EPUB 就没有"优化"
   这个概念、只看有没有落库；EPUB 要优化完+落库才算。 */
const isBookDone=it=>{
  if(it.busy)return false;
  const dv=it.delivered||{};
  if((dv.optimize&&dv.optimize.status==='failed')||(dv.deliver&&dv.deliver.status==='failed'))return false;
  const optimizedOk=it.format!=='epub'||it.optimized;
  const deliveredOk=!!(dv.native||dv.koreader);
  return optimizedOk&&deliveredOk;
};
/* 停一会儿再继续：用在"先弹出一条状态文字，再触发会重画掉这条文字的动作"这种场景——不等的话状态
   文字刚显示就被紧跟着的重画冲掉，用户根本来不及看见（点重转/生成笔记本弹出消耗那次踩过的坑）。 */
const wait=ms=>new Promise(res=>setTimeout(res,ms));
/* 合并并发刷新：包一个异步刷新函数，同一时刻最多一个在飞；跑的时候又被调用只记一个"跑完再来一次"，多次调用合成
   一次。SSE 事件突发、切 tab、别的触发源叠在一起时不会并发出一堆请求，旧响应也不会盖过新响应（串行执行，
   结果按顺序落地）。返回的 Promise 在"包含本次调用之后的那一轮"结束时完成。 */
const coalesce=fn=>{let running=null,again=false;
  const loop=async()=>{do{again=false;try{await fn()}catch(e){console.error(e)}}while(again)};
  return()=>{if(running){again=true;return running}running=loop().finally(()=>{running=null});return running}};
/* 一个区块（顶层 tab 或「其他」里的子面板）的刷新统一走它（切 tab、SSE、可见性恢复）：按区块各自 coalesce，合并并发触发。 */
const refreshSec=sec=>{if(!sec.refresh)return;(sec._rf||(sec._rf=coalesce(async()=>{await sec.refresh()})))()};
/* 防双击：按钮点击后立即禁用，异步操作完成（不管成功失败）再解禁。很多按钮的异步操作是删除/
   落库这类不该被同一次操作重复触发两遍的动作——不加这一层，手指点快了或者网络慢的时候网络请求
   还没回来就能再点一次，2026-09-18 真机反馈"优化过程中点击删除"这类并发操作会撞在一起。母版库
   列表 stagingList 的 btn() 助手（2026-09-19 起）也走这个——原来自己手写了一遍禁用/复位逻辑，
   跟这里是同一件事的第二份实现，改成在这层之外只叠列表特有的按钮文案/本地忙态记账。已经自己
   一开始就手动 disabled=true 的按钮（超限/未安装这类"根本点不了"，不是"点了在跑"）不需要套这层。*/
const guardClick=(el,fn)=>{el.onclick=async()=>{if(el.disabled)return;el.disabled=true;try{await fn()}catch(e){console.error(e);toast(T('common.failed'))}finally{el.disabled=false}}};
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
const toast=(msg,kind='bad',ms=4200)=>{if(!msg)return;const t=el('div',{class:'toast '+kind,text:msg});toastHost.appendChild(t);
  requestAnimationFrame(()=>t.classList.add('show'));
  const kill=()=>{t.classList.remove('show');setTimeout(()=>t.remove(),200)};
  t.onclick=kill;setTimeout(kill,ms)};
/* 三种对话框（确认 / 输入 / 单选）的公共骨架：遮罩 + 盒子 + 关闭收尾（摘掉键盘监听、移除节点、兑现 Promise）。
   点遮罩 / Esc = 取消（cancelValue）；onKey 处理其余按键（Enter 等）。build(close) 返回盒子里的节点数组与要聚焦的元素。 */
const modal=(cancelValue,build,onKey)=>new Promise(resolve=>{
  const close=v=>{document.removeEventListener('keydown',key);overlay.remove();resolve(v)};
  const {nodes,focus}=build(close);
  const overlay=el('div',{class:'confirm-overlay'},[el('div',{class:'confirm-box'},nodes)]);
  const key=e=>{if(e.key==='Escape')close(cancelValue);else if(onKey)onKey(e,close)};
  overlay.onclick=e=>{if(e.target===overlay)close(cancelValue)};
  document.addEventListener('keydown',key);
  document.body.appendChild(overlay);
  if(focus)focus();
});
/* 自定义确认框：浏览器原生 confirm() 跟已经禁掉的 alert() 是同一类问题——阻塞整个页面、样式跟
   站内其它地方完全脱节，全站原来散落的 9 处 confirm() 统一改走这个（2026-09-19 用户反馈"母版库
   删除确认还是 alert"——严格说原来用的是 confirm() 不是 alert()，但对用户来说是同一类"浏览器
   弹出个原生对话框"体验，touch 一次改到底，不分是 alert 还是 confirm）。返回 `Promise<boolean>`，
   调用方需要 `await`（跟原来 `if(confirm(msg))` 同步调用不一样，全部改成
   `if(await confirmDialog(msg))`）；点"是"/`Enter`/取消按钮外没有对应处理，点遮罩/`Esc`/"否"
   都算取消，跟原生 confirm() 的"确定/取消"行为对齐。 */
const confirmDialog=msg=>modal(false,close=>{
  const yesBtn=el('button',{class:'btn pri',text:T('common.yes')});yesBtn.onclick=()=>close(true);
  const noBtn=el('button',{class:'btn',text:T('common.no')});noBtn.onclick=()=>close(false);
  return {nodes:[el('div',{class:'confirm-msg',text:msg}),el('div',{class:'confirm-actions'},[noBtn,yesBtn])],focus:()=>yesBtn.focus()};
},(e,close)=>{if(e.key==='Enter')close(true)});
/* 输入框版的 confirmDialog：返回 `Promise<string|null>`（取消 = null）。给改名这类"要用户敲一个值"的操作用。 */
const promptDialog=(msg,value='')=>{const inp=el('input',{type:'text',class:'confirm-input'});inp.value=value;
  return modal(null,close=>{
    const yesBtn=el('button',{class:'btn pri',text:T('common.ok')});yesBtn.onclick=()=>close(inp.value);
    const noBtn=el('button',{class:'btn',text:T('common.cancel')});noBtn.onclick=()=>close(null);
    return {nodes:[el('div',{class:'confirm-msg',text:msg}),inp,el('div',{class:'confirm-actions'},[noBtn,yesBtn])],focus:()=>{inp.focus();inp.select()}};
  },(e,close)=>{if(e.key==='Enter')close(inp.value)})};
/* 单选版的 confirmDialog：一排选项按钮（当前值高亮），点一个就返回它的 value，取消/遮罩/Esc = null。
   给"阅读方向"这类三选一的设置用（2026-09-25）。opts: [{value,label}]；note: 选项下方的一行小字说明。 */
const choiceDialog=(msg,opts,current,note='')=>modal(null,close=>{
  const noBtn=el('button',{class:'btn',text:T('common.cancel')});noBtn.onclick=()=>close(null);
  const pick=el('div',{class:'choice-opts'},opts.map(o=>{const b=el('button',{class:'btn'+(o.value===current?' pri':''),type:'button',text:o.label});b.onclick=()=>close(o.value);return b}));
  const nodes=[el('div',{class:'confirm-msg',text:msg}),pick];if(note)nodes.push(el('div',{class:'small choice-note',text:note}));nodes.push(el('div',{class:'confirm-actions'},[noBtn]));
  return {nodes,focus:()=>(pick.querySelector('.pri')||pick.firstChild).focus()};
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
/* 标签栏吸顶位置跟着页头实际高度（手机英文界面页头会折两行），见 style.css nav 的 --hdr-h。只在尺寸变化时回调，不轮询。 */
if(window.ResizeObserver)new ResizeObserver(([e])=>document.documentElement.style.setProperty('--hdr-h',Math.ceil(e.target.getBoundingClientRect().height)+'px')).observe($('header'));
$('#logout').onclick=e=>{e.preventDefault();fetch('/logout',{method:'POST'}).then(()=>location.href='/login')};
// 徽章的完整解释（渲染自检失败原因、优化档位差异…）以前只写进 title——触屏摸不到 hover，只看得见
// 图标+数字，看不见"为什么/该怎么办"（2026-09-09 审计发现）。这里全局委托一个点击处理：任何带
// title 的徽章点一下就 alert 出完整内容，不用逐个改模板字符串；desktop 上点了也只是多一次确认，
// 不冲突。`.badge[title]` 的 `cursor` 在 style.css 里配套改成 help，给一个"这能点"的视觉提示。
document.addEventListener('click',e=>{const b=e.target.closest('.badge[title]');if(b&&b.title)toast(b.title,'info',6500)});
// 响应不是合法 JSON（网关自身 502/504、反代错误页…）时，以前直接把裸状态码当 message 弹给用户
// （"HTTP 502"），技术术语没翻译成人话（2026-09-09 审计发现）。改成一句人话+状态码放在括号里，
// 报障时还能带出这个号。
// 网络层异常（设备休眠、WiFi 断开、网关重启中）fetch 直接抛 TypeError——此前没接住，开关一直灰着、按钮没反应、
// 用户看不到任何提示（2026-09-24 审查）。统一在这里转成 {ok:false,message}，调用方照常按失败处理。
async function j(url,opt){let r;try{r=await fetch(url,opt)}catch(e){console.error(e);return {ok:false,message:T('common.networkError')}}if(r.status===401){location.href='/login?next='+encodeURIComponent(location.pathname);return {ok:false,message:T('common.needLogin')}}if(r.status===403){location.href='/password';return {ok:false,message:T('common.needChangePassword')}}
  const httpErr=T('common.httpErr',{status:r.status});
  let d;try{d=await r.json()}catch{d={ok:false,message:httpErr}}if(!r.ok&&d.ok!==false)d={ok:false,message:d.message||httpErr};return d}
/* 带 JSON body 的请求：`jsend(url,'PUT',{a:1})`——全站 POST/PUT 带 body 的调用共用，不再各处手写 `{method,body:JSON.stringify}`。 */
const jsend=(url,method,body)=>j(url,{method,body:JSON.stringify(body)});
/* 开关复选框绑定 PUT：勾选即 PUT `{key:checked}`，期间禁用；失败弹 toast 并把勾选还原。「系统增强」/「实验室」的开关共用。 */
const bindToggle=(box,url,key)=>{box.onchange=async()=>{const want=box.checked;box.disabled=true;
  let r;try{r=await jsend(url,'PUT',{[key]:want})}catch(e){console.error(e);r={ok:false}}finally{box.disabled=false}
  if(r.ok===false){toast(r.message||T('common.saveFailed'));box.checked=!want}}};
const postJ=async(url,body)=>{const r=await jsend(url,'POST',body);if(r.ok===false)toast(r.message||T('common.failed'));return r};

/* 上传区 HTML（拖放框 + 隐藏 input + 队列 + 按钮），一处生成、各页复用；uploader() 认这个 .up 容器 */
const upHtml=(icon,label,ext,btn)=>`<div class="up"><div class="drop"><span class="big">${icon}</span>${label}</div><input type="file" multiple hidden accept="${ext.join(',')}"><ul class="q"></ul><div class="row"><button class="btn pri go">${btn}</button></div></div>`;

/* 通用上传器：逐文件一请求，进度条，逐项回执；失败项可重传，队列可逐项删/清空，顶部总进度。box=.up 容器 */
function uploader(box,urlOf,queryOf,okExt,onFinish,dedupeApi){
  const list=$('ul.q',box), input=$('input[type=file]',box), drop=$('.drop',box), go=$('.go',box);
  let files=[], sum=null, busy=false;
  const clr=el('button',{type:'button',class:'btn',text:T('common.clear')});clr.onclick=()=>{files=[];render()};go.after(clr);
  const summary=()=>{if(!sum){sum=el('div',{class:'small',style:'margin:.3em 0'});list.parentNode.insertBefore(sum,list)}
    const ok=files.filter(f=>f.st==='ok').length,bad=files.filter(f=>f.st==='bad').length;
    sum.innerHTML=files.length?T('common.uploadSummary',{ok,total:files.length,badPart:bad?T('common.uploadBadPart',{bad}):''}):'';};
  const row=f=>{const li=document.createElement('li');li.dataset.k=f.k;li.className=f.st||'';
      li.innerHTML=`<div class="name">${esc(f.file.name)} <span class="small">${fmtB(f.file.size)}</span> <button class="btn x" type="button" title="${T('common.remove')}" aria-label="${T('common.remove')}">×</button></div><progress value="${f.st==='ok'?100:0}" max="100"></progress><div class="msg">${esc(f.msg||T('common.waitingUpload'))}</div>`;
      const x=li.querySelector('.x');x.disabled=busy;x.onclick=()=>{if(busy)return;files=files.filter(y=>y.k!==f.k);render()};return li};
  const render=()=>{list.innerHTML='';files.forEach(f=>list.appendChild(row(f)));summary()};
  /* 上传进行中不整表重画（删行按钮禁用、新加的文件只追加行）：重画会把正在传的那一项的进度条/状态文字换成新节点，
     上传回调还挂在旧节点上，之后再也不更新。新追加的文件本轮循环会接着传（循环遍历的就是 files 这个数组）。 */
  const add=fl=>{for(const f of fl){const rej=okExt&&!okExt.some(e=>f.name.toLowerCase().endsWith(e));
      const it={file:f,k:Math.random().toString(36).slice(2),rej,st:rej?'bad':'',msg:rej?T('common.rejectedExt',{ext:okExt.join(' / ')}):''};
      files.push(it);if(busy)list.appendChild(row(it))}
    if(busy)summary();else render()};
  input.onchange=()=>{add(input.files);input.value=''};
  drop.ondragover=e=>{e.preventDefault();drop.classList.add('hi')};drop.ondragleave=()=>drop.classList.remove('hi');
  drop.ondrop=e=>{e.preventDefault();drop.classList.remove('hi');add(e.dataTransfer.files)};
  drop.onclick=()=>input.click();
  const lockRows=on=>{busy=on;list.querySelectorAll('.x').forEach(x=>x.disabled=on)};
  go.onclick=async()=>{go.disabled=true;clr.disabled=true;lockRows(true);
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
        // 401/403 与 j() 同一处理（登录过期 → 登录页；首登未改密 → 改密页）；非 JSON 应答给人话 + 状态码（原来是裸 "HTTP 502"，不走语言包）。
        x.onload=()=>{if(x.status===401){location.href='/login?next='+encodeURIComponent(location.pathname);return}if(x.status===403){location.href='/password';return}
          let d;try{d=JSON.parse(x.responseText)}catch{d={ok:false,message:T('common.httpErr',{status:x.status})}}
          const it=(d.items&&d.items[0])||d;f.st=it.ok?'ok':'bad';f.msg=(it.message||(it.ok?T('common.done'):T('common.failed')))+(d.note&&it.ok?' · '+d.note:'');li.className=f.st;msg.textContent=f.msg;pg.value=100;summary();
          // 同一批里排了两份同名同大小：这份传完了要马上补进快照，下一份循环到时才躲得开——只查一次
          // 快照、循环里不更新的话，两份会一起溜过去（都不在最初那份快照里），2026-09-13 真机踩到。
          if(existing&&it.ok)existing.add(f.file.name+'|'+f.file.size);
          res()};
        x.onerror=()=>{f.st='bad';f.msg=T('common.networkError');li.className='bad';msg.textContent=f.msg;summary();res()};
        const fd=new FormData();fd.append('file',f.file);x.send(fd)})}
    lockRows(false);go.disabled=false;clr.disabled=false;if(onFinish)onFinish()};
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
  items.forEach(it=>{const li=el('li'),left=el('span'),right=el('span',{class:'small',style:'display:flex;align-items:center;gap:.4em;flex-wrap:wrap'});
    row(it,left,right,li);li.append(left,right);ul.appendChild(li)})}
/* 删除按钮：confirmDialog → DELETE → 刷新 */
function delBtn(msg,url,refresh){const d=el('button',{class:'btn',text:T('action.delete')});
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

/* 母版库列表（2026-09-20 重设计，兼顾手机和 PC）。此前的问题：一整张卡片里堆了文件夹输入框、说明、搜索、两个
   下拉、开关、三条状态提示，每行最多 5 个徽章 + 4 个按钮，下载站的长文件名在手机上折成好几行；批量要逐行勾选、
   由浏览器逐个提交。现在：每行 = 勾选框 + 清爽书名（去掉 `-- 作者 -- hash` 尾巴，完整名在「更多」里）+ 一行徽章 +
   **一个主按钮**（随状态变：待优化→优化；已优化→加入 xochitl；cbz/其它→加入 KOReader）+ `⋯` 菜单（其余操作）。
   批量走服务端队列（网关 `/api/batch`），全选/一键"优化全部待优化"，关掉页面照跑。 */
const stgClean=n=>{const s=n.replace(/\.(epub|pdf|cbz)$/i,'');return (s.split(' -- ')[0]||s).trim()};
/* 搜索框的下拉建议：**书名 = 第一个 "-" 之前的内容**（用户 2026-09-20 指定）。"亂馬1⁄2 典藏版 - 07卷" → "亂馬1⁄2 典藏版"，
   同一本书的多卷合成一条；选中后按名字包含匹配，正好筛出这本书的所有卷。 */
const stgTitle=n=>stgClean(n).split('-')[0].trim();
const stgNameOptions=items=>[...new Set(items.map(it=>stgTitle(it.name)).filter(Boolean))].map(n=>`<option value="${esc(n)}">`).join('');
const stgIsTodo=it=>(it.format==='epub'||it.format==='pdf')&&!it.optimized;
/* 一本书的徽章 HTML + 一条可见的状态文字（失败原因等）。逻辑沿用旧列表：优化档位/PDF 来源/落库记录/渲染自检/忙态。 */
function stgBadges(it,busy){
  const fmt=it.format==='epub'?'EPUB':it.format==='pdf'?'PDF':(it.name.includes('.')?it.name.split('.').pop().toUpperCase():T('transfer.staging.fmtOther'));
  const st=it.format==='pdf'?(it.optimized?`<span class="badge on" title="${T('transfer.staging.badge.comicPdfTitle')}">${T('transfer.staging.badge.comicPdf')}</span>`:`<span class="badge">${T('transfer.staging.badge.asIs')}</span>`)
    :it.format!=='epub'?`<span class="badge">${T('transfer.staging.badge.asIs')}</span>`
    :it.level==='full'?`<span class="badge on">${T('transfer.staging.badge.optimized')}</span>`
    :it.level==='core'?`<span class="badge" title="${T('transfer.staging.badge.optimizedUncleanTitle')}">${T('transfer.staging.badge.optimizedUnclean')}</span>`
    :it.level==='old'?`<span class="badge" title="${T('transfer.staging.badge.oldOptimizedTitle')}">${T('transfer.staging.badge.oldOptimized')}</span>`
    :`<span class="badge">${T('transfer.staging.badge.notOptimized')}</span>`;
  // 按书设置的阅读方向（2026-09-25）：自动不显示；设了就标出来，设置还没写进书里（要再点「优化」）另标一枚。
  const dir=it.format==='epub'&&it.direction&&it.direction!=='auto'?`<span class="badge on" title="${T('stg.dir.badgeTitle')}">${T('stg.dir.'+it.direction)}</span>`+(it.directionStale?`<span class="badge off" title="${T('stg.dir.staleTitle')}">${T('stg.dir.staleBadge')}</span>`:''):'';
  const ps=(it.format==='epub'&&it.pdfSource)?`<span class="badge on" title="${T('transfer.staging.badge.pdfSourceTitle')}">${T('transfer.staging.badge.pdfSource')}</span>`:'';
  const dv=it.delivered||{},stale=t=>t&&it.mtime&&t<it.mtime;
  const dl=(dv.native?`<span class="badge on" title="${stale(dv.native)?T('transfer.staging.delivered.native.staleTitle'):T('transfer.staging.delivered.native.title')}">${T('transfer.staging.delivered.native.badge')}${stale(dv.native)?T('transfer.staging.staleSuffix'):''}</span>`:'')+(dv.koreader?`<span class="badge on" title="${stale(dv.koreader)?T('transfer.staging.delivered.koreader.staleTitle'):T('transfer.staging.delivered.koreader.title')}">${T('transfer.staging.delivered.koreader.badge')}${stale(dv.koreader)?T('transfer.staging.staleSuffix'):''}</span>`:'');
  const rc=dv.render,rb=!rc?'':rc.status==='onopen'?`<span class="badge" title="${T('stg.render.onopenTitle')}">${T('stg.render.onopenBadge')}</span>`:rc.status==='ok'?`<span class="badge on" title="${rc.expected>=20?T('transfer.staging.render.okTitle',{pages:rc.pages,expected:rc.expected}):T('stg.render.okTitle',{pages:rc.pages})}">${T('transfer.staging.render.okBadge',{pages:rc.pages})}</span>`:rc.status==='warn'?`<span class="badge off" title="${T('transfer.staging.render.warnTitle',{pages:rc.pages,expected:rc.expected})}">${T('transfer.staging.render.warnBadge',{pages:rc.pages,expected:rc.expected})}</span>`:rc.status==='pending'?`<span class="badge" title="${T('transfer.staging.render.pendingTitle')}">${T('transfer.staging.render.pendingBadge')}</span>`:`<span class="badge" title="${T('transfer.staging.render.noneTitle')}">${T('transfer.staging.render.noneBadge')}</span>`;
  const oc=dv.optimize,dc=dv.deliver;
  // 卡在 pending 但 busy=false＝上次处理被服务/设备重启打断（2026-09-19 真机撞过），不是"还在跑"。
  const stalePending=k=>k&&k.status==='pending'&&!it.busy;
  const fails=(stalePending(oc)||stalePending(dc)?`<span class="badge off" title="${T('transfer.staging.stalePending.title')}">${T('transfer.staging.stalePending.badge')}</span>`:'')
    +(oc&&oc.status==='failed'?`<span class="badge off" title="${esc(oc.message)}">${T('transfer.staging.optimizeFailed.badge')}</span>`:'')
    +(dc&&dc.status==='failed'?`<span class="badge off" title="${esc(dc.message)}">${T('transfer.staging.deliverFailed.badge')}</span>`:'');
  const msg=dc&&dc.status==='failed'?T('transfer.staging.deliverFailedPrefix')+dc.message
    :oc&&oc.status==='failed'?T('transfer.staging.optimizeFailedPrefix')+oc.message
    :(oc&&oc.status==='cancelled')||(dc&&dc.status==='cancelled')?T('stg.row.cancelled'):(stalePending(oc)||stalePending(dc))?T('transfer.staging.stalePending.title'):'';
  const muted=!(dc&&dc.status==='failed')&&!(oc&&oc.status==='failed')&&((oc&&oc.status==='cancelled')||(dc&&dc.status==='cancelled')); // 仅"上次已取消"这类淡色提示，不再靠正则匹配文案
  return {html:`<span class="badge fmt">${esc(fmt)}</span><span class="stg-size">${fmtB(it.bytes)}</span>${st}${dir}${ps}${dl}${rb}${busy?'':fails}`,msg,muted};
}
/* 一行。ctx: {picked,gatedPending,gatedActive,batchQueued,bs,syncSel(),refresh()} */
function stgRow(it,ctx){
  const dv=it.delivered||{},oc=dv.optimize,dc=dv.deliver;
  const busy=!!it.busy;
  const gatedPending=ctx.gatedPending.has(it.name),gatedActive=ctx.gatedActive.has(it.name);
  const queued=gatedPending||ctx.batchQueued.has(it.name)||(ctx.bs.running&&ctx.bs.current===it.name&&!busy);
  const b=stgBadges(it,busy);
  const cb=el('input',{type:'checkbox','aria-label':it.name});cb.checked=ctx.picked.has(it.name);
  cb.onchange=()=>{if(cb.checked)ctx.picked.add(it.name);else ctx.picked.delete(it.name);li.classList.toggle('sel',cb.checked);ctx.syncSel()};
  const title=el('div',{class:'stg-name',title:it.name,text:stgClean(it.name)});
  const meta=el('div',{class:'stg-meta',html:b.html});
  const main=el('div',{class:'stg-main'},[title,meta]);
  if(b.msg)main.appendChild(el('div',{class:'small stg-err'+(b.muted?' stg-muted':''),text:b.msg}));
  if(busy){
    const dcP=dc&&dc.status==='pending',ocP=oc&&oc.status==='pending';
    const label=dcP?T('transfer.staging.progress.delivering'):ocP?T('transfer.staging.progress.optimizing'):T('transfer.staging.progress.working');
    renderStepProgress(main,{label,prog:(dcP&&dc.progress)||(ocP&&oc.progress)||null,msg:dcP?dc.message:ocP?oc.message:''});
  }else if(queued){
    renderStepProgress(main,{label:ctx.batchQueued.has(it.name)?T('stg.batch.queuedHere'):T('transfer.staging.progress.queued')});
  }
  // 行内按钮（只有停止/取消排队两种）：guardClick 防双击，点完刷新。
  const act=(t,fn)=>{const x=el('button',{class:'btn btn-bad',text:t});
    guardClick(x,async()=>{x.textContent=t+'…';await fn();ctx.refresh()});return x};
  // 列表只显示书名/类型/大小/状态/进度；**所有操作**（优化/加入 xochitl/加入 KOReader/删除/全部中止）由勾选后的底部操作栏统一控制
  // （用户 2026-09-20 明确要求）。行内唯一的按钮：这本书正在处理/排队时的「停止」。
  const doStop=async()=>{const r=await postJ('/api/books/staging/cancel',{name:it.name});if(r.ok!==false)toast(r.message,r.cancelled?'ok':'warn',5000)};
  const actions=el('div',{class:'stg-actions'});
  if(gatedPending&&!busy){
    actions.appendChild(act(T('transfer.staging.btn.cancelQueued'),async()=>{const r=await postJ('/api/budget/cancel',{name:it.name});if(r.ok!==false)toast(r.cancelled?T('transfer.staging.cancelQueuedOk',{name:it.name}):T('transfer.staging.cancelQueuedTooLate',{name:it.name}),r.cancelled?'ok':'warn')}));
  }else if(busy||gatedActive){
    actions.appendChild(act(T('stg.row.stop'),doStop));
  }
  const li=el('li',{class:'stg-row'+(cb.checked?' sel':'')},actions.children.length?[el('label',{class:'stg-check'},[cb]),main,actions]:[el('label',{class:'stg-check'},[cb]),main]);
  return li;
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
  <div class="subpanel" id="stgroot">
    <div class="card stg-head">
      <div class="stg-headrow"><h3 style="margin:0">${T('transfer.staging.title')}</h3><span class="small" id="stgcap"></span></div>
      <div class="stg-dest">
        <div class="stg-destpair">
          <div class="stg-destcol"><label class="small" for="folder">${T('stg.dest.xochitl')}</label><select id="folder"></select></div>
          <div class="stg-destcol"><label class="small" for="kfolder">${T('stg.dest.koreader')}</label><select id="kfolder"></select></div>
        </div>
        <span class="stg-newrow" id="xnew" hidden><input type="text" id="xnewname" placeholder="${T('stg.dest.newPlaceholder')}"><button class="btn pri" id="xnewgo">${T('stg.dest.create')}</button><button class="btn" id="xnewx">${T('stg.dest.cancel')}</button></span>
        <span class="stg-newrow" id="knew" hidden><input type="text" id="knewname" placeholder="${T('stg.dest.newPlaceholder')}"><button class="btn pri" id="knewgo">${T('stg.dest.create')}</button><button class="btn" id="knewx">${T('stg.dest.cancel')}</button></span>
        <details class="cmp"><summary>${T('transfer.staging.optDetailsSummary')}</summary><p class="small">${T('transfer.staging.optNote')}</p></details>
      </div>
      <div class="small" id="stgfree"></div>
      <div class="small" id="stgnotice"></div>
    </div>
    <div class="stg-tools"><input type="text" id="stgq" list="stgnames" autocomplete="off" placeholder="${T('transfer.staging.searchPlaceholder')}" aria-label="${T('transfer.staging.searchAria')}"><datalist id="stgnames"></datalist><select id="stgfmt" aria-label="${T('transfer.staging.fmtFilterAria')}"><option value="">${T('transfer.staging.fmtAll')}</option><option value="epub">EPUB</option><option value="pdf">PDF</option><option value="other">${T('transfer.staging.fmtOther')}</option></select></div>
    <div class="stg-chips" id="stgchips"></div>
    <div class="stg-selrow"><label class="toggle"><input type="checkbox" id="stgall"> <span id="stgalltxt"></span></label><span class="stg-spacer"></span><label class="toggle"><input type="checkbox" id="stghide"> ${T('stg.hideDone')}</label></div>
    <ul class="stg-list" id="stglist"></ul>
    <div class="stg-pager" id="stgpager"></div>
    <details class="card stg-orig" id="stgorig" hidden><summary id="stgorigsum"></summary><p class="small">${T('stg.orig.lead')}</p><ul class="stg-list" id="stgoriglist"></ul></details>
    <div class="stgbar" id="stgbar" hidden></div>
  </div>`;
  let koInstalled=false,items=[];
  const picked=new Set();                                  // 勾选的书名（跨页保留）
  // 服务端批量队列状态（网关 /api/batch/status）：关掉页面重开、换设备都读得到，不再依赖本标签页提交过什么。
  let bs={running:false,total:0,done:0,failed:[],queued:[],current:null,action:null};
  let batchQueued=new Set(),dismissedSig='';
  // 网关并发闸门的服务端真相（排队/处理中），见 budget.rs。
  let gatedPending=new Set(),gatedActive=new Set();
  const g=id=>$('#'+id,sec);
  // 加入位置：下拉（现有文件夹）+「＋新建文件夹」。选中值记在本机；xochitl 新建走 book-serve 的 mkdir 队列（xochitl 里 QML
  // 代理每 8 秒轮询建出来），KOReader 直接建目录。"根目录"= 空串。
  const NEW='__new__';
  const val=id=>{const v=g(id).value;return v===NEW?'':v};
  const xFolder=()=>val('folder'),kFolder=()=>val('kfolder');
  const fillSel=(id,names,lsKey)=>{const sel=g(id),want=LS.get(lsKey,'');const list=[...new Set(names.filter(Boolean))];if(want&&!list.includes(want))list.push(want);
    sel.innerHTML='';sel.appendChild(el('option',{value:'',text:T('stg.dest.root')}));
    list.forEach(n=>sel.appendChild(el('option',{value:n,text:n})));sel.appendChild(el('option',{value:NEW,text:T('stg.dest.new')}));sel.value=want};
  const bindDest=(id,lsKey,newBox,nameInp,goBtn,cancelBtn,create)=>{
    g(id).addEventListener('change',()=>{if(g(id).value===NEW){g(newBox).hidden=false;g(nameInp).focus()}else{g(newBox).hidden=true;LS.set(lsKey,g(id).value)}});
    g(cancelBtn).onclick=()=>{g(newBox).hidden=true;g(nameInp).value='';g(id).value=LS.get(lsKey,'')};
    guardClick(g(goBtn),async()=>{const name=g(nameInp).value.trim();if(!name){toast(T('stg.dest.needName'),'warn');return}
      if(await create(name)){LS.set(lsKey,name);g(newBox).hidden=true;g(nameInp).value='';await refresh()}})};
  bindDest('folder','folder','xnew','xnewname','xnewgo','xnewx',async name=>{
    const r=await postJ('/api/books/mkdir/add',{name});if(r.ok===false)return false;
    toast(T('stg.dest.created',{name}),'info',6000);
    // xochitl 侧由 QML 代理轮询建文件夹（约 8 秒一次），等它真出现再选中，最多 ~24 秒。
    for(let i=0;i<12;i++){await wait(2000);const s=await j('/api/books/status');if((s.xochitlFolders||[]).includes(name))break}
    return true});
  bindDest('kfolder','kfolder','knew','knewname','knewgo','knewx',async name=>{
    const r=await postJ('/api/koreader/books/mkdir',{folder:name});if(r.ok===false)return false;toast(T('stg.dest.createdKo',{name}),'ok');return true});
  // 筛选/分页状态。"隐藏已完成"只在「全部」筛选下生效（选了「已优化」就是想看它们）。
  let st=LS.get('stgSt','all'),hideDone=LS.get('stgHideDone','1')==='1',page=1,pageSize=+LS.get('stgPageSize','25')||25;
  g('stghide').checked=hideDone;g('stghide').onchange=()=>{hideDone=g('stghide').checked;LS.set('stgHideDone',hideDone?'1':'0');page=1;render()};
  const fmtOf=it=>it.format==='cbz'?'other':it.format;
  const filtered=()=>{const q=g('stgq').value.toLowerCase(),f=g('stgfmt').value;
    return items.filter(it=>(!q||it.name.toLowerCase().includes(q))&&(!f||fmtOf(it)===f)&&(st==='todo'?stgIsTodo(it):st==='done'?!!it.optimized:st==='finished'?isBookDone(it):(!hideDone||!isBookDone(it))))};
  const batchTitle=a=>T('stg.batch.'+a);
  const enqueue=async(action,body)=>{const r=await postJ('/api/batch',{action,folder:action==='koreader'?kFolder():xFolder(),...body});
    if(r.ok===false)return;
    toast(r.queued?T('stg.batch.queuedToast',{queued:r.queued,skip:r.skipped?T('stg.batch.skipped',{n:r.skipped}):''}):T('stg.batch.none'),r.queued?'ok':'warn');
    if(r.queued)picked.clear();await refresh()};
  const ctx=()=>({picked,gatedPending,gatedActive,batchQueued,bs,syncSel:()=>{syncSelUi();renderBar()},refresh:()=>refresh()});
  const syncSelUi=()=>{const list=filtered();const n=list.length,all=n>0&&list.every(it=>picked.has(it.name));
    g('stgall').checked=all;g('stgall').indeterminate=!all&&list.some(it=>picked.has(it.name));g('stgalltxt').textContent=T('stg.selectAll',{n})};
  g('stgall').onchange=()=>{const list=filtered();if(g('stgall').checked)list.forEach(it=>picked.add(it.name));else list.forEach(it=>picked.delete(it.name));render()};
  const renderChips=()=>{const chips=g('stgchips');chips.innerHTML='';
    // 四个筛选统一都带数量（数量 = 该筛选下的书本数，与"隐藏已完成"开关无关）。
    const cnt={all:items.length,todo:items.filter(stgIsTodo).length,done:items.filter(it=>!!it.optimized).length,finished:items.filter(isBookDone).length};
    [['all','stg.chip.all'],['todo','stg.chip.todo'],['done','stg.chip.done'],['finished','stg.chip.finished']].forEach(([k,key])=>{
      const b=el('button',{class:'chip'+(st===k?' on':''),type:'button',text:T(key,{n:cnt[k]})});b.onclick=()=>{st=k;LS.set('stgSt',k);page=1;render()};chips.appendChild(b)})};
  const renderPager=(total)=>{const box=g('stgpager');box.innerHTML='';if(total<=0)return;
    const pages=Math.max(1,Math.ceil(total/pageSize));const from=(page-1)*pageSize+1,to=Math.min(total,page*pageSize);
    const go=p=>{page=Math.min(pages,Math.max(1,p));render();g('stglist').scrollIntoView({block:'start'})};
    const prev=el('button',{class:'btn',type:'button',text:'‹ '+T('stg.pager.prev')});prev.disabled=page<=1;prev.onclick=()=>go(page-1);
    const next=el('button',{class:'btn',type:'button',text:T('stg.pager.next')+' ›'});next.disabled=page>=pages;next.onclick=()=>go(page+1);
    box.appendChild(el('div',{class:'small stg-range',text:T('stg.pager.range',{from,to,total})}));
    const nav=el('div',{class:'stg-pgnav'},[prev]);
    // 页码：首页、当前页±1、末页，中间省略——宽屏显示数字，窄屏只留"第 x/y 页"
    const nums=[...new Set([1,page-1,page,page+1,pages].filter(p=>p>=1&&p<=pages))].sort((a,b)=>a-b);
    let last=0;const numbox=el('span',{class:'stg-pgnums'});
    nums.forEach(p=>{if(last&&p-last>1)numbox.appendChild(el('span',{class:'small',text:'…'}));const b=el('button',{class:'btn'+(p===page?' pri':''),type:'button',text:String(p)});b.onclick=()=>go(p);numbox.appendChild(b);last=p});
    nav.appendChild(numbox);nav.appendChild(el('span',{class:'small stg-pgtxt',text:T('stg.pager.page',{cur:page,total:pages})}));nav.appendChild(next);box.appendChild(nav);
    const sz=el('select',{'aria-label':T('stg.pager.size',{n:pageSize})});[25,50,100].forEach(n=>{const o=el('option',{value:String(n),text:T('stg.pager.size',{n})});if(n===pageSize)o.selected=true;sz.appendChild(o)});
    sz.onchange=()=>{pageSize=+sz.value;LS.set('stgPageSize',String(pageSize));page=1;render()};box.appendChild(sz)};
  // 底部操作栏：批量运行中＝进度+停止；有勾选＝批量按钮；跑完＝一条结果小结（可收起，失败原因可展开）。
  const renderBar=()=>{const bar=g('stgbar');bar.innerHTML='';
    const sig=`${bs.action}|${bs.total}|${bs.done}|${bs.failed.length}`;
    if(bs.running){
      bar.hidden=false;bar.className='stgbar run';
      const t=batchTitle(bs.action||'optimize');
      bar.appendChild(el('div',{class:'stgbar-main'},[el('b',{text:T('stg.batch.progress',{title:t,done:bs.done,total:bs.total})}),el('span',{class:'small',text:(bs.current?' · '+T('stg.batch.current',{name:stgClean(bs.current)}):'')+(bs.failed.length?' · '+T('stg.batch.failedN',{n:bs.failed.length}):'')})]));
      const p=el('progress');p.max=Math.max(1,bs.total);p.value=bs.done;bar.appendChild(p);
      const stop=el('button',{class:'btn btn-bad',type:'button',text:T('stg.batch.stopAll')});
      guardClick(stop,async()=>{const r=await postJ('/api/batch/stop',{});if(r.ok!==false)toast(T('stg.batch.stopped',{n:r.cleared||0}),'info');await refresh()});bar.appendChild(stop);
    }else if(picked.size){
      bar.hidden=false;bar.className='stgbar sel';
      // 第一行：已选数 + 清除；第二行：批量按钮等宽。按钮上直接标"可处理数"（已优化的再优化、非 EPUB/PDF 加入 xochitl 等会被跳过），
      // 0 本可处理就置灰——不再等点完才提示"跳过了 N 本"。
      const chosen=items.filter(it=>picked.has(it.name));
      const isBook=it=>it.format==='epub'||it.format==='pdf';
      const cnt={optimize:chosen.filter(stgIsTodo).length,deliver:chosen.filter(isBook).length,koreader:koInstalled?chosen.length:0};
      const clr=el('button',{class:'btn',type:'button',text:T('stg.batch.clear')});clr.onclick=()=>{picked.clear();render()};
      bar.appendChild(el('div',{class:'stgbar-top'},[el('b',{text:T('stg.selected',{n:picked.size})}),clr]));
      // 按钮排布（2026-09-24 用户要求手机上不折行）：第二行 = 处理/加入（主操作，等分一行）；第三行 = 单本操作 + 删除。
      // 文案 = 标签 + 数量角标；窄屏去掉"加入"前缀（.lbl-long），一行三个也放得下。
      const lbl=(key,n)=>{const f=document.createDocumentFragment();const t=T(key);const m=t.match(/^(加入 |Add to )(.*)$/);
        if(m){f.appendChild(el('span',{class:'lbl-long',text:m[1]}));f.appendChild(document.createTextNode(m[2]))}else f.appendChild(document.createTextNode(t));
        if(n!=null)f.appendChild(el('span',{class:'cnt',text:String(n)}));return f};
      const mk=(a,pri)=>{const b=el('button',{class:'btn'+(pri?' pri':''),type:'button',title:T('stg.bar.'+a)+'（'+cnt[a]+'）'},[lbl('stg.bar.'+a,cnt[a])]);
        if(!cnt[a]){b.disabled=true;b.title=T('stg.bar.noneApplicable')}else guardClick(b,()=>enqueue(a,{names:[...picked]}));return b};
      const btns=el('div',{class:'stgbar-btns'},[mk('optimize',true),mk('deliver')]);
      if(koInstalled)btns.appendChild(mk('koreader'));
      btns.style.setProperty('--cols',String(btns.children.length));
      const delN=chosen.filter(it=>!it.busy).length;
      const del=el('button',{class:'btn btn-bad',type:'button',title:T('action.delete')+'（'+delN+'）'},[lbl('action.delete',delN)]);
      guardClick(del,async()=>{
        const names=chosen.filter(it=>!it.busy).map(it=>it.name),busyN=chosen.length-names.length;
        if(!names.length){toast(T('stg.bar.noneApplicable'),'warn');return}
        if(!await confirmDialog(T('stg.batch.deleteConfirm',{n:names.length})))return;
        let ok=0;for(const n of names){const r=await jsend('/api/books/staging/delete','POST',{name:n});if(r.ok!==false)ok++}
        toast(T('stg.batch.deleted',{n:ok})+(busyN?T('stg.batch.deleteSkipped',{n:busyN}):''),ok?'ok':'warn');picked.clear();refresh()});
      bar.appendChild(btns);
      const row3=el('div',{class:'stgbar-btns stgbar-sub'});
      // 「阅读方向」（2026-09-25）：选中的 EPUB 可设 自动/从右往左/从左往右，多选批量设；只存设置，要再点「优化」才写进书里。
      const epubs=chosen.filter(it=>it.format==='epub');
      const dirBtn=el('button',{class:'btn',type:'button',title:T('stg.bar.direction')+'（'+epubs.length+'）'},[lbl('stg.bar.direction',chosen.length>1?epubs.length:null)]);
      if(!epubs.length){dirBtn.disabled=true;dirBtn.title=T('stg.dir.epubOnly')}
      else guardClick(dirBtn,async()=>{
        const cur=epubs.every(it=>(it.direction||'auto')===(epubs[0].direction||'auto'))?(epubs[0].direction||'auto'):null;
        const v=await choiceDialog(epubs.length===1?T('stg.dir.promptOne',{name:stgClean(epubs[0].name)}):T('stg.dir.promptMany',{n:epubs.length}),
          ['auto','rtl','ltr'].map(value=>({value,label:T('stg.dir.'+value)})),cur,T('stg.dir.note'));
        if(v==null)return;
        const r=await postJ('/api/books/staging/direction',{names:epubs.map(it=>it.name),direction:v});
        if(r.ok===false)return;
        const parts=[T('stg.dir.done',{n:r.updated})];
        if(r.stale)parts.push(T('stg.dir.doneStale',{n:r.stale}));
        if(r.synced)parts.push(T('stg.dir.doneSynced',{n:r.synced}));
        (r.failed||[]).forEach(f=>parts.push(stgClean(f.name)+'：'+f.message));
        toast(parts.join('；'),(r.failed||[]).length?'warn':'ok',8000);refresh()});
      // 只选了一本：再给「下载原件」「改名」（都是针对单本的操作，多选时不出现），删除排在同一行最右。
      if(chosen.length===1){const one=chosen[0];
        const dl=el('a',{class:'btn',href:'/api/books/staging/file?name='+encodeURIComponent(one.name),download:one.name,text:T('stg.bar.download')});
        const rn=el('button',{class:'btn',type:'button',text:T('stg.bar.rename')});
        if(one.busy){rn.disabled=true;rn.title=T('stg.bar.busy')}
        else guardClick(rn,async()=>{const ext='.'+one.name.split('.').pop();
          const v=await promptDialog(T('stg.rename.prompt',{ext}),one.name.slice(0,-ext.length));
          if(v==null||!v.trim())return;
          const r=await postJ('/api/books/staging/rename',{name:one.name,newName:v.trim()});
          if(r.ok!==false){picked.clear();picked.add(r.name);toast(T('stg.rename.done',{name:r.name}),'ok')}refresh()});
        row3.append(dl,rn)}
      row3.append(dirBtn,del);row3.style.setProperty('--cols',chosen.length===1?'4':'3');row3.classList.toggle('cols4',chosen.length===1);bar.appendChild(row3);
    }else if(bs.total&&sig!==dismissedSig){
      bar.hidden=false;bar.className='stgbar done';
      const fail=bs.failed.length;
      bar.appendChild(el('div',{class:'stgbar-main'},[el('span',{text:T('stg.batch.finished',{title:batchTitle(bs.action||'optimize'),ok:bs.done-fail,fail})})]));
      if(fail)bar.appendChild(el('details',{class:'small stgbar-fails'},[el('summary',{text:T('stg.batch.failedN',{n:fail})}),el('div',{html:bs.failed.map(f=>`<div>${esc(stgClean(f.name))}：${esc(f.message)}</div>`).join('')})]));
      const x=el('button',{class:'btn',type:'button',text:T('stg.batch.dismiss')});x.onclick=()=>{dismissedSig=sig;renderBar()};bar.appendChild(x);
    }else{bar.hidden=true}};
  const render=()=>{
    const list=filtered();const pages=Math.max(1,Math.ceil(list.length/pageSize));if(page>pages)page=pages;
    const ul=g('stglist');ul.innerHTML='';
    if(!list.length)ul.appendChild(el('li',{class:'small stg-empty',text:items.length?T('transfer.staging.emptyFiltered'):T('transfer.staging.emptyAll')}));
    else{const c=ctx();list.slice((page-1)*pageSize,page*pageSize).forEach(it=>ul.appendChild(stgRow(it,c)))}
    renderChips();renderPager(list.length);syncSelUi();renderBar()};
  // 搜索框每敲一个字符都整表重画（最多 100 行）太浪费：input 防抖 150ms，change（失焦/回车/选中建议）与格式下拉立即生效。
  let qTimer=0;const rerender=()=>{clearTimeout(qTimer);page=1;render()};
  g('stgq').addEventListener('input',()=>{clearTimeout(qTimer);qTimer=setTimeout(rerender,150)});
  g('stgq').addEventListener('change',rerender);
  g('stgfmt').addEventListener('change',rerender);
  const refresh=()=>refreshAt(3);
  // 网关自身的批量队列 / 并发闸门状态（见 batch.rs、budget.rs）。
  const applyQueue=(bg,bt)=>{
    gatedPending=new Set(bg.ok!==false?bg.pending||[]:[]);gatedActive=new Set(bg.ok!==false?bg.active||[]:[]);
    if(bt.ok!==false){bs={running:!!bt.running,action:bt.action,total:bt.total||0,done:bt.done||0,current:bt.current,queued:bt.queued||[],failed:bt.failed||[]};batchQueued=new Set(bs.queued)}
    const gated=(gatedPending.size||gatedActive.size)?T('transfer.staging.gatedSummary',{pending:gatedPending.size,active:gatedActive.size}):'';
    g('stgnotice').textContent=[koInstalled?'':T('transfer.staging.btn.koNotInstalled'),gated].filter(Boolean).join(' · ')};
  /* 按事件决定取多少（不轮询）。三档，数字越大取得越全：
     1 = 网关自己的批量队列 / 并发闸门事件（area=books、不带 svc）：只重取这两个状态（2 个请求）。一轮批量里每本书网关要发 4～5 条。
     2 = book-serve 的 `staging` 事件（入库、忙态开始/结束、优化/落库**进度**——大书处理期间约每秒一条）：只有母版库列表会变，
         再加上面两个状态（3 个请求）；xochitl / KOReader 文件夹列表与 KOReader 安装状态不会因此变化，不重取。
     3 = 其余（book-serve 的 mkdir/trash/inbox 事件、切 tab、重连、操作后主动刷新）：全量 6 个请求。
     所有刷新走同一个 coalesce 串行执行（need 记"下一轮至少要取到哪一档"，取最大），不会出现旧的全量结果盖掉新的排队状态。 */
  let need=0;
  const run=coalesce(async()=>{const lvl=need;need=0;if(!lvl)return;
    if(lvl===1){const [bg,bt]=await Promise.all([j('/api/budget/status'),j('/api/batch/status')]);applyQueue(bg,bt);render();return}
    const full=lvl>=3;
    const [d,bg,bt,s,k,kb]=await Promise.all([j('/api/books/staging'),j('/api/budget/status'),j('/api/batch/status')].concat(full?[j('/api/books/status'),j('/api/koreader/status'),j('/api/koreader/books')]:[]));
    if(full){
      koInstalled=!!(k.ok&&k.installed);
      fillSel('folder',s.ok?s.xochitlFolders||[]:[],'folder');
      fillSel('kfolder',(kb.items||[]).filter(x=>x.kind==='dir').map(x=>x.name),'kfolder');
    }
    applyQueue(bg,bt);
    if(d.ok===false){items=[];render();g('stgcap').textContent='';g('stgfree').textContent='';g('stglist').innerHTML=`<li class="small stg-empty" style="color:var(--bad)">${esc(T('transfer.staging.unavailable',{msg:d.message||T('transfer.staging.notOpen')}))}</li>`;return}
    items=d.items||[];const tot=items.reduce((a,b)=>a+b.bytes,0);g('stgcap').textContent=items.length?T('transfer.staging.capSummary',{count:items.length,size:fmtB(tot)}):'';
    // 清掉选中集合里的幽灵条目（书被改名/删除后旧名字再也选不中也取消不掉）
    const names=new Set(items.map(it=>it.name));for(const n of [...picked])if(!names.has(n))picked.delete(n);
    const fr=d.freeBytes;const low=fr!=null&&fr<300*1048576;g('stgfree').style.color=low?'var(--bad)':'';g('stgfree').textContent=fr!=null?T('transfer.staging.freeSpace',{free:fmtB(fr),lowWarn:low?T('transfer.staging.lowWarn'):''}):'';
    renderOriginals(d.originals||[]);
    g('stgnames').innerHTML=stgNameOptions(items);render()});
  const refreshAt=lvl=>{need=Math.max(need,lvl);return run()};
  // 原 PDF 备份（有文字层 PDF 转 EPUB 后保留 7 天）：恢复回母版库 / 提前删除。
  const renderOriginals=list=>{const box=g('stgorig'),ul=g('stgoriglist');box.hidden=!list.length;if(!list.length)return;
    g('stgorigsum').textContent=T('stg.orig.summary',{n:list.length});ul.innerHTML='';
    list.forEach(o=>{const days=Math.max(0,Math.ceil((o.expiresAt-Date.now()/1000)/86400));
      const restore=el('button',{class:'btn',type:'button',text:T('stg.orig.restore')});
      guardClick(restore,async()=>{const r=await postJ('/api/books/staging/originals/restore',{name:o.name});if(r.ok!==false)toast(T('stg.orig.restored',{name:o.name}),'ok');refresh()});
      const del=el('button',{class:'btn btn-bad',type:'button',text:T('action.delete')});
      guardClick(del,async()=>{if(!await confirmDialog(T('stg.orig.deleteConfirm',{name:o.name})))return;const r=await postJ('/api/books/staging/originals/delete',{name:o.name});if(r.ok!==false)toast(T('stg.orig.deleted'),'ok');refresh()});
      ul.appendChild(el('li',{class:'stg-row'},[el('div',{class:'stg-main'},[el('div',{class:'stg-name',title:o.name,text:o.name}),el('div',{class:'stg-meta small',text:fmtB(o.bytes)+' · '+T('stg.orig.left',{days})})]),el('div',{class:'stg-actions'},[restore,del])]))})};
  uploader($('.up',sec),()=>'/api/books/staging',()=>({}),BOOK_EXT,()=>refresh(),'/api/books/staging');   // 书籍格式原样入库；选中即按 BOOK_EXT 拦；传 dedupeApi 防重传出重复
  const am=g('artmsg'),au=g('arturl'),ag=g('artgo'),ao=g('artopt');
  // 「同步优化」记在本机（per-viewer 便利态，跟 folder/kfolder 那几个一个规矩）；缺省开——网文正文
  // 没有任何 CSS（article.rs 属性白名单本来就不留 class/style），不经优化会在设备上按默认段距渲染出大片
  // 留空（真机反馈），默认帮用户把这一步做了，不想要（比如想快点抓完自己再调）可以关掉。
  ao.checked=LS.get('artopt','1')==='1';ao.onchange=()=>LS.set('artopt',ao.checked?'1':'0');
  ag.onclick=async()=>{const url=au.value.trim();if(!url){am.textContent=T('transfer.fetchArticle.needUrl');return}ag.disabled=true;am.style.color='';am.textContent=T('transfer.fetchArticle.fetching');
    const r=await jsend('/api/books/staging/fetch-article','POST',{url,optimize:ao.checked});ag.disabled=false;
    am.style.color=r.ok===false?'var(--bad)':'var(--ok)';am.textContent=r.ok===false?('✗ '+(r.message||T('transfer.fetchArticle.failed'))):('✓ '+r.message);if(r.ok!==false){au.value='';refresh()}};
  refresh();sec.refresh=refresh;sec.onEvent=ev=>refreshAt(ev.area!=='books'?3:!ev.svc&&(ev.kind==='batch'||ev.kind==='budget')?1:ev.kind==='staging'?2:3);subtabs(sec);}

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
     const fb=$('#fbchain',sec);fb.style.display='';fb.innerHTML=cjk.length?T('assets.fonts.fallbackChain',{chain:cjk.map(it=>`${esc(it.name)} <span class="small">${esc(it.extra.cjkPct)}%</span>`).join(' → ')}):T('assets.fonts.noCjkWarn');
     const fst=await j('/api/fonts/status');const eb=$('#embold',sec);if(fst.ok){eb.checked=!!fst.emboldenCjkFallback;eb.onchange=async()=>{const r=await jsend('/api/fonts/config','PUT',{emboldenCjkFallback:eb.checked});if(r.ok===false){toast(r.message);eb.checked=!eb.checked}}}},
   row:(it,left,right,refresh)=>{const ex=it.extra||{};
     left.innerHTML=`${esc(it.name)}${ex.names&&ex.names.cn&&ex.names.cn!==it.name?' <span class="small">'+esc(ex.names.cn)+'</span>':''}${ex.files&&ex.files.length>1?' <span class="small">×'+ex.files.length+'</span>':''}`;
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
    $('#ks',sec).innerHTML=s.ok?`<b>${T('koreader.status.installed')}</b><span>${s.installed?T('common.yes'):T('common.no')} ${s.version?'('+esc(s.version)+')':''}</span><b>${T('koreader.status.running')}</b><span>${s.running?T('koreader.status.runningYes'):T('common.no')}</span><b>${T('koreader.status.installedCount')}</b><span>${T('koreader.status.countLabel',{fonts:s.fonts,dicts:s.dicts||0})}</span>`:`<span>${esc(s.message)}</span>`;
    fillList($('#kf',sec),f.items||[],(it,left,right)=>{left.textContent=it.name;right.insertAdjacentHTML('beforeend',cjkBadge(it.cjkPct)+`<span>${fmtB(it.bytes)}</span>`);right.appendChild(delBtn(T('koreader.fonts.deleteConfirm',{name:it.name}),'/api/koreader/fonts/'+encodeURIComponent(it.name),refresh))},T('koreader.fonts.emptyHint'));
    fillList($('#kd',sec),dc.items||[],(it,left,right)=>{left.textContent='📖 '+it.name;right.textContent=T('koreader.dicts.countSuffix',{count:it.ifo})},T('koreader.dicts.emptyHint'))};
  refresh();sec.refresh=refresh;subtabs(sec)}},
 'wallpaper-serve':{titleKey:'tab.wallpaper',title:'壁纸',render(sec){assetTab(sec,'/api/wallpapers',{
   hint:T('wallpaper.hint'),
   header:`<label class="field">${T('wallpaper.rotateLabel')}</label><div class="row"><select id="wpmode" style="max-width:12em"><option value="sequential">${T('wallpaper.mode.sequential')}</option><option value="random">${T('wallpaper.mode.random')}</option><option value="fixed">${T('wallpaper.mode.fixed')}</option></select><span id="wpst" class="small"></span></div>`,
   icon:'🖼',label:T('wallpaper.dropLabel'),accept:IMG_EXT,btn:T('wallpaper.btn'),
   onRender:async(sec,refresh)=>{const st=await j('/api/wallpapers/status');const sel=$('#wpmode',sec);if(st.ok){sel.value=st.mode;const nv=st.native||{};$('#wpst',sec).textContent=T('wallpaper.status',{current:st.current||T('wallpaper.none'),nativeState:nv.enabled?T('wallpaper.nativeEnabled'):T('wallpaper.nativeDisabled'),restartNote:nv.restartPending?T('wallpaper.restartNote'):''})}
     sel.onchange=async()=>{const r=await jsend('/api/wallpapers/mode','PUT',{mode:sel.value});if(r.ok===false){toast(r.message||T('wallpaper.switchFailed'));return}refresh()}},
   row:(it,left,right,refresh)=>{const cur=(it.extra||{}).current;
     // alt="" 原来把这张图当装饰性处理，但壁纸缩略图本身就是内容（"这张壁纸长什么样"），屏幕阅读器
     // 会整个跳过（2026-09-09 审计发现）；文件名本身当描述最直接，跟右边视觉上显示的文字一致。
     left.style.cssText='display:flex;align-items:center;gap:.6em'; // 缩略图固定在左、长文件名在右侧自己折行，不绕着图片流
     left.innerHTML=`<img src="/api/wallpapers/${encodeURIComponent(it.name)}" alt="${esc(T('wallpaper.thumbAlt',{name:it.name}))}" loading="lazy" style="height:3.4em;flex:none;border-radius:.3em;border:1px solid var(--line)"><span style="min-width:0">${esc(it.name)}</span>`;
     right.insertAdjacentHTML('beforeend',`<span>${fmtB(it.bytes)}</span>`+(cur?`<span class="badge on">${T('wallpaper.current')}</span>`:''));
     if(!cur){const b=el('button',{class:'btn',text:T('wallpaper.use')});guardClick(b,async()=>{const r=await jsend('/api/wallpapers/current','PUT',{name:it.name});if(r.ok===false){toast(r.message||T('wallpaper.setFailed'));return}refresh()});right.appendChild(b);
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
function renderOther(sec,svcs,areaOf){
  const items=[{name:'font-serve',icon:'🔤',label:'xochitl'},{name:'koreader-serve',icon:'📖',label:'KOReader'},{name:'wallpaper-serve',icon:'🖼️',label:T('tab.wallpaper')}]
    .filter(it=>svcs.some(s=>s.name===it.name));
  sec.innerHTML=`<div class="subnav">${items.map((it,i)=>`<button${i===0?' class="on"':''}>${it.icon} ${it.label}</button>`).join('')}</div>
    ${items.map((it,i)=>`<div class="subpanel${i===0?' on':''}" id="other-${it.name}"></div>`).join('')}`;
  items.forEach(it=>TABS[it.name].render($('#other-'+it.name,sec)));
  const pane=it=>$('#other-'+it.name,sec);
  sec.refresh=()=>Promise.all(items.map(it=>{const c=pane(it);return c&&c.refresh&&c.refresh()}));
  /* 事件只刷发事件的那个服务的子面板（字体/KOReader/壁纸各自 2～3 个请求），不再三块一起重取——壁纸每次休眠轮换、
     KOReader 每次加书都会发事件。认不出来源（没有映射）时退回整块刷新。 */
  sec.onEvent=ev=>{const it=items.find(x=>areaOf(x.name)===ev.area);const c=it&&pane(it);if(c&&c.refresh)refreshSec(c);else refreshSec(sec)};
  subtabs(sec);
}

/* 「笔记」tab（note-serve 注册；数据来自 ink-serve 条目库）：按书→按章列条目，左裁图右文本，改即存。
   设备只负责写、不负责改：这里就是"改"的地方（e-ink 上打字太痛苦）。三期（2026-09-08）砍掉了"分区"——
   AI 触发早就是按条目单发（勾「问AI」+ 填问题+点提问，调 mind-serve 拼"书名+章节+勾画原文+转写文本+问题"
   发模型，二期，白皮书 §03n），分区兼职的笔记本排版分组也不要了，条目一律按页序平铺，格式=Entry.style。 */
// 顶层常量只放 i18n key 名，不放翻译好的文字——真正的 T() 查找挪到调用点（渲染时执行），见 T() 头注的硬性规则。
const STYLE_NAMES={body:'notes.style.body',bullet:'notes.style.bullet',numbered:'notes.style.numbered',checkbox:'notes.style.checkbox',heading1:'notes.style.heading1',heading2:'notes.style.heading2'};
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
    <div class="row"><span class="small">${T('notes.bookLabel')}</span><select id="nbook" style="flex:1;min-width:10em"></select><button class="btn" id="nrescan" title="${T('notes.rescanTitle')}">${T('notes.rescanBtn')}</button><button class="btn" id="nkoimport" title="${T('notes.koreaderImportTitle')}">${T('notes.koreaderImportBtn')}</button></div>
    <div class="row small" id="nsum"></div>
    <div class="row"><input type="search" id="nq" placeholder="${T('notes.search.placeholder')}" aria-label="${T('notes.search.placeholder')}" style="flex:1;min-width:10em"><button class="btn" id="nqgo">${T('notes.search.btn')}</button></div>
    <div id="nqres"></div>
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
  // 当前书的后端路径：`bookApi('ink')` → `/api/ink/books/<uuid>`，`bookApi('ink',`/entries/${id}`)` 带后缀（读 `book`，调用时求值）。
  const bookApi=(svc,sub='')=>`/api/${svc}/books/${encodeURIComponent(book.uuid)}${sub}`;
  const sel=$('#nbook',sec),chaptertabs=$('#nchaptertabs',sec),chapterbody=$('#nchapterbody',sec),browse=$('#nbrowse',sec),sum=$('#nsum',sec);let book=null;
  // 「推送本章」/「重新转写」/「提问」点完显示结果文案、停留 3s 再让用户看清（见下面三处 wait(3000)）——
  // 但这三个动作本身会让 ink-serve 发 `entries` 事件，笔记 tab 正开着时 SSE 会立刻调 `sec.refresh`
  // 整段重画，比 3s 计时器快得多，文案实际上一闪就被这个"我以为没关系的"刷新冲掉了（真机反馈"重复
  // 推送的提示看不清，一闪而过"，2026-09-17；上一轮把 1.5s 延到 3s 完全没解决，根子根本不在计时器
  // 长短）。这里挡一下：显示文案的同时记一个"暂停到几点"的时间戳，`refresh()` 起手先看这个时间戳，
  // 没过就直接跳过这次 SSE 触发的重画——不会漏刷新，三处调用点末尾自己的 `wait(3000)` 之后本来就会
  // 主动重画一次，只是不再被 SSE 抢跑。
  let holdRefreshUntil=0;
  const holdFor=ms=>{holdRefreshUntil=ms?Date.now()+ms:0};
  /* 结果文案停留 3s（这期间 SSE 触发的重画被 holdRefreshUntil 挡住），再执行 fn（通常是拉新数据重画）。三处「点完显示结果」共用。 */
  const lingerThen=async fn=>{holdFor(3000);await wait(3000);await fn()};
  const cropUrl=(uuid,f)=>`/api/ink/books/${encodeURIComponent(uuid)}/crops/${encodeURIComponent(f)}`;
  // 2026-09-16 截图走查发现：`e.ink` 有值但 `e.ink.crop` 是空串（ink-serve 自渲染裁图失败/写盘失败时
  // 会发生，见 ingest.rs 的 render_ink/write_atomic 错误分支，只记服务端日志、条目照常落盘）此前被
  // cropHtml 误判成"纯勾画没有手写"（notes.noCrop），实际上这条明明有手写，只是裁图暂时没生成——
  // 两种情况分开提示，别让用户误以为手写没被识别到。
  const cropHtml=e=>e.ink&&e.ink.crop?`<img src="${cropUrl(book.uuid,e.ink.crop)}" alt="${T('notes.cropAlt')}">`:`<div class="empty">${T(e.ink?'notes.cropMissing':'notes.noCrop')}</div>`;
  // 在途的保存请求：重取数据前先等它们落地（失焦保存 `onchange` 不 await，紧跟着的重画可能先拿到旧文本）。
  // 用 jsend 不用 postJ：postJ 失败时自己弹一次 toast，这里再弹"保存失败"就成了两条（此前如此）。
  const inflight=new Set();
  const patch=(id,body)=>{const p=jsend(bookApi('ink',`/entries/${encodeURIComponent(id)}`),'POST',body).then(r=>{if(r.ok===false)toast(r.message||T('notes.saveFailed'))}).finally(()=>inflight.delete(p));inflight.add(p);return p};
  /* 编辑区文本失焦才存（`onchange`），但点旁边的按钮（重转/去处/问AI…）会先让文本框失焦触发保存，
     两件事几乎同时各发一个 HTTP 请求，谁先到服务端不一定——按钮那次的收尾动作会拉新数据整页重画，
     如果保存请求还没落地，重画拿到的还是旧文本，编辑就跟着"消失"了（用户反馈"改了内容点重转不存"）。
     用一个 pendingText 记住"还没确认存上"的最新值，任何会拉新数据重画的动作之前先 flush 一遍，
     保证读到的一定是最新的。 */
  const pendingText=new Map();
  const flushPendingText=async()=>{if(book&&pendingText.size){const items=[...pendingText];pendingText.clear();for(const[id,val]of items)await patch(id,{text:val})}
    if(inflight.size)await Promise.all([...inflight])};
  /* 每章"设备笔记本/Obsidian md 是不是已经跟当前条目内容同步"（整理区第三轮反馈）：一次性取整本书
     的同步状态，章头徽章、「整理」列表默认收起已同步章节、回收站显示这条大概去哪了，三处共用同一份，
     不用各自发请求。`refreshSync()` 在 loadBook 里、以及每次生成/导出动作之后调用刷新。 */
  let syncMap=new Map();
  const refreshSync=async()=>{if(!book){syncMap=new Map();return}const r=await j(bookApi('notes',`/sync`));syncMap=new Map((r.chapters||[]).map(c=>[c.chapter,c]))};
  /* 「保存并刷新」这条 5 步链（flush 未落地的改字 → 重取整本书 → 重取同步状态 → 重画指定的几个
     子视图）原来在 triage/archiveEntry/restore/去处切换/转写/问 AI 七处各自逐字重复（2026-09-09
     审计发现），任何一处漏改都容易造成"某个动作之后画面没更新"这类不容易被发现的 bug——收成一个
     辅助函数，调用方只需要说清楚"这次要重画哪几个子视图"。 */
  const reloadBook=async(...views)=>{await flushPendingText();book=await j(bookApi('ink'));await refreshSync();views.forEach(fn=>fn())};
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
  const trashedEntries=()=>(book.entries||[]).filter(e=>TRASH_STATUSES.includes(e.status));
  /* 回收站（点 3）：不是一个只会清空的黑盒按钮——列出「不需要」「不要了」「已撤销」的条目实际内容，
     清空前能看清要丢的是什么。数据不用额外接口：GET /books/{uuid} 本来就带全部条目（含终态的）。 */
  /* 「恢复」（回收站点 3）：Skipped/Revoked/Archived 都能恢复，落点由服务端按条目已有内容倒推
     （见 notecore::model::Entry::restore）——书里已经把笔画擦了也能恢复，找回的是条目库里已经存好
     的裁图/校对文本，不代表设备原页面的笔迹会重新出现（这条限制在页面文案里说清楚，不是网页能力）。 */
  const restoreOne=async id=>{const r=await postJ(bookApi('ink',`/entries/${encodeURIComponent(id)}/restore`),{});if(r.ok===false)return false;return true};
  const renderTrash=()=>{if(!book){trashList.innerHTML=`<p class="small">${T('notes.pickBookFirst')}</p>`;trashSum.textContent='';return}trashList.innerHTML='';
    const items=trashedEntries().sort((a,b)=>b.updated-a.updated);
    trashSum.textContent=items.length?T('notes.trash.count',{count:items.length}):T('notes.trash.empty');
    if(!items.length){trashList.innerHTML=`<p class="small">${T('notes.trash.noneHint')}</p>`;return}
    items.forEach(e=>{const row=el('div',{class:'trash-item'});
      const text=e.text||(e.drafts&&e.drafts[0]&&e.drafts[0].text)||(e.quote&&e.quote.text)||T('notes.noTextContent');
      const dv=e.destination||'both';
      const chSync=e.chapter!=null?syncMap.get(e.chapter):null;
      // 这条本身去哪（配置的目的地）+ 它所在章节目前的生成/导出状态（章节维度，不是这条自己确认被
      // 收进去了没——归档/撤销后这条已经不在活条目集合里，没法再逆推"当初有没有被打进那次生成"，
      // 只能诚实地给"这一章大致是什么状态"这个参考信息，用户反馈"回收站该显示导出到哪里"）。
      row.innerHTML=`<div class="trash-badges"><span class="badge">${T(STATUS_NAMES[e.status])||e.status}</span><span class="badge">${DEST_ICON[dv]()}</span>${syncBadges(chSync,dv)}</div>
        <div class="txt">p.${e.page_index+1}${e.chapter_title?' · '+esc(e.chapter_title):''}<br><span class="q">${esc(text)}</span>${e.status==='revoked'?`<br><span class="small">${T('notes.trash.revokedHint')}</span>`:''}</div>
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
    const r=await postJ(bookApi('notes',`/import-md`),{title,markdown}); // 失败 postJ 已经 alert 过
    importBtn.disabled=false;
    if(r.ok===false){importStat.textContent='';return}
    importStat.textContent=T('notes.import.done',{name:r.visibleName});importFile.value='';importFileContent='';importFilename.textContent=''};
  guardClick($('#nrestoreall',sec),async()=>{if(!book)return;
    const items=trashedEntries();
    if(!items.length){toast(T('notes.trash.noneToRestore'),'warn');return}
    if(!await confirmDialog(T('notes.trash.confirmRestoreAll',{count:items.length})))return;
    for(const e of items)await restoreOne(e.id);
    await reloadBook(renderTrash,renderBrowse,renderBook)});
  /* 浏览态动作：Mined→Pending（转入笔记）/ Mined→Skipped（不需要），见 ink-serve::triage。三个子视图都要重画（条目跨视图搬家）。 */
  const triage=async(id,action)=>{const r=await postJ(bookApi('ink',`/entries/${encodeURIComponent(id)}/${action}`),{});if(r.ok===false)return;
    await reloadBook(renderBrowse,renderBook,renderTrash)};
  const updateSummary=()=>{if(!book){sum.textContent='';return}const es=book.entries||[];
    const c=st=>es.filter(e=>e.status===st).length;
    sum.textContent=T('notes.summary',{mined:c('mined'),pending:c('pending'),draft:c('draft'),reviewed:c('reviewed')})};
  // 全站唯一一处"按钮文案暗示有代价、却没有二次确认"（2026-09-09 审计发现）：清掉页记录会强制整本
  // 重新摄取。实际数据风险不大（已校对文本/条目不会被覆盖，见 notecore::ingest 的增量规则），但操作
  // 本身不常用、容易误触，补一句说清楚"安全在哪"的确认。
  guardClick($('#nrescan',sec),async()=>{if(!book)return;if(!await confirmDialog(T('notes.confirmRescan')))return;await flushPendingText();await postJ(bookApi('ink',`/rescan`),{});refresh()});
  // KOReader 回流跟当前选的书无关（拉全量高亮/生词、内部按增量规则合并），不用 bookApi/不用 book 判空。
  guardClick($('#nkoimport',sec),async()=>{const r=await postJ('/api/ink/koreader/import',{});if(r.ok===false)return;toast(T('notes.koreaderImportDone',r),'ok');refresh()});
  guardClick($('#npurge',sec),async()=>{if(!book)return;
    const items=trashedEntries();
    if(!items.length){toast(T('notes.trash.noneToPurge'),'warn');return}
    if(!await confirmDialog(T('notes.trash.confirmPurge',{count:items.length})))return;
    await flushPendingText();
    const r=await j(bookApi('ink',`/purge`),{method:'POST'});
    if(r.ok===false){toast(r.message||T('notes.trash.purgeFailed'));return}
    book=await j(bookApi('ink'));renderTrash();refresh()});
  /* 「不要了」（三期）：转 Archived，两处投影都摘掉，条目库里软删留痕（真删靠「回收站」清空）。 */
  const archiveEntry=async(id)=>{if(!await confirmDialog(T('notes.confirmArchive')))return;
    const r=await postJ(bookApi('ink',`/entries/${encodeURIComponent(id)}/archive`),{});if(r.ok===false)return;
    await reloadBook(renderBook,renderTrash)};
  /* 浏览：按页分组、只列 Mined（待决定的），最近变更的页在前；转入笔记/不需要两个按钮直接调 triage。 */
  const renderBrowse=()=>{if(!book){browse.innerHTML=`<div class="card"><p class="small">${T('notes.pickBookFirst')}</p></div>`;return}browse.innerHTML='';updateSummary();
    const mined=(book.entries||[]).filter(e=>e.status==='mined');
    if(!mined.length){browse.innerHTML=`<div class="card"><p class="small">${T('notes.browse.empty')}</p></div>`;return}
    const groups=new Map();mined.forEach(e=>{const k=e.page_index;if(!groups.has(k))groups.set(k,[]);groups.get(k).push(e)});
    const recency=k=>Math.max(...groups.get(k).map(e=>e.updated));
    [...groups.keys()].sort((a,b)=>recency(b)-recency(a)).forEach(k=>{const es=groups.get(k).sort((a,b)=>(a.ink?a.ink.bbox[1]:0)-(b.ink?b.ink.bbox[1]:0));
      const card=el('div',{class:'card'});card.innerHTML=`<h3 style="margin-top:0">${T('notes.pageHeading',{page:k+1})}${es[0].chapter_title?' · '+esc(es[0].chapter_title):''} <span class="small">${T('notes.entryCount',{count:es.length})}</span></h3>`;
      es.forEach(e=>{const row=el('div',{class:'entry'});
        row.innerHTML=`<div class="entry-body">
          <div class="entry-crop">${cropHtml(e)}</div>
          <div class="entry-main">
            ${e.quote?`<div class="entry-quote">「${esc(e.quote.text)}」</div>`:''}
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
    visibleKeys.forEach(k=>{const b=el('button',{class:k===selectedChapter?'on':'',text:k<0?T('notes.unfiledChapter'):T('notes.chapterHeading',{n:k+1})});b.onclick=()=>{selectedChapter=k;renderBook()};chaptertabs.appendChild(b)});
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
    const card=el('div',{class:'card'});
    card.innerHTML=`<h3 style="margin-top:0">${k<0?T('notes.unfiledChapterParen'):esc(T('notes.chapterHeadingTitled',{n:k+1,title:es[0].chapter_title||''}))} <span class="small">${T('notes.entryCount',{count:es.length})}</span></h3>${k>=0?`<div class="row"><button class="btn pri" data-sync title="${T('notes.pushChapterTitle')}">${T('notes.pushChapterBtn')}</button>${syncBadges(s)}<span class="small" data-genmsg></span></div>`:''}<div data-body></div>`;
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
      syncBtn.onclick=async()=>{syncBtn.disabled=true;msg.textContent='';holdFor(15000);
        // 服务端没有天然的分步数据（耗时来自生成笔记本+导出 md 两次整章调用，不是可数的"第几步"）——
        // 跟 stagingList 普通整本落库同一处境，共用同一套不确定态滚动条（2026-09-19 代码质量审计，
        // 原来这里只有一句不会变的静态文字"推送中…"）。
        const prog=renderStepProgress(row,{label:T('notes.pushing'),prog:null,msg:''});
        const gr=await j(bookApi('notes',`/chapters/${k}/generate`),{method:'POST'});
        const er=await j(bookApi('notes',`/chapters/${k}/export`),{method:'POST'});
        prog.remove();
        syncBtn.disabled=false;
        const gc=(gr.chapters&&gr.chapters[0])||{};
        const parts=[];
        if(gr.ok===false)parts.push('✗ '+T('notes.push.notebook')+'：'+(gr.message||T('common.failed')));
        else if(gc.status==='failed')parts.push('✗ '+T('notes.push.notebook')+'：'+gc.error);
        else if(gc.status==='generated')parts.push('✓ '+T('notes.push.notebookUpdated'));
        if(er.ok===false)parts.push('✗ md：'+(er.message||T('common.failed')));
        else if(er.status==='written'){parts.push('✓ '+T('notes.push.mdExported'));window.open(bookApi('notes',`/chapters/${k}/export.md`),'_blank')}
        msg.textContent=parts.length?parts.join(' · '):T('notes.push.noChange');
        // 结果文案刚显示出来，从这一刻起再保 3s，不管上面两次请求实际花了多久
        await lingerThen(async()=>{await refreshSync();renderBook({advance:true})})};
    }
    es.forEach(e=>{const failed=failedIds.has(e.id);const row=el('div',{class:'entry'+(failed?' entry-failed':'')});
      const draft=(e.drafts&&e.drafts[0])?e.drafts[0].text:'';
      const dv=e.destination||'both';
      row.innerHTML=`
        <div class="entry-head">
          <span>p.${e.page_index+1}${e.subhead?' · '+esc(e.subhead):''}</span>
          <span class="badge">${T(STYLE_NAMES[e.style])||e.style}</span>
          ${syncBadges(s,dv)}
          <span class="badge ${e.status==='reviewed'?'on':''}" style="margin-left:auto">${T(STATUS_NAMES[e.status])||e.status}</span>
        </div>
        <div class="entry-body">
          <div class="entry-crop">${cropHtml(e)}</div>
          <div class="entry-main">
            ${e.quote?`<div class="entry-quote">「${esc(e.quote.text)}」</div>`:''}
            <textarea class="entry-text" rows="2" placeholder="${esc(draft?T('notes.draftPlaceholder',{draft}):T('notes.waitingTranscribe'))}">${esc(e.text||draft)}</textarea>
            <div class="small">${T('notes.styleHint')}</div>
            <div class="entry-ops">
              <div class="grp"><button class="btn" data-dest title="${T('notes.dest.switchTitle')}">${DEST_ICON[dv]()} <span aria-hidden="true" style="opacity:.55">⟳</span></button></div>
              <div class="grp">${(e.ink&&e.ink.crop)?`<button class="btn${failed?' btn-bad':''}" data-transcribe title="${T('notes.retranscribeTitle')}">${failed?T('notes.transcribeFailed'):(draft?T('notes.retranscribe'):T('notes.transcribe'))}</button>`:''}<button class="btn" data-archive title="${T('notes.archiveTitle')}">${T('notes.archiveBtn')}</button></div>
            </div>
            <div class="small" data-txstat></div>
            <div class="entry-ask">
              <div class="row"><label class="toggle"><input type="checkbox" data-ask ${e.ask_ai?'checked':''}> ${T('notes.askAi')}</label>
                <input type="text" data-question placeholder="${T('notes.questionPlaceholder')}" value="${e.question?esc(e.question):''}" style="flex:1;min-width:9em" ${e.ask_ai?'':'disabled'}>
                <button class="btn pri" data-askbtn ${e.ask_ai&&e.question?'':'disabled'}>${T('notes.askBtn')}</button></div>
              <div class="small" data-askstat></div>
              ${e.answer?`<div class="entry-answer"><b>${T('notes.aiAnswer')}</b>（${esc(T('notes.askedLabel',{brief:e.answer.brief}))}）<br>${esc(e.answer.text).replace(/\n/g,'<br>')}</div>`:''}
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
      if(tb)tb.onclick=async()=>{tb.disabled=true;txStat.textContent=T('notes.transcribing');holdFor(15000);
        const r=await j(bookApi('transcribe',`/entries/${encodeURIComponent(e.id)}`),{method:'POST'});
        tb.disabled=false;
        txStat.textContent=r.ok===false?('✗ '+(r.message||T('notes.transcribeFailed'))):T('notes.transcribeDone',{promptTokens:r.promptTokens||0,completionTokens:r.completionTokens||0});
        await lingerThen(()=>reloadBook(renderBook))};
      /* 「问AI」勾选框 + 问题 + 提问按钮：改即存（ink-serve），点提问才真的调 mind-serve。 */
      const askBox=row.querySelector('[data-ask]'),qInput=row.querySelector('[data-question]'),askBtn=row.querySelector('[data-askbtn]'),askStat=row.querySelector('[data-askstat]');
      const syncAskUi=()=>{qInput.disabled=!askBox.checked;askBtn.disabled=!(askBox.checked&&qInput.value.trim())};
      askBox.onchange=()=>{patch(e.id,{askAi:askBox.checked});syncAskUi()};
      qInput.onchange=()=>{patch(e.id,{question:qInput.value});syncAskUi()};
      askBtn.onclick=async()=>{askBtn.disabled=true;askStat.textContent=T('notes.asking');holdFor(15000);
        const r=await j(bookApi('mind',`/entries/${encodeURIComponent(e.id)}/ask`),{method:'POST'});
        askBtn.disabled=false;
        if(r.ok===false){askStat.textContent='✗ '+(r.message||T('notes.askFailed'));holdFor(0)}
        else{askStat.textContent=T('notes.askDone',{promptTokens:r.promptTokens||0,completionTokens:r.completionTokens||0});await lingerThen(()=>reloadBook(renderBook))}};
      body.appendChild(row)});
    chapterbody.appendChild(card)};
  exportTabsEl.querySelectorAll('button').forEach(b=>b.onclick=()=>{if(exportTab===b.dataset.etab)return;exportTab=b.dataset.etab;selectedChapter=null;renderBook()});
  const loadBook=async()=>{await flushPendingText();selectedChapter=null;
    book=null;
    if(sel.value){const b=await j(`/api/ink/books/${encodeURIComponent(sel.value)}`);
      // 取失败（ink-serve 重启中、书刚被删）按"没选书"画，别把 {ok:false} 当成书——此前后续请求会拼出 /books/undefined。
      if(b.ok===false)toast(b.message||T('common.failed'));else book=b}
    await refreshSync();renderBrowse();await renderBook();renderTrash();renderImport()};
  sel.onchange=loadBook;
  /* 全文搜索（跨所有书，ink-serve /search）：结果点一下就切到那本书的「浏览」。命中词加粗——片段先 esc 再替换，
     替换用的也是 esc 过的查询词，不会引入未转义的 HTML。 */
  const nq=$('#nq',sec),nqres=$('#nqres',sec);
  const FIELD_KEYS={quote:'notes.search.field.quote',text:'notes.search.field.text',draft:'notes.search.field.draft',question:'notes.search.field.question',answer:'notes.search.field.answer',title:'notes.search.field.title'};
  const mark=(snip,q)=>{const e=esc(snip),qe=esc(q);if(!qe)return e;const i=e.toLowerCase().indexOf(qe.toLowerCase());return i<0?e:e.slice(0,i)+'<b>'+e.slice(i,i+qe.length)+'</b>'+e.slice(i+qe.length)};
  const doSearch=async()=>{const q=nq.value.trim();nqres.innerHTML='';if(!q)return;
    const r=await j('/api/ink/search?q='+encodeURIComponent(q));
    if(r.ok===false){nqres.innerHTML=`<p class="small">${esc(r.message||T('common.failed'))}</p>`;return}
    const items=r.items||[];
    if(!items.length){nqres.innerHTML=`<p class="small">${T('notes.search.none')}</p>`;return}
    const ul=el('ul',{class:'nsearch'});
    items.forEach(h=>{const li=el('li',{html:`<div class="small">${esc(h.title)} · p.${h.pageIndex+1}${h.chapterTitle?' · '+esc(h.chapterTitle):''} · ${T(FIELD_KEYS[h.field]||'notes.search.field.text')}</div><div>${mark(h.snippet,q)}</div>`});
      // 跳到这条目实际所在的子页：未处理（mined）在「浏览」，跳过/归档在「回收站」，其余（待转写/草稿/定稿）在「整理」。
      const tab=h.status==='mined'?0:(h.status==='skipped'||h.status==='archived')?2:1;
      li.onclick=async()=>{if(sel.value!==h.uuid){sel.value=h.uuid;await loadBook()}$('#nsubnav',sec).children[tab].click();nqres.innerHTML=''};
      ul.appendChild(li)});
    nqres.appendChild(el('p',{class:'small',text:T('notes.search.count',{n:items.length})}));nqres.appendChild(ul)};
  guardClick($('#nqgo',sec),doSearch);
  nq.addEventListener('keydown',e=>{if(e.key==='Enter')doSearch()});
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
  // checkImport=false：SSE 事件触发的刷新不重查「导入 md」开关（那个开关在「管理」页改，切回本 tab 的刷新会查）——
  // 自动转写期间每转完一条都有事件，原来每次都连带让网关扫一遍 xochitl 扩展状态。
  const refresh=async(checkImport=true)=>{if(Date.now()<holdRefreshUntil)return; // 正显示着结果提示，别被 SSE 抢跑冲掉（见 holdRefreshUntil 声明处注释）
    const d=await j('/api/ink/books');const cur=sel.value;sel.innerHTML=(d.items||[]).map(b=>`<option value="${esc(b.uuid)}">${esc(b.title)}（${b.entries}）</option>`).join('')||`<option value="">${T('notes.noBooks')}</option>`;
    if(cur&&[...sel.options].some(o=>o.value===cur))sel.value=cur;await loadBook();if(checkImport)await syncImportVisible()};
  /* 事件刷新会整段重画（含正在编辑的文本框）：用户正在本 tab 的输入框里打字时先不重画，只记一笔，焦点离开输入框再补一次。
     此前自动转写/另一台设备的改动一来，光标连同输入框一起被重画掉（文字靠 pendingText 保住了，但得重新点进去）。 */
  const editing=()=>{const a=document.activeElement;return !!a&&sec.contains(a)&&(a.tagName==='TEXTAREA'||(a.tagName==='INPUT'&&/^(text|search)$/.test(a.type)))};
  let deferred=false;
  const evRefresh=coalesce(()=>refresh(false));
  sec.onEvent=()=>{if(editing()){deferred=true;return}evRefresh()};
  sec.addEventListener('focusout',()=>setTimeout(()=>{if(deferred&&!editing()){deferred=false;evRefresh()}},0));
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
  const card=el('div',{class:'card',style:'width:100%;margin:0'});
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
      <input type="number" step="0.001" min="0" data-pricein placeholder="${T('models.priceInPlaceholder')}" style="max-width:6em">
      <input type="number" step="0.001" min="0" data-priceout placeholder="${T('models.priceOutPlaceholder')}" style="max-width:6em">
      <button class="btn" data-pricesave>${T('models.savePriceBtn')}</button>
    </div>
    <label class="field">${T('models.usageLabel')}</label>
    <div class="tblwrap" data-usagewrap><table class="cmp"><thead><tr><th>${T('models.usage.colModel')}</th><th>${T('models.usage.colCalls')}</th><th>${T('models.usage.colTokens')}</th><th>${T('models.usage.colCost')}</th></tr></thead><tbody data-usagebody></tbody></table></div>
    <div class="small" data-stat style="margin-top:.3em;overflow-wrap:anywhere"></div>`;
  root.appendChild(card);
  const vendorSel=card.querySelector('[data-vendor]'),modelBox=card.querySelector('[data-modelbox]'),presetSel=card.querySelector('[data-preset]'),customBox=card.querySelector('[data-custom]'),modelInp=card.querySelector('[data-model]'),urlInp=card.querySelector('[data-url]'),keyRow=card.querySelector('[data-keyrow]'),stat=card.querySelector('[data-stat]'),autoBox=card.querySelector('[data-auto]'),priceIn=card.querySelector('[data-pricein]'),priceOut=card.querySelector('[data-priceout]'),usageBody=card.querySelector('[data-usagebody]');
  const put=body=>jsend(`/api/${seg}/config`,'PUT',body);
  // 「改一项配置 → 失败弹 toast → 无论成败都重画」：厂家/模型/密钥/自定义/单价这几个动作共用。
  const putR=async(body,failKey='common.failed')=>{const r=await put(body);if(r.ok===false)toast(r.message||T(failKey));refresh()};
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
    vendorSel.innerHTML=vendors.map(v=>`<option value="${esc(v)}">${esc(T(PROVIDER_NAMES[v])||v)}</option>`).join('')+`<option value="custom">${T('models.customVendor')}</option>`;
    const activeVendor=c.activePreset==='custom'?'custom':(presets.find(p=>p.id===c.activePreset)||{}).provider||'custom';
    vendorSel.value=activeVendor;
    const isCustom=activeVendor==='custom';
    customBox.hidden=!isCustom;modelBox.hidden=isCustom;
    if(isCustom){modelInp.value=c.model||'';urlInp.value=c.baseUrl||''}
    else{presetSel.innerHTML=modelsOf(activeVendor).map(p=>`<option value="${esc(p.id)}">${esc(p.label)}</option>`).join('');presetSel.value=c.activePreset}
    keyRow.innerHTML=c.hasKey
      ?`<span class="small">${T('models.keySaved',{key:esc(c.keyMasked||'••••')})}</span><button class="btn" data-delkey>${T('action.delete')}</button>`
      :`<input type="password" placeholder="${T('models.keyInputPlaceholder')}" data-keyinput style="flex:1;min-width:11em" autocomplete="off"><button class="btn pri" data-savekey>${T('models.saveKeyBtn')}</button>`;
    if(autoBox){autoBox.checked=!!c.auto;autoBox.onchange=async()=>{const r=await put({auto:autoBox.checked});if(r.ok===false){toast(r.message||T('common.failed'));autoBox.checked=!autoBox.checked}}}
    const price=c.price||{inputPer1k:0,outputPer1k:0};
    priceIn.value=price.inputPer1k||'';priceOut.value=price.outputPer1k||'';
    const rows=st.usageByModel||[];
    usageBody.innerHTML=rows.length?rows.map(m=>`<tr${m.active?' style="font-weight:600"':''}><td>${esc(m.label)}${m.active?` <span class="badge on">${T('models.usage.active')}</span>`:''}</td><td>${m.calls}${m.failed?` <span style="color:var(--bad)">${T('models.usage.failedCount',{n:m.failed})}</span>`:''}</td><td>${m.promptTokens}/${m.completionTokens}</td><td>${fmtCost(m.costEstimate)}</td></tr>`).join(''):`<tr><td colspan="4" class="small">${T('models.usage.none')}</td></tr>`;
    stat.textContent=rows.find(m=>m.active&&m.lastError)?.lastError?T('models.lastError',{err:rows.find(m=>m.active).lastError}):'';
    const delBtn=keyRow.querySelector('[data-delkey]'),saveBtn=keyRow.querySelector('[data-savekey]');
    if(delBtn)guardClick(delBtn,async()=>{if(!await confirmDialog(T('models.confirmDeleteKey',{title})))return;await putR({clearKey:true},'models.deleteFailed')});
    if(saveBtn)guardClick(saveBtn,async()=>{const v=keyRow.querySelector('[data-keyinput]').value.trim();if(!v)return;await putR({apiKey:v})});
  };
  /* 选厂家：不是自定义就直接定位到该厂家第一个模型并原子切换（不用再点一次「确认」）；选自定义只切
     UI（露出手填框），真正生效要等用户填完点「保存自定义」——避免半吊子状态被当成已保存的配置发出去。 */
  vendorSel.onchange=async()=>{const v=vendorSel.value;customBox.hidden=v!=='custom';modelBox.hidden=v==='custom';
    if(v==='custom')return;
    const first=modelsOf(v)[0];if(!first)return;
    await putR({preset:first.id})};
  presetSel.onchange=async()=>{await putR({preset:presetSel.value})};
  guardClick(card.querySelector('[data-savecustom]'),async()=>{await putR({preset:'custom',model:modelInp.value.trim(),baseUrl:urlInp.value.trim()})});
  guardClick(card.querySelector('[data-pricesave]'),async()=>{await putR({price:{input:parseFloat(priceIn.value)||0,output:parseFloat(priceOut.value)||0}})});
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
  ?`<ul class="list">${items.map(it=>`<li><span>${esc(it.name)}</span><span class="small">${fmtMs(it.ms)} · ${it.pct}%</span></li>`).join('')}</ul>`
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
  const refresh=async(known)=>{const r=known||await j('/api/enhance/status');if(r.ok===false)return; // known：调用方刚取过的 /api/enhance/status，省一次重复请求
    const st=r.battop||{};installed=!!st.installed;
    box.checked=!!st.running;box.disabled=!installed;
    note.textContent=installed?'':T('battop.toggle.notInstalled')};
  box.onchange=async()=>{if(!installed)return;const want=box.checked;box.disabled=true;
    const r=await j(`/api/enhance/battop/${want?'start':'stop'}`,{method:'POST'});
    if(r.ok===false){toast(r.message||T('common.failed'));box.checked=!want}
    box.disabled=false;await refresh()};
  // 挂载时不自己取：唯一调用方（「管理」页）挂载后紧接着就取 /api/enhance/status 并把结果传进来（erApply），
  // 这里再取一次就是同一个接口连发两遍（每次都让网关查 systemctl + 扫 xochitl 进程映射）。
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
  // 不在这里先取一次：这个面板只在 battop 运行时显示，由「管理」页的 erApply 在确认运行后调 sec.refresh。
  sec.refresh=refresh;subtabs(sec);
}

/* 「管理 → 设备健康」（2026-09-25，gateway/src/device/）：只读体检 + 清理遗留数据。**只在这一屏被打开、或点刷新时
   取数**，不跟管理页其它子标签一起刷、不订阅任何定时器（设备要省电）。网关侧结果缓存 15 秒，刷新按钮带 fresh=1 现采。
   2026-09-25 同日用户反馈"太长"，拆成五个二级 tab（概览/服务/扩展/日志/清理，subtabs() 惯例，第三层嵌套靠它的
   `:scope >` 限定）：一次 /api/device/health（+ /api/device/ota）的结果分发到前四个 tab，切 tab 只显隐、不重取；
   清理是单独接口，进这一屏时它不在前台就只记"待取"，第一次切到它时才取。上次停在哪个二级 tab 记在 LS。 */
const fmtUptime=s=>{const d=Math.floor(s/86400),h=Math.floor(s%86400/3600),m=Math.floor(s%3600/60);
  return d?T('health.upDays',{d,h}):h?T('health.upHours',{h,m}):T('health.upMins',{m})};
// 两分钟以内按秒（开机时序要看到 0.01 s 级），更长的（服务开机后被重启过）换成"12 分钟"这类读法，不写 761.5 s。
const fmtSec=ms=>ms>=120000?fmtUptime(Math.floor(ms/1000)):(ms/1000).toFixed(ms<10000?2:1)+' s';
const fmtTime=secs=>secs?new Date(secs*1000).toLocaleString():'';
function mountHealth(box){
  const card=k=>`<div class="subpanel" data-p="${k}"><div class="card" data-body><p class="small">${T('health.loading')}</p></div></div>`;
  box.innerHTML=`<div class="card"><div class="row" style="justify-content:space-between;margin-top:0"><h2 style="margin:0">${T('health.title')}</h2><button class="btn" data-refresh>${T('health.refresh')}</button></div>
    <p class="lead">${T('health.lead')}</p><p class="small" data-at></p></div>
    <div class="subnav"><button>${T('health.tab.overview')}</button><button>${T('health.tab.services')}</button><button>${T('health.tab.extensions')}</button><button>${T('health.tab.log')}</button><button>${T('health.tab.cleanup')}</button></div>
    ${card('overview')}${card('services')}${card('extensions')}${card('log')}
    <div class="subpanel" data-p="cleanup"><div class="card" data-cleanup></div></div>`;
  const q=s=>box.querySelector(s),body=k=>q(`[data-p="${k}"] [data-body]`),btn=q('[data-refresh]');
  const cleanupLoad=mountCleanup(q('[data-cleanup]'));
  const CLEANUP=4;let cleanupStale=true;
  const cleanupIfShown=()=>{if(cleanupStale&&q('[data-p="cleanup"]').classList.contains('on')){cleanupStale=false;return cleanupLoad()}};
  subtabs(box);
  const btns=[...q('.subnav').children],saved=+LS.get('healthSub','0');
  // 恢复上次的二级 tab：先调 subtabs 装的原始切换，再包记忆/取数那层——挂载管理页时不该去取清理数据。
  btns[saved>=0&&saved<btns.length?saved:0].onclick();
  btns.forEach((b,i)=>{const sw=b.onclick;b.onclick=()=>{sw();LS.set('healthSub',String(i));if(i===CLEANUP)cleanupIfShown()}});
  const loadHealth=async fresh=>{
    const [d,o]=await Promise.all([j('/api/device/health'+(fresh?'?fresh=1':'')),j('/api/device/ota'+(fresh?'?fresh=1':''))]);
    if(d.ok===false){const m=`<p class="small">${esc(d.message)}</p>`;['overview','services','extensions','log'].forEach(k=>body(k).innerHTML=m);return}
    const units=d.units||[],xu=units.find(u=>u.unit==='xochitl.service')||{},x=d.xochitl||{};
    const none=`<span class="small">${T('health.none')}</span>`;
    const names=l=>l&&l.length?esc(l.join(' · ')):none;
    const fw=d.firmware||{};
    const fwTxt=fw.state==='done'?(fw.known?`<span class="badge on" title="${esc(fw.label||'')}">${esc(T('health.fw.known',{label:(fw.label||'').split(/\s/)[0]}))}</span>`
        :`<span class="badge off" title="${T('health.fw.unknownTitle')}">${T('health.fw.unknown')}</span>`)+` <code style="overflow-wrap:anywhere">${esc((fw.sha256||'').slice(0,16))}…</code>`
      :fw.state==='error'?`<span class="small">${esc(T('health.fw.error',{msg:fw.message||''}))}</span>`:`<span class="small">${T('health.fw.pending')}</span>`;
    // OTA 判定跟页头横幅同一个接口（device/ota.rs）；这里只是换个地方常驻显示，横幅被 × 掉之后也能在这看到。
    const otaTxt=o.ok===false?'<span class="small">—</span>':o.needsReinstall
      ?badge(o.recovery==='full'?T('ota.banner.title'):T('ota.banner.xoviTitle'),false)+`<br><span class="small">${(o.reasons||[]).map(r=>esc(T('ota.reason.'+r,{units:(o.missingUnits||[]).join(', ')}))).join('<br>')}</span>`
      :badge(T('health.ota.ok'),true);
    const home=d.home||{};
    q('[data-at]').textContent=T('health.at',{time:fmtTime(d.at)});
    body('overview').innerHTML=`<div class="kv small">
      <b>${T('health.uptime')}</b><span>${d.uptimeSecs!=null?fmtUptime(d.uptimeSecs):'—'}</span>
      <b>xochitl</b><span>${badge(esc(xu.active||'?'),xu.active==='active')} <span title="${T('health.restartsTitle')}">${T('health.unit.restarts',{n:xu.nRestarts??'?'})}</span>${xu.pid?' · PID '+xu.pid:''}</span>
      <b>${T('health.xovi')}</b><span>${x.readable?badge(x.xovi?T('manage.loaded.on'):T('manage.loaded.off'),!!x.xovi):'<span class="small">—</span>'}</span>
      <b>${T('health.home')}</b><span>${home.freeBytes!=null?T('health.homeVal',{free:fmtB(home.freeBytes),total:fmtB(home.totalBytes||0)}):'—'}</span>
      <b>${T('health.firmware')}</b><span>${fwTxt}</span>
      <b>${T('health.ota')}</b><span>${otaTxt}</span></div>`;
    body('services').innerHTML=`<h3 style="margin-top:0">${T('health.services')}</h3><p class="small">${T('health.servicesHint')}</p>`
      +(d.systemctlError?`<p class="small">${esc(T('health.systemctlError',{msg:d.systemctlError}))}</p>`:'')+'<ul class="list" data-units></ul>';
    const ul=body('services').querySelector('[data-units]');
    units.forEach(u=>{
      const missing=u.load==='not-found';
      const st=missing?`<span class="badge">${T('health.unit.notFound')}</span>`:badge(esc((u.active||'?')+(u.sub?' / '+u.sub:'')),u.active==='active');
      const bits=[];
      if(!missing&&u.nRestarts!=null)bits.push(`<span title="${T('health.restartsTitle')}">${T('health.unit.restarts',{n:u.nRestarts})}</span>`);
      if(u.rssKb!=null)bits.push(esc(T('health.unit.mem',{rss:fmtB(u.rssKb*1024),hwm:fmtB((u.hwmKb||0)*1024)})));
      if(u.startedAtMs!=null)bits.push(esc(u.startMs!=null&&u.startMs>=50?T('health.unit.started',{at:fmtSec(u.startedAtMs),dur:fmtSec(u.startMs)}):T('health.unit.startedAt',{at:fmtSec(u.startedAtMs)})));
      ul.appendChild(el('li',{class:'stack'},[el('span',{html:`${esc(u.unit.replace(/\.service$/,''))} ${st}`}),el('span',{class:'small',html:bits.map(b=>`<span class="nw">${b}</span>`).join(' · ')})]))});
    // 扩展：徽章 title 在触屏上看不到，所以把"换了文件未重启 / 待换入"的解释直接写成小字放在各自下面。
    body('extensions').innerHTML=`<div class="kv small">
      <b>${T('health.xovi')}</b><span>${x.readable?badge(x.xovi?T('manage.loaded.on'):T('manage.loaded.off'),!!x.xovi):'<span class="small">—</span>'}</span>
      <b>${T('health.extensions')}</b><span>${names(x.extensions)}</span></div>
      ${(x.deleted&&x.deleted.length)||(d.soPending&&d.soPending.length)?`
      <h3>${T('health.deleted')}</h3><p class="small">${T('health.deletedTitle')}</p>
      <p>${x.deleted&&x.deleted.length?`<span class="badge off">${esc(x.deleted.join(' · '))}</span>`:none}</p>
      <h3>${T('health.soPending')}</h3><p class="small">${T('health.soPendingTitle')}</p>
      <p>${d.soPending&&d.soPending.length?`<span class="badge">${esc(d.soPending.join(' · '))}</span>`:none}</p>`
      :`<p class="small">${T('health.extClean')}</p>`}`;
    // 上次开机最后几行 journal（设备冻死/意外重启的线索）；journal 没持久化时后端给 null。
    body('log').innerHTML=`<h3 style="margin-top:0">${T('health.prevBoot')}</h3>`
      +(d.prevBoot&&d.prevBoot.length?`<pre class="hlog">${esc(d.prevBoot.join('\n'))}</pre>`:`<p class="small">${T('health.prevBootNone')}</p>`);
  };
  /* 进这一屏（fresh=false）或点刷新（fresh=true）：健康数据现取；清理只在它正在前台时取，否则等第一次切过去。 */
  const load=async fresh=>{cleanupStale=true;await Promise.all([loadHealth(fresh),cleanupIfShown()])};
  guardClick(btn,()=>load(true));
  return load;
}
function mountCleanup(box){
  box.innerHTML=`<h2>${T('cleanup.title')}</h2><p class="lead">${T('cleanup.lead')}</p>
    <h3>${T('cleanup.done.title')}</h3><p class="small" data-filesdesc>${T('cleanup.done.desc')}</p>
    <ul class="list" data-files></ul><div class="row" data-filesrow><button class="btn btn-bad" data-delfiles disabled></button></div>
    <h3>${T('cleanup.lib.title')}</h3><p class="small">${T('cleanup.lib.desc')}</p><p class="small" data-agent></p>
    <div class="stg-tools"><input type="search" data-q placeholder="${T('cleanup.lib.search')}"><select data-filter><option value="dup">${T('cleanup.lib.filterDup')}</option><option value="all">${T('cleanup.lib.filterAll')}</option></select></div>
    <ul class="list" data-lib></ul><div class="row" data-librow><button class="btn btn-bad" data-trash disabled></button></div>`;
  const q=s=>box.querySelector(s);
  const pickF=new Set(),pickL=new Map();let files=[],lib=[];
  const syncBtns=()=>{const a=q('[data-delfiles]'),b=q('[data-trash]');
    a.textContent=T('cleanup.deleteBtn',{n:pickF.size});a.disabled=!pickF.size;
    b.textContent=T('cleanup.lib.trashBtn',{n:pickL.size});b.disabled=!pickL.size};
  const check=(on,fn)=>{const c=el('input',{type:'checkbox'});c.checked=on;c.onchange=()=>{fn(c.checked);syncBtns()};return el('label',{class:'stg-check'},[c])};
  const renderFiles=()=>{const ul=q('[data-files]');ul.innerHTML='';
    q('[data-filesdesc]').hidden=q('[data-filesrow]').hidden=!files.length;
    if(!files.length){ul.appendChild(el('li',{class:'small',text:T('cleanup.done.empty')}));return}
    files.forEach(f=>ul.appendChild(el('li',{},[el('span',{style:'display:flex;gap:.5em;align-items:flex-start'},[check(pickF.has(f.name),v=>v?pickF.add(f.name):pickF.delete(f.name)),el('span',{text:f.name})]),
      el('span',{class:'small',text:`${fmtB(f.bytes)} · ${fmtTime(f.mtime)}`})])))};
  const renderLib=()=>{const ul=q('[data-lib]');ul.innerHTML='';
    const kw=q('[data-q]').value.trim().toLowerCase(),dup=q('[data-filter]').value==='dup';
    const list=lib.filter(b=>(!dup||b.sameName>1)&&(!kw||(b.name+' '+b.folder).toLowerCase().includes(kw)));
    q('[data-librow]').hidden=!list.length&&!pickL.size;
    if(!list.length){ul.appendChild(el('li',{class:'small',text:T('cleanup.lib.empty')}));return}
    list.forEach(b=>{
      const meta=[esc(b.folder||T('cleanup.lib.root')),b.kind.toUpperCase(),fmtB(b.bytes),esc(fmtTime(Math.floor(b.createdMs/1000)))].join(' · ');
      const same=b.sameName>1?` <span class="badge" title="${esc(T('cleanup.lib.sameNameTitle',{n:b.sameName}))}">${T('cleanup.lib.sameName',{n:b.sameName})}</span>`:'';
      ul.appendChild(el('li',{},[el('span',{style:'display:flex;gap:.5em;align-items:flex-start;flex:1;margin-left:0'},[check(pickL.has(b.uuid),v=>v?pickL.set(b.uuid,b.name):pickL.delete(b.uuid)),el('span',{html:`${esc(b.name)}${same}<br><span class="small">${meta}</span>`})])]))})};
  q('[data-q]').oninput=renderLib;q('[data-filter]').onchange=renderLib;
  const listOf=names=>names.slice(0,12).map(n=>'· '+n).join('\n')+(names.length>12?'\n…':'');
  /* 不用 guardClick：它在 finally 里无条件解禁按钮，而这里"没勾选"时按钮应保持禁用——收尾交给 syncBtns。 */
  const busyClick=(b,fn)=>{b.onclick=async()=>{if(b.disabled)return;b.disabled=true;try{await fn()}catch(e){console.error(e);toast(T('common.failed'))}finally{syncBtns()}}};
  busyClick(q('[data-delfiles]'),async()=>{const names=[...pickF];
    if(!names.length||!await confirmDialog(T('cleanup.confirmFiles',{n:names.length,list:listOf(names)})))return;
    const r=await jsend('/api/device/cleanup/delete','POST',{area:'books-done',names});
    if(r.failed&&r.failed.length)toast(T('cleanup.partial',{ok:(r.deleted||[]).length,bad:r.failed.length,msg:r.failed.map(f=>f.name+'：'+f.error).join('；')}));
    else if(r.ok===false)toast(r.message||T('common.failed'));
    else toast(T('cleanup.deleted',{n:(r.deleted||[]).length}),'ok');
    pickF.clear();await load()});
  busyClick(q('[data-trash]'),async()=>{const picks=[...pickL];
    if(!picks.length||!await confirmDialog(T('cleanup.lib.confirm',{n:picks.length,list:listOf(picks.map(p=>p[1]))})))return;
    let ok=0;const bad=[];
    for(const [uuid,name] of picks){const r=await jsend('/api/books/trash/add','POST',{uuid,name});if(r.ok===false)bad.push(name+'：'+(r.message||''));else ok++}
    if(bad.length)toast(T('cleanup.partial',{ok,bad:bad.length,msg:bad.join('；')}));else toast(T('cleanup.lib.queued',{n:ok}),'ok',6500);
    pickL.clear();await load()});
  const load=async()=>{const d=await j('/api/device/cleanup');
    if(d.ok===false){q('[data-files]').innerHTML=`<li class="small">${esc(d.message)}</li>`;return}
    files=d.files||[];lib=d.library||[];
    for(const n of [...pickF])if(!files.some(f=>f.name===n))pickF.delete(n);
    for(const u of [...pickL.keys()])if(!lib.some(b=>b.uuid===u))pickL.delete(u);
    q('[data-agent]').textContent=!d.xochitl?T('cleanup.lib.noXochitl'):d.trashAgent?'':T('cleanup.lib.agentOff');
    renderFiles();renderLib();syncBtns()};
  syncBtns();
  return load;
}
/* 页头"需要重新安装"横幅（2026-09-25）：页面打开时取一次 /api/device/ota（网关侧判定见 device/ota.rs：单元文件缺失 /
   xovi 未生效；固件哈希不在白名单只作附加原因），不轮询。关掉只在本次页面会话内有效。 */
async function showOtaBanner(){
  const d=await j('/api/device/ota');if(d.ok===false||!d.needsReinstall)return;
  const reasons=(d.reasons||[]).map(r=>`<li>${esc(T('ota.reason.'+r,{units:(d.missingUnits||[]).join(', ')}))}</li>`).join('');
  const cmd=d.recovery==='full'?`<p>${T('ota.recovery.full',{cmd1:'<code>/home/root/xovi/rebuild_hashtable</code>'})}</p><pre class="hlog">cd packaging &amp;&amp; sh install-all.sh ${esc(location.hostname)}${(d.reasons||[]).includes('firmware-unknown')?' --force':''}</pre>`
    :`<p>${T('ota.recovery.xovi')}</p><pre class="hlog">cd packaging &amp;&amp; sh deploy-xovi-apply.sh ${esc(location.hostname)}</pre>`;
  const x=el('button',{class:'btn x',type:'button',title:T('ota.dismiss'),'aria-label':T('ota.dismiss'),text:'×'});
  const ban=el('div',{class:'otabanner',role:'alert',html:`<b>${d.recovery==='full'?T('ota.banner.title'):T('ota.banner.xoviTitle')}</b><ul>${reasons}</ul>${cmd}<p class="small">${T('ota.recovery.doc')}</p>`});
  ban.prepend(x);x.onclick=()=>ban.remove();
  document.body.insertBefore(ban,$('#main'));
}
/* 页头"设备上没做成"横幅（2026-09-25）：移进 xochitl 回收站、在 xochitl 书库建文件夹，这两件事由设备端代理执行，
   交满 5 次仍没做成 book-serve 就放弃（见 book-serve agent_failures.rs）。页面打开时取一次，之后收到 `agent-failed`
   事件再取，不轮询；「知道了」清空服务端记录（换台设备/刷新后也不再出现）。book-serve 没开时接口不通，不显示。 */
async function showAgentFailBanner(){
  const old=$('#agentfail');
  const d=await j('/api/books/agent-failures');const items=d.ok===false?[]:(d.items||[]);
  if(!items.length){if(old)old.remove();return}
  const li=items.slice().reverse().map(f=>`<li>${esc(T('agentfail.'+(f.kind==='mkdir'?'mkdir':'trash'),{name:f.name}))} <span class="small">${esc(fmtTime(f.at))}</span></li>`).join('');
  const ok=el('button',{class:'btn',type:'button',text:T('agentfail.ack')});
  const ban=el('div',{class:'otabanner',id:'agentfail',role:'alert',html:`<b>${T('agentfail.title',{n:items.length})}</b><ul>${li}</ul><p class="small">${T('agentfail.hint')}</p>`});
  ban.appendChild(ok);
  ok.onclick=async()=>{ok.disabled=true;const r=await postJ('/api/books/agent-failures/clear',{});if(r.ok===false){ok.disabled=false;return}ban.remove()};
  if(old)old.replaceWith(ban);else document.body.insertBefore(ban,$('#main'));
}

/* 管理台/引导（固定 tab，始终在——它是网关自身页面，不由服务注册表驱动） */
/* 「管理」二级 tab（2026-09-09 起三个，2026-09-10 加到五个）：① 基石与模块（原来就有的引导/开关/
   卸载）② 模型管理（原来挂在这页最下面，现在单独一屏，不用跟基石列表一起滚）③ 系统增强（只留真正
   "系统级"的开关，CJK 画线吸附）④ 电池刺客（`battop.running` 时才出现，放在「实验室」前面——用户
   要求顺序）⑤ 实验室（还在打磨/覆盖面没到日常好用程度的功能：CJK 手写笔迹优化开关+漫画页边距开关
   +导入md文档可见性开关）。电池刺客开关 2026-09-21 起在③「系统增强」里（用户要求从实验室移出）。
   （曾在这页的 shelf push 命令卡片已随 2026-09-18 砍掉 host CLI 一并删除。）另有「设备健康」（2026-09-25）。 */
/* 模块管理动作（start / stop / uninstall），「基石与模块」列表与「全部开启/关闭」共用。 */
const modAct=(seg,act)=>j('/api/manage/'+seg+'/'+act,{method:'POST'});
function renderManage(sec){sec.innerHTML=`
  <div class="subnav"><button class="on">${T('manage.subnav.foundation')}</button><button data-sub="health">${T('manage.subnav.health')}</button><button>${T('manage.subnav.models')}</button><button>${T('manage.subnav.enhance')}</button><button hidden data-sub="battop">${T('manage.subnav.battop')}</button><button>${T('manage.subnav.lab')}</button></div>
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
  <div class="subpanel" id="healthBox"></div>
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
      <label class="toggle"><input type="checkbox" id="erHlSnap"> ${T('manage.enhance.hlSnap.toggle')}</label> <span id="erHlSnapLoaded"></span></div>
    <div class="card"><h3 style="margin-top:0">${T('manage.enhance.pageTurn.title')} <span id="erPageTurnLoaded"></span></h3>
      <p class="small">${T('manage.enhance.pageTurn.desc')}</p>
      <label class="toggle"><input type="checkbox" id="erTapPageTurn"> ${T('manage.enhance.pageTurn.tapToggle')}</label>
      <p class="small">${T('manage.enhance.pageTurn.tapHint')}</p>
      <label class="toggle"><input type="checkbox" id="erRtlPageTurn"> ${T('manage.enhance.pageTurn.rtlToggle')}</label>
      <p class="small">${T('manage.enhance.pageTurn.rtlHint')}</p></div>
    <div class="card" id="enhBattopCard"></div>
  </div>
  <div class="subpanel" id="battopDetail" hidden></div>
  <div class="subpanel">
    <div class="card"><h3 style="margin-top:0">${T('manage.lab.hwStroke.title')}</h3>
      <p class="small">${T('manage.lab.hwStroke.desc')}</p>
      <label class="toggle"><input type="checkbox" id="labHwStroke"> ${T('manage.lab.hwStroke.toggle')}</label> <span id="labHwStrokeLoaded"></span></div>
    <div class="card"><h3 style="margin-top:0">${T('manage.lab.comicMargin.title')}</h3>
      <p class="small">${T('manage.lab.comicMargin.desc')}</p>
      <label class="toggle"><input type="checkbox" id="labComicMargin"> ${T('manage.lab.comicMargin.toggle')}</label> <span id="labComicMarginLoaded"></span></div>
    <div class="card"><h3 style="margin-top:0">${T('manage.lab.importMd.title')}</h3>
      <p class="small">${T('manage.lab.importMd.desc')}</p>
      <label class="toggle"><input type="checkbox" id="labImportMd"> ${T('manage.lab.importMd.toggle')}</label> <span class="badge" title="${T('manage.loaded.webOnlyTitle')}">${T('manage.loaded.webOnly')}</span></div>
  </div>`;
  const mvRefresh=mountModelPanel($('#modelcards',sec),'transcribe',T('manage.models.visionTitle'),'👁',true);
  const mtRefresh=mountModelPanel($('#modelcards',sec),'mind',T('manage.models.textTitle'),'✎');
  /* 基石 + 模块三态 + 系统增强开关：三个接口并行取，/api/enhance/status 只取一次（扩展加载状态在「基石」与
     「系统增强/实验室」两处都要用，原来各取一遍，每次都让网关扫一遍 /proc）。 */
  const refresh=async()=>{
    const [f,es,d]=await Promise.all([j('/api/foundation'),j('/api/enhance/status'),j('/api/manage')]);
    const inst=v=>badge(v?T('common.installed'):T('common.notInstalled'),v);
    const ld=(es.ok!==false&&es.loaded)||{},live=[...(ld.extensions||[]),...(ld.qmds||[])];
    $('#found',sec).innerHTML=f.ok===false?`<span>${esc(f.message)}</span>`:
      `<b>xovi</b><span>${inst(f.xovi)}</span><b>appload</b><span>${inst(f.appload)}</span><b>qt-resource-rebuilder</b><span>${inst(f.qrr)}</span><b>KOReader</b><span>${inst(f.koreader)}</span><b>WeRead</b><span>${inst(f.weread)}</span>`
      +`<b>${T('manage.loaded.xoviLive')}</b><span>${badge(ld.xovi?T('manage.loaded.on'):T('manage.loaded.off'),!!ld.xovi)}${live.length?' <span class="small">'+esc(live.join(' · '))+'</span>':''}</span>`;
    const ul=$('#mods',sec);ul.innerHTML='';(d.modules||[]).forEach(m=>{
      const [state,cls]=!m.installable?[T('manage.modules.state.notLaunched'),'']:!m.installed?[T('common.notInstalled'),'off']:m.running?[T('manage.modules.state.on'),'on']:[T('manage.modules.state.installedOff'),''];
      const lk='manage.modules.label.'+m.seg,label=I18N[lk]?T(lk):m.label; // 语言包缺这个 seg 时兜底用后端 Rust 侧的中文 label（T() 缺 key 返回 key 本身，不能靠 ||）
      const left=el('span',{html:`${esc(label)} <span class="small">${esc(m.service)}</span> <span class="badge ${cls}">${state}</span>`});
      const right=el('span',{style:'display:flex;gap:.4em;align-items:center'});
      if(m.installable&&m.installed){
        const t=el('button',{class:'btn',text:m.running?T('manage.modules.turnOff'):T('manage.modules.turnOn')});
        guardClick(t,async()=>{const r=await modAct(m.seg,m.running?'stop':'start');if(r.ok===false)toast(r.message);setTimeout(refresh,600)});
        const u=el('button',{class:'btn',text:T('manage.modules.uninstallBtn')});
        guardClick(u,async()=>{if(await confirmDialog(T('manage.modules.confirmUninstall',{label}))){const r=await modAct(m.seg,'uninstall');if(r.ok===false)toast(r.message);else toast(T('manage.modules.uninstalled',{label}),'ok');setTimeout(()=>location.reload(),800)}});
        right.append(t,u);
      }else if(m.installable)right.appendChild(el('span',{class:'small',html:T('manage.modules.installCmd',{only:esc(m.only)})}));
      ul.appendChild(el('li',{style:'flex-wrap:wrap'},[left,right]))});
    if(es.ok!==false)await erApply(es)};
  guardClick($('#allon',sec),async()=>{const d=await j('/api/manage');for(const m of (d.modules||[]))if(m.installable&&m.installed&&!m.running)await modAct(m.seg,'start');refresh()});
  guardClick($('#alloff',sec),async()=>{if(!await confirmDialog(T('manage.modules.confirmAllOff')))return;const d=await j('/api/manage');for(const m of (d.modules||[]))if(m.installable&&m.installed&&m.running)await modAct(m.seg,'stop');refresh()});
  /* 系统增强/实验室（Track 3，2026-09-09；实验室 2026-09-10 加）：CJK 画线吸附/CJK 手写笔迹优化/
     导入md文档可见性都是真开关（写 reading-qol.json，走同一个 /api/enhance/qol）。battop 拆两处：
     「系统增强」卡片只留开关+说明（mountBattopToggleCard），详细数据挪到本函数下面新增的第 5 个
     二级 tab「电池刺客」（renderBattopDetail）——这个 tab 本身「运行才出现」，规则/实现都照抄
     「笔记」tab「导入 md 文档」子标签那套 hidden 属性+点走再隐藏的写法（见 renderNotes 里
     syncImportVisible 的注释，这里不重复讲一遍）。 */
  const tapBox=$('#erTapPageTurn',sec),rtlBox=$('#erRtlPageTurn',sec);
  const hlBox=$('#erHlSnap',sec),hwBox=$('#labHwStroke',sec),importMdBox=$('#labImportMd',sec),comicMarginBox=$('#labComicMargin',sec);
  const battopToggleRefresh=mountBattopToggleCard($('#enhBattopCard',sec)); // 电池刺客开关在「系统增强」里（2026-09-21 从实验室移过来）
  const manageNav=sec.querySelector(':scope > .subnav');
  const battopNavBtn=manageNav.querySelector('[data-sub="battop"]'),battopPanel=$('#battopDetail',sec);
  renderBattopDetail(battopPanel);
  const erApply=async r=>{
    hlBox.checked=!!r.hlSnapCjk;
    hwBox.checked=!!r.hwStrokeEnabled;
    importMdBox.checked=!!r.notesImportMdEnabled;
    comicMarginBox.checked=!!r.comicMinMargin;
    tapBox.checked=!!r.tapPageTurn;rtlBox.checked=!!r.rtlPageTurn;
    // 开关旁边标"xochitl 里实际有没有加载这个扩展"（查主进程 maps，见 gateway enhance/loaded.rs）：开关只是配置，
    // 扩展没加载时开了也不生效——历史上两次"看着装了、其实没生效"就是这种情况。
    const ld=r.loaded||{},exts=ld.extensions||[];
    const loadedBadge=so=>!ld.xochitl?`<span class="badge" title="${T('manage.loaded.noXochitlTitle')}">${T('manage.loaded.unknown')}</span>`
      :exts.includes(so)?`<span class="badge on" title="${T('manage.loaded.onTitle')}">${T('manage.loaded.on')}</span>`
      :`<span class="badge off" title="${T('manage.loaded.offTitle')}">${T('manage.loaded.off')}</span>`;
    $('#erHlSnapLoaded',sec).innerHTML=loadedBadge('hl-snap.so');
    $('#labHwStrokeLoaded',sec).innerHTML=loadedBadge('hw-stroke.so');
    // 漫画页边距、阅读器翻页靠 qmd 补丁（xochitl 启动时由 qt-resource-rebuilder 读一次），不是 .so：看 loaded.qmds / qmdsPending。
    const qmdBadge=qmd=>!ld.xochitl?loadedBadge(qmd)
      :(ld.qmds||[]).includes(qmd)?`<span class="badge on" title="${T('manage.loaded.qmdOnTitle')}">${T('manage.loaded.on')}</span>`
      :(ld.qmdsPending||[]).includes(qmd)?`<span class="badge" title="${T('manage.loaded.qmdPendingTitle')}">${T('manage.loaded.pending')}</span>`
      :`<span class="badge off" title="${T('manage.loaded.qmdOffTitle')}">${T('manage.loaded.off')}</span>`;
    $('#labComicMarginLoaded',sec).innerHTML=qmdBadge('shelf-comic-margins.qmd');
    $('#erPageTurnLoaded',sec).innerHTML=qmdBadge('reader-page-turn.qmd');
    await battopToggleRefresh(r);
    const running=!!(r.battop&&r.battop.running);
    if(!running&&battopNavBtn.classList.contains('on'))manageNav.children[0].click();
    battopNavBtn.hidden=!running;battopPanel.hidden=!running;
    if(running&&battopPanel.refresh)battopPanel.refresh()};
  bindToggle(hlBox,'/api/enhance/qol','hlSnapCjk');bindToggle(hwBox,'/api/enhance/qol','hwStrokeEnabled');bindToggle(importMdBox,'/api/enhance/qol','notesImportMdEnabled');bindToggle(comicMarginBox,'/api/enhance/qol','comicMinMargin');
  bindToggle(tapBox,'/api/enhance/qol','tapPageTurn');bindToggle(rtlBox,'/api/enhance/qol','rtlPageTurn');
  /* 设备健康：切到这个子标签时才取数（每次切过去都取一次，网关侧有 15 秒缓存），不跟着管理页的 SSE 刷新走；
     清理那组只在它是当前二级 tab 时一起取（见 mountHealth）。 */
  const healthLoad=mountHealth($('#healthBox',sec));
  const healthNavBtn=manageNav.querySelector('[data-sub="health"]');
  refresh();sec.refresh=()=>Promise.all([refresh(),mvRefresh(),mtRefresh()]);subtabs(sec);
  const tabClick=healthNavBtn.onclick;healthNavBtn.onclick=()=>{tabClick();healthLoad(false)};}

(async()=>{
  // 语言包先拿到手：下面 addTab 用得到 T()，晚拿会让顶层导航先短暂显示 key 本身再跳成文字。
  // 拿不到（离线/服务重启中）静默留空对象——T() 兜底显示 key，不是白屏，也不阻塞页面其余部分。
  const lang=currentLang();
  try{I18N=await(await fetch(`/ui/locales/${lang}.json`)).json()}catch{I18N={}}
  document.title=T('app.title');$('#applogo').textContent=T('app.title');
  $('#navpw').textContent=T('nav.changePassword');$('#navca').textContent=T('nav.caCert');$('#logout').textContent=T('nav.signOut');
  $('#mainloading').textContent=T('main.loading');
  showOtaBanner(); // 不 await：横幅晚一点出现无妨，不挡页面主体
  showAgentFailBanner();
  const langsel=$('#langsel');langsel.value=lang;langsel.setAttribute('aria-label',T('nav.lang'));
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
  /* 各 tab **第一次切过去时才渲染**（渲染本身就会取一次数据）：此前页面一打开就把笔记/其他/管理全部渲染、各自取一遍数据
     （二十来个请求，还让网关扫 /proc、问模型服务），而且首个 tab 渲染完紧接着又被点击刷新一次，同样的 6 个请求发两遍。
     之后再切回来才走 refreshSec 刷新。 */
  const addTab=(title,render,first,area)=>{const b=document.createElement('button');b.textContent=title;const sec=document.createElement('section');sec.area=area;secByArea[area]=sec;
    let rendered=false;
    b.onclick=()=>{[...nav.children].forEach(x=>x.classList.remove('on'));[...main.children].forEach(x=>x.classList.remove('on'));b.classList.add('on');sec.classList.add('on');dirty.delete(area);
      if(!rendered){rendered=true;render(sec)}else if(sec.refresh)refreshSec(sec)};
    nav.appendChild(b);main.appendChild(sec);if(first)b.onclick();return sec};
  addTab(T('tab.transfer'),renderTransfer,true,'books');          // 总入口（入库｜母版库），固定第一位（book-serve 不在时列表里提示去管理页开）
  noteSvc.forEach((s)=>addTab(TABS[s.name].titleKey?T(TABS[s.name].titleKey):TABS[s.name].title,TABS[s.name].render,false,AREA[s.name]||s.name));
  if(otherSvcs.length){
    const otherSec=addTab(T('tab.other'),(sec)=>renderOther(sec,otherSvcs,n=>AREA[n]||n),false,'other');
    // fonts/koreader/wallpapers 各自的 SSE 事件原来路由到各自独立顶层 section，现在都嵌进了同一个
    // 「其他」section——三个 area 名都指向同一个 otherSec，事件到了随便哪个都触发它的合并 refresh
    // （2026-09-25 起按事件的 svc 只刷发事件的那块子面板；切到「其他」tab、重连时才三块一起刷）。
    otherSvcs.forEach(s=>{secByArea[AREA[s.name]||s.name]=otherSec});
  }
  addTab(T('tab.manage'),renderManage,false,'manage');            // 固定管理台，始终可进
  /* 事件推送（SSE，零轮询）：服务在变更处发事件 → 网关 /api/events 汇聚 → 这里只刷对应 tab；不在前台的 tab 记脏，切过去时刷。
     manage 事件（服务启停）：tab 集合变了就整页重载，否则只刷管理台。断线（WiFi 掉/设备休眠醒来）EventSource 自动重连，
     重连成功（非首次 onopen）补刷一次当前 tab——断线期间的事件没人推给我们。
     省电/省流量：页面被隐藏（切标签页、手机锁屏）时**不刷新**，只记"当前 tab 待刷"，`visibilitychange` 变可见时补刷一次；
     每个 tab 的刷新用 coalesce 合并（事件突发/多来源叠加时同一时刻最多一个在飞）。 */
  const svcKey=svcs.map(s=>s.name).join(',');
  const liveDot=el('span',{id:'live',title:T('common.eventStream'),text:'●'});liveDot.style.cssText='margin-left:.5em;font-size:.8em;color:var(--bad)';$('#hdr').appendChild(liveDot);
  const activeSec=()=>[...main.children].find(x=>x.classList.contains('on'));
  let activeStale=false,opened=false,es=null,hiddenTimer=0;
  /* 心跳 ?ka=60：默认 20 秒一帧空注释，只是为了让中间代理不掐空闲连接；网关直连浏览器用不着这么勤（设备上每帧都是一次唤醒+TLS 写）。
     页面隐藏超过 60 秒就**主动断开** SSE（锁屏/切走的标签页不再让设备为它保活），重新可见时重连——重连成功的 onopen
     本来就会补刷当前 tab（见上），断开期间漏掉的事件不丢；别的 tab 切过去时无条件刷新（addTab 的点击处理），也不依赖 dirty。 */
  const HIDDEN_CLOSE_MS=60000;
  const openEs=()=>{
    es=new EventSource('/api/events?ka=60');
    es.onopen=()=>{liveDot.style.color='var(--ok)';liveDot.title=T('common.eventStreamConnected');
      if(opened){if(document.hidden)activeStale=true;else{activeStale=false;const s=activeSec();if(s)refreshSec(s)}}opened=true};
    es.onerror=()=>{liveDot.style.color='var(--bad)';liveDot.title=T('common.eventStreamReconnecting')};
    es.onmessage=async(e)=>{let ev;try{ev=JSON.parse(e.data)}catch{return}
      if(ev.kind==='agent-failed'){showAgentFailBanner();return} // 全站横幅，与哪个 tab 无关
      if(ev.area==='manage'){const d=await j('/api/services');const k=(d.services||[]).filter(s=>s.ui&&TABS[s.name]).map(s=>s.name).join(',');if(k!==svcKey){location.reload();return}}
      const sec=secByArea[ev.area];if(!sec)return;
      if(!sec.classList.contains('on'))dirty.add(ev.area);
      else if(document.hidden)activeStale=true;
      else if(sec.onEvent)sec.onEvent(ev); // tab 自己按事件决定刷多少（母版库：网关排队/进度事件只重取两个状态）
      else refreshSec(sec)}};
  const closeEs=()=>{if(!es)return;es.close();es=null;liveDot.style.color='var(--bad)';liveDot.title=T('common.eventStreamReconnecting')};
  document.addEventListener('visibilitychange',()=>{
    if(document.hidden){clearTimeout(hiddenTimer);hiddenTimer=setTimeout(closeEs,HIDDEN_CLOSE_MS);return}
    clearTimeout(hiddenTimer);
    if(!es){openEs();return} // 重连后的 onopen 负责补刷当前 tab
    if(activeStale){activeStale=false;const s=activeSec();if(s)refreshSec(s)}});
  openEs();
  if(document.hidden)hiddenTimer=setTimeout(closeEs,HIDDEN_CLOSE_MS); // 页面是在后台标签页里打开的
})();

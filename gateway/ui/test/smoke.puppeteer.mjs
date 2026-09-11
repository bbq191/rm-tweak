// 前端冒烟（真浏览器，需要 puppeteer 或 playwright；**手动跑，不进 CI**——CI 只跑零依赖的 xss.test.mjs）。
// 用法：PUPPETEER_NODE_MODULES=<含 puppeteer 的 node_modules 目录> node gateway/ui/test/smoke.puppeteer.mjs
// （本机可用 mermaid-cli 自带的：/…/lib/node_modules/@mermaid-js/mermaid-cli/node_modules/）；
// 或 PLAYWRIGHT_NODE_MODULES=<含 playwright 的 node_modules 目录>（如 gateway/tools/screenshot-walkthrough/node_modules）。
// 不起网关：拦截请求直接喂拼好的页面，并在页面里 mock 掉 fetch 与 EventSource，验证：
//   ① 文件名/错误文案里的 HTML 不会注入 DOM（无 <img>、onerror 不触发）；
//   ② 10 个 SSE 事件连发被 coalesce 成至多 2 次刷新；网关自身的批量/闸门事件只重取那两个状态，book-serve 的 staging 事件不重取
//      KOReader 目录；③ 页面隐藏时不刷新、可见后补刷一次；④ 空闲无轮询；⑤ 三种对话框的键盘/点击行为；⑥ 笔记 tab 正在输入时
//      事件不重画、失焦补刷；⑦「其他」tab 的事件只刷发事件服务的子面板；⑧ 代理放弃横幅（显示 / 事件重取 / 知道了）。
import { createRequire } from 'node:module';
import fs from 'node:fs';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
const PW = process.env.PLAYWRIGHT_NODE_MODULES;
const require = createRequire((PW || process.env.PUPPETEER_NODE_MODULES || process.cwd() + '/node_modules') + '/');
const UI = join(dirname(fileURLToPath(import.meta.url)), '..') + '/';
let html = fs.readFileSync(UI + 'index.html', 'utf8');
const exts = JSON.stringify({book:['epub','pdf'],native:['epub','pdf'],font:['ttf'],dict:['ifo'],image:['png']});
html = html.replace('__STYLE__', fs.readFileSync(UI + 'style.css','utf8')).replace('__SCRIPT__', fs.readFileSync(UI + 'app.js','utf8').replace('__EXTS__', exts));
const zh = fs.readFileSync(UI + 'locales/zh-CN.json','utf8');
const mock = `
window.__hits = {}; window.__es = []; window.__hidden = false; window.__xss = 0;
Object.defineProperty(document, 'hidden', {get: () => window.__hidden});
class ES { constructor(u){ this.u=u; this.closed=false; window.__es.push(this); setTimeout(() => this.onopen && !this.closed && this.onopen(), 10);} close(){ this.closed=true; } }
const __st = window.setTimeout.bind(window); window.setTimeout = (f, ms, ...a) => __st(f, ms === 60000 && window.__hiddenCloseMs ? window.__hiddenCloseMs : ms, ...a);
window.EventSource = ES;
const zh = ${JSON.stringify(zh)};
const evil = '<img src=x onerror=window.__xss=1>.epub';
const routes = {
  '/ui/locales/zh-CN.json': () => JSON.parse(zh),
  '/ui/locales/en-US.json': () => JSON.parse(zh), // 无头浏览器缺省英文界面：同样喂中文包，文案断言不必分两套
  '/api/services': () => ({services: [{name:'note-serve', ui:{order:1}}, {name:'font-serve', ui:{order:2}}, {name:'wallpaper-serve', ui:{order:3}}]}),
  '/api/manage': () => ({modules: [{service:'note-serve', seg:'notes'}, {service:'font-serve', seg:'fonts'}, {service:'wallpaper-serve', seg:'wallpapers'}]}),
  '/api/fonts': () => ({items:[]}), '/api/fonts/status': () => ({ok:true}),
  '/api/wallpapers': () => ({items:[]}), '/api/wallpapers/status': () => ({ok:true, mode:'sequential'}),
  '/api/ink/books': () => ({items:[{uuid:'u1', title:'书', entries:1}]}),
  '/api/ink/books/u1': () => ({uuid:'u1', entries:[{id:'e1', status:'pending', chapter:0, page_index:0, destination:'both', style:'body', text:'hi', updated:1}]}),
  '/api/notes/books/u1/sync': () => ({chapters:[]}),
  '/api/transcribe/status': () => ({failures:[]}),
  '/api/books/staging': () => ({ok:true, items:[{name: evil, format:'epub', bytes:1000, mtime:1, optimized:false, delivered:{optimize:{status:'failed', message:'"><img src=x onerror=window.__xss=1>'}}}], freeBytes: 9e9}),
  '/api/books/status': () => ({ok:true, xochitlFolders:[]}),
  '/api/koreader/status': () => ({ok:true, installed:false}),
  '/api/koreader/books': () => ({items:[]}),
  '/api/budget/status': () => ({pending:[], active:[]}),
  '/api/batch/status': () => ({running:false,total:0,done:0,queued:[],failed:[]}),
  '/api/foundation': () => ({}), '/api/enhance/status': () => ({}),
  '/api/books/agent-failures': () => ({items: window.__fails}),
  '/api/books/agent-failures/clear': () => { const n = window.__fails.length; window.__fails = []; return {ok:true, cleared:n}; },
};
window.__fails = [{kind:'trash', name: evil, uuid:'11111111-1111-1111-1111-111111111111', at:1}, {kind:'mkdir', name:'新文件夹', at:2}];
window.fetch = async (url, opt) => {
  const p = String(url).split('?')[0];
  window.__hits[p] = (window.__hits[p]||0) + 1;
  await new Promise(r => setTimeout(r, 30));
  const f = routes[p]; const body = f ? f() : {};
  return { status: 200, ok: true, json: async () => body, text: async () => JSON.stringify(body) };
};
`;
html = html.replace('<head>', '<head><script>' + mock + '</script>');
let browser, page;
if (PW) {
  browser = await require('playwright').chromium.launch();
  page = await browser.newPage();
  await page.route('**/*', r => r.request().url() === 'http://shelf.test/' ? r.fulfill({status:200, contentType:'text/html', body: html}) : r.abort());
} else {
  browser = await require('puppeteer').launch({ headless: true, args: ['--no-sandbox'] });
  page = await browser.newPage();
  await page.setRequestInterception(true);
  page.on('request', r => r.url() === 'http://shelf.test/' ? r.respond({status:200, contentType:'text/html', body: html}) : r.abort());
}
const errs = [];
page.on('pageerror', e => errs.push('pageerror: ' + e.message));
page.on('console', m => { if (m.type() === 'error') errs.push('console: ' + m.text()); });
await page.goto('http://shelf.test/', { waitUntil: 'load' });
await new Promise(r => setTimeout(r, 600));
const hits = (p = '/api/books/staging') => page.evaluate(p => window.__hits[p] || 0, p);
const out = {};
out.initialHits = await hits();
out.xss = await page.evaluate(() => window.__xss);
out.listText = await page.evaluate(() => (document.querySelector('#stglist')||{}).textContent || '');
out.imgInjected = await page.evaluate(() => document.querySelectorAll('#stglist img').length);
// 代理放弃横幅：页面打开即显示，名字按文本显示；agent-failed 事件只重取这一个接口；「知道了」清空并移除横幅
out.failBanner = await page.evaluate(() => { const b = document.querySelector('#agentfail'); return b ? {li: b.querySelectorAll('li').length, img: b.querySelectorAll('img').length, text: b.textContent} : null; });
{ const a0 = await hits('/api/books/agent-failures'), st0 = await hits();
  await page.evaluate(() => window.__es[0].onmessage({data: JSON.stringify({area:'books', kind:'agent-failed', svc:'books'})}));
  await new Promise(r => setTimeout(r, 300));
  out.failEventHits = [(await hits('/api/books/agent-failures')) - a0, (await hits()) - st0]; }
await page.evaluate(() => document.querySelector('#agentfail .btn').click());
await new Promise(r => setTimeout(r, 300));
out.failAck = [await hits('/api/books/agent-failures/clear'), await page.evaluate(() => !!document.querySelector('#agentfail'))];
// 事件突发：10 个 book-serve 事件连发 → 合并
await page.evaluate(() => { for (let i = 0; i < 10; i++) window.__es[0].onmessage({data: JSON.stringify({area:'books', kind:'staging', svc:'books'})}); });
await new Promise(r => setTimeout(r, 500));
out.afterBurst = (await hits()) - out.initialHits;
out.burstKoBooks = (await hits('/api/koreader/books')) - 1; // 首次全量取过 1 次；staging 事件不该再取
// 网关自身的批量/闸门事件（不带 svc）：只重取批量/闸门状态，不全量刷新
const s0 = await hits(), b0 = await hits('/api/batch/status');
await page.evaluate(() => { for (let i = 0; i < 5; i++) window.__es[0].onmessage({data: JSON.stringify({area:'books', kind: i % 2 ? 'budget' : 'batch'})}); });
await new Promise(r => setTimeout(r, 500));
out.queueEventStagingHits = (await hits()) - s0;
out.queueEventBatchHits = (await hits('/api/batch/status')) - b0;
// 隐藏时不刷新
const h0 = await hits();
await page.evaluate(() => { window.__hidden = true; window.__es[0].onmessage({data: JSON.stringify({area:'books'})}); });
await new Promise(r => setTimeout(r, 300));
out.hiddenHits = (await hits()) - h0;
// 可见后补刷一次
await page.evaluate(() => { window.__hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
await new Promise(r => setTimeout(r, 300));
out.afterVisible = (await hits()) - h0;
// 搜索框防抖：连敲字不立即整表重画，停手 150ms 后才生效
out.searchImmediate = await page.evaluate(() => { const q = document.querySelector('#stgq'); q.value = 'zzz-no-match'; q.dispatchEvent(new Event('input')); return document.querySelectorAll('#stglist .stg-row').length; });
await new Promise(r => setTimeout(r, 400));
out.searchAfter = await page.evaluate(() => { const q = document.querySelector('#stgq'); const n = document.querySelectorAll('#stglist .stg-row').length; q.value = ''; q.dispatchEvent(new Event('change')); return n; });
// 隐藏超时后断开 SSE、重新可见时重连并补刷一次（把 60s 缩成 50ms）；心跳参数 ka=60
out.esUrl = await page.evaluate(() => window.__es[0].u);
await page.evaluate(() => { window.__hiddenCloseMs = 50; window.__hidden = true; document.dispatchEvent(new Event('visibilitychange')); });
await new Promise(r => setTimeout(r, 300));
out.esClosedWhileHidden = await page.evaluate(() => window.__es[window.__es.length - 1].closed);
const h2 = await hits();
await page.evaluate(() => { window.__hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
await new Promise(r => setTimeout(r, 400));
out.esCount = await page.evaluate(() => window.__es.length);
out.esReopenedOpen = await page.evaluate(() => !window.__es[window.__es.length - 1].closed);
out.reopenRefresh = (await hits()) - h2;
// 对话框：Enter=确认、Esc=取消、点选项返回其值；关闭后遮罩移除、键盘监听摘掉
out.dialogs = await page.evaluate(async () => {
  const key = k => document.dispatchEvent(new KeyboardEvent('keydown', {key: k}));
  const r = [];
  let p = confirmDialog('x'); key('Enter'); r.push(await p);
  p = confirmDialog('x'); key('Escape'); r.push(await p);
  p = promptDialog('x', 'abc'); key('Enter'); r.push(await p);
  p = promptDialog('x', 'abc'); document.querySelector('.confirm-overlay').click(); r.push(await p);
  p = choiceDialog('x', [{value:'a', label:'A'}, {value:'b', label:'B'}], 'a'); document.querySelectorAll('.choice-opts button')[1].click(); r.push(await p);
  key('Enter'); // 已关闭的对话框不该再响应
  r.push(document.querySelectorAll('.confirm-overlay').length);
  return r;
});
// 笔记 tab：正在输入框里打字时 SSE 事件不重画（光标不丢），失焦后补一次；事件刷新不重查「导入 md」开关
await page.evaluate(() => document.querySelectorAll('#tabs button')[1].click()); // 传书 / 笔记 / 管理
await new Promise(r => setTimeout(r, 600));
const n0 = await hits('/api/ink/books'), e0 = await hits('/api/enhance/status');
out.noteTa = await page.evaluate(() => { document.querySelector('#nsubnav').children[1].click(); const ta = document.querySelector('textarea.entry-text'); if (!ta) return false; ta.focus(); window.__ta = ta; return true; });
await page.evaluate(() => window.__es[window.__es.length - 1].onmessage({data: JSON.stringify({area:'notes', kind:'entries', svc:'ink'})}));
await new Promise(r => setTimeout(r, 400));
out.noteWhileTyping = (await hits('/api/ink/books')) - n0;
out.noteFocusKept = await page.evaluate(() => document.activeElement === window.__ta && document.contains(window.__ta));
await page.evaluate(() => window.__ta.blur());
await new Promise(r => setTimeout(r, 500));
out.noteAfterBlur = (await hits('/api/ink/books')) - n0;
out.noteEventEnhance = (await hits('/api/enhance/status')) - e0;
// 「其他」tab：壁纸服务的事件只刷壁纸子面板，不连带重取字体
await page.evaluate(() => document.querySelectorAll('#tabs button')[2].click()); // 传书 / 笔记 / 其他 / 管理
await new Promise(r => setTimeout(r, 500));
const f0 = await hits('/api/fonts'), w0 = await hits('/api/wallpapers');
await page.evaluate(() => window.__es[window.__es.length - 1].onmessage({data: JSON.stringify({area:'wallpapers', kind:'pool', svc:'wallpapers'})}));
await new Promise(r => setTimeout(r, 400));
out.otherFonts = (await hits('/api/fonts')) - f0;
out.otherWalls = (await hits('/api/wallpapers')) - w0;
// 无轮询：等 4 秒无新请求
const h1 = await hits();
await new Promise(r => setTimeout(r, 4000));
out.idleHits = (await hits()) - h1;
await browser.close();
console.log(JSON.stringify({ ...out, errs }, null, 1));
assert.equal(out.xss, 0, '含 HTML 的文件名/错误文案不能执行脚本');
assert.equal(out.imgInjected, 0, '不能注入 <img>');
assert.ok(out.failBanner && out.failBanner.li === 2 && out.failBanner.img === 0 && out.failBanner.text.includes('<img src=x'), '代理放弃横幅：两条、名字按文本显示');
assert.deepEqual(out.failEventHits, [1, 0], 'agent-failed 事件只重取放弃记录，不刷母版库');
assert.deepEqual(out.failAck, [1, false], '「知道了」清空服务端记录并移除横幅');
assert.ok(out.listText.includes('<img src=x'), '文件名应作为文本显示');
assert.ok(out.afterBurst >= 1 && out.afterBurst <= 2, `事件突发应合并，实际 ${out.afterBurst} 次`);
assert.equal(out.burstKoBooks, 0, 'book-serve 的 staging 事件只重取母版库列表与排队状态，不重取 KOReader 目录');
assert.deepEqual(out.dialogs, [true, false, 'abc', null, 'b', 0], '对话框行为');
assert.equal(out.noteTa, true, '笔记 tab 应渲染出条目文本框');
assert.equal(out.noteWhileTyping, 0, '正在输入时事件不该触发重画');
assert.equal(out.noteFocusKept, true, '输入框焦点不该被重画冲掉');
assert.equal(out.noteAfterBlur, 1, '失焦后补刷一次');
assert.equal(out.noteEventEnhance, 0, '事件刷新不重查 /api/enhance/status');
assert.equal(out.otherFonts, 0, '壁纸事件不该重取字体列表');
assert.equal(out.otherWalls, 1, '壁纸事件应刷壁纸子面板一次');
assert.equal(out.queueEventStagingHits, 0, '网关排队/进度事件不该全量重取母版库列表');
assert.ok(out.queueEventBatchHits >= 1 && out.queueEventBatchHits <= 2, `排队事件应重取批量状态且合并，实际 ${out.queueEventBatchHits} 次`);
assert.equal(out.hiddenHits, 0, '页面隐藏时不该刷新');
assert.equal(out.afterVisible, 1, '可见后应补刷一次');
assert.equal(out.searchImmediate, 1, '敲字后同一时刻不该立即重画（防抖）');
assert.equal(out.searchAfter, 0, '防抖到点后过滤应生效');
assert.ok(out.esUrl.includes('ka=60'), 'SSE 应带 ?ka=60 拉长心跳');
assert.equal(out.esClosedWhileHidden, true, '页面隐藏超时后应断开 SSE');
assert.equal(out.esCount, 2, '重新可见应重连（新建一条 EventSource）');
assert.equal(out.esReopenedOpen, true);
assert.equal(out.reopenRefresh, 1, '重连成功应补刷当前 tab 一次');
assert.equal(out.idleHits, 0, '空闲时不该有轮询');
assert.deepEqual(errs, [], '不该有 JS 报错');
console.log('smoke OK');

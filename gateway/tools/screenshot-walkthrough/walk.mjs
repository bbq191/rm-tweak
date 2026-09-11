// gateway 前端可视渲染走查：登录 + 中英文各切一遍顶层 tab/子 tab，全页截图落盘 + 收集控制台/
// 页面 JS 错误、4xx/5xx HTTP 响应。不是自动化断言测试（没有"通过/失败"判定，UI 截图靠人眼看），
// 是给"改完前端从没人眼看过浏览器实际渲染"这类缺口提供一个可重复、覆盖面固定的走查工具——用法/
// 前置条件见同目录 README.md，不要单独跑这个文件（先跑 run.sh 起服务+灌 fixture）。
//
// 环境变量：
//   WALK_BASE  网关地址，默认 https://127.0.0.1:8778
//   WALK_PW    网关密码，默认 screenshot-walkthrough-pw-9527（跟 run.sh 里 `gateway passwd` 设的一致）
// 参数：node walk.mjs <截图输出目录>（默认 ./shots）
import { chromium } from 'playwright';
import fs from 'node:fs';

const BASE = process.env.WALK_BASE || 'https://127.0.0.1:8778';
const PW = process.env.WALK_PW || 'screenshot-walkthrough-pw-9527';
const OUT = process.argv[2] || './shots';
fs.mkdirSync(OUT, { recursive: true });

// 这几个 4xx 是"服务缺席→404 未安装，UI 据 /api/services 隐藏 tab"的正常设计行为（见
// gateway/src/main.rs 头注）——run.sh 只起 book-serve/ink-serve/note-serve 三个服务，font/
// koreader/wallpaper/transcribe/mind 五个缺席时前端仍会轮询它们的 status，全是预期噪音，
// 过滤掉之后剩下的才是真正值得人看一眼的异常响应。
const EXPECTED_404 = [/\/api\/(font|koreader|wallpaper|transcribe|mind)\//];

const browser = await chromium.launch();

async function walkLang(lang) {
  const ctx = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1280, height: 900 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(`[pageerror] ${e.message}`));
  // "Failed to load resource: ... 404" 是浏览器对网络失败请求的自动控制台回声，不带 URL 文本、
  // 跟下面 'response' 监听器已经按 URL 精确记录/过滤的信息完全重复——只留这里没见过的、真正的 JS
  // 运行时 console.error()，别让同一条 404 在报告里出现两次又没法按 URL 过滤掉已知噪音。
  page.on('console', (m) => { if (m.type() === 'error' && !/Failed to load resource/.test(m.text())) errors.push(`[console] ${m.text()}`); });
  page.on('response', (r) => {
    if (r.status() < 400) return;
    if (r.status() === 404 && EXPECTED_404.some((re) => re.test(r.url()))) return;
    errors.push(`[http ${r.status()}] ${r.url()}`);
  });

  // 登录
  await page.goto(`${BASE}/login`);
  await page.fill('#pw', PW);
  await page.click('button[type=submit], form button');
  await page.waitForLoadState('networkidle');

  // 切语言（写 localStorage 后 reload，跟 app.js 的 langsel.onchange 行为一致；LS 对象的 key
  // 都加 'shelf.' 前缀，见 gateway/ui/app.js 的 const LS）。
  await page.evaluate((l) => localStorage.setItem('shelf.lang', l), lang);
  await page.reload();
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(600); // 等 SSE/异步 refresh 落定

  const navButtons = await page.$$('#tabs > button');
  console.log(`[${lang}] nav 按钮数：${navButtons.length}`);

  for (let i = 0; i < navButtons.length; i++) {
    const btns = await page.$$('#tabs > button');
    const label = (await btns[i].textContent())?.trim() || `tab${i}`;
    await btns[i].click();
    await page.waitForTimeout(500);
    const safeLabel = label.replace(/[^\w一-鿿]+/g, '_');
    await page.screenshot({ path: `${OUT}/${lang}-${i}-${safeLabel}.png`, fullPage: true });
    console.log(`[${lang}] 截图 tab[${i}]="${label}"`);

    // 一级子 tab（`.subnav`，如笔记的 浏览/整理/回收站）+ 「整理」内嵌的未导出/已导出切换
    // （`[data-etab]`）——只有两层，不递归进章节 tab 那一层（可读性 vs 自动化程度的取舍，
    // 见 README「已知局限」）。只点可见按钮，跳过 hidden 的（如「导入 md 文档」默认关闭）。
    const sec = await page.$('#main > section.on');
    if (sec) {
      const subBtnsAll = await sec.$$(':scope > .subnav > button, [data-etab]');
      const visIdx = [];
      for (let j = 0; j < subBtnsAll.length; j++) if (await subBtnsAll[j].isVisible()) visIdx.push(j);
      for (const j of visIdx) {
        const subs = await sec.$$(':scope > .subnav > button, [data-etab]');
        if (j >= subs.length || !(await subs[j].isVisible())) continue;
        const subLabel = (await subs[j].textContent())?.trim() || `sub${j}`;
        await subs[j].click();
        await page.waitForTimeout(400);
        const safeSub = subLabel.replace(/[^\w一-鿿]+/g, '_');
        await page.screenshot({ path: `${OUT}/${lang}-${i}-${safeLabel}-sub${j}-${safeSub}.png`, fullPage: true });
        console.log(`[${lang}]   子tab[${j}]="${subLabel}"`);
      }
    }
  }

  await ctx.close();
  return errors;
}

const zhErrors = await walkLang('zh-CN');
const enErrors = await walkLang('en-US');
await browser.close();

const allErrors = { 'zh-CN': zhErrors, 'en-US': enErrors };
fs.writeFileSync(`${OUT}/console-errors.json`, JSON.stringify(allErrors, null, 2));
const total = zhErrors.length + enErrors.length;
if (total > 0) {
  console.log(`⚠ 共 ${total} 条未过滤的控制台/HTTP 错误，见 ${OUT}/console-errors.json`);
} else {
  console.log('✅ 零未过滤的控制台/HTTP 错误');
}

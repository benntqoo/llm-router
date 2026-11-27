/**
 * 临时探测：验证 `waitForRender` 之后**能读到登录按钮**（修 bug ③ 的假阳性）。
 *
 * 用法：node probe-render.mjs [url]
 */
import { chromium } from "playwright";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const url = process.argv[2] ?? "https://agent.minimax.cn/";
const profile = mkdtempSync(join(tmpdir(), "llmr-probe-"));
const context = await chromium.launchPersistentContext(profile, {
  headless: true,
  channel: "chrome",
  locale: "zh-CN",
});
const page = context.pages()[0] ?? (await context.newPage());

const t0 = Date.now();
await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
const tDom = Date.now();

// 辅轨里的 waitForRender（当前实现：等按钮出现 + 800ms）
await page.waitForSelector("button", { timeout: 15_000 }).catch(() => {});
await new Promise((r) => setTimeout(r, 800));
const tRender = Date.now();

async function loginButtonCount() {
  let total = 0;
  for (const text of ["登录", "登 录", "立即登录", "Sign in", "Log in"]) {
    total += await page.locator(`button:has-text("${text}")`).count();
    total += await page.locator(`a:has-text("${text}")`).count();
  }
  return total;
}

const count = await loginButtonCount();

console.log(JSON.stringify({
  domContentLoadedMs: tDom - t0,
  afterRenderMs: tRender - t0,
  loginButtonCount: count,
  note: count > 0 ? "✓ 能读到登录按钮 → 不会被误判成已登录" : "✗ 读不到（页面渲染慢）",
}));

await context.close();

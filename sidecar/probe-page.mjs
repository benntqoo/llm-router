/**
 * 临时探测：MiniMax Code 网页版的真实结构（找签到入口）。
 *
 * **不是**正式辅轨的一部分——只用于确定 DOM 选择器 / 按钮文案。
 * 用法：node probe-page.mjs [url]
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

const calls = [];
page.on("request", (req) => {
  if (req.url().includes("/minimax-cloud/api/")) {
    calls.push(`${req.method()} ${req.url()}`);
  }
});

await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
await new Promise((r) => setTimeout(r, 4000));

console.log("URL:", page.url());
console.log("TITLE:", await page.title());

const buttons = await page.evaluate(() => {
  const out = [];
  for (const el of document.querySelectorAll("button, a, [role=button], [class*=sign], [class*=check], [class*=daily]")) {
    const text = (el.textContent ?? "").trim().replace(/\s+/g, " ");
    if (text && text.length < 40) {
      out.push(`${el.tagName}.${String(el.className).slice(0, 40)} :: ${text}`);
    }
  }
  return [...new Set(out)].slice(0, 60);
});
console.log("--- 可点元素 ---");
for (const line of buttons) console.log(" ", line);

console.log("--- 页面正文前 600 字 ---");
const body = await page.evaluate(() => (document.body?.innerText ?? "").replace(/\n{2,}/g, "\n").slice(0, 600));
console.log(body);

console.log("--- API 调用 ---");
for (const call of [...new Set(calls)]) console.log(" ", call);

await context.close();

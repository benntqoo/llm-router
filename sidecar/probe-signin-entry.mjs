/**
 * 临时探测 5：逐个点开候选触发点，找**签到入口**。
 *
 * 候选（按可能性排序）：用户头像下拉（`.ant-dropdown-trigger`）→ 侧栏「更多」→
 * 任意含 avatar/user 类的可点元素。每点一次就抓一次「菜单/浮层文字 + 关键词命中」。
 *
 * **脱敏**：所有输出文本过 scrub（手机号 / 长数字 id / 长不透明串 → 占位符）——
 * 用户下拉里**有账号 id 和手机号**，不能原样打进日志。
 *
 * 用法：LLMR_BROWSER_PROFILE=... node probe-signin-entry.mjs [url]
 */
import { chromium } from "playwright";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const url = process.argv[2] ?? "https://agent.minimax.cn/";
const profile = process.env.LLMR_BROWSER_PROFILE
  ? resolve(process.env.LLMR_BROWSER_PROFILE)
  : join(homedir(), ".llm-router", "browser-profile");

const KEYWORDS = ["签到", "积分", "奖励", "福利", "领取", "每日", "连签", "账户", "个人中心", "我的"];

/** 脱敏：手机号 / 长数字 id / 长不透明串 → 占位符。 */
function scrub(text) {
  return String(text)
    .replace(/[\w.+-]+@[\w-]+\.[\w.]+/g, "<email>")
    .replace(/\b1[3-9]\d{9}\b/g, "<phone>")
    .replace(/\b\d{12,}\b/g, "<id>")
    .replace(/[A-Za-z0-9_-]{32,}/g, "<opaque>");
}

const context = await chromium.launchPersistentContext(profile, {
  headless: true,
  channel: "chrome",
  viewport: { width: 1440, height: 900 },
  locale: "zh-CN",
});
const page = context.pages()[0] ?? (await context.newPage());

await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
await page.waitForSelector("button", { timeout: 15_000 }).catch(() => {});
await new Promise((r) => setTimeout(r, 1500));

/** 当前可见的浮层文字（菜单 / 弹窗 / 下拉）。 */
async function overlays() {
  return page.evaluate(() => {
    const out = [];
    const nodes = document.querySelectorAll(
      '[role="menu"], [role="menuitem"], [role="dialog"], [class*="dropdown"], [class*="popover"], [class*="modal"], [class*="drawer"], [class*="pop-menu"], [class*="tooltip"]',
    );
    for (const node of nodes) {
      const rect = node.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) continue;
      const text = (node.innerText || "").trim();
      if (!text) continue;
      out.push({ cls: (node.className || "").toString().slice(0, 70), text: text.slice(0, 240) });
    }
    return out;
  });
}
/** 关键词命中（含不可见的——有些入口是隐藏的浮层）。 */
async function keywordHits(keys) {
  return page.evaluate((ks) => {
    const out = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node) {
      const text = (node.textContent || "").trim();
      if (text && text.length <= 40 && ks.some((k) => text.includes(k))) {
        const el = node.parentElement;
        if (el) {
          const rect = el.getBoundingClientRect();
          out.push({
            tag: el.tagName.toLowerCase(),
            cls: (el.className || "").toString().slice(0, 70),
            text,
            visible: rect.width > 0 && rect.height > 0,
          });
        }
      }
      node = walker.nextNode();
    }
    return out.slice(0, 40);
  }, keys);
}

/** 候选触发点（返回可点的元素描述）。 */
async function candidates() {
  return page.evaluate(() => {
    const out = [];
    const push = (el, label) => {
      const rect = el.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;
      out.push({
        label,
        tag: el.tagName.toLowerCase(),
        cls: (el.className || "").toString().slice(0, 70),
        text: (el.innerText || "").trim().slice(0, 30),
        aria: el.getAttribute("aria-label") || "",
      });
    };
    document.querySelectorAll(".ant-dropdown-trigger").forEach((el, i) => push(el, `ant-dropdown-trigger#${i}`));
    document.querySelectorAll("[class*='avatar'], [class*='Avatar'], [class*='user']").forEach((el, i) => {
      if (el.tagName === "BUTTON" || el.getAttribute("role") === "button") push(el, `user#${i}`);
    });
    return out;
  });
}

const plan = [
  { name: "ant-dropdown-trigger#0", click: async () => (await page.$(".ant-dropdown-trigger"))?.click({ timeout: 5000 }) },
  { name: "更多", click: async () => (await page.getByRole("button", { name: "更多", exact: true }).first().click({ timeout: 5000 })) },
];

const report = { candidates: await candidates(), steps: [] };

for (const step of plan) {
  let ok = false;
  try {
    await step.click();
    ok = true;
  } catch (err) {
    report.steps.push({ step: step.name, ok: false, error: String(err).split("\n")[0] });
    continue;
  }
  await new Promise((r) => setTimeout(r, 1200));
  report.steps.push({
    step: step.name,
    ok,
    overlays: (await overlays()).map((item) => ({ ...item, text: scrub(item.text) })),
    hits: (await keywordHits(KEYWORDS)).map((item) => ({ ...item, text: scrub(item.text) })),
    bodyHead: scrub((await page.locator("body").innerText().catch(() => ""))).slice(0, 600),
  });
  // 关掉浮层，避免影响下一个候选
  await page.keyboard.press("Escape").catch(() => {});
  await new Promise((r) => setTimeout(r, 400));
}

report.finalHits = (await keywordHits(KEYWORDS)).map((item) => ({ ...item, text: scrub(item.text) }));
console.log(JSON.stringify(report, null, 2));

await context.close();

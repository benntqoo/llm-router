/**
 * 临时探测 6：**走一遍真实签到流程**（用户头像下拉 → 「每日签到」→ 面板 → 领取按钮），
 * 并抓下 `/signin/status` 与 `/signin/claim` 的真实响应体。
 *
 * 用途：拿到①签到面板的真实 DOM / 文案 ②`status` 字段的枚举语义
 * ③`/signin/claim` 响应体形状（回填 02 §7.4）。
 *
 * **脱敏**：所有输出文本过 scrub（手机号 / 长数字 id / 长不透明串 → 占位符）。
 *
 * 用法：LLMR_BROWSER_PROFILE=... node probe-signin-panel.mjs [url]
 */
import { chromium } from "playwright";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const url = process.argv[2] ?? "https://agent.minimax.cn/";
const profile = process.env.LLMR_BROWSER_PROFILE
  ? resolve(process.env.LLMR_BROWSER_PROFILE)
  : join(homedir(), ".llm-router", "browser-profile");

function scrub(text) {
  return String(text)
    .replace(/[\w.+-]+@[\w-]+\.[\w.]+/g, "<email>")
    .replace(/\b1[3-9]\d{9}\b/g, "<phone>")
    .replace(/\b\d{12,}\b/g, "<id>")
    .replace(/[A-Za-z0-9_-]{32,}/g, "<opaque>");
}

function scrubDeep(value, depth = 0) {
  if (depth > 8) return "<deep>";
  if (value === null || value === undefined) return value;
  if (typeof value === "number" || typeof value === "boolean") return value;
  if (typeof value === "string") return scrub(value).slice(0, 200);
  if (Array.isArray(value)) return value.slice(0, 30).map((v) => scrubDeep(v, depth + 1));
  if (typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = scrubDeep(v, depth + 1);
    return out;
  }
  return "<other>";
}

const context = await chromium.launchPersistentContext(profile, {
  headless: true,
  channel: "chrome",
  viewport: { width: 1440, height: 900 },
  locale: "zh-CN",
});
const page = context.pages()[0] ?? (await context.newPage());

const responses = [];
page.on("response", async (response) => {
  const target = response.url();
  if (!target.includes("/minimax-cloud/api/v1/signin/")) return;
  const path = target.replace(/^https?:\/\/[^/]+/, "").split("?")[0];
  let body = null;
  try {
    body = scrubDeep(await response.json());
  } catch {
    body = "<non-json>";
  }
  responses.push({ path, status: response.status(), method: response.request().method(), body });
});

await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
await page.waitForSelector("button", { timeout: 15_000 }).catch(() => {});
await new Promise((r) => setTimeout(r, 1500));

/** 打开用户头像下拉：第一个**有文字**的 `ant-dropdown-trigger`（附件那个是空的）。 */
async function openUserMenu() {
  const triggers = page.locator(".ant-dropdown-trigger");
  const total = await triggers.count();
  for (let i = 0; i < total; i += 1) {
    const trigger = triggers.nth(i);
    const text = ((await trigger.innerText().catch(() => "")) || "").trim();
    if (!text) continue;
    await trigger.click({ timeout: 5_000 }).catch(() => {});
    await new Promise((r) => setTimeout(r, 800));
    return { ok: true, index: i, triggerText: scrub(text) };
  }
  return { ok: false };
}

const menuResult = await openUserMenu();

/** 点「每日签到」菜单项。 */
let clickedEntry = false;
try {
  const item = page.getByText("每日签到", { exact: true }).first();
  if ((await item.count()) > 0 && (await item.isVisible())) {
    await item.click({ timeout: 5_000 });
    clickedEntry = true;
  }
} catch {
  clickedEntry = false;
}
await new Promise((r) => setTimeout(r, 1800));

/** 面板里的可见按钮 / 文案。 */
const panel = await page.evaluate(() => {
  const pick = (selector) =>
    Array.from(document.querySelectorAll(selector))
      .filter((n) => {
        const rect = n.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      })
      .map((n) => ({
        tag: n.tagName.toLowerCase(),
        text: (n.innerText || "").trim().slice(0, 60),
        aria: n.getAttribute("aria-label") || "",
        cls: (n.className || "").toString().slice(0, 60),
      }));
  return {
    buttons: pick("button"),
    dialogs: pick('[role="dialog"], [class*="modal"], [class*="drawer"], [class*="mask"]'),
    dialogText: Array.from(document.querySelectorAll('[role="dialog"], [class*="modal"]'))
      .map((n) => (n.innerText || "").trim().slice(0, 500))
      .filter(Boolean),
  };
});

console.log(JSON.stringify({ menuResult, clickedEntry, panel, responses }, null, 2));

await context.close();

/**
 * 临时探测：**登录态**下 `agent.minimax.cn` 的签到入口到底长什么样。
 *
 * 背景（2026-11-27 真机）：登录成功后 `/minimax-cloud/api/v1/signin/status`
 * 回了 **HTTP 200**，但辅轨按**文案候选**找不到签到入口（`no_signin_entry`）——
 * 需要把登录后页面上真实的按钮 / 入口 / 接口路径摸出来。
 *
 * **用持久化 profile**（应用那个，已登录）——**不用临时 profile**
 * （临时 profile 会是「新设备」，既拿不到登录态，也会打站点频控）。
 *
 * 用法：LLMR_BROWSER_PROFILE=... node probe-signedin.mjs [url]
 */
import { chromium } from "playwright";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const url = process.argv[2] ?? "https://agent.minimax.cn/";
const profile = process.env.LLMR_BROWSER_PROFILE
  ? resolve(process.env.LLMR_BROWSER_PROFILE)
  : join(homedir(), ".llm-router", "browser-profile");

if (!existsSync(profile)) {
  console.log(JSON.stringify({ error: "profile 不存在", profile }));
  process.exit(1);
}

/** 脱敏：长数字串 / 邮箱 / 手机号 / 长不透明串 → ***（别把账号信息打进日志）。 */
function scrub(text) {
  return String(text)
    .replace(/[\w.+-]+@[\w-]+\.[\w.]+/g, "<email>")
    .replace(/\b1[3-9]\d{9}\b/g, "<phone>")
    .replace(/\b\d{9,}\b/g, "<id>")
    .replace(/[A-Za-z0-9_-]{32,}/g, "<opaque>");
}

function scrubDeep(value, depth = 0) {
  if (depth > 6) return "<deep>";
  if (value === null || value === undefined) return value;
  if (typeof value === "number" || typeof value === "boolean") return value;
  if (typeof value === "string") return scrub(value).slice(0, 120);
  if (Array.isArray(value)) return value.slice(0, 20).map((v) => scrubDeep(v, depth + 1));
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

/** 抓到的接口响应（只留路径 + 状态码 + 脱敏后的体）。 */
const api = [];

function onResponse(response) {
  const target = response.url();
  if (!target.includes("/minimax-cloud/api/")) return;
  const path = target.replace(/^https?:\/\/[^/]+/, "").split("?")[0];
  api.push({ path, status: response.status() });
  if (path.includes("/signin/status")) {
    response
      .json()
      .then((body) => {
        api.push({ path: path + " [body]", body: scrubDeep(body) });
      })
      .catch(() => {});
  }
}
page.on("response", onResponse);

await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
await page.waitForSelector("button", { timeout: 15_000 }).catch(() => {});
await new Promise((r) => setTimeout(r, 1500));

/** 页面上所有可见按钮 / 链接的文字。 */
async function textsOf(selector) {
  try {
    return await page.$$eval(
      selector,
      (nodes) =>
        nodes
          .filter((n) => {
            const rect = n.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0;
          })
          .map((n) => ({
            tag: n.tagName.toLowerCase(),
            text: (n.innerText || n.textContent || "").trim().slice(0, 40),
            aria: n.getAttribute("aria-label") || "",
            title: n.getAttribute("title") || "",
          })),
    );
  } catch {
    return [];
  }
}

const buttons = await textsOf("button");
const links = await textsOf("a");

/** 含关键词的元素（含父级容器，取最小命中）。 */
const KEYWORDS = ["签到", "积分", "奖励", "福利", "领取", "每日", "连签", "魔粒", "签"];
const keywordHits = await page.evaluate((keys) => {
  const out = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (node) {
    const text = (node.textContent || "").trim();
    if (text && text.length <= 40 && keys.some((k) => text.includes(k))) {
      const el = node.parentElement;
      if (el) {
        const rect = el.getBoundingClientRect();
        out.push({
          tag: el.tagName.toLowerCase(),
          cls: (el.className || "").toString().slice(0, 80),
          text,
          visible: rect.width > 0 && rect.height > 0,
        });
      }
    }
    node = walker.nextNode();
  }
  return out.slice(0, 60);
}, KEYWORDS);

const bodyText = scrub(await page.locator("body").innerText().catch(() => "")).slice(0, 1500);

console.log(
  JSON.stringify(
    {
      url: page.url(),
      title: await page.title(),
      buttons,
      links,
      keywordHits,
      bodyText,
      api,
    },
    null,
    2,
  ),
);

await context.close();

/**
 * 临时探测：ModelScope（魔搭）**匿名态**页面结构 —— 为了定「登录完成」的判定信号。
 *
 * 关心四件事：
 * 1. 页面上有没有「登录」按钮（登录后会不会消失 → 判据 B 是否可用）；
 * 2. Cookie 的**名字**集合（登录后会多出会话 Cookie → 判据 C）；
 * 3. 页面自己打了哪些 API（有没有能判登录态的无副作用接口）；
 * 4. 登录入口长什么样（用户要点哪里）。
 *
 * **脱敏**：只输出 Cookie 的**名字**，绝不输出值。
 *
 * 用法：LLMR_BROWSER_PROFILE=... node probe-modelscope-login.mjs [url]
 */
import { chromium } from "playwright";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const url = process.argv[2] ?? "https://www.modelscope.cn/home";
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

const context = await chromium.launchPersistentContext(profile, {
  headless: true,
  channel: "chrome",
  viewport: { width: 1440, height: 900 },
  locale: "zh-CN",
});
const page = context.pages()[0] ?? (await context.newPage());

const apiPaths = [];
page.on("response", (response) => {
  const target = response.url();
  if (target.includes("modelscope.cn/api/")) {
    apiPaths.push({
      path: target.replace(/^https?:\/\/[^/]+/, "").split("?")[0],
      status: response.status(),
    });
  }
});

await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
await page.waitForSelector("button", { timeout: 15_000 }).catch(() => {});
await new Promise((r) => setTimeout(r, 3000));

/** 页面上可见的按钮 / 链接（文字截断 + 脱敏）。 */
const controls = await page.evaluate(() => {
  const pick = (selector) =>
    Array.from(document.querySelectorAll(selector))
      .filter((n) => {
        const rect = n.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      })
      .map((n) => ({
        tag: n.tagName.toLowerCase(),
        text: (n.innerText || "").trim().slice(0, 24),
        aria: n.getAttribute("aria-label") || "",
        cls: (n.className || "").toString().slice(0, 50),
      }));
  return { buttons: pick("button"), links: pick("a") };
});

/** Cookie 的**名字**（绝不输出值）。 */
const cookieNames = (await context.cookies()).map((c) => ({ name: c.name, domain: c.domain }));

/** 页面上含「登录 / 注册 / 账号」字样的元素。 */
const loginHints = await page.evaluate(() => {
  const keys = ["登录", "登陆", "注册", "账号", "登入"];
  const out = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (node) {
    const text = (node.textContent || "").trim();
    if (text && text.length <= 20 && keys.some((k) => text.includes(k))) {
      const el = node.parentElement;
      if (el) {
        const rect = el.getBoundingClientRect();
        out.push({
          tag: el.tagName.toLowerCase(),
          cls: (el.className || "").toString().slice(0, 60),
          text,
          visible: rect.width > 0 && rect.height > 0,
        });
      }
    }
    node = walker.nextNode();
  }
  return out.slice(0, 40);
});

console.log(
  JSON.stringify(
    {
      url: page.url(),
      title: await page.title(),
      cookieNames,
      apiPaths: apiPaths.slice(0, 30),
      loginHints,
      loginButtons: controls.buttons.filter((b) => /登录|登陆|注册|登入/.test(b.text + b.aria)),
      loginLinks: controls.links.filter((b) => /登录|登陆|注册|登入/.test(b.text + b.aria)),
      headerText: scrub(await page.locator("header").first().innerText().catch(() => "")).slice(0, 300),
      bodyHead: scrub(await page.locator("body").innerText().catch(() => "")).slice(0, 600),
    },
    null,
    2,
  ),
);

await context.close();

#!/usr/bin/env node
/**
 * 浏览器辅轨（T18；02 §7.4 §3）。
 *
 * **为什么存在**：MiniMax Code 的签到接口带 `x-timestamp` + `x-signature`（每次都变），
 * HAR 模板化无法重放，自己实现签名 = 逆向客户端私有协议（撞 AGENT.md §5.7）。
 * **让浏览器自己算签名**——页面自己的 JS 完成签名，我们只负责打开页面、点签到、
 * **读页面自己发出的网络响应**。
 *
 * **协议（CLI）**：Rust 每次起一个 Node 进程跑一个动作，**stdout 只吐一行 JSON**，
 * 用完即退——无端口占用、无孤儿进程、无本地 socket 鉴权问题。
 * 所有日志走 stderr（stdout 必须是干净 JSON）。
 *
 * | 动作 | 参数 | 返回 |
 * |---|---|---|
 * | `health` | — | `{nodeVersion, browserAvailable, browserChannel, profileDir, loggedIn}` |
 * | `login` | `<pageUrl>` | `{ok, loggedIn, message}`——**有头**浏览器，等用户手动登录 |
 * | `checkin` | `<pageUrl>` | `{ok, alreadyClaimed, points, day, code, message}` |
 *
 * **profile 与 §5.3 的边界**：持久化 profile 里的会话凭证**由浏览器自己加密保管**
 * （Windows DPAPI / macOS Keychain），**本脚本不读、不导、不打印**任何 cookie/token。
 */

import { chromium } from "playwright";
import { existsSync, mkdirSync, mkdtempSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import process from "node:process";

// ────────────────────────────── 常量 ──────────────────────────────

/**
 * 等页面**渲染完**（React 应用是前端渲染的——DOMContentLoaded 时按钮可能还没出现）。
 *
 * **必须等**：否则「登录按钮数量 == 0」会被误判成「已登录」——真机踩过：
 * 页面还没渲染就返回了 `loggedIn: true`（实际根本没登录）。
 *
 * 判据用「**页面上出现按钮**」（等 `networkidle` 太慢——该站有持续请求，
 * 实测要等 20 s 才超时），再留 800 ms 让首屏组件挂完。
 */
async function waitForRender(page) {
  await page.waitForSelector("button", { timeout: 15_000 }).catch(() => {});
  await sleep(800);
}

/** 动作超时（毫秒）。 */
const CHECKIN_TIMEOUT_MS = 90_000;

/**
 * 登录动作的超时（毫秒）——**默认 15 分钟**。
 *
 * 真人登录要输密码/收 2FA/走扫码，**5 分钟太紧**（真机踩过：用户还在输验证码，
 * 交接窗口就到期了）。可用 `LLMR_LOGIN_TIMEOUT_MS` 覆盖。
 */
function loginTimeoutMs() {
  const raw = Number(process.env.LLMR_LOGIN_TIMEOUT_MS);
  return Number.isFinite(raw) && raw > 0 ? raw : 900_000;
}

/** 登录成功判定：**连续两次**读到 0 个登录按钮，且至少等了这么久。 */
const LOGIN_STABLE_MS = 2_000;
const LOGIN_MIN_DWELL_MS = 5_000;

/** 默认页面地址（国内站）。 */
const DEFAULT_PAGE_URL = "https://agent.minimax.cn/";

/** 签到入口的候选文案（**未实测**：页面上中文文案可能变，逐个试）。 */
const SIGNIN_TEXTS = [
  "每日签到",
  "立即签到",
  "去签到",
  "签到",
  "领取奖励",
  "每日奖励",
  "领积分",
  "立即领取",
  "领取",
];

/** 「已经领过」的判定文案。 */
const CLAIMED_TEXTS = ["已签到", "已领取", "明日再来", "已完成"];

/**
 * 签到面板里「今天已经领过了」的文案（**真机实测，2026-11-27**）。
 *
 * 点开签到面板后，今天那一格的按钮会变成 **「今日已签到」**（且是禁用态
 * `cursor-default`）——**这就是「已领」的 DOM 信号**，比任何接口都直接。
 */
const ALREADY_CLAIMED_TEXTS = [
  "今日已签到",
  "今天已签到",
  "今日已领取",
  "已签到",
  "明日再来",
  "已完成签到",
];

/**
 * 用户头像下拉菜单里的**签到入口文案**（真机实测）。
 *
 * 入口**不在首页**——首页一个字都没有「签到」；它在**用户头像下拉**里：
 * 设置 / 用量 / **每日签到** / 问题反馈 / 联系我们 / 了解更多 / 退出登录。
 */
const MENU_SIGNIN_TEXTS = ["每日签到"];

/**
 * 页面上**直接**摆着的签到入口（首页直接放着的版本；实测本站在首页没有）。
 */
const DIRECT_SIGNIN_TEXTS = ["每日签到", "立即签到", "去签到"];

/**
 * 签到面板里的**领取按钮**文案候选（**未实测**——真机那天已经领过，按钮是禁用态，
 * 拿不到真实按钮文案；候选按常见写法排）。
 */
const CLAIM_BUTTON_TEXTS = [
  "立即签到",
  "去签到",
  "签到领取",
  "领取奖励",
  "立即领取",
  "领取",
];

/**
 * 用户头像下拉的触发选择器（实测：`.ant-dropdown-trigger`）。
 *
 * **坑**：页面上有**两个** `.ant-dropdown-trigger`——第一个是头像（**有文字**：
 * 用户名 + 套餐），第二个是「添加附件或技能」（**没有文字**）。按顺序取第一个
 * 会被附件那个坑到，所以**只认有文字的那个**。
 */
const USER_MENU_TRIGGER = ".ant-dropdown-trigger";

/** 登录入口的候选文案（**未登录的可靠信号**——实测未登录时页面上有两个「登录」按钮）。 */
const LOGIN_TEXTS = ["登录", "登 录", "立即登录", "Sign in", "Log in"];

/**
 * 账号密码登录的候选文案（**优先这条路径**）。
 *
 * 真机踩过（2026-11-27）：短信/验证码登录被站点频控——点「获取验证码」时提示
 * 「操作频繁」，短信根本发不出去。账号密码登录**没有这个频控**，
 * 所以辅轨要主动把登录方式切过去（用户自己点也得先找 tab）。
 */
const PASSWORD_LOGIN_TEXTS = [
  "账号密码登录",
  "密码登录",
  "密码登陆",
  "账号密码",
  "Password login",
  "Password Login",
];

/** 站点限流提示（**登录通道被打爆**的信号——真机踩过：短信通道报「操作频繁」）。 */
const RATE_LIMIT_TEXTS = [
  "操作频繁",
  "请求过于频繁",
  "访问过于频繁",
  "调用过于频繁",
  "发送过于频繁",
];

// ────────────────────────────── 工具 ──────────────────────────────

function log(...args) {
  console.error("[sidecar]", ...args);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** 持久化 profile 目录（**不进 git**）。 */
function profileDir() {
  const fromEnv = process.env.LLMR_BROWSER_PROFILE;
  const dir = fromEnv && fromEnv.trim()
    ? resolve(fromEnv)
    : join(homedir(), ".llm-router", "browser-profile");
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
  return dir;
}

/**
 * 启动浏览器：**优先系统 Chrome**（`channel: "chrome"`），没有则回落 Playwright 自带
 * chromium——**不强制下载 150MB**。
 *
 * **不做任何指纹伪装**（AGENT.md §5.7）：不加反检测开关、不改 UA、不隐藏自动化特征。
 */
async function launch({ headless, profile = profileDir() } = {}) {
  const attempts = [
    { channel: "chrome", label: "chrome" },
    { channel: undefined, label: "chromium" },
  ];
  let lastError = null;
  for (const attempt of attempts) {
    try {
      const context = await chromium.launchPersistentContext(profile, {
        headless,
        channel: attempt.channel,
        viewport: { width: 1440, height: 900 },
        locale: "zh-CN",
      });
      log(`浏览器已启动：${attempt.label}（headless=${headless}）`);
      return { context, channel: attempt.label };
    } catch (err) {
      lastError = err;
      log(`浏览器启动失败（${attempt.label}）：${String(err).split("\n")[0]}`);
    }
  }
  throw new Error(`没有可用浏览器：${String(lastError).split("\n")[0]}`);
}

/** 在页面里找一个**可见**的、命中候选文案的元素；返回文案（没找到返回 `null`）。 */
async function findVisibleText(page, texts) {
  for (const text of texts) {
    try {
      const locator = page.getByText(text, { exact: false }).first();
      if ((await locator.count()) === 0) {
        continue;
      }
      if (!(await locator.isVisible())) {
        continue;
      }
      return text;
    } catch {
      // 页面还在加载 / 选择器不支持 → 当作没找到
    }
  }
  return null;
}

/**
 * 打开**用户头像下拉菜单**。
 *
 * **坑**：页面上有两个 `.ant-dropdown-trigger`——第一个是头像（有文字），
 * 第二个是「添加附件或技能」（没文字）。**只认有文字的那个**。
 */
async function openUserMenu(page) {
  const triggers = page.locator(USER_MENU_TRIGGER);
  const total = await triggers.count();
  for (let index = 0; index < total; index += 1) {
    const trigger = triggers.nth(index);
    const text = ((await trigger.innerText().catch(() => "")) || "").trim();
    if (!text) {
      continue;
    }
    await trigger.click({ timeout: 5_000 }).catch(() => {});
    await sleep(700);
    log("已打开用户头像下拉菜单");
    return true;
  }
  return false;
}

/**
 * 打开**签到面板**。
 *
 * 真机实测（2026-11-27）：**入口不在首页**——首页没有任何「签到」文案；
 * 它在**用户头像下拉菜单**里的「每日签到」项。
 * 先试「首页直接摆着」（别的版本可能是这样），再走头像下拉。
 */
async function openSigninEntry(page) {
  // 候选 1：页面上直接摆着的入口
  if (await clickVisibleText(page, DIRECT_SIGNIN_TEXTS)) {
    await sleep(1_000);
    return true;
  }
  // 候选 2：用户头像下拉 → 「每日签到」
  if (!(await openUserMenu(page))) {
    return false;
  }
  const hit = await clickVisibleText(page, MENU_SIGNIN_TEXTS);
  if (!hit) {
    await page.keyboard.press("Escape").catch(() => {});
    await sleep(300);
    return false;
  }
  await sleep(1_200); // 等签到面板渲染
  return true;
}

/**
 * 在签到面板里点「领取 / 签到」按钮。
 *
 * **注意**：面板里「今日已签到」这个**禁用**按钮也含「签到」二字——
 * 调用方必须**先判过「已领」**再调这里，否则会点到禁用按钮。
 */
async function clickClaimButton(page) {
  for (const text of CLAIM_BUTTON_TEXTS) {
    try {
      const locator = page.getByText(text, { exact: false }).first();
      if ((await locator.count()) === 0) {
        continue;
      }
      if (!(await locator.isVisible())) {
        continue;
      }
      // 禁用态按钮点了也没用——提前排除
      const disabled = await locator
        .evaluate((el) => {
          const button = el.closest("button") ?? el;
          return button.disabled === true || button.getAttribute("aria-disabled") === "true";
        })
        .catch(() => false);
      if (disabled) {
        log(`「${text}」是禁用态（今天大概已经领过了）`);
        continue;
      }
      await locator.click({ timeout: 8_000 });
      log(`已点击签到按钮：「${text}」`);
      return true;
    } catch (err) {
      log(`点击「${text}」失败：${String(err).split("\n")[0]}`);
    }
  }
  return false;
}

/** 递归找第一个「键名命中且值是数字」的字段。 */
function findNumber(value, keys, depth = 0) {
  if (depth > 6 || value === null || typeof value !== "object") {
    return null;
  }
  for (const key of keys) {
    const hit = value[key];
    if (typeof hit === "number" && Number.isFinite(hit)) {
      return hit;
    }
  }
  for (const item of Object.values(value)) {
    const found = findNumber(item, keys, depth + 1);
    if (found !== null) {
      return found;
    }
  }
  return null;
}

/** 递归找第一个「键名命中且值是字符串」的字段。 */
function findString(value, keys, depth = 0) {
  if (depth > 6 || value === null || typeof value !== "object") {
    return null;
  }
  for (const key of keys) {
    const hit = value[key];
    if (typeof hit === "string" && hit.trim()) {
      return hit.trim();
    }
  }
  for (const item of Object.values(value)) {
    const found = findString(item, keys, depth + 1);
    if (found !== null) {
      return found;
    }
  }
  return null;
}

/**
 * status 接口里「今天已领取」的判定：**今天那一条 `status == 3`**。
 *
 * 真机实测（2026-11-27）：7 天的 `status` 全是 3，且面板今天那格显示
 * 「今日已签到」→ **3 = 已领取**。没领过的天应该是别的状态值。
 */
function isTodayClaimed(body) {
  if (!body || typeof body !== "object") {
    return false;
  }
  const data = body.data && typeof body.data === "object" ? body.data : body;
  if (!Array.isArray(data.days)) {
    return false;
  }
  const today = data.days.find((item) => item && item.is_today === true);
  return Boolean(today && today.status === 3);
}

/**
 * 「今天已经领过了」的判定（**幂等 ≠ 失败**）。
 *
 * 真实形状（真机实测，2026-11-27）：`base_resp.status_code == 0` 表示业务成功；
 * 已领过的情况落在 `base_resp.status_msg` 或业务 `message` 的中文文案里。
 */
function detectAlreadyClaimed(body) {
  if (!body || typeof body !== "object") {
    return false;
  }
  if (body.claim_result === 2 || body.claimResult === 2) {
    return true;
  }
  const baseResp = body.base_resp ?? body.baseResp;
  if (baseResp && typeof baseResp === "object") {
    const msg = String(baseResp.status_msg ?? baseResp.statusMsg ?? "");
    if (/已签到|已领取|重复|already/i.test(msg)) {
      return true;
    }
  }
  const code = findString(body, ["code", "Code", "error_code", "errorCode"]);
  if (code && /already|claimed|repeat/i.test(code)) {
    return true;
  }
  const message = findString(body, ["message", "Message", "msg", "error", "detail", "status_msg"]);
  if (message && CLAIMED_TEXTS.some((text) => message.includes(text))) {
    return true;
  }
  return false;
}

/**
 * 业务成功判定（**看业务字段，不只看状态码**）。
 *
 * **MiniMax 约定（AGENT.md 已知坑 1）**：HTTP 200 **也可能是业务失败**——
 * 成功看 `base_resp.status_code == 0`。带了 `base_resp` 就**只认它**，
 * 不再往下滑去找别的成功标志（否则会把业务失败误判成成功）。
 */
function detectSuccess(body) {
  if (!body || typeof body !== "object") {
    return false;
  }
  const baseResp = body.base_resp ?? body.baseResp;
  if (baseResp && typeof baseResp === "object" && typeof baseResp.status_code === "number") {
    return baseResp.status_code === 0;
  }
  if (body.success === true || body.Success === true) {
    return true;
  }
  if (body.code === 0 || body.Code === 0) {
    return true;
  }
  const code = findString(body, ["code", "Code", "error_code", "errorCode"]);
  if (code && /already|claimed|repeat/i.test(code)) {
    return true;
  }
  const message = findString(body, ["message", "Message", "msg"]);
  if (message && CLAIMED_TEXTS.some((text) => message.includes(text))) {
    return true;
  }
  return false;
}

/**
 * 从签到响应里提取积分与连签天数。
 *
 * **status 接口的真实形状（真机实测，2026-11-27）**：
 * ```json
 * {"data":{"scene":3,"days":[
 *   {"day_no":1,"points":400,"status":3,"is_today":false,"bonus_points":0},
 *   {"day_no":4,"points":1000,"status":3,"is_today":false,"bonus_points":0},
 *   {"day_no":7,"points":1000,"status":3,"is_today":true,"bonus_points":0}]},
 *  "base_resp":{"status_code":0,"status_msg":"ok"}}
 * ```
 * → **今天那一条** = `days` 里 `is_today == true` 的那条：`points` = 当日积分，
 * `day_no` = 连签第几天；`status == 3` = **已领取**（实测：面板显示「今日已签到」）。
 */
function extractReward(body) {
  if (!body || typeof body !== "object") {
    return { points: null, day: null };
  }
  const data = body.data && typeof body.data === "object" ? body.data : body;

  // 形状 1：`days[]` 数组（status 接口）
  if (Array.isArray(data.days)) {
    const today = data.days.find((item) => item && item.is_today === true);
    if (today) {
      const points =
        typeof today.points === "number" ? today.points : findNumber(today, ["points", "bonus_points"]);
      const day = typeof today.day_no === "number" ? today.day_no : findNumber(today, ["day_no", "day"]);
      return { points: points ?? null, day: day ?? null };
    }
  }

  // 形状 2：平铺字段（claim 接口）
  const points = findNumber(data, [
    "points",
    "point",
    "amount",
    "credits",
    "credit",
    "reward",
    "rewardPoints",
    "addPoints",
    "gain",
    "bonus_points",
  ]);
  const day = findNumber(data, [
    "day_no",
    "day",
    "dayCount",
    "continuousDays",
    "continuousDay",
    "streak",
    "totalDays",
  ]);
  return { points, day };
}

/** 页面里能不能看到签到入口。 */
async function hasSigninEntry(page) {
  for (const text of SIGNIN_TEXTS) {
    try {
      const count = await page.getByText(text, { exact: false }).count();
      if (count > 0) {
        return true;
      }
    } catch {
      // 页面还在加载 / 选择器不支持 → 当作没找到
    }
  }
  return false;
}

/**
 * 页面上还有几个「登录」按钮（**> 0 = 没登录**）。
 *
 * 实测（2026-11-27，匿名态 agent.minimax.cn）：页面上有两个 `BUTTON` 文本为「登录」；
 * 登录成功后这两个按钮会消失（换成用户信息）——所以它是**最可靠的未登录信号**。
 */
async function loginButtonCount(page) {
  let total = 0;
  for (const text of LOGIN_TEXTS) {
    try {
      total += await page.locator(`button:has-text("${text}")`).count();
      total += await page.locator(`a:has-text("${text}")`).count();
    } catch {
      // 选择器不支持 → 当作没有
    }
  }
  return total;
}

/**
 * **整个浏览器上下文**里还有几个「登录」按钮。
 *
 * 登录流程可能开**新标签页**（点「登录」→ 弹出登录页 → 登录完回到原页）——
 * 只看最初那一页会把「原页的登录按钮还在」误当成没登录。
 */
async function contextLoginButtonCount(context) {
  let total = 0;
  for (const page of context.pages()) {
    total += await loginButtonCount(page);
  }
  return total;
}

/** 整个上下文里有没有签到入口（**已登录的正面信号**）。 */
async function contextHasSigninEntry(context) {
  for (const page of context.pages()) {
    if (await hasSigninEntry(page)) {
      return true;
    }
  }
  return false;
}

/**
 * 点一个「文案命中且可见」的元素（tab / 链接 / 按钮都算）。
 *
 * 返回点到的文案；没找到返回 `null`。
 */
async function clickVisibleText(page, texts) {
  for (const text of texts) {
    try {
      const locator = page.getByText(text, { exact: false }).first();
      if ((await locator.count()) === 0) {
        continue;
      }
      if (!(await locator.isVisible())) {
        continue;
      }
      await locator.click({ timeout: 5_000 });
      log(`已点击：「${text}」`);
      return text;
    } catch (err) {
      log(`点击「${text}」失败：${String(err).split("\n")[0]}`);
    }
  }
  return null;
}

/**
 * 打开登录弹窗：点页面上文案**正好是**「登录」的按钮。
 *
 * **精确匹配**是有意的——用包含匹配会先点到「账号密码登录」这种长文案。
 */
async function openLoginDialog(context) {
  for (const page of context.pages()) {
    for (const name of ["登录", "立即登录", "Sign in", "Log in"]) {
      try {
        const button = page.getByRole("button", { name, exact: true }).first();
        if ((await button.count()) === 0) {
          continue;
        }
        if (!(await button.isVisible())) {
          continue;
        }
        await button.click({ timeout: 5_000 });
        log(`已打开登录弹窗（「${name}」）`);
        return true;
      } catch (err) {
        log(`打开登录弹窗失败（「${name}」）：${String(err).split("\n")[0]}`);
      }
    }
  }
  return false;
}

/**
 * 切到「账号密码登录」tab。
 *
 * 返回是否点到（`false` = 页面上还没出现这个 tab——可能登录弹窗还没开）。
 */
async function switchToPasswordLogin(context) {
  for (const page of context.pages()) {
    const hit = await clickVisibleText(page, PASSWORD_LOGIN_TEXTS);
    if (hit) {
      log(`登录方式已切到账号密码（「${hit}」）——避开短信通道的频控`);
      return true;
    }
  }
  return false;
}

/** 上下文里有没有**可见的密码输入框**（= 已经是密码登录形态）。 */
async function passwordInputVisible(context) {
  for (const page of context.pages()) {
    try {
      const input = page.locator('input[type="password"]').first();
      if ((await input.count()) === 0) {
        continue;
      }
      if (await input.isVisible()) {
        return true;
      }
    } catch {
      // 页面还在加载 → 当作没看到
    }
  }
  return false;
}

/** 页面上有没有站点限流提示（「操作频繁」——真机踩过的短信通道频控）。 */
async function detectRateLimit(context) {
  for (const page of context.pages()) {
    try {
      const body = await page.locator("body").innerText({ timeout: 2_000 });
      const hit = RATE_LIMIT_TEXTS.find((text) => body.includes(text));
      if (hit) {
        return hit;
      }
    } catch {
      // 页面还在加载 / 没有 body → 当作没限流
    }
  }
  return null;
}

// ────────────────────────────── 动作 ──────────────────────────────

/** 探活 + 报浏览器可用性。 */
async function health() {
  const result = {
    nodeVersion: process.version,
    browserAvailable: false,
    browserChannel: "",
    profileDir: profileDir(),
    loggedIn: null,
  };
  // 用**临时 profile**探活：不碰真实 profile（避免与正在跑的签到抢 profile 锁）
  const tempProfile = mkdtempSync(join(tmpdir(), "llmr-sidecar-"));
  try {
    const { context, channel } = await launch({ headless: true, profile: tempProfile });
    result.browserAvailable = true;
    result.browserChannel = channel;
    await context.close();
  } catch (err) {
    log(`探活失败：${String(err).split("\n")[0]}`);
  }
  return result;
}

/** 拉起**有头**浏览器，等用户手动登录；登录态写进持久化 profile。 */
async function login(pageUrl) {
  const url = pageUrl || DEFAULT_PAGE_URL;
  const { context } = await launch({ headless: false });
  try {
    const page = context.pages()[0] ?? (await context.newPage());

    // **登录态的硬信号**：页面自己的 API 请求里 `user_id` 是不是还是 0。
    // 实测（2026-11-27，匿名态）：`/minimax-cloud/api/v1/config?...&user_id=0`——
    // 登录后同一接口会带真实 user_id。这比「按钮消失」更硬，不依赖文案。
    const seen = { userId: 0, signinStatus: null };
    context.on("request", (req) => {
      const target = req.url();
      if (!target.includes("/minimax-cloud/api/v1/")) {
        return;
      }
      const match = target.match(/[?&]user_id=(\d+)/);
      if (match) {
        const id = Number(match[1]);
        if (id > 0) {
          seen.userId = id;
        }
      }
    });
    context.on("response", async (response) => {
      if (!response.url().includes("/minimax-cloud/api/v1/signin/status")) {
        return;
      }
      try {
        seen.signinStatus = await response.json();
      } catch {
        seen.signinStatus = null;
      }
    });

    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
    await waitForRender(page);

    // **自动打开登录弹窗 + 切到「账号密码登录」**——
    // 真机踩过：短信/验证码登录被站点频控（「操作频繁」，短信根本发不出去）。
    // 账号密码路径没有这个频控，辅轨要主动把用户送到那条路上。
    if (await openLoginDialog(context)) {
      await sleep(800);
    }
    /** 密码 tab 只点一次（点多了会和用户抢操作）。 */
    let passwordTabTried = await switchToPasswordLogin(context);

    const started = Date.now();
    const deadline = started + loginTimeoutMs();
    let loggedIn = false;
    /** 连续读到「没有登录按钮」的起点。 */
    let zeroSince = null;
    /** 连续看到「签到入口」的起点（已登录的正面信号）。 */
    let signinSince = null;
    while (Date.now() < deadline) {
      // 站点限流（短信通道被打爆）→ 立刻返回，别让用户白等 15 分钟
      const limited = await detectRateLimit(context);
      if (limited) {
        return {
          ok: false,
          loggedIn: false,
          code: "rate_limited",
          message: `站点提示「${limited}」——登录通道被限流了；等几分钟再试，或改用账号密码登录`,
        };
      }

      // 登录弹窗可能是用户自己点开的 → 没切到过就再试一次
      if (!passwordTabTried) {
        passwordTabTried = await switchToPasswordLogin(context);
      }

      // 判据 A（**硬信号**）：页面自己的 API 已经带上真实 user_id
      if (seen.userId > 0) {
        loggedIn = true;
        log(`检测到登录成功（user_id=${seen.userId}）`);
        break;
      }

      // 判据 B：整个上下文都没有「登录」按钮了（**连续两次**读到 0 + 最短停留）
      const buttons = await contextLoginButtonCount(context);
      if (buttons === 0) {
        if (zeroSince === null) {
          zeroSince = Date.now();
        } else if (
          Date.now() - zeroSince >= LOGIN_STABLE_MS &&
          Date.now() - started >= LOGIN_MIN_DWELL_MS
        ) {
          loggedIn = true;
          log("检测到登录成功（登录按钮消失）");
          break;
        }
      } else {
        zeroSince = null;
      }

      // 判据 C：看到**签到入口** = 已登录的正面信号（连签面板只有登录后才出现）
      if (await contextHasSigninEntry(context)) {
        if (signinSince === null) {
          signinSince = Date.now();
        } else if (Date.now() - signinSince >= LOGIN_STABLE_MS) {
          loggedIn = true;
          log("检测到登录成功（出现签到入口）");
          break;
        }
      } else {
        signinSince = null;
      }

      if (context.pages().length === 0) {
        // 用户关了窗口 → 认为已完成交接
        break;
      }
      await sleep(1_000);
    }
    return {
      ok: loggedIn,
      loggedIn,
      code: loggedIn ? null : "login_timeout",
      message: loggedIn
        ? "登录完成，登录态已存进本地浏览器 profile"
        : (await passwordInputVisible(context))
          ? "没等到登录完成——登录框还开着（密码没提交、验证码没过，或站点仍在限流）"
          : "没等到登录完成——可能没登录成功，或页面改版了",
    };
  } finally {
    await context.close().catch(() => {});
  }
}

/** 打开签到页 → 触发签到 → **读页面自己的网络响应**取结果。 */
async function checkin(pageUrl) {
  const url = pageUrl || DEFAULT_PAGE_URL;
  const { context } = await launch({ headless: true });
  try {
    const page = context.pages()[0] ?? (await context.newPage());

    /** 抓到的响应（只留判签到需要的那几个）。 */
    const captured = { claim: null, status: null };

    const capture = (kind) => async (response) => {
      const target = response.url();
      if (!target.includes(`/signin/${kind}`)) {
        return;
      }
      if (captured[kind]) {
        return;
      }
      let body = null;
      try {
        body = await response.json();
      } catch {
        body = null;
      }
      captured[kind] = { httpStatus: response.status(), body };
      log(`抓到 ${kind} 响应：HTTP ${response.status()}`);
    };

    page.on("response", capture("claim"));
    page.on("response", capture("status"));

    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
    await waitForRender(page);

    // **先判登录态**：没登录就别找签到入口了（省时间，提示也更准）
    if ((await loginButtonCount(page)) > 0) {
      return {
        ok: false,
        alreadyClaimed: false,
        points: null,
        day: null,
        code: "not_logged_in",
        message: "没登录——请先点「登录」在弹出的浏览器里登录一次",
      };
    }

    // **打开签到面板**——真机实测（2026-11-27）：入口**不在首页**，
    // 在**用户头像下拉**菜单里的「每日签到」项。
    const opened = await openSigninEntry(page);
    if (!opened) {
      // 没找到签到入口 —— 分不清是「没登录」还是「页面改版」
      const loggedOut = await looksLoggedOut(page);
      return {
        ok: false,
        alreadyClaimed: false,
        points: null,
        day: null,
        code: loggedOut ? "not_logged_in" : "no_signin_entry",
        message: loggedOut
          ? "没登录——请先点「登录」在弹出的浏览器里登录一次"
          : "页面上没找到签到入口（页面可能改版了）",
      };
    }

    // **「今天已经领过了」**——**幂等 ≠ 失败**，判成功。
    //   DOM 信号（优先）：面板里今天的按钮变成「今日已签到」（禁用态）；
    //   接口信号（兜底）：status 接口里今天那一条 `status == 3`（= 已领取）。
    const statusBody = captured.status?.body ?? null;
    const alreadyText = await findVisibleText(page, ALREADY_CLAIMED_TEXTS);
    if (alreadyText || isTodayClaimed(statusBody)) {
      const { points: dayPoints, day } = extractReward(statusBody);
      return {
        ok: true,
        alreadyClaimed: true,
        // 本次**没拿到**积分（是之前领的）→ 不回写额度池，避免重复计数
        points: null,
        day,
        code: null,
        message: day
          ? `今天已经领过了（连签第 ${day} 天${dayPoints ? `，当日积分 ${dayPoints}` : ""}）`
          : "今天已经领过了",
      };
    }

    // 点「领取 / 签到」按钮（**只有今天还没领时才有可点的按钮**）
    const clicked = await clickClaimButton(page);
    if (!clicked && !captured.claim) {
      return {
        ok: false,
        alreadyClaimed: false,
        points: null,
        day: null,
        code: "no_signin_entry",
        message: "签到面板里没找到可点的「签到 / 领取」按钮（今天大概已经领过了，或页面改版了）",
      };
    }

    // 等 claim 响应（**结果以网络响应为准**，不解析 DOM 里的数字）
    const deadline = Date.now() + CHECKIN_TIMEOUT_MS;
    while (Date.now() < deadline && !captured.claim) {
      await sleep(500);
    }

    if (!captured.claim) {
      return {
        ok: false,
        alreadyClaimed: false,
        points: null,
        day: null,
        code: "no_claim_response",
        message: "点了签到，但没抓到 /signin/claim 的响应",
      };
    }

    const { body, httpStatus } = captured.claim;
    const alreadyClaimed = detectAlreadyClaimed(body);
    const success = detectSuccess(body) || alreadyClaimed;
    const { points, day } = extractReward(body);
    const message =
      findString(body, ["message", "Message", "msg", "detail"]) ??
      (alreadyClaimed ? "今天已经领过了" : success ? "签到成功" : `签到未成功（HTTP ${httpStatus}）`);

    return {
      ok: success,
      alreadyClaimed,
      points,
      day,
      code: success ? null : "claim_failed",
      message,
    };
  } finally {
    await context.close().catch(() => {});
  }
}

/** 粗判是否未登录（页面里有登录按钮 / 跳到了登录页）。 */
async function looksLoggedOut(page) {
  try {
    if (page.url().includes("login") || page.url().includes("signin")) {
      return true;
    }
    return (await loginButtonCount(page)) > 0;
  } catch {
    // 判不出来 → 交给上层按「没找到签到入口」处理
  }
  return false;
}

// ────────────────────────────── 入口 ──────────────────────────────

const ACTIONS = {
  health: () => health(),
  login: (args) => login(args[0]),
  checkin: (args) => checkin(args[0]),
};

async function main() {
  const action = process.argv[2];
  const handler = ACTIONS[action];
  if (!handler) {
    console.log(
      JSON.stringify({
        ok: false,
        code: "unknown_action",
        message: `未知动作：${action ?? "(空)"}（可用：${Object.keys(ACTIONS).join(" / ")}）`,
      }),
    );
    process.exitCode = 2;
    return;
  }
  try {
    const result = await handler(process.argv.slice(3));
    console.log(JSON.stringify(result));
  } catch (err) {
    console.log(
      JSON.stringify({
        ok: false,
        code: "sidecar_error",
        message: String(err?.message ?? err).split("\n")[0],
      }),
    );
    process.exitCode = 1;
  }
}

await main();

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
const HEALTH_TIMEOUT_MS = 30_000;
const LOGIN_TIMEOUT_MS = 300_000;
const CHECKIN_TIMEOUT_MS = 90_000;

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

/** 登录入口的候选文案（**未登录的可靠信号**——实测未登录时页面上有两个「登录」按钮）。 */
const LOGIN_TEXTS = ["登录", "登 录", "立即登录", "Sign in", "Log in"];

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

/** 在页面里找一个可点的签到入口；返回是否点了。 */
async function clickSignin(page) {
  for (const text of SIGNIN_TEXTS) {
    try {
      const locator = page.getByText(text, { exact: false }).first();
      if ((await locator.count()) === 0) {
        continue;
      }
      await locator.click({ timeout: 8_000 });
      log(`已点击签到入口：「${text}」`);
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

/** 「今天已经领过了」的判定（**幂等 ≠ 失败**）。 */
function detectAlreadyClaimed(body) {
  if (!body || typeof body !== "object") {
    return false;
  }
  if (body.claim_result === 2 || body.claimResult === 2) {
    return true;
  }
  const code = findString(body, ["code", "Code", "error_code", "errorCode"]);
  if (code && /already|claimed|repeat/i.test(code)) {
    return true;
  }
  const message = findString(body, ["message", "Message", "msg", "error", "detail"]);
  if (message && CLAIMED_TEXTS.some((text) => message.includes(text))) {
    return true;
  }
  return false;
}

/** 业务成功判定（**看业务字段，不只看状态码**）。 */
function detectSuccess(body) {
  if (!body || typeof body !== "object") {
    return false;
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

/** 从 claim 响应里提取积分与连签天数。 */
function extractReward(body) {
  const points = findNumber(body, [
    "points",
    "point",
    "amount",
    "credits",
    "credit",
    "reward",
    "rewardPoints",
    "addPoints",
    "gain",
  ]);
  const day = findNumber(body, [
    "day",
    "days",
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
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60_000 });
    await waitForRender(page);

    const started = Date.now();
    const deadline = started + LOGIN_TIMEOUT_MS;
    let loggedIn = false;
    let zeroSince = null;
    while (Date.now() < deadline) {
      // 登录成功的信号：**「登录」按钮消失**（实测未登录时页面上有两个）
      const count = await loginButtonCount(page);
      if (count === 0 && !(await hasSigninEntry(page))) {
        if (zeroSince === null) {
          zeroSince = Date.now();
        } else if (
          Date.now() - zeroSince >= LOGIN_STABLE_MS &&
          Date.now() - started >= LOGIN_MIN_DWELL_MS
        ) {
          // **连续两次读到 0** 且已过最短停留时间 → 判为登录成功
          loggedIn = true;
          break;
        }
      } else {
        zeroSince = null;
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
      message: loggedIn
        ? "登录完成，登录态已存进本地浏览器 profile"
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

    // 再看状态接口：已经领过就直接返回（**幂等 ≠ 失败**）
    if (captured.status && detectAlreadyClaimed(captured.status.body)) {
      const { day } = extractReward(captured.status.body);
      return {
        ok: true,
        alreadyClaimed: true,
        points: null,
        day,
        code: null,
        message: "今天已经领过了",
      };
    }

    const clicked = await clickSignin(page);
    if (!clicked) {
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

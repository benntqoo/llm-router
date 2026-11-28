/**
 * 声明式站点的**纯函数**（T25；02 §7.6）。
 *
 * 不碰 Tauri、不碰 DOM —— 可以用 `node --test` 直接跑单测
 * （与 `src/checkin/schedule.ts` 同一路子）。
 *
 * 关键语义：
 * - **占位符只有两个**：`{{token}}`（登录态，取自加密保险库）与 `{{today}}`（当天日期）。
 *   别的名字一律不认（后端也会拒），**不做表达式引擎**——免得 spec 变成脚本。
 * - **前端校验只是提前拦一道**，后端仍会全量校验（前端校验能过、后端拒了 → 以后端为准）。
 */

import type { SpecSite } from "./types";

/** 支持的占位符名。 */
export const SUPPORTED_PLACEHOLDERS = ["token", "today"] as const;

/** 表单里的一项（用来拼 spec）。 */
export type SpecForm = {
  id: string;
  name: string;
  method: string;
  url: string;
  /** 登录态（明文，只在提交那一刻进保险库）。 */
  secret: string;
  /** 期望状态码（空 = 不校验状态码）。 */
  assertStatus: string;
  /** JSON 路径断言（每行一条 `路径` 或 `路径=期望值`）。 */
  jsonPaths: string;
  /** 「本次拿到」的字段路径。 */
  rewardPath: string;
  /** 「本次拿到」的单位。 */
  rewardUnit: string;
  /** 「当前余额」的查询地址。 */
  balanceUrl: string;
  /** 「当前余额」的字段路径。 */
  balancePath: string;
  /** 「当前余额」的单位。 */
  balanceUnit: string;
};

/** 空白表单。 */
export function emptyForm(): SpecForm {
  return {
    id: "",
    name: "",
    method: "POST",
    url: "",
    secret: "",
    assertStatus: "200",
    jsonPaths: "",
    rewardPath: "",
    rewardUnit: "point",
    balanceUrl: "",
    balancePath: "",
    balanceUnit: "point",
  };
}

/** 从一段文本里抽出 `{{name}}` 的 name。 */
export function placeholdersIn(text: string): string[] {
  const out: string[] = [];
  const pattern = /\{\{\s*([^{}]*?)\s*\}\}/g;
  let match = pattern.exec(text);
  while (match) {
    const name = match[1];
    if (name && !out.includes(name)) {
      out.push(name);
    }
    match = pattern.exec(text);
  }
  return out;
}

/** 解析一条 JSON 路径断言（`路径` 或 `路径=期望值`）。 */
export function parseJsonPathRule(raw: string): { path: string; equals?: unknown } | null {
  const text = raw.trim();
  if (!text) {
    return null;
  }
  const index = text.indexOf("=");
  if (index < 0) {
    return { path: text };
  }
  const path = text.slice(0, index).trim();
  const value = text.slice(index + 1).trim();
  if (!path) {
    return null;
  }
  // 期望值：能当 JSON 就当 JSON（`true` / `123` / `"x"`），否则当字符串
  try {
    return { path, equals: JSON.parse(value) };
  } catch {
    return { path, equals: value };
  }
}

/** 把断言文本拆成规则列表（**忽略空行**）。 */
export function parseJsonPathRules(text: string): { path: string; equals?: unknown }[] {
  const out: { path: string; equals?: unknown }[] = [];
  for (const line of text.split(/\r?\n/)) {
    const rule = parseJsonPathRule(line);
    if (rule) {
      out.push(rule);
    }
  }
  return out;
}

/** 表单 → spec。 */
export function formToSpec(form: SpecForm): SpecSite {
  const spec: SpecSite = {
    id: form.id.trim(),
    name: form.name.trim(),
    method: (form.method || "GET").trim().toUpperCase(),
    url: form.url.trim(),
  };

  // 只在**用了 {{token}}** 时才带 Authorization 头（免得给不需要登录态的站点硬塞一个头）
  if (form.url.includes("{{token}}")) {
    spec.headers = [{ name: "Authorization", value: "Bearer {{token}}" }];
  }

  const status = Number(form.assertStatus.trim());
  const rules = parseJsonPathRules(form.jsonPaths);
  if (Number.isFinite(status) && status > 0) {
    spec.assert = {
      status,
      ...(rules.length > 0 ? { jsonPaths: rules } : {}),
    };
  } else if (rules.length > 0) {
    spec.assert = { jsonPaths: rules };
  }

  if (form.rewardPath.trim()) {
    spec.reward = {
      path: form.rewardPath.trim(),
      unit: form.rewardUnit.trim() || "point",
    };
  }

  if (form.balanceUrl.trim() && form.balancePath.trim()) {
    spec.balance = {
      url: form.balanceUrl.trim(),
      path: form.balancePath.trim(),
      unit: form.balanceUnit.trim() || "point",
    };
  }
  return spec;
}

/** 解析一段 spec JSON（**只做语法解析**，字段校验交给后端）。 */
export function parseSpecJson(
  text: string,
): { spec?: SpecSite; error?: string } {
  const trimmed = text.trim();
  if (!trimmed) {
    return { error: "内容是空的" };
  }
  try {
    const parsed = JSON.parse(trimmed) as SpecSite;
    if (!parsed || typeof parsed !== "object") {
      return { error: "不是一个 JSON 对象" };
    }
    return { spec: parsed };
  } catch (err) {
    return { error: `JSON 解析失败：${String(err)}` };
  }
}

/** 前置校验（**提前拦一道**，后端仍会全量校验）。 */
export function validateSpec(spec: SpecSite): string | null {
  if (!spec.id.trim()) {
    return "站点 id 不能为空";
  }
  if (!/^[a-z0-9_-]+$/.test(spec.id.trim())) {
    return "站点 id 只能是小写字母、数字、下划线和连字符";
  }
  if (!spec.name.trim()) {
    return "站点名不能为空";
  }
  if (!spec.url.trim()) {
    return "签到 URL 不能为空";
  }
  if (!/^https?:\/\//i.test(spec.url.trim())) {
    return "签到 URL 必须以 http:// 或 https:// 开头";
  }
  if (spec.balance?.url && !/^https?:\/\//i.test(spec.balance.url.trim())) {
    return "余额 URL 必须以 http:// 或 https:// 开头";
  }
  const texts = [spec.url, ...(spec.headers ?? []).map((h) => h.value)];
  for (const text of texts) {
    for (const name of placeholdersIn(text)) {
      if (!SUPPORTED_PLACEHOLDERS.includes(name as never)) {
        return `不支持的占位符 {{${name}}}——只支持 {{token}} 与 {{today}}`;
      }
    }
  }
  return null;
}

/** 一行摘要（给列表用）。 */
export function describeSpec(spec: SpecSite): string {
  const bits = [`${(spec.method ?? "GET").toUpperCase()} ${spec.url}`];
  if (spec.reward?.path) {
    bits.push(`本次拿到：${spec.reward.path}`);
  }
  if (spec.balance?.path) {
    bits.push(`当前余额：${spec.balance.path}`);
  }
  return bits.join(" · ");
}

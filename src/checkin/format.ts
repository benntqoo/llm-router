/**
 * 签到导入向导的纯函数（02 §7.1.1 / §7.1.2）。
 *
 * 这些函数**不碰 Tauri**、不碰 DOM —— 可以用 `node --test` 直接跑单测
 * （与 `src/usage/format.ts` 同一路子）。
 *
 * 关键语义：
 * - **变量名合法性**与后端 `checkin::har::validate_var_name` **同规则**
 *   （只允许字母 / 数字 / 下划线 / 连字符，长度有上限）——前端先拦一道，
 *   后端再拦一道，两边口径一致；
 * - **敏感值只以掩码出现**：UI 永远拿不到原值（后端在 IPC 边界就掩掉了）。
 */

import type { EntryView, FieldView, VarTarget } from "./types";

/** 变量名长度上限（与后端 `VAR_NAME_MAX_LEN` 一致）。 */
export const VAR_NAME_MAX_LEN = 32;

/** 变量的位置说明（与后端 `VarTarget::label()` 同口径）。 */
export function targetLabel(target: VarTarget): string {
  switch (target.kind) {
    case "header":
      return `请求头 ${target.field}`;
    case "headerPart":
      return `请求头 ${target.field} · ${target.part}`;
    case "query":
      return `查询参数 ${target.field}`;
    case "body":
      return `请求体 ${target.field}`;
  }
}

/** target 的原始字段名（判断敏感、生成建议名都用它）。 */
export function targetRawName(target: VarTarget): string {
  switch (target.kind) {
    case "header":
      return target.field;
    case "headerPart":
      return target.part;
    case "query":
      return target.field;
    case "body":
      return target.field;
  }
}

/**
 * 把任意字段名洗成**合法**的变量名（只保留字母 / 数字 / 下划线 / 连字符）。
 *
 * 与后端 `auto_name` 同一套清洗规则（只是后端多一个 `auto_` 前缀）。
 */
export function sanitizeVarName(raw: string): string {
  let out = "";
  let prevUnderscore = false;
  for (const ch of raw.toLowerCase()) {
    if (/[a-z0-9]/.test(ch)) {
      out += ch;
      prevUnderscore = false;
    } else if (!prevUnderscore) {
      out += "_";
      prevUnderscore = true;
    }
  }
  while (out.endsWith("_")) {
    out = out.slice(0, -1);
  }
  if (out.length > VAR_NAME_MAX_LEN) {
    out = out.slice(0, VAR_NAME_MAX_LEN);
    while (out.endsWith("_")) {
      out = out.slice(0, -1);
    }
  }
  return out;
}

/** 建议的变量名（用户可以直接改名）。 */
export function suggestVarName(target: VarTarget): string {
  return sanitizeVarName(targetRawName(target));
}

/**
 * 变量名是否合法（与后端 `validate_var_name` 同规则）。
 *
 * 后端规则：非空、长度 ≤ 32、只允许 `[A-Za-z0-9_-]`。
 */
export function isValidVarName(name: string): boolean {
  if (name.length === 0 || name.length > VAR_NAME_MAX_LEN) {
    return false;
  }
  return /^[A-Za-z0-9_-]+$/.test(name);
}

/** entry 的一行摘要（列表展示）。 */
export function entryLine(entry: EntryView): string {
  return `${entry.method} ${shortUrl(entry.url, 80)} · ${entry.status}`;
}

/** URL 缩略（去掉 query，超长截断）。 */
export function shortUrl(url: string, max: number): string {
  const withoutQuery = url.split("?")[0];
  if (withoutQuery.length <= max) {
    return withoutQuery;
  }
  return `${withoutQuery.slice(0, max - 1)}…`;
}

/** 按关键字过滤 entry（匹配方法 / URL / 状态码）。 */
export function filterEntries(entries: EntryView[], keyword: string): EntryView[] {
  const needle = keyword.trim().toLowerCase();
  if (!needle) {
    return entries;
  }
  return entries.filter(
    (entry) =>
      entry.method.toLowerCase().includes(needle) ||
      entry.url.toLowerCase().includes(needle) ||
      String(entry.status).includes(needle),
  );
}

/** 一条 entry 里的敏感字段数量（向导提示用）。 */
export function sensitiveCount(fields: FieldView[]): number {
  return fields.filter((field) => field.sensitive).length;
}

/** 站点名的默认建议（去掉 `.har` 后缀）。 */
export function defaultSiteName(source: string): string {
  const base = source.split(/[\\/]/).pop() ?? source;
  return base.replace(/\.har$/i, "") || "未命名站点";
}

/** target 的稳定键（前端用它把"字段 ↔ 变量标记"对上）。 */
export function targetKey(target: VarTarget): string {
  switch (target.kind) {
    case "header":
      return `header:${target.field.toLowerCase()}`;
    case "headerPart":
      return `headerPart:${target.field.toLowerCase()}:${target.part.toLowerCase()}`;
    case "query":
      return `query:${target.field.toLowerCase()}`;
    case "body":
      return `body:${target.field.toLowerCase()}`;
  }
}

/**
 * 猜一条"最像签到请求"的 entry（向导默认选中）。
 *
 * 优先带 `/api/` 的接口请求（页面资源、埋点都不是签到）；
 * 都没有就取第一条。
 */
export function suggestEntryIndex(entries: EntryView[]): number | null {
  if (entries.length === 0) {
    return null;
  }
  const api = entries.find((entry) => entry.url.includes("/api/"));
  return (api ?? entries[0]).index;
}

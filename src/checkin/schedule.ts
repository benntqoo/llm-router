/**
 * 签到调度的纯函数（T17；02 §7.2.1）。
 *
 * 这些函数**不碰 Tauri**、不碰 DOM —— 可以用 `node --test` 直接跑单测
 * （与 `src/checkin/format.ts` 同一路子）。
 *
 * 关键语义：
 * - **cron 字段口径**与后端 `checkin::cron` 一致：用户写**经典 5 字段**
 *   （分 时 日 月 周），后端内部补秒位成 6 字段给 `cron` crate；
 * - `describeCron` 的分类与后端 `CronShape` **同口径**（UI 显示的频率
 *   与系统兜底任务实际注册的频率必须一致）；
 * - 前端校验只拦**一定错**的写法（字段数 / 越界数字），**最终以后端为准**——
 *   复杂表达式（如 `MON-FRI` 名称写法）不由前端猜。
 */

/** 拆出来的 cron 字段（按 5 字段口径命名）。 */
export type CronFields = {
  minute: string;
  hour: string;
  dom: string;
  month: string;
  dow: string;
};

/** 字段的取值范围（经典 5 字段：分 时 日 月 周）。 */
const FIELD_RANGES: Array<{ key: keyof CronFields; min: number; max: number }> = [
  { key: "minute", min: 0, max: 59 },
  { key: "hour", min: 0, max: 23 },
  { key: "dom", min: 1, max: 31 },
  { key: "month", min: 1, max: 12 },
  { key: "dow", min: 0, max: 7 },
];

/** 星期几的中文名（cron：0 和 7 都是周日）。 */
const WEEKDAY_NAMES = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
/** 两位补零。 */
function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

/**
 * 把表达式拆成 5 字段口径。
 *
 * - 5 字段 → 直接就是「分 时 日 月 周」；
 * - 6 / 7 字段 → `cron` crate 的口径（秒 分 时 日 月 周 [年]），**丢掉秒位**。
 *
 * 字段数不对返回 `null`。
 */
export function cronFields(expr: string): CronFields | null {
  const parts = expr.trim().split(/\s+/).filter((part) => part !== "");
  if (parts.length === 5) {
    return {
      minute: parts[0],
      hour: parts[1],
      dom: parts[2],
      month: parts[3],
      dow: parts[4],
    };
  }
  if (parts.length === 6 || parts.length === 7) {
    return {
      minute: parts[1],
      hour: parts[2],
      dom: parts[3],
      month: parts[4],
      dow: parts[5],
    };
  }
  return null;
}

/** 单个字段是否合法（只认数字 / `*` / 范围 / 列表 / 步长）。 */
function fieldOk(field: string, min: number, max: number): boolean {
  if (field === "") {
    return false;
  }
  for (const part of field.split(",")) {
    if (part === "") {
      return false;
    }
    const pieces = part.split("/");
    if (pieces.length > 2) {
      return false;
    }
    const [range, step] = pieces;
    if (step !== undefined && !/^\d+$/.test(step)) {
      return false;
    }
    if (range === "*") {
      continue;
    }
    if (range.includes("-")) {
      const bounds = range.split("-");
      if (bounds.length !== 2) {
        return false;
      }
      const [from, to] = bounds;
      if (!/^\d+$/.test(from) || !/^\d+$/.test(to)) {
        return false;
      }
      const fromValue = Number(from);
      const toValue = Number(to);
      if (fromValue < min || toValue > max) {
        return false;
      }
    } else if (/^\d+$/.test(range)) {
      const value = Number(range);
      if (value < min || value > max) {
        return false;
      }
    } else {
      return false;
    }
  }
  return true;
}

/**
 * 校验 cron 表达式；合法返回 `null`，不合法返回中文原因。
 *
 * **空串算合法**——空 = 「不调度」（调用方把 `schedule` 传 `null`）。
 */
export function validateCron(expr: string): string | null {
  const trimmed = expr.trim();
  if (trimmed === "") {
    return null;
  }
  const count = trimmed.split(/\s+/).filter((part) => part !== "").length;
  if (count < 5 || count > 7) {
    return `cron 表达式必须是 5~7 个字段（分 时 日 月 周），实际 ${count} 个`;
  }
  const fields = cronFields(trimmed);
  if (!fields) {
    return "cron 表达式解析不了";
  }
  for (const range of FIELD_RANGES) {
    if (!fieldOk(fields[range.key], range.min, range.max)) {
      return `cron 的${
        { minute: "分钟", hour: "小时", dom: "日", month: "月", dow: "星期" }[range.key]
      }字段不合法（应在 ${range.min}~${range.max}）`;
    }
  }
  return null;
}

/** 展开星期字段（`1-5` / `1,3` / `0`）→ 星期编号列表（0 = 周日）。 */
export function expandWeekdays(field: string): number[] {
  const days: number[] = [];
  for (const part of field.split(",")) {
    const piece = part.trim();
    if (piece.includes("-")) {
      const [from, to] = piece.split("-");
      const fromValue = Number(from);
      const toValue = Number(to);
      if (!Number.isFinite(fromValue) || !Number.isFinite(toValue)) {
        continue;
      }
      if (fromValue <= toValue) {
        for (let day = fromValue; day <= toValue; day += 1) {
          days.push(day);
        }
      } else {
        // 跨周末的区间（如 5-0 = 周五到周日）
        for (let day = fromValue; day <= 7; day += 1) {
          days.push(day);
        }
        for (let day = 0; day <= toValue; day += 1) {
          days.push(day);
        }
      }
    } else {
      const value = Number(piece);
      if (Number.isFinite(value)) {
        days.push(value);
      }
    }
  }
  return days.filter((day) => day >= 0 && day <= 7);
}

/** 星期字段的中文说明（`1-5` → "周一至周五"）。 */
export function weekdayLabel(field: string): string {
  const days = expandWeekdays(field);
  if (days.length === 0) {
    return field;
  }
  const names = days.map((day) => WEEKDAY_NAMES[day === 7 ? 0 : day]);
  if (names.length === 1) {
    return names[0];
  }
  // 连续区间 → 「周一至周五」
  const sorted = [...days].sort((a, b) => a - b);
  const contiguous = sorted.every((day, index) =>
    index === 0 ? true : day === sorted[index - 1] + 1,
  );
  if (contiguous && names.length > 1) {
    return `${names[0]}至${names[names.length - 1]}`;
  }
  return names.join("、");
}

/**
 * 把 cron 表达式说成中文（与后端 `CronShape` 同口径）。
 *
 * 认不出来的表达式返回「自定义（表达式）」——**不猜**。
 */
export function describeCron(expr: string | null | undefined): string {
  const trimmed = (expr ?? "").trim();
  if (trimmed === "") {
    return "未设置调度";
  }
  const fields = cronFields(trimmed);
  if (!fields) {
    return `自定义（${trimmed}）`;
  }
  const { minute, hour, dom, month, dow } = fields;

  // 每分钟：分钟字段是 `*`（与后端 `CronShape::EveryMinutes(1)` 同口径）
  if (minute === "*" && hour === "*" && dom === "*" && month === "*" && dow === "*") {
    return "每 1 分钟";
  }

  // 每 N 分钟：`*/N * * * *`
  if (minute.startsWith("*/")) {
    const step = Number(minute.slice(2));
    if (Number.isFinite(step) && step > 0) {
      if (hour === "*" && dom === "*" && month === "*" && dow === "*") {
        return `每 ${step} 分钟`;
      }
    }
    return `自定义（${trimmed}）`;
  }

  const minuteValue = Number(minute);
  const hourValue = Number(hour);
  if (!Number.isFinite(minuteValue)) {
    return `自定义（${trimmed}）`;
  }

  // 每小时第 M 分：`M * * * *`
  if (hour === "*" && dom === "*" && month === "*" && dow === "*") {
    return `每小时第 ${minuteValue} 分钟`;
  }
  if (!Number.isFinite(hourValue)) {
    return `自定义（${trimmed}）`;
  }

  // 每天：`M H * * *`
  if (dom === "*" && month === "*" && dow === "*") {
    return `每天 ${pad(hourValue)}:${pad(minuteValue)}`;
  }
  // 每周：`M H * * D`
  if (dom === "*" && month === "*" && dow !== "*") {
    return `每周 ${weekdayLabel(dow)} ${pad(hourValue)}:${pad(minuteValue)}`;
  }
  // 每月：`M H D * *`
  if (dom !== "*" && month === "*" && dow === "*") {
    const day = Number(dom);
    if (Number.isFinite(day)) {
      return `每月 ${day} 日 ${pad(hourValue)}:${pad(minuteValue)}`;
    }
  }
  return `自定义（${trimmed}）`;
}

/** Unix 秒 → 本地时间 `MM-DD HH:mm:ss`。 */
export function formatClock(ts: number | null | undefined): string {
  if (ts === null || ts === undefined) {
    return "—";
  }
  const date = new Date(ts * 1000);
  const month = pad(date.getMonth() + 1);
  const day = pad(date.getDate());
  const hour = pad(date.getHours());
  const minute = pad(date.getMinutes());
  const second = pad(date.getSeconds());
  return `${month}-${day} ${hour}:${minute}:${second}`;
}

/** Unix 秒 → 相对时间（"3 分钟前" / "2 小时后"）。 */
export function formatRelative(
  ts: number | null | undefined,
  now: number,
): string {
  if (ts === null || ts === undefined) {
    return "—";
  }
  const diff = ts - now;
  const abs = Math.abs(diff);
  const suffix = diff >= 0 ? "后" : "前";
  if (abs < 60) {
    return diff >= 0 ? "马上" : "刚刚";
  }
  if (abs < 3600) {
    return `${Math.floor(abs / 60)} 分钟${suffix}`;
  }
  if (abs < 86400) {
    return `${Math.floor(abs / 3600)} 小时${suffix}`;
  }
  return `${Math.floor(abs / 86400)} 天${suffix}`;
}

/** 系统兜底任务的一行说明。 */
export function describeSystemTask(
  registered: boolean,
  frequency: string | null,
): string {
  if (!registered) {
    return "系统兜底：未注册";
  }
  return frequency
    ? `系统兜底：已注册（${frequency}）`
    : "系统兜底：已注册";
}

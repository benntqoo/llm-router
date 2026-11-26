/**
 * 用量面板的纯格式化函数（02 §8.4）。
 *
 * 这些函数**不碰 Tauri**、不碰 DOM —— 可以用 `node --test` 直接跑单测
 * （与 `src/vault/reveal.ts` 同一路子）。
 *
 * 关键语义：**空值与 0 必须可区分**。
 * - 成本 `null` = **未知**（没配牌价，02 §8.3）→ 显示「未知」，不显示「¥0」；
 * - TTFT / 输出速度 `null` = **没有样本** → 显示「—」，不显示 0。
 */

/** 聚合维度（与后端 `store::analytics::Dim` 一一对应）。 */
export type Dim = "day" | "channel" | "model" | "client";

/** 维度的中文名（UI 标签）。 */
export const DIM_LABELS: Record<Dim, string> = {
  day: "按天",
  channel: "按渠道",
  model: "按模型",
  client: "按客户端",
};

/** 全部维度（UI 切换顺序）。 */
export const DIMS: Dim[] = ["day", "channel", "model", "client"];

/**
 * 成本（1e-6 元）→ 可读金额。
 *
 * `null` / `undefined` = **未知**（该行没命中价目表）→ 显示「未知」，
 * **不能**显示成「¥0」——那会被读成"这次调用不花钱"。
 */
export function formatCost(micros: number | null | undefined): string {
  if (micros === null || micros === undefined) {
    return "未知";
  }
  const yuan = micros / 1_000_000;
  // 最多 6 位小数（micros 的最小刻度就是 1e-6 元），去掉末尾多余的 0
  const text = yuan.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
  return `¥${text} 元`;
}

/**
 * token 数 → 千分位。
 *
 * **不做单位缩写**：统计页要的是精确值，「1.2M」会让用户无法核对。
 */
export function formatTokens(tokens: number | null | undefined): string {
  if (tokens === null || tokens === undefined) {
    return "—";
  }
  return tokens.toLocaleString("en-US");
}

/**
 * 输出速度（tokens·s⁻¹）。
 *
 * `null` = 没有有效样本（失败行 / 耗时为 0）→ 「—」。
 */
export function formatTps(tps: number | null | undefined): string {
  if (tps === null || tps === undefined) {
    return "—";
  }
  return `${tps.toFixed(1)} tok/s`;
}

/**
 * TTFT（毫秒）。
 *
 * `null` = 该行不是流式（没有「首个 token」的概念）→ 「—」。
 */
export function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) {
    return "—";
  }
  return `${Math.round(ms)} ms`;
}

/** Unix 秒 → 本地时间（`YYYY-MM-DD HH:mm:ss`）。 */
export function formatTime(ts: number): string {
  const date = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ` +
    `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
  );
}

/**
 * 估算标记的显示文案。
 *
 * `estimated=true` = 上游没回 usage，token 数是 tiktoken 估算的（02 §8.1）。
 */
export function formatEstimated(estimated: boolean): string {
  return estimated ? "估算" : "精确";
}

/**
 * 桶的条形图比例（0~1）。
 *
 * 用于 SVG 条形图的宽度：按最大值归一化。
 * 全部为 0 时返回 0（不画条），避免除零。
 */
export function barRatio(value: number, max: number): number {
  if (!Number.isFinite(value) || !Number.isFinite(max) || max <= 0) {
    return 0;
  }
  const ratio = value / max;
  if (ratio < 0) {
    return 0;
  }
  return ratio > 1 ? 1 : ratio;
}

/**
 * 坐标轴的"好看刻度"上限。
 *
 * 图表 y 轴要一个整数刻度：把最大值向上取整到 1/2/5 × 10^n，
 * 免得出现「0.0037 元」这种没法读的刻度。
 */
export function niceMax(value: number): number {
  if (!Number.isFinite(value) || value <= 0) {
    return 1;
  }
  const exponent = Math.floor(Math.log10(value));
  const magnitude = Math.pow(10, exponent);
  const normalized = value / magnitude;
  let step = 10;
  if (normalized <= 1) {
    step = 1;
  } else if (normalized <= 2) {
    step = 2;
  } else if (normalized <= 5) {
    step = 5;
  }
  return step * magnitude;
}

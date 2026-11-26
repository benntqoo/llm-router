/**
 * 聚合图表（手写 SVG，**不引第三方图表库**）。
 *
 * 取向与 02 §6.6 的事件总线一致：轻量、可离线构建、包体不增。
 * 图表组件单独成文件——要换实现（例如换成图表库）只动这一个文件。
 *
 * 横向条形图：横向更适合「渠道名 / 模型名 / client key」这类长标签。
 */

import { barRatio, formatCost, formatTokens } from "./format";
import type { Bucket } from "./types";

/** 图表度量。 */
export type Metric = "tokens" | "cost" | "requests";

/** 度量的中文名。 */
export const METRIC_LABELS: Record<Metric, string> = {
  tokens: "总 token",
  cost: "已知成本",
  requests: "请求数",
};

/** 全部度量（UI 切换顺序）。 */
export const METRICS: Metric[] = ["tokens", "cost", "requests"];

/** 取桶在某度量下的数值。 */
export function metricValue(bucket: Bucket, metric: Metric): number {
  switch (metric) {
    case "tokens":
      return bucket.total_tokens;
    case "cost":
      // 成本以「元」入图，便于 y 轴刻度；展示时再换回 micros 走同一套格式化
      return bucket.cost_micros / 1_000_000;
    case "requests":
      return bucket.requests;
  }
}

/** 按度量格式化数值。 */
export function formatMetric(value: number, metric: Metric): string {
  switch (metric) {
    case "tokens":
      return formatTokens(value);
    case "cost":
      return formatCost(Math.round(value * 1_000_000));
    case "requests":
      return formatTokens(value);
  }
}

/** 标签过长时截断（图表左侧标签区宽度有限）。 */
export function truncateLabel(label: string, max = 22): string {
  if (label.length <= max) {
    return label;
  }
  return `${label.slice(0, max - 1)}…`;
}

// ── SVG 布局常量 ──────────────────────────────────────────────────────

const VIEW_WIDTH = 720;
const ROW_HEIGHT = 26;
const TOP_PADDING = 10;
const LABEL_WIDTH = 180;
const BAR_START = LABEL_WIDTH + 8;
const BAR_WIDTH = 380;
const VALUE_X = BAR_START + BAR_WIDTH + 10;

type Props = {
  /** 桶列表（已按后端排序）。 */
  buckets: Bucket[];
  /** 当前度量。 */
  metric: Metric;
  /** 无数据时的提示文案。 */
  emptyHint?: string;
};

/**
 * 横向条形图。
 *
 * 空数据时**不画空坐标系**，直接给一句提示——避免"看起来是 0"的误读。
 */
export default function UsageChart({ buckets, metric, emptyHint }: Props) {
  if (buckets.length === 0) {
    return <p className="muted">{emptyHint ?? "当前时间范围内没有用量记录。"}</p>;
  }

  const values = buckets.map((bucket) => metricValue(bucket, metric));
  const max = Math.max(...values, 0);
  const height = TOP_PADDING * 2 + buckets.length * ROW_HEIGHT;

  return (
    <svg
      className="usage-chart"
      viewBox={`0 0 ${VIEW_WIDTH} ${height}`}
      preserveAspectRatio="xMinYMin meet"
      role="img"
      aria-label={`按${metric === "tokens" ? "token" : metric === "cost" ? "成本" : "请求数"}的聚合条形图`}
    >
      {buckets.map((bucket, index) => {
        const value = values[index];
        const ratio = barRatio(value, max);
        const y = TOP_PADDING + index * ROW_HEIGHT;
        const barWidth = ratio * BAR_WIDTH;
        return (
          <g key={bucket.key}>
            <text x={0} y={y + 16} className="chart-label">
              {truncateLabel(bucket.label || bucket.key)}
            </text>
            <rect
              x={BAR_START}
              y={y + 5}
              width={BAR_WIDTH}
              height={ROW_HEIGHT - 12}
              className="chart-track"
            />
            <rect
              x={BAR_START}
              y={y + 5}
              width={barWidth}
              height={ROW_HEIGHT - 12}
              className="chart-bar"
            />
            <text x={VALUE_X} y={y + 16} className="chart-value">
              {formatMetric(value, metric)}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

/**
 * 用量聚合的 IPC 数据形状（与后端 `store::analytics` 一一对应）。
 *
 * 字段名保持 **snake_case**——Tauri IPC 的**返回体**就是 serde 的原始 JSON，
 * 只有**入参**在 JS 侧要用 camelCase（见 02 §6.6）。
 */

/** 一个聚合桶。 */
export type Bucket = {
  key: string;
  label: string;
  requests: number;
  success_count: number;
  error_count: number;
  aborted_count: number;
  prompt_tokens: number;
  completion_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
  /** 已知成本合计（1e-6 元）。 */
  cost_micros: number;
  /** 成本未知的行数（没配牌价）。 */
  cost_unknown_rows: number;
  estimated_rows: number;
  ttft_p50_ms: number | null;
  ttft_p95_ms: number | null;
  output_tps: number | null;
};

/** 聚合结果：分桶 + 全范围合计。 */
export type AggregateResult = {
  dim: string;
  since_ts: number | null;
  until_ts: number | null;
  buckets: Bucket[];
  total: Bucket;
};

/** 最近一条请求的明细。 */
export type RecentRequest = {
  id: number;
  ts: number;
  client_key_id: string | null;
  client_key_label: string | null;
  channel_id: string | null;
  channel_name: string | null;
  model_alias: string | null;
  upstream_model: string | null;
  protocol: string | null;
  prompt_tokens: number;
  completion_tokens: number;
  cache_read_tokens: number;
  total_tokens: number;
  estimated: boolean;
  ttft_ms: number | null;
  duration_ms: number | null;
  output_tps: number | null;
  status: string | null;
  error_kind: string | null;
  cost_micros: number | null;
};

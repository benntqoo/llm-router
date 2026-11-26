import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import UsageChart, { METRIC_LABELS, METRICS, type Metric } from "./UsageChart";
import {
  DIM_LABELS,
  DIMS,
  formatCost,
  formatEstimated,
  formatMs,
  formatTime,
  formatTokens,
  formatTps,
  type Dim,
} from "./format";
import type { AggregateResult, RecentRequest } from "./types";

/** 自动刷新间隔（毫秒）。 */
const REFRESH_MS = 5000;
/** 最近请求明细条数。 */
const RECENT_LIMIT = 50;
/** 聚合桶数上限。 */
const BUCKET_LIMIT = 50;
/** 可选时间窗（天）。 */
const RANGE_DAYS = [7, 30, 90];

/**
 * 用量统计面板（02 §8.4）。
 *
 * 四维聚合（日 / 渠道 / 模型 / 客户端）+ 最近请求明细，
 * 数据来自 `usage_aggregate` / `usage_recent` 两个 IPC——
 * **后端聚合、前端只做展示**，不在 UI 侧另写一份统计口径。
 */
export default function UsagePanel() {
  const [dim, setDim] = useState<Dim>("day");
  const [days, setDays] = useState(30);
  const [metric, setMetric] = useState<Metric>("tokens");
  const [aggregate, setAggregate] = useState<AggregateResult | null>(null);
  const [recent, setRecent] = useState<RecentRequest[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const sinceTs = Math.floor(Date.now() / 1000) - days * 24 * 3600;
      const [agg, rows] = await Promise.all([
        invoke<AggregateResult>("usage_aggregate", {
          dim,
          sinceTs,
          untilTs: null,
          limit: BUCKET_LIMIT,
        }),
        invoke<RecentRequest[]>("usage_recent", { limit: RECENT_LIMIT }),
      ]);
      setAggregate(agg);
      setRecent(rows);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoaded(true);
    }
  }, [dim, days]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => {
      void refresh();
    }, REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const total = aggregate?.total ?? null;

  const kpis = useMemo(() => {
    if (!total) {
      return [];
    }
    return [
      { label: "请求数", value: formatTokens(total.requests), hint: `失败 ${total.error_count} · 断连 ${total.aborted_count}` },
      { label: "总 token", value: formatTokens(total.total_tokens), hint: `输入 ${formatTokens(total.prompt_tokens)} · 输出 ${formatTokens(total.completion_tokens)}` },
      { label: "已知成本", value: formatCost(total.cost_micros), hint: total.cost_unknown_rows > 0 ? `另有 ${total.cost_unknown_rows} 行未配牌价（未知）` : "全部行都有牌价" },
      { label: "TTFT P50 / P95", value: `${formatMs(total.ttft_p50_ms)} / ${formatMs(total.ttft_p95_ms)}`, hint: "只统计流式请求" },
      { label: "输出速度", value: formatTps(total.output_tps), hint: "成功行按耗时加权" },
      { label: "估算行", value: formatTokens(total.estimated_rows), hint: "上游没回 usage，由 tiktoken 估算" },
    ];
  }, [total]);

  return (
    <section className="card">
      <h2>用量统计</h2>
      <p className="muted">
        四维聚合（日 / 渠道 / 模型 / 客户端）· 数据全部由 <code>usage_log</code> 派生，
        成本按价目表换算（没配牌价的行显示为「未知」，不按 0 元计）。
      </p>

      <div className="row-inline">
        {DIMS.map((item) => (
          <button
            key={item}
            type="button"
            className={item === dim ? "active" : ""}
            onClick={() => setDim(item)}
          >
            {DIM_LABELS[item]}
          </button>
        ))}
        <span className="sep" />
        {RANGE_DAYS.map((item) => (
          <button
            key={item}
            type="button"
            className={item === days ? "active" : ""}
            onClick={() => setDays(item)}
          >
            近 {item} 天
          </button>
        ))}
        <span className="sep" />
        {METRICS.map((item) => (
          <button
            key={item}
            type="button"
            className={item === metric ? "active" : ""}
            onClick={() => setMetric(item)}
          >
            {METRIC_LABELS[item]}
          </button>
        ))}
        <button type="button" onClick={() => void refresh()}>
          刷新
        </button>
      </div>

      {error && <p className="error">读取用量统计失败：{error}</p>}
      {!error && !loaded && <p className="muted">正在读取…</p>}

      {total && (
        <div className="usage-kpis">
          {kpis.map((kpi) => (
            <div className="usage-kpi" key={kpi.label}>
              <div className="usage-kpi-label">{kpi.label}</div>
              <div className="usage-kpi-value">{kpi.value}</div>
              <div className="usage-kpi-hint">{kpi.hint}</div>
            </div>
          ))}
        </div>
      )}

      {aggregate && (
        <>
          <h3>聚合（{DIM_LABELS[dim]}）</h3>
          <UsageChart
            buckets={aggregate.buckets}
            metric={metric}
            emptyHint={`近 ${days} 天内没有${DIM_LABELS[dim]}的用量记录。`}
          />

          {aggregate.buckets.length > 0 && (
            <table className="usage-table">
              <thead>
                <tr>
                  <th>{DIM_LABELS[dim].replace("按", "")}</th>
                  <th>请求数</th>
                  <th>失败</th>
                  <th>总 token</th>
                  <th>输入 / 输出</th>
                  <th>已知成本</th>
                  <th>TTFT P50 / P95</th>
                  <th>输出速度</th>
                  <th>估算行</th>
                </tr>
              </thead>
              <tbody>
                {aggregate.buckets.map((bucket) => (
                  <tr key={bucket.key}>
                    <td title={bucket.label || bucket.key}>{bucket.label || bucket.key}</td>
                    <td>{formatTokens(bucket.requests)}</td>
                    <td>{formatTokens(bucket.error_count)}</td>
                    <td>{formatTokens(bucket.total_tokens)}</td>
                    <td>
                      {formatTokens(bucket.prompt_tokens)} / {formatTokens(bucket.completion_tokens)}
                    </td>
                    <td>
                      {formatCost(bucket.cost_micros)}
                      {bucket.cost_unknown_rows > 0 && (
                        <span className="muted"> （{bucket.cost_unknown_rows} 行未知）</span>
                      )}
                    </td>
                    <td>
                      {formatMs(bucket.ttft_p50_ms)} / {formatMs(bucket.ttft_p95_ms)}
                    </td>
                    <td>{formatTps(bucket.output_tps)}</td>
                    <td>{formatTokens(bucket.estimated_rows)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}

      <h3>最近请求</h3>
      {!error && loaded && recent.length === 0 && (
        <p className="muted">还没有请求记录。发一次 chat 请求后再回来看。</p>
      )}
      {recent.length > 0 && (
        <table className="usage-table">
          <thead>
            <tr>
              <th>时间</th>
              <th>渠道 / 模型</th>
              <th>输入 / 输出</th>
              <th>总 token</th>
              <th>TTFT</th>
              <th>速度</th>
              <th>成本</th>
              <th>来源</th>
              <th>状态</th>
            </tr>
          </thead>
          <tbody>
            {recent.map((item) => (
              <tr key={item.id}>
                <td>{formatTime(item.ts)}</td>
                <td>
                  {item.channel_name ?? "—"} · {item.upstream_model ?? "—"}
                </td>
                <td>
                  {formatTokens(item.prompt_tokens)} / {formatTokens(item.completion_tokens)}
                </td>
                <td>{formatTokens(item.total_tokens)}</td>
                <td>{formatMs(item.ttft_ms)}</td>
                <td>{formatTps(item.output_tps)}</td>
                <td>{formatCost(item.cost_micros)}</td>
                <td>{formatEstimated(item.estimated)}</td>
                <td>{item.status ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

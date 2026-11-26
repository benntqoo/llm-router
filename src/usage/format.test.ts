import assert from "node:assert/strict";
import test from "node:test";

import {
  DIM_LABELS,
  DIMS,
  barRatio,
  formatCost,
  formatEstimated,
  formatMs,
  formatTps,
  formatTokens,
  formatTime,
  niceMax,
} from "./format.ts";

test("四个维度都有中文标签，且顺序稳定", () => {
  assert.deepEqual(DIMS, ["day", "channel", "model", "client"]);
  assert.equal(DIM_LABELS.day, "按天");
  assert.equal(DIM_LABELS.channel, "按渠道");
  assert.equal(DIM_LABELS.model, "按模型");
  assert.equal(DIM_LABELS.client, "按客户端");
});

test("成本：NULL 是「未知」，不是 ¥0", () => {
  // 02 §8.3：没命中价目 → 未知。显示成 ¥0 会被读成"不花钱"。
  assert.equal(formatCost(null), "未知");
  assert.equal(formatCost(undefined), "未知");
  assert.notEqual(formatCost(null), formatCost(0));
});

test("成本：micros 换算成元且去掉多余的 0", () => {
  assert.equal(formatCost(0), "¥0 元");
  assert.equal(formatCost(536), "¥0.000536 元");
  assert.equal(formatCost(108), "¥0.000108 元");
  assert.equal(formatCost(10_000_000), "¥10 元");
  assert.equal(formatCost(13_000_000), "¥13 元");
  assert.equal(formatCost(1_500_000), "¥1.5 元");
});

test("token 数用千分位，不做单位缩写", () => {
  assert.equal(formatTokens(0), "0");
  assert.equal(formatTokens(1234), "1,234");
  assert.equal(formatTokens(1_000_000), "1,000,000");
  assert.equal(formatTokens(null), "—");
});

test("输出速度：没有样本显示「—」而不是 0", () => {
  assert.equal(formatTps(null), "—");
  assert.equal(formatTps(undefined), "—");
  assert.equal(formatTps(0), "0.0 tok/s");
  assert.equal(formatTps(58.333), "58.3 tok/s");
});

test("TTFT：非流式（NULL）显示「—」", () => {
  assert.equal(formatMs(null), "—");
  assert.equal(formatMs(undefined), "—");
  assert.equal(formatMs(120), "120 ms");
  assert.equal(formatMs(119.6), "120 ms");
});

test("时间戳转本地时间", () => {
  const text = formatTime(1_704_067_200);
  assert.match(text, /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
});

test("估算标记：估算 / 精确", () => {
  assert.equal(formatEstimated(true), "估算");
  assert.equal(formatEstimated(false), "精确");
});

test("条形图比例按最大值归一化", () => {
  assert.equal(barRatio(50, 100), 0.5);
  assert.equal(barRatio(100, 100), 1);
  assert.equal(barRatio(150, 100), 1, "超出最大值应截到 1");
  assert.equal(barRatio(-5, 100), 0, "负值不画条");
  assert.equal(barRatio(10, 0), 0, "最大值为 0 时不画条");
});

test("坐标轴上限取整到 1/2/5 × 10^n", () => {
  assert.equal(niceMax(0), 1);
  assert.equal(niceMax(0.0037), 0.005);
  assert.equal(niceMax(1.2), 2);
  assert.equal(niceMax(3.4), 5);
  assert.equal(niceMax(7), 10);
  assert.equal(niceMax(230), 500);
});

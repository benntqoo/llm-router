import assert from "node:assert/strict";
import { test } from "node:test";

import {
  describeCopilotState,
  describeModel,
  describeModelSync,
  formatExpiry,
  nextPollDelayMs,
  shouldKeepPolling,
  type CopilotModelsView,
  type CopilotStatus,
} from "./copilot.ts";

test("pending 与 slow_down 继续轮询，其余是终态", () => {
  assert.equal(shouldKeepPolling("pending"), true);
  assert.equal(shouldKeepPolling("slow_down"), true);
  assert.equal(shouldKeepPolling("authorized"), false);
  assert.equal(shouldKeepPolling("expired"), false);
  assert.equal(shouldKeepPolling("denied"), false);
});

test("轮询间隔用后端给的秒数，且有下限", () => {
  assert.equal(nextPollDelayMs({ state: "pending", interval: 5, expiresAt: null, apiBase: null }), 5000);
  assert.equal(nextPollDelayMs({ state: "slow_down", interval: 10, expiresAt: null, apiBase: null }), 10000);
  // 0 或负数不能让前端疯狂空转
  assert.equal(nextPollDelayMs({ state: "pending", interval: 0, expiresAt: null, apiBase: null }), 1000);
  assert.equal(nextPollDelayMs({ state: "pending", interval: -3, expiresAt: null, apiBase: null }), 1000);
});

test("没授权时说清下一步做什么", () => {
  const none: CopilotStatus = {
    authorized: false,
    expiresAt: null,
    apiBase: null,
    channelId: null,
    channelEnabled: false,
  };
  assert.match(describeCopilotState(none), /设备授权/);

  const hasChannel: CopilotStatus = { ...none, channelId: "ch_1" };
  assert.match(describeCopilotState(hasChannel), /短码/);
});

test("已授权时给出到期时间，并说明渠道默认关闭", () => {
  const status: CopilotStatus = {
    authorized: true,
    expiresAt: 1795765650,
    apiBase: "https://api.githubcopilot.com",
    channelId: "ch_1",
    channelEnabled: false,
  };
  const text = describeCopilotState(status);
  assert.match(text, /已授权/);
  assert.match(text, /到期/);
  assert.match(text, /默认关闭/);
});

test("过期时刻格式化，缺失时返回空串", () => {
  assert.equal(formatExpiry(null), "");
  assert.equal(formatExpiry(0), "");
  assert.notEqual(formatExpiry(1795765650), "");
});

test("模型目录刷新结果说清新增 / 更新 / 禁用 / 跳过", () => {
  const view: CopilotModelsView = {
    models: [
      {
        id: "gpt-5-mini",
        name: "GPT-5 mini",
        contextLen: 264000,
        supportsToolCalls: true,
        supportsVision: false,
      },
    ],
    added: 3,
    updated: 2,
    disabled: 1,
    skipped: ["gpt-4o"],
  };
  const text = describeModelSync(view);
  assert.match(text, /共 1 个模型/);
  assert.match(text, /新增 3/);
  assert.match(text, /更新 2/);
  assert.match(text, /禁用 1/);
  // 被跳过的原因要说清（别名被别的渠道占用）
  assert.match(text, /跳过 1/);
  assert.match(text, /gpt-4o/);
});

test("计数为 0 时不啰嗦", () => {
  const text = describeModelSync({
    models: [],
    added: 0,
    updated: 0,
    disabled: 0,
    skipped: [],
  });
  assert.equal(text, "共 0 个模型");
});

test("模型能力说明区分有与没有", () => {
  assert.match(
    describeModel({
      id: "gpt-5-mini",
      name: "GPT-5 mini",
      contextLen: 264000,
      supportsToolCalls: true,
      supportsVision: false,
    }),
    /上下文 264K/,
  );
  assert.match(
    describeModel({
      id: "m",
      name: "m",
      contextLen: null,
      supportsToolCalls: false,
      supportsVision: false,
    }),
    /—/,
  );
});

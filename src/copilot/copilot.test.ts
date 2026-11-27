import assert from "node:assert/strict";
import { test } from "node:test";

import {
  describeCopilotState,
  formatExpiry,
  nextPollDelayMs,
  shouldKeepPolling,
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

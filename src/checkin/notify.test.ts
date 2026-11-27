import assert from "node:assert/strict";
import test from "node:test";

import {
  describeNotifyState,
  needsChatId,
  webhookPlaceholder,
  webhookUrlToSend,
  WEBHOOK_KINDS,
} from "./notify.ts";

test("只有 Telegram 要 chat_id", () => {
  assert.equal(needsChatId("telegram"), true);
  assert.equal(needsChatId("wecom"), false);
  assert.equal(needsChatId("lark"), false);
  assert.equal(needsChatId(null), false);
});

test("webhook 输入框按类型给不同提示", () => {
  assert.ok(webhookPlaceholder("wecom").includes("qyapi.weixin.qq.com"));
  assert.ok(webhookPlaceholder("lark").includes("feishu"));
  assert.ok(webhookPlaceholder("telegram").includes("api.telegram.org"));
  assert.equal(webhookPlaceholder(null), "先选一个 webhook 类型");
});

test("留空 = 不动已配置的地址（不能被空输入清掉）", () => {
  assert.equal(webhookUrlToSend(""), null);
  assert.equal(webhookUrlToSend("   "), null);
  assert.equal(
    webhookUrlToSend("  https://example.com/hook?key=abc  "),
    "https://example.com/hook?key=abc",
  );
});

test("通知状态一句话说清后果", () => {
  assert.ok(describeNotifyState(false, true, true, false).includes("已关闭"));
  assert.ok(describeNotifyState(false, true, true, true).includes("已关闭"));
  const on = describeNotifyState(true, true, true, true);
  assert.ok(on.includes("系统通知"));
  assert.ok(on.includes("webhook"));
  assert.ok(on.includes("成功也通知"));
  const onlyFail = describeNotifyState(true, true, false, false);
  assert.ok(onlyFail.includes("只通知失败"));
  assert.ok(!onlyFail.includes("webhook"));
  // 只要群消息不要弹窗
  const webhookOnly = describeNotifyState(true, false, true, true);
  assert.ok(!webhookOnly.includes("系统通知"));
  assert.ok(webhookOnly.includes("webhook"));
  // 一个通道都没有 → 说清楚
  assert.ok(describeNotifyState(true, false, true, false).includes("没有可用通道"));
});

test("webhook 清单覆盖三种群机器人 + 不使用", () => {
  const values = WEBHOOK_KINDS.map((item) => item.value);
  assert.deepEqual(values, ["", "wecom", "lark", "telegram"]);
});

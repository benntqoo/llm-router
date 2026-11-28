import assert from "node:assert/strict";
import { test } from "node:test";

import {
  COPILOT_TOS_STATEMENT,
  COPILOT_TOS_VERSION,
  describeEnableEffect,
  describeEnableState,
  formatAcceptedAt,
  needsTosConfirmation,
  type TosStatus,
} from "./tos.ts";

const never: TosStatus = {
  channelEnabled: false,
  tosAcceptedAt: null,
  tosRequired: true,
};

test("没确认过 → 开启前必须先弹确认框", () => {
  assert.equal(needsTosConfirmation(never), true);
});

test("已经确认过 → 直接启用，不再弹框", () => {
  assert.equal(
    needsTosConfirmation({
      channelEnabled: false,
      tosAcceptedAt: 1795765650,
      tosRequired: false,
    }),
    false,
  );
});

test("后端说「还需要确认」（声明改版）→ 照样弹框", () => {
  // 版本对不上时后端会拦；前端要提前弹框，别让用户点了才吃报错
  assert.equal(
    needsTosConfirmation({
      channelEnabled: false,
      tosAcceptedAt: 1795765650,
      tosRequired: true,
    }),
    true,
  );
});

test("已启用 → 这次是关，不用弹框", () => {
  assert.equal(
    needsTosConfirmation({
      channelEnabled: true,
      tosAcceptedAt: 1795765650,
      tosRequired: false,
    }),
    false,
  );
});

test("状态说明说清后果", () => {
  assert.match(describeEnableState(never), /默认关闭/);
  assert.match(describeEnableState(never), /确认/);
  assert.match(
    describeEnableState({
      channelEnabled: true,
      tosAcceptedAt: 1795765650,
      tosRequired: false,
    }),
    /已启用/,
  );
  assert.match(
    describeEnableState({
      channelEnabled: false,
      tosAcceptedAt: 1795765650,
      tosRequired: false,
    }),
    /已确认过/,
  );
});

test("启用后果写在前面（用户知道自己在同意什么）", () => {
  assert.match(describeEnableEffect(), /\/v1\/models/);
  assert.match(describeEnableEffect(), /额度/);
});

test("声明原文与后端逐字一致（改文案要升版本）", () => {
  // 与 Rust 侧 providers::compliance::COPILOT_TOS_STATEMENT 对齐
  assert.match(COPILOT_TOS_STATEMENT, /违反 GitHub Copilot ToS/);
  assert.match(COPILOT_TOS_STATEMENT, /账号可能受限/);
  assert.match(COPILOT_TOS_STATEMENT, /用户显式确认后才启用/);
  assert.equal(COPILOT_TOS_VERSION, "copilot-tos-2026-11-28");
});

test("确认时刻格式化，缺失时返回空串", () => {
  assert.equal(formatAcceptedAt(null), "");
  assert.equal(formatAcceptedAt(0), "");
  assert.notEqual(formatAcceptedAt(1795765650), "");
});

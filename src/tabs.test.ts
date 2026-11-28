import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_TAB,
  readStoredTab,
  resolveTab,
  storeTab,
  tabById,
  tabOfPanel,
  TABS,
} from "./tabs.ts";

test("四个功能分组，顺序固定", () => {
  assert.deepEqual(
    TABS.map((tab) => tab.id),
    ["access", "credentials", "ops", "checkin"],
  );
  assert.deepEqual(
    TABS.map((tab) => tab.label),
    ["接入", "凭证", "运维", "签到与积分"],
  );
});

test("每个面板都归且只归一个组", () => {
  const seen = new Set<string>();
  for (const tab of TABS) {
    for (const panel of tab.panels) {
      assert.equal(seen.has(panel), false, `面板 ${panel} 重复了`);
      seen.add(panel);
    }
  }
  // 8 个面板一个都不能漏
  assert.equal(seen.size, 8);
});

test("分组内容符合功能域", () => {
  assert.deepEqual(tabById("access")?.panels, ["channel", "models", "copilot"]);
  assert.deepEqual(tabById("credentials")?.panels, ["vault", "clientKey"]);
  assert.deepEqual(tabById("ops")?.panels, ["circuit", "usage"]);
  assert.deepEqual(tabById("checkin")?.panels, ["checkin"]);
});

test("未知 tab 值回落到默认 tab（不能让界面空掉）", () => {
  assert.equal(resolveTab("ops"), "ops");
  assert.equal(resolveTab("nope"), DEFAULT_TAB);
  assert.equal(resolveTab(null), DEFAULT_TAB);
  assert.equal(resolveTab(undefined), DEFAULT_TAB);
  assert.equal(resolveTab(""), DEFAULT_TAB);
});

test("面板能反查自己所在的组", () => {
  assert.equal(tabOfPanel("vault"), "credentials");
  assert.equal(tabOfPanel("checkin"), "checkin");
  assert.equal(tabOfPanel("usage"), "ops");
});

test("没有 localStorage 时读写都不报错", () => {
  // node 环境没有 localStorage → 读回默认、写也不炸
  assert.equal(readStoredTab(), DEFAULT_TAB);
  storeTab("ops");
  assert.equal(readStoredTab(), DEFAULT_TAB);
});

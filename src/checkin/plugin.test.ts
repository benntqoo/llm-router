import assert from "node:assert/strict";
import test from "node:test";

import {
  credentialState,
  describeCredentialState,
  describeReward,
  isBuiltinPlugin,
  pluginLabel,
  unitLabel,
} from "./plugin.ts";

test("登录态分三类：HAR 模板 / 辅轨型 / 接口型", () => {
  assert.equal(
    credentialState({ plugin: "har", needsSidecar: false, credentialReady: false }),
    "har",
  );
  assert.equal(
    credentialState({
      plugin: "minimax_code",
      needsSidecar: true,
      credentialReady: false,
    }),
    "sidecar",
  );
  assert.equal(
    credentialState({
      plugin: "modelscope",
      needsSidecar: false,
      credentialReady: false,
    }),
    "missing",
  );
  assert.equal(
    credentialState({ plugin: "modelscope", needsSidecar: false, credentialReady: true }),
    "ready",
  );
});

test("登录态说明是中文且能区分状态", () => {
  assert.ok(describeCredentialState("har").includes("HAR"));
  assert.ok(describeCredentialState("sidecar").includes("浏览器"));
  assert.ok(describeCredentialState("missing").includes("还没填"));
  assert.ok(describeCredentialState("ready").includes("已填"));
});

test("积分单位有中文名，未知单位原样显示", () => {
  assert.equal(unitLabel("magicube"), "魔粒");
  assert.equal(unitLabel("point"), "积分");
  assert.equal(unitLabel("whatever"), "whatever");
});

test("积分描述写的是「本次拿到」，不是「余额」", () => {
  assert.equal(describeReward(null), "还没有积分记录");
  const text = describeReward({ points: 400, unit: "magicube", updatedAt: 1_700_000_000 });
  assert.ok(text.includes("本次拿到 400 魔粒"), text);
  assert.ok(!text.includes("余额"), text);
});

test("插件展示名有兜底", () => {
  assert.equal(pluginLabel("modelscope"), "ModelScope（魔搭）");
  assert.equal(pluginLabel("minimax_code"), "MiniMax Code（网页版）");
  assert.equal(pluginLabel("har"), "HAR 模板");
  assert.equal(pluginLabel("别的"), "别的");
});

test("内置插件判定：只有 har 不是内置", () => {
  assert.equal(isBuiltinPlugin("har"), false);
  assert.equal(isBuiltinPlugin(""), false);
  assert.equal(isBuiltinPlugin("modelscope"), true);
});

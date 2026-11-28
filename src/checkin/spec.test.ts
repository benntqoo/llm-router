import assert from "node:assert/strict";
import test from "node:test";

import {
  describeSpec,
  emptyForm,
  formToSpec,
  parseJsonPathRule,
  parseJsonPathRules,
  parseSpecJson,
  placeholdersIn,
  validateSpec,
} from "./spec.ts";

test("占位符只认 {{token}} 与 {{today}}", () => {
  assert.deepEqual(placeholdersIn("a {{token}} b {{ today }}"), ["token", "today"]);
  assert.deepEqual(placeholdersIn("没有占位符"), []);
});

test("JSON 路径断言：路径 / 路径=期望值", () => {
  assert.deepEqual(parseJsonPathRule("data.ok"), { path: "data.ok" });
  assert.deepEqual(parseJsonPathRule("data.ok=true"), { path: "data.ok", equals: true });
  assert.deepEqual(parseJsonPathRule("data.n=123"), { path: "data.n", equals: 123 });
  assert.deepEqual(parseJsonPathRule('data.s="x"'), { path: "data.s", equals: "x" });
  // 不是 JSON 的期望值按字符串处理
  assert.deepEqual(parseJsonPathRule("data.s=hello"), { path: "data.s", equals: "hello" });
  assert.equal(parseJsonPathRule("   "), null);
});

test("多行断言忽略空行", () => {
  const rules = parseJsonPathRules("data.ok=true\n\n  \ndata.n=1");
  assert.equal(rules.length, 2);
  assert.equal(rules[1].path, "data.n");
});

test("表单能拼出 spec", () => {
  const form = {
    ...emptyForm(),
    id: "my-platform",
    name: "我的平台",
    method: "post",
    url: "https://example.com/api/checkin",
    assertStatus: "200",
    jsonPaths: "data.ok=true",
    rewardPath: "data.credits",
    rewardUnit: "point",
    balanceUrl: "https://example.com/api/balance",
    balancePath: "data.balance",
  };
  const spec = formToSpec(form);
  assert.equal(spec.id, "my-platform");
  assert.equal(spec.method, "POST", "方法要归一为大写");
  assert.equal(spec.assert?.status, 200);
  assert.deepEqual(spec.assert?.jsonPaths, [{ path: "data.ok", equals: true }]);
  assert.equal(spec.reward?.path, "data.credits");
  assert.equal(spec.balance?.path, "data.balance");
});

test("URL 里用了 {{token}} 才带 Authorization 头", () => {
  const withToken = formToSpec({
    ...emptyForm(),
    id: "a",
    name: "a",
    url: "https://example.com/api/checkin?t={{token}}",
  });
  assert.deepEqual(withToken.headers, [
    { name: "Authorization", value: "Bearer {{token}}" },
  ]);

  const withoutToken = formToSpec({
    ...emptyForm(),
    id: "b",
    name: "b",
    url: "https://example.com/api/checkin",
  });
  assert.equal(withoutToken.headers, undefined);
});

test("前端校验拦得住明显的错", () => {
  const base = { id: "my-platform", name: "我的平台", url: "https://example.com/x" };
  assert.equal(validateSpec(base), null);
  assert.ok(validateSpec({ ...base, id: "" })?.includes("id"));
  assert.ok(validateSpec({ ...base, id: "Bad Id" })?.includes("id"));
  assert.ok(validateSpec({ ...base, name: "" })?.includes("站点名"));
  assert.ok(validateSpec({ ...base, url: "ftp://example.com/x" })?.includes("http"));
  assert.ok(
    validateSpec({
      ...base,
      headers: [{ name: "X", value: "{{session}}" }],
    })?.includes("占位符"),
  );
});

test("JSON 解析有错要说人话", () => {
  assert.ok(parseSpecJson("").error);
  assert.ok(parseSpecJson("{不是 json").error);
  const ok = parseSpecJson('{"id":"a","name":"a","url":"https://example.com/x"}');
  assert.equal(ok.error, undefined);
  assert.equal(ok.spec?.id, "a");
});

test("摘要分清「本次拿到」与「当前余额」", () => {
  const text = describeSpec({
    id: "a",
    name: "a",
    method: "post",
    url: "https://example.com/api/checkin",
    reward: { path: "data.credits" },
    balance: { url: "https://example.com/api/balance", path: "data.balance" },
  });
  assert.ok(text.includes("POST https://example.com/api/checkin"), text);
  assert.ok(text.includes("本次拿到：data.credits"), text);
  assert.ok(text.includes("当前余额：data.balance"), text);
});

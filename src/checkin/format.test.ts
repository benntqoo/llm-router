import assert from "node:assert/strict";
import test from "node:test";

import {
  VAR_NAME_MAX_LEN,
  defaultSiteName,
  entryLine,
  filterEntries,
  isValidVarName,
  sanitizeVarName,
  sensitiveCount,
  shortUrl,
  suggestEntryIndex,
  suggestVarName,
  targetKey,
  targetLabel,
  targetRawName,
} from "./format.ts";
import type { EntryView, FieldView } from "./types.ts";

const entries: EntryView[] = [
  {
    index: 0,
    method: "GET",
    url: "https://www.modelscope.cn/api/v1/users/login/info?x=1",
    status: 401,
    fields: [],
  },
  {
    index: 1,
    method: "POST",
    url: "https://www.modelscope.cn/api/v1/checkin",
    status: 200,
    fields: [],
  },
  {
    index: 2,
    method: "GET",
    url: "https://g.alicdn.com/app.js",
    status: 200,
    fields: [],
  },
];

test("target 的位置说明与后端口径一致", () => {
  assert.equal(
    targetLabel({ kind: "header", field: "Authorization" }),
    "请求头 Authorization",
  );
  assert.equal(
    targetLabel({ kind: "headerPart", field: "Cookie", part: "csrf_token" }),
    "请求头 Cookie · csrf_token",
  );
  assert.equal(
    targetLabel({ kind: "query", field: "channel" }),
    "查询参数 channel",
  );
  assert.equal(targetLabel({ kind: "body", field: "data.csrf" }), "请求体 data.csrf");
});

test("target 的原始名：Cookie 子项取子项名，其余取字段名", () => {
  assert.equal(
    targetRawName({ kind: "headerPart", field: "Cookie", part: "csrf_token" }),
    "csrf_token",
  );
  assert.equal(
    targetRawName({ kind: "header", field: "X-CSRF-Token" }),
    "X-CSRF-Token",
  );
});

test("变量名清洗：只保留合法字符", () => {
  assert.equal(sanitizeVarName("csrf_session"), "csrf_session");
  assert.equal(sanitizeVarName("X-CSRF-Token"), "x_csrf_token");
  assert.equal(sanitizeVarName("modelscope locale"), "modelscope_locale");
  assert.equal(sanitizeVarName("a..b"), "a_b");
  assert.equal(sanitizeVarName("__"), "");
});

test("建议变量名可被后端接受", () => {
  const name = suggestVarName({
    kind: "headerPart",
    field: "Cookie",
    part: "csrf_token",
  });
  assert.equal(name, "csrf_token");
  assert.ok(isValidVarName(name));
});

test("变量名合法性：与后端同规则（空 / 超长 / 非法字符都不行）", () => {
  assert.ok(isValidVarName("cookie"));
  assert.ok(isValidVarName("csrf-token_1"));
  assert.ok(!isValidVarName(""));
  assert.ok(!isValidVarName("has space"));
  assert.ok(!isValidVarName("变量"));
  assert.ok(!isValidVarName("x".repeat(VAR_NAME_MAX_LEN + 1)));
  assert.ok(isValidVarName("x".repeat(VAR_NAME_MAX_LEN)));
});

test("entry 过滤：按方法 / URL / 状态码都能命中，空关键字放行全部", () => {
  assert.equal(filterEntries(entries, "").length, 3);
  assert.equal(filterEntries(entries, "login/info").length, 1);
  assert.equal(filterEntries(entries, "POST").length, 1);
  assert.equal(filterEntries(entries, "401").length, 1);
  assert.equal(filterEntries(entries, "alicdn").length, 1);
  assert.equal(filterEntries(entries, "不存在的关键字").length, 0);
});

test("entry 摘要行：方法 + 去 query 的路径 + 状态码", () => {
  assert.equal(
    entryLine(entries[0]),
    "GET https://www.modelscope.cn/api/v1/users/login/info · 401",
  );
});

test("URL 缩略：去掉 query，超长截断加省略号", () => {
  assert.equal(shortUrl("https://a.invalid/x?y=1", 80), "https://a.invalid/x");
  const long = "https://a.invalid/" + "z".repeat(100);
  const short = shortUrl(long, 20);
  assert.equal(short.length, 20);
  assert.ok(short.endsWith("…"));
});

test("敏感字段计数", () => {
  const fields: FieldView[] = [
    {
      target: { kind: "header", field: "Authorization" },
      value: "***",
      sensitive: true,
      suggested: true,
    },
    {
      target: { kind: "header", field: "Accept" },
      value: "application/json",
      sensitive: false,
      suggested: false,
    },
  ];
  assert.equal(sensitiveCount(fields), 1);
  assert.equal(sensitiveCount([]), 0);
});

test("默认站点名：去掉 .har 后缀", () => {
  assert.equal(defaultSiteName("modelscope.har"), "modelscope");
  assert.equal(defaultSiteName("C:\\tmp\\modelscope.har"), "modelscope");
  assert.equal(defaultSiteName("modelscope"), "modelscope");
  assert.equal(defaultSiteName(".har"), "未命名站点");
});

test("target 键：区分头 / Cookie 子项 / 查询参数 / 请求体", () => {
  assert.equal(
    targetKey({ kind: "header", field: "Accept" }),
    "header:accept",
  );
  assert.equal(
    targetKey({ kind: "headerPart", field: "Cookie", part: "csrf_token" }),
    "headerPart:cookie:csrf_token",
  );
  assert.equal(targetKey({ kind: "query", field: "Channel" }), "query:channel");
  assert.equal(targetKey({ kind: "body", field: "data.csrf" }), "body:data.csrf");
  // 同一个 Cookie 里的不同子项要能分开
  assert.notEqual(
    targetKey({ kind: "headerPart", field: "Cookie", part: "csrf_token" }),
    targetKey({ kind: "headerPart", field: "Cookie", part: "csrf_session" }),
  );
});

test("默认选中的 entry：优先带 /api/ 的接口请求", () => {
  assert.equal(suggestEntryIndex(entries), 0);
  assert.equal(
    suggestEntryIndex([
      { index: 7, method: "GET", url: "https://x.invalid/app.js", status: 200, fields: [] },
      { index: 8, method: "POST", url: "https://x.invalid/api/v1/checkin", status: 200, fields: [] },
    ]),
    8,
  );
  assert.equal(suggestEntryIndex([]), null);
});

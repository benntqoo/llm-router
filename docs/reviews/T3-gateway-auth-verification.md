# T3 验收留档 — 网关骨架 + 入站鉴权

| 项 | 值 |
|---|---|
| 任务 | T3 网关服务骨架 + 入站鉴权（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-25 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 结论 | **通过**（判据逐条实测见 §1） |

## 1. 判据对照

任务卡原文：*目标：axum 服务、`/v1/chat/completions`、`/v1/models`、client key 校验（SHA-256）、统一错误结构（02 §4.4）、Host 校验、限流。验收：无 key 返回 401 统一结构；`/v1/models` 返回空别名列表。*

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | axum 服务 | ✅ | `/healthz`、`/v1/chat/completions`、`/v1/models` 三条路由，监听 `127.0.0.1:8787` |
| 2 | `/v1/chat/completions` | ✅ | 鉴权通过后进入路由阶段（503 + 统一结构，T4/T5 落地后接通） |
| 3 | `/v1/models` | ✅ | 带 key 返回 `{"object":"list","data":[]}`（当前无别名，故为空列表） |
| 4 | client key 校验（SHA-256） | ✅ | 只存哈希；端到端用例断言落库值 = SHA-256(key) 且 ≠ 明文 |
| 5 | 统一错误结构（02 §4.4） | ✅ | 真机 5 类错误全部返回 `{"error":{message,type,code?,channel?,retried[]}}` |
| 6 | Host 校验 | ✅ | 非回环 Host → 400 `invalid_request_error` |
| 7 | 限流 | ✅ | RPM 超限 → 429 `rate_limit_error`（单测 + 真机 HTTP） |
| 8 | **无 key → 401 统一结构** | ✅ | 真机 HTTP 实测（§2.2） |
| 9 | **`/v1/models` 返回空别名列表** | ✅ | 真机 HTTP 实测（§2.2） |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 62 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 4 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs
```

新增用例要点：

| 用例 | 覆盖点 |
|---|---|
| `gateway::server::tests::*`（7 个） | 401 统一结构 / 无效 key / 有效 key 进路由 / models 空列表 / Host 拒绝 / RPM 429 / 绑定策略 |
| `gateway::auth::tests::*`（5 个） | key 生成格式、哈希稳定性与长度、base62、key_prefix、两种传参头、Host 白名单 |
| `gateway::ratelimit::tests::*`（6 个） | RPM 拦截、窗口滑动、日 token 拦截与跨日重置、无限制、按 client 隔离 |
| `gateway::errors::tests::*`（3 个） | 统一错误体结构、状态码映射、五个 type 字面量与 02 §4.4 一致 |
| `gateway::inbound::openai::tests::*`（5 个） | model/stream 解析、缺 model、非对象 body、字段透传（R-8） |
| `acceptance_client_key_hash_is_stored_not_plaintext` | 落库只有哈希 |

### 2.2 真机 HTTP 验收（应用在跑，真实请求打 `127.0.0.1:8787`）

```
[PASS] 无 key → 401 统一结构: HTTP 401 type=authentication_error
        body={"error":{"message":"缺少 client key：请在 Authorization: Bearer 或 x-api-key 头中提供",
              "type":"authentication_error","retried":[]}}
[PASS] 错误 key → 401: HTTP 401 type=authentication_error
        body={"error":{"message":"client key 无效","type":"authentication_error","retried":[]}}
[PASS] 带 key GET /v1/models: HTTP 200 body={"object":"list","data":[]}
[PASS] 带 key POST /v1/chat/completions → 503（路由未接入，T4/T5）: HTTP 503 type=api_error
        body={"error":{"message":"网关出站尚未接入：别名 \"deepseek-chat\" 的路由与上游适配在 T4/T5 落地",
              "type":"api_error","retried":[]}}
[PASS] x-api-key 头同样可用: HTTP 200 body={"object":"list","data":[]}
[PASS] 非回环 Host → 拒绝: HTTP 400 type=invalid_request_error
        body={"error":{"message":"Host 校验失败：evil.example.com（网关只接受回环地址）",...}}
[PASS] 非法 JSON 体 → 400: HTTP 400 type=invalid_request_error
[PASS] /healthz: HTTP 200 body={"status":"ok","service":"llm-router","version":"0.1.0"}
ALL_PASS
```

### 2.3 绑定红线（AGENT.md §5-4）真机实测

```
=== 场景A：无 client key 时绑定 0.0.0.0 ===
已拒绝启动
Failed to setup app: error encountered during setup hook:
拒绝绑定非回环地址 0.0.0.0:8788：改绑必须先存在有效 client key（AGENT.md §5-4），否则本机将变成开放代理

=== 场景B：已有 client key 时绑定 0.0.0.0 ===
允许启动（符合红线）
```

> T0 阶段的策略是"非回环一律拒绝"（当时还没有 client key 概念）；T3 按红线细化为
> "非回环 + 已存在有效 client key 才允许"。T0 留档 §4 已补指针。

### 2.4 界面实测

`assets/T3-client-key-panel.png`：Client Key 面板（创建 / 列表 / 删除），列表项显示
`llmr_qph… · seeder · 2026/11/25 19:31:34`，**不含明文**。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/gateway/errors.rs` | 02 §4.4 统一错误结构 + 状态码映射 + `IntoResponse` |
| `src-tauri/src/gateway/auth.rs` | client key 生成（`llmr_` + 32B base62）、SHA-256 哈希、鉴权、Host 校验 |
| `src-tauri/src/gateway/ratelimit.rs` | RPM 滑窗 + daily_tokens（UTC 日切分），内存计数 |
| `src-tauri/src/gateway/inbound/openai.rs` | 入站解析（model/stream，其余字段透传）+ `/v1/models` 响应结构 |
| `src-tauri/src/gateway/server.rs` | 路由表、绑定策略、三个处理函数 |
| `src-tauri/src/commands/client_key.rs` | client key 创建/列表/改限额/删除 IPC |
| `src-tauri/tests/gateway_e2e.rs` | 端到端验收用例（4 个）+ 联调种入工具 |
| `src/ClientKeyPanel.tsx` | client key 面板（最小接入，T9 接管） |

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| `/v1/models` 也要求 client key | 02 §3.1 的入站鉴权覆盖整个客户端面；模型别名列表同样属于需授权的资源 |
| `daily_tokens` 计数来源 | T3 阶段恒为 0（token 统计随 T7 用量采集回填）；限流器已预留 `record_tokens` 接口 |
| 限流计数存内存 | 与熔断冷却同理（02 §2.3），重启即清零 |
| `daily_tokens` 按 UTC 日切分 | 未引入时区库；本地单用户工具，误差可接受 |
| 统一错误体的 `code`/`channel` | 本阶段无上游，故省略（`skip_serializing_if`）；T4/T5 接入上游错误码后填充 |
| 403 vs 400 | Host 校验失败归类为 `invalid_request_error` + HTTP 400（02 §4.4 的 type 枚举里没有 403 类） |
| client key 明文只在创建时返回 | 02 §3.1 明确：明文只展示一次，之后不可回看；IPC 层不写日志 |
| 前端 client key 面板 | T3 任务卡未含 UI，但 key 无处创建则网关不可用，故做最小面板；**T9 将接管完整 UI** |

## 5. 未做

- 别名解析与路由（T5）、出站适配器（T4）、流式管道与熔断（T6）
- `/v1/models` 的别名数据（依赖 T5 建 `model` 表数据；当前表为空，故返回空列表，正合验收判据）
- 网关请求日志与脱敏中间件接线（`vault::redact` 已就绪，随 T4/T6 接入）

# T6 验收留档 — 流式管道 + fallback + 熔断

| 项 | 值 |
|---|---|
| 任务 | T6 流式管道 + fallback + 熔断（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `ad50996` |
| 结论 | **通过**（判据逐条实测见 §1） |

## 1. 判据对照

任务卡原文：*目标：SSE 零缓冲直通、首字节前重试、客户端断连 abort 上游、熔断状态机、P-4 学习式兼容、P-7 `X-Routed-Via` 响应头注入、429 响应 `Retry-After`/`x-ratelimit-*` 解析进冷却参数。验收：mock 上游可复现"首字节后失败不重试""断连取消上游""3 连败熔断""429 冷却时长遵循 Retry-After"四个场景（用内存 mock server 测试）。*

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **首字节后失败不重试** | ✅ | §2.2 场景 1、§2.4 真机（上游只被打 1 次，补 SSE error 事件） |
| 2 | **断连取消上游** | ✅ | §2.2 场景 2、§2.4 真机（上游观测到连接被取消） |
| 3 | **3 连败熔断** | ✅ | §2.2 场景 3、§2.4 真机（第 4 次不打上游） |
| 4 | **429 冷却时长遵循 Retry-After** | ✅ | §2.2 场景 4、§2.4 真机（冷却 ≈5s 而非退避 2s / 上限 60s） |
| 5 | SSE 零缓冲直通 | ✅ | §2.3（首帧 <300ms，逐帧累加到达） |
| 6 | 首字节前重试 | ✅ | §2.3（5xx → 切备选渠道） |
| 7 | 熔断状态机 | ✅ | §2.2（closed→open→half_open→closed/failed 探测） |
| 8 | P-4 学习式兼容 | ✅ | §2.3（400 提到 `stream_options` → 降级重试同一候选） |
| 9 | P-7 `X-Routed-Via` | ✅ | §2.3、§2.4 真机（含 ASCII 安全编码） |
| 10 | 429 `Retry-After`/`x-ratelimit-*` 进冷却参数 | ✅ | §2.2、§2.4 真机 |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 147 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 5 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs
test result: ok. 6 passed;  0 failed; 0 ignored                                 # tests/outbound_e2e.rs（T4）
test result: ok. 11 passed; 0 failed; 0 ignored                                 # tests/router_e2e.rs（T5）
test result: ok. 11 passed; 0 failed; 0 ignored                                 # tests/stream_e2e.rs（T6 新增）
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs（T2）
```

编译告警：0。前端 `npm run build` 通过（`tsc && vite build`，20 modules transformed）。

> 单元测试数 117 → 147：新增 30 个（熔断 11 / 出站 6 / 流式 4 / fallback 7 / 错误体 2）。

### 2.2 四个验收场景（`tests/stream_e2e.rs`，内存 mock server）

| 场景 | 用例 | 断言要点 |
|---|---|---|
| 首字节后失败不重试 | `acceptance_failure_after_first_byte_does_not_retry` | 首帧 `chunk-0` 已透传后上游断流 → 补 SSE `error` 事件；**备选渠道命中数 = 0**（已写首字节不得重试） |
| 断连取消上游 | `acceptance_client_disconnect_aborts_upstream` | 客户端 drop 响应体后，上游流的 guard 被 drop → 上游观测到连接关闭（避免上游空跑计费） |
| 3 连败熔断 | `acceptance_three_consecutive_failures_open_circuit` | 前 3 次 502 + 上游被打 3 次；第 4 次**不再打上游**，502 且错误信息含"熔断" |
| 429 冷却遵循 Retry-After | `acceptance_rate_limit_cooldown_follows_retry_after` | 冷却截止时间 = `Retry-After: 2`；冷却期内不再打上游，回 429 + `Retry-After` 剩余秒数 |

熔断状态机本身由 11 个单测覆盖（`gateway/circuit.rs`）：连败 3 次 open、open 60s → half_open 放**一次**探测（探测未回结果前不再放行，防并发穿透）、探测成功 closed / 探测失败重新 open 且冷却重新计时、成功清零失败计数、429 指数退避（2/4/8/16/32/60s 封顶）、`Retry-After` 与退避取大者、key 互不影响、`reset` 清零。全部用**注入时钟**（`CircuitBreaker::with_clock`），不依赖真实时间。

### 2.3 附加验收

| 项 | 用例 | 断言要点 |
|---|---|---|
| SSE 零缓冲 | `acceptance_sse_is_zero_buffer` | 首帧 <300ms 到达；第 5 帧 ≥400ms（逐帧累加，非全量缓冲）；`content-type: text/event-stream` 保持 |
| 首字节前重试 | `acceptance_fallback_switches_candidate_before_first_byte` | 首选渠道 500 → 切备选渠道 200；`X-Routed-Via: Healthy/deepseek-chat` |
| P-4 学习式兼容 | `acceptance_p4_learns_to_disable_stream_usage_injection` | 上游 400 提到 `stream_options` → 同渠道重试一次且**不再带注入**；该渠道本次会话内标记为降级 |
| P-3 MiniMax 业务错误 | `acceptance_minimax_business_error_is_mapped` | HTTP 200 + `base_resp.status_code=1004` → 映射 401 + `code: minimax_1004` + 提示查 GroupId |
| P-7 `X-Routed-Via` | `acceptance_response_has_routed_via_header` | `DeepSeek/deepseek-chat`；非流式 body 原样透传（`chatcmpl-1`） |
| P-7 ASCII 安全 | `acceptance_routed_via_is_ascii_safe_for_chinese_channel_name` | 中文渠道名 → 百分号编码（`DeepSeek 官方` → `DeepSeek %E5%AE%98%E6%96%B9`），头值全 ASCII |
| 4xx 不可重试 | `acceptance_unauthorized_is_not_retried` | 400 直接回传，**不切候选**（备选命中数 = 0） |

### 2.4 真机 HTTP 验收（应用在跑，真实请求打 `127.0.0.1:8787`）

应用以**独立数据目录**启动（`LLMR_STORE_PATH` / `LLMR_VAULT_PATH` 指向临时目录，不污染用户真实库）：

```
[store] 数据库 C:\Users\Ben\AppData\Local\Temp\llmr-t6-verify\store.db（schema v4）
[vault] 保险库 C:\Users\Ben\AppData\Local\Temp\llmr-t6-verify\vault.bin（主密钥 v1）
[gateway] 监听 http://127.0.0.1:8787/v1（OpenAI 兼容入站）
```

种入数据：渠道 `DeepSeek 官方`（provider=deepseek，凭证由 vault 真实加密写入）、5 个模型别名
（`demo-model`/`midfail-model`/`abort-model`/`fail-model`/`rl-model`，**upstream_model 各不相同**，
使熔断粒度 `(channel_id, upstream_model)` 互不干扰）、client key（只存哈希 + 前缀）。

```
[PASS] healthz 200: HTTP 200
[PASS] SSE 逐帧到达（首帧 < 0.6s，末帧 >= 0.3s）: 帧数=4 到达时刻=[0.003, 0.153, 0.304, 0.455]
[PASS] [DONE] 直通: 尾部=data: [DONE]
[PASS] X-Routed-Via 头存在: X-Routed-Via=DeepSeek %E5%AE%98%E6%96%B9/deepseek-chat
[PASS] X-Routed-Via 为 ASCII 安全值（中文渠道名百分号编码）: X-Routed-Via='DeepSeek %E5%AE%98%E6%96%B9/deepseek-chat'
[PASS] 流式响应 200: HTTP 200
[PASS] 非流式 body 透传: HTTP 200 body=b'{"id": "chatcmpl-1", "choices": [{"message": {"role": "assistant", "content": "hello"}}], "usage": {"prompt_tokens": 3, '
[PASS] 上游 5xx → 502 统一错误结构: HTTP 502 body=b'{"error":{"message":"上游 DeepSeek 官方 返回 500 Internal Server Error","type":"upstream_error","channel":"DeepSeek 官方","retried":["DeepSeek 官方"]}}'
[PASS] 首字节后失败不重试（上游只被打 1 次）: mock midfail 计数 1 → 2
[PASS] 首字节后失败：补 SSE error 事件: HTTP 200 body='data: {"id": "chunk-0"}\n\ndata: {"error":{"channel":"DeepSeek 官方","code":null,"message":"上游 DeepSeek 官方 流式响应中断","retried":[],"type":"upstream_error"}}\n\n'
[PASS] 客户端断连后上游连接被取消（不再空跑计费）: mock aborted 计数 1 → 2
[PASS] 前 3 次失败：都打到上游并返回 502: 状态码=[502, 502, 502] mock 计数 5 → 8
[PASS] 第 4 次：熔断已开，不再打上游: mock 计数保持 8
[PASS] 第 4 次：502 且错误信息说明熔断: HTTP 502 message='没有可用候选：全部候选被跳过或熔断中'
[PASS] 上游 429 → 429 统一错误结构 + Retry-After: 5: HTTP 429 Retry-After=5 body=b'{"error":{"message":"上游 DeepSeek 官方 限流（429），冷却 5s","type":"rate_limit_error",...}}'
[PASS] 429 只打上游一次: mock 计数 4 → 5
[PASS] 冷却期(t+3s)：不再打上游（说明冷却 >= 3s，遵循 Retry-After=5）: 第二次后 mock 计数=5（第一次=5），HTTP=429
[PASS] 冷却期(t+3s)：回 429 + Retry-After 剩余秒数: HTTP 429 Retry-After=2
[PASS] 冷却到期(t+6s)：重新打上游（冷却时长≈5s 而非 60s）: mock 计数 5 → 6，HTTP=429
[PASS] 凭证零明文：数据库不含 client key 明文: db 字节扫描长度=126976
ALL_PASS
```

**429 冷却时长的判别设计**：mock 返回 `Retry-After: 5`（网关的指数退避基数是 2s、上限 60s）。
若网关忽略 `Retry-After`，冷却会是 2s → t+3s 那次请求应当打到上游；实测**没有打到上游**，
说明冷却 ≥3s（取了 `Retry-After`）；t+6s 又**重新打到上游**，说明冷却 ≈5s 而不是 60s 上限。
两项合起来证明冷却时长确实跟随 `Retry-After`。

### 2.5 凭证零明文复查（AGENT.md §5）

- 凭证由 `vault_e2e --ignored seed_demo_credential` 经**真实 vault 加密**写入（`AES-256-GCM` + OS 原生主密钥），库中只存密文 + nonce。
- 真机验收后扫描 `store.db` 字节：client key 明文**未命中**。
- 出站请求头的 `Authorization: Bearer <凭证>` 由 vault 在**内存中解密**注入，不落盘、不进日志。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/gateway/circuit.rs` | `CircuitBreaker`（closed/open/half_open）、`OpenReason`、`rate_limit_backoff`；11 个单测（注入时钟） |
| `src-tauri/src/gateway/outbound.rs` | `OutboundClient::send`（连接超时 30s / 首字节超时 600s）、`UpstreamBody::{Buffered,Stream}`、`RateLimitInfo`、`mentions_stream_options`、`StreamUsageDowngrades`（P-4） |
| `src-tauri/src/gateway/stream.rs` | `into_body` / `GuardedStream`（逐 chunk 直通 + 中途失败补 SSE error 事件）、`format_sse_event`、`error_event` |
| `src-tauri/src/gateway/fallback.rs` | `Forwarder::forward`（候选链消费、熔断跳过、首字节前重试、P-4 降级重试、P-3 业务错误改写、P-7 `X-Routed-Via`）、`circuit_key`、`ascii_safe` |
| `src-tauri/src/gateway/errors.rs` | 统一错误体新增 `Retry-After` 响应头承载（不进 JSON 体） |
| `src-tauri/src/gateway/server.rs` | `GatewayState` 增加 `forwarder`；`chat_completions` 接到出站转发 |
| `src-tauri/src/gateway/router.rs` | `RouteCandidate` 增加 `channel_kind` / `channel_base_url` / `channel_meta_json`（免二次查库构造出站配置） |
| `src-tauri/src/lib.rs` | 启动时构造 `Forwarder`（reqwest + 熔断 + 降级表 + 保险库共享句柄） |
| `src-tauri/tests/stream_e2e.rs` | 端到端验收用例（11 个，内存 mock server） |
| `src-tauri/tests/common/mod.rs` | 共享 mock 上游工具（`start_mock` / `mock_always_500`） |

新增依赖：`reqwest 0.12.28`（`rustls-tls` + `stream`，**不开 default-features**：native-tls 需系统 OpenSSL）、`futures-util 0.3`、`bytes 1`；dev-dependency `http-body-util 0.1`。

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| **重试窗口 = 拿到上游 2xx 响应头之前** | 一旦拿到上游响应（状态码与响应头已写给客户端），就不再重试：重试会产生重复内容（02 §3.3"中途失败"）。只有连接层错误 / 429 / 5xx 在响应前才允许切候选 |
| **4xx 不可重试，且不计入熔断** | 401/403 → `authentication_error`；400/404/422 → `invalid_request_error`；保留上游状态码与 message/code。只有传输错误与 5xx 计熔断失败次数 |
| **熔断状态只存内存** | 与 02 §2.3 同口径，重启即清零（真机验收时重启应用即验证）。粒度 `(channel_id, upstream_model)` |
| **429 冷却 = `max(指数退避, Retry-After)`** | 指数退避基数 2s、逐次翻倍、上限 60s；`Retry-After` 与 `x-ratelimit-reset-requests` 取较大值。冷却期内**不打上游** |
| **冷却期回 429 + `Retry-After`（行为新增）** | 原本"候选全被熔断跳过"会回 502 `upstream_error`（"没有可用候选"），把"限流"这个真实原因藏起来了。现在区分打开原因：限流冷却 → 429 `rate_limit_error` + `Retry-After` 剩余秒数；连续失败熔断 → 仍回 502 + "熔断"说明。`Retry-After` **只走响应头，不进错误体**（02 §4.4 字段表未变） |
| **上游错误响应体一律整块缓冲** | 即使请求是流式：错误分类与 P-4 判定都需要读出错误体（"stream_options" 关键词） |
| **P-4 只在流式请求上判定** | `injection_enabled = request.stream && config.inject_stream_usage && !downgraded`——非流式请求本来就不注入 `stream_options`，上游报 400 不能算"注入被拒"。降级状态**只存内存**（本次会话生效，不落库） |
| **P-3 实现范围** | 非流式响应完整解析 `base_resp.status_code` 并改写状态码/错误体；**流式的 P-3 分支未做**（要解析 SSE 帧），留待 T7 usage tee 一并接入 |
| **P-7 `X-Routed-Via` 值做 ASCII 百分号编码** | 渠道名是用户自定义的，可能是中文；HTTP 头值必须 ASCII，否则客户端 `to_str()` 读不出来。ASCII 部分原样、空格保留 |
| **上游响应头透传** | 除 hop-by-hop 与 `content-length` 外原样透传（P-6） |
| **401 的"凭证失效标记"未实现** | DDL 中 `credential` 与 `account` 无外键关联，无处可标记；留待 T8/T9 账号池落地 |
| **保险库锁定态直接拒绝转发** | 保险库锁定（主密码模式未解锁）时返回 `api_error`，提示先在应用内解锁——不拿明文凭证去试 |
| **熔断打开的 60s 冷却无法在真机里等待验证** | 真机只验证了"熔断已开 → 不再打上游"；60s 恢复与 half_open 探测由单元测试（注入时钟）覆盖 |

## 5. 未做

- 流式 usage 采集与 `usage_log` 落库、R-4 注入的 usage 帧 tee —— T7
- 流式响应的 P-3 业务错误判定（SSE 帧解析）—— 随 T7 接入
- 多账号池（同一渠道多凭证轮换）、配额自学习回写 `quota_json` —— T7/T8/T9
- 熔断/冷却状态的持久化与 UI 展示 —— 按 02 §2.3 不持久化；UI 属 T8/T9
- 上游真实渠道的联调（DeepSeek/Kimi/GLM 等）—— T10 实测矩阵

## 6. 待裁决

| # | 项 | 说明 |
|---|---|---|
| 1 | **限流冷却期内回 429 + `Retry-After` 响应头**是否需要回填 02 §4.4 | 02 §3.3 把 429 单列为一类错误，§2.3 只说"429 触发连接冷却"，未写冷却期内回给客户端什么。已按"限流就该说限流"实现为 429 + 响应头（**错误体字段表未改**），是否回填文档待头头裁决 |
| 2 | 熔断状态是否需要落库 / 给 UI 看 | 02 §2.3 明确"冷却状态存内存，重启即清零（不做持久化，避免误伤）"，当前按此实现；若要在 UI 展示"某渠道正在冷却"，需要额外设计 |

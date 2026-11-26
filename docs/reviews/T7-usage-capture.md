# T7 验收留档 — usage 采集（流式 tee / 非流式直读 / 估算入口 / 落库 / 限额自学习）

| 项 | 值 |
|---|---|
| 任务 | T7 usage 采集（03 §2 任务卡；关键路径 T7→T10） |
| 依据 | `docs/planning/02-technical-design.md` §8.1 / §8.2（v0.6）；`docs/planning/03-delivery-plan.md` T7 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `59053b2`（`feat: T7 usage 采集（流式 tee 解析 usage 帧 + estimated 估算入口 + usage_log 落库 + 429 限额自学习）`） |
| 结论 | **通过**（任务卡两条判据均实测留证，见 §1） |

> 本文件改动范围：仅记录 T7 的验收判据、实测证据、交付物与取舍；**不含**任何后续任务的结论。

## 1. 判据对照

任务卡验收原文：「流式/非流式各一条用例，usage_log 字段与上游返回一致；429 触发后 quota_json 记录更新。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **流式**：`usage_log` 字段与上游返回一致 | ✅ | §2.3 真机：上游 usage 帧 `prompt_tokens=42 / completion_tokens=9` → 落库 `42 / 9`、`estimated=0`、`status=success`、有 TTFT（51ms） |
| 2 | **非流式**：`usage_log` 字段与上游返回一致 | ✅ | §2.3 真机：上游 `usage`（31 / 7 / cached 5 / reasoning 3）→ 落库逐字段一致、`estimated=0`、`ttft_ms=NULL` |
| 3 | **429 触发后 `quota_json` 记录更新** | ✅ | §2.3 真机：429 后 `account.quota_json` = `{"rpm":60,"tpm":120000,"source":"upstream_headers",...,"last_rate_limit":{"retry_after_secs":5,"remaining_requests":0}}` |
| 4 | 一条记录 = 一次出站尝试 | ✅ | §2.4 真机：4 次请求 → 4 行（`t7-usage` / `t7-sse` / `t7-nousage` / `t7-rl`），fallback 试 N 个候选即 N 行 |
| 5 | 上游没回 usage → 走估算入口 | ✅ | §2.3 真机：`estimated=1`、`prompt_tokens=1`、`completion_tokens=4`（启发式估算，非上游值） |
| 6 | 失败尝试记 `error` 行且**不估 token** | ✅ | §2.3 真机：429 那次 `status=error` / `error_kind=rate_limit_error` / token 全 0 / `estimated=0` |
| 7 | 迁移 v7 可升可降、**真实旧库**能补列 | ✅ | §2.3 真机：T6 时期的旧库（无 `account_id` 列）启动后补出该列并记 `schema_migrations` v7；§2.2 单测覆盖 up/down |
| 8 | 凭证零明文（AGENT.md §5） | ✅ | §2.5：库内无 client key 明文；`vault.bin` 无 `sk-` / `Bearer ` |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 208 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 5 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs
test result: ok. 6 passed;  0 failed; 0 ignored                                 # tests/outbound_e2e.rs（T4）
test result: ok. 11 passed; 0 failed; 0 ignored                                 # tests/router_e2e.rs（T5）
test result: ok. 13 passed; 0 failed; 0 ignored                                 # tests/stream_e2e.rs（T6）
test result: ok. 5 passed;  0 failed; 0 ignored                                 # tests/usage_e2e.rs（T7，本次新增）
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs（T2）
```

编译告警：0。前端 `npm run build` 通过（`tsc && vite build`）。

> 单元测试 180 → **208**（+28：usage 解析与 tee 17 + 落库与窗口计数 5 + 限额自学习 6）；集成测试新增 5 条（`usage_e2e.rs`）。全量 **251** 项，0 失败。

### 2.2 新增用例

| 层 | 用例 | 断言要点 |
|---|---|---|
| 单测 | `parses_openai_usage` / `parses_minimax_reasoning_tokens` / `parses_anthropic_style_usage` | OpenAI / Anthropic 两套命名都认；`cached_tokens`→cache_read、`cache_creation_input_tokens`→cache_write、`completion_tokens_details.reasoning_tokens`→reasoning |
| 单测 | `missing_usage_is_none` | 没有 `usage` 键 / `usage` 非对象 / **`usage` 为空对象** / 无已知字段 → 一律不是"上游回报" |
| 单测 | `sse_scanner_splits_events` / `handles_split_chunks` / `handles_crlf_and_skips_done` / `flushes_trailing_frame` | SSE 帧切分：空行边界、跨块、`\r\n\r\n`、`[DONE]` 跳过、末帧冲刷 |
| 单测 | `tap_captures_usage_frame` | 流末 usage 帧被抓到，`estimated=false` |
| 单测 | `tap_flushes_trailing_frame_without_blank_line` | 上游没用空行收尾时，末帧靠流结束冲刷拿到（**不能丢 usage 帧**） |
| 单测 | `tap_records_ttft_and_estimates_without_upstream_usage` | 无 usage → `estimated=true`；首个 token 到达即记 TTFT |
| 单测 | `tap_marks_upstream_error` / `tap_marks_aborted_when_dropped_mid_stream` | 上游报错 → `error`/`upstream_error`；客户端断连 → `aborted` |
| 单测 | `estimator_counts_cjk_as_one_token` / `extracts_prompt_text_from_messages` / `extracts_completion_text_from_choices` | 启发式估算与文本抽取 |
| 单测 | `usage_row_round_trips` | 落库行字段逐个回读一致 |
| 单测 | `window_counters_split_minute_and_day` | RPM/TPM 只算最近 60s，RPD/TPD 只算 UTC 当日 |
| 单测 | `counters_are_scoped_to_channel_and_account` | 计数按 `(channel, account)` 隔离 |
| 单测 | `default_account_is_created_on_demand` | API Key 型渠道按需补建默认账号 `acct_default_<channel_id>` |
| 单测 | `learning_writes_the_ceiling_into_quota_json` | 429 学到的上限写进 `quota_json`，`last_rate_limit` 记事实 |
| 单测 | `learning_only_tightens_never_relaxes` | 只收紧：更宽松的上限**不覆盖**已记录值 |
| 单测 | `user_declared_ceiling_is_also_only_tightened` | 用户手填的上限同样只会被压低，不会被抬高 |
| 单测 | `corrupted_quota_json_reads_as_default_state` | 坏 JSON 不让读取失败 |
| e2e | `acceptance_non_stream_usage_log_matches_upstream` | 非流式：`usage_log` 与上游逐字段一致 |
| e2e | `acceptance_stream_usage_log_matches_upstream` | 流式：tee 抓到末帧 usage，`ttft_ms` 有值 |
| e2e | `acceptance_rate_limit_updates_quota_json` | 429 → `quota_json` 更新 + 一条 `error` 用量 |
| e2e | `acceptance_success_without_upstream_usage_is_estimated` | 估算入口 |
| e2e | `acceptance_failed_attempt_is_logged_without_tokens` | 失败行不估 token |

### 2.3 真机 HTTP 验收（应用实跑，`127.0.0.1:8787`）

环境：mock 上游 `127.0.0.1:18788`（Python 标准库，场景 `/usage` `/nousage` `/sseusage` `/fail429limits` `/fail500`）；应用用 **T6 时期的旧库**（`usage_log` 无 `account_id` 列）启动，走真实迁移 v7。

```
[PASS] 迁移 v7 在真实旧库上补出 usage_log.account_id: 列=[..., 'cost_micros', 'account_id'] 版本=[1,2,3,4,5,6,7]
[PASS] 非流式请求 200 且上游 usage 原样透传: HTTP 200 usage={'prompt_tokens': 31, 'completion_tokens': 7, 'prompt_tokens_details': {'cached_tokens': 5}, 'completion_tokens_details': {'reasoning_tokens': 3}}
[PASS] 非流式 usage_log 与上游一致（token / 缓存 / 推理）: 行={'prompt_tokens': 31, 'completion_tokens': 7, 'cache_read_tokens': 5, 'reasoning_tokens': 3}
[PASS] 非流式 usage_log 字段完整（渠道/账号/别名/协议/状态）: 行={'channel_id': 'ch_baeb...', 'account_id': 'acct_default_ch_baeb...', 'model_alias': 't7-usage', 'upstream_model': 'deepseek-chat', 'protocol': 'openai', 'client_key_id': 'ck_t7', 'status': 'success', 'estimated': 0, 'ttft_ms': None, 'cost_micros': None}
[PASS] 流式请求 200 且 usage 帧原样透传: HTTP 200 usage_frame={'prompt_tokens': 42, 'completion_tokens': 9}
[PASS] 流式 usage_log 与上游一致（tee 抓到末帧 usage）: 行={'prompt_tokens': 42, 'completion_tokens': 9}
[PASS] 流式 usage_log 标 success / 非估算 / 有 TTFT: 行={'status': 'success', 'estimated': 0, 'ttft_ms': 51, 'model_alias': 't7-sse', 'duration_ms': 151}
[PASS] 上游无 usage → 走估算入口（estimated=1 且 token > 0）: HTTP 200 行={'estimated': 1, 'prompt_tokens': 1, 'completion_tokens': 4}
[PASS] 上游 429 → 429 统一错误结构: HTTP 429 body=b'{"error":{"message":"\xe4\xb8\x8a\xe6\xb8\xb8 DeepSeek \xe5\xae\x98\xe6\x96\xb9 \xe9\x99\x90\xe6\xb5\x81\xef\xbc\x88429\xef\xbc\x89\xef\xbc\x8c\xe5\x86\xb7\xe5\x8d\xb4 5s","type":"rate_limit_error",...}}'
[PASS] 429 记一条 error 用量（token 全 0、不估算）: 行={'status': 'error', 'error_kind': 'rate_limit_error', 'prompt_tokens': 0, 'completion_tokens': 0, 'estimated': 0}
[PASS] 429 后 quota_json 记入学到的上限（RPM=60 / TPM=120000）: quota_json={'rpm': 60, 'tpm': 120000, 'source': 'upstream_headers', 'learned_at': 1795662802, 'last_rate_limit': {'ts': 1795662802, 'retry_after_secs': 5, 'remaining_requests': 0}}
[PASS] quota_json.last_rate_limit 记下限流事实（事实，非推断）: last_rate_limit={'ts': 1795662802, 'retry_after_secs': 5, 'remaining_requests': 0}
[PASS] 一条 usage_log = 一次出站尝试（本轮 4 次请求 = 4 行）: 行数=4 摘要=[('t7-usage', 'success'), ('t7-sse', 'success'), ('t7-nousage', 'success'), ('t7-rl', 'error')]
[PASS] 凭证零明文：数据库不含 client key 明文: db 字节扫描长度=143360
ALL_PASS
```

### 2.4 库内证据（验收后 `usage_log` 全量）

| id | alias | prompt | completion | cache_read | reasoning | estimated | ttft_ms | status | error_kind | cost_micros |
|---|---|---|---|---|---|---|---|---|---|---|
| 5 | `t7-usage` | 31 | 7 | 5 | 3 | 0 | NULL | success | NULL | NULL |
| 6 | `t7-sse` | 42 | 9 | 0 | 0 | 0 | 51 | success | NULL | NULL |
| 7 | `t7-nousage` | 1 | 4 | 0 | 0 | **1** | NULL | success | NULL | NULL |
| 8 | `t7-rl` | 0 | 0 | 0 | 0 | 0 | NULL | **error** | `rate_limit_error` | NULL |

`account`（限额自学习的唯一落点）：

```json
{"id":"acct_default_ch_baeb2b80cd4bc153283563ce356e60cb","channel_id":"ch_baeb2b80cd4bc153283563ce356e60cb","label":"默认账号",
 "quota_json":"{\"rpm\":60,\"tpm\":120000,\"source\":\"upstream_headers\",\"learned_at\":1795662802,\"last_rate_limit\":{\"ts\":1795662802,\"retry_after_secs\":5,\"remaining_requests\":0}}"}
```

`schema_migrations`：v1–v7 齐（v7 = `usage_log_account_id`，`applied_at=1795662786`）。

### 2.5 凭证零明文复查（AGENT.md §9 相关项）

- `store.db` 全表 + DDL 扫描：client key **明文**未命中。
- 关键词命中来源逐条核对，均为**已知口径项**（账本里 🔴 那条待裁决项）：DDL 注释里的 `api_key` 字样、`channel.kind` / `credential.kind` 的枚举值 `api_key`、以及 `client_key.key_prefix`（T3 起就有的"前 8 位供 UI 展示"，非完整密钥）。
- `vault.bin` 扫描：无 `sk-`、无 `Bearer `。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/gateway/usage.rs`（新） | usage 解析（OpenAI / Anthropic 两套命名 + 缓存 / 推理 token）、`SseScanner`（SSE 帧切分）、`UsageTap`（**旁路 tee**：转发字节同时扫帧找 usage、记 TTFT、累积输出文本）、`TokenEstimator` trait + `CharRatioEstimator`（启发式占位）、`resolve_tokens`（上游优先，否则估算）、`extract_prompt_text` / `extract_completion_text` |
| `src-tauri/src/store/usage.rs`（新） | `UsageRecord` / `UsageStatus` / `UsageSink` trait / `StoreUsageSink`（`spawn_blocking` 落库）、`insert_usage`、`QuotaCounters` + `window_counters`（四个窗口从 `usage_log` 派生）、`utc_day_start` |
| `src-tauri/src/store/quota.rs`（新） | `QuotaState` / `RateLimitFact` / `RateLimitObservation`、`default_account_id`、`ensure_account`、`read_quota` / `write_quota`、`learn_rate_limit`（**只收紧不放宽**） |
| `src-tauri/src/store/schema.rs` | `usage_log` 增 `account_id` 列（DDL 唯一权威载体）+ `ensure_usage_log_account_id`（幂等补列） |
| `src-tauri/src/store/migrations.rs` | **v7 `usage_log_account_id`** 迁移（可 up 可 down；up 走幂等补列函数，新库建表即含该列） |
| `src-tauri/src/gateway/fallback.rs` | `Forwarder` 接入 `store` / `usage_sink` / `estimator`；`forward()` 透传 `client_key_id`；成功/失败/429 各路径记用量；流式分支用 `UsageTap` 包住上游流；429 分支调 `learn_quota` |
| `src-tauri/src/gateway/outbound.rs` | `RateLimitInfo` 增 `limit_requests` / `limit_tokens`（解析 `x-ratelimit-limit-*`，供自学习用） |
| `src-tauri/src/gateway/errors.rs` | `ErrorKind::as_str()`（错误类别落库用） |
| `src-tauri/src/gateway/server.rs` | 入站 client key id 透传到转发层 |
| `src-tauri/src/lib.rs` | 构造 `StoreUsageSink` + `CharRatioEstimator` 并注入 `Forwarder` |
| `src-tauri/tests/usage_e2e.rs`（新） | 5 条端到端验收用例（§2.2 表末五行） |
| `docs/planning/02-technical-design.md` | §8.1 采集口径 / §8.2 账号口径与限额自学习（v0.6）、§9 `usage_log` 增 `account_id`、§13 变更记录 |

无新增依赖。无前端改动（T7 不含 UI；聚合与图表属 T14）。

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| **一条记录 = 一次出站尝试** | fallback 链试 N 个候选就是 N 行——每个候选都真实消耗了上游额度。未到达任何候选就失败（鉴权失败 / 别名未命中 / 全部候选被跳过）**不写** `usage_log`（没有渠道上下文） |
| **API Key 型渠道补建默认账号** | `account` 表原本只服务 OAuth / 登录态型；任务卡要求 per `(channel, account)` 计数且 `quota_json` 要有唯一落点，故 API Key 型渠道按需补建默认账号（id = `acct_default_<channel_id>`，确定性生成，label「默认账号」）。**待头头确认**（账本 🔴 行） |
| **计数从 `usage_log` 派生**，不另设计数器表 | 流水是唯一真值，重启不丢计数。代价：**在途请求不计**，并发突发时窗口计数偏低（已回填 02 §8.2） |
| **窗口口径** | RPM/TPM = 最近 60 秒滑动；RPD/TPD = 当日 **UTC 日界** |
| **只收紧不放宽** | 新学到的上限低于已记录值才覆盖；用户按渠道文档手填的上限写进同一组字段，学习只会压低。撞到上限说明预检放行了太多，放宽只会更糟 |
| **`last_rate_limit` 是事实不是推断** | `Retry-After` / `x-ratelimit-reset-*` / `x-ratelimit-remaining-*` 是上游**报的**，原样存档 |
| **失败尝试不估 token** | 失败的调用没有产出 → token 全 0、`estimated=0`；成功但上游没回 usage 才走估算（`estimated=1`） |
| **tee 放在 `GuardedStream` 之下** | `UsageTap` 包住原始上游流，因此能看到原始 `Err` 并记 `upstream_error`；客户端中途断连（`Drop`）记 `aborted` |
| **流式输出文本累积上限 64 KiB** | tee 不得缓冲全量（02 §2.2 零缓冲直通不变）；超限后估算退化为"前缀估算" |
| **落库走 `spawn_blocking`** | 不在 tokio 工作线程上做同步写；**落库失败只记日志**，不让用户的请求失败 |
| **迁移 v7 幂等** | 新库建表即含 `account_id`；老库走 `ALTER TABLE` 补列。SQLite 没有 `ADD COLUMN IF NOT EXISTS`，故先查 `PRAGMA table_info` 再决定（真机已在旧库上验证） |
| **估算器是占位** | 当前是字符数启发式（CJK 1 字 1 token，其余 4 字符 1 token）；**T12 换 tiktoken-rs**（02 §4 技术栈已锁定） |
| `cost_micros` | 恒为 NULL——价目表换算属 **T13** |

### 待上报头头

1. **账号口径**（02 §8.2）：API Key 型渠道自动补建默认账号 `acct_default_<channel_id>`。这是在任务卡"per `(channel, account)` 计数 + `quota_json` 回写"要求下的必然选择（原 `account` 表只服务 OAuth / 登录态型），已按此实现并回填 02 §8.2（v0.6），**请确认**。
2. **窗口计数的在途缺口**：流水只在尝试结束时写入，并发突发时 RPM/TPM 会偏低。若后续要做精确的速率准入，需另加在途计数（FreeLLMAPI 用 in-flight lease）。

## 5. 未做

- **真实 tokenizer 估算**（tiktoken-rs）—— T12
- **价目表与 `cost_micros` 换算** —— T13
- **用量聚合视图与图表 UI** —— T14
- **配额监控 probe**（GLM/Kimi/MiniMax 包月额度查询）—— T15 之后
- **签到获得积分回写 `account.quota_json`**（与限额自学习共用同一字段，但语义不同）—— T18
- **在途请求计数**（精确速率准入）—— 未排期，见 §4

# T5 验收留档 — 路由与别名

| 项 | 值 |
|---|---|
| 任务 | T5 路由与别名（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `eb3fddd` |
| 结论 | **通过**（判据逐条实测见 §1） |

## 1. 判据对照

任务卡原文：*目标：alias 唯一索引、route_group 四策略（priority/round_robin/cost/**sticky_session**）、未命中返回可用别名。验收：route_group 命中/单别名命中/未命中三条路径单测通过；sticky_session 对同一 client_key+模型连续请求返回同渠道（多账号池场景）。*

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | alias 唯一索引 | ✅ | `idx_model_alias`（T1 已建）+ `route_group.alias` 的 UNIQUE 约束；重复别名插入被拒（§2.4） |
| 2 | route_group **四**策略 | ✅ | `priority` / `round_robin` / `cost` / `sticky_session`（§2.3） |
| 3 | **未命中返回可用别名** | ✅ | 400 + `error.available_models` 列表（§2.5 真机、§2.2 单测） |
| 4 | **route_group 命中**路径单测 | ✅ | `acceptance_route_group_hit_selects_by_priority` |
| 5 | **单别名命中**路径单测 | ✅ | `acceptance_single_alias_hit` |
| 6 | **未命中**路径单测 | ✅ | `acceptance_alias_miss_returns_available_aliases` |
| 7 | **sticky_session 同 client_key 连续请求 → 同渠道** | ✅ | 单测 + 真机（§2.2、§2.5） |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 117 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 5 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs
test result: ok. 6 passed;  0 failed; 0 ignored                                 # tests/outbound_e2e.rs（T4）
test result: ok. 11 passed; 0 failed; 0 ignored                                 # tests/router_e2e.rs（T5 新增）
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs（T2）
```

编译告警：0（T2 遗留的 `use rusqlite::Row` 未使用告警已在本任务顺手清除）。

### 2.2 三条路径（验收判据 4/5/6）

| 路径 | 用例 | 断言要点 |
|---|---|---|
| route_group 命中 | `acceptance_route_group_hit_selects_by_priority` | 组 `smart`（priority）两成员 → 按 `channel.priority` 选 `c1`；候选链长度 2，首选在首位、其余按 priority 排序；`credential_id` = `cred1` |
| 单别名命中 | `acceptance_single_alias_hit` | 别名 `moonshot-chat` → 渠道 `c2` / 上游 `moonshot-v1-8k`；无 fallback 备选（候选链长度 1） |
| 未命中 | `acceptance_alias_miss_returns_available_aliases` | 未知别名 → `RouteError::AliasNotFound`；可用别名含 `model.alias` 与 `route_group.alias` 两种 |

### 2.3 四种策略

| 策略 | 用例 | 断言要点 |
|---|---|---|
| `priority` | `acceptance_route_group_hit_selects_by_priority` | `channel.priority` 越小越优先（`10 < 20` → 选 c1） |
| `round_robin` | `acceptance_round_robin_rotates_candidates` | 两次等权候选 → 依次 c1、c2、c1（平滑加权轮询） |
| `cost` | `acceptance_cost_strategy_prefers_cheapest_candidate` | deepseek 贵（10e-6×1M 口径）、kimi 便宜 → 即使 kimi 的 priority 更高，仍选 kimi |
| `sticky_session` | `acceptance_sticky_session_pins_same_channel_for_same_client_key`、`acceptance_sticky_session_reassigns_when_pinned_channel_disabled` | 见下 |

**sticky_session 语义验证（两条用例合起来证明"真粘住"而不是"永远取 priority 首个"）**：

1. 首次解析 → `c1`（priority 最小）；
2. 连续三次解析 → 仍是 `c1`（同一 client key 连续请求同渠道）；
3. **新增一个 priority 更优的候选 `c3`** → 原 client key 仍解析到 `c1`（粘性生效，不被新候选抢走）；
4. **新** client key → 解析到 `c3`（按 priority 重新分配，粘性表按 client 隔离）；
5. 粘住的渠道被禁用 → 重新分配到 `c2`（不继续打已禁用渠道）。

### 2.4 alias 唯一索引

```
acceptance_model_alias_is_unique   ok
    重复 model.alias（'ds-chat'）插入 → 被 idx_model_alias 拒绝
    重复 route_group.alias（'smart'）插入 → 被 UNIQUE 约束拒绝
```

### 2.5 真机 HTTP 验收（应用在跑，真实请求打 `127.0.0.1:8787`）

应用以**独立数据目录**启动（`LLMR_STORE_PATH` / `LLMR_VAULT_PATH` 指向临时目录，不污染用户真实库）：

```
[store] 数据库 C:\Users\Ben\AppData\Local\Temp\llmr-t5-verify\store.db（schema v4）
[vault] 保险库 C:\Users\Ben\AppData\Local\Temp\llmr-t5-verify\vault.bin（主密钥 v1）
[gateway] 监听 http://127.0.0.1:8787/v1（OpenAI 兼容入站）
```

种入数据：渠道 `DeepSeek 官方`（priority=10）/ `Kimi 官方`（priority=20）、模型 `ds-chat` / `moonshot-chat`、
路由组 `smart`(priority) / `cheap`(cost) / `sticky`(sticky_session)、价目表（kimi 明显便宜）。

```
[PASS] healthz 200: HTTP 200 {'status': 'ok', 'service': 'llm-router', 'version': '0.1.0'}
[PASS] /v1/models 列出单别名与路由组别名: HTTP 200 ids=['cheap', 'ds-chat', 'moonshot-chat', 'smart', 'sticky']
[PASS] 单别名命中 → 503 且带出渠道: HTTP 503 channel=DeepSeek 官方
        message=路由已解析：别名 "ds-chat" → 渠道 "DeepSeek 官方"（上游模型 "deepseek-chat"，策略 "priority"）；出站转发在 T6 落地
[PASS] priority 路由组命中 → 选中 DeepSeek: HTTP 503 channel=DeepSeek 官方
[PASS] cost 路由组命中 → 选中更便宜的 Kimi: HTTP 503 channel=Kimi 官方
[PASS] sticky_session 同 client key 连续请求 → 同渠道: first=DeepSeek 官方 second=DeepSeek 官方
[PASS] 未命中 → 400 且附可用别名列表: HTTP 400 code=model_not_found
        available=['cheap', 'ds-chat', 'moonshot-chat', 'smart', 'sticky']
[PASS] 无 key → 401 统一结构: HTTP 401 type=authentication_error
ALL_PASS
```

> "503 + 带出渠道"是**预期行为**：T5 止步于路由解析，出站转发属 T6。不做假响应。

### 2.6 凭证零明文复查（AGENT.md §5）

```
$ python -c "... 扫描 store.db 二进制 ..."
db 明文命中 client key: False
db 含 llmr_ 前缀: False
client_key 行: [('ck_t5', 't5-verify', '5431436f9345...', 'llmr_qrh')]     # 只存 SHA-256 + 8 位前缀
```

路由输出中的 `credential_id` **只有 id，不含明文**（解密属出站侧，T6 接入）。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/gateway/router.rs` | `AliasRouter`（别名解析 + 四策略 + 候选链）、`Strategy`、`RouteCandidate`、`RoutePlan`、`RouteError` |
| `src-tauri/src/gateway/errors.rs` | 统一错误体新增可选 `available_models`（02 §2.1 ③） |
| `src-tauri/src/gateway/server.rs` | `GatewayState` 增加 `router`；`chat_completions` 接入路由解析；`/v1/models` 同时列出路由组别名 |
| `src-tauri/src/gateway/mod.rs` | 声明 `router` 子模块 |
| `src-tauri/src/lib.rs` | 启动时构造 `AliasRouter` 并注入 `GatewayState` |
| `src-tauri/tests/router_e2e.rs` | 端到端验收用例（11 个） |
| `src-tauri/tests/gateway_e2e.rs` | 同步 T3 的旧断言（见 §4）+ 新增"未命中 400"用例 |
| `src-tauri/tests/vault_e2e.rs` | 移除未使用导入（消除编译告警） |

无新增依赖。

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| **route_group 优先于 model.alias** | 02 §2.1 ③ 的判定顺序即如此：先查组再查单别名。同名时组生效 |
| **策略内存状态只存内存** | round_robin 游标与 sticky 粘性表放在内存，**重启即清零**——与熔断状态（02 §2.3）同一口径，不持久化避免误伤 |
| sticky 粘性粒度 = model 行 | 粘住 `model_id`，隐含同渠道。**账号级**粘性（多账号池选到具体 account）当前 DDL 做不到：`credential` 表没有 `account_id`，与 `account` 无外键关联。要做需改 DDL（走提案 + 迁移），留待 T8/T9 账号池落地 |
| cost 排序键 = `input_per_1m + output_per_1m` | 单位 1e-6 元，语义为"输入输出各 1M token 的合计成本"。**没有价目 → `None`，按最贵处理**（避免"没填价"被当成"免费"优先选中） |
| 价目表 `model_pattern` 支持 `*` | 通配匹配（字节级安全）；多条命中时取**最具体**（字面字符多者）者。价目表为空时 cost 策略退化为 priority 顺序 |
| round_robin 用平滑加权轮询 | 权重取 `route_member.weight`（DDL 默认 100）。SWRR 对同权重候选等价于严格轮转，且不会因权重悬殊而饿死小权重候选 |
| 候选链 = 首选 + 其余按 priority 排序 | 02 §2.1 ④ 的"取候选链"；**消费**（熔断跳过、逐个重试）属 T6 |
| `credential_id` 只取 id | 按 `kind` 优先级取该渠道下首选凭证（`api_key` > `oauth_copilot_token` > `oauth_gh_token` > `session_workbuddy`），**不解密**，凭证零明文边界不被破坏 |
| 统一错误体新增 `available_models` | 02 §2.1 ③ 要求"响应体附可用别名列表"，但 §4.4 的错误体字段表未列该字段。实现为**可选字段**（仅未命中时出现，其余错误的响应体与旧版逐字节一致），是否回填 §4.4 见 §6 |
| `/v1/models` 现在也列路由组别名 | 路由组别名同样是客户端可见的对外面名；T3 时代只列 `model.alias` |
| **行为变化：未命中从 503 变 400** | T3 阶段无路由，任何请求都返回 503；T5 按 02 §2.1 ③ 改为 400 + 可用别名。T3 的两条旧断言（`chat_completions_with_valid_key_passes_auth` 等）已同步为"种入模型后仍 503"，并新增未命中用例 |
| 路由查询走 `spawn_blocking` | 数据库访问不阻塞 tokio 工作线程；策略选择是纯内存操作 |

## 5. 未做

- 出站转发、SSE 直通、熔断、fallback 消费候选链 —— T6
- 渠道 / 模型 / 路由组的增删改 API 与 UI（含 MiniMax 的 GroupId 输入框）—— T8/T9
- 账号级粘性与 `credential ↔ account` 关联（需改 DDL，走提案）
- 路由组权重的手工配置入口 —— T8/T9
- `/v1/models` 的分页与 owned_by 细分 —— 当前 `route_group` 统一标 `route_group`

## 6. 待裁决

| # | 项 | 说明 |
|---|---|---|
| 1 | 统一错误体新增 `available_models` 字段是否回填 02 §4.4 | 02 §2.1 ③ 要求附可用别名，但 §4.4 字段表未列。已按"可选字段、不影响其它错误响应体"实现；是否在 §4.4 补一行，待头头裁决 |

# T21 Copilot 出站适配 —— 验收留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2 **T21**（M4 第二张卡）
> 设计口径：`docs/planning/02-technical-design.md` **§6.2.2**（+ §6.2.1），变更记录 **v0.29 / v0.30**
> 日期：2026-11-27

## 1. 结论

**6 条判据逐条实测全绿**（本地 mock 单测 + e2e + **真机真实上游**）。

**真机四证**：

| # | 证据 | 结果 |
|---|---|---|
| ① | **R-6 四项固定头 + `x-request-id` 经真实上游实测** | `https://api.individual.githubcopilot.com/chat/completions` **正常回话**，**无头缺失 / 无鉴权错误**（出站报文原样打印，`Authorization` 打码） |
| ② | **经真网关流式对话成功** | `gpt-4o-mini-2024-07-18`，**15 个 SSE 帧**真实内容直通（`[DONE]` 直通）；**非流式**同模型亦成功 |
| ③ | **用量落库证据** | `usage_log`：`status=success` / `alias=gpt-4o-mini-2024-07-18` / `upstream=gpt-4o-mini-2024-07-18` / `tokens=13/19` / **`estimated=0`** / 耗时 2999~5071 ms / `error=None` |
| ④ | **四层零明文** | 官方 `scripts/security_check.py` **明文凭证命中 0 条**（结构性命中 12 条全是 DDL 注释与字段名）；另跑 `ghu_` / `gho_` / `eyJ` / `Bearer+长串` **形态扫描**：`store.db` / `-wal` / `-shm` / `vault.bin` **四层 0 命中** |

**真机揪出并修掉两个真问题**（都是"文档自述 ≠ 事实"）：

1. **端点字段的形状错了**（→ §4.5）；
2. **真机验收工具把渠道留在开启态**（→ §4.6）。

## 2. 改了什么（范围）

| 层 | 改动 | 位置 |
|---|---|---|
| 出站适配 | **新增 `providers::copilot`**：`CopilotAdapter` + R-6 常量 + `x-request-id`（UUID v4）+ 模型目录拉取与解析 + Copilot 专属配置 | `src-tauri/src/providers/copilot.rs`（新） |
| 适配器注册表 | `adapter_for` / `adapter_for_row` **认领 `copilot`**；新增 `AdapterDeps`（vault + Copilot 端点 + token 缓存） | `src-tauri/src/providers/mod.rs` |
| 渠道配置 | `config_from_row` 增 **copilot 分支**（`/chat/completions` + R-6 四项固定头 + `kind=oauth` + `inject_stream_usage=true`） | 同上 |
| 网关 | `Forwarder` 持有 `AdapterDeps`；**OAuth 型渠道网关不再解密**（凭证由 adapter 供给）；**adapter 构造失败改走下一个候选**（不再整条链失败） | `src-tauri/src/gateway/fallback.rs` |
| OAuth 层 | `TokenCache` 全局实例改 `Arc`（出站侧要**共享**缓存，测试侧要**独立**实例）；`ensure_token` 改收 `&Arc<RwLock<Vault>>`（**读锁不跨 await**——`RwLockReadGuard` 不是 `Send`） | `src-tauri/src/providers/oauth/copilot.rs` |
| 数据层 | 新增 `sync_models_from_catalog`（目录 → `model` 表）+ `CatalogModel` / `ModelSyncReport`；`OAUTH_PROVIDER` 改为引用 `providers::copilot::PROVIDER`（**唯一真值**） | `src-tauri/src/store/channels.rs` |
| IPC | 新增 **`copilot_models_refresh`**（拉上游 `/models` → 落库 → 回视图） | `src-tauri/src/commands/copilot.rs`、`src-tauri/src/lib.rs` |
| 前端 | Copilot 面板新增「**模型目录（T21）**」区块（刷新按钮 + 模型表）；新增 2 个纯函数 | `src/copilot/` |
| 真机工具 | 新增 `cargo run --example copilot_outbound`（**临时开渠道 → 真机对话 → Drop 守卫还原**） | `src-tauri/examples/copilot_outbound.rs`（新） |

**没做**（任务卡明列的边界）：ToS 弹窗与启用开关（**T24**）、`/responses` 与 `/messages` 端点、`machine-id` 头、`openai-intent` / `openai-organization` / `x-github-api-version`。

## 3. 判据逐条对账

| # | 判据（03 §2 T21） | 证据 | 结论 |
|---|---|---|---|
| ① | R-6 四头**逐头断言值** + `x-request-id` 每次都不同 | `providers::copilot::tests::r6_headers_carry_the_four_fixed_values` / `request_id_is_a_uuid_v4_and_changes_every_time`；e2e `acceptance_the_gateway_forwards_to_copilot_with_the_r6_headers`（**逐头断言**上游收到的值）+ `acceptance_the_request_id_changes_between_requests` | ✔ |
| ② | 出站 URL 与 Bearer（**是 Copilot token，不是 GitHub token**） | e2e 同上：路径 = `/chat/completions`（**不带 `/v1`**）；`authorization` = `Bearer MOCK-COPILOT-TOKEN-1`；**断言 `!auth.contains("MOCK-GH-TOKEN")`** | ✔ |
| ③ | **token 过期后自动续** | e2e `acceptance_a_fresh_copilot_token_is_reused_without_asking_github`（新鲜 → **不再打 GitHub**，上游调用数恒为 1）+ `acceptance_an_expiring_copilot_token_is_refreshed_automatically`（有效期 100 s < 提前量 300 s → **自动重取**，Bearer 换成第 2 枚 token） | ✔ |
| ④ | 模型目录解析与落库（含**别名冲突跳过**与**旧模型禁用**） | `providers::copilot::tests`（解析 4 条）+ e2e `acceptance_the_model_catalog_is_fetched_and_stored` / `acceptance_an_alias_owned_by_another_channel_is_skipped` / `acceptance_a_model_missing_from_the_catalog_gets_disabled_not_deleted` | ✔ |
| ⑤ | **经网关流式对话成功**（本地 mock 走**真网关**） | e2e `acceptance_the_gateway_forwards_to_copilot_with_the_r6_headers`（真网关 + 本地 mock 上游 + SSE 直通）；**真机**再打真实上游一次（§4.3） | ✔ |
| ⑥ | 02 §10.1 实测矩阵**第 8 列至少 3 项**回填 | 已回填 **3 项**（非流式 chat ✔ / 流式 chat ✔ / usage 返回 ✔）；`tool_calls` 与 `多轮上下文` **未测**（**不猜**） | ✔ |

## 4. 证据

### 4.1 纯函数与单元测试（`providers::copilot::tests`，**不出网**）

- `r6_headers_carry_the_four_fixed_values` —— R-6 四项固定头**逐项断言**，并断言**不含** `x-request-id`（动态头不进静态表）；
- `request_id_is_a_uuid_v4_and_changes_every_time` —— 形状 `8-4-4-4-12`、全十六进制、version 位为 `4`、两次调用**不同**；
- `attach_request_id_writes_the_header`；
- `the_config_carries_the_copilot_specifics` —— `kind=oauth` / `auth=bearer` / `chat_path=/chat/completions` / `inject_stream_usage=true` / R-6 四头；
- `models_are_parsed_from_the_openai_style_list` —— 含 `max_context_window_tokens` / `supports.*` / `family` / `vendor`；
- `disabled_models_are_not_collected` / `models_without_the_chat_endpoint_are_not_collected` / `a_body_without_data_is_an_error`；
- `the_real_upstream_shapes_are_filtered_correctly` —— **按真机真实形状**写的（路径形式端点 + `capabilities.type`）→ 断言只收 `chat-path` / `chat-and-responses` / `legacy-chat` / `no-type-no-endpoints`。

### 4.2 本地 mock 上游 e2e（`tests/copilot_outbound_e2e.rs`，**全程不出网**）

mock 上游**一个进程**同时提供三个端点（路径与真实一致）：④ `/copilot_internal/v2/token`、`/models`、`/chat/completions`（回 SSE）。

| 用例 | 断言要点 |
|---|---|
| `acceptance_the_gateway_forwards_to_copilot_with_the_r6_headers` | 上游被打到 1 次；路径 `/chat/completions`；`authorization` = Copilot token 且**不含** GitHub token；R-6 四头**逐头**；`x-request-id` 是 UUID；body `model` 已替换（R-1）；流式带 `stream_options.include_usage`（R-4）；客户端收到 SSE 内容与 `[DONE]` |
| `acceptance_the_request_id_changes_between_requests` | 两次请求的 `x-request-id` **不同** |
| `acceptance_a_fresh_copilot_token_is_reused_without_asking_github` | 缓存新鲜 → **不再打 GitHub**（调用数恒为 1），Bearer 不变 |
| `acceptance_an_expiring_copilot_token_is_refreshed_automatically` | 快过期 → **自动续**（调用数 1→2），Bearer 换成新 token |
| `acceptance_the_model_catalog_is_fetched_and_stored` | `/models` 请求也带 R-6 头 + Bearer；**只收 chat 端点 + 策略启用的**（codex / embeddings / disabled 三类都不收）；落库行 = `(alias=gpt-5-mini, upstream=gpt-5-mini, enabled=1)` |
| `acceptance_an_alias_owned_by_another_channel_is_skipped` | 别名被别的渠道占着 → **跳过**，**不抢**（原行的 `upstream_id` 不变），Copilot 渠道下**不建行** |
| `acceptance_a_model_missing_from_the_catalog_gets_disabled_not_deleted` | 目录里没有的旧模型 → **置为禁用**（行还在，`enabled=0`） |

### 4.3 真机证据（**真实上游**，`cargo run --example copilot_outbound`）

> 账号 `api.individual.githubcopilot.com`（④ 的 `endpoints.api`）；凭据**只从保险库取**，终端**不回显**。

**① 渠道与 token**

```
① Copilot 渠道
   id：ch_9c291d4d9b79a18b2614e2b78a8e1faf
   出站域名：https://api.individual.githubcopilot.com
   已启用：false          ← 默认关闭（AGENT.md §5.5）
② Copilot token 已就绪（**不回显**）
   到期时刻（Unix 秒）：1795877577
```

**② 真实模型目录**（原始 **39 条** → 收录 **19 条**）

原始条目的关键字段（真机逐条打印）：

```
- albatross｜type=chat｜endpoints=/responses｜policy=enabled
- copilot-search-a｜type=chat｜endpoints=/chat/completions｜policy=-
- gpt-5.4-nano｜type=chat｜endpoints=/responses/ws:/responses｜policy=-
- gpt-6-luna-decisions｜type=decisions｜endpoints=/v1/decisions｜policy=-
- microsoft-decision-1-1｜type=decisions｜endpoints=/v1/systemone｜policy=-
- kimi-k3-base｜type=chat｜endpoints=/chat/completions｜policy=enabled
- mai-code-2-flash｜type=chat｜endpoints=/chat/completions//responses｜policy=enabled
- gpt-4o-mini-2024-07-18｜type=chat｜endpoints=-｜policy=-
- text-embedding-3-small｜type=embeddings｜endpoints=-｜policy=-
- text-embedding-ada-002｜type=embeddings｜endpoints=-｜policy=-
```

排除口径实测生效：`policy.state=disabled` **8 条** + `capabilities.type != chat` **3 条** + 只支持 `/responses` 的 **9 条** → 39 − 20 = **19 条**。

**③ 真实出站请求（`Authorization` 打码）**

```
④ 出站请求（真实构造）
   URL：https://api.individual.githubcopilot.com/chat/completions
   copilot-integration-id: vscode-chat
   editor-version: vscode/1.104.1
   editor-plugin-version: copilot-chat/0.26.7
   user-agent: GitHubCopilotChat/0.26.7
   authorization: Bearer ***（已打码）
   content-type: application/json
   x-request-id: e63db6d5-6567-4077-bab9-a09555cc273f
```

**④ 经真网关流式 + 非流式**

```
⑤ 经网关流式对话：**成功**
   别名：gpt-4o-mini-2024-07-18
   SSE 帧数：15
   上游回话：我是一个人工智能助手，旨在通过提供信息和解答问题来帮助用户。
   usage_log：status=success alias=gpt-4o-mini-2024-07-18 upstream=gpt-4o-mini-2024-07-18 tokens=13/19 estimated=0 耗时=5071ms error=None
⑤' 经网关非流式对话：**成功**
   上游回话：我是一个基于人工智能技术的智能助手，旨在为用户提供信息和支持。
```

**⑤ 四层零明文**

```
明文凭证命中：0 条（必须为 0）
结构性命中（DDL 注释 / 字段名 / 枚举值，非明文）：12 条
结论：四条判据全过            ← 官方执行器，退出码 0
```

形态扫描（官方执行器**不认** GitHub / Copilot 的凭据形态，故另扫）：

```
store.db: 143360 字节 → 0 命中
store.db-wal: 0 字节 → 0 命中
store.db-shm: 32768 字节 → 0 命中
vault.bin: 596 字节 → 0 命中
四层形态扫描合计命中：0（必须为 0）
```

（形态：`ghu_…` / `gho_…` / `eyJ…`（JWT）/ `Bearer ` + 长串 / `device_code` UUID 形态。）

### 4.4 回归

| 项 | T20 收口时 | T21 后 | 结论 |
|---|---|---|---|
| Rust 单测（lib） | 395 | **406** | +11（T21 新增） |
| Rust e2e | 81 | **88** | +7（`copilot_outbound_e2e.rs` 7 条） |
| **Rust 合计** | **476** | **494** | **0 失败 / 0 编译告警**（4 条 ignored 沿用既有标记） |
| 前端单测 | 54 | **57** | +3（`describeModelSync` / `describeModel`） |
| `npm run build` | 65.38 kB | **65.84 kB** | 通过（35 modules） |

### 4.5 真机踩到的真问题①：**端点字段的形状错了**

**现象**：首版收录规则写的是「`supported_endpoints` 含 `chat` 才收录」——依据是社区 schema
（opencode `github-copilot/models.ts` 用短名 `chat` / `responses`）。

**真机打脸**：真实上游给的是**路径**：

```
copilot-search-a｜endpoints=/chat/completions
gpt-5.4-nano｜endpoints=/responses/ws:/responses
gpt-6-luna-decisions｜endpoints=/v1/decisions
```

→ 短名比较**一条都匹配不上** → **端点过滤等于没生效** → 实测把
`text-embedding-3-small` / `text-embedding-3-small-inference` / `text-embedding-ada-002`
（**embeddings** 模型）和一批只支持 `/responses` 的模型一起收了进来（首轮 17 条，含 3 个错的）。

**修法**（02 §6.2.2，§13 v0.30）：

1. **补一条规则**：`capabilities.type` 存在且**不是 `chat`** → 不收录（真实取值有 `chat` / `embeddings` / `decisions`）；
2. **两种形状都认**：路径含 `chat/completions` **或**短名等于 `chat` → 两种都收（各留一条单测）；
3. **`supported_endpoints` 缺失的老模型按 chat 收**（`gpt-4o` 家族没这个键，但 `capabilities.type=chat`，且**真机实测能对话**）。

**修正后的真机结果**：原始 39 条 → 收录 **19 条**；**上轮误收的 3 个 embeddings 模型在重新同步时被置为禁用**（`model` 表 22 行 / 启用 19 行）——**真机实证**了「旧模型置为禁用而非物理删除」这条落库口径。

### 4.6 真机踩到的真问题②：验收工具把渠道留在开启态

**现象**：一次验收的输出被管道 `Select-Object -First` 打断 → 进程**被提前杀掉** → 工具里的
`process::exit` 路径不跑析构 → 渠道停在 `enabled=1`（违反 AGENT.md §5.5「反代默认关闭」）。

**修法**：把「临时开渠道」包进 **`Drop` 守卫**（`EnabledGuard`）——**任何**退出路径（含 panic）都还原原状；并把 `process::exit` 移到 `run()` **返回之后**（`exit` 不跑析构，守卫必须先 Drop）。修后复跑实测：验收前 `enabled=0` → 验收后 `enabled=0` ✔。

### 4.7 密文边界

- 终端**不回显**任何 token（`authorization` 打码、`Copilot token 已就绪（不回显）`）；
- `device_code` 仍在后端内存（T20 口径未动）；
- Copilot token / GitHub token **只进保险库**，IPC 只回布尔与到期时刻；
- **网关不再解密 OAuth 凭证**（`CallContext.credential = None`）——凭证由 adapter 供给，**少一次解密**。

## 5. 边界与已知限制

| # | 项 | 说明 |
|---|---|---|
| 1 | **渠道仍默认关闭** | 本卡**不提供**启用入口（ToS 弹窗 + 开关属 **T24**）。真机验收用「临时开 → 测完还原」，终态 `enabled=0`。 |
| 2 | **只走 `/chat/completions`** | codex / luna / sol 这类只支持 `/responses`（或 `/responses/ws:`）的模型**不收录**——v1 **不做协议转换**（§5.6）。想用它们要等 v2 的 Responses 端点。 |
| 3 | **`tool_calls` / 多轮上下文未测** | 实测矩阵第 8 列只回填 3 项；其余**标「未测」，不猜**。 |
| 4 | **`machine-id` 不实现** | 设备指纹 → 撞 AGENT.md §5.7；liter-llm 与 npm 参考实现也都不发。 |
| 5 | **别名 = 模型 id** | 真实 id 带日期后缀（`gpt-4o-mini-2024-07-18`）；用户可在渠道面板改别名（改过之后目录同步会**跳过**该模型，不抢用户的命名）。 |
| 6 | **④ 端点仍是社区观察到的内部端点** | `/copilot_internal/v2/token` GitHub **未公开承诺稳定**（T20 风险声明继续有效）。 |
| 7 | **ToS 风险不变** | 把 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限——**首次启用的确认弹窗属 T24**。 |
| 8 | **模型目录里的老模型** | `gpt-4o` 家族没有 `supported_endpoints` 字段 → 按 chat 收；真机实测 `gpt-4o-mini-2024-07-18` 可正常对话。 |

## 6. 下一步

- **T22** WorkBuddy 协议确认（抓包）——可与 T23 并行准备；
- **T23** WorkBuddy adapter（登录态扫描 + 转发 + 失效标记）——**沿用本卡的凭证供给口径**（OAuth / 登录态型渠道的凭证由 adapter 供给，网关不解密）；
- **T24** ToS 确认弹窗 + 启用开关 —— 做完它，Copilot 渠道才能由用户在 UI 里正式打开（现在只能改库）。

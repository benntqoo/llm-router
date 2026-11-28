# LLM Router — 技术设计文档（v1）

| 项 | 值 |
|---|---|
| 版本 | v0.11（变更记录见 §13） |
| 日期 | 2026-11-26 |
| 上游文档 | `01-architecture-and-feasibility.md`（可行性基线与竞品调研） |
| 状态 | 待评审后进入开发。**本文档未改动任何代码或配置**。 |
| 决策依据 | 2026-11-25 头头确认：v1 仅反代 GitHub Copilot + WorkBuddy 两类 harness 内置额度；Anthropic 入站不进 v1；MiniMax 需处理 group_id |

---

## 0. v1 边界与非目标

### 0.1 v1 交付范围

| 能力 | 范围 |
|---|---|
| 入站协议 | **仅 OpenAI Chat Completions**（`POST /v1/chat/completions` + `GET /v1/models`），监听 `127.0.0.1` |
| 出站渠道（API Key 型） | DeepSeek、Kimi、GLM、MiniMax（国际/国内双 profile）、阿里百炼、硅基流动 |
| 出站渠道（OAuth 型） | **GitHub Copilot** |
| 出站渠道（登录态型） | **WorkBuddy** |
| 加密保险库 | AES-256-GCM + DPAPI/Keychain 主密钥，密钥可回看 |
| 自动签到 | HAR 重放框架 + 调度 + 通知，内置 ≥2 个站点插件 |
| 用量统计 | tokens / TTFT / 成本，按渠道/模型/客户端聚合 |

### 0.2 非目标（明确不做）

| 项 | 理由 |
|---|---|
| Anthropic Messages 入站（Claude Code） | 头头确认不进 v1。pi 已验证支持 OpenAI 兼容端点，无需此协议。见 §12 扩展项 |
| OpenAI Responses 入站（Codex） | 同上 |
| 修改任何 harness 的配置文件 | 产品边界：harness 由用户自行填写 base_url + key + 模型名 |
| 多用户 / 计费 / 充值 / 分发 | 单人本地工具 |
| 云端同步 | v1 纯本地 |
| Gemini / GPT / Claude 等海外付费渠道预设 | v1 聚焦国产模型 + Copilot/WorkBuddy 额度反代（头头确认）。技术上 OpenAI 兼容渠道均可自定义添加，只是不做预设 |
| Claude / Gemini / GPT 模型族协议适配 | Copilot 上游返回的多为 OpenAI 格式，直接透传；WorkBuddy 同理（待抓包确认） |
| 一键配置 harness 向导（FreeLLMAPI `setup-claude` 式） | 2026-11-25 决策：**维持"永不修改 harness 配置文件"红线**，不提供任何代写配置能力 |

### 0.3 客户端接入方式（不变承诺）

任何 OpenAI 兼容客户端（pi、cherry studio、自写脚本等）只需：

```
base_url = http://127.0.0.1:<port>/v1
api_key  = <在应用内创建的 client key>
model    = <应用内定义的别名，如 deepseek-chat 或 smart>
```

pi 的接入指引见 §6.5。

---

## 1. 总体架构

### 1.1 进程模型

单进程：Tauri 2 应用内嵌 axum HTTP 服务。前端 React ↔ Tauri IPC ↔ 命令层。

```
┌──────────────────────────────────────────────────────────┐
│  Tauri 2 App                                             │
│                                                          │
│  React UI ──IPC──► commands/                             │
│                      │                                   │
│     ┌────────────────┼──────────────────┐                │
│     ▼                ▼                  ▼                │
│  gateway/         vault/            checkin/             │
│  ├ server.rs      ├ crypto.rs       ├ scheduler.rs       │
│  ├ inbound/openai ├ oskey.rs        ├ har_replay.rs      │
│  ├ router.rs      └ vault.bin       └ sites/*            │
│  ├ fallback.rs                                            │
│  ├ usage_sink.rs   providers/         account/            │
│  └ sse.rs          ├ adapter.rs     ├ oauth/copilot.rs   │
│                    ├ openai_compat └ session/workbuddy.rs │
│                    └ presets/*                            │
│                      │                                   │
│                      ▼                                   │
│              store/ (SQLite WAL)                          │
└──────────────────────────────────────────────────────────┘
        │                                  ▲
        ▼                                  │
  各上游 LLM API                 harness（pi 等）填本地地址
```

### 1.2 目录结构

```
src-tauri/src/
  main.rs
  commands/            # Tauri IPC：channel / vault / usage / checkin / account / settings
  gateway/
    server.rs          # axum 路由 + 入站鉴权中间件
    inbound/openai.rs  # 请求解析、别名解析
    router.rs          # alias → (channel, upstream_model) + route_group 策略
    fallback.rs        # 熔断 + 重试链
    sse.rs             # 流式透传 + 旁路 tee
    usage_sink.rs      # usage 提取、估算、落库
  providers/
    adapter.rs         # trait ProviderAdapter + 注册表
    openai_compat.rs   # 通用 OpenAI 兼容出站（含 URL/头/查询串改写）
    presets/           # 各渠道预设（纯数据：base_url、路径模板、鉴权样式、特殊规则）
      deepseek.rs kimi.rs glm.rs minimax_intl.rs minimax_cn.rs
      dashscope.rs siliconflow.rs
    oauth/
      copilot.rs       # device flow + token 换取 + 刷新
    session/
      workbuddy.rs     # 登录态扫描 + 转发
  vault/
    crypto.rs oskey.rs mod.rs
  checkin/
    scheduler.rs har.rs sites/
  account/
    quota.rs           # 额度探测（渠道支持时）
  store/
    schema.rs migrations/
```

仓库根目录另有运维脚本（**不进应用二进制**，只用于验证与文档生成）：

```
sidecar/                 # 浏览器辅轨 sidecar（02 §7.4 §3）：Node + Playwright，
                         #   持久化 profile 复用登录态，跑 MiniMax Code 签到
scripts/
  channel_matrix.py     # 渠道实测矩阵执行器（02 §10.1）：经网关跑 6 项，产出 docs/reviews/channel-matrix-*.md
  pi_models.py          # 生成 pi 的 models.json / settings.json 片段（02 §6.5），别名从应用库直读
  security_check.py     # M1 安全验收执行器（02 §10.2）：四条判据，只读
src-tauri/examples/
  presets.rs            # 导出 A 类预设清单，供脚本侧引用（避免抄名单）
```

### 1.3 技术选型

| 项 | 选型 | 说明 |
|---|---|---|
| 桌面壳 | Tauri 2 | 头头已定 |
| HTTP 服务 | axum + tokio | 流式友好 |
| HTTP 客户端 | reqwest（stream on） | `bytes_stream()` 逐 chunk |
| 存储 | SQLite（rusqlite + WAL） | 纯 Rust，无 CGO 依赖 |
| 加密 | AES-256-GCM（RustCrypto `aes-gcm`）+ Argon2id | §5 |
| OS 密钥 | Windows DPAPI（`windows` crate）/ macOS Keychain（`security-framework`） | §5 |
| tokenizer | tiktoken-rs | 仅估算，标 `estimated=1` |
| HAR 解析 | 自写（HAR 1.2 JSON 子集） | §7 |

---

## 2. 反代流程（端到端）

### 2.1 请求生命周期

```
harness (pi 等)
   │  POST /v1/chat/completions
   │  Authorization: Bearer <client_key>
   ▼
① server.rs 鉴权
   SHA-256(client_key) 命中 client_key.key_hash？
   否 → 401（统一错误结构 §4.4）
   ▼
② inbound/openai.rs 解析
   解析 body → 提取 model 字段（其他字段原样保留）
   ▼
③ router.rs 解析别名
   命中 route_group.alias → 按 strategy 选出 model
      strategy: priority | round_robin | cost | sticky_session
      （sticky_session：同一 client_key+模型粘住同一渠道/账号，
        多账号池场景必需，参考 FreeLLMAPI 与 TraeWorkAssistant 双源实践）
   命中 model.alias       → 直接使用
   未命中                 → 400，响应体附可用别名列表
   输出：(channel, upstream_model, credential_id)
   ▼
④ fallback.rs 取候选链
   [首选 (channel, model)] + route_group 内按 priority 排序的备选
   熔断状态为 open 的候选跳过
   ▼
⑤ providers/adapter 改写出站请求（§4 转换规则）
   改写：URL / Authorization / body.model / 特殊 query / 特殊头
   ▼
⑥ reqwest 转发（bytes_stream，零缓冲直通）
   旁路 tee：将响应流复制到 usage_sink（解析 SSE data 帧的 usage 字段与首 token 时间）
   ▼
⑦ usage_sink 落库 usage_log（含 estimated 标记）
   ▼
⑧ 失败重试
   若尚未向客户端写出首字节且错误属于可重试类（§3.3）→ 取下一候选回 ⑤
   已写出首字节 → 中断并按 §4.4 错误结构收尾
   ▼
⑨ 返回 harness
```

### 2.2 流式透传细则

| 规则 | 说明 |
|---|---|
| 逐 chunk 直通 | `bytes_stream()` 收到即转发，不重组、不缓冲全量 |
| 首字节超时 | 600s（推理模型长思考）；连接建立超时 30s |
| 客户端断连 | `tokio::select!` 监听，立即 abort 上游请求（避免上游空跑计费） |
| `[DONE]` | 原样透传 |
| usage 帧旁路 | OpenAI 兼容流式最后一个数据帧含 `usage`；tee 解析不阻塞主链路 |
| 请求侧注入 | `stream_options: {"include_usage": true}`（默认开启；渠道不认识时从错误中学习并关闭，见 §4.3 P-4） |

### 2.3 熔断

粒度 `(channel_id, upstream_model)`。连续 3 次失败 → open 60s → half_open 放一次探测 → 成功 closed / 失败重开。429 触发连接冷却（指数退避，遵守 `Retry-After`）。

**状态持久化**（2026-11-26 头头裁决）：非 closed 的熔断状态**写穿**到 `circuit_state` 表——状态一变即落库，回到 closed 即删行；**重启后恢复**，不再是"重启即清零"。理由：

- 重启清零等于让限流保护被绕过：重启网关就能立刻重试被 429 的上游；
- 冷却窗口本身很短（失败熔断 60s、429 冷却上限 60s），持久化不会造成长期误伤。

只持久化 open / half_open；**未达阈值的连续失败计数只存内存**（重启清零）。落库失败只记日志，不影响请求（熔断状态丢失只是重新计数）。

**UI 展示**：应用内展示当前处于熔断/冷却的 `(channel, upstream_model)`：状态、原因（连续失败 / 上游限流）、剩余秒数（IPC 命令 `circuit_status`）。

**熔断时长可配置**（2026-11-26 头头裁决）：连续失败触发的 open 持续时长默认 **60s**，用户可在应用内调整，**最小单位 1 秒**。设置存 `app_setting` 表（键 `circuit_open_secs`），启动时读回，改动**立即生效**（作用于之后新打开的熔断，不追溯修改已在冷却中的熔断）。

**429 冷却上限可配置**（2026-11-26 头头裁决）：429 的冷却时长取 `min(max(指数退避, 上游 Retry-After), 用户上限)`，**用户上限默认 60s、最小单位 1 秒**，存 `app_setting` 表（键 `rate_limit_cooldown_cap_secs`），启动时读回，改动立即生效。

- 指数退避（2s / 4s / 8s / 16s / 32s / 64s…）的**封顶也用这个上限**——一个旋钮管住 429 冷却的全部时长。
- **优先级**：用户上限可以**缩短**上游 `Retry-After` 要求的等待（上游要等 300s、上限设 60s → 实际等 60s）。这是用户显式选择的结果，代价是可能更频繁地撞上游限流；把上限调大即可恢复"完全尊重 Retry-After"。回给客户端的 `Retry-After` 头与错误信息里的冷却秒数，一律是**实际生效的冷却时长**（不是上游原始值）。
- 该上限**不作用于**连续失败触发的熔断时长（那是上面的 `circuit_open_secs`）。

**手动解除熔断**：UI 每条熔断记录带"解除"按钮，调用 `circuit_reset`——清掉内存状态并删除落库行（重启也不会复活）。适用场景：上游已恢复、但用户不想等冷却窗口自然到期。

---

## 3. 鉴权设计

### 3.1 入站鉴权（client key）

- 生成：`llmr_` + 32 字节随机 base62。
- 存储：**只存 SHA-256 哈希**（`client_key.key_hash`）+ 前 8 位前缀（`key_prefix`，供 UI 辨认）。明文只在创建时展示一次，之后不可回看（client key 是本地口令，无"只显示一次"痛点，不需要回看）。
- 传输：`Authorization: Bearer <key>` 或 `x-api-key`。
- 绑定校验：`Host` 头必须为 `127.0.0.1` / `localhost`；`client_key.rpm_limit` 与 `daily_tokens` 超限时返回 429。

### 3.2 出站鉴权三型

| 型 | 渠道 | 凭证形态 | 刷新 |
|---|---|---|---|
| A API Key | DeepSeek/Kimi/GLM/MiniMax/百炼/硅基流动 | 静态 Bearer（vault 加密存） | 无 |
| B OAuth | GitHub Copilot | GitHub token（长期）→ Copilot token（~30min） | 自动，见 §6.2 |
| C 登录态 | WorkBuddy | 本地登录态文件 + device-id | 跟随桌面应用会话，过期需重登（提示用户） |

### 3.3 错误分类与重试

| 类 | 判据 | 动作 |
|---|---|---|
| 可重试 | 429 / 5xx / 连接超时 / 网络错误 | 取下一候选（首字节前） |
| 不可重试 | 401/403（凭证失效）/ 400（参数） | 停止，原样回传；401 额外触发凭证失效标记（UI 提示） |
| 中途失败 | 已写首字节后断流 | 向客户端发 SSE `error` 事件（若客户端还在监听），标记 usage_log.status=aborted |

---

## 4. 请求/响应转换规则

**总原则：v1 是改写器，不是转换器。** 入站出站同为 OpenAI Chat 格式，body 做 JSON 级透传，只动以下字段。

### 4.1 请求侧改写清单（按渠道）

| # | 规则 | 适用 |
|---|---|---|
| R-1 | `body.model = upstream_model`（别名替换） | 全部 |
| R-2 | `Authorization: Bearer <凭证>` 头替换 | 全部 |
| R-3 | 出站 URL = `channel.base_url` + 渠道路径模板（见 §6.1 预设表；MiniMax/GLM 路径与 OpenAI 不同） | 全部 |
| R-4 | 注入 `stream_options: {"include_usage": true}`（流式时） | 声明支持者：DeepSeek/Kimi/百炼/硅基流动/Copilot；MiniMax 不注入（其流式自带 usage，注入多余字段有 400 风险，标"待实测"） |
| R-5 | query 追加 `GroupId=<group_id>`（仅当渠道 `meta_json.group_id` 非空） | MiniMax 国内 profile，见 §4.2 |
| R-6 | 追加 Copilot 伪装头：`editor-version` / `editor-plugin-version` / `copilot-integration-id` / `user-agent`（**具体值见 §6.2.2**，另有每次请求动态的 `x-request-id`） | Copilot |
| R-7 | 剥离 hop-by-hop 头（`connection` / `keep-alive` / `transfer-encoding` / `content-length` 重算） | 全部 |
| R-8 | 不认识的 body 字段一律透传（含 `tools` / `tool_choice` / `temperature` / `reasoning_effort` 等） | 全部 |

> 明确不做（v1）：`tools` 格式转换（各家均为 OpenAI tool schema）、消息结构转换、`reasoning_content` ↔ `reasoning_effort` 语义调整。国产渠道 `tools` 兼容性差异列入实测矩阵（§10），实测不过的渠道在预设中标 `tool_calls: partial` 并在 UI 提示。

### 4.2 MiniMax 口径（头头点名，专节）

**已验证事实（来源分档）：**

| 站点 | 端点 | GroupId | 鉴权 | 来源 |
|---|---|---|---|---|
| 国际站 | `POST https://api.minimax.io/v1/text/chatcompletion_v2` | **URL 不带 GroupId** | 仅 `Authorization: Bearer` | **已验证**：官方文档 platform.minimax.io/docs/api-reference/text-post，示例 URL 与 curl 均无 GroupId |
| 国内站 | `POST https://api.minimaxi.com(/chat)/v1/text/chatcompletion_v2` | **口径分裂**：多数第三方称 v2 无需（Bearer 已绑组织）；部分示例带 `?GroupId=` | Bearer | 部分验证：CSDN 两篇（162341154、mcp.csdn.net）vs aiqz.cc 示例。**以控制台实测为准** |
| 国内站旧版 | `POST https://api.minimaxi.com/v1/text/chatcompletion?GroupId=xxx` | 必须拼在 URL query | Bearer | 已验证（多源一致），但属 deprecated 接口，本项目不使用 |

**MiniMax 响应特有结构（官方已验证）：**

```json
{
  "id": "...", "choices": [{"finish_reason":"stop","index":0,
    "message": {"content":"...", "role":"assistant", "reasoning_content":"..."}}],
  "usage": {"prompt_tokens":26, "completion_tokens":223,
    "completion_tokens_details": {"reasoning_tokens":214}},
  "base_resp": {"status_code": 0, "status_msg": ""}
}
```

要点：
1. **`reasoning_content` 字段透传**，不解析不丢弃（客户端若支持可显示思考链）。
2. **`usage.completion_tokens_details.reasoning_tokens`** 计入用量统计的 `reasoning_tokens` 列。
3. **`base_resp.status_code` 必须解析**：MiniMax 可能返回 **HTTP 200 但业务错误**（与 OpenAI"错误必配非 200 状态码"不同）。

**MiniMax 错误码映射（写入 `providers/presets/minimax_*.rs`）：**

| `base_resp.status_code` | 含义 | 映射动作 |
|---|---|---|
| 1000 | 未知错误 | 500 upstream_error |
| 1001 | 引擎异常 | 502，可重试 |
| 1004 | **鉴权失败（常见诱因：国内站缺 GroupId，而非 key 错）** | 401 + UI 提示"检查 GroupId 配置，而非重新生成 key" |
| 1008 | 余额不足 | 402 语义，UI 提示充值，不重试 |
| 1027 / 1039 | 限流 | 429，可重试 |
| 其他 | — | 502 |

**配置拼接口径（实现规范）：**

```sql
-- channel.meta_json（MiniMax 国内 profile 示例）
{
  "group_id": "17895xxxxxxx",   -- null 则不拼接（国际站 profile 固定 null）
  "supports_groupid_query": true
}
```

- 出站 URL 构造：`base_url + "/v1/text/chatcompletion_v2"`；`group_id` 非空时追加 `?GroupId=<value>`（URL encode）。
- **严禁**把 GroupId 放进 body 或 header（两处官方均不认）。
- UI 表单：MiniMax 渠道增加"Group ID（国内站必填/国际站留空）"输入框 + 说明文案。

### 4.3 响应侧规则

| # | 规则 | 适用 |
|---|---|---|
| P-1 | 状态码透传；body 透传 | 全部 |
| P-2 | SSE 帧 `data:` 逐条直通；`[DONE]` 直通 | 流式 |
| P-3 | `base_resp.status_code != 0` 且 HTTP=200（MiniMax）→ 按 §4.2 表映射改写状态码与错误体 | MiniMax |
| P-4 | 学习式兼容：若某渠道因 R-4 注入报 400 且 body 含 `stream_options` 关键字，自动降级关闭注入并记入渠道 meta（本次会话生效） | 流式 |
| P-5 | 上游无 `usage` 字段（个别渠道流式）→ `usage_log.estimated=1`，本地估算 | 全部 |
| P-6 | 响应头除 hop-by-hop 外透传（`content-type` 必须为 `text/event-stream` 时不得缓冲） | 全部 |
| P-7 | 注入 `X-Routed-Via: <channel.name>/<upstream_model>` 响应头（路由可追溯，调试用） | 全部 |

### 4.4 统一错误结构（入站返回给 harness）

```json
{
  "error": {
    "message": "<人类可读>",
    "type": "invalid_request_error | authentication_error | rate_limit_error | api_error | upstream_error",
    "code": "<上游原始码，如 minimax_1004>",
    "channel": "<channel.name>",
    "available_models": ["deepseek-chat", "kimi-chat"],
    "retried": ["deepseek 官方", "kimi 官方"]   // fallback 走过的候选
  }
}
```

**键的可选性**：`code` / `channel` / `available_models` 三个键在对应信息缺失时**整个键不出现**（不出 `null`）。

`available_models` 是 §2.1 ③「未命中 → 400，响应体附可用别名列表」的实现载体，**只在别名未命中**时填充，
此时 `code = "model_not_found"`。别名**命中**但没有可用候选（渠道全禁用 / 熔断中 / 无凭证）走的是另一条路径，
**不带**该字段——那条路径的问题不是"名字不认识"，附别名列表会误导客户端。

**429 与 `Retry-After`**（2026-11-26 头头裁决；`Retry-After` 只走**响应头**，错误体字段表不变）：

| 场景 | HTTP 状态 | `type` | 额外响应头 |
|---|---|---|---|
| 上游直接返回 429 | 429 | `rate_limit_error` | `Retry-After: <冷却秒数>` |
| 冷却期内再请求（候选全被冷却跳过） | 429 | `rate_limit_error` | `Retry-After: <剩余冷却秒数>` |
| 连续失败导致熔断（候选被跳过） | 502 | `upstream_error` | — |

冷却时长 = `min(max(指数退避（2s 起步、逐次翻倍、封顶取用户上限）, 上游 `Retry-After`), 用户上限)`（§2.3）。限流原因必须让客户端看得见——冷却期回 502 会把"限流"这个真实原因藏成"没有可用候选"。回给客户端的 `Retry-After` 是**实际生效的冷却秒数**。

---

## 5. 加密保险库

### 5.1 密钥层次

```
OS 原生保护（DPAPI CurrentUser / macOS Keychain）
   └── wraps → MasterKey (32B 随机)
                 └── AES-256-GCM 加密每条 credential.ciphertext（独立 12B nonce）
可选模式：用户主密码 --Argon2id(m=64MiB,t=3,p=1)--> KEK ──wrap── MasterKey
```

### 5.2 规则

| 项 | 规范 |
|---|---|
| 落盘 | `%APPDATA%\llm-router\vault.bin`：`{"kdf":..., "wrapped_key":..., "key_version":1}` |
| 解密 | 仅内存；禁止写临时文件；解密后凭证引用以 `Arc<SecretBytes>` 短生命周期持有 |
| 回看 | UI"显示"按钮 → 解密 → 30s 自动遮蔽；一键复制；导出 `.env`/JSON 需二次确认 + 警示文案 |
| 轮换 | 换主密钥时按 `key_version` 逐条重加密 |
| IPC 边界 | 前端永不接收明文凭证字段，除非显式调用"回看"命令；日志中间件全局脱敏 `Authorization` / `api_key` / `token` 键值 |
| 崩溃安全 | `panic = "abort"` + 禁用 core dump 中的敏感页（Windows: `SetProcessWorkingSetSize` 不适用，改为关键结构 zeroize） |

**对照现状（差异化依据）**：CCR 3.1.3 的 `config.sqlite` 中 provider key 为 plain JSON、`api_keys.encryption` 仅接受 `plain`（来源：formal.ai 实测 SQL + greenlitbooks 审计，见 01 文档 §2.2）。本项目验收时用 `sqlite3 dump | grep` 证明无明文（§10）。

---

## 6. 渠道适配器规格

### 6.1 v1 预设表（API Key 型，7 profile）

| 渠道 | base_url | 出站路径 | 鉴权 | 特殊规则 |
|---|---|---|---|---|
| DeepSeek | `https://api.deepseek.com` | `/v1/chat/completions` | Bearer | 标准 |
| Kimi | `https://api.moonshot.cn` | `/v1/chat/completions` | Bearer | 标准 |
| GLM | `https://open.bigmodel.cn` | `/api/paas/v4/chat/completions` | Bearer | 路径非标准 |
| MiniMax 国际 | `https://api.minimax.io` | `/v1/text/chatcompletion_v2` | Bearer | §4.2；`group_id=null` |
| MiniMax 国内 | `https://api.minimaxi.com` | `/v1/text/chatcompletion_v2` | Bearer (+GroupId query 可配) | §4.2；1004 映射 |
| 阿里百炼 | `https://dashscope.aliyuncs.com` | `/compatible-mode/v1/chat/completions` | Bearer | 兼容模式 |
| 硅基流动 | `https://api.siliconflow.cn` | `/v1/chat/completions` | Bearer | 标准 |

> DeepSeek/Kimi/GLM/百炼/硅基流动的 base_url 为通用常识，实现时以各自控制台文档复核；MiniMax 已按官方文档核实（见 §4.2）。

预设是**纯数据**（`presets/*.rs` 返回 `PresetDef`）：新增一个 OpenAI 兼容渠道 = 填一张表单（base_url / 路径模板 / 鉴权样式 / group_id 等 meta），**零代码**。

#### 6.1.1 预设的默认模型（未实测建议值）

为了让「选中预设后**仅剩填 key 一步**」成立，每个预设额外带一条**默认模型**（上游模型名 = 默认别名）：

| 预设 | 默认上游模型名（= 默认别名） |
|---|---|
| DeepSeek | `deepseek-chat` |
| Kimi | `moonshot-v1-8k` |
| GLM | `glm-4-flash` |
| MiniMax 国际 | `abab6.5s-chat` |
| MiniMax 国内 | `abab6.5s-chat` |
| 阿里百炼 | `qwen-plus` |
| 硅基流动 | `deepseek-ai/DeepSeek-V3` |

这些是**公开常识的默认建议值，不是实测结论**——02 §10.1 的实测矩阵（T10）跑完之前一律按「未验证」对待，用户可随时改。UI 上标注「默认建议（未实测，可改）」。

#### 6.1.2 自定义 OpenAI 兼容渠道的表单字段

| 表单项 | 落到哪 | 约束 |
|---|---|---|
| 渠道名 | `channel.name` | 非空、唯一 |
| base_url | `channel.base_url` | 非空、必须是 `http`/`https` |
| 路径模板 | `meta_json.path_chat` | 必须以 `/` 开头；默认 `/v1/chat/completions` |
| 鉴权样式 | `meta_json.auth_style` | `bearer`（默认）/ `custom_header`（须配 `auth_header_name`） |
| GroupId | `meta_json.group_id` + `supports_groupid_query` | MiniMax 国内站用（§4.2）；国际站留空 |
| 注入 `stream_options.include_usage` | `meta_json.inject_stream_usage` | 默认开；上游报 400 时可关（P-4 会自动降级） |
| 额外固定头 | `meta_json.extra_headers` | 如 `{"x-foo": "bar"}` |
| 模型别名 / 上游模型名 | `model.alias` / `model.upstream_id` | **至少一条**；别名全局唯一 |

保存成功后，别名**立即**出现在 `GET /v1/models`（别名 = `model.alias` ∪ `route_group.alias`）。

### 6.2 GitHub Copilot（OAuth 型）

**流程（四步，均为社区已验证路径，参考实现：bjornmelin/github-copilot-proxy、npm 包 coxy）：**

1. **Device Flow**：`POST https://github.com/login/device/code`（Accept: application/json，client_id 复用 VS Code 公开 client_id）→ `device_code` + `user_code` + `interval`。
2. 用户浏览器打开 `https://github.com/login/device` 输入 `user_code`。
3. 轮询 `POST https://github.com/login/oauth/access_token`（`grant_type=urn:ietf:params:oauth:grant-type:device_code`）→ GitHub token（`ghu_`，长期有效，vault 存储）。
4. **Token 换取**：`GET https://api.github.com/copilot_internal/v2/token`（Bearer ghu_）→ Copilot token（JWT，**~30 分钟有效**）→ 内存缓存，过期前 5min 自动重取（R-6 伪装头也用于此请求）。

**出站**：`POST {endpoints.api}/chat/completions`（**不带 `/v1`**），Bearer = **Copilot token**，附 R-6 伪装头（**具体取值与来源见 §6.2.2**，T21 已固化到常量）。

**合规声明（必须写进 UI 首次启用弹窗）**：将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。

#### 6.2.1 协议细节与口径（T20 落地，2026-11-27 核实）

**外部事实**（多个社区实现一致：opencode-github-copilot / corvus / xcodeai / zeroclaw-providers / synaps-core）：

| 项 | 值 |
|---|---|
| OAuth client_id | `Iv1.b507a08c87ecfe98`（VS Code Copilot 的**公开** client_id，**不是密钥**，用户无须填） |
| scope | `read:user`（最小权限） |
| ① device code | `POST https://github.com/login/device/code`（`Accept: application/json`、`Content-Type: application/x-www-form-urlencoded`、`User-Agent: GitHubCopilotChat/…`）→ `device_code` / `user_code` / `verification_uri` / `interval`（**默认 5 s**）/ `expires_in`（**默认 900 s**） |
| ③ 轮询 | `POST https://github.com/login/oauth/access_token`（`grant_type=urn:ietf:params:oauth:grant-type:device_code`）→ `access_token`（**`ghu_` / `gho_` 前缀，长期有效**） |
| ④ token 换取 | `GET https://api.github.com/copilot_internal/v2/token`（`Authorization: token <gh_token>`）→ `{ token, expires_at, endpoints }`（`expires_at` 是 **float** Unix 秒；`endpoints.api` 可能给出**该账号实际要用的 API 域名**） |
| 出站 base | 默认 `https://api.githubcopilot.com`（**若换取响应给了 `endpoints.api` 则以它为准**） |

**轮询状态机**（GitHub 的 device flow 错误码）：

| 返回 | 含义 | 处理 |
|---|---|---|
| `authorization_pending` | 用户还没在浏览器点确认 | 等 `interval` 秒再轮询 |
| `slow_down` | 轮询太快 | 间隔 **+5 s** |
| `expired_token` | `device_code` 过期（900 s） | 让用户重新发起 |
| `access_denied` | 用户拒绝授权 | 结束并提示 |
| 有 `access_token` | 授权成功 | 换 Copilot token |

**网络层容错（本机实测，2026-11-27）**：

本机到 `github.com` 的链路**时好时坏**——这是 OAuth 三个端点能不能走通的决定性因素（实测数据）：

| 实测项 | 结果 |
|---|---|
| 同一 IP 的成功率随时间翻转 | `20.27.177.113` 21:15 时 5/5 通、21:47 时 0/5；`20.247.166.64` 早前是黑洞、22:05 时 POST 2/2 通 |
| **同时刻**对同一 IP 各打一次 GET / POST | `20.200.245.247` GET 302 / POST **200**；`140.82.112.3` GET 302 / POST **200** → **「GET 通、POST 不通」是时间差，不是边缘属性**（早前那组 0/3 是在不同时间测的，结论作废） |
| 随机回非 2xx | 同一 IP 会随机回 **400 / 403**（**不是** OAuth 业务错误） |
| `api.github.com` 的 IP 配 SNI=`github.com` | 5/5 秒回，但 **HTTP 400**（后端不认这个组合）→ **不可用** |
| 系统 DNS 连续 16 次解析 `github.com` | **全是同一个 A 记录**（ISP 侧有缓存，不轮询）→ 动态发现候选 IP **不可行** |
| 同一 IP 的成功率随时间翻转 | `20.27.177.113` 21:15 时 5/5 通、21:47 时 0/5；`20.205.243.166` 21:23 与 21:5x 均 5/5 |

**口径**：

- **候选 IP 池 + 轮换**：内置一组 `github.com` 公网边缘 IP，**每次重试换下一个候选**——钉死单个 IP 会把设备码的 900 s 寿命全耗在一条坏路径上（实测：钉 `20.27.177.113` 后连续 40 次轮询全超时，设备码到期作废）；
- **「非 2xx」也换 IP 重试**：GitHub 的 OAuth **业务错误实测全是 HTTP 200 + JSON `error`**（`authorization_pending` / `slow_down` / `expired_token` / `access_denied` 都实测过是 200）→ ①②③ 上**任何非 2xx 都意味着「这个节点没正确服务该端点」**，换候选重试；④ 的 **401 / 403 是业务错误**（token 无效 / 没有订阅）→ **不重试**；
- **DNS 兜底**：池子末尾**始终保留一个走普通 DNS 的 client** → 池内 IP 全部失效时仍有一条路；
- **超时收紧**：connect **3 s** / 单请求总超时 **8 s**（GitHub OAuth 正常 < 2 s）→ 失败一次代价小，能在设备码寿命内多试很多次；
- **只重试网络层错误**（超时 / 连不上），**HTTP 状态码不重试**（交给 §6.2.1 的轮询状态机判定）；
- **可覆盖**：环境变量 `LLMR_GITHUB_IPS`（逗号分隔）替换内置池——边缘 IP 会变，**不用改代码**。

**风险**：内置 IP 是 GitHub 的公网边缘地址，**会随时间变更**；靠「轮换 + DNS 兜底 + 环境变量覆盖」三层兜住。

**存储与刷新**：

- **GitHub token（长期）** → **保险库**，`credential.kind = 'oauth_gh_token'`（**零明文**、不回显、不进日志）；
- **Copilot token（约 25~30 min）** → **保险库** `credential.kind = 'oauth_copilot_token'` + **`expires_at` 精确到秒**（§9 的列已就位），**另加进程内缓存**（避免每次出站都解密）；
- **刷新**：过期前 **5 min** 自动重取（`ProviderAdapter::refresh()` 由 **T21** 的 adapter 调用；本层提供 `ensure_token()`——缓存命中就用，否则用保险库里的 GitHub token 重取）。

**边界与风险（必须写进 UI 与留档）**：

- `/copilot_internal/v2/token` 是**社区观察到的内部端点**，GitHub **未公开承诺稳定** → 可能随时变更或撤销（参考实现普遍在注释里声明这一点）；
- **不做**指纹伪造、验证码绕过、签名伪造（AGENT.md §5.7）——设备流是 **GitHub 官方 OAuth**，走官方路径；
- **本卡只取凭证**：出站 `/chat/completions` 转发与 **R-6 伪装头**（`Editor-Version` / `Editor-Plugin-Version` / `Copilot-Integration-Id` / `User-Agent`）属 **T21**；
- **合规**：Copilot 渠道**默认关闭**（`enabled = 0`），**首次启用的 ToS 确认弹窗属 T24**（AGENT.md §5.5）——本卡不提供启用开关。

#### 6.2.2 出站适配口径（T21 落地，2026-11-27 核实）

**R-6 伪装头（四项固定 + 一项动态）**

| 头 | 值 | 来源 |
|---|---|---|
| `copilot-integration-id` | `vscode-chat` | liter-llm（docs.rs `provider/github_copilot.rs` 静态头）、npm `@opeoginni/github-copilot-openai-compatible` README |
| `editor-version` | `vscode/1.104.1` | npm README（明确给 `Editor-Version: vscode/1.104.1`）。**取值分歧**：liter-llm 用 `vscode/1.95.0`、`ericc-ch/copilot-api` 动态探测 VS Code 版本（兜底 `1.104.3`）、`Alorse/copilot-to-api` 按客户端写成 `Neovim/0.6.1` → **头是按客户端变的**。本卡**取 npm 那个**（其协议契约与本项目一致：OpenAI 兼容 + `/chat/completions`），**且是常量**——版本串变了改常量即可，**不用动协议** |
| `editor-plugin-version` | `copilot-chat/0.26.7` | liter-llm（`COPILOT_VERSION="0.26.7"`）、npm README |
| `user-agent` | `GitHubCopilotChat/0.26.7` | 同上（与 T20 授权请求用的 UA 同串） |
| `x-request-id` | **每次请求动态**的 UUID v4 | liter-llm（每次请求生成）、bjornmelin（`X-Request-Id`） |

**真机验证（2026-11-27）**：以上四项 + `x-request-id` 经**真实上游**
`https://api.individual.githubcopilot.com/chat/completions` **实测通过**——上游正常回话
（流式 6~12 帧，内容真实），**没有**报任何头缺失或鉴权错误；出站报文由
`cargo run --example copilot_outbound` 原样打印（`Authorization` 打码）。

**明确不发的头（边界，不是遗漏）**：

- **`machine-id`**：bjornmelin 有发，但那是**设备指纹** → 撞 AGENT.md §5.7「不内置任何风控规避能力」；liter-llm 与 npm 参考实现都**不发** → **不实现**；
- **`openai-intent` / `openai-organization` / `x-github-api-version` / `x-vscode-user-agent-library-version`**：liter-llm 有发，npm 参考实现**没发**；R-6 是「改写器不是转换器」（§5.6）→ **只认 §4.1 明列的四项**，不擅自加。

**出站 URL 与鉴权**

| 项 | 值 | 依据 |
|---|---|---|
| URL | `{endpoints.api}` + `/chat/completions`（**不带 `/v1`**） | liter-llm（路径 `/chat/completions` / `/embeddings` / `/models`）、npm（codex 走 `/responses`，其余走 `/chat/completions`） |
| base | **④ 的 `endpoints.api` 为准**（本账号实测 `https://api.individual.githubcopilot.com`），缺省 `https://api.githubcopilot.com` | §6.2.1 |
| 鉴权 | `Authorization: Bearer <Copilot token>`（**Copilot token**，~30 min 短期凭据） | §6.2.1 |
| R-4 | **注入** `stream_options.include_usage`（Copilot 声明支持） | §4.1 |

**凭证供给口径（关键，与 T20 的分工）**

**OAuth 型渠道的凭证由 adapter 自己供给，网关不解密。** 理由：GitHub token（`ghu_`，长期）与 Copilot token（~30 min）**不能互换**——前者只能用来换后者。若沿用网关「按 `credential.kind` 选一条」的旧口径，Copilot token 一过期就会**误取 GitHub token** 当 Bearer → 必然 401。

- `CopilotAdapter::complete()` 出站前调 `ensure_token()`（T20 提供）：**缓存命中且离过期 ≥ 300 s** → 直接用；否则用保险库里的 GitHub token 重取（回写保险库 + 缓存）。
- 网关侧：`fallback.rs` 对 `kind = oauth` 的候选**不解密**（`CallContext.credential = None`），凭证一律由 adapter 供给；WorkBuddy（T23）沿用同一口径。
- `refresh()`（02 §6.4「OAuth 型必须实现」）：Copilot 的凭证刷新**发生在 `complete()` 内按需进行**（`ensure_token` 自带续期），`refresh()` 不单独取渠道上下文 → 保持 no-op 并在此说明。
- **注入方式**：`AdapterDeps`（vault + Copilot 端点 + token 缓存）→ 网关的 `Forwarder` 持有它 → **生产用真实端点 + 全局缓存，测试用本地 mock 端点 + 独立缓存**（并行用例互不污染，**不出网**）。

**模型目录口径**

`GET {endpoints.api}/models`（Bearer + R-6 头）→ OpenAI 风格的 `{"data": [...], "object": "list"}`。

**真机实测（2026-11-27，账号 `api.individual.githubcopilot.com`，原始条目 39 条 → 收录 19 条）**：

| 字段 | 实测取值 | 用途 |
|---|---|---|
| `id` | 如 `gpt-4o-mini-2024-07-18` / `kimi-k3-copilot` / `copilot-search-a` | **上游真实模型名**（R-1 的替换目标）**兼**对外别名 |
| `name` | 如 `GPT-4o mini` | 展示名（UI 用） |
| `policy.state` | `enabled` / `disabled` / **缺省**（该键整个没有） | `disabled` → **不收录**（策略禁用） |
| `capabilities.type` | `chat` / `embeddings` / `decisions` | **不是 `chat` → 不收录**（见下） |
| `supported_endpoints` | **路径形式**：`/chat/completions`、`/responses`、`/responses/ws:`、`/v1/decisions`、`/v1/systemone`；**老模型没有这个键** | **含 chat 端点才收录**（端点字段缺失 → 按 chat 收） |
| `capabilities.supports.*` / `capabilities.limits.*` / `capabilities.family` / `vendor` | `tool_calls` / `streaming` / `vision` / `max_context_window_tokens`（实测 128000 / 260000 / 1048576…）/ `family` / `vendor`（`Azure OpenAI`） | 展示用 |

**收录规则（三条，缺一不可）**：

1. `policy.state == "disabled"` → **不收录**；
2. `capabilities.type` **存在且不是 `chat`** → **不收录**（`embeddings` / `decisions` 走别的端点，v1 **不做协议转换**，收录了也打不通）；
3. `supported_endpoints` **存在但不含 chat 端点** → **不收录**（`/responses`、`/responses/ws:` 这类只有 Responses 端点的模型）。

**端点字段有两种形状，都要认**（**这是真机实测踩到的坑**）：真实上游给的是**路径**（`/chat/completions`），而社区 schema（opencode `github-copilot/models.ts`）里是**短名**（`chat`）→ 实现按「**路径含 `chat/completions` 或短名等于 `chat`**」判定，两种都收。

**落库口径**（`model` 表）：

- **别名 = `id`**（例 `gpt-4o-mini-2024-07-18`）；**别名已被别的渠道占用 → 跳过**（不抢、不覆盖），并在结果里报出来；
- **用户改过别名**（同一 `upstream_id` 挂在别的别名下）→ **跳过**（尊重用户的命名）；
- 目录里**没有**的旧模型 → **置为禁用**（`enabled = 0`），**不物理删除**（避免连带删掉 `route_member` 编排；与 `save_channel` 的既有语义一致）；
- 渠道本身**仍默认关闭**（`enabled = 0`）→ 模型**不会**出现在 `GET /v1/models`；启用开关属 T24。


### 6.3 WorkBuddy（登录态型）

**本机实测（2026-11-25，只读扫描）：**

| 实测项 | 结果 |
|---|---|
| 设备标识 | `~/.workbuddy/device-id`（36B UUID `357bb4e7-…`；`device-id.machine` 同值）**可读可写** |
| 用户登录态目录 | `~/.workbuddy/app/users/<64位hex>/`（目录存在；内部结构**未验证**，任务 T22 确认） |
| 连接器密钥 | `~/.workbuddy/app/connector-keys/<uuid>.key`（32B 二进制；`~/.workbuddy-key-fallback/connector-keys/` 存同名备份） |
| 全局配置 | `~/.workbuddy/app/app-config.json` 仅含 locale 等，无凭证 |

**adapter 设计：**

1. 扫描 `app/users/` 取当前登录态（T22 抓包确认文件与字段后固化读取逻辑）。
2. 转发时携带登录态凭证 + **复写后的 device-id**（隔离用途，默认不复写用户本机文件，仅在"多账号隔离"模式下经确认写入）。
3. 上游 endpoint = WorkBuddy 自家 API host（**未验证，T22 抓包产出**）。
4. 失效判定：401/403 → 标记账号 `expired`，UI 提示"请在 WorkBuddy 桌面端重新登录"。
5. 读取登录态文件**只读**，绝不改写 WorkBuddy 自身数据（区别于 TraeWorkAssistant 的登录态切换/指纹伪造——v1 不做切换器，只做"读当前登录态 + 转发"）。

> **T22 实测修订（2026-11-28，详见 §6.3.1）**：上面 1/2/3 三条与**实测不符**，已列入下方「设计变更提案」，**待头头批准后才改写本节**——按 §12 的「提案 → 头头批准 → 更新 02 → 才可动代码」顺序，本卡**只记录事实、不擅自改口径**。

**设计变更提案（2026-11-28，T22 产出，待头头批准）**

| # | 现有条款 | 实测结论 | 建议修订 |
|---|---|---|---|
| P1 | ①「扫描 `app/users/` 取当前登录态」 | 登录态**不在** `app/users/<hash>/`（本机实测该目录**为空**）；真实位置见 §6.3.1 §4 | 改为「读 `<sharedDataPath>/auth/<authentication.id>.info` 的 `auth.accessToken` 字段」 |
| P2 | ②「转发时携带**复写后的 device-id**」 | **device-id 不参与 chat 请求**（§6.3.1 §3） | **删除该条**——多账号隔离改用「独立 token + 独立 `X-User-Id`」，**不复写用户本机 device-id**（也免了 §5.7 指纹伪造的嫌疑） |
| P3 | ③「上游 endpoint = WorkBuddy 自家 API host（未验证）」 | = `https://copilot.tencent.com`，chat 端点 `POST /v2/chat/completions`（§6.3.1 §1） | 写死为常量 + 出站口径（头清单见 §6.3.1 §2） |
| P4 | ⑤「读取登录态文件只读」 | 文件**字段级 AES-256-GCM 加密**，主密钥在 `~/.workbuddy/keyblob`，**构建期密钥只在定制 Electron 运行时内**（§6.3.1 §4） | **新增一条硬约束**：凭证获取方式**待头头拍板**（用户显式提供 / 逆向定制运行时 / 降级档），**本卡不自行决定** |

### 6.3.1 WorkBuddy 协议记录（T22 实测）

> **取证方式**：WorkBuddy 是 **Electron 桌面端**（本机安装于 `D:\IDE\WorkBuddy`，`version` = `37.10.3-43`，`app.asar` 182 MB / 9894 个文件）。**发行版代码即协议真值源** → 本节证据全部取自 `app.asar` 内模块（给到模块名 + 常量/函数名），并与本机运行日志 `~/.workbuddy/logs/main.log` 交叉验证。**全程只读**，未写回 WorkBuddy 任何文件；**本文不含任何 token / cookie / device-id 明文值**。

#### §1 上游 host 与端点

| 项 | 值 | 证据 |
|---|---|---|
| 产品域名 | `https://copilot.tencent.com` | asar `main/server.js` 常量 `LF="https://copilot.tencent.com/v2/chat/completions"`；`product.cloudhosted.json` → `links.officialWebsite="https://copilot.tencent.com/cli"` |
| 站点 / 配置域 | `https://www.workbuddy.cn` | `product.internal.json` / `product.ioa.json` → `endpoint`；main.log 实测 44 次 `https://www.workbuddy.cn/v3/config` |
| **chat 端点** | **`POST /v2/chat/completions`** | asar `main/auth-liveness.js` 路由表：`{id:"chat.stream", routeId:"core-api", method:"POST", pathPattern:"/v2/chat/completions", replayClass:"forbidden", maxEndpointAttempts:2, commitPoint:"first-valid-token"}` |
| 公开配置 | `GET /v3/config` | **实测 HTTP 200，无需鉴权**（返回 `{code:0, data:{agent,models,mcp,codebase,features}}`） |
| 模型目录 | `…\resources\app.asar.unpacked\cli\product.cloudhosted.json` | **免鉴权普通 JSON**，22 个模型；字段 `id` / `name` / `credits` / `vendor` / `maxOutputTokens` / `maxInputTokens` / `supportsToolCall` / `supportsImages` / `maxAllowedSize` / `supportsReasoning` / `onlyReasoning` / `tags` / `relatedModels` |
| 额度 | `GET /v2/user/cloudagent/quota` | asar `main/authority-snapshot.js` 路由清单（**响应形状未实测**） |

模型目录抽样（`product.cloudhosted.json`，22 条）：
`default`（x2.00 credits）/ `auto` / `glm-5.1` / `glm-5.0` / `glm-5.0-turbo` / `glm-5v-turbo` / `glm-4.7` / `glm-4.6` / `glm-4.6v` / `deepseek-v4-pro` / `deepseek-v3-2-volc` / `deepseek-v3.1` / `deepseek-v3-0324` / `kimi-k2.6` / `kimi-k2.5` / `kimi-k2-thinking` / `hunyuan-chat` / `hunyuan-2.0-instruct` / `hunyuan-image-v3.0` / `minimax-m2.5` / `minimax-m2.7` / `deepseek-v4-pro-exclusive`。

#### §2 鉴权头（`main/server.js` → `VlmUpstream.send` / `buildVlmGatewayHeaders` / `applyGatewayHeaders` / `applyTraceHeaders`）

**必填**

| 头 | 值 |
|---|---|
| `Authorization` | `Bearer <accessToken>` —— **最后写入**，覆盖此前任何值 |
| `X-User-Id` | `<account.uid>` |

**产品头**（`buildVlmGatewayHeaders`）

`X-Product: SaaS` / `X-IDE-Type: WorkBuddy` / `X-IDE-Name: WorkBuddy` / `X-IDE-Version: <appVersion>` / `X-Private-Data: true` / `User-Agent: WorkBuddy/<ver> WorkBuddy/<ver> CLI/<cliVer>`
（`User-Agent` 的这一串格式经 main.log 实测交叉验证：`"user-agent":"WorkBuddy/5.9.2 WorkBuddy/5.9.2 CLI/2.165.0"`）

**会话头**（`applyGatewayHeaders`）

`X-Conversation-ID` / `X-Conversation-Request-ID` / `acp-connection-id` / `X-Conversation-Message-ID`（UUID）/ `X-Request-ID`（UUID）/ `X-Agent-Intent: craft` / `X-Agent-Purpose: conversation` / `x-codebuddy-request: 1` / `x-requested-with: XMLHttpRequest` / `Accept: application/json` / `Content-Type: application/json`

**链路头**（`applyTraceHeaders`）

`traceparent: 00-<32hex>-<16hex>-01` / `b3: <32hex>-<16hex>-1-<16hex>` / `X-B3-TraceId` / `X-B3-ParentSpanId` / `X-B3-SpanId` / `X-B3-Sampled: 1` / `X-Trace-ID`

**条件头**：`X-Enterprise-Id` + `X-Tenant-Id`（企业版）/ `X-Domain`（`auth.domain`）/ `X-Department-Info` / `X-Refresh-Token`

**转发前剥离的头**（集合 `zF`，逐字取自代码）：
`connection` `keep-alive` `te` `trailer` `trailers` `transfer-encoding` `upgrade` `proxy-authorization` `proxy-connection` `host` `content-length` `expect` `accept-encoding` **`authorization`** **`x-workbuddy-mcp-context`** `origin`
→ **入站来的 `Authorization` 会被剥掉，再由上游自己的 token 重新注入**（与 Copilot 的凭证供给口径同构）。

**token 失效语义**：`401` → 调 `refreshSession()` → **重放一次**（`retryAfterRefresh`；上游 401 时先把响应体缓存下来再重放）。与 T21 的 Copilot「过期前 300 s 自动续」是同一类语义。

#### §3 device-id / 设备指纹 / 签名 —— **不参与 chat 请求**

| 东西 | 真实用途 | 证据 |
|---|---|---|
| `~/.workbuddy/device-id`（36B UUID）、`device-id.machine` | **不上 chat 请求**；二者属 `WORKBUDDY_PLATFORM_OWNED_FILES`（平台自管文件） | asar `main/server.js` |
| `X-Machine-Id` | **只用于 `POST /v2/feedback`** | asar `main/facade.js`（`{"X-Machine-Id":…,"X-User-Id":…,"Authorization":…}` + `/v2/feedback`） |
| **qimei36**（腾讯 QIMEI 设备指纹，`@tencent/qimei-node` + `qimei.dll`，`WORKBUDDY_QIMEI_APP_KEY`） | **注入 sidecar / CLI 进程的「环境变量」**，**不是 HTTP 头** | asar `main/sidecar-manager.js`：`let o=(await this.dependencies.getQimei36?.())?.trim(); let s=o?{[Te]:o}:{}` → 并入 child env |
| `X-Device-Token` / `X-Device-Token-Error`（Turing 设备令牌，`TuringShieldSDK.dll`） | **只用于签到类请求**（`getCheckinRequestHeaders`） | asar `main/account.js` + `main/authority-snapshot.js` |
| `x-workbuddy-mcp-context`（HMAC-SHA256 签名信封） | **本地 VLM 代理的会话信封**，转发前被剥离 | asar `main/server.js` → `VlmContextEnvelope.sign()` / `zF` |

**结论**
1. **chat 请求无动态签名、无设备指纹绑定** → **不触发 03 §5 的降级触发条件**（「抓包显示请求含动态签名」**实测不成立**）。
2. **不需要、也不允许**为此实现任何指纹伪造 / 签名伪造（§5.7 无冲突面）。

#### §4 登录态：字段名与位置（**只记字段名，不记值**）

| 项 | 实测 |
|---|---|
| 文件 | `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\workbuddy-desktop.info`（10137 B） |
| 路径规则 | `<sharedDataPath>/auth/<authentication.id>.info`（`main/file-authentication-storage.js` → `getAuthSavePath()`）；`sharedDataPath` = `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public`；`authentication.id` = `workbuddy-desktop` |
| **不是** | `~/.workbuddy/app/users/<64位hex>/` —— 本机实测该目录**为空**（§6.3 旧口径 P1） |
| 格式 | **普通 JSON**；敏感字段为**字段级加密** `{"$wbEncrypted":1,"envelope":"<base64>"}` |

字段清单（顶层 `account` / `auth` / `accounts` / `allAccounts`）：

| 字段 | 形态 |
|---|---|
| `account.uid` | 明文 UUID（36） |
| `account.uin` | 明文串（12） |
| `account.uniqueUserId` | 明文串（45） |
| `account.type` / `account.idp` / `account.accountType` | 明文串 |
| `account.loginChannels` | 数组（n=1，元素为 12 字符串） |
| `account.deployStatus` | 对象（`statusCode` / `statusMsg` / `detailMsg`） |
| `account.sso` | 对象（`domain` / `domainModifiedTimes`） |
| `account.nickname` / `account.phoneNumber` | **加密信封** |
| `auth.accessToken` | **加密信封** |
| `auth.refreshToken` | **加密信封** |
| `auth.tokenType` | 明文串（6） |
| `auth.expiresIn` / `auth.refreshExpiresIn` | 数字（实测 2419200 / 2591999） |
| `auth.expiresAt` / `auth.refreshExpiresAt` / `auth.lastRefreshTime` | 毫秒时间戳 |
| `auth.domain` | 明文串（19） |
| `auth.sessionState` | 明文 UUID |

**加密链（逐跳）**

```
atRestSecretKey（构建期密钥，base64 32B）
  └─ sha256(该字符串的 UTF-8 字节) = protector key（keyId 9127dea1b44020a7）
       └─ 解 ~/.workbuddy/keyblob → master key（32B，keyId fca8d6b22ed65a97）
            └─ AES-256-GCM 解字段信封 {suite,keyId,nonce,authTag,ciphertext}
                 └─ accessToken / refreshToken 明文
```

- 信封算法：**AES-256-GCM**（`AtRestCrypto`，AAD 前缀 `WB-AAD\0` / 包裹前缀 `WB-WRAP\0`）。
- `~/.workbuddy/keyblob` = `{version:1, keyId:"fca8d6b22ed65a97", slots:[{type:"static-v1", protectorKeyId:"9127dea1b44020a7", wrapped:"<base64>"}]}`。
- **构建期密钥不在磁盘、不在 asar** —— 由**定制 Electron** 的 `electron.workbuddyStorage.loggerGet()` 在运行时提供（`main/index.js` → `loadMainCredentialProtectionBootstrap(policy, provider, {configDir})`）。代码里**没有** DPAPI / `CryptProtectData` 调用（asar 全量扫描 0 命中）。

#### §5 可实现性判定（T23 的输入）

| 侧 | 结论 |
|---|---|
| **协议侧** | **可转发** —— 无签名、无指纹绑定；端点 / 头清单 / 401 自动续 全部明确 |
| **凭证侧** | **卡点** —— 要拿到 `accessToken`，必须复现上面整条 at-rest 解密链，而**构建期密钥只在定制 Electron 运行时内** → 需要逆向定制 Electron 才能取到 |
| **请求体侧** | **未验证** —— daemon 对 body 是**字节级透传**（`new Uint8Array(e.body)`），body 由 CLI / agent 运行时构造 → 与「v1 入站仅 OpenAI chat」（§5.6）的**兼容性未实测** |

→ **建议（待头头批准）**：T23 的凭证获取方式三选一 —— ① **用户显式提供 token**（UI 粘贴 → 进保险库，最省事、不碰厂商保护）；② 逆向定制 Electron 取构建期密钥（成本高、逼近 §5.7 边界）；③ **降级档**（只做「保活 + 额度查看 + 模型目录」，转发延至 v2，03 §5 预案已列，工期减 3–5d）。

#### §5.1 付费前提（2026-11-28 头头口述 + 本机实测补证）

> **头头口径**：**没有付费之前，无法在 WorkBuddy 建立 token。**

**本机补证（只读，未解密任何信封）**

| 项 | 实测 |
|---|---|
| 账号 | `account.type = "personal"`；`uniqueUserId = "personal_e236dd98-…"`；`loginChannels = ["openplatform"]`；`uin` 为 12 位数字串 |
| 会话 | **活的**：`auth.lastRefreshTime` = **2026-11-28 09:49:26**（本次核查前 1 分钟）；`expiresAt` = 2026-12-26（28 天）；`refreshExpiresAt` = 2026-12-28（30 天）；`tokenType = "Bearer"`；`auth.domain = "copilot.tencent.com"` |
| 额度消耗 | `~/.workbuddy/workbuddy.db` → `session_usage` **46 行**，每行带 `credit_json`（各模型 credits 消耗，如 `15.04` / `769.69`）→ **额度确实在被消耗** |
| 付费结构 | 订阅包字段：`PackageCode` / `PackageName` / `DeductionEndTime` / `CycleEndTime` / `AutoRenewFlag`；额度字段 `usageTotal` / `usageUsed` / `usageLeft`（asar `main/account.js`） |
| 档位映射 | `free` / `freeMon` / `freeMonIntl` / `gift` → `isPro=false`、`isTria=false`；`proTrialMon` / `proTrialYear` → `isTria=true`；`proMon` / `proMonPlus` / `proYear` → `isPro=true`；另有 `youth` / `advanced` / `flagship` / `buddyOneVip`（asar `main/account.js` → `resolveEditionFlagsBySubscriptionCode`） |
| 客户端开关 | `subscription-gate@workbuddy-builtin = true`、`weixinpay@workbuddy-builtin = true`；`product.cloudhosted.json` → `productFeatures.DisableCreditsEntry = true`、`DisableCreditPurchaseGuide = false`（asar `main/account.js` → `getTuringDeviceTokenResultForRequest` 邻近的套餐常量表） |

**两条硬事实（都从发行版代码读到，不是推测）**

1. **WorkBuddy 没有面向第三方的官方 API Key。** asar 里的 `/v2/api-keys` **不是**「给外部开发者签发长期 key」的通道——它只服务于 `connect_cloud_service` 工具：请求体 `{name:"workbuddy-connect-cloud-service", expire_in_seconds:900, temporary:true}` → 返回 `{code:0, data:{key:"ck_t_…", expires_at}}`，**15 分钟临时 key**，供 `miora_*`（多模态生成）与 `neodata-financial-search` 等云服务 skill 用。**不能**当反代凭证。
2. **唯一长期凭证就是会话 `accessToken`**，而它被 at-rest 加密（§4）→ 要拿到它**只有 P4-b 一条路**（逆向定制 Electron）。

**结论（对排期的直接影响）**

- 付费前：**P4-a 与 P4-b 都不产生可交付物** —— P4-b 逆向出来的是**你自己账号额度**下的会话 token，**无官方授权通道**、**逼近 §5.7 边界**、且把个人账号额度分给 harness **违反平台 ToS**（§11 风险 1）；P4-a 粘贴来的仍是同一类 token。→ **现在做这两条都是白工**。
- 因此 **M4 判据②「WorkBuddy 登录态可作为上游完成对话」在付费前不可达** → **T23 应延后到付费后**；付费后**第一件事是查有没有官方 API Key / 开发者通道**（发行版里另有 `open.workbuddy.cn` / `api.skillhub.cn` / `open-test.workbuddy.cn` 等域，**待查**），**而不是默认走逆向**。
- 付费前**能**做的只有**免鉴权**的两件：`GET /v3/config`（保活 / 版本配置探测，实测 HTTP 200）+ `product.cloudhosted.json`（模型目录，22 个模型）→ 对应 03 §5 降级档的「保活 + 模型目录」；**「额度查看」需鉴权 → 付费前也不可达**。
- **T24（ToS 弹窗 + 启用开关）可只做 Copilot 那半边**，WorkBuddy 的开关等有合规凭证再做 → M4 判据③ 对 Copilot 先闭环。

#### §6 本节未验证项（不猜）

- **请求体 / 响应体的 JSON schema**（是否 OpenAI `chat/completions` 兼容、`tools` / `stream` / `usage` 形状）—— daemon 只透传，需真发一次才能定。
- **真实 chat 请求端到端**（WorkBuddy 当前**未运行** → 本轮未做活体抓包；且需先解决凭证）。
- `/v2/user/cloudagent/quota` 的**响应形状**（额度查看降级档要用）。
- `X-Device-Token`（Turing）在**非签到**场景是否也会被附带（代码里只见到签到路径调用点）。

### 6.4 trait 契约

```rust
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn kind(&self) -> ChannelKind;                       // ApiKey | OAuth | Session
    async fn complete(&self, ctx: &CallContext, req: OpenAIRequest)
        -> Result<OutgoingCall, AdapterError>;           // 返回"已改写"的出站调用（URL/头/body）
    async fn probe(&self) -> Result<ProbeResult, AdapterError>;   // 连通性/额度
    async fn refresh(&self) -> Result<(), AdapterError>; // OAuth 型必须实现；其余 no-op
}
```

`openai_compat.rs` 是唯一通用实现；预设只提供数据。Copilot/WorkBuddy 各自实现 trait（主要差异在凭证供给与头改写）。

### 6.5 pi 接入指引（写入 UI 帮助页）

**本节是 pi 接入方式的唯一权威载体。** 依据：pi 1.6.1 自带文档
（`~/.pi/agent/install/releases/<版本>/node_modules/@earendil-works/pi-coding-agent/docs/`
与 `schemas/models.schema.json`），以及 2026-11-26 的实连验证（见
`docs/reviews/T10-channel-matrix-and-pi.md`）。

#### 6.5.1 安装 pi（Windows）

官方 PowerShell 安装脚本（脚本自带 Node 预检与 managed install）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File <(curl -fsSL https://pi.dev/install.ps1)
```

约束：pi 要求 **Node.js ≥ 22.19.0**。脚本在**无交互终端**（`[Console]::IsInputRedirected`）
下会跳过自带的 `Install-NodeStandalone`，直接落到系统 Node——若系统 Node 版本过低会报
`error: Pi requires Node.js 22.19.0 or newer.`。此时用**脚本自带的同一个函数**先装独立 Node：

```powershell
# 从官方脚本里摘出 Install-NodeStandalone（去掉末尾 Invoke-PiInstaller 调用）后 dot-source 调用
. .\pi-install-lib.ps1 ; Install-NodeStandalone
```

它从 nodejs.org 下载 + SHA256 校验 + 解压到 `%LOCALAPPDATA%\pi-node\current`，**不动用户既有 Node 环境**；
装完把该目录放到 `PATH` 前端再跑官方脚本即可完成 managed install（本机验证：Pi 1.6.1，
装到 `%USERPROFILE%\.pi\agent\bin\pi.cmd`）。

#### 6.5.2 配置文件位置

| 文件 | 作用 |
|---|---|
| `~/.pi/agent/models.json` | provider 定义（baseUrl / api / apiKey / models） |
| `~/.pi/agent/settings.json` | 默认 provider 与 model（可选） |
| `~/.pi/agent/auth.json` | pi 自己的登录态凭据（`/login` 写入；本方案不用它） |

JSON schema 以 pi 安装目录内的 `schemas/models.schema.json` 为准（provider 可选字段：
`name` / `baseUrl` / `apiKey` / `api` / `headers` / `compat` / `authHeader` / `models` /
`modelOverrides`；model 条目**只强制 `id`**，其余有默认值）。

#### 6.5.3 配置片段

```json
{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "llm-router": {
      "name": "LLM Router (本地网关)",
      "baseUrl": "http://127.0.0.1:8787/v1",
      "api": "openai-completions",
      "apiKey": "$LLMR_CLIENT_KEY",
      "models": [
        { "id": "<别名>", "name": "<别名>", "contextWindow": 128000, "maxTokens": 8192 }
      ]
    }
  }
}
```

关键点：

1. **`api` 只能是 `openai-completions`**：v1 入站仅 OpenAI 协议（§0.2），网关不转换协议。
2. **`apiKey` 写环境变量插值 `$LLMR_CLIENT_KEY`，文件里不留明文**。pi 的插值语法：
   `$NAME` / `${NAME}` 取环境变量，`$$` → 字面 `$`，`$!` → 字面 `!`，`!command` → 取命令输出。
   环境变量必须在**启动 pi 的那个进程**里存在。
3. **`models[].id` 必须是网关发布的别名**，且与 `GET /v1/models` 同源。
4. `contextWindow` / `maxTokens` 按实际渠道能力填；填错只影响 pi 侧的预算与截断，不影响网关转发。

**别名清单不要手抄**：用 `scripts/pi_models.py` 从应用库直读生成（与 `GET /v1/models` 同一份数据）：

```powershell
python scripts/pi_models.py --store "$env:APPDATA/llm-router/store.db" `
    --out "$env:USERPROFILE/.pi/agent/models.json" `
    --settings-out "$env:USERPROFILE/.pi/agent/settings.json" --write
```

#### 6.5.4 验证命令

```powershell
$env:LLMR_CLIENT_KEY = "<client key 明文>"      # §3.1：创建时只显示一次
pi --list-models llm-router                      # 应列出别名
pi --model llm-router/<别名> -p "用一句话回答：1+1 等于几？"
```

#### 6.5.5 实连结论（2026-11-26，mock 上游经真网关）

| 项 | 实测结果 |
|---|---|
| `pi --list-models llm-router` | 列出 `deepseek-chat`（context 128K / max-out 8.2K） |
| 一轮带工具调用的对话 | 成功：模型出 `read` tool_call → pi 执行真实工具 → 结果回传 → 模型收尾 |
| 网关请求数 | **2 次**（第 1 次出 tool_call，第 2 次带 tool 结果收尾） |
| 密钥改写 | 网关把入站 client key 换成上游密钥（上游侧看到的 `Authorization` 是渠道密钥） |
| 用量落库 | 2 条 `usage_log`，`status=success estimated=0 prompt=11 completion=5`（上游真 usage，非估算） |

pi 默认工具是 `read` / `bash` / `edit` / `write`；**流式**下的 `delta.tool_calls` 帧经网关透传正常，
这是「改写器不是转换器」在真实客户端上的直接证据。

### 6.6 应用内 UI 面板口径（T9 落地）

| 面板 | 职责 | 权威数据源 |
|---|---|---|
| 渠道 | 渠道与模型别名的增删改、启用/禁用、优先级、密钥录入 | `channel` / `model` 行（`store::channels`） |
| 加密保险库 | 密钥录入、**30s 回看遮蔽**、删除、明文/加密备份导出、主密码模式 | `credential` 行 + `vault.bin` |
| Client Key | 创建（明文只显示一次）、标签与限额（rpm / 每日 tokens）编辑、删除 | `client_key` 行 |
| **模型预览** | 列出客户端能调到的别名 + 接入片段（base_url / api_key / model） | **与 `GET /v1/models` 同源** |
| 熔断状态 | 当前冷却中的 `(渠道, 上游模型)`、熔断设置、手动解除 | 运行中的熔断器 + `circuit_state` 行 |

**两条口径（防止后续实现走偏）：**

1. **模型预览必须与网关同源**：UI 预览的别名 = `GET /v1/models` 返回的别名 = 同一条查询
   （`store::channels::list_published_models`）。**不得**在 UI 侧另写一份查询——两份查询必然漂移
   （用户在 UI 看到的模型，客户端却调不到）。
2. **跨面板数据变更要广播**：改了数据的面板（如渠道面板存了密钥）要通知其它面板刷新，
   否则会出现"渠道显示密钥已录入，保险库却显示 0 条凭据"的自相矛盾。
   实现上用轻量事件总线（`src/panels/bus.ts`）+ 定时轮询兜底，不引第三方依赖。

### 6.7 应用内 UI 功能分组与导航（T26 落地）

**问题（2026-11-28 头头指出）**：8 个面板在 `src/App.tsx` 单页平铺（渠道 / Copilot / 保险库 / Client Key / 模型目录 / 熔断 / 用量 / 签到），
首屏被三张说明卡片占掉，找功能靠纵向滚动，无位置感。

**口径（头头 2026-11-28 拍板：4 组方案）**：

| tab | 归入面板 | 语义 |
|---|---|---|
| **接入** | 渠道 · 模型目录 · Copilot 反代 | 「把渠道/上游接进来」 |
| **凭证** | 加密保险库 · Client Key | 「保管密钥与本地口令」 |
| **运维** | 熔断状态 · 用量统计 | 「运行期的健康与花费」 |
| **签到与积分** | 签到面板（T15–T19） | 「自动签到与积分」 |

**四条实现口径**：

1. **分组定义是纯数据**：放在 `src/tabs.ts`（`TABS: TabDef[]`，含 `id` / `label` / `hint` / 面板列表），
   **可单测**（`node --test`），不与 React 组件耦合。
2. **一次只渲染当前 tab 的面板**（**不做「全渲染 + CSS 隐藏」**）→ 首屏 DOM 显著变小，签到面板那 39 KB 的区块不再拖累其它页。
3. **网关状态常驻顶栏**（运行状态点 + 监听地址 + `base_url`）——**切到任何 tab 都能一眼看到「网关在不在跑」**；
   「快速开始（从零接入三步）」移入**「接入」组首卡**（它讲的就是配渠道）；「当前范围」移到底部**折叠区**（历史说明，不是操作入口）。
4. **选中项持久化**：`localStorage` 键 `llm-router.activeTab` → 重开应用回到原地；**未知值回落到默认 tab（接入）**。

**边界**：不加路由库（单页 tab 用 `useState` 即可）；不引第三方依赖；**不改任何面板内部逻辑**（只改「谁被渲染」）。

---

## 7. 自动签到框架

### 7.1 双轨设计

| 轨道 | 实现 | 适用 |
|---|---|---|
| **HAR 重放**（主轨） | 存 HAR 1.2 JSON + 变量表；调度时按模板重放 HTTP 请求，按响应特征（状态码/JSON 路径断言）判定成败 | 纯接口型签到（多数站点） |
| 浏览器自动化（辅轨） | sidecar 起 Playwright chromium 持久化 profile | 需 JS 渲染/OAuth 跳转的少数站点 |

**HAR 模板化规范**：导入 HAR → 交互式标记变量（如 `{{cookie}}`、`{{csrf}}`）→ 保存为站点模板（`checkin_site.config_json`）。用户可自建站点插件，无需写代码（区别于签到盒类框架：模板即配置）。

### 7.1.1 HAR 1.2 子集与模板结构（T15 落地）

**自写解析**（02 §1.3），只取 HAR 1.2 的下列字段，其余（`timings` / `cache` / `pages` / `browser` / **响应体**）**一律丢弃**：

| 来源 | 保留 | 丢弃 |
|---|---|---|
| `log.entries[].request` | `method` / `url` / `headers` / `queryString` / `postData.text` + `mimeType` | `cookies` 数组（与 `headers` 里的 `Cookie` 重复）、`postData.params` |
| `log.entries[].response` | **只留 `status`** | 响应体、响应头（实测 ModelScope 签到响应的 `Set-Cookie` 带 `csrf_session` / `csrf_token`，属密文） |

**模板结构**（`checkin_site.config_json`；`version` 用于将来改格式）：

```json
{
  "version": 1,
  "source": "modelscope.har",
  "entryCount": 167,
  "request": {
    "method": "GET",
    "url": "https://www.modelscope.cn/api/v1/users/login/info",
    "headers": [{ "name": "Cookie", "value": "acw_tc={{auto_acw_tc}}; csrf_session={{cookie}}; csrf_token={{csrf}}" }],
    "query": [],
    "postData": null
  },
  "variables": [
    { "name": "cookie", "target": { "kind": "headerPart", "field": "Cookie", "part": "csrf_session" }, "sensitive": true, "auto": false, "credentialId": "cred_xxx", "default": null },
    { "name": "csrf", "target": { "kind": "headerPart", "field": "Cookie", "part": "csrf_token" }, "sensitive": true, "auto": false, "credentialId": "cred_yyy", "default": null },
    { "name": "auto_acw_tc", "target": { "kind": "headerPart", "field": "Cookie", "part": "acw_tc" }, "sensitive": true, "auto": true, "credentialId": "cred_zzz", "default": null }
  ],
  "assert": { "status": 401 }
}
```

- **占位符语法**：`{{name}}`；`name` 只允许 `[A-Za-z0-9_-]`，同一模板内全局唯一。
- **替换发生在"值内部"**：标记 `Cookie` 里的某个子项时，模板保留整条 Cookie 的其余部分，只把该子项的值换成占位符（`headerPart`）；标记整个头换整条值（`header`）；查询参数同理（`query`）；JSON 请求体按点路径替换字符串叶子（`body`）。
- 导入的模板 **`enabled = 0`**（02 §7.3：站点插件默认全关）。

### 7.1.2 变量标记与密文边界（T15 落地）

**向导四步**：选 HAR 文件 → 解析并列出 entry → 选一条作为签到请求 → 逐字段标记变量 → 保存模板。

- **候选自动识别**：
  - **`Cookie` 头的所有子项一律按敏感处理**（cookie = 会话凭据，**不靠名字猜**）——实测 ModelScope 的 Cookie 里 `cna`、`ssxmod_itna` 这类名字不含关键词，但同样是会话态数据；
  - 其它字段按**键名关键词**（大小写不敏感，命中即默认建议标记）：
    `cookie`、`set-cookie`、`authorization`、`x-csrf-token`、`csrf`、`token`、`session`、`jwt`、`acw_tc`、`x-xsrf-token`。
- **未标记的敏感字段自动补齐**：敏感但用户没标的字段，系统自动补成 `auto_<字段名>` 变量（`auto = true`）——**不能留在模板里**（会明文泄漏），**也不能丢掉**（重放会失效）。
- **密文边界（AGENT.md §5.3）**：
  - `checkin_site.config_json` 存**模板**：被标记的值在模板里是占位符，**不含任何真实密文**；落库前自检——若模板里仍出现被标记字段的原始值，**拒绝保存**。
  - 变量的**真实取值**存**保险库**（`credential` 表，AES-256-GCM 密文）：Cookie 子项用 `kind = 'cookie'`，其它敏感头/参数用 `kind = 'access_token'`（01 §4.4 的 kind 取值）。
  - **归属渠道**（头头 2026-11-27 裁决 · 方案 A）：`credential.channel_id` 是 NOT NULL 外键，而签到站点在数据模型里没有渠道 → 每个站点归属一个渠道：导入时**选一个已有渠道**，或**用站点名新建一个**（`provider = 'custom'`）。与 02 §7.2「积分回写 `account.quota_json`」同源（`account` 属于渠道）。
  - **非敏感变量**（如固定 ID、日期、结构类请求头）可**直接留在模板里**，不入保险库。
- **变量取值更新**：重录 HAR 后同名变量覆盖旧凭据（删旧 `credential` 行、写新行），模板里的 `credential_id` 同步更新。
- **IPC 只传掩码值**：导入返回给 UI 的是**敏感值掩码**（`***`），真实值不出 Rust 进程；保存时前端回传**文件路径 + 标记清单**，Rust 重新读取并解析（无导入态驻留内存）。

### 7.1.3 重放引擎口径（T16 落地）

**一句话**：模板 + 保险库里的真实取值 → 拼成一次 HTTP 请求 → 断言响应 → 出一个**错误分类** → 写一行 `checkin_result`。

**1. 变量注入**

- 敏感变量：按 `credential_id` 从保险库解密；非敏感变量：用模板里的 `default`。
- **缺凭据不发请求**：直接分类 `credential_missing`（凭据不存在 / 被删 / 保险库锁着）。
- 替换发生在**值内部**（与 §7.1.1 的四种 target 一致）：整个头值、`Cookie` 头里的子项、查询参数值、JSON 请求体叶子。
- **未知占位符拒绝重放**：模板里出现 `{{x}}` 但变量表没有 `x` → 分类 `template_invalid`（否则会把 `{{x}}` 原样发到上游）。

**2. 请求构造**

- 方法 / URL / 头 / 查询 / 请求体全部取自模板。
- **query 以模板 `request.query` 数组为准重建**：HAR 的 `url` 自带 query 且与 `queryString` 内容重复 → 先从 URL 剥掉 query string，再按数组重建（否则同一个参数会发两遍）。
- **剔除 `Host` 与 `Content-Length` 头**：这两个由 HTTP 客户端按实际 URL 与请求体生成；留着录制时的旧值会把请求打歪（`Host` 与 URL 不一致时尤其危险）。
- 超时：连接 **10s**、整次请求 **30s**（可配）；每次重放是一次独立请求，**引擎自己不重试**（重试与冷却属 T17 调度层）。

**3. 响应断言**

- `assert.status` 是**期望状态码**（不是"录制时的状态码"）：导入时先填录制值占位，用户应改成签到成功时的值（T18 的站点插件会带正确断言）。
- `assert.jsonPaths`（可选）：`{"path": "data.ok", "equals": true}`；点号路径，支持数组下标（`data.items.0.id`）。
  - 路径不存在 → 断言失败；存在但值不等 → 断言失败；响应体不是 JSON → 断言失败。
  - `equals` 缺省 = 「存在即通过」。
- 状态码与 JSON 路径断言是**与**关系，都过才算成功；两者都没配 → 只判「请求没出错且状态码是 2xx/3xx」。
- **旧模板兼容**：T15 落库的模板只有 `assert.status`，新增字段一律**可选**（缺失按"没配"处理），旧模板继续可用。

**4. 错误分类**（引擎的唯一输出口径）

| 分类 | 判据 | 可重试 |
|---|---|---|
| `success` | 断言全过 | — |
| `assert_failed` | 状态码不符 / JSON 路径不符 | 否 |
| `network_error` | 连接失败 / 超时 / 读响应体失败 | 是 |
| `credential_missing` | 模板变量的凭据缺失 | 否 |
| `template_invalid` | 模板不可用（JSON 解析失败 / 未知占位符 / 断言配置非法） | 否 |
| `credential_invalid` | 上游 401 / 403 | 否 |
| `rate_limited` | 上游 429 | 是（冷却与重试属 T17） |
| `http_error` | 其它 4xx / 5xx | 5xx 是 / 4xx 否 |

- 「可重试」沿用 §3.3 的判据（429 / 5xx / 连接超时 / 网络错可重试；401/403/400 不可重试）。
- **不猜验证码**：不内置验证码识别（AGENT.md §5.7 不做风控规避）。验证码表现为状态码或正文与断言不符 → 归 `assert_failed` / `http_error`；要单独表达由用户用 JSON 路径断言或站点插件表达。

**5. 结果落库**

- 每次重放写一行 `checkin_result`（`site_id` / `ts` / `ok` / `message` / `reward_json`）。
- `ok = 1` **仅当**分类为 `success`；`message` 是人类可读的中文原因（T19 通知直接用）。
- **响应体不进库**：失败排障用的响应体片段只在内存里（`ReplayOutcome.body_preview`，截断 512 字节），**不落 `checkin_result`**——响应体可能带 csrf / token 一类密文。
- `reward_json` 在 T16 **留空**：积分提取与 `account.quota_json` 回写属站点插件（T18）/ 调度层（T17）。

**6. 边界**

- T16 只做引擎：**不含**调度（cron）、系统计划任务、通知、浏览器自动化辅轨、UI 触发入口。
- 「立即执行一次」按钮留给 T17 / T18 一并做（已记 backlog）。

### 7.2 调度与通知

- 应用内：tokio 定时器（cron 语义，`cron` crate）。
- 系统兜底：Windows 计划任务 / macOS launchd 注册保底触发（应用未运行时），唤起后补签。
- 结果写 `checkin_result`；获得积分回写 `account.quota_json`（签到与额度池同源——本项目的差异化点）。
- 通知：系统通知 + 可选 webhook（企微/飞书/Telegram）。

### 7.2.1 调度与补签口径（T17 落地）

**1. 调度模型**

- **每分钟 tick 一次**（tokio 定时器），用 `cron` crate 解析 5 字段表达式（分 时 日 月 周）。
- **本地时区**：cron 表达式按**用户所在时区**解释（`chrono::Local`），不是 UTC。
- 站点级 cron 存在 `checkin_site.schedule`（**空 = 不参与调度**）；`enabled = 0` 的站点不跑（02 §7.3）。

**2. 应跑判定（去重）——一个应触发周期最多跑一次**

- `due_at` = **最近一次应触发时刻**（≤ 现在的那个）。算法：取 cron 的下一次触发 `T_next`，再取 `T_next` 之前紧邻的那次触发即 `due_at`。
- `last_attempt` = 该站点最近一次**尝试**（不论成败）的 `checkin_result.ts`。
- 判定：`last_attempt >= due_at` → **本周期已跑过，跳过**；否则 → 该跑。
- 因为 `last_attempt` 落库，**跨重启也成立**（应用重启不会把同一个周期再打一遍）。

**3. 错过补签**

- **应用启动**与**系统计划任务唤起**时做**同一套判定** → 关应用期间漏掉的周期自动补上（判据：「关闭应用后计划任务仍触发并补签」）。
- 新导入的站点 `enabled = 0` → 不跑；用户**启用**后第一个 tick 会跑一次（此时 `last_attempt` 为空）。
- **不因失败而立刻重试**：同一应触发周期只跑一次；下次周期自然重试。要立刻再试用「立即执行一次」。

**4. 系统兜底任务**

- **Windows**：每个「启用且有 cron」的站点注册**一个**计划任务，任务名 `LLM Router Checkin - <site_id>`（**连字符**，与代码 `system::TASK_PREFIX` 一致），动作 = `"<exe>" --checkin-once --site <site_id>`。
  - cron → 计划任务频率映射：分钟字段固定 + 小时 `*` → `/sc hourly`；分钟字段 `*/N` → `/sc minute /mo N`；**分钟字段 `*` → `/sc minute /mo 1`（每分钟）**；固定时分 → `/sc daily`；带星期 → `/sc weekly /d MON,TUE,…`；其余表达式 → **退回每天一次**（宁可多跑，不可漏跑）。
- **macOS**：`~/Library/LaunchAgents/com.llmrouter.checkin.<site>.plist`（`StartCalendarInterval`；`*/N` 分钟用 `StartInterval = N×60`）。
- **注册时机**：站点**启用**或 **cron 变更**时同步注册；**停用或删除**时注销。注册/注销/查询走系统命令（Windows `schtasks`、macOS `launchctl`）。
- **「任务不存在」= 已注销**：注销走**先查后删**（`/query` 判存在 → 不在就直接返回"已注销"），**不靠错误文案匹配**——`schtasks` 的报错文案随系统语言变（英文系统 "ERROR: The system cannot find the file specified." / 中文系统 "错误: 系统找不到指定的文件。"），按字符串匹配必然漏判。
- **错误文案按系统 OEM 代码页解码**：`schtasks` 在**无控制台的 GUI 进程**里按 OEM 代码页输出（中文 Windows = GBK/936），直接按 UTF-8 解会出乱码；解码顺序 UTF-8 → UTF-16LE → OEM 代码页 → 有损兜底。
- **CLI 入口**：`llm-router.exe --checkin-once [--site <id>]` → **不起 UI、不起网关**，跑完补签判定后退出——这正是计划任务/launchd 唤起后的执行方式。

**5. 边界**

- 本卡**不含**：通知（T19）、积分回写（`account.quota_json`，T18）、浏览器自动化辅轨（02 §7.1 辅轨）、多用户/云同步。
- **调度层不重试**：T16 引擎一次一打；失败只落 `checkin_result`，由下个应触发周期再试。

### 7.3 合规边界

- 站点插件默认全关，启用即视为用户知晓自动化签到可能违反站点条款。
- 不内置任何"反风控"能力（不伪造指纹、不绕验证码；撞到验证码即失败并通知）。

### 7.4 站点插件口径（T18 落地）

**1. 插件机制（`checkin_site.plugin` 的取值与分派）**

- `plugin` 是**站点类型**，决定「这次签到怎么打」：

  | 取值 | 来源 | 执行方式 |
  |---|---|---|
  | `har` | 用户自建（T15 导入模板） | 走 **T16 重放引擎**（`checkin::replay`） |
  | `modelscope` | **内置**（T18） | 走 **`checkin::sites::modelscope`**：接口固定、断言固定、用户只提供登录态 |
  | `minimax_code` | **内置**（T18） | 走 **`checkin::sites::minimax_code`** → **浏览器辅轨**（`checkin::sidecar`） |

- 内置插件与 HAR 模板**共用同一套结果语义**：分类仍用 02 §7.1.3 §4 的 8 类 `ReplayClass`，`ok = 1` 仅当 `success`，`message` 是可直接用于通知的中文原因。
- **内置插件带正确的断言**——这正是 T16/T17 留档里那条已知限制的解法（导入模板会带着**录制时的状态码**：ModelScope 匿名态 401 / 真实成功 200，用户必须自己改）。内置插件的断言**随插件发布**，用户不用改。
- **插件结果可带积分**（`Reward { amount, unit, raw }`）→ 落 `checkin_result.reward_json`（T16 留空的字段，本卡填上）→ 见本节 §4。

**2. ModelScope 插件（内置，接口型）**

- **签到**：`GET https://www.modelscope.cn/api/v1/users/login/info`。
- **对账**：`GET https://www.modelscope.cn/openapi/v1/magicubes/transactions`（取魔粒余额 / 流水）。
- **认证**：**两套体系**（**已实测**，2026-11-27 匿名态 + 假凭据探测）——
  - `/api/v1/*` = **网页会话 Cookie**（签到走这套；与 T15 抓的 HAR 夹具同源）；
  - `/openapi/v1/*` = **开放平台 Bearer token**（`/openapi/v1/users/me` 的报错原文是 `missing or invalid Authorization header`）——**不是网页 Cookie**。
- **凭据需求（渐进增强）**：
  - **访问令牌**（`ms-…`）是必需的——用户在应用里填 → 进保险库 `credential`（**不落模板、不落库、不进日志**）；**签到与查魔粒都用它**（见下「访问令牌」一条）；
  - **也接受原始 Cookie 头**（用户手工抄的 `name=value; …`）→ 原样使用，**但不再是推荐路径**；
  - **查不到魔粒不影响签到成功**（**不因为查不到积分就判签到失败**）；
  - **网页会话 Cookie 也能开 `/openapi/v1/magicubes/transactions`**（真机实测 2026-11-27：Cookie 形态与 Bearer 形态**回同一个 200 + 同一份流水**）→ **一个令牌，签到与对账都够用**。
- **接口面（已实测，2026-11-27 匿名态）**：

  | 路径 | 匿名态 | 用途 |
  |---|---|---|
  | `GET /api/v1/users/login/info` | **401** `{"Code":10019901001,"Message":"禁止访问","Success":false}` | **触发当日签到** |
  | `GET /openapi/v1/magicubes/transactions` | **401** `{"success":false,"code":"InvalidAuthentication"}` | 对账（流水） |
  | `GET /openapi/v1/magicubes/balance` | **401** `{"success":false,"code":"InvalidAuthentication"}` | **魔粒余额**（同样存在，探测得出） |
  | `GET /api/v1/users/binding` | **200** `{"Code":200,"Data":{…},"Message":"success"}` | 匿名可读——**证明 `/api/v1/*` 成功响应体的形状**：`Code` / `Data` / `Message` / `Success` |
  | `GET /api/v1/users/me` | 400 `{"Code":10010104001,"Message":"获取用户信息失败…"}` | 存在 |

  不存在的路径回 `404 page not found` → **可以用「401 统一错误体 vs 404」在无登录态时判定接口是否存在**（本卡的接口面就是这么探出来的）。
- **断言**：判成功必须断言 **JSON 业务字段**，不能只看状态码——
  - `/api/v1/*` 系列：成功时 `Success == true`（`Code == 200`）；失败时 `Success == false` + `Message` 是中文原因（如「禁止访问」）。
  - `/openapi/v1/*` 系列：成功时 `success == true`；失败时 `code == "InvalidAuthentication"` + `message`。
  - 依据：`/api/v1/users/binding` 匿名可读且回 `Success`/`Code` 两套字段 → **`/api/v1/*` 与 `/openapi/v1/*` 是两套不同的响应体约定**。
- **魔粒流水的真实形状（真机实测，2026-11-27，登录态）**：`GET /openapi/v1/magicubes/transactions` → **200**，
  顶层 `{"data":{"page","page_size","records":[...],"total"},"request_id","success"}`，
  `records[]` 每条 = `{category, category_key, count, days_remaining, description, expire_at, expire_days, gmt_created, rule_key, title, total_amount, type}`：
  - **金额字段 = `total_amount`**（**不是** `amount` / `points`）——实测当日 `daily_active` 那条 `total_amount = 200`；
  - **日期字段 = `gmt_created`**（实测值 `"2026-11-27"`，**只有日期、没有时分秒**）；
  - **类型 = `type`（实测 `EARN`）+ 规则 = `rule_key`**——实测当日三条：`daily_active`（每日签到）/ `infocomplement_profile_experience` / `infocomplement_email_verification`（后两条是**一次性资料奖励**，各 50）。
- **奖励口径（「本次拿到多少」）**：
  - **只认 `rule_key == "daily_active"`**（每日签到那一笔）——**不把同日的资料完善一次性奖励算进来**（实测当日 `daily_active` 200 + 资料类 50 + 50，合计 300 **≠** 本次签到拿到）；
  - **用「签到前后的差值」算本次到账**：签到前查一次当日 `daily_active` 总额（基线）→ 打签到 → 再查一次 → **差值 > 0 才是本次拿到**；
    - **为什么必须用差值**：当日那条 `daily_active` **整天都在**（重复签到不会让它消失）→ 直接取当日总额，会把**同一天的第 2、3 次运行**也当成新到账，**额度池被重复计数**；
    - 差值 = 0 → 判**「今天已领过」**：`points = null`、**不回写额度池**（与 §3 MiniMax 的「今天已领」口径一致），当日总额写进回执文案；
  - 站点 `config_json` 的 **`reward.path` 覆盖仍然生效**（用户自己说了算）。
- **登录态用「访问令牌」，不用手抄 Cookie**（2026-11-27 真机反馈：「cookies 只能手动输入吗？这样很不友好，并不是所有人都会获取 cookies」）：
  - **魔搭的开放平台访问令牌**（`ms-…`，在「个人中心 → 访问令牌」**点一下复制**就有）**可以直接当网页会话用**——`Cookie: m_session_id=<ms-token>` → `GET /api/v1/users/login/info` 即触发当日签到（第三方魔搭签到脚本的实测口径）；**同一个令牌也是 `/openapi/v1/*` 的 Bearer**（查魔粒）→ **一个令牌，签到 + 魔粒全包**；
  - **为什么不用「让应用开浏览器自动登录」**：魔搭登录页带**人机验证**，它**专门拦自动化浏览器**——真机实测：Chrome 顶部挂着「**自动测试软件正在控制 Chrome**」，滑块直接报 **`验证失败（error:22phmt）`**，滑块根本没渲染出来。而**做反检测 / 绕过验证码是 AGENT.md §5.7 红线** → 这条路是死的，**不内置**；
  - **为什么不用「浏览器接管会话」**（辅轨开浏览器 → 登录 → 把 Cookie 取回来）：凡是我们能读 Cookie 的浏览器，**都是被自动化控制的浏览器** → 登录同样过不了人机验证；
  - **手抄 Cookie 的那条路保留**（粘进来的原始 `name=value; …` 头 → 原样使用），但**不再是推荐路径**；
  - **密文边界不变**：令牌**只进保险库**，不回显、不进日志、**不进 IPC 明文**；
  - **判别规则**（纯函数，单测覆盖）：密文**含 `=`** → 当作原始 Cookie 头；**不含 `=`** → 当作访问令牌（`m_session_id=<令牌>`，并作为 Bearer）。

**3. MiniMax Code 插件（内置，辅轨型）**

- **为什么必须走辅轨（不能走 HAR 主轨）**：其签到接口为 `GET /minimax-cloud/api/v1/signin/status`（读签到面板）与 `POST /minimax-cloud/api/v1/signin/claim`（领取当日奖励），请求带 **OAuth bearer + `x-timestamp` + `x-signature` + `yy`** 等头——**签名每次都变**（客户端公开盐值派生），HAR 模板化无法重放；自己实现签名算法 = **逆向客户端私有协议**，撞 AGENT.md §5.7「不做签名伪造」。**让浏览器自己算签名**（页面自己的 JS 完成）既可行又合规。
- **辅轨形态**（承 02 §7.1「sidecar 起 Playwright chromium 持久化 profile」）：
  - `sidecar/`（仓库根，Node 脚本）：`playwright` 驱动**浏览器**跑签到流程，**持久化 profile** 复用登录态；
  - **浏览器**：优先用**系统 Chrome**（`channel: "chrome"`），没有则回落 Playwright 自带 chromium——**不强制下载 150MB**；
  - **Rust ↔ sidecar 协议**：**CLI 协议**——Rust 每次调辅轨就**起一个 Node 进程**跑一个动作，
    **stdout 输出 JSON**，用完即退。比本地 HTTP 更少活动部件：**没有端口占用/冲突、没有孤儿进程、
    没有"本地 socket 要不要鉴权"的问题**。动作有三个：

    | 动作 | 作用 |
    |---|---|
    | `health` | 探活 + 报浏览器可用性 + profile 路径 |
    | `login` | 拉起**有头**浏览器到登录页，等用户**手动登录**（登录态写进持久化 profile）→ 供一次性交接 |
    | `checkin` | 打开签到页 → 触发签到 → **从页面自己的网络响应里取结果**（不解析 DOM 里的数字）→ 返回 `{ok, points, day, message}` |

    - **登录态交接**：用户只在浏览器里登录一次，profile 落盘后**之后都是无头运行**；
    - **超时**：辅轨动作的耗时**不受重放引擎的 30s 限制**（浏览器启动 + 页面加载更慢），
      按动作分别给（`checkin` 90s / `login` **15min**——真人要输密码、过人机验证，5min 太紧；
      可用 `LLMR_LOGIN_TIMEOUT_MS` 覆盖）。

  - **结果以网络响应为准**：辅轨不自己算积分，**读页面自己发出的 `/signin/claim` 响应**——站点怎么算就怎么记，避免「页面显示与接口不一致」时误报。
- **登录流程由辅轨铺路（真机实测，2026-11-27）**：`login` 动作**自动打开登录弹窗**并**自动切到「账号密码登录」**——
  - **为什么必须走账号密码**：站点对**短信/验证码登录**有频控，真机实测点「获取验证码」直接报**「操作频繁」**，短信发不出去（这是站点的防刷策略，不是我们的 bug）；**账号密码登录没有这个频控**；
  - 辅轨**只做「点开 tab」这一步**——**不代填、不代输、不过人机验证**（撞 AGENT.md §5.7：不做验证码绕过）；剩下的账号密码与人机验证由用户在浏览器里自己完成；
  - 铺路失败（页面改版、找不到 tab）**不阻断**：辅轨继续等用户自己操作。
- **登录态的判定信号（真机实测，2026-11-27）**：**三条独立判据，任一命中即判已登录**——
  - **判据 A（硬信号，不依赖文案）**：页面自己的 API 请求 `/minimax-cloud/api/v1/config?...&user_id=` 里 **`user_id` 从 0 变成真实 id**（匿名态实测是 `user_id=0`）——登录后页面发请求必然带真实 id；
  - **判据 B**：「登录」按钮在整个浏览器上下文里**都消失**（**连续两次读到 0** + **至少停留 5 s**）——匿名态 `agent.minimax.cn` 页面上有**两个** `BUTTON` 文本为「登录」（页头 + 首页卡片），登录成功后换成用户信息。**必须看整个上下文**：登录流程可能开新标签页，只看最初那一页会把「原页的登录按钮还在」误当成没登录；
  - **判据 C（正面信号）**：出现**签到入口**（连签面板只有登录后才出现）——连续两次读到即判成功；
  - **必须等页面渲染完再判**：该站是前端渲染，`DOMContentLoaded` 时按钮还没挂上——
    此时「登录按钮数 == 0」会被**误判成已登录**（真机踩过：`login` 动作秒回「登录完成」）。
    判据用「**页面上出现按钮**」（`waitForSelector("button")`）而不是 `networkidle`
    ——该站有持续请求，`networkidle` 实测要等 20 s 才超时；
  - **登录成功的判定要连续两次读到 0 个按钮**（间隔 1 s）**且至少停留 5 s**——单次读数不可靠。
- **限流要能识别、不能让人白等**：辅轨在等待期间扫页面上的限流文案（「操作频繁」等）——
  命中就**立刻返回**并说明原因（而不是让用户对着 15 min 超时干等）；超时未登录时，若**登录框还开着**，回执里明说「密码没提交 / 验证码没过 / 站点仍在限流」。
- **签到入口的真实位置（真机实测，2026-11-27，登录态下）**：**不在首页**——首页**一个字都没有「签到」**；
  入口在**用户头像下拉菜单**里：
  - 下拉项实测顺序：设置 / 用量 / **每日签到** / 问题反馈 / 联系我们 / 了解更多 / 退出登录；
  - 头像下拉的触发器是 `.ant-dropdown-trigger`，**页面上有两个**（第二个是「添加附件或技能」，**没有文字**）→ **只认有文字的那个**；
  - 点「每日签到」→ 打开**签到面板**；当天那格的按钮是**禁用态**的「今日已签到」。
- **签到状态接口的真实形状（真机实测，登录态）**：`GET /minimax-cloud/api/v1/signin/status` →

  ```json
  {"data":{"scene":3,"days":[
    {"day_no":1,"points":400,"status":3,"is_today":false,"bonus_points":0},
    {"day_no":4,"points":1000,"status":3,"is_today":false,"bonus_points":0},
    {"day_no":7,"points":1000,"status":3,"is_today":true,"bonus_points":0}]},
   "base_resp":{"status_code":0,"status_msg":"ok"}}
  ```

  - **`status == 3` = 已领取**（实测：今天那格面板显示「今日已签到」，接口里今天那条 `status` 也是 3）；
  - **今天那一条** = `days` 里 `is_today == true` 的那条 → `points` = 当日积分、`day_no` = 连签第几天；
  - **口径与 03 任务卡对上了**：**每日 400 分、第 4/7 天各 1000**（实测 `points` 字段，**不再是任务卡的推测**）；实测当天为**连签第 7 天、当日 1000 分**；
  - 业务成功看 **`base_resp.status_code == 0`**（**MiniMax 约定**：HTTP 200 也可能是业务失败，AGENT.md §6 已知坑 1）。
- **「今天已领」= 成功（幂等）**：辅轨的判定顺序是 **DOM 优先**（面板里出现「今日已签到」）→ **接口兜底**（今天那条 `status == 3`）→ 命中即判 **`ok = true` + `alreadyClaimed = true`**（**不是失败**）。
  - **已领过时 `points` 报 `null`**：本次**没拿到**积分（是之前领的）→ **不回写额度池**，避免重复计数；当日积分与连签天数写进回执文案。
- **profile 与 AGENT.md §5.3 的边界**：持久化 profile 里的会话凭证**由浏览器自己加密保管**（Windows DPAPI / macOS Keychain），**应用不读取、不导出、不入库、不进 IPC**；AGENT.md §5.3 的「凭证零明文」约束的是**我们的存储路径**（保险库 / IPC / 日志 / 库文件），辅轨的浏览器 profile 是**第三方组件的凭证存储**，落在应用数据目录、不进 git。
- **签到幂等**：重复领取返回 `claim_result = 2` / 业务码 `AlreadyClaimed` → **判为「今天已领」而不是失败**。

**4. 积分回写 `account.quota_json`（本项目的差异化点：签到与额度池同源）**

- 签到拿到积分 → 更新该站点**归属账号**的 `account.quota_json`（**额度快照**，不是余额真值）。
- `quota_json` 的形状：`{"points": 400, "unit": "magicube", "source": "checkin", "updated_at": <unix>}`；**只增不减的语义由取数方负责**——签到侧**只写「本次拿到了多少」**，不猜测账户总额。
- **写回与签到结果解耦**：签到成功但回写失败 → **签到仍算成功**（结果已落 `checkin_result`），回写失败记日志 + 提示，**不回滚签到**。
- **归属账号的落点**（与 §7.1.2 的「站点归属渠道」同源）：内置站点建站时按站点名建一条**占位渠道**（`provider = 'custom'`），再取 §8.2 的**默认账号** `acct_default_<channel_id>`；站点 `config_json` 里记 `channelId` / `accountId`。
  - 理由：`credential.channel_id` 与 `account.channel_id` 都是 `NOT NULL REFERENCES channel(id)`（§9）——**站点没有渠道就没有地方挂凭据，也没有账号可写积分**；
  - 与 T15 的 HAR 模板站点**同一套归属口径**（一个站点 = 一个渠道 + 一个默认账号）。
- **合并写，不覆盖限额字段**：`quota_json` 里 `points` / `unit` / `source` / `updated_at` 与 §8.2 的限额字段（`rpm` / `rpd` / `tpm` / `tpd` / `learned_at` / `last_rate_limit`）**并存**——回写只改这四个键，**不动**其它键；`source = "checkin"` 用来与限额来源 `upstream_headers` 区分。

**5. 边界与未验证项**

- 本卡**不含**：通知（T19）、多站点批量编排 UI、浏览器 profile 的跨机迁移、辅轨的分发形态（见下）。
- **未验证（真机口径）**：① **ModelScope 的「本次到账 > 0」分支**——真机当天**已领过**（当日 `daily_active` 200 魔粒当天就在流水里），实测跑出来的是**差值 = 0 → 判「今天已领过」**（**幂等分支已验**；**差值 > 0 的分支要等次日首次运行**才能验）；② MiniMax Code 的 **`/signin/claim` 真正领取的响应体**（真机当天**已经领过**，面板是禁用态「今日已签到」，**没有触发过 claim**）与**积分回写对真实站点**（实测那次回写为 0——已领过 → 本次无新增积分）。
- **已实测收敛（2026-11-27，ModelScope 访问令牌）**：① **真签到成功**——`GET /api/v1/users/login/info` 带 `Cookie: m_session_id=<ms-令牌>` → **HTTP 200**、`Success = true`、`Message = "success"`，**277 ms**，**落库 `ok = 1`**；② **魔粒流水的字段形状已收敛**（`data.records[]` + `total_amount` + `gmt_created` + `rule_key` + `type`，见 §2）；③ **网页会话 Cookie 也能开 `/openapi/v1/*`**（与 Bearer 回同一份流水）；④ **签到接口的响应体只是用户资料**（`Code` / `Data` / `Message` / `RequestId` / `Success`，`Data` 里是头像/昵称/邮箱等，**还含 `GitlabAccessToken` 等敏感字段**）→ **站点不返回「今日魔粒 / 今天已领」信号**，插件**不存响应体**（`body_preview` 留空），「本次 vs 已领」只能靠流水差值判定。
- **已实测收敛（2026-11-27，MiniMax Code 登录态）**：① **签到入口已定位并收敛成一条**——**用户头像下拉 → 「每日签到」**（不再是文案候选）；② **每日积分与连签天数已实测**——**每日 400、第 4/7 天各 1000**（`/signin/status` 的 `points` 字段），实测当天为**连签第 7 天**；③ **「今天已领」路径已在真实站点跑通**（Rust → Node → Playwright → 系统 Chrome → 真实 `agent.minimax.cn` → 落库 `ok=1`，**4307 ms**）。
- **真机已验证的部分（2026-11-27）**：两站的**失败路径**已在真实站点上跑通——ModelScope 用错误 Cookie → **HTTP 401** → 判 `credential_invalid`（152 ms）；MiniMax Code 未登录 → 辅轨开真实浏览器读页面 → 判 `not_logged_in`（2.1 s）。即**「能打到真站点 + 断言与分类正确」已验证**；**成功路径**：MiniMax Code 已验（落库 `ok=1`，4307 ms），**ModelScope 也已验**（访问令牌 → 200 / 277 ms / 落库 `ok=1`）。
- **登录交接的已知约束（真机，2026-11-27）**：站点**短信/验证码登录有频控**——点「获取验证码」直接报**「操作频繁」**，短信发不出去；**账号密码登录没有这个频控**。辅轨已**自动切到账号密码 tab**（§3），但**只铺路不代劳**：密码、人机验证仍由用户自己在浏览器里完成（撞 AGENT.md §5.7，**不绕验证码**）。
- **探测脚本不许反复打线上站点**：用**临时 profile** 反复访问会被站点判成「新设备」并打频控（真机踩过：直接把短信通道打限流）。**正式辅轨一律用持久化 profile**；探测脚本只在必要时用一次。
- **辅轨的已知成本**：依赖 **Node 运行时**（本机实测 v22.16.0 ✓）+ `playwright` npm 包（`sidecar/` 目录，`npm install`）。**「开箱即用」的打包分发形态不在本卡**——另排。
- **删站点不清凭据**：`checkin_delete_site` 只在 `config_json` 是 HAR 模板时清理凭据；**内置站点的凭据留在保险库**（下次建同名站点会复用同一渠道）。**已记 backlog**。

### 7.5 通知口径（T19 落地）

**1. 触发时机与落点**

- **落点 = `checkin::scheduler::run_site` 之后**（**结果已落 `checkin_result` 之后**）——一处落点覆盖**全部四条执行路径**：应用内每分钟 tick / UI「立即执行」/ CLI `--checkin-once` / **Windows 计划任务（应用关闭时）**。
- **成功与失败都通知**：M3 判据要求「成功写 `checkin_result` 并通知」「失败通知含站点名与原因」。
  - **失败永远通知**（不可关）——AGENT.md §5.7「签到撞到验证码 = 失败并通知」在这里落地；
  - **成功也通知**可关（`notify_success`，默认开）——每天一条系统通知对多数人是需要的，但可以关。

**2. 通知内容（文案口径）**

| 字段 | 内容 |
|---|---|
| 标题 | `签到成功 · {站点名}` / `签到失败 · {站点名}` |
| 正文 | 结果分类 + **中文原因** + （成功时）**本次到账多少**；失败时**必须含站点名与原因**（M3 验收判据） |

- **零敏感信息**：只带分类、中文原因、站点名与本次到账数；**不带**凭据、Cookie、令牌、完整响应体（§5.3）。

**3. 通道**

- **系统通知**（Windows 通知中心 / macOS 通知 / Linux DBus）：用 **`notify-rust`**——
  **必须在 Tauri 运行时之外也能用**（CLI / 计划任务路径**没有 Tauri app handle**，
  用 Tauri 官方通知插件就只能在应用开着时通知，**覆盖不了「应用关闭时」这条判据**）。
- **Webhook**（群机器人）：**企微群机器人 / 飞书群机器人 / Telegram Bot** 三种格式共用一个配置：

  | 类型 | 请求 | 请求体 |
  |---|---|---|
  | 企微 | `POST {url}` | `{"msgtype":"text","text":{"content":"…"}}` |
  | 飞书 | `POST {url}` | `{"msg_type":"text","content":{"text":"…"}}` |
  | Telegram | `POST {url}`（url 里带 bot token） | `{"chat_id":"…","text":"…"}` |

  - **Telegram 需要额外的 `chat_id`**（单独一项设置，不是密钥）。

**4. 配置存哪（密文红线）**

- `app_setting`（键值字符串）只存**非密文**项：
  `notify_enabled`（总开关）/ `notify_success` / `notify_webhook_kind`（`wecom` / `lark` / `telegram` / 空）/
  `notify_webhook_credential_id`（**只存 id**）/ `notify_telegram_chat_id`。
- **webhook 地址与 bot token 是密文**（企微/飞书的 URL 里带 `key`，Telegram 的 URL 里带 token）→
  **进保险库**（`credential`，`kind = 'webhook'`，挂在占位渠道「通知」上——`credential.channel_id` 是 NOT NULL 外键）。
  **零明文**：不落 `app_setting`、不进日志、不回显到 IPC（UI 只显示「已配置」）。
- **默认值**：通知**默认开**（系统通知零配置即可用）；webhook 默认不配（没地址就不发）。

**5. 失败与边界**

- **通知发送失败不影响签到结果**（与 §7.4 §4「写回失败不回滚签到」同口径）：只记日志（**不回显 URL**），**不重试**（不轰炸）。
- 本节**不含**：邮件 / 短信 / 钉钉 / 企微**应用**消息、通知历史与已读状态、模板自定义、@人。
- 系统通知依赖 **OS 通知权限**（Windows 专注助手会挡；macOS 需授权）；Telegram 在**网络受限**区域不可达（大陆用户默认推荐企微 / 飞书）。

**6. 系统通知的「静默丢弃」与如实报告（真机实测，2026-11-27）**

Windows 的 toast 有**两层**独立的拦截，任一层挡住都会**静默丢弃**（`notify-rust` 照样返回成功，日志写着「已发送」、屏幕上什么都没有）：

| 层 | 机制 | 现场取证 | 修法 |
|---|---|---|---|
| ① **调用进程没有对应 AUMID** | Windows 10 1607+ 要求调用进程拥有该 `System.AppUserModel.ID`，否则**丢弃** | `PushNotifications\Backup\<AUMID>` 里没有我们的记录 | 自建 AUMID + COM 注册开始菜单快捷方式（`checkin::system_notify::ensure_aumid`） |
| ② **全局通知总开关关着** | `HKCU\Software\Microsoft\Windows\CurrentVersion\PushNotifications\ToastEnabled = 0` → **所有**应用的 toast 被拦 | 事件日志 `Microsoft-Windows-PushNotification-Platform/Operational`：`Id=3150 … PolicyReason [GlobalSettingDisabled]` | **如实报告**：发送前读这个开关，关着就报「Windows 全局通知已关闭（设置 → 系统 → 通知）——请打开后再试」，**不再谎报成功** |

- **②层的两个坑（真机踩过）**：
  - 改完注册表**必须重启 `WpnUserService_<sid>`**（或注销重登）才生效——通知平台服务把策略**缓存**在内存里（连改两次注册表、当场重发都还是 `GlobalSettingDisabled`）；
  - 这个开关是**全局**的：它关着时**别的应用的 toast 同样被拦**（本机另一个应用的通知也记着同一条 `Id=3150`）→ 所以「PowerShell 原生 toast 也不显示」**不是** AUMID 问题，别再往那条路查。
- **投递成功的判据**（比「API 返回成功」可靠）：事件日志出现 `Id=3153 Toast … is delivered to <AUMID> on session N`，且通知库 `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db` 的 `Notification` 表里有对应 payload（被丢弃的 toast **从不入库**）。

### 7.6 站点目录与声明式站点（T25 落地）

**问题（2026-11-28 头头指出）**：「很多 agent harness 都可以有签到领积分」——
但**新增一家平台的边际成本**现状偏高：内置插件要写 Rust + 发版；HAR 模板要抓包 + 逐条标变量 + 手改断言（**无断言编辑 UI**，T16/T17 留档已知限制）。

**结论（头头 2026-11-28 拍板）**：**框架不缺，缺的是「轻量接入通道」**——
引擎层（注入 / 重放 / 断言 / 8 类分类）、调度层（cron + 系统兜底）、通知层**都已是通用且已验证**；
因此**新增「声明式站点（spec）+ 站点目录（catalog）」**：新增一家平台 = 填一份 JSON（或导入一个目录文件），**不改一行 Rust、不发版**。
提案与影响面见 `docs/reviews/proposal-t25-t26.md`。

#### 7.6.1 spec 数据模型

```jsonc
{
  "id": "my-platform",              // 目录内唯一 id（导入时按它去重）
  "name": "我的平台",                // UI 展示名
  "method": "POST",                 // 默认 GET
  "url": "https://example.com/api/checkin",
  "headers": [ { "name": "Authorization", "value": "Bearer {{token}}" } ],
  "query":   [ { "name": "day", "value": "{{today}}" } ],
  "body":    { "mimeType": "application/json", "text": "{\"type\":\"daily\"}" },
  "credentialId": "cred_xxx",       // 敏感值来源 = 保险库里的凭据 id（真实值不出模板）
  "assert":  { "status": 200, "jsonPaths": [ { "path": "data.ok", "equals": true } ] },
  "reward":  { "path": "data.credits", "unit": "point" },       // 可选：本次拿到多少
  "balance": { "method": "GET", "url": "https://example.com/api/balance",
               "path": "data.balance", "unit": "point" }          // 可选：当前余额
}
```

**校验规则（导入与保存都跑）**：

| 字段 | 规则 |
|---|---|
| `id` | 非空、只允许 `[a-z0-9_-]`、≤64 字符、**目录内唯一** |
| `name` | 非空、≤64 字符 |
| `url` | **必须是 http/https 的绝对 URL**（禁 `file:` / `javascript:` 等） |
| `method` | 缺省 `GET`；非空时按大写归一 |
| `credentialId` | 可选；**给了就必须能在保险库里取到值**（取不到 → `credential_missing`，**不发请求**） |
| `assert.status` | 可选；给了就是**期望状态码**（不是录制值） |
| `assert.jsonPaths` | 可选；点号路径 + 数组下标，`equals` 缺省 = 「存在即通过」 |
| `reward.path` / `balance.path` | 可选；点号路径，**取不到 → 没有积分**（**不判签到失败**） |

**占位符只有两种**（刻意不做表达式引擎，避免把 spec 变成脚本）：

| 占位符 | 类别 | 取值 |
|---|---|---|
| `{{token}}` | **敏感** | 保险库 `credentialId` 指向的凭据真实值（**只进请求，不进模板、不回显**） |
| `{{today}}` | 非敏感 | 当天日期 `YYYY-MM-DD`（`chrono::Local`）——跨天自动变，不用重录 |

#### 7.6.2 编译成现有模板（零新引擎）

spec → **`SiteTemplate`**（02 §7.1.1 的同一套结构）→ 交给**已验证的重放引擎**（§7.1.3）执行：

- `variables`：`token`（`sensitive = true` + `credentialId`）、`today`（`default = <当天日期>`）。
- `request`：method / url / headers / query / postData **一一映射**。
- `assert`：`status` + `jsonPaths` **直接映射**（§7.1.1 §3 的 `TemplateAssert`）。
- 于是**注入、占位符替换、断言、8 类分类、重试语义、结果落库**——**全部沿用现有实现，一行不改**。
- 唯一新增：**编译后的模板 + 积分/余额路径提取**。

#### 7.6.3 目录（catalog）

- **目录文件**：`{ "version": 1, "sites": [ spec, … ] }`。
- **导入**：逐条校验 → **逐条建站**；**非法条目带原因返回**（**不静默丢弃**）；建站后 **`enabled = 0`**（与 §7.3「站点插件默认全关」一致）；
  **同名 id 视为更新**（幂等：同一份目录导入两次 → 站点数不变）。
- **导出**：把自建站点导出成目录文件——**只含 `credentialId`（id，不是密文），不含任何真实凭据** → 可分享给别的 harness 用户。
- **内置目录**：首版只收**已实测**的条目（ModelScope / MiniMax Code 走**代码插件**，以「内置条目」形式列出，用户不必重填）+ 一个**空白模板**（通用 HTTP 签到，填 URL 即可）。
  **不虚构 endpoint**：任何「听说的」平台（智谱 / 硅基流动 / 讯飞等）**不进内置目录**，等实测后再收（进 backlog）。

#### 7.6.4 积分：本次拿到 vs 当前余额

| 概念 | 口径 | 落点 |
|---|---|---|
| **本次拿到** | 这一次签到**入账了多少**（`Reward{amount, unit, raw}`，`raw` 只留数值/布尔——§7.4 §4 密文边界不变） | `checkin_result.reward_json` + `account.quota_json` 合并写 |
| **当前余额** | 平台账户**现在有多少**（`spec.balance` 声明式查询） | **只回前端展示**，**不落 `quota_json`**（不与 §8.2 限额自学习字段打架） |

- **UI 文案严格区分**：「本次拿到 400 积分」 vs 「当前余额 1,200 积分」——**「本次拿到」四个字不省，「余额」不用于前者**。
- **取不到余额不算失败**（余额接口挂了不影响签到结论）。

#### 7.6.5 分派与落库

- `checkin_site.plugin = "spec"`（新增取值）→ `scheduler::execute` 分派时走**第三支**：编译 → 引擎 → 取 `reward`。
- **不改 schema**：复用 `checkin_site`（`plugin` / `config_json` / `schedule` / `enabled` 四个字段够用）。
- 站点归属渠道与账号口径**沿用 T18**（占位渠道 `provider = custom` → 默认账号 `acct_default_<channel_id>`），
  积分回写 `quota_json` 的**合并写**语义不变。

#### 7.6.6 边界（明确不做）

- **不做**验证码绕过 / 指纹伪造 / 签名伪造（AGENT.md §5.7）——撞上验证码 = 失败并通知（与 §7.3 一致）。
- **不做**任意表达式 / 脚本化 spec（占位符只有 `{{token}}` 与 `{{today}}`）。
- **不内置未实测的 endpoint**。
- **不改重放引擎的分类与断言语义**、**不改 schema**、不引第三方依赖。
- 不做多账号切换器、不做代理池、不做「签到后自动下单/兑换」等业务动作（**只签到与查余额**）。

---

## 8. 用量统计

- 采集：上游 `usage` 精确值优先；无则 tiktoken-rs 估算并标 `estimated=1`。
- MiniMax 特例：`usage.completion_tokens_details.reasoning_tokens` → `usage_log.reasoning_tokens`（§4.2）。
- 成本：`price` 表（用户可编辑，预填国产渠道公开牌价）→ `cost_micros`（1e-6 元）。口径见 §8.3。
- 聚合视图：按 日/渠道/模型/client_key 四维；TTFT P50/P95；输出 tokens/s。
- 配额监控：对支持额度查询的渠道（GLM/Kimi/MiniMax 包月等）定时 probe，展示剩余与重置时间。
- **限额自学习**：per `(channel, account)` 维护 RPM/RPD/TPM/TPD 计数；429 响应的 `Retry-After` 与 `x-ratelimit-*` 头、以及渠道文档上报值，写入 `account.quota_json`，作为熔断冷却时长与调度排序依据。（源自 FreeLLMAPI 参考实现，见 `docs/reviews/freellmapi-analysis.md` §4-2）

### 8.1 采集口径（T7）

**一条 `usage_log` 记录 = 一次出站尝试**（不是一次入站请求）。fallback 链试了 3 个候选就是 3 行——每个候选都真实消耗了上游额度。请求在到达任何候选之前就失败（鉴权失败 / 别名未命中 / 全部候选被跳过）**不写** `usage_log`（没有渠道上下文）。

| 字段 | 来源 |
|---|---|
| `ts` | 记录写入时刻（Unix 秒） |
| `client_key_id` | 入站 client key id |
| `channel_id` / `account_id` | 出站候选的渠道 / 账号（见 §8.2 的账号口径） |
| `model_alias` | 入站别名（客户端看到的那个名字） |
| `upstream_model` | 上游真实模型名（R-1 替换后的） |
| `protocol` | 入站协议，v1 恒为 `openai` |
| `prompt_tokens` / `completion_tokens` | 上游 `usage`；缺失时由估算器补 |
| `cache_read_tokens` | 上游 `usage.prompt_tokens_details.cached_tokens` |
| `cache_write_tokens` | 上游 `usage.cache_creation_input_tokens` |
| `reasoning_tokens` | 上游 `usage.completion_tokens_details.reasoning_tokens`（MiniMax 特例，§4.2） |
| `estimated` | 1 = 本地估算值（上游没回 usage），0 = 上游精确值 |
| `ttft_ms` | 首个 token 到达时刻（流式才有；非流式为 NULL） |
| `duration_ms` | 出站尝试耗时 |
| `status` | `success` / `error` / `aborted`（客户端中途断连） |
| `error_kind` | 失败时的错误类别（02 §4.4 的 `ErrorKind`） |
| `cost_micros` | 价目表换算（**T13** 起由落库时算出，见 §8.3）；没命中价目时为 NULL（未知，非 0） |

**流式采集**：在 SSE 通路上**旁路 tee**——转发字节的同时在**副本**上扫 `data:` 帧：找 `usage` 帧（`stream_options.include_usage` 注入后由上游主动下发，R-4），并记录**首个 token**时刻（TTFT）与输出字符数。tee 不得缓冲全量（§2.2 的零缓冲直通不变）。

**非流式采集**：响应体已整块缓冲，直接从 JSON 的 `usage` 字段解析。

**估算入口（T12 细化）**：

| 情形 | 处理 | `estimated` |
|---|---|---|
| 上游给了完整 `usage` | **原样用，不覆盖**（01 §5.5 规则 1） | `0` |
| 上游**没回** `usage` | tiktoken-rs 估算输入与输出 | `1` |
| 上游回了 `usage`，但**缺其中一项**（usage 对象里**没有这个键**；**值为 0 不算缺失**） | **只补缺的那一项**，另一项仍用上游的精确值 | `1`（这一行含非上游原样的数） |
| 出站**失败** | **不估 token**：没有产出，token 全 0 | `0` |

- **编码选择**：`cl100k_base`（默认，含全部国产模型）/ `o200k_base`（gpt-4o / gpt-4.1 / gpt-5 / o 系列）。
- **国产模型只能近似**：tiktoken 的编码与国产模型真实分词不一致，估算值**必须**带 `estimated=1`
  （01 §5.5 规则 3）；`estimated=1` 的行**不参与预算告警**（R7）。
- **绝不用估算值冒充精确值**（R7）：上游已给的精确值一律不被估算覆盖。
- 实现：`gateway::estimator::TiktokenEstimator`（tiktoken-rs）；
  `gateway::usage::CharRatioEstimator` 保留为**轻量回退**与单测用（不产线上默认）。

### 8.2 账号口径与限额自学习（T7）

**账号（account）**：v1 的 `account` 表原本只服务 OAuth / 登录态型渠道。但限额自学习要 per `(channel, account)` 计数、`quota_json` 要有唯一的落点，因此 **每个渠道至少有一个账号**：

| 渠道类型 | 账号 |
|---|---|
| API Key 型 | **默认账号**，id = `acct_default_<channel_id>`（确定性生成），首次需要写限额状态时自动补建 |
| OAuth / 登录态型 | 账号池里的真实账号（T8/T9 落地） |

这样"账号"在 v1 是个统一概念，后续接账号池不需要改采集层。

**四个窗口**（计数从 `usage_log` 派生，不另设计数器表——流水就是唯一真值）：

| 窗口 | 口径 |
|---|---|
| RPM | 最近 60 秒内该 `(channel, account)` 的**尝试次数** |
| RPD | 当日（UTC 日界）内的尝试次数 |
| TPM | 最近 60 秒内该 `(channel, account)` 的 token 总数 |
| TPD | 当日内的 token 总数 |

**已知局限**：流水只在**尝试结束时**写入，因此**在途请求不计**——并发突发时窗口计数会偏低（FreeLLMAPI 用 in-flight lease 规避，v1 未做）。若日后要做精确的速率准入，应另加在途计数。

**学习规则**（源自 FreeLLMAPI 的 "learn real limits from error bodies"）：从 429 响应的 `x-ratelimit-limit-requests` / `x-ratelimit-limit-tokens` 学到上游的**上限**，写进 `account.quota_json`：

- **只收紧，不放宽**：新学到的上限低于已记录值才覆盖。撞到上限说明我们的预检放行了太多，放宽上限只会更糟。
- `Retry-After` 与 `x-ratelimit-reset-*` / `x-ratelimit-remaining-*` 记进 `quota_json` 的 `last_rate_limit` 段（这是**事实**，不是推断）。
- "渠道文档上报值"（用户按渠道文档手填的上限）与学习值写进同一组字段，学习只会把它压低、不会抬高。
- `quota_json` 是**人工可编辑**的 JSON；用户改小它同样被视为上限，自学习不会把它改大。

### 8.3 成本换算口径（T13）

**价目表**：`price` 表，主键 `(provider, model_pattern)`，**用户可编辑**，预填国产渠道公开牌价。
三档单价单位统一为 **1e-6 元 / 1M tokens**（即 1 元/1M tokens 记 `1_000_000`）。

**匹配规则**：

- `model_pattern` 里的 `*` 通配任意字符串（可出现在任意位置，可有多个）；**不含 `*` 即精确匹配**。
- 命中多条时取**最具体**的一条：① 通配符个数少者优先 ② 字面量长度长者优先 ③ 仍相同则按 pattern 字典序取小者。
- `provider` 取渠道的 `channel.provider`（预设标识：`deepseek` / `kimi` / `glm` / `dashscope` / `minimax_cn` / `minimax_intl` / `siliconflow`）。

**换算公式**（`cost_micros`，单位 1e-6 元）：

```
可计费输入 = prompt_tokens - cache_read_tokens
cost_micros = round( ( 可计费输入   × input_per_1m
                     + completion_tokens × output_per_1m
                     + cache_read_tokens  × cache_read_per_1m
                     + cache_write_tokens × input_per_1m ) / 1_000_000 )
```

- **缓存命中 token 有独立档价，只按该档计一次**——它已经含在 `prompt_tokens` 里，不能再按输入价重复计一遍。
- 价目表**没有独立的缓存写入档**（`price` 表只有三列），故**缓存写入按输入价计**。
- 四舍五入在**总额**上做一次（不逐项舍入），避免多次舍入累积误差。

**空值语义**（`0` 与 `NULL` 必须可区分）：

| 情况 | `cost_micros` | 含义 |
|---|---|---|
| 命中价目且算出金额 | 整数 | 已知成本 |
| 命中价目但某档单价为 `NULL` | 该档按 0 计（**`NULL` ≡ 0 = 该档不计费**） | 免费档 |
| **没命中任何价目** | **NULL** | **未知（未配置牌价）**，不是 0 |

**估算行照样算成本**：上游没回 usage、由 tiktoken 估算的行（`estimated=1`）**照样算** `cost_micros`——
该行已用 `estimated=1` 标明含非上游原样的数，不必再用 NULL 表达不确定。
估算值**不参与预算告警**（§8.1 R7），但成本照算。

**改价后重算历史**：`cost_micros` 是**落库时算好的快照**。用户改价 / 删价后，**该 provider 的历史行一并重算**
（`price_upsert` / `price_delete` 自动触发），否则历史成本会永远停留在旧牌价上。

**预填牌价（快照 2026-11-26）**：取自各厂商官方页面，取价规则 = **取主列表价**
（不取促销档 / 限时折扣档 / 时段折扣档；分段计价取第一档）。

| provider | 官方来源 | 备注 |
|---|---|---|
| `deepseek` | `api-docs.deepseek.com/zh-cn/quick_start/pricing` | **峰谷定价**：空闲价 = 高峰价的一半（高峰 = 周一至周五 9:00–12:00、14:00–18:00）。表里存**高峰价**（列表价），空闲时段实际约一半 |
| `kimi` | `platform.kimi.com/docs/pricing/chat` | 单档价；缓存写入另有 TTL 档（5min/1h），本表只记缓存读取价 |
| `glm` | `docs.bigmodel.cn/cn/guide/start/pricing` | **分段计价**（按单次请求的输入长度分档，GLM-4.7 还要看输出长度）；表里存**第一档**（输入 <32K） |
| `dashscope` | `bailian.console.aliyun.com` 模型价格页 | qwen-plus 的「输入 ≤128k」档；思考模式另有一档 |
| `minimax_cn` | `platform.minimax.cn/docs/guides/pricing-paygo` | MiniMax-M3 的「≤512K 输入」档（永久五折后的现价） |
| `siliconflow` | `siliconflow.cn/pricing` | 部分模型分夜间 / 白天两档，表里存主档 |

**已知限制**：

- 价目表是**单一价目快照**，**厂商调价不会自动跟随**——每条都可编辑，改一下即可。
- **分段 / 峰谷 / 优先级服务（priority 1.5 倍）等多档定价无法用"每模型一组三价"表达**，本表只取一档；真实账单以厂商为准。
- 口径假定 **`prompt_tokens` 含缓存命中 token**（OpenAI 语义）。v1 全部出站渠道均为 OpenAI 协议，满足该假定；
  将来接入 Anthropic 协议上游（v2）时，须先把 `cache_read_tokens` 并入 `prompt_tokens` 再入库，否则会**低估**输入成本。
- `minimax_intl`（国际站）按 **USD** 计价，本表以元计价、**不做汇率折算** → 预填**不覆盖**该 provider
  （`cost_micros` 保持 NULL），用户可自行填行。

### 8.4 聚合与图表口径（T14）

**数据来源**：全部从 `usage_log` 派生——**流水是唯一真值**，不另设计数表（与 §8.2 的窗口计数同一口径）。

**四个维度**：

| `dim` | 分桶键 | 可读名 | 排序 |
|---|---|---|---|
| `day` | `ts` 按 **UTC 日**截断（`%Y-%m-%d`，与 §8.2 的 UTC 日界一致） | 同键 | **日期升序** |
| `channel` | `channel_id` | `channel.name`（查不到回退为 id） | token 总量降序 |
| `model` | `upstream_model`（上游真实模型名，R-1 替换后的） | 同键 | token 总量降序 |
| `client` | `client_key_id` | `client_key.label`（查不到回退为 id） | token 总量降序 |

- 可空列一律 `COALESCE` 成 `(未记录)`，**不因 NULL 丢数据**。
- 桶数上限默认 **50**；**合计桶不受上限影响**（截断只影响展示，不影响合计）。

**时间范围**：`ts >= since_ts`（**含**）且 `ts < until_ts`（**不含**）；默认近 **30 天**、上界不限。

**每个桶的指标**：

| 指标 | 口径 |
|---|---|
| `requests` | 桶内行数 = **出站尝试数**（一条 `usage_log` 一行，§8.1） |
| `success_count` / `error_count` / `aborted_count` | 按 `status` 分 |
| `prompt_tokens` / `completion_tokens` / `cache_read_tokens` / `cache_write_tokens` / `reasoning_tokens` | SUM |
| `total_tokens` | **输入 + 输出**（缓存读写**已含在输入里**，不重复计） |
| `cost_micros` | `SUM(COALESCE(cost_micros, 0))` = **已知成本合计** |
| `cost_unknown_rows` | `cost_micros IS NULL` 的行数（**未配牌价**）——延续 §8.3 的 `0` / `NULL` 语义：**已知成本合计为 0 不等于"不花钱"** |
| `estimated_rows` | `estimated=1` 的行数（估算行） |
| `ttft_p50_ms` / `ttft_p95_ms` | **只在 `ttft_ms IS NOT NULL` 的行上算**（= 流式行）；**无样本 → NULL**（不是 0） |
| `output_tps` | 成功行按「输出 token ÷ 耗时」加权；**无有效样本 → NULL** |

**分位数**：**最近秩法**——排序后取第 `ceil(p × n)` 个（1 起）。`p50` 即中位数，`p95` 即 P95。
（SQLite 无分位函数，故取原始值在内存里算；空样本返回 `None`。）

**输出速度（tokens·s⁻¹）**：

- **单行**：`completion_tokens ÷ (duration_ms ÷ 1000)`，**仅在 `status='success'` 且 `duration_ms > 0` 时有值**；
  否则 `NULL`（**不是 0，也不是无穷大**——耗时为 0 时速度无从计算）。
- **桶级**：`Σ(成功行的输出 token) × 1000 ÷ Σ(成功行的耗时)`（按耗时加权）。

**估算值与成本**：估算行（`estimated=1`）**照常计入**聚合与成本，但 UI 必须标出"估算"；
估算值**不参与预算告警**（§8.1 R7）。

**最近请求明细**（M2 判据）：逐条给出 tokens / TTFT / tokens·s⁻¹ / 成本 / `estimated` 标记，
按 `ts` 倒序，默认 50 条——这是"一次流式请求后 UI 显示 tokens / TTFT / tokens·s⁻¹ / 估算成本"的落地载体。

---

## 9. 数据模型（增量修订）

以 01 文档 §4.4 DDL 为基础，修订与补充：

```sql
-- channel.meta_json 约定字段（MiniMax 等）
-- { "group_id": "..." | null, "supports_groupid_query": true|false,
--   "path_chat": "/v1/text/chatcompletion_v2", "inject_stream_usage": false,
--   "auth_style": "bearer" | "custom_header", "auth_header_name": "x-api-key",
--   "extra_headers": { "x-foo": "bar" } }

-- credential 增加_oauth 型两类凭据（同 channel 下并存）
--   kind: 'api_key' | 'oauth_gh_token' | 'oauth_copilot_token' | 'session_workbuddy'
--   oauth_copilot_token.expires_at 精确到秒；刷新由 copilot adapter 负责
-- credential.kind 另增签到站点的会话取值（02 §7.1.2；挂站点的归属渠道上）
--   kind: 'cookie'（Cookie 头里的子项） | 'access_token'（其它敏感头 / 查询参数）

-- account 增加登录态路径引用（WorkBuddy）
--   meta_json: { "users_dir": "C:/Users/<u>/.workbuddy/app/users/<hash>",
--                "device_id_file": "C:/Users/<u>/.workbuddy/device-id" }

-- circuit_state：熔断状态（02 §2.3；非 closed 才留行，回到 closed 即删行）
--   主键 (channel_id, upstream_model) 与熔断粒度一致
--   state: open | half_open；reason: failures | rate_limit
--   open_until: 冷却截止（Unix 秒）；failures / rate_limit_hits 供 UI 展示

-- app_setting：应用级设置（键值对；02 §2.3 熔断时长等）
--   当前取值：circuit_open_secs = 连续失败熔断时长（秒，默认 60，最小 1）
--              rate_limit_cooldown_cap_secs = 429 冷却上限（秒，默认 60，最小 1）

-- usage_log 增加 account_id（T7 限额自学习：per (channel, account) 计数）
--   account_id：账号 id；API Key 型渠道用默认账号 acct_default_<channel_id>（02 §8.2）
--   计数（RPM/RPD/TPM/TPD）直接由 usage_log 派生，不另设计数器表
```

其余表结构沿用 01 文档，不重复。

---

## 10. 实测矩阵与验收判据

### 10.1 渠道实测矩阵（T10 任务执行）

每渠道 × 每项：

| 项 | DeepSeek | Kimi | GLM | MiniMax国际 | MiniMax国内 | 百炼 | 硅基流动 | Copilot | WorkBuddy |
|---|---|---|---|---|---|---|---|---|---|
| 非流式 chat | | | | | | | | ✔已验证 | |
| 流式 chat | | | | | | | | ✔已验证 | |
| tool_calls | | | | | | | | · 未测 | |
| usage 返回 | | | | | | | | ✔已验证 | |
| reasoning 字段 | | n/a | | ✔已验证 | | | | n/a | |
| 多轮上下文 | | | | | | | | · 未测 | |

判据：全绿才能在预设中标 `stable`；部分绿标 `partial`（UI 明示哪些特性不可用）。

> **Copilot 列（T21，2026-11-27 真机实测，账号 `api.individual.githubcopilot.com`）**：
> ① **非流式 chat ✔** —— 经真网关 `gpt-4o-mini-2024-07-18` 回「我是一个基于人工智能技术的智能助手…」；
> ② **流式 chat ✔** —— 15 个 SSE 帧真实内容直通（`[DONE]` 直通）；
> ③ **usage 返回 ✔** —— `usage_log` 落库 `status=success`、`tokens=13/19`、**`estimated=0`**（上游 usage 精确值，**未走估算**）；
> ④ `tool_calls` / `多轮上下文` **未测**（**不猜**）；`reasoning` 字段对所测模型 **n/a**。
> 留档：`docs/reviews/T21-copilot-outbound.md`。

**执行方式（T10 落地）**：`scripts/channel_matrix.py` 对**已在应用里配好的渠道**经网关跑
上表前 7 列的 6 项，产物落到 `docs/reviews/channel-matrix-<日期>.md`（主矩阵 + 逐项证据 +
网关 `usage_log` 落库证据）。

- **必须经网关打，不直连上游**：判据是"这个预设能不能标 stable / partial"，
  那是**预设 + 网关改写（R-1~R-8）之后的联合属性**，不是渠道裸接口的属性。
- 预设清单的权威载体是 `src-tauri/src/providers/presets/`；脚本通过
  `cargo run --example presets` 现场导出，**不在 Python 侧抄一份**（抄了必然漂移）。
- 脚本**只读**应用库；client key 只从命令行/环境变量取，**不打印、不落盘**。

```powershell
$env:LLMR_CLIENT_KEY = "<client key 明文>"
python scripts/channel_matrix.py --store "$env:APPDATA/llm-router/store.db"
```

**当前状态（2026-11-26）**：矩阵框架已在**本地 mock 上游**经真网关自测，6 项全绿，
产物见 `docs/reviews/channel-matrix-2026-11-26-mock.md`（该文件是**框架自测**，
不是真实渠道数据）。**真实 7 列矩阵仍待真实 API Key**——本机无任何 A 类渠道密钥，
故上表 7 列尚未回填。真实 key 到位后重跑同一条命令即可覆盖该文件。

### 10.2 安全验收（M1 必过）

```
1. sqlite3 store.db .dump | grep -iE "sk-|api_key|Bearer"   → 无明文命中
2. vault.bin 用十六进制查看 → 无明文 key 片段
3. 日志文件 grep Authorization → 全部脱敏为 ***
4. 监听 0.0.0.0 且无 client key → 启动拒绝并告警
```

**执行口径（T11 落地）**

判据 1 的原始正则**必然命中** DDL 注释与 `credential.kind` / `channel.kind` 的枚举值
（`api_key`），无法区分"字段名"与"真实密钥明文"。因此命中项**分两类**：

| 类别 | 判定 | 要求 |
|---|---|---|
| **明文凭证命中** | 命中已知密钥值（比对得出）或匹配凭证形态（`sk-…` / `llmr_…` / `Bearer …`） | **必须为 0**，否则判据不通过 |
| **结构性命中** | DDL 注释、列名、`kind` 枚举值等字段名/取值 | 逐条列出供人工核对，**不静默忽略** |

可复跑执行器（**只读**，不修改库与保险库）：

```powershell
python scripts/security_check.py `
    --store "$env:APPDATA\llm-router\store.db" `
    --vault "$env:APPDATA\llm-router\vault.bin" `
    --log <应用 stdout 日志> --log <应用 stderr 日志> `
    --secret "<渠道 key 明文>" --secret "<client key 明文>" `
    --dump-out <dump.sql> `
    --bind-check "<llm-router.exe>"
```

补充要求（均为本轮实跑确认的坑）：

- **库要扫三层**：逻辑 dump + `store.db` 主库原始字节 + **`-wal` / `-shm` 原始字节**
  （只扫主库会漏掉 WAL 里的新数据）；判据 1 要在**已实际录入过密钥**的库上跑，空库扫不出任何东西。
- **判据 3 要有机制，不只是"没泄漏"**：02 §5.2 承诺的「日志中间件全局脱敏」由
  `gateway::logging::request_log` 承担（敏感键值一律 `***`，请求体不落日志），
  挂在 `gateway::server::router()` 上。
- **判据 4 要做正反两例**：无 client key ⇒ 拒绝启动且端口不监听；有 client key ⇒ 允许改绑。
  策略是"有 client key 才允许改绑"，不是"一律禁止 0.0.0.0"。

> `AGENT.md` §9 的命令原文**未改**（按 §6-3 走提案流程）；上面的分类口径对应
> `docs/reviews/T2-vault-verification.md` §6 的提案 1 + 提案 3，正式口径仍待头头裁决。

### 10.3 里程碑验收

见交付计划文档（`03-delivery-plan.md`）。

---

## 11. 风险提示（v1 范围内）

| # | 风险 | 应对 |
|---|---|---|
| 1 | Copilot/WorkBuddy 额度反代违反平台 ToS，账号可能受限 | UI 首次启用强制确认；默认关闭；不提供规避检测能力 |
| 2 | WorkBuddy 上游协议未验证，T22 抓包可能发现签名/加密 | 预留 buffer；失败则降级为"仅保活 + 额度查看，不转发" |
| 3 | MiniMax 国内站 GroupId 口径分裂 | 双 profile + 可配置拼接；实测矩阵定稿 |
| 4 | 国产渠道 tools 兼容性参差 | §10.1 矩阵标注 partial，不假装全兼容 |
| 5 | HAR 模板随站点改版失效 | 失败即通知；模板可由用户自行编辑重录 |

---

## 12. 后续可扩展项（v2+，本期只留接口不实现）

| 项 | 预留点 |
|---|---|
| **Anthropic Messages 入站**（Claude Code） | `gateway/inbound/` 增加 `anthropic.rs`；trait `InboundHandler` 泛化；tool_use ↔ tool_calls 转换器独立成 `gateway/convert/` |
| **OpenAI Responses 入站**（Codex） | 同上 `responses.rs` |
| 海外渠道预设（GPT/Gemini/Claude 官方） | presets 机制零代码支持，随时可加 |
| 多账号轮询 / 凭证池 | `account` 表已按多账号设计；调度策略加 `round_robin` |
| 登录态切换器 / 设备指纹隔离（TraeWorkAssistant 式） | 明确**不进 roadmap**：维护成本与合规风险双高 |
| 云同步 / 多设备 | store 层抽象预留，v1 不做 |
| Gemini 协议出站 | adapter trait 已按协议无关设计 |

---

## 13. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v0.1 | 2026-11-25 | 初稿 |
| v0.2 | 2026-11-25 | 依据 FreeLLMAPI 调研（`docs/reviews/freellmapi-analysis.md`，头头批准路线 B）：P-7 X-Routed-Via 响应头；§8 限额自学习；route_group 增加 sticky_session 策略（DDL 注释同步 01 文档）；§0.2 补非目标"一键配置 harness 向导" |
| v0.3 | 2026-11-26 | 头头裁决两项：① 限流冷却期内回 **429 + `Retry-After`**（§4.4 回填，错误体字段表不变）；② 熔断状态由"只存内存、重启清零"改为**落库 + 重启恢复**（§2.3），新增 `circuit_state` 表（§9）与 UI 展示 |
| v0.4 | 2026-11-26 | 头头裁决：③ 熔断**时长可配置**（默认 60s、最小单位 1s、存 `app_setting` 表，§2.3/§9）；④ UI 增加**手动解除熔断**（`circuit_reset`，§2.3） |
| v0.5 | 2026-11-26 | 头头裁决：⑤ **429 冷却上限可配置**（默认 60s、最小 1s；冷却 = `min(max(退避, Retry-After), 上限)`，上限可缩短上游要求的等待，§2.3/§4.4/§9） |
| v0.6 | 2026-11-26 | T7 用量采集口径回填：§8.1 采集口径（一条记录 = 一次出站尝试；流式 tee / 非流式直读；估算入口；**失败尝试不估 token**）、§8.2 账号口径（API Key 型渠道用默认账号 `acct_default_<channel_id>`）与限额自学习细则（四个窗口从 `usage_log` 派生、**在途请求不计**；只收紧不放宽）、§9 `usage_log` 增 `account_id` |
| v0.7 | 2026-11-26 | T8 预设接入与自定义渠道回填：§6.1.1 预设的**默认模型建议值**（未实测，可改）、§6.1.2 **自定义 OpenAI 兼容渠道的表单字段**（base_url / 路径模板 / 鉴权样式 / GroupId / include_usage 开关 / 额外头 / 模型别名）、§9 `channel.meta_json` 增 `auth_style` 与 `extra_headers` |
| v0.8 | 2026-11-26 | T9 UI 第一轮回填：§6.5 注明 pi 实连归 T10；新增 §6.6 **应用内 UI 面板口径**（各面板职责与权威数据源 + **模型预览必须与 `GET /v1/models` 同源** + **跨面板数据变更要广播**，防止 UI 与网关漂移） |
| v0.9 | 2026-11-26 | T10 实测矩阵与 pi 实连回填：§6.5 **重写**为 pi 接入指引（安装 / 配置文件位置 / JSON 片段 / 环境变量插值 / 验证命令 / 实连结论，依据 pi 1.6.1 自带 schema 与官方文档）；§10.1 补**矩阵执行方式**（经网关打、预设名单单一权威、只读取库、client key 不落盘）与当前状态（**真实 7 列待 API Key**）；§1.2 补 `scripts/` 与 `src-tauri/examples/` 目录说明 |
| v0.10 | 2026-11-26 | T11 安全验收回填：§10.2 补**执行口径**（命中项分"明文凭证命中"与"结构性命中"两类，前者必须为 0、后者逐条列出；判据 1 要扫 dump + 主库 + WAL/SHM 三层；判据 3 要有日志脱敏中间件机制；判据 4 做正反两例）+ 可复跑执行器 `scripts/security_check.py`；§1.2 补该脚本。**§5.2 的「日志中间件全局脱敏」此前未接入**（T2 §7 记着"随 T3 接入"，T3 已过但一直没接），本轮由 `gateway::logging::request_log` 补齐 |
| v0.11 | 2026-11-26 | T12 本地估算器回填：§8.1 把「估算入口」细化为**四种情形**（上游给全 / 上游没回 / 上游缺一项→**只补缺的那一项** / 出站失败→不估），明确"缺失 = usage 对象里没有这个键，**值为 0 不算缺失**"；补**编码选择**（`cl100k_base` 默认含全部国产模型 / `o200k_base` 给 gpt-4o·gpt-4.1·gpt-5·o 系列）与**国产模型只能近似**的硬约束（估算值必须带 `estimated=1`、不参与预算告警）。实现 `gateway::estimator::TiktokenEstimator`（tiktoken-rs），产线默认由 `CharRatioEstimator` 换为 tiktoken |
| v0.12 | 2026-11-26 | 账本清理三项回填（头头 2026-11-26 裁决）：① **任务编号交叉引用对齐**——以 03 任务卡为唯一真值源，改 §6.1.1「T9/T10→T10」、§6.2「T17→T21」、§6.3 三处与 §11 风险表「T18a→T22」，并修 §6.5 指向不存在文件的坏引用（`T10-pi-integration.md` → `T10-channel-matrix-and-pi.md`）；② **§4.4 统一错误结构补 `available_models`**——写明该键只在**别名未命中**（`code = "model_not_found"`）时出现、缺失时**整个键不出现**（不出现 `null`），别名命中但无可用候选的路径**不带**该字段；③ **T18 签到站点清单已指定**（ModelScope + MiniMax Code，见 03 T18 卡片） |
| v0.13 | 2026-11-26 | T13 价目表与成本换算回填：**新增 §8.3 成本换算口径**——`price` 表的匹配规则（`*` 通配、命中多条取最具体）、**三价换算公式**（可计费输入 = `prompt_tokens - cache_read_tokens`；缓存命中**只按缓存价计一次**；缓存写按输入价；**总额**四舍五入一次）、**空值语义**（单价 `NULL` ≡ 0 = 该档不计费；**没命中价目 → `cost_micros` 为 NULL（未知），与"免费 0"必须可区分**）、估算行（`estimated=1`）照样算成本但**不参与预算告警**、**改价/删价后该 provider 的历史行一并重算**；并给出**预填牌价快照（2026-11-26）的六家官方来源与取价规则**（取主列表价）与已知限制（多档定价表达不了、`minimax_intl` 按 USD 计价不折算、假定 `prompt_tokens` 含缓存命中 token 的 OpenAI 语义）。§8 的成本条目与 §8.1 的 `cost_micros` 字段来源同步指向 §8.3 |
| v0.14 | 2026-11-26 | T14 聚合与图表回填：**新增 §8.4 聚合与图表口径**——把 §8 的「按 日/渠道/模型/client_key 四维；TTFT P50/P95；输出 tokens/s」细化为可实现的定义：四维的**分桶键与可读名**（渠道名 / client key 标签查不到时回退为 id）、**时间范围**（`since` 含 / `until` 不含，默认近 30 天）、**每个桶的 13 项指标**（请求数 / 三态计数 / 五类 token / 总 token / 已知成本合计 / **成本未知行数** / 估算行数 / TTFT P50·P95 / 输出速度）、**分位数用最近秩法**、**输出速度的单行与桶级公式**（耗时为 0 → `NULL`，不是 0 也不是无穷大）、估算行照常计入但 UI 须标注、**最近请求明细**（M2 判据的落地载体）。并明确**已知成本合计为 0 ≠ 不花钱**（未配牌价的行单独计数，延续 §8.3 的 `0`/`NULL` 语义） |
| v0.15 | 2026-11-27 | T15 HAR 导入与模板化回填：**新增 §7.1.1 HAR 1.2 子集与模板结构**（只取 `request` 的 method/url/headers/queryString/postData 与 `response.status`；**响应体与响应头一律丢弃**——实测签到响应的 `Set-Cookie` 带 csrf 密文；模板落 `checkin_site.config_json`，占位符语法 `{{name}}`，替换发生在"值内部"（`header` / `header_part` / `query` / `body` 四种 target），导入模板 `enabled = 0`）与 **§7.1.2 变量标记与密文边界**（候选自动识别清单；**模板零密文**，变量真实取值进保险库 `credential`（`kind = 'cookie' | 'access_token'`）；**归属渠道**——因 `credential.channel_id` 为 NOT NULL 外键而站点无渠道，按头头 2026-11-27 裁决取方案 A：站点归属一个渠道（选已有或按站点名新建），与 §7.2 积分回写 `account.quota_json` 同源；落库前自检模板不得残留被标记字段原值；IPC 只传掩码值，保存时回传路径重新解析）；§9 补 `credential.kind` 的签到取值 |
| v0.16 | 2026-11-27 | T16 重放引擎回填：**新增 §7.1.3 重放引擎口径**——① **变量注入**（敏感变量按 `credential_id` 从保险库解密、非敏感变量用 `default`；**缺凭据不发请求**直接分类 `credential_missing`；替换发生在值内部；**未知占位符拒绝重放**→`template_invalid`）；② **请求构造**（**query 以模板 `request.query` 数组为准重建**——HAR 的 url 自带 query 与 `queryString` 重复，先剥 URL 再重建，否则参数发两遍；**剔除 `Host` 与 `Content-Length`**——由客户端按实际 URL/请求体生成，留着录制旧值会把请求打歪；连接 10s / 整请求 30s，**引擎自己不重试**，重试与冷却属 T17）；③ **响应断言**（`assert.status` 是**期望状态码**而非录制值，导入时先填录制值占位；新增可选 `assert.jsonPaths`（点号路径 + 数组下标 + `equals`），状态码与 JSON 路径是"与"关系；**旧模板兼容**——新字段一律可选）；④ **错误分类**（`success` / `assert_failed` / `network_error` / `credential_missing` / `template_invalid` / `credential_invalid`(401·403) / `rate_limited`(429) / `http_error`，「可重试」沿用 §3.3 判据；**不猜验证码**——不内置验证码识别，验证码表现为断言不符，归 `assert_failed`/`http_error`）；⑤ **结果落库**（每次重放写一行 `checkin_result`，`ok=1` 仅当 `success`；**响应体不进库**——只在内存里留 512 字节截断片段，防 csrf/token 密文落库；`reward_json` 留空，积分回写属 T18/T17）；⑥ **边界**（T16 只做引擎：无调度 / 无计划任务 / 无通知 / 无浏览器辅轨 / 无 UI 触发入口，「立即执行一次」按钮留 T17·T18） |
| v0.17 | 2026-11-27 | T17 调度器与系统兜底回填：**新增 §7.2.1 调度与补签口径**——① **调度模型**（每分钟 tick 一次；`cron` crate 解析 5 字段；**本地时区** `chrono::Local` 解释 cron 表达式，不是 UTC；`schedule` 空 = 不参与调度，`enabled = 0` 不跑）；② **应跑判定（去重）**——`due_at` = 最近一次应触发时刻（≤ now，由 cron 的下一次触发倒推紧邻的前一次）；`last_attempt` = 该站点最近一次尝试的 `checkin_result.ts`；**`last_attempt >= due_at` → 本周期已跑过、跳过**（落库去重 ⇒ **跨重启也成立**）；③ **错过补签**——应用启动与系统计划任务唤起时做**同一套判定**，关应用期间漏掉的周期自动补上；**不因失败立刻重试**（同周期只跑一次），要立刻再试用「立即执行一次」；④ **系统兜底**——Windows 每个启用站点注册**一个**计划任务（`LLM Router Checkin - <site_id>`，动作 `"<exe>" --checkin-once --site <site_id>`；cron → `/sc hourly`/`minute /mo N`/`daily`/`weekly` 映射，**其余表达式退回每天一次**，宁可多跑不可漏跑）、macOS launchd plist（`StartCalendarInterval` / `StartInterval`）；**注册时机** = 启用或改 cron 时注册、停用或删除时注销；**CLI 入口** `--checkin-once [--site <id>]`（**不起 UI、不起网关**，跑完退出）；⑤ **边界**（不含通知 T19 / 积分回写 T18 / 浏览器辅轨 / 云同步；**调度层不重试**） |
| v0.18 | 2026-11-27 | T17 真机验收后的口径修订（**§7.2.1 §4 同步改写**）：① **任务名以连字符为准**（`LLM Router Checkin - <site_id>`，与代码 `system::TASK_PREFIX` 一致；v0.17 行里的 `·` 是笔误，已一并订正）；② **频率映射补「分钟字段 `*` → `/sc minute /mo 1`」**——真机实测发现 `* * * * *` 曾落进"未识别 → 每天一次"，会把"每分钟"降级成"每天"→ **系统兜底漏跑**，已修并加回归单测；③ **「任务不存在」= 已注销**（注销走**先查后删**，不靠错误文案匹配）——`schtasks` 的报错文案随系统语言变（英文 "ERROR: The system cannot find the file specified." / 中文 "错误: 系统找不到指定的文件。"），按字符串匹配必然漏判；④ **系统命令输出按系统 OEM 代码页解码**（无控制台的 GUI 进程里 `schtasks` 按 OEM 输出，中文 Windows = GBK/936；直接按 UTF-8 解出乱码），解码顺序 UTF-8 → UTF-16LE → OEM → 有损兜底；⑤ **补齐 T17 的 UI 入口**（T16 留档里"立即执行一次"明确记在 T17/T18）：签到面板新增**调度区**——cron 输入 / 启用开关 / 「立即执行」/「同步系统兜底任务」/ 最近执行记录表（`src/checkin/schedule.ts` 为纯函数模块，`node --test` 可跑） |
| v0.19 | 2026-11-27 | T18 站点插件回填：**新增 §7.4 站点插件口径**——① **插件机制**（`checkin_site.plugin` 取值 = `har`（用户自建，走 T16 重放引擎）/ `modelscope` / `minimax_code`（内置）；内置插件与 HAR 模板**共用 02 §7.1.3 §4 的 8 类 `ReplayClass` 结果语义**；**内置插件带正确断言**——解掉 T16/T17 留档那条「导入模板带录制时的状态码、用户得自己改」的限制）；② **ModelScope 插件**（`GET /api/v1/users/login/info` 签到 + `GET /openapi/v1/magicubes/transactions` 对账；网页会话 Cookie 进保险库；**断言必须走 JSON 路径**——**已实测匿名态两个接口都返回 HTTP 401**（`{"Success":false,"Message":"禁止访问"}` 与 `{"code":"InvalidAuthentication"}`），只看状态码会把"未登录"误判成成功；魔粒字段路径**未实测**，内置默认路径 + 允许 `config_json` 覆盖）；③ **MiniMax Code 插件走辅轨**（签到接口 `GET/POST /minimax-cloud/api/v1/signin/status|claim` 带 **OAuth bearer + `x-timestamp` + `x-signature`**——签名每次都变 → **HAR 模板化不可行**，自己实现签名 = 逆向客户端私有协议、撞 AGENT.md §5.7 → **让浏览器自己算签名**；辅轨 = `sidecar/` Node + Playwright + **持久化 profile**，优先用**系统 Chrome**（`channel: "chrome"`）、不强制下载 150MB；Rust ↔ sidecar 走 **CLI 协议**（每次起一个 Node 进程跑一个动作 `health` / `login` / `checkin`，**stdout 吐 JSON**，用完即退——无端口占用、无孤儿进程、无本地 socket 鉴权问题）；**结果以页面自己的网络响应为准**，不解析 DOM 里的数字；`AlreadyClaimed`/`claim_result=2` 判「今天已领」而非失败；**profile 的凭证由浏览器自己加密保管，应用不读不导不入库不进 IPC**——AGENT.md §5.3 约束的是我们的存储路径）；④ **积分回写 `account.quota_json`**（`{"points","unit","source","updated_at"}` 快照；**归属账号** = 站点占位渠道下的默认账号 `acct_default_<channel_id>`（与 §7.1.2「站点归属渠道」同源）；**合并写**——只改这四个键，不覆盖 §8.2 的限额字段；**写回失败不回滚签到**）；⑤ **边界与未验证项**（通知属 T19；辅轨依赖 Node 运行时，`sidecar/` 需 `npm install`，**「开箱即用」的打包分发不在本卡**；登录态相关的三项待实测） |
| v0.20 | 2026-11-27 | T18 **真机验收后的口径修订**：① **§7.4 §3 补「登录态判定信号」**（真机实测：匿名态 `agent.minimax.cn` 页面有**两个**「登录」按钮 → **按钮还在 = 没登录**；**必须等页面渲染完再判**——前端渲染站 `DOMContentLoaded` 时按钮还没挂上，会把「按钮数 == 0」**误判成已登录**（真机实测 `login` 动作秒回「登录完成」）；判据用 `waitForSelector("button")` 而**不是 `networkidle`**——该站有持续请求，`networkidle` 实测要等 20 s 才超时；登录成功要**连续两次读到 0** 且**至少停留 5 s**）；② **§7.4 §5 补真机已验部分与新边界**（**失败路径已在真实站点跑通**：ModelScope 用错误 Cookie → **HTTP 401** → 判 `credential_invalid`（152 ms）；MiniMax Code 未登录 → 辅轨开真实浏览器读页面 → 判 `not_logged_in`（2.1 s）——即「**能打到真站点 + 断言与分类正确**」已验证，「**成功路径**」待登录态；**删站点不清内置站点的凭据**——`checkin_delete_site` 只清 HAR 模板的凭据 → 已记 backlog）；③ **§7.4 §4 的归属与合并语义已在 v0.19 行补齐**（占位渠道 → 默认账号 `acct_default_<channel_id>`；`quota_json` 只改 `points`/`unit`/`source`/`updated_at` 四个键）；④ **真机揪出并修掉 4 个 bug**（辅轨同名变量 TDZ / 插件没覆盖 trait 的 `default_schedule` 导致 MiniMax 默认 cron 错 / `login` 动作的渲染前假阳性 / `checkin` 动作的渲染前误判登录态——后两个的修法即 ①） |
| v0.21 | 2026-11-27 | T18 **登录交接真机修订**：① **§7.4 §3 补「登录流程由辅轨铺路」**——站点**短信/验证码登录有频控**（真机实测点「获取验证码」报**「操作频繁」**），**账号密码登录没有该频控**；辅轨 `login` 动作改为**自动打开登录弹窗 + 自动切到「账号密码登录」tab**（**只铺路**：不代填密码、**不绕人机验证**，撞 AGENT.md §5.7）；② **§7.4 §3 把「登录态判定信号」收敛成三条独立判据**——**A 硬信号**（页面自己的 API 请求里 `user_id` 从 0 变真实 id）、**B**（整个浏览器上下文里「登录」按钮都消失，**连续两次 + 至少停留 5 s**）、**C**（出现签到入口）；**B 必须看整个上下文**——登录可能开新标签页，只看首屏会把「原页按钮还在」误当没登录；③ **§7.4 §3 超时口径**：`login` 5min → **15min**（真人要输密码 + 过人机验证），可用 `LLMR_LOGIN_TIMEOUT_MS` 覆盖；④ **§7.4 §3 补「限流要能识别、不能让人白等」**——等待期间扫「操作频繁」类文案，命中即返回并说明原因；超时且登录框仍开着时，回执明说「密码没提交 / 验证码没过 / 仍在限流」；⑤ **§7.4 §5 补两条边界**（登录交接的频控约束 + **探测脚本不许用临时 profile 反复打线上站点**——真机踩过：直接把短信通道打限流） |
| v0.22 | 2026-11-27 | T18 **MiniMax Code 成功路径真机回填**（辅轨 + 登录交接实测）：① **§7.4 §3 补「签到入口的真实位置」**——**入口不在首页**（首页无任何「签到」文案），在**用户头像下拉菜单**里的「每日签到」项；头像触发器是 `.ant-dropdown-trigger`，**页面上有两个**（另一个是「添加附件或技能」、无文字）→ **只认有文字的那个**；② **§7.4 §3 补 `/signin/status` 的真实响应形状**——`data.days[]` 每条含 `day_no` / `points` / `status` / `is_today` / `bonus_points`，**`status == 3` = 已领取**，**今天那一条 = `is_today == true`**；**口径与 03 任务卡对上了：每日 400、第 4/7 天各 1000**（实测值，非推测），实测当天为**连签第 7 天**；业务成功看 **`base_resp.status_code == 0`**（MiniMax 约定，HTTP 200 可能是业务失败）；③ **§7.4 §3 补「今天已领 = 成功（幂等）」**——DOM 信号（面板出现「今日已签到」）优先、接口信号（今天那条 `status == 3`）兜底，命中即 `ok = true` + `alreadyClaimed = true`；**已领过时报 `points = null`**（本次没拿到 → 不回写额度池，避免重复计数），当日积分与连签天数写进回执文案；④ **§7.4 §5 收敛未验证项**——签到入口 DOM、每日积分与连签天数**已实测收敛**；剩余未验证 = ModelScope 真签到与魔粒字段路径（**仍缺网页会话 Cookie**）+ MiniMax Code `/signin/claim` 真正领取的响应体（**真机当天已领，没触发过 claim**）；⑤ 辅轨 `checkin` 动作改为**先开签到面板再判状态**（原来直接找首页上的签到文案，必然找不到） |
| v0.23 | 2026-11-27 | T18 **登录态口径改用「访问令牌」（不再手抄 Cookie）**：① **§7.4 §2 改写凭据口径**——魔搭的**开放平台访问令牌**（`ms-…`，个人中心 → 访问令牌 → 点一下复制）**可直接当网页会话用**（`Cookie: m_session_id=<令牌>` → `GET /api/v1/users/login/info` 即触发当日签到），且**同一个令牌就是 `/openapi/v1/*` 的 Bearer**（查魔粒）→ **一个令牌，签到 + 魔粒全包**；② **判别规则**（纯函数 + 单测）：密文**含 `=`** → 原始 Cookie 头（老路保留）；**不含 `=`** → 访问令牌；③ **记下两条走不通的路（都不内置）**——「让应用开浏览器自动登录」与「浏览器接管会话」：魔搭登录页的**人机验证专门拦自动化浏览器**（真机实测：Chrome 顶部挂着「自动测试软件正在控制 Chrome」，滑块报 `验证失败（error:22phmt）`、滑块根本没渲染），而**反检测 / 绕过验证码是 §5.7 红线**；**浏览器接管那套实现已在同轮全部撤销**（辅轨 `session` 动作、`run_secret`、IPC、前端按钮均已删，不留半成品）；④ **奖励口径**：魔粒改从 `/openapi/v1/magicubes/transactions` 算（**不用余额接口**——余额是总额，与「本次拿到多少」语义不符）；⑤ **密文边界不变**：令牌**只进保险库**，不回显、不进日志、不进 IPC 明文；⑥ **手抄 Cookie 那条路保留**（备选 / 排障），但不再是推荐路径 |
| v0.24 | 2026-11-27 | T18 **ModelScope 真签到 + 魔粒字段形状实测收敛**：① **真签到成功**——访问令牌 → `Cookie: m_session_id=<令牌>` → `GET /api/v1/users/login/info` **HTTP 200**、`Success=true`、`Message="success"`，**277 ms**，**落库 `ok=1`**（走应用内 `scheduler::run_now`，与 UI「立即执行」同一条路径）；② **魔粒流水真实形状收敛**——`data.records[]`，金额字段 = **`total_amount`**（不是 `amount` / `points`）、日期字段 = **`gmt_created`**（**只有日期、没有时分秒**）、类型 = `type=EARN` + 规则 = `rule_key`；当日实测三条：`daily_active` **200** + `infocomplement_profile_experience` 50 + `infocomplement_email_verification` 50 → **只有 `daily_active` 是「每日签到」那一笔**（另两条是一次性资料奖励，不算进本次签到）；③ **奖励口径改为「签到前后差值」**——签到前取当日 `daily_active` 总额作基线、签到后再取一次，**差值 > 0 才是本次到账**；差值 = 0 → 判**「今天已领过」**（`points = null`、**不回写额度池**，与 §3 MiniMax 口径一致）——因为当日那条流水**整天都在**，直接取当日总额会让**同一天的第 2、3 次运行**被当成新到账、**额度池重复计数**；④ **网页会话 Cookie 也能开 `/openapi/v1/*`**（与 Bearer 回同一份流水）；⑤ **签到接口的响应体只是用户资料**（含 `GitlabAccessToken` 等敏感字段）→ 插件**不存响应体**（`body_preview` 留空），站点也**不提供「今日魔粒 / 已领」信号**；⑥ **§5 未验证项收敛**：ModelScope 只剩「差值 > 0 分支待次日首次运行」；⑦ **环境提示**：本机 `SetCursorPos` 被环境屏蔽（光标坐标钉死）→ UI 自动化点不动按钮，改走 `scheduler::run_now` 做真机验证 |
| v0.25 | 2026-11-27 | T18/M3 **通知卡（T19）**：① **新增 §7.5 通知口径**——落点 = `scheduler::run_site` 之后（**结果已落库之后**），一处覆盖**四条执行路径**（应用 tick / UI 立即执行 / CLI `--checkin-once` / **Windows 计划任务（应用关闭时）**）；**成功与失败都通知**（失败永远通知 = §5.7「撞到验证码 = 失败并通知」落地，成功可关）；文案**必含站点名与原因**，零敏感信息（不带凭据/响应体）；② **系统通知用 `notify-rust`**——**刻意不用 Tauri 通知插件**：CLI / 计划任务路径**没有 Tauri app handle**，用插件就只能在应用开着时通知，**覆盖不了「应用关闭时」**这条 M3 判据；③ **Webhook 三种格式**（企微群机器人 / 飞书群机器人 / Telegram Bot）共用一个配置，Telegram 另需 `chat_id`；④ **配置落点**：`app_setting` 只存**非密文**项（总开关 / 成功也通知 / 类型 / **凭据 id** / chat_id），**webhook 地址与 bot token 进保险库**（`kind='webhook'`，挂占位渠道「通知」——`credential.channel_id` 是 NOT NULL 外键），**零明文**：不落设置表、不进日志、不回显 IPC；⑤ **通知失败不影响签到结果**（只记日志、不重试、不回显 URL）；⑥ **边界**：不含邮件/短信/钉钉/通知历史/模板自定义；系统通知依赖 OS 通知权限，Telegram 在网络受限区域不可达 |
| v0.26 | 2026-11-27 | T19 **系统通知「静默丢弃」的根因与如实报告**（头头反馈「屏幕上没有」后查出来的）：① **新增 §7.5 §6**——Windows 的 toast 有**两层**独立的静默拦截，任一层挡住都**静默丢弃**（`notify-rust` 照样返回成功、日志写着「已发送」、屏幕上什么都没有）：**① 调用进程没有对应 AUMID**（Windows 10 1607+ 要求进程拥有 `System.AppUserModel.ID`）→ 自建 AUMID + COM 注册开始菜单快捷方式；**② 全局通知总开关关着**（`HKCU\…\CurrentVersion\PushNotifications\ToastEnabled = 0`）→ **所有**应用的 toast 都被拦；② **根因定位方法（别再走错路）**：事件日志 `Microsoft-Windows-PushNotification-Platform/Operational` 的 `Id=3150 … PolicyReason [GlobalSettingDisabled]` 是决定性证据；**别的应用同样中招** → 所以「连 PowerShell 原生 toast 也不显示」是**同一个全局开关**，**不是** AUMID 问题；③ **两个坑**：改完注册表**必须重启 `WpnUserService_<sid>`**（或注销重登）才生效——通知平台服务把策略**缓存**在内存里（连改两次注册表、当场重发都还是被拦）；④ **投递成功的判据**（比「API 返回成功」可靠）：事件日志出现 `Id=3153 Toast … is delivered to <AUMID> on session N`，且通知库 `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db` 的 `Notification` 表里有对应 payload（**被丢弃的 toast 从不入库**）；⑤ **代码侧补「如实报告」**：发送前读全局开关（`system_notify::global_toasts_enabled` + 纯函数 `global_block_reason`），关着就报「Windows 全局通知已关闭（设置 → 系统 → 通知）——请打开后再试」，**不再谎报「已发送」** |
| v0.27 | 2026-11-27 | M4 **T20 Copilot OAuth（卡片补全 + 协议细节）**：① **新增 §6.2.1**——把 §6.2 的四步落成**可实现的协议细节**：client_id `Iv1.b507a08c87ecfe98`（VS Code 的**公开** client_id，**不是密钥**、用户无须填）、scope `read:user`、三个端点、轮询错误码状态机（`authorization_pending` / `slow_down`(+5s) / `expired_token` / `access_denied` / 成功）、Copilot token 响应形状 `{token, expires_at(float), endpoints}`（**`endpoints.api` 以它为准**）；② **存储口径**：GitHub token（长期）→ 保险库 `kind='oauth_gh_token'`；Copilot token（~25~30min）→ 保险库 `kind='oauth_copilot_token'` + `expires_at`（§9 的列）+ 进程内缓存，**过期前 5 min 自动重取**；③ **风险声明**：`/copilot_internal/v2/token` 是**社区观察到的内部端点**，GitHub 未公开承诺稳定 → 可能随时变更；④ **边界**：本卡**只取凭证**——出站转发与 R-6 伪装头属 **T21**，**ToS 确认弹窗与启用开关属 T24**（渠道**默认关闭** `enabled=0`，AGENT.md §5.5）；⑤ **合规**：走 GitHub **官方** device flow，**不做**指纹伪造/验证码绕过/签名伪造（§5.7） |
| v0.28 | 2026-11-27 | M4 **T20 网络层容错（判据⑤ 真机授权受阻后的根因与修法）**：① **新增 §6.2.1「网络层容错」**——本机到 `github.com` **时好时坏**，且**失败模式因请求方法而异**：`GET /login/device` 能通的 IP，`POST /login/oauth/access_token` 可能 0/N（实测 `20.205.243.166` POST 3/3，`20.200.245.247` / `20.26.156.215` / `140.82.112.3` / `140.82.114.4` 虽 GET 3~5/5 但 **POST 0/3**）；② **两条已证伪的绕法**（别再走）：**`api.github.com` 的 IP 配 SNI=`github.com`** → 5/5 秒回但 **HTTP 400**（后端不认）；**动态 DNS 发现候选 IP** → 系统 DNS 连续 16 次解析**全是同一个 A 记录**（ISP 侧有缓存，不轮询）；③ **修法口径**：**候选 IP 池 + 每次重试轮换**（钉死单个 IP 会把 900 s 设备码寿命全耗在一条坏路径上——实测钉 `20.27.177.113` 后连续 40 次轮询全超时、设备码到期作废）+ **池尾保留一个走普通 DNS 的 client 兜底** + **超时收紧到 connect 3 s / 总 8 s**（GitHub OAuth 正常 < 2 s）+ **只重试网络层错误**、HTTP 状态码交给轮询状态机 + **环境变量 `LLMR_GITHUB_IPS`（逗号分隔）可覆盖内置池**（边缘 IP 会变）；④ **不改协议面**：端点 / client_id / scope / 状态机 / 存储口径 / 边界**全部不动**，只动传输层容错 |
| v0.29 | 2026-11-27 | M4 **T21 Copilot 出站适配（卡片补全 + R-6 取值固化）**：① **新增 §6.2.2 出站适配口径**——**R-6 四项固定头 + 一项动态头**（`copilot-integration-id: vscode-chat` / `editor-version: vscode/1.104.1` / `editor-plugin-version: copilot-chat/0.26.7` / `user-agent: GitHubCopilotChat/0.26.7` / `x-request-id: <UUID v4，每次请求不同>`），取值与来源逐条列出；**取值分歧已裁决**（`editor-version`：liter-llm `vscode/1.95.0` / npm `vscode/1.104.1` / ericc-ch 动态探测兜底 `1.104.3` / copilot-to-api 按客户端写 `Neovim/0.6.1` → **头是按客户端变的**；本卡取 npm 那个，**且是常量**——版本串变了改常量即可，不用动协议）；② **明确不发的头**（边界，不是遗漏）：**`machine-id`**（**设备指纹** → 撞 §5.7；liter-llm 与 npm 参考实现都不发）、`openai-intent` / `openai-organization` / `x-github-api-version` / `x-vscode-user-agent-library-version`（R-6 只认 §4.1 明列的四项，§5.6 改写器不是转换器）；③ **出站 URL 与鉴权**：`POST {endpoints.api}/chat/completions`（**不带 `/v1`**）+ `Authorization: Bearer <Copilot token>`（**不是 GitHub token**）+ R-4 注入 `stream_options.include_usage`；④ **凭证供给口径（本卡最关键的一条）**：**OAuth 型渠道的凭证由 adapter 自己供给，网关不解密**——GitHub token（`ghu_`，长期）与 Copilot token（~30 min）**不能互换**，沿用网关「按 `credential.kind` 选一条」的旧口径会在 token 过期时**误取 GitHub token** 当 Bearer → 必然 401；`CopilotAdapter::complete()` 出站前调 `ensure_token()`（过期前 300 s 自动重取）；`refresh()` 因**拿不到渠道上下文**保持 no-op 并写明理由；注入方式 = `AdapterDeps`（vault + Copilot 端点 + token 缓存）→ 网关 `Forwarder` 持有 → **生产真实端点 + 全局缓存 / 测试本地 mock 端点 + 独立缓存**（不出网、并行用例互不污染）；⑤ **模型目录口径**：`GET {endpoints.api}/models` → `{"data":[…]}`（字段 `id` / `name` / `policy.state` / `supported_endpoints` / `capabilities.*`）→ **别名 = `id`**、**别名被别的渠道占用则跳过**、**目录里没有的旧模型置为禁用**（不物理删除，避免连带删掉 `route_member` 编排）；**只收录 `policy.state != disabled` 且 `supported_endpoints` 含 `chat` 的模型**（codex 类只支持 `/responses` → v1 **不做协议转换**）；⑥ **边界**：**ToS 弹窗与启用开关属 T24**（本卡**不提供**启用入口）、**不实现 `/responses` 与 `/messages`**、`machine-id` 不实现 |
| v0.30 | 2026-11-27 | M4 **T21 真机实测修订（模型目录口径按真实响应订正）**：① **§6.2.2「模型目录口径」按真机响应重写**——真实上游的 `supported_endpoints` 给的是**路径**（`/chat/completions`、`/responses`、`/responses/ws:`、`/v1/decisions`），**不是**社区 schema 里的短名（`chat`）→ v0.29 写的「含 `chat` 才收录」在真实数据上**等于没过滤**，实测把 `text-embedding-3-small` 等 3 个 **embeddings** 模型与一批只支持 `/responses` 的模型一起收了进来；② **收录规则补第二条**：`capabilities.type` **存在且不是 `chat`** → 不收录（真实取值有 `chat` / `embeddings` / `decisions`）；③ **端点判定两种形状都认**（路径含 `chat/completions` **或**短名等于 `chat`）——路径与短名各留一条单测；④ **实测数据**：账号 `api.individual.githubcopilot.com` 的 `/models` 原始 **39 条 → 收录 19 条**（`policy.state=disabled` 8 条、`capabilities.type != chat` 3 条、只支持 `/responses` 9 条被排除）；⑤ **真机四证**：R-6 四头 + `x-request-id` 经真实上游**实测通过**（上游正常回话，无头缺失/鉴权错误）、**经真网关流式对话成功**（`gpt-4o-mini-2024-07-18`，6~12 帧真实内容）、`usage_log` 落库 `status=success`（`tokens=13/22`、`estimated=0`、耗时 2999~3575 ms）、**四层零明文**（官方执行器明文命中 0 条 + `ghu_`/`eyJ`/`Bearer` 形态扫描 0 命中）；⑥ **订正后的落库语义**经真机验证：上轮误收的 3 个 embedding 模型在重新同步时**被置为禁用而非物理删除**（`model` 表 22 行 / 启用 19 行）；⑦ **新增落库口径**：`supported_endpoints` 缺失的老模型（`gpt-4o` 家族）**按 chat 收**（`capabilities.type=chat`，真机实测能对话）；⑧ **不改协议面**：R-6 取值 / 出站 URL / 凭证供给口径 / 边界**全部不动**，只修收录规则 |
| v0.31 | 2026-11-28 | M4 **T22 WorkBuddy 协议确认（卡片补全 + 出站口径实测）**：① **方法调整**——WorkBuddy 是 **Electron 桌面端**（本机 `D:\IDE\WorkBuddy`，`version` = `37.10.3-43`，`app.asar` 182 MB / 9894 文件），**发行版代码即协议真值源** → 改为「**发行版静态取证为主 + 可选 CDP 复核**」（`~/.workbuddy/app/session/DevToolsActivePort` 实测存在，端口 `9222`），**不打扰用户桌面端**；② **新增 §6.3.1「WorkBuddy 协议记录（T22 实测）」**（上游 host / 端点 / 鉴权头清单 / 指纹与签名结论 / 登录态字段 / 可实现性判定 / 未验证项，**全部只记字段名不记值**）；③ **协议侧结论：chat 请求无动态签名、无设备指纹绑定**——`device-id` 不上请求、`X-Machine-Id` 只用于 `/v2/feedback`、**qimei36 是注入 sidecar 的环境变量不是 HTTP 头**、`X-Device-Token` 只用于签到、`x-workbuddy-mcp-context`（HMAC-SHA256）是本地代理会话信封且转发前被剥离 → **不触发 03 §5 降级触发条件**（「抓包显示请求含动态签名」**实测不成立**），且**无需实现任何指纹伪造**（§5.7 无冲突面）；④ **出站口径**：产品域名 `https://copilot.tencent.com`，chat 端点 `POST /v2/chat/completions`，鉴权 `Authorization: Bearer <accessToken>`（**最后写入、覆盖入站值**）+ `X-User-Id` + 产品头 + 会话头 + 链路头；**转发前剥离** `authorization` / `x-workbuddy-mcp-context` / `origin` / `host` 等；**401 → `refreshSession()` → 重放一次**（与 Copilot 同构）；⑤ **登录态实测推翻旧口径**：登录态**不在** `~/.workbuddy/app/users/<hash>/`（本机实测**为空**），真实位置 = `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\workbuddy-desktop.info`（**普通 JSON + 字段级 AES-256-GCM**，敏感字段为 `{"$wbEncrypted":1,"envelope":…}`）；加密链 = 构建期 `atRestSecretKey` → `sha256` 得 protector key（`9127dea1b44020a7`）→ 解 `~/.workbuddy/keyblob` 得 master key（`fca8d6b22ed65a97`）→ 解字段信封；**构建期密钥不在磁盘、不在 asar**，由**定制 Electron** 的 `electron.workbuddyStorage.loggerGet()` 在运行时提供（代码里**无 DPAPI** 调用）；⑥ **可实现性判定**：**协议侧可转发、凭证侧是卡点** → T23 的凭证获取方式（用户显式提供 / 逆向定制运行时 / 降级档「保活+额度+模型目录」）**列为提案待头头拍板**，本卡**不自行决定**、**不擅自改写 §6.3 既有条款**（P1~P4 四条修订建议见 §6.3「设计变更提案」）；⑦ **模型目录来源**：免鉴权普通文件 `…\app.asar.unpacked\cli\product.cloudhosted.json`（22 个模型，字段 `id`/`name`/`credits`/`vendor`/`supportsToolCall`/`supportsImages`/`maxInputTokens`/`maxOutputTokens`/`supportsReasoning`/`tags`/`relatedModels`）；⑧ **未验证**：请求体/响应体 JSON schema（daemon **字节级透传** body，由 CLI 运行时构造 → 与「v1 入站仅 OpenAI chat」的兼容性未实测）、**真实 chat 端到端**（WorkBuddy 未运行，本轮无活体抓包）、`/v2/user/cloudagent/quota` 响应形状 |
| v0.32 | 2026-11-28 | M4 **T22 付费前提与账号实况（头头口述 + 本机补证）**：① **新增 §6.3.1 §5.1「付费前提」**——头头口径：**没有付费之前，无法在 WorkBuddy 建立 token**；② **本机补证（只读、未解密任何信封）**：账号 `type="personal"` / `loginChannels=["openplatform"]`；会话**活的**（`lastRefreshTime` = 2026-11-28 09:49:26，本次核查前 1 分钟；`expiresAt` 2026-12-26 / `refreshExpiresAt` 2026-12-28；`tokenType="Bearer"`；`domain="copilot.tencent.com"`）；`~/.workbuddy/workbuddy.db` → `session_usage` **46 行**带 `credit_json`（各模型 credits 消耗）→ **额度确实在被消耗**；③ **付费结构**：订阅包字段 `PackageCode`/`PackageName`/`DeductionEndTime`/`CycleEndTime`/`AutoRenewFlag` + 额度字段 `usageTotal`/`usageUsed`/`usageLeft`；档位映射 `free`/`freeMon`/`freeMonIntl`/`gift` → `isPro=false,isTria=false`，`proTrialMon`/`proTrialYear` → `isTria=true`，`proMon`/`proMonPlus`/`proYear` → `isPro=true`（另有 `youth`/`advanced`/`flagship`/`buddyOneVip`）；客户端开关 `subscription-gate@workbuddy-builtin=true`、`weixinpay@workbuddy-builtin=true`，`productFeatures.DisableCreditsEntry=true`；④ **两条硬事实（发行版代码，不是推测）**：**WorkBuddy 没有面向第三方的官方 API Key**——asar 的 `/v2/api-keys` 只服务于 `connect_cloud_service` 工具（请求体 `{name:"workbuddy-connect-cloud-service", expire_in_seconds:900, temporary:true}` → 返回 `{code:0,data:{key:"ck_t_…",expires_at}}`，**15 分钟临时 key**，供 `miora_*` 多模态生成与 `neodata-financial-search` 等云服务 skill 用），**不能**当反代凭证；**唯一长期凭证就是会话 `accessToken`**（被 at-rest 加密 → 取它只剩 P4-b 逆向一条路）；⑤ **排期结论**：付费前 **P4-a 与 P4-b 都不产生可交付物**（逆向出来的是**自己账号额度**下的会话 token，无官方授权通道、逼近 §5.7 边界、且把个人额度分给 harness **违反平台 ToS**（§11 风险 1））→ **M4 判据②「WorkBuddy 登录态可作为上游完成对话」在付费前不可达** → **T23 延后到付费后**，付费后**第一件事是查有没有官方 API Key / 开发者通道**（发行版另有 `open.workbuddy.cn` / `api.skillhub.cn` / `open-test.workbuddy.cn` 等域，**待查**），**而不是默认走逆向**；付费前能做的只有**免鉴权**两件（`GET /v3/config` 保活 + `product.cloudhosted.json` 模型目录 22 模型）→ 对应 03 §5 降级档的「保活 + 模型目录」，**「额度查看」需鉴权 → 付费前也不可达**；⑥ **T24（ToS 弹窗 + 启用开关）可只做 Copilot 那半边**，WorkBuddy 的开关等有合规凭证再做 → M4 判据③ 对 Copilot 先闭环 |
| v0.33 | 2026-11-28 | **T26 UI 功能分组（头头 2026-11-28 拍板）**：**新增 §6.7「应用内 UI 功能分组与导航」**——① **问题**：8 个面板在 `src/App.tsx` 单页平铺，首屏被三张说明卡片占掉，找功能靠纵向滚动；② **分组（4 组）**：**接入**（渠道 · 模型目录 · Copilot 反代）｜**凭证**（加密保险库 · Client Key）｜**运维**（熔断状态 · 用量统计）｜**签到与积分**（签到面板）——**Copilot 归「接入」**，WorkBuddy 落地后并入同组、**不新增 tab**；③ **四条实现口径**：分组定义放 `src/tabs.ts`（**纯数据、可单测**）｜**一次只渲染当前 tab 的面板**（不做「全渲染 + CSS 隐藏」→ 首屏 DOM 变小，39 KB 的签到区块不再拖累其它页）｜**网关状态常驻顶栏**（切任何 tab 都看得见「网关在不在跑」），「快速开始」移入「接入」组首卡、「当前范围」移到底部折叠区｜**选中项持久化**（`localStorage` 键 `llm-router.activeTab`，未知值回落默认 tab）；④ **边界**：不加路由库、不引第三方依赖、**不改任何面板内部逻辑**（只改「谁被渲染」） |
| v0.34 | 2026-11-28 | **T25 签到通用化（头头 2026-11-28 拍板）**：**新增 §7.6「站点目录与声明式站点」**——① **问题**：诉求「很多 agent harness 都可以有签到领积分」的真问题是**新增一家平台的边际成本**（内置插件要写 Rust + 发版；HAR 模板要抓包 + 逐条标变量 + 手改断言，**无断言编辑 UI**）；**框架本身不缺**（引擎 / 调度 / 通知已通用且已验证）→ 新增**「轻量接入通道」**；② **spec 数据模型**（`id` / `name` / `method` / `url` / `headers` / `query` / `body` / `credentialId` / `assert{status,jsonPaths}` / `reward{path,unit}` / `balance{method,url,path,unit}`）+ **校验规则**（id 限 `[a-z0-9_-]` 且目录内唯一、**url 必须 http/https 绝对 URL**、method 缺省 GET、assert.status 是**期望值**而非录制值、reward/balance 取不到**不判签到失败**）；③ **占位符只有两种**——`{{token}}`（敏感，取保险库 `credentialId` 指向的真实值，**只进请求不进模板不回显**）与 `{{today}}`（非敏感，`chrono::Local` 当天日期，跨天自动变）；**刻意不做表达式引擎**（避免把 spec 变成脚本）；④ **编译成现有 `SiteTemplate`** → 交给**已验证的重放引擎**执行——注入 / 占位符替换 / 断言 / 8 类分类 / 重试语义 / 结果落库**全部沿用现有实现，一行不改**（唯一新增 = 编译 + 积分/余额路径提取）；⑤ **目录（catalog）**：文件 = `{version, sites[]}`；导入**逐条校验、非法条目带原因返回不静默丢弃**、建站后 **`enabled = 0`**、**同名 id 视为更新**（幂等）；导出**只含 `credentialId`（id 不是密文）**→ 可分享；**内置目录只收已实测条目**（ModelScope / MiniMax Code 走代码插件）+ 一个**空白模板**，**不虚构 endpoint**；⑥ **积分两概念严格区分**：**本次拿到**（`Reward`，落 `checkin_result.reward_json` + `account.quota_json` 合并写）vs **当前余额**（`spec.balance` 查询，**只回前端展示、不落 `quota_json`**，不与 §8.2 限额自学习字段打架）；UI 文案「本次拿到 N 积分」/「当前余额 N 积分」**不混用**；⑦ **分派与落库**：`checkin_site.plugin = "spec"` → `scheduler::execute` 第三支；**不改 schema**（复用 `checkin_site` 四字段）；站点归属渠道与账号口径**沿用 T18**（占位渠道 → 默认账号 `acct_default_<channel_id>`）；⑧ **边界**：**不做**验证码绕过 / 指纹伪造 / 签名伪造（§5.7）、**不做**表达式 spec、**不内置未实测 endpoint**、不改引擎分类与断言语义、不引第三方依赖、不做多账号切换与「签到后兑换」类业务动作 |

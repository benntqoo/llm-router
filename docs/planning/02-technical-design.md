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
| R-6 | 追加 Copilot 伪装头：`editor-version` / `editor-plugin-version` / `copilot-integration-id` / `user-agent`（具体值以参考仓库源码为准，见 §6.2） | Copilot |
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

**出站**：`POST https://api.githubcopilot.com/chat/completions`，Bearer = Copilot token，附 editor 伪装头（具体 UA/版本串以参考仓库源码为准，实现任务 T21 中核实并固化到常量）。

**合规声明（必须写进 UI 首次启用弹窗）**：将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。

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
| 非流式 chat | | | | | | | | | |
| 流式 chat | | | | | | | | | |
| tool_calls | | | | | | | | | |
| usage 返回 | | | | | | | | | |
| reasoning 字段 | | n/a | | ✔已验证 | | | | | |
| 多轮上下文 | | | | | | | | | |

判据：全绿才能在预设中标 `stable`；部分绿标 `partial`（UI 明示哪些特性不可用）。

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

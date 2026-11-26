# LLM Router — 技术设计文档（v1）

| 项 | 值 |
|---|---|
| 版本 | v0.7（变更记录见 §13） |
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
    "retried": ["deepseek 官方", "kimi 官方"]   // fallback 走过的候选
  }
}
```

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

### 6.1 v1 预设表（API Key 型，8 profile）

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

这些是**公开常识的默认建议值，不是实测结论**——02 §10.1 的实测矩阵（T9/T10）跑完之前一律按「未验证」对待，用户可随时改。UI 上标注「默认建议（未实测，可改）」。

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

**出站**：`POST https://api.githubcopilot.com/chat/completions`，Bearer = Copilot token，附 editor 伪装头（具体 UA/版本串以参考仓库源码为准，实现任务 T17 中核实并固化到常量）。

**合规声明（必须写进 UI 首次启用弹窗）**：将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。

### 6.3 WorkBuddy（登录态型）

**本机实测（2026-11-25，只读扫描）：**

| 实测项 | 结果 |
|---|---|
| 设备标识 | `~/.workbuddy/device-id`（36B UUID `357bb4e7-…`；`device-id.machine` 同值）**可读可写** |
| 用户登录态目录 | `~/.workbuddy/app/users/<64位hex>/`（目录存在；内部结构**未验证**，任务 T18a 确认） |
| 连接器密钥 | `~/.workbuddy/app/connector-keys/<uuid>.key`（32B 二进制；`~/.workbuddy-key-fallback/connector-keys/` 存同名备份） |
| 全局配置 | `~/.workbuddy/app/app-config.json` 仅含 locale 等，无凭证 |

**adapter 设计：**

1. 扫描 `app/users/` 取当前登录态（T18a 抓包确认文件与字段后固化读取逻辑）。
2. 转发时携带登录态凭证 + **复写后的 device-id**（隔离用途，默认不复写用户本机文件，仅在"多账号隔离"模式下经确认写入）。
3. 上游 endpoint = WorkBuddy 自家 API host（**未验证，T18a 抓包产出**）。
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

已验证（pi 官方 README）：pi-ai 支持"任意 OpenAI 兼容 API"，`createProvider({ baseUrl, auth, models, api: openAICompletionsApi() })` 明确面向 "local inference servers, proxies"。接入方式二选一：
1. 若 pi CLI 提供 custom provider 配置文件（`.pi/settings.json` 机制，**具体 JSON schema 未验证**），填 base_url + client key。
2. 否则按 pi-ai 编程式写一个 provider 定义（README 示例完整可抄，含 `envApiKeyAuth`）。

> 该项归入任务 T9（实测矩阵）：用 pi 实际连一次本地网关，产出一份可复制的配置片段写进文档。

---

## 7. 自动签到框架

### 7.1 双轨设计

| 轨道 | 实现 | 适用 |
|---|---|---|
| **HAR 重放**（主轨） | 存 HAR 1.2 JSON + 变量表；调度时按模板重放 HTTP 请求，按响应特征（状态码/JSON 路径断言）判定成败 | 纯接口型签到（多数站点） |
| 浏览器自动化（辅轨） | sidecar 起 Playwright chromium 持久化 profile | 需 JS 渲染/OAuth 跳转的少数站点 |

**HAR 模板化规范**：导入 HAR → 交互式标记变量（如 `{{cookie}}`、`{{csrf}}`）→ 保存为站点模板（`checkin_site.config_json`）。用户可自建站点插件，无需写代码（区别于签到盒类框架：模板即配置）。

### 7.2 调度与通知

- 应用内：tokio 定时器（cron 语义，`cron` crate）。
- 系统兜底：Windows 计划任务 / macOS launchd 注册保底触发（应用未运行时），唤起后补签。
- 结果写 `checkin_result`；获得积分回写 `account.quota_json`（签到与额度池同源——本项目的差异化点）。
- 通知：系统通知 + 可选 webhook（企微/飞书/Telegram）。

### 7.3 合规边界

- 站点插件默认全关，启用即视为用户知晓自动化签到可能违反站点条款。
- 不内置任何"反风控"能力（不伪造指纹、不绕验证码；撞到验证码即失败并通知）。

---

## 8. 用量统计

- 采集：上游 `usage` 精确值优先；无则 tiktoken-rs 估算并标 `estimated=1`。
- MiniMax 特例：`usage.completion_tokens_details.reasoning_tokens` → `usage_log.reasoning_tokens`（§4.2）。
- 成本：`price` 表（用户可编辑，预填国产渠道公开牌价）→ `cost_micros`（1e-6 元）。
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
| `cost_micros` | **T13 负责**（价目表换算），T7 恒为 NULL |

**流式采集**：在 SSE 通路上**旁路 tee**——转发字节的同时在**副本**上扫 `data:` 帧：找 `usage` 帧（`stream_options.include_usage` 注入后由上游主动下发，R-4），并记录**首个 token**时刻（TTFT）与输出字符数。tee 不得缓冲全量（§2.2 的零缓冲直通不变）。

**非流式采集**：响应体已整块缓冲，直接从 JSON 的 `usage` 字段解析。

**估算入口**：上游没回 usage 时，走 `TokenEstimator` 估算并标 `estimated=1`。T7 只提供**入口**与启发式实现（字符数 / 4）；T12 用 tiktoken-rs 替换实现。**失败尝试不估 token**——失败的调用没有产出，token 全 0 且 `estimated=0`。

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

### 10.1 渠道实测矩阵（T9 任务执行）

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

### 10.2 安全验收（M1 必过）

```
1. sqlite3 store.db .dump | grep -iE "sk-|api_key|Bearer"   → 无明文命中
2. vault.bin 用十六进制查看 → 无明文 key 片段
3. 日志文件 grep Authorization → 全部脱敏为 ***
4. 监听 0.0.0.0 且无 client key → 启动拒绝并告警
```

### 10.3 里程碑验收

见交付计划文档（`03-delivery-plan.md`）。

---

## 11. 风险提示（v1 范围内）

| # | 风险 | 应对 |
|---|---|---|
| 1 | Copilot/WorkBuddy 额度反代违反平台 ToS，账号可能受限 | UI 首次启用强制确认；默认关闭；不提供规避检测能力 |
| 2 | WorkBuddy 上游协议未验证，T18a 抓包可能发现签名/加密 | 预留 buffer；失败则降级为"仅保活 + 额度查看，不转发" |
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

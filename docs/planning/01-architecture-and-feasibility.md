# LLM Router — 架构与可行性基线

| 项 | 值 |
|---|---|
| 版本 | v0.1（草案，待评审） |
| 日期 | 2026-11-24 |
| 状态 | 待头头确认第 9 节待确认清单后方可进入实现 |
| 本文档改动范围 | 仅新建文档，**未改动任何代码或配置** |

---

## 0. 结论摘要

1. **该赛道功能已饱和**：5 个模块全部有成熟开源实现。CCR（~39k★）、OmniRoute（~90k★）、9router（~33k★）日更迭代；TraeWorkAssistant（Tauri 2 + Rust，514★）在技术栈与功能面与本项目高度重叠。
2. **差异化只剩四点的交集**：① 密钥真加密（现有网关普遍明文，有实测证据）② 中文渠道预设 ③ 签到一体化 ④ 桌面开箱即用且不碰 harness 配置文件。
3. **v1 若含「harness 内置额度」+「完整签到」，工程量约为 TraeWorkAssistant 团队 3 个月产出的 1.5–2 倍**，且高维护成本项占比过半。建议按阶段交付，先跑通「干净内核」。
4. **一个可省两个月的判断**：v1 入站/出站均可只走 OpenAI 协议，零协议转换。协议转换（Anthropic Messages / OpenAI Responses）推迟到 v2。

---

## 1. 产品定义

> 一个本地常驻服务（Tauri 桌面壳 + Rust 内核）。用户在应用内录入各渠道 API Key 与账号凭证，全部加密存储、可随时回看。各 agent harness 只需填写一次 `http://127.0.0.1:<port>/v1` 与一个本地 key，之后新增渠道、切换模型、查看用量、自动签到均在此服务内完成，**不修改任何 harness 的配置文件**。

### 1.1 已确认的需求边界

| 模块 | v1 是否包含 | 备注 |
|---|---|---|
| A 多渠道路由网关 | 是 | 本地服务，OpenAI 兼容入站 |
| B 加密凭据保险库 | 是 | 密钥可回看，解决"只显示一次" |
| C 自动签到 | 是（完整） | 含浏览器自动化型站点 |
| D 反代 harness 内置额度 | 是 | 需分类处理，见 5.3 |
| E Token 用量统计 | 是 | 含成本换算 |

### 1.2 明确不做

- 不修改任何 harness 的配置文件（不写 `~/.claude/settings.json`、`~/.codex/config.toml` 等）。用户自行填写 base_url + key + 模型名。
- 不做多用户 / 计费 / 充值 / 分发。单人本地工具。
- 不做云端同步（v1）。

---

## 2. 竞品格局

### 2.1 能力矩阵

| 能力 | CCR (musistudio) | OmniRoute | 9router (decolua) | TraeWorkAssistant | 本项目目标 |
|---|---|---|---|---|---|
| 本地 OpenAI 兼容入口 | ✅ `:3456` | ✅ `:20128` | ✅ `:20128` | ✅ | ✅ |
| Anthropic 入站 | ✅ | ✅ | ✅ | ✅ | v2 |
| Responses 入站（Codex） | ✅ | ✅ | ✅ | ✅ | v2 |
| 多渠道路由 / fallback | ✅ | ✅ 22 种策略 | ✅ Combo 3 层 | ✅ 多池 | ✅ |
| 用量统计 | ✅ 含成本估算 | ✅ | ✅ 含配额倒计时 | ✅ | ✅ |
| harness 内置额度反代 | ✅ 订阅 provider 插件 | ✅ OAuth 层 | ✅ OAuth 5 类 | ✅ MITM + 登录态扫描 | ✅ |
| 自动签到 | fork 版有 Qoder/WorkBuddy | ❌ | ❌ | ✅ 5 应用 | ✅ |
| 密钥加密存储 | ❌ **明文** | 声称 AES-256-GCM（未实测） | 未确认 | ✅ Stronghold + DPAPI | ✅ |
| 桌面应用 | ✅ Electron | ✅ Electron | ❌ | ✅ Tauri 2 | ✅ Tauri 2 |
| License | MIT | MIT | MIT | MIT | — |

### 2.2 关键证据：现有网关的密钥是明文

第三方实测（formal.ai，对 CCR 3.1.3）：

```
$ sqlite3 ~/.claude-code-router/config.sqlite \
  "select json_extract(value,'$.name'), json_extract(value,'$.api_key') \
   from app_config, json_each(app_config.value_json,'$.Providers')"
mock-local|sk-dummy-ccr-test-123
mock-ui|sk-dummy-ccr-test-456

$ sqlite3 ~/.claude-code-router/config.sqlite \
  "select name, encryption from api_keys"
Local Gateway|plain
Profile: Claude Code|plain
```

独立代码审计（greenlitbooks）原文：

> "it stores all provider keys and imported logins **unencrypted** in one database file"

仅有 0700/0600 文件权限兜底，任何以当前用户身份运行的进程均可读取。

> 状态：**已验证**（两个独立来源，含 SQL 实测输出）。此为 B 模块价值的直接支撑。

---

## 3. 差异化定位

必须在以下四点同时成立，否则本项目无存在理由：

1. **密钥是机密，不是配置**。AES-256-GCM + OS 原生主密钥保护（DPAPI / Keychain），可选主密码（Argon2id）。解密仅在内存，无临时明文文件。
2. **中文渠道预设开箱可用**。DeepSeek / Kimi / GLM / MiniMax / 阿里百炼 / 硅基流动 / 火山方舟，填 key 即用，无需理解协议差异。
3. **签到与网关同源**。签到拿到的额度直接进同一渠道池，无需在两个工具间搬运凭证。
4. **零侵入**。不写别人配置文件，不与任何 harness 争夺配置所有权。

---

## 4. 系统架构

### 4.1 进程模型

单进程。Tauri 2 应用内嵌 HTTP 服务（axum），前端 React 通过 Tauri IPC 调命令层。

```
┌─────────────────────────────────────────────────────┐
│  Tauri 2 App (Windows / macOS)                      │
│                                                     │
│  ┌──────────┐   IPC    ┌─────────────────────────┐  │
│  │ React UI │ ───────► │ commands/ (Tauri 命令层) │  │
│  └──────────┘          └───────────┬─────────────┘  │
│                                    │                │
│         ┌──────────────────────────┼──────────┐     │
│         ▼              ▼           ▼          ▼     │
│  ┌────────────┐ ┌───────────┐ ┌────────┐ ┌───────┐  │
│  │  gateway/  │ │  vault/   │ │sched-  │ │ acct/ │  │
│  │  路由+转发  │ │  加密存储  │ │uler/   │ │ 账号池 │  │
│  └─────┬──────┘ └───────────┘ └───┬────┘ └───┬───┘  │
│        │                          │          │      │
│        ▼                          ▼          ▼      │
│  ┌────────────┐            ┌──────────────────────┐ │
│  │ providers/ │            │  checkin/ (站点插件)  │ │
│  │ 渠道适配器  │            └──────────────────────┘ │
│  └─────┬──────┘                                     │
│        │                                            │
│        ▼                                            │
│  ┌────────────────────────┐                         │
│  │ store/ SQLite (WAL)    │                         │
│  └────────────────────────┘                         │
└─────────────────────────────────────────────────────┘
         │                          │
         ▼                          ▼
   上游 LLM API            本地 harness 登录态 / OAuth
```

### 4.2 建议目录结构

```
src-tauri/
  src/
    main.rs
    commands/          # Tauri IPC 命令层，按域拆分
      channel.rs       # 渠道 CRUD
      vault.rs         # 密钥读写
      usage.rs         # 用量查询
      checkin.rs       # 签到
      account.rs       # 账号池
      settings.rs
    gateway/
      mod.rs
      server.rs        # axum router，入站鉴权
      inbound/         # 入站协议解析
        openai.rs      # v1
        anthropic.rs   # v2
        responses.rs   # v2
      outbound/        # 出站协议组装
        openai_compat.rs
      router.rs        # 模型名 → (channel, upstream_model)
      fallback.rs      # 失败链 / 熔断
      usage_sink.rs    # 用量采集
    providers/         # 渠道适配器（trait + 实现）
      mod.rs           # trait ProviderAdapter
      openai_compat.rs # deepseek/kimi/glm/minimax/... 通用
      oauth/
        copilot.rs
        qoder.rs
        kiro.rs
      session/         # 登录态型
        trae.rs
        workbuddy.rs
    vault/
      mod.rs
      crypto.rs        # AES-256-GCM + Argon2id
      oskey.rs         # DPAPI / Keychain
    checkin/
      mod.rs
      scheduler.rs
      sites/           # 站点插件
    store/
      mod.rs
      schema.rs
      migrations/
  migrations/
  tauri.conf.json
src/                   # React 前端
```

### 4.3 协议矩阵

| 入站（harness → 本地） | v1 | v2 | 说明 |
|---|---|---|---|
| `POST /v1/chat/completions` | ✅ | | 主力，所有渠道原生支持 |
| `GET /v1/models` | ✅ | | 返回别名列表 |
| `POST /v1/embeddings` | 可选 | | 视需求 |
| `POST /v1/messages`（Anthropic） | ❌ | ✅ | Claude Code 需要 |
| `POST /v1/responses`（OpenAI） | ❌ | ✅ | Codex 需要 |

| 出站（本地 → 上游） | v1 | 说明 |
|---|---|---|
| OpenAI 兼容（Bearer key） | ✅ | deepseek / kimi / glm / minimax / 百炼 / 硅基流动 |
| OAuth 型（Bearer access_token） | ✅ | copilot / qoder / kiro |
| 登录态型（Cookie / JWT） | ✅ | trae / workbuddy |

> **关键判断**：v1 的入站与出站都是 OpenAI 格式，`request.body` 只需替换 `model` 字段 + 替换 `Authorization`。这是纯管道，不是网关。协议转换推迟到 v2 可省约 2 个月。

### 4.4 数据模型

```sql
-- 渠道：一个上游来源
CREATE TABLE channel (
  id            TEXT PRIMARY KEY,        -- uuid
  name          TEXT NOT NULL UNIQUE,    -- 用户可读名，如 "DeepSeek 官方"
  kind          TEXT NOT NULL,           -- api_key | oauth | session
  provider      TEXT NOT NULL,           -- deepseek|kimi|glm|minimax|copilot|qoder|trae|...
  base_url      TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1,
  priority      INTEGER NOT NULL DEFAULT 100,  -- 越小越优先
  created_at    INTEGER NOT NULL,
  last_check_at INTEGER,
  last_check_ok INTEGER,
  meta_json     TEXT                     -- 渠道私有参数，如 minimax group_id
);

-- 模型：渠道下的可调模型（从 /models 同步或手填）
CREATE TABLE model (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
  upstream_id   TEXT NOT NULL,           -- 上游真实模型名
  alias         TEXT NOT NULL,           -- 对外暴露名，全局唯一
  context_len   INTEGER,
  enabled       INTEGER NOT NULL DEFAULT 1,
  UNIQUE(channel_id, upstream_id)
);
CREATE UNIQUE INDEX idx_model_alias ON model(alias);

-- 逻辑模型：一个别名 → 多条 fallback
CREATE TABLE route_group (
  id            TEXT PRIMARY KEY,
  alias         TEXT NOT NULL UNIQUE,    -- 对外名，如 "smart"
  strategy      TEXT NOT NULL DEFAULT 'priority'  -- priority|round_robin|cost|sticky_session
);
CREATE TABLE route_member (
  group_id      TEXT NOT NULL REFERENCES route_group(id) ON DELETE CASCADE,
  model_id      TEXT NOT NULL REFERENCES model(id) ON DELETE CASCADE,
  weight        INTEGER NOT NULL DEFAULT 100,
  PRIMARY KEY (group_id, model_id)
);

-- 凭据：密文存储，绝不明文落盘
CREATE TABLE credential (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
  kind          TEXT NOT NULL,           -- api_key | access_token | refresh_token | cookie | jwt
  ciphertext    BLOB NOT NULL,           -- AES-256-GCM
  nonce         BLOB NOT NULL,
  key_version   INTEGER NOT NULL,        -- 主密钥轮换用
  expires_at    INTEGER,                 -- OAuth token 过期时间
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

-- 账号（OAuth / 登录态型渠道可能有多个账号）
CREATE TABLE account (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
  label         TEXT NOT NULL,           -- 脱敏显示，如 "user@gm***.com"
  quota_json    TEXT,                    -- 最近一次额度快照
  enabled       INTEGER NOT NULL DEFAULT 1
);

-- 本地客户 key（harness 填的这个）
CREATE TABLE client_key (
  id            TEXT PRIMARY KEY,
  label         TEXT NOT NULL,
  key_hash      TEXT NOT NULL UNIQUE,    -- SHA-256，不存明文
  key_prefix    TEXT NOT NULL,           -- 前 8 位，供 UI 展示
  created_at    INTEGER NOT NULL,
  expires_at    INTEGER,
  rpm_limit     INTEGER,
  daily_tokens  INTEGER
);

-- 用量流水
CREATE TABLE usage_log (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  ts            INTEGER NOT NULL,
  client_key_id TEXT,
  channel_id    TEXT,
  model_alias   TEXT,
  upstream_model TEXT,
  protocol      TEXT,                    -- openai|anthropic|responses
  prompt_tokens INTEGER DEFAULT 0,
  completion_tokens INTEGER DEFAULT 0,
  cache_read_tokens INTEGER DEFAULT 0,
  cache_write_tokens INTEGER DEFAULT 0,
  reasoning_tokens INTEGER DEFAULT 0,
  estimated     INTEGER NOT NULL DEFAULT 0,  -- 1 = 本地估算值，非上游回报
  ttft_ms       INTEGER,
  duration_ms   INTEGER,
  status        TEXT,                    -- success|error|aborted
  error_kind    TEXT,
  cost_micros   INTEGER                  -- 按价目表换算，单位 1e-6 元
);
CREATE INDEX idx_usage_ts ON usage_log(ts);

-- 签到
CREATE TABLE checkin_site (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  plugin        TEXT NOT NULL,           -- 插件标识
  config_json   TEXT,                    -- 站点私有配置
  schedule      TEXT,                    -- cron
  enabled       INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE checkin_result (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  site_id       TEXT NOT NULL,
  ts            INTEGER NOT NULL,
  ok            INTEGER NOT NULL,
  reward_json   TEXT,                    -- {credits: 400} 等
  message       TEXT
);

-- 价目表（用户可编辑）
CREATE TABLE price (
  provider      TEXT NOT NULL,
  model_pattern TEXT NOT NULL,
  input_per_1m  INTEGER,                 -- 单位 1e-6 元
  output_per_1m INTEGER,
  cache_read_per_1m INTEGER,
  PRIMARY KEY (provider, model_pattern)
);
```

**加密边界**：`credential.ciphertext`、`account.quota_json`（含账号标识）必加密；`usage_log` 与 `channel` 默认不加密（可选开启 SQLCipher）。

---

## 5. 模块设计与技术路径

### 5.1 网关

**入站鉴权**：`Authorization: Bearer <client_key>` 或 `x-api-key`。校验 SHA-256 哈希，命中后才放行。默认只监听 `127.0.0.1`；监听 `0.0.0.0` 时必须先设置 client key。

**路由解析顺序**：

```
请求 model 字段
  ├─ 命中 route_group.alias  → 按 strategy 选 model
  ├─ 命中 model.alias        → 直接用
  └─ 未命中                  → 400，附可用别名列表
```

**转发管道**（v1，零转换）：

```
1. 解析 model → (channel, upstream_model)
2. 读取 credential → 内存解密
3. 改写 body.model = upstream_model
4. 改写 Host / Authorization
5. 流式转发（不缓冲 body）
6. 旁路 tee 采集 usage 与 TTFT
7. 落库 usage_log
8. 失败 → 按 fallback 链重试（幂等判断：仅在未向客户端输出首字节前重试）
```

**必须注意的实现细节**：

| 项 | 要求 |
|---|---|
| SSE 透传 | 逐 chunk 转发，不累积。`Content-Type: text/event-stream` 原样回传。 |
| usage 采集 | 请求体注入 `stream_options: {include_usage: true}`（OpenAI 兼容渠道支持）。上游不返回 `usage` 时标记 `estimated=1` 并本地估算。 |
| 首字节前重试 | 一旦向客户端写出首字节，禁止重试（会产生重复内容）。用 `auto` 模式缓冲首 chunk 判定。 |
| 客户端断连 | 立即取消上游请求，避免空跑计费。 |
| 超时 | 连接超时 30s，首字节超时 600s（长推理模型），总超时按需。 |
| 压缩 | 请求不压缩；响应透传上游 `Content-Encoding`，不解压再压。 |

**熔断**：每 (channel, model) 维护状态机 `closed / open / half_open`。连续 3 次失败进 open，冷却 60s 后 half_open 探测。

### 5.2 凭据保险库

| 项 | 设计 |
|---|---|
| 算法 | AES-256-GCM，每字段独立 nonce |
| 主密钥 | 32 字节随机，由 OS 原生保护 |
| 默认模式 | Windows: DPAPI（`CryptProtectData`，`CRYPTPROTECT_LOCAL_MACHINE` 关闭，绑定当前用户）；macOS: Keychain `kSecClassGenericPassword` |
| 可选模式 | 用户设主密码 → Argon2id（m=64MiB, t=3, p=1）派生 KEK → 包裹主密钥 |
| 落盘位置 | `%APPDATA%\llm-router\vault.bin` / `~/Library/Application Support/llm-router/vault.bin` |
| 内存策略 | 解密结果只在调用栈内存活，不写临时文件，不驻留长生命周期结构 |
| 轮换 | `key_version` 字段支持后续换主密钥；换版本时逐条重加密 |
| 备份 | 导出的备份文件必须仍是密文；导出明文需二次确认 + 明确警告 |

**回看能力**（解决"密钥只显示一次"）：UI 支持点击"显示"后解密展示，30 秒后自动遮蔽；支持一键复制；支持导出为 `.env` 或 JSON（导出明文需二次确认）。

### 5.3 渠道适配器

分三类，技术路径与风险完全不同：

| 类型 | 代表渠道 | 路径 | 稳定性 |
|---|---|---|---|
| **A. API Key** | DeepSeek / Kimi / GLM / MiniMax / 百炼 / 硅基流动 / 火山方舟 / OpenRouter | 静态 Bearer key + OpenAI 兼容端点 | 高 |
| **B. OAuth** | GitHub Copilot / Qoder / Kiro / Grok / Cursor | OAuth 2.0 Device Flow 或 PKCE → access_token → 官方 endpoint；refresh_token 自动续期 | 中高 |
| **C. 登录态** | Trae / WorkBuddy / CodeBuddy / 豆包 | 扫描本地凭证文件（authfile / JWT / cookie）；或 MITM 抓包 | 低 |

**C 类的实际情况**（据 TraeWorkAssistant README，已验证为其自述）：

- 登录态布局分四类管线：`icube` / `chromium` / `authfile` / `electron_root`
- Trae 需处理 6 层设备标识：`machineid` / `storage.json` 遥测 / `aha.device` / 注册表 `MachineGuid` / webview 追踪数据 / `aha TinyStorage`
- Qoder 有设备指纹覆写
- 豆包需 MITM 抓包回写凭证

**结论**：C 类不是"写一个 adapter"，是"每个应用一套逆向工程 + 指纹伪造"。TraeWorkAssistant 用了 47 个命令模块做这件事。

**本项目建议**：C 类做成**外挂插件**（独立进程 / sidecar），核心只定义接口：

```rust
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn kind(&self) -> ChannelKind;
    /// 上游调用，返回统一流式响应
    async fn complete(&self, ctx: &CallContext, req: OpenAIRequest)
        -> Result<BoxStream<'static, Result<Chunk>>, AdapterError>;
    /// 连通性与额度探测
    async fn probe(&self) -> Result<ProbeResult, AdapterError>;
    /// 凭证刷新（OAuth 型必需）
    async fn refresh(&self) -> Result<(), AdapterError>;
}
```

C 类插件优先复用 TraeWorkAssistant 的 MIT 代码，而不是重写。

### 5.4 签到调度

**核心设计：HAR 模板驱动，而非浏览器自动化。**

参考 QD（qd-today/qd，5.6k★）的做法：用户 F12 录制一次签到请求 → 导出 HAR → 模板化（提取可变字段为 `{{key}}`）→ 定时重放。

理由：
- 无浏览器依赖，包体小，启动快
- 不触发 Cloudflare 的 JS 挑战判定（Playwright 反而更容易被识别）
- 站点脚本是纯数据（HAR + 变量），用户可自行扩展，无需写代码

**双轨设计**：

| 轨道 | 适用 | 实现 |
|---|---|---|
| HAR 重放 | 纯 HTTP 接口签到（多数站点） | 存 HAR + 变量表，按 cron 重放，比对响应特征判定成功 |
| 浏览器自动化 | 需 JS 渲染 / 有验证码 / 需 OAuth 跳转 | 内嵌 Playwright（chromium 持久化 profile），只在少数站点启用 |

**调度**：应用内 tokio 定时器 + 系统级兜底（Windows 计划任务 / macOS launchd），避免应用未启动导致漏签。

**结果回写**：签到获得的额度直接更新 `account.quota_json`，UI 上体现为渠道池额度变化。这是相对其他工具的真实价值。

**通知**：桌面通知 + 可选 webhook（企微 / 飞书 / Telegram）。

### 5.5 用量统计

**采集**：

1. 上游返回 `usage` → 精确值
2. 上游不返回（部分渠道流式不返回）→ 本地 estimator 估算，标 `estimated=1`
3. 流式请求必须注入 `stream_options.include_usage`，否则拿不到

**tokenizer 选型**：`tiktoken-rs`（cl100k_base / o200k_base）。对国产模型只能近似，必须标 `estimated=1`。绝不用估算值冒充精确值。

**指标**：

| 维度 | 指标 |
|---|---|
| 总量 | 请求数 / 成功率 / 输入 tokens / 输出 tokens / 缓存命中 tokens |
| 性能 | TTFT 均值与 P95 / 输出速度 tokens/s / 总耗时 |
| 成本 | 按 `price` 表换算，按渠道 / 模型 / 天聚合 |
| 来源 | 按 `client_key` 区分是哪个 harness 在用 |

**配额监控**：对支持额度查询的渠道（GLM Coding Plan / Kimi / MiniMax / 百炼）额外调用额度接口，展示 5h / 日 / 周剩余与重置倒计时。

---

## 6. 工程量评估

以单人 + AI 辅助、熟练 Rust 为基准。

| 阶段 | 内容 | 估时 | 依赖 |
|---|---|---|---|
| P0 | 项目骨架：Tauri 2 + axum + SQLite + 基础 UI 壳 | 3–5 天 | — |
| P1 | 保险库：AES-GCM + DPAPI/Keychain + UI 录入/回看 | 4–6 天 | P0 |
| P2 | 网关内核：OpenAI 入站 + OpenAI 兼容出站 + 路由 + 流式 + 熔断 | 7–10 天 | P0 |
| P3 | A 类渠道适配（6–8 家预设） | 4–5 天 | P2 |
| P4 | 用量统计：采集 + tokenizer + 成本表 + 图表 | 5–7 天 | P2 |
| P5 | B 类 OAuth 适配（Copilot 1 家先行） | 5–8 天 | P2 |
| P6 | 签到框架：HAR 重放 + 调度 + 通知 | 6–9 天 | P0 |
| P7 | 签到站点插件（每站点） | 1–2 天/站点 | P6 |
| P8 | C 类登录态适配（复用 TraeWorkAssistant） | 10–20 天/应用 | P2,P5 |
| P9 | Anthropic 入站（Claude Code） | 10–14 天 | P2 |
| P10 | Responses 入站（Codex） | 8–12 天 | P9 |

**P0–P6 合计约 35–45 工作日**，可得一个可用的"干净内核"（A 类渠道 + 加密 + 用量 + HAR 签到）。P8 是最大不确定性，单应用就可能吃掉 3 周。

---

## 7. 风险登记册

| # | 风险 | 影响 | 概率 | 应对 |
|---|---|---|---|---|
| R1 | **违反第三方平台条款**。9router 的 provider 目录中有 12 家被标 `tos: avoid`（含 Kiro）。自动化签到与额度反代普遍违反条款，可能导致账号封禁。 | 高 | 高 | 产品内明确免责声明；默认关闭 C 类与高风险签到站点；启动首次使用弹窗确认；不提供开箱即用的"封号规避"能力 |
| R2 | **C 类适配随对方更新失效**。设备指纹、凭证格式、签名随时变。 | 高 | 高 | 插件化隔离；失败降级为"需手动更新"；不做自动化对抗 |
| R3 | **上游迭代速度压制**。CCR 一周多版，OmniRoute 90k★ 日更。功能面半年内可能被完全覆盖。 | 高 | 中 | 差异化锚定在加密 + 中文生态 + 签到一体化；不追 provider 数量 |
| R4 | **协议转换的兼容性黑洞**。Anthropic tool_use ↔ OpenAI tool_calls 字段不对称，流式增量拼装易出错。 | 中 | 高 | 推迟到 v2；届时以真实 harness 做回归测试集 |
| R5 | **密钥加密被旁路**。前端 IPC 明文传递、崩溃转储、日志泄漏。 | 高 | 中 | IPC 只传密文索引不传明文；日志脱敏中间件；禁用崩溃转储中的敏感内存 |
| R6 | **MITM 需要管理员权限与证书信任**，且易被检测。 | 中 | 中 | 仅在用户显式开启时安装自签 CA；提供一键卸载；不默认启用 |
| R7 | **估算 token 被误当精确值**，导致成本统计失真。 | 中 | 高 | `estimated` 字段强约束 UI 展示；估算值不参与预算告警 |
| R8 | **本地服务被同机其他进程滥用**。若监听 0.0.0.0 且无 key，等于开放代理。 | 高 | 低 | 默认 127.0.0.1；绑 0.0.0.0 时强制要求 client key；启动日志警告 |

---

## 8. 分阶段交付

| 里程碑 | 交付物 | 验收判据 |
|---|---|---|
| **M1 可用内核** | P0–P3 | 用 DeepSeek+Kimi+GLM 三家 key，通过本地 `:PORT/v1` 在任意 OpenAI 兼容客户端完成一次多轮对话；密钥在 SQLite 中为密文（`hexdump` 验证无明文）；重启后仍可用。 |
| **M2 可观测** | P4 | 一次请求后能在 UI 看到 tokens / TTFT / 速度 / 成本；流式请求 usage 非全零。 |
| **M3 可签到** | P6 + 2 个站点 | 定时任务按 cron 触发，失败有通知，成功写回额度。 |
| **M4 可反代** | P5 + 1 个 C 类 | Copilot OAuth 走通；Trae 或 WorkBuddy 至少一个能作为上游。 |
| **M5 全协议** | P9–P10 | Claude Code 与 Codex 直连本地服务正常工作，工具调用与流式均正确。 |

M1 完成后即可自用；M3/M4 是差异化卖点；M5 决定能否替代 CCR。

---

## 9. 待确认清单

实现前必须逐项确认，否则会产生返工。

1. **目标 harness 清单**（要反代哪些？影响 P8 工期，每多一个 +2~3 周）
   - 候选：Trae / WorkBuddy / Qoder / CodeBuddy / 豆包 / Cursor / GitHub Copilot / Kiro / Grok CLI
   - 建议 v1 只做 **GitHub Copilot**（OAuth 路径最规范）+ **WorkBuddy**（自有、格式可控）

2. **目标签到站点清单**（影响 P7 工期，每站点 1~2 天）
   - 候选：Trae / WorkBuddy / Qoder / 智谱清言 / MiniMax Code / ModelScope / 讯飞 Loomy / ZCode

3. **是否需要 Anthropic 入站进 v1**
   - 若主力是 `pi` 且 pi 支持 OpenAI 协议 → 可不做，直接省 2 周
   - 若需要 Claude Code 接入 → 必须做

4. **MiniMax 端点实测**
   - `api.minimax.chat/v1` 是否要求 `group_id`、是否完全兼容 OpenAI 流式 —— **未验证**，需实测后决定是否单列 adapter

5. **分发形态**
   - 仅自用？还是要发布（涉及免责声明、签名公证、自动更新）

6. **多账号是否需要**
   - 若需要（同一渠道多个 key 轮询），`account` 表设计需提前，否则后期迁移成本高

---

## 附：本文档的证据来源

| 结论 | 来源 | 状态 |
|---|---|---|
| CCR ~39k★、v3.1.6、支持 Pi/Codex/WorkBuddy 等 10 个 agent、MIT | github.com/musistudio/claude-code-router | 已验证（抓取 README） |
| CCR 密钥明文存储 | formal.ai 实测 SQL 输出 + greenlitbooks 代码审计 | 已验证（两独立来源） |
| OmniRoute 声称 394 providers、AES-256-GCM、MIT | github.com/cheaper-inference/OmniRoute | 部分验证（README 自述，加密未实测） |
| 9router 32.8k★、MIT、OAuth 5 类、Combo 3 层、RTK | github.com/decolua/9router + 多个第三方统计站 | 已验证（多源一致） |
| TraeWorkAssistant Tauri 2 + Rust + Stronghold + MITM + 5 应用 | github.com/smart-open/TraeWorkAssistant | 已验证（抓取 README） |
| QD 5.6k★、HAR 模板驱动 | github.com/qd-today/qd | 已验证 |
| Trae 需 6 层设备标识、四类登录态布局 | TraeWorkAssistant README 自述 | 未独立验证（仅其自述） |
| MiniMax OpenAI 兼容性 | — | **未验证，需实测** |

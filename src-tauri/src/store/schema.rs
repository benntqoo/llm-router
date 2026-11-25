//! 数据库表结构定义（DDL 唯一权威载体）。
//!
//! 来源：`docs/planning/01-architecture-and-feasibility.md` §4.4 的 DDL 全量，
//! 并按 `docs/planning/02-technical-design.md` §9 的增量修订调整：
//!
//! - `credential.kind` 增加 OAuth / 登录态两类取值；
//! - `account` 增加 `meta_json`（WorkBuddy 登录态路径引用）；
//! - `route_group.strategy` 注释口径与 01 文档同步（含 `sticky_session`）。
//!
//! 本文件只放 DDL；迁移的编排（up/down 顺序、版本记录）在 `migrations.rs`。

/// 渠道：一个上游来源。
pub const CHANNEL_DDL: &str = r#"
CREATE TABLE channel (
  id            TEXT PRIMARY KEY,        -- uuid
  name          TEXT NOT NULL UNIQUE,    -- 用户可读名，如 "DeepSeek 官方"
  kind          TEXT NOT NULL,           -- api_key | oauth | session
  provider      TEXT NOT NULL,           -- deepseek|kimi|glm|minimax|copilot|workbuddy|...
  base_url      TEXT NOT NULL,
  enabled       INTEGER NOT NULL DEFAULT 1,
  priority      INTEGER NOT NULL DEFAULT 100,  -- 越小越优先
  created_at    INTEGER NOT NULL,
  last_check_at INTEGER,
  last_check_ok INTEGER,
  meta_json     TEXT                     -- 渠道私有参数，如 minimax group_id
);
"#;

/// 模型：渠道下的可调模型。
pub const MODEL_DDL: &str = r#"
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
"#;

/// 逻辑模型：一个别名 → 多条 fallback。
pub const ROUTE_GROUP_DDL: &str = r#"
CREATE TABLE route_group (
  id            TEXT PRIMARY KEY,
  alias         TEXT NOT NULL UNIQUE,    -- 对外名，如 "smart"
  strategy      TEXT NOT NULL DEFAULT 'priority'  -- priority|round_robin|cost|sticky_session
);
"#;

/// 逻辑模型成员。
pub const ROUTE_MEMBER_DDL: &str = r#"
CREATE TABLE route_member (
  group_id      TEXT NOT NULL REFERENCES route_group(id) ON DELETE CASCADE,
  model_id      TEXT NOT NULL REFERENCES model(id) ON DELETE CASCADE,
  weight        INTEGER NOT NULL DEFAULT 100,
  PRIMARY KEY (group_id, model_id)
);
"#;

/// 凭据：密文存储，绝不明文落盘（加密实现见 T2 保险库）。
pub const CREDENTIAL_DDL: &str = r#"
CREATE TABLE credential (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
  kind          TEXT NOT NULL,           -- api_key|oauth_gh_token|oauth_copilot_token|session_workbuddy
  ciphertext    BLOB NOT NULL,           -- AES-256-GCM
  nonce         BLOB NOT NULL,
  key_version   INTEGER NOT NULL,        -- 主密钥轮换用
  expires_at    INTEGER,                 -- OAuth token 过期时间
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);
"#;

/// 账号（OAuth / 登录态型渠道可能有多个账号）。
pub const ACCOUNT_DDL: &str = r#"
CREATE TABLE account (
  id            TEXT PRIMARY KEY,
  channel_id    TEXT NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
  label         TEXT NOT NULL,           -- 脱敏显示，如 "user@gm***.com"
  quota_json    TEXT,                    -- 最近一次额度快照
  enabled       INTEGER NOT NULL DEFAULT 1,
  meta_json     TEXT                     -- 02 §9：登录态路径引用，如 WorkBuddy users_dir / device_id_file
);
"#;

/// 本地客户 key（harness 填的这个；只存 SHA-256 哈希）。
pub const CLIENT_KEY_DDL: &str = r#"
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
"#;

/// 用量流水。
pub const USAGE_LOG_DDL: &str = r#"
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
"#;

/// 签到站点配置。
pub const CHECKIN_SITE_DDL: &str = r#"
CREATE TABLE checkin_site (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  plugin        TEXT NOT NULL,           -- 插件标识
  config_json   TEXT,                    -- 站点私有配置
  schedule      TEXT,                    -- cron
  enabled       INTEGER NOT NULL DEFAULT 1
);
"#;

/// 签到结果。
pub const CHECKIN_RESULT_DDL: &str = r#"
CREATE TABLE checkin_result (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  site_id       TEXT NOT NULL,
  ts            INTEGER NOT NULL,
  ok            INTEGER NOT NULL,
  reward_json   TEXT,                    -- {credits: 400} 等
  message       TEXT
);
"#;

/// 价目表（用户可编辑）。
pub const PRICE_DDL: &str = r#"
CREATE TABLE price (
  provider      TEXT NOT NULL,
  model_pattern TEXT NOT NULL,
  input_per_1m  INTEGER,                 -- 单位 1e-6 元
  output_per_1m INTEGER,
  cache_read_per_1m INTEGER,
  PRIMARY KEY (provider, model_pattern)
);
"#;

/// 熔断状态：非 closed 的熔断/冷却记录（02 §2.3）。
///
/// 只在熔断打开（open）或半开探测（half_open）时存在；回到 closed 即删除该行，
/// 因此这张表天然只反映"当前正在冷却的渠道"。
pub const CIRCUIT_STATE_DDL: &str = r#"
CREATE TABLE circuit_state (
  channel_id      TEXT NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
  upstream_model  TEXT NOT NULL,
  state           TEXT NOT NULL,              -- open|half_open
  reason          TEXT,                       -- failures|rate_limit
  failures        INTEGER NOT NULL DEFAULT 0, -- 连续失败次数
  rate_limit_hits INTEGER NOT NULL DEFAULT 0, -- 429 命中次数（指数退避用）
  open_until      INTEGER,                    -- 冷却截止（Unix 秒）
  updated_at      INTEGER NOT NULL,
  PRIMARY KEY (channel_id, upstream_model)
);
"#;

/// 应用级设置（键值对；02 §2.3 熔断时长等）。
pub const APP_SETTING_DDL: &str = r#"
CREATE TABLE app_setting (
  key        TEXT PRIMARY KEY,   -- 如 circuit_open_secs
  value      TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
"#;

/// v1 全部业务表名（迁移后应全部存在；用于测试断言）。
pub const ALL_TABLES: &[&str] = &[
    "channel",
    "model",
    "route_group",
    "route_member",
    "credential",
    "account",
    "client_key",
    "usage_log",
    "checkin_site",
    "checkin_result",
    "price",
    "circuit_state",
    "app_setting",
];

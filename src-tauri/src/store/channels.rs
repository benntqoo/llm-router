//! 渠道与模型的读写（T8；02 §6.1.2、§9）。
//!
//! - **一条渠道至少一个模型**：别名来自 `model.alias`，没有模型的渠道不会出现在
//!   `GET /v1/models`，等于白建；
//! - **模型别名全局唯一**（`idx_model_alias`）：别名是路由入口；
//! - 渠道私有参数（路径模板 / 鉴权样式 / GroupId / include_usage 开关 / 额外头）
//!   落在 `channel.meta_json`（**整份替换**，不与旧值合并——表单状态即真相）；
//! - **凭证不在这里**：密钥由保险库负责（`credential` 表密文 + `vault.bin` 主密钥，02 §5）。
//!
//! 编辑语义：渠道行原地更新；模型按别名 upsert；**表单里删掉的模型置为禁用**
//! （不物理删除，避免连带删掉 `route_member` 的编排关系）。

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::store::{Result, Store, StoreError};

/// 自定义渠道的 provider 标识（02 §6.1）。
pub const CUSTOM_PROVIDER: &str = "custom";

/// 渠道私有参数（写进 `channel.meta_json`；字段见 02 §9）。
///
/// 序列化时**全部字段都写**（含 `null`）：`meta_json` 是整份替换，
/// 写 `null` 才能表达"清空这个字段"（例如清掉 GroupId）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelMeta {
    /// chat 路径模板（`null` = 用预设/默认值）。
    pub path_chat: Option<String>,
    /// 鉴权样式：`bearer` / `custom_header`（`null` = 用预设/默认值）。
    pub auth_style: Option<String>,
    /// 自定义鉴权头名（`auth_style = custom_header` 时必填）。
    pub auth_header_name: Option<String>,
    /// MiniMax 国内站 GroupId（02 §4.2）。
    pub group_id: Option<String>,
    /// 是否支持 `?GroupId=` 追加（R-5）。
    pub supports_groupid_query: Option<bool>,
    /// 是否注入 `stream_options.include_usage`（R-4）。
    pub inject_stream_usage: Option<bool>,
    /// 额外固定头（R-6）。
    pub extra_headers: BTreeMap<String, String>,
}

/// 模型摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSummary {
    /// `model.id`
    pub id: String,
    /// 对外别名（路由入口）
    pub alias: String,
    /// 上游真实模型名（R-1 替换的目标）
    pub upstream_id: String,
    /// 是否启用
    pub enabled: bool,
}

/// 渠道摘要（列表与回显）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelSummary {
    /// `channel.id`
    pub id: String,
    /// 渠道名
    pub name: String,
    /// 渠道类型（`api_key`）
    pub kind: String,
    /// 预设标识（`deepseek` …；自定义渠道为 `custom`）
    pub provider: String,
    /// 基础地址
    pub base_url: String,
    /// 是否启用
    pub enabled: bool,
    /// 优先级（越小越优先）
    pub priority: i64,
    /// 渠道私有参数
    pub meta: ChannelMeta,
    /// 该渠道下的模型
    pub models: Vec<ModelSummary>,
    /// 是否已录入凭证（**只有布尔值**，不含密文）
    pub has_credential: bool,
}

/// 模型入参。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ModelInput {
    /// 对外别名
    pub alias: String,
    /// 上游真实模型名
    pub upstream_id: String,
    /// 是否启用
    pub enabled: bool,
}

impl Default for ModelInput {
    fn default() -> Self {
        Self {
            alias: String::new(),
            upstream_id: String::new(),
            enabled: true,
        }
    }
}

/// 渠道入参（IPC payload；字段名按 Tauri 惯例用 camelCase）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChannelInput {
    /// 渠道 id（`null` = 新建；给出则为更新）
    pub id: Option<String>,
    /// 渠道名（唯一）
    pub name: String,
    /// 预设标识（`deepseek` … / `custom`）
    pub provider: String,
    /// 基础地址
    pub base_url: String,
    /// 渠道类型（默认 `api_key`）
    pub kind: Option<String>,
    /// 是否启用
    pub enabled: bool,
    /// 优先级（默认 100）
    pub priority: Option<i64>,
    /// 渠道私有参数
    pub meta: ChannelMeta,
    /// 模型列表（至少一条）
    pub models: Vec<ModelInput>,
}

impl Default for ChannelInput {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            provider: String::new(),
            base_url: String::new(),
            kind: None,
            enabled: true,
            priority: None,
            meta: ChannelMeta::default(),
            models: Vec::new(),
        }
    }
}

/// 校验入参（不合法直接报错，不落库）。
pub fn validate(input: &ChannelInput) -> Result<()> {
    if input.name.trim().is_empty() {
        return Err(StoreError::Invalid("渠道名不能为空".to_string()));
    }
    if input.provider.trim().is_empty() {
        return Err(StoreError::Invalid("必须选择预设或自定义渠道".to_string()));
    }
    let base = input.base_url.trim();
    if base.is_empty() {
        return Err(StoreError::Invalid("base_url 不能为空".to_string()));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(StoreError::Invalid(
            "base_url 必须以 http:// 或 https:// 开头".to_string(),
        ));
    }
    if let Some(path) = input.meta.path_chat.as_deref() {
        let path = path.trim();
        if !path.is_empty() && !path.starts_with('/') {
            return Err(StoreError::Invalid("路径模板必须以 / 开头".to_string()));
        }
    }
    if input.models.is_empty() {
        return Err(StoreError::Invalid(
            "至少要有一个模型（否则渠道不会出现在 /v1/models）".to_string(),
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for model in &input.models {
        let alias = model.alias.trim();
        if alias.is_empty() {
            return Err(StoreError::Invalid("模型别名不能为空".to_string()));
        }
        if model.upstream_id.trim().is_empty() {
            return Err(StoreError::Invalid(format!("模型 {alias} 的上游模型名不能为空")));
        }
        if seen.contains(&alias) {
            return Err(StoreError::Invalid(format!("模型别名重复：{alias}")));
        }
        seen.push(alias);
    }
    Ok(())
}

/// 保存渠道（新建或更新）及其模型，返回渠道摘要。
pub fn save_channel(store: &Store, input: &ChannelInput) -> Result<ChannelSummary> {
    validate(input)?;
    let name = input.name.trim().to_string();
    let provider = input.provider.trim().to_string();
    let base_url = input
        .base_url
        .trim()
        .trim_end_matches('/')
        .to_string();
    let kind = input
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .unwrap_or("api_key")
        .to_string();
    let priority = input.priority.unwrap_or(100);
    let meta_json = serde_json::to_string(&input.meta)
        .map_err(|e| StoreError::Invalid(format!("渠道参数序列化失败: {e}")))?;

    let channel_id = store.with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| StoreError::Sqlite(e))?;

        let channel_id = match input.id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => {
                let exists: i64 = tx
                    .query_row("SELECT COUNT(*) FROM channel WHERE id = ?1", [id], |row| {
                        row.get(0)
                    })
                    .map_err(StoreError::Sqlite)?;
                if exists == 0 {
                    return Err(StoreError::Invalid(format!("渠道 {id} 不存在")));
                }
                tx.execute(
                    "UPDATE channel
                     SET name = ?2, kind = ?3, provider = ?4, base_url = ?5,
                         enabled = ?6, priority = ?7, meta_json = ?8
                     WHERE id = ?1",
                    rusqlite::params![
                        id,
                        name,
                        kind,
                        provider,
                        base_url,
                        input.enabled,
                        priority,
                        meta_json
                    ],
                )
                .map_err(StoreError::Sqlite)?;
                id.to_string()
            }
            None => {
                let id = crate::vault::new_id("ch");
                let now = crate::vault::now_secs();
                tx.execute(
                    "INSERT INTO channel
                     (id, name, kind, provider, base_url, enabled, priority, created_at, meta_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        id,
                        name,
                        kind,
                        provider,
                        base_url,
                        input.enabled,
                        priority,
                        now,
                        meta_json
                    ],
                )
                .map_err(StoreError::Sqlite)?;
                id
            }
        };

        // 模型 upsert：按别名匹配已有行（仅限本渠道）
        let mut keep: Vec<String> = Vec::new();
        for model in &input.models {
            let alias = model.alias.trim().to_string();
            let upstream_id = model.upstream_id.trim().to_string();
            keep.push(alias.clone());

            let owned: Option<String> = tx
                .query_row(
                    "SELECT channel_id FROM model WHERE alias = ?1",
                    [&alias],
                    |row| row.get(0),
                )
                .optional()
                .map_err(StoreError::Sqlite)?;
            match owned {
                Some(owner) if owner == channel_id => {
                    tx.execute(
                        "UPDATE model SET upstream_id = ?2, enabled = ?3 WHERE alias = ?1",
                        rusqlite::params![alias, upstream_id, model.enabled],
                    )
                    .map_err(StoreError::Sqlite)?;
                }
                Some(_owner) => {
                    return Err(StoreError::Invalid(format!(
                        "模型别名 {alias} 已被其它渠道占用"
                    )));
                }
                None => {
                    tx.execute(
                        "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            crate::vault::new_id("m"),
                            channel_id,
                            upstream_id,
                            alias,
                            model.enabled
                        ],
                    )
                    .map_err(StoreError::Sqlite)?;
                }
            }
        }

        // 表单里删掉的模型 → 禁用（不物理删除，避免连带删掉 route_member 编排）
        let existing: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT alias FROM model WHERE channel_id = ?1")
                .map_err(StoreError::Sqlite)?;
            let rows = stmt
                .query_map([&channel_id], |row| row.get::<_, String>(0))
                .map_err(StoreError::Sqlite)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(StoreError::Sqlite)?);
            }
            out
        };
        for alias in existing {
            if !keep.contains(&alias) {
                tx.execute(
                    "UPDATE model SET enabled = 0 WHERE channel_id = ?1 AND alias = ?2",
                    rusqlite::params![channel_id, alias],
                )
                .map_err(StoreError::Sqlite)?;
            }
        }

        tx.commit().map_err(StoreError::Sqlite)?;
        Ok(channel_id)
    })?;

    channel_summary(store, &channel_id)
}

/// 列出全部渠道（含模型与"是否已录凭证"）。
pub fn list_channels(store: &Store) -> Result<Vec<ChannelSummary>> {
    let ids: Vec<String> = store.with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT id FROM channel ORDER BY created_at, id")
            .map_err(StoreError::Sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(StoreError::Sqlite)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(StoreError::Sqlite)?);
        }
        Ok(out)
    })?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(channel_summary(store, &id)?);
    }
    Ok(out)
}

/// 取单个渠道摘要。
pub fn channel_summary(store: &Store, channel_id: &str) -> Result<ChannelSummary> {
    store.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, kind, provider, base_url, enabled, priority, meta_json
                 FROM channel WHERE id = ?1",
            )
            .map_err(StoreError::Sqlite)?;
        let mut rows = stmt.query([channel_id]).map_err(StoreError::Sqlite)?;
        let Some(row) = rows.next().map_err(StoreError::Sqlite)? else {
            return Err(StoreError::Invalid(format!("渠道 {channel_id} 不存在")));
        };
        let id: String = row.get(0).map_err(StoreError::Sqlite)?;
        let name: String = row.get(1).map_err(StoreError::Sqlite)?;
        let kind: String = row.get(2).map_err(StoreError::Sqlite)?;
        let provider: String = row.get(3).map_err(StoreError::Sqlite)?;
        let base_url: String = row.get(4).map_err(StoreError::Sqlite)?;
        let enabled: i64 = row.get(5).map_err(StoreError::Sqlite)?;
        let priority: i64 = row.get(6).map_err(StoreError::Sqlite)?;
        let meta_json: Option<String> = row.get(7).map_err(StoreError::Sqlite)?;
        drop(rows);
        drop(stmt);

        let meta = match meta_json.as_deref().map(str::trim) {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<ChannelMeta>(raw).map_err(|e| {
                StoreError::Invalid(format!("渠道 {id} 的 meta_json 不是合法 JSON: {e}"))
            })?,
            _ => ChannelMeta::default(),
        };

        let mut model_stmt = conn
            .prepare(
                "SELECT id, alias, upstream_id, enabled FROM model
                 WHERE channel_id = ?1 ORDER BY alias",
            )
            .map_err(StoreError::Sqlite)?;
        let model_rows = model_stmt
            .query_map([channel_id], |row| {
                Ok(ModelSummary {
                    id: row.get(0)?,
                    alias: row.get(1)?,
                    upstream_id: row.get(2)?,
                    enabled: row.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(StoreError::Sqlite)?;
        let mut models = Vec::new();
        for row in model_rows {
            models.push(row.map_err(StoreError::Sqlite)?);
        }

        let credential_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM credential WHERE channel_id = ?1",
                [channel_id],
                |row| row.get(0),
            )
            .map_err(StoreError::Sqlite)?;

        Ok(ChannelSummary {
            id,
            name,
            kind,
            provider,
            base_url,
            enabled: enabled != 0,
            priority,
            meta,
            models,
            has_credential: credential_count > 0,
        })
    })
}

/// 删除渠道（连带删除其模型与凭证；`route_member` 由外键级联清理）。
pub fn delete_channel(store: &Store, channel_id: &str) -> Result<bool> {
    store.with_conn(|conn| {
        let affected = conn
            .execute("DELETE FROM channel WHERE id = ?1", [channel_id])
            .map_err(StoreError::Sqlite)?;
        Ok(affected > 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        (dir, store)
    }

    fn input(name: &str, provider: &str) -> ChannelInput {
        ChannelInput {
            name: name.to_string(),
            provider: provider.to_string(),
            base_url: "https://api.example.com".to_string(),
            models: vec![ModelInput {
                alias: "chat-a".to_string(),
                upstream_id: "chat-a".to_string(),
                enabled: true,
            }],
            ..ChannelInput::default()
        }
    }

    #[test]
    fn save_creates_channel_and_model() {
        let (_dir, store) = store();
        let summary = save_channel(&store, &input("我的渠道", "custom")).expect("保存失败");
        assert_eq!(summary.name, "我的渠道");
        assert_eq!(summary.provider, "custom");
        assert_eq!(summary.base_url, "https://api.example.com");
        assert_eq!(summary.models.len(), 1);
        assert_eq!(summary.models[0].alias, "chat-a");
        assert!(!summary.has_credential, "新建渠道尚未录凭证");
    }

    #[test]
    fn saved_channel_appears_in_model_table() {
        let (_dir, store) = store();
        save_channel(&store, &input("我的渠道", "custom")).expect("保存失败");
        let alias: String = store
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT alias FROM model", [], |row| row.get(0))?)
            })
            .expect("读取模型");
        assert_eq!(alias, "chat-a");
    }

    #[test]
    fn update_replaces_meta_json_wholesale() {
        let (_dir, store) = store();
        let mut first = input("我的渠道", "custom");
        first.meta.group_id = Some("g-1".to_string());
        first.meta.path_chat = Some("/v1/chat/completions".to_string());
        save_channel(&store, &first).expect("保存失败");

        // 二次保存：清掉 group_id
        let mut second = first.clone();
        second.id = Some(
            list_channels(&store)
                .expect("列渠道失败")
                .first()
                .expect("应有渠道")
                .id
                .clone(),
        );
        second.meta.group_id = None;
        second.meta.path_chat = Some("/api/chat".to_string());
        let summary = save_channel(&store, &second).expect("更新失败");
        assert_eq!(summary.meta.group_id, None, "group_id 应被清空");
        assert_eq!(summary.meta.path_chat.as_deref(), Some("/api/chat"));
    }

    #[test]
    fn removed_model_is_disabled_not_deleted() {
        let (_dir, store) = store();
        let summary = save_channel(&store, &input("我的渠道", "custom")).expect("保存失败");
        let channel_id = summary.id.clone();

        let mut second = input("我的渠道", "custom");
        second.id = Some(channel_id.clone());
        second.models = vec![ModelInput {
            alias: "chat-b".to_string(),
            upstream_id: "chat-b".to_string(),
            enabled: true,
        }];
        save_channel(&store, &second).expect("更新失败");

        let rows: Vec<(String, i64)> = store
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT alias, enabled FROM model")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .expect("读取模型");
        assert_eq!(rows.len(), 2, "旧模型不物理删除");
        let old = rows.iter().find(|(a, _)| a == "chat-a").expect("旧模型仍在");
        assert_eq!(old.1, 0, "旧模型应被禁用");
    }

    #[test]
    fn alias_owned_by_another_channel_is_rejected() {
        let (_dir, store) = store();
        save_channel(&store, &input("渠道 A", "custom")).expect("保存失败");
        let err = save_channel(&store, &input("渠道 B", "custom")).expect_err("别名重复应报错");
        assert!(err.to_string().contains("已被其它渠道占用"), "实际 {err}");
    }

    #[test]
    fn duplicate_alias_in_one_form_is_rejected() {
        let (_dir, store) = store();
        let mut input = input("我的渠道", "custom");
        input.models.push(ModelInput {
            alias: "chat-a".to_string(),
            upstream_id: "chat-a".to_string(),
            enabled: true,
        });
        let err = save_channel(&store, &input).expect_err("别名重复应报错");
        assert!(err.to_string().contains("重复"), "实际 {err}");
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        let (_dir, store) = store();

        let mut empty_name = input("", "custom");
        assert!(save_channel(&store, &empty_name).is_err());
        empty_name.name = "  ".to_string();
        assert!(save_channel(&store, &empty_name).is_err());

        let mut bad_url = input("渠道", "custom");
        bad_url.base_url = "ftp://example.com".to_string();
        assert!(save_channel(&store, &bad_url).is_err());

        let mut bad_path = input("渠道", "custom");
        bad_path.meta.path_chat = Some("v1/chat/completions".to_string());
        assert!(save_channel(&store, &bad_path).is_err());

        let mut no_model = input("渠道", "custom");
        no_model.models.clear();
        let err = save_channel(&store, &no_model).expect_err("没有模型应报错");
        assert!(err.to_string().contains("至少要有一个模型"), "实际 {err}");
    }

    #[test]
    fn delete_removes_channel_models_and_credentials() {
        let (_dir, store) = store();
        let summary = save_channel(&store, &input("我的渠道", "custom")).expect("保存失败");
        let channel_id = summary.id.clone();

        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO credential (id, channel_id, kind, ciphertext, nonce, key_version, created_at, updated_at)
                     VALUES ('cred-1', ?1, 'api_key', X'00', X'00', 1, 0, 0)",
                    [&channel_id],
                )?;
                Ok(())
            })
            .expect("写入凭证");

        assert!(delete_channel(&store, &channel_id).expect("删除失败"));
        assert!(!delete_channel(&store, &channel_id).expect("重复删除应返回 false"));

        let left: i64 = store
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM model", [], |row| row.get(0))?)
            })
            .expect("读取模型");
        assert_eq!(left, 0, "模型应随渠道级联删除");

        let creds: i64 = store
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM credential", [], |row| row.get(0))?)
            })
            .expect("读取凭证");
        assert_eq!(creds, 0, "凭证应随渠道级联删除");
    }

    #[test]
    fn list_returns_channels_with_models() {
        let (_dir, store) = store();
        save_channel(&store, &input("渠道 A", "custom")).expect("保存失败");
        let mut second = input("渠道 B", "custom");
        second.models = vec![ModelInput {
            alias: "chat-b".to_string(),
            upstream_id: "chat-b".to_string(),
            enabled: true,
        }];
        save_channel(&store, &second).expect("保存失败");

        let list = list_channels(&store).expect("列表失败");
        assert_eq!(list.len(), 2);
        // 列表按 (created_at, id) 排序：同秒创建时顺序不定，按名字找
        let b = list
            .iter()
            .find(|c| c.name == "渠道 B")
            .expect("应有渠道 B");
        assert_eq!(b.models.len(), 1);
        assert_eq!(b.models[0].alias, "chat-b");
    }

    #[test]
    fn meta_json_round_trips_through_channel_summary() {
        let (_dir, store) = store();
        let mut input = input("我的渠道", "custom");
        input.meta.auth_style = Some("custom_header".to_string());
        input.meta.auth_header_name = Some("x-api-key".to_string());
        input.meta.supports_groupid_query = Some(true);
        input.meta.inject_stream_usage = Some(false);
        input.meta.extra_headers.insert("x-foo".to_string(), "bar".to_string());
        let summary = save_channel(&store, &input).expect("保存失败");

        assert_eq!(summary.meta.auth_style.as_deref(), Some("custom_header"));
        assert_eq!(summary.meta.auth_header_name.as_deref(), Some("x-api-key"));
        assert_eq!(summary.meta.supports_groupid_query, Some(true));
        assert_eq!(summary.meta.inject_stream_usage, Some(false));
        assert_eq!(
            summary.meta.extra_headers.get("x-foo").map(String::as_str),
            Some("bar")
        );
    }
}

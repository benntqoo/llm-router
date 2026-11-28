//! 渠道管理 IPC（T8；02 §6.1）。
//!
//! 面向两件事：
//! 1. **预设接入**：把 02 §6.1 的 7 个 profile 作为**纯数据**交给 UI，
//!    用户选中后只需填密钥（默认模型已给出，可改）；
//! 2. **自定义 OpenAI 兼容渠道**：填一张表（base_url / 路径模板 / 鉴权样式 / GroupId …），
//!    保存后立即成为可路由的渠道，别名出现在 `GET /v1/models`。
//!
//! 边界：**明文只有入库那一刻经过这里**，命令不回显密钥（凭证元信息由 `vault_*` 系列负责）。
//! 每个命令都有一份**不依赖 Tauri `State` 的可测版本**（`*_of`），便于单测与 e2e。

use tauri::State;

use crate::providers::presets::PresetView;
use crate::store::channels::{
    self, ChannelInput, ChannelSummary, ModelInput, PublishedModel, CUSTOM_PROVIDER,
};
use crate::vault::crypto::SecretBytes;
use crate::vault::CredentialMeta;
use crate::AppState;

/// 预设目录（02 §6.1 的 7 个 profile）。
#[tauri::command]
pub fn channel_presets() -> Result<Vec<PresetView>, String> {
    Ok(crate::providers::presets::all_views())
}

/// 渠道列表（含模型与"是否已录凭证"）。
#[tauri::command]
pub fn channel_list(state: State<'_, AppState>) -> Result<Vec<ChannelSummary>, String> {
    channel_list_of(&state)
}

/// 渠道列表（可测版本）。
pub fn channel_list_of(state: &AppState) -> Result<Vec<ChannelSummary>, String> {
    channels::list_channels(&state.store).map_err(|e| e.to_string())
}

/// 保存渠道（新建或更新）。
#[tauri::command]
pub fn channel_save(
    state: State<'_, AppState>,
    payload: ChannelInput,
) -> Result<ChannelSummary, String> {
    channel_save_of(&state, payload)
}

/// 保存渠道（可测版本）。
pub fn channel_save_of(state: &AppState, payload: ChannelInput) -> Result<ChannelSummary, String> {
    check_provider(&payload.provider)?;
    channels::save_channel(&state.store, &payload).map_err(|e| e.to_string())
}

/// 删除渠道（连带其模型与凭证）。
#[tauri::command]
pub fn channel_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    channel_delete_of(&state, &id)
}

/// 删除渠道（可测版本）。
pub fn channel_delete_of(state: &AppState, id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("渠道 id 不能为空".to_string());
    }
    channels::delete_channel(&state.store, id).map_err(|e| e.to_string())
}

/// 改渠道的启用状态（T24、02 §6.8.4）。
///
/// **受限渠道（Copilot）启用前必须已确认 ToS**——**后端强制**，
/// 前端的确认弹窗只是「知情」环节，**不是唯一防线**。
///
/// **关闭永远不需要确认**（安全方向不该设门槛）。
#[tauri::command]
pub fn channel_set_enabled(
    state: State<'_, AppState>,
    channel_id: String,
    enabled: bool,
) -> Result<ChannelSummary, String> {
    channel_set_enabled_of(&state, &channel_id, enabled)
}

/// 改启用状态（可测版本）。
pub fn channel_set_enabled_of(
    state: &AppState,
    channel_id: &str,
    enabled: bool,
) -> Result<ChannelSummary, String> {
    channels::set_channel_enabled(&state.store, channel_id, enabled).map_err(|e| e.to_string())
}

/// 给渠道录入密钥（加密后落库；**不回显明文**）。
#[tauri::command]
pub fn channel_put_key(
    state: State<'_, AppState>,
    channel_id: String,
    secret: String,
) -> Result<CredentialMeta, String> {
    channel_put_key_of(&state, &channel_id, &secret)
}

/// 录入密钥（可测版本）。
pub fn channel_put_key_of(
    state: &AppState,
    channel_id: &str,
    secret: &str,
) -> Result<CredentialMeta, String> {
    let channel_id = channel_id.trim();
    if channel_id.is_empty() {
        return Err("渠道 id 不能为空".to_string());
    }
    if secret.trim().is_empty() {
        return Err("密钥不能为空".to_string());
    }
    // 渠道必须存在（避免把密钥挂到不存在的渠道上）
    channels::channel_summary(&state.store, channel_id).map_err(|e| e.to_string())?;

    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let secret_bytes = SecretBytes::from_slice(secret.as_bytes());
    let id = vault
        .put_credential(channel_id, "api_key", &secret_bytes)
        .map_err(|e| e.to_string())?;
    println!("[channel] 已保存渠道 {channel_id} 的凭据 {id}（明文不落日志）");
    vault
        .list_credentials(Some(channel_id))
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| "凭证写入后未能读取元信息".to_string())
}

/// 模型预览（与网关 `GET /v1/models` **同源**）。
#[derive(serde::Serialize, Clone, Debug)]
pub struct ModelsPreview {
    /// 客户端应填的 base_url（`http://127.0.0.1:<port>/v1`）
    pub base_url: String,
    /// 对外发布的别名（= 网关会返回的列表）
    pub models: Vec<PublishedModel>,
}

/// 预览网关当前对外发布的模型别名。
///
/// 数据源与 `GET /v1/models` 是同一份查询（`store::channels::list_published_models`），
/// 所以 UI 里看到的**就是**客户端能调到的。
#[tauri::command]
pub fn models_preview(state: State<'_, AppState>) -> Result<ModelsPreview, String> {
    models_preview_of(&state)
}

/// 模型预览（可测版本）。
pub fn models_preview_of(state: &AppState) -> Result<ModelsPreview, String> {
    let models = channels::list_published_models(&state.store).map_err(|e| e.to_string())?;
    Ok(ModelsPreview {
        base_url: state.gateway_base_url.clone(),
        models,
    })
}

/// 校验 provider：必须是已知预设或 `custom`。
fn check_provider(provider: &str) -> Result<(), String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Err("必须选择预设或自定义渠道".to_string());
    }
    if provider == CUSTOM_PROVIDER {
        return Ok(());
    }
    if crate::providers::presets::find(provider).is_some() {
        return Ok(());
    }
    Err(format!(
        "未知的渠道预设 {provider:?}（可选：{} 或 {CUSTOM_PROVIDER}）",
        crate::providers::presets::all()
            .iter()
            .map(|p| p.provider)
            .collect::<Vec<_>>()
            .join(" / ")
    ))
}

/// 便捷：由预设生成一份渠道入参（UI 与测试共用）。
///
/// 预设已经把 base_url / 路径 / 鉴权样式 / 默认模型都填好了——用户只剩"填密钥"。
pub fn input_from_preset(
    preset: &PresetView,
    name: &str,
    models: Vec<ModelInput>,
) -> ChannelInput {
    ChannelInput {
        id: None,
        name: name.to_string(),
        provider: preset.provider.clone(),
        base_url: preset.base_url.clone(),
        kind: Some(preset.kind.clone()),
        enabled: true,
        priority: None,
        meta: channels::ChannelMeta {
            path_chat: Some(preset.chat_path.clone()),
            auth_style: Some(preset.auth_style.clone()),
            auth_header_name: None,
            group_id: None,
            supports_groupid_query: Some(preset.supports_groupid_query),
            inject_stream_usage: Some(preset.inject_stream_usage),
            extra_headers: Default::default(),
            // T24：ToS 确认记录**不来自表单**（服务端管理，02 §6.8.3）
            tos_accepted_at: None,
            tos_statement_version: None,
        },
        models: if models.is_empty() {
            preset
                .default_models
                .iter()
                .map(|m| ModelInput {
                    alias: m.alias.clone(),
                    upstream_id: m.upstream_id.clone(),
                    enabled: true,
                })
                .collect()
        } else {
            models
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use crate::store::Store;
    use crate::vault::Vault;

    fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
        let vault = Arc::new(RwLock::new(
            Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
        ));
        let state = AppState {
            gateway_addr: "127.0.0.1:8787".to_string(),
            gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
            store,
            vault,
            limiter: Arc::new(crate::gateway::ratelimit::RateLimiter::new()),
            circuit: Arc::new(crate::gateway::circuit::CircuitBreaker::new()),
        };
        (dir, state)
    }

    #[test]
    fn presets_expose_seven_profiles() {
        let presets = channel_presets().expect("预设目录");
        assert_eq!(presets.len(), 7);
        assert!(presets.iter().all(|p| !p.default_models.is_empty()));
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let (_dir, state) = state();
        let payload = ChannelInput {
            name: "渠道".to_string(),
            provider: "not-a-provider".to_string(),
            base_url: "https://example.com".to_string(),
            models: vec![ModelInput {
                alias: "a".to_string(),
                upstream_id: "a".to_string(),
                enabled: true,
            }],
            ..ChannelInput::default()
        };
        let err = channel_save_of(&state, payload).expect_err("未知预设应报错");
        assert!(err.contains("未知的渠道预设"), "实际 {err}");
    }

    #[test]
    fn custom_provider_is_accepted() {
        let (_dir, state) = state();
        let payload = ChannelInput {
            name: "我的代理".to_string(),
            provider: CUSTOM_PROVIDER.to_string(),
            base_url: "https://proxy.example.com".to_string(),
            models: vec![ModelInput {
                alias: "my-alias".to_string(),
                upstream_id: "my-model".to_string(),
                enabled: true,
            }],
            ..ChannelInput::default()
        };
        let summary = channel_save_of(&state, payload).expect("保存失败");
        assert_eq!(summary.provider, CUSTOM_PROVIDER);
        assert_eq!(summary.models[0].alias, "my-alias");
    }

    #[test]
    fn put_key_attaches_credential_to_the_channel() {
        let (_dir, state) = state();
        let payload = ChannelInput {
            name: "我的渠道".to_string(),
            provider: CUSTOM_PROVIDER.to_string(),
            base_url: "https://proxy.example.com".to_string(),
            models: vec![ModelInput {
                alias: "a".to_string(),
                upstream_id: "a".to_string(),
                enabled: true,
            }],
            ..ChannelInput::default()
        };
        let summary = channel_save_of(&state, payload).expect("保存失败");
        assert!(!summary.has_credential);

        let meta = channel_put_key_of(&state, &summary.id, "sk-secret").expect("录入密钥失败");
        assert_eq!(meta.channel_id, summary.id);
        assert_eq!(meta.kind, "api_key");

        let list = channel_list_of(&state).expect("列表失败");
        assert!(list[0].has_credential, "列表应显示已录凭证");
    }

    #[test]
    fn put_key_rejects_unknown_channel_and_empty_secret() {
        let (_dir, state) = state();
        assert!(channel_put_key_of(&state, "ch-not-exist", "sk-x").is_err());
        assert!(channel_put_key_of(&state, "ch-not-exist", "  ").is_err());
    }

    #[test]
    fn delete_removes_the_channel() {
        let (_dir, state) = state();
        let payload = ChannelInput {
            name: "临时渠道".to_string(),
            provider: CUSTOM_PROVIDER.to_string(),
            base_url: "https://proxy.example.com".to_string(),
            models: vec![ModelInput {
                alias: "a".to_string(),
                upstream_id: "a".to_string(),
                enabled: true,
            }],
            ..ChannelInput::default()
        };
        let summary = channel_save_of(&state, payload).expect("保存失败");
        assert!(channel_delete_of(&state, &summary.id).expect("删除失败"));
        assert!(channel_list_of(&state).expect("列表失败").is_empty());
    }

    #[test]
    fn models_preview_lists_published_aliases() {
        let (_dir, state) = state();
        let payload = ChannelInput {
            name: "DeepSeek 官方".to_string(),
            provider: "deepseek".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            models: vec![ModelInput {
                alias: "deepseek-chat".to_string(),
                upstream_id: "deepseek-chat".to_string(),
                enabled: true,
            }],
            ..ChannelInput::default()
        };
        channel_save_of(&state, payload).expect("保存失败");

        let preview = models_preview_of(&state).expect("预览失败");
        assert_eq!(preview.base_url, "http://127.0.0.1:8787/v1");
        assert_eq!(preview.models.len(), 1);
        assert_eq!(preview.models[0].alias, "deepseek-chat");
        assert_eq!(preview.models[0].owned_by, "DeepSeek 官方");
    }

    #[test]
    fn input_from_preset_fills_defaults() {
        let presets = channel_presets().expect("预设目录");
        let deepseek = presets
            .iter()
            .find(|p| p.provider == "deepseek")
            .expect("应有 deepseek 预设");
        let input = input_from_preset(deepseek, "DeepSeek 官方", Vec::new());
        assert_eq!(input.base_url, "https://api.deepseek.com");
        assert_eq!(input.meta.path_chat.as_deref(), Some("/v1/chat/completions"));
        assert_eq!(input.models.len(), 1);
        assert_eq!(input.models[0].alias, "deepseek-chat");
    }
}

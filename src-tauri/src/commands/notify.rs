//! 通知配置 IPC（T19；02 §7.5）。
//!
//! 三个动作：**读配置** / **保存配置** / **发一条测试通知**。
//!
//! **密文红线**：webhook 地址与 bot token **只进保险库**（`kind='webhook'`），
//! 设置表只存凭据 id；IPC **不回显地址**（只回「配没配」）。

use tauri::State;

use crate::checkin::notify::{
    self, ChannelResult, NotifyConfig, NotifyConfigInput, Notification,
};
use crate::AppState;

/// 读通知配置。
#[tauri::command]
pub fn notify_get_config(state: State<'_, AppState>) -> Result<NotifyConfig, String> {
    notify_get_config_of(&state)
}

/// 读通知配置（可测版本）。
pub fn notify_get_config_of(state: &AppState) -> Result<NotifyConfig, String> {
    let settings = notify::load_settings(&state.store);
    Ok(notify::config_view(&settings))
}

/// 保存通知配置（**webhook 地址进保险库**）。
#[tauri::command]
pub fn notify_set_config(
    state: State<'_, AppState>,
    input: NotifyConfigInput,
) -> Result<NotifyConfig, String> {
    notify_set_config_of(&state, &input)
}

/// 保存通知配置（可测版本）。
pub fn notify_set_config_of(
    state: &AppState,
    input: &NotifyConfigInput,
) -> Result<NotifyConfig, String> {
    notify::save_settings(&state.store, &state.vault, input)
}

/// 发一条测试通知（**不受总开关限制**——用户点了就是要试通道）。
#[tauri::command]
pub async fn notify_test(state: State<'_, AppState>) -> Result<Vec<ChannelResult>, String> {
    notify_test_of(&state).await
}

/// 发一条测试通知（可测版本）。
pub async fn notify_test_of(state: &AppState) -> Result<Vec<ChannelResult>, String> {
    let settings = notify::load_settings(&state.store);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|err| format!("构造 HTTP 客户端失败：{err}"))?;
    let notification = Notification {
        title: "LLM Router 通知测试".to_string(),
        body: "这是一条测试通知：收到它说明通知通道可用。\n站点：测试\n分类：test".to_string(),
    };
    Ok(notify::send(&client, &settings, &state.vault, &notification).await)
}

//! 网关运行状态查询：UI 通过 IPC 读取本地网关地址与 base_url。

use serde::Serialize;
use tauri::State;

use crate::AppState;

/// 网关状态。
#[derive(Serialize, Clone, Debug)]
pub struct GatewayStatus {
    /// 网关是否已监听
    pub running: bool,
    /// 实际监听地址，如 `127.0.0.1:8787`
    pub addr: String,
    /// 客户端应填的 base_url，如 `http://127.0.0.1:8787/v1`
    pub base_url: String,
    /// 当前实现范围提示
    pub note: String,
}

/// 返回网关运行状态。
#[tauri::command]
pub fn gateway_status(state: State<'_, AppState>) -> GatewayStatus {
    GatewayStatus {
        running: true,
        addr: state.gateway_addr.clone(),
        base_url: state.gateway_base_url.clone(),
        note: "T3 已接入：OpenAI 入站 + client key 鉴权 + Host 校验 + 限流；别名路由与出站适配在 T4/T5 落地".to_string(),
    }
}

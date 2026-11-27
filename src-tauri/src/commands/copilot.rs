//! GitHub Copilot 的 OAuth IPC（T20；02 §6.2 / §6.2.1）。
//!
//! 三个动作：**发起设备授权** / **轮询授权结果** / **查状态 + 断开连接**。
//!
//! **密文红线**：`device_code` 与两个 token **都不出后端**——
//! IPC 只回 `flow_id` / `user_code` / `verification_uri` / 状态 / 过期时间。

use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::providers::oauth::copilot::{
    self, DeviceFlowView, Endpoints, PollOutcome, DEFAULT_API_BASE,
};
use crate::store::channels::{ensure_oauth_channel, OAUTH_PROVIDER};
use crate::AppState;

/// Copilot 渠道名（**唯一**）。
pub const CHANNEL_NAME: &str = "GitHub Copilot";

/// 授权状态（IPC 视图）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotPollView {
    /// `pending` / `slow_down` / `expired` / `denied` / `authorized`
    pub state: String,
    /// 建议**下次**轮询前等待的秒数。
    pub interval: u64,
    /// Copilot token 的过期时刻（Unix 秒；未授权时为 `None`）。
    pub expires_at: Option<i64>,
    /// 该账号实际要用的 API 域名。
    pub api_base: Option<String>,
}

/// 授权状态总览（IPC 视图）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotStatusView {
    /// 是否已授权（保险库里有没有 GitHub token）。
    pub authorized: bool,
    /// Copilot token 的过期时刻。
    pub expires_at: Option<i64>,
    /// 该账号实际要用的 API 域名。
    pub api_base: Option<String>,
    /// 渠道 id（**没有渠道** = 还没发起过授权）。
    pub channel_id: Option<String>,
    /// 渠道是否已启用（**默认关**；启用开关属 T24）。
    pub channel_enabled: bool,
}

/// 发起设备授权（①）。
#[tauri::command]
pub async fn copilot_authorize_start(
    state: State<'_, AppState>,
) -> Result<DeviceFlowView, String> {
    copilot_authorize_start_of(&state, &Endpoints::default()).await
}

/// 发起设备授权（可测版本：端点可注入 → 测试指向本地 mock，**不出网**）。
pub async fn copilot_authorize_start_of(
    state: &AppState,
    endpoints: &Endpoints,
) -> Result<DeviceFlowView, String> {
    let client = http_client()?;
    let flow = copilot::start_device_flow(&client, endpoints).await?;
    let flow_id = copilot::flows().insert(flow.clone());
    // 渠道行先建好（**默认关闭**）：UI 能看到「有一个 Copilot 渠道，但还没授权」
    let _ = ensure_oauth_channel(
        &state.store,
        CHANNEL_NAME,
        OAUTH_PROVIDER,
        DEFAULT_API_BASE,
    );
    Ok(DeviceFlowView::of(&flow_id, &flow))
}

/// 轮询授权结果（③）；成功时顺手完成 ④（换 Copilot token 并入库）。
#[tauri::command]
pub async fn copilot_authorize_poll(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<CopilotPollView, String> {
    copilot_authorize_poll_of(&state, &flow_id, &Endpoints::default()).await
}

/// 轮询授权结果（可测版本：端点可注入 → 测试指向本地 mock，**不出网**）。
pub async fn copilot_authorize_poll_of(
    state: &AppState,
    flow_id: &str,
    endpoints: &Endpoints,
) -> Result<CopilotPollView, String> {
    let flow = copilot::flows()
        .get(flow_id)
        .ok_or_else(|| "授权会话已过期或不存在——请重新发起设备授权".to_string())?;
    let client = http_client()?;
    let outcome = copilot::poll_device_flow(&client, endpoints, &flow.device_code).await?;
    match outcome {
        PollOutcome::Pending => Ok(CopilotPollView {
            state: "pending".to_string(),
            interval: flow.interval,
            expires_at: None,
            api_base: None,
        }),
        PollOutcome::SlowDown => Ok(CopilotPollView {
            state: "slow_down".to_string(),
            // GitHub 要求：轮询太快要**放慢**（+5s）
            interval: flow.interval + 5,
            expires_at: None,
            api_base: None,
        }),
        PollOutcome::Expired => {
            copilot::flows().remove(flow_id);
            Ok(CopilotPollView {
                state: "expired".to_string(),
                interval: flow.interval,
                expires_at: None,
                api_base: None,
            })
        }
        PollOutcome::Denied => {
            copilot::flows().remove(flow_id);
            Ok(CopilotPollView {
                state: "denied".to_string(),
                interval: flow.interval,
                expires_at: None,
                api_base: None,
            })
        }
        PollOutcome::Authorized(gh_token) => {
            // 会话用完即焚（`device_code` 不再留在内存）
            copilot::flows().remove(flow_id);
            let channel_id = ensure_oauth_channel(
                &state.store,
                CHANNEL_NAME,
                OAUTH_PROVIDER,
                DEFAULT_API_BASE,
            )
            .map_err(|err| err.to_string())?;
            {
                let vault = state.vault.read().map_err(|err| err.to_string())?;
                copilot::store_github_token(&vault, &channel_id, &gh_token)?;
            }
            let token = copilot::fetch_copilot_token(&client, endpoints, &gh_token).await?;
            {
                let vault = state.vault.read().map_err(|err| err.to_string())?;
                copilot::store_copilot_token(&vault, &channel_id, &token)?;
            }
            copilot::cache().put(token.clone());
            // ④ 的响应可能带 `endpoints.api` → 渠道的出站域名以它为准
            let _ = ensure_oauth_channel(&state.store, CHANNEL_NAME, OAUTH_PROVIDER, &token.api_base);
            Ok(CopilotPollView {
                state: "authorized".to_string(),
                interval: 0,
                expires_at: Some(token.expires_at),
                api_base: Some(token.api_base),
            })
        }
    }
}

/// 查授权状态（**只有布尔与过期时间，没有 token**）。
#[tauri::command]
pub fn copilot_status(state: State<'_, AppState>) -> Result<CopilotStatusView, String> {
    copilot_status_of(&state)
}

/// 查授权状态（可测版本）。
pub fn copilot_status_of(state: &AppState) -> Result<CopilotStatusView, String> {
    let Some((channel_id, enabled, base_url)) = find_copilot_channel(state)? else {
        return Ok(CopilotStatusView {
            authorized: false,
            expires_at: None,
            api_base: None,
            channel_id: None,
            channel_enabled: false,
        });
    };
    let vault = state.vault.read().map_err(|err| err.to_string())?;
    let authorized = copilot::load_github_token(&vault, &channel_id)?.is_some();
    let expires_at = copilot::copilot_token_meta(&vault, &channel_id)?.and_then(|(_, at)| at);
    let api_base = copilot::cache()
        .peek()
        .map(|token| token.api_base)
        .or(Some(base_url));
    Ok(CopilotStatusView {
        authorized,
        expires_at,
        api_base,
        channel_id: Some(channel_id),
        channel_enabled: enabled,
    })
}

/// 断开连接：删掉两条凭据 + 清内存缓存。
#[tauri::command]
pub fn copilot_disconnect(state: State<'_, AppState>) -> Result<bool, String> {
    copilot_disconnect_of(&state)
}

/// 断开连接（可测版本）。
pub fn copilot_disconnect_of(state: &AppState) -> Result<bool, String> {
    let Some((channel_id, _, _)) = find_copilot_channel(state)? else {
        return Ok(false);
    };
    let vault = state.vault.read().map_err(|err| err.to_string())?;
    copilot::disconnect(&vault, &channel_id)?;
    Ok(true)
}

/// 找 Copilot 渠道（**没有就返回 None**——不自动建，免得只查状态就产生一行空渠道）。
fn find_copilot_channel(state: &AppState) -> Result<Option<(String, bool, String)>, String> {
    let rows = crate::store::channels::list_channels(&state.store).map_err(|err| err.to_string())?;
    Ok(rows
        .into_iter()
        .find(|row| row.provider == OAUTH_PROVIDER || row.name == CHANNEL_NAME)
        .map(|row| (row.id, row.enabled, row.base_url)))
}

/// 出站 HTTP 客户端（**只给 OAuth 端点用**；超时 30 s）。
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| format!("构造 HTTP 客户端失败：{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use crate::gateway::circuit::CircuitBreaker;
    use crate::gateway::ratelimit::RateLimiter;
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
            limiter: Arc::new(RateLimiter::new()),
            circuit: Arc::new(CircuitBreaker::new()),
        };
        (dir, state)
    }

    /// 走完 ① → ③ → ④（**全程打本地 mock，零出网**）。
    async fn authorize(state: &AppState, endpoints: &Endpoints) -> DeviceFlowView {
        let view = copilot_authorize_start_of(state, endpoints)
            .await
            .expect("① 发起设备授权");
        // ③ 轮询：pending → slow_down → authorized
        let first = copilot_authorize_poll_of(state, &view.flow_id, endpoints)
            .await
            .expect("③ 轮询一次");
        assert_eq!(first.state, "pending");
        let second = copilot_authorize_poll_of(state, &view.flow_id, endpoints)
            .await
            .expect("③ 轮询两次");
        assert_eq!(second.state, "slow_down");
        let third = copilot_authorize_poll_of(state, &view.flow_id, endpoints)
            .await
            .expect("③ 轮询三次");
        assert_eq!(third.state, "authorized");
        view
    }

    #[tokio::test]
    async fn the_device_flow_runs_end_to_end_against_a_local_mock() {
        let (base, mock) = copilot::testing::start().await;
        let endpoints = copilot::Endpoints::local(&base);
        let (_dir, state) = state();

        // ① 发起设备授权
        let view = copilot_authorize_start_of(&state, &endpoints)
            .await
            .expect("① 发起设备授权");
        assert_eq!(view.user_code, "ABCD-1234");
        assert_eq!(view.interval, 5);
        assert_eq!(view.expires_in, 900);

        // **device_code 不出后端**（它能换出 token）
        let json = serde_json::to_string(&view).expect("序列化");
        assert!(!json.contains("MOCK-DEVICE-CODE"), "{json}");
        assert!(!json.contains("device_code"), "{json}");

        // ① 请求体：client_id + scope
        let body = mock.device_code_body();
        assert!(body.contains("client_id=Iv1.b507a08c87ecfe98"), "{body}");
        assert!(body.contains("scope=read%3Auser"), "{body}");

        // 渠道行建好，但**默认关闭**（AGENT.md §5.5）
        let channels = crate::store::channels::list_channels(&state.store).expect("列渠道");
        let channel = channels
            .iter()
            .find(|row| row.provider == OAUTH_PROVIDER)
            .expect("Copilot 渠道行");
        assert!(!channel.enabled, "Copilot 渠道必须默认关闭");
        assert_eq!(channel.kind, "oauth");

        // ③ 轮询状态机
        let first = copilot_authorize_poll_of(&state, &view.flow_id, &endpoints)
            .await
            .expect("③ 轮询一次");
        assert_eq!(first.state, "pending");
        assert_eq!(first.interval, 5, "pending 用后端给的间隔");
        let second = copilot_authorize_poll_of(&state, &view.flow_id, &endpoints)
            .await
            .expect("③ 轮询两次");
        assert_eq!(second.state, "slow_down");
        assert_eq!(second.interval, 10, "slow_down 要 +5s");

        let third = copilot_authorize_poll_of(&state, &view.flow_id, &endpoints)
            .await
            .expect("③ 轮询三次");
        assert_eq!(third.state, "authorized");
        assert!(third.expires_at.is_some(), "授权成功要带回过期时间");
        assert_eq!(
            third.api_base.as_deref(),
            Some("https://api.githubcopilot.com"),
            "④ 响应里的 endpoints.api 要生效"
        );

        // ③ 请求体：device_code + device_code 授权类型
        let bodies = mock.poll_bodies().join("\n");
        assert_eq!(mock.poll_count(), 3, "轮询了三次");
        assert!(bodies.contains("client_id=Iv1.b507a08c87ecfe98"), "{bodies}");
        assert!(bodies.contains("device_code=MOCK-DEVICE-CODE"), "{bodies}");
        assert!(
            bodies.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"),
            "{bodies}"
        );

        // ④ 用的是 GitHub token
        assert!(
            mock.auth_header().contains("MOCK-GH-TOKEN"),
            "{}",
            mock.auth_header()
        );

        // 状态：已授权 + 渠道默认关
        let status = copilot_status_of(&state).expect("查状态");
        assert!(status.authorized);
        assert!(!status.channel_enabled, "渠道仍然默认关闭");
        assert!(status.channel_id.is_some());
        assert!(status.expires_at.is_some());

        // 两条凭据都在保险库里（**密文**），copilot token 带过期时间
        let channel_id = status.channel_id.expect("渠道 id");
        let vault = state.vault.read().expect("读保险库");
        let metas = vault.list_credentials(Some(&channel_id)).expect("列凭据");
        let kinds: Vec<&str> = metas.iter().map(|m| m.kind.as_str()).collect();
        assert!(
            kinds.contains(&copilot::GH_TOKEN_KIND),
            "缺 GitHub token：{kinds:?}"
        );
        assert!(
            kinds.contains(&copilot::COPILOT_TOKEN_KIND),
            "缺 Copilot token：{kinds:?}"
        );
        let copilot_meta = metas
            .iter()
            .find(|m| m.kind == copilot::COPILOT_TOKEN_KIND)
            .expect("Copilot token 元信息");
        assert!(copilot_meta.expires_at.is_some(), "Copilot token 要带过期时间");
        let gh_meta = metas
            .iter()
            .find(|m| m.kind == copilot::GH_TOKEN_KIND)
            .expect("GitHub token 元信息");
        assert!(gh_meta.expires_at.is_none(), "GitHub token 是长期的");
    }

    #[tokio::test]
    async fn the_secrets_never_land_in_plaintext_on_disk() {
        let (base, _mock) = copilot::testing::start().await;
        let endpoints = copilot::Endpoints::local(&base);
        let (dir, state) = state();

        authorize(&state, &endpoints).await;

        // 密文红线（AGENT.md §5.3）：库里与保险库文件里都**不该**出现明文 token
        for name in ["store.db", "vault.bin"] {
            let blob = std::fs::read(dir.path().join(name)).expect("读文件");
            let text = String::from_utf8_lossy(&blob);
            assert!(!text.contains("MOCK-GH-TOKEN"), "{name} 里出现了 GitHub token 明文");
            assert!(
                !text.contains("MOCK-COPILOT-TOKEN"),
                "{name} 里出现了 Copilot token 明文"
            );
            assert!(!text.contains("MOCK-DEVICE-CODE"), "{name} 里出现了 device_code 明文");
        }
    }

    #[tokio::test]
    async fn disconnect_removes_both_credentials() {
        let (base, _mock) = copilot::testing::start().await;
        let endpoints = copilot::Endpoints::local(&base);
        let (_dir, state) = state();

        authorize(&state, &endpoints).await;
        assert!(copilot_status_of(&state).expect("查状态").authorized);

        assert!(copilot_disconnect_of(&state).expect("断开"));
        let status = copilot_status_of(&state).expect("查状态");
        assert!(!status.authorized, "断开后不该还是已授权");

        let channel_id = status.channel_id.expect("渠道 id");
        let vault = state.vault.read().expect("读保险库");
        assert!(
            vault
                .list_credentials(Some(&channel_id))
                .expect("列凭据")
                .is_empty(),
            "断开后不该留下孤儿凭据"
        );
    }

    #[tokio::test]
    async fn polling_an_unknown_flow_id_fails_without_touching_the_network() {
        let (_dir, state) = state();
        let err = copilot_authorize_poll_of(&state, "flow_不存在", &Endpoints::default())
            .await
            .expect_err("不存在的会话应当报错");
        assert!(err.contains("重新发起设备授权"), "{err}");
    }

    #[tokio::test]
    async fn the_copilot_token_failures_are_readable() {
        let (base, _mock) = copilot::testing::start().await;
        let client = reqwest::Client::new();

        let unauthorized = copilot::Endpoints {
            device_code_url: format!("{base}/login/device/code"),
            access_token_url: format!("{base}/login/oauth/access_token"),
            copilot_token_url: format!("{base}/bad-token/copilot_internal/v2/token"),
        };
        let err = copilot::fetch_copilot_token(&client, &unauthorized, "gh")
            .await
            .expect_err("401 应当报错");
        assert!(err.contains("无效"), "{err}");

        let forbidden = copilot::Endpoints {
            device_code_url: format!("{base}/login/device/code"),
            access_token_url: format!("{base}/login/oauth/access_token"),
            copilot_token_url: format!("{base}/no-subscription/copilot_internal/v2/token"),
        };
        let err = copilot::fetch_copilot_token(&client, &forbidden, "gh")
            .await
            .expect_err("403 应当报错");
        assert!(err.contains("订阅"), "{err}");
    }

    #[tokio::test]
    async fn the_local_endpoints_keep_the_real_github_paths() {
        let endpoints = copilot::Endpoints::local("http://127.0.0.1:1234/");
        assert_eq!(
            endpoints.device_code_url,
            "http://127.0.0.1:1234/login/device/code"
        );
        assert_eq!(
            endpoints.access_token_url,
            "http://127.0.0.1:1234/login/oauth/access_token"
        );
        assert_eq!(
            endpoints.copilot_token_url,
            "http://127.0.0.1:1234/copilot_internal/v2/token"
        );

        // 生产默认仍是真实 GitHub
        let default = Endpoints::default();
        assert_eq!(default.device_code_url, copilot::DEVICE_CODE_URL);
        assert_eq!(default.access_token_url, copilot::ACCESS_TOKEN_URL);
        assert_eq!(default.copilot_token_url, copilot::COPILOT_TOKEN_URL);
    }
}

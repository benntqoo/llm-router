//! GitHub Copilot 的 OAuth 凭证层（T20；02 §6.2 / §6.2.1）。
//!
//! **只取凭证，不出站**（出站转发与 R-6 伪装头属 T21）。
//!
//! 四步（① ③ 是 **GitHub 官方** device flow；④ 是**社区观察到的内部端点**）：
//!
//! 1. `POST https://github.com/login/device/code` → `device_code` / `user_code` / `verification_uri`
//! 2. 用户**自己在浏览器**输入 `user_code` 并确认
//! 3. 轮询 `POST https://github.com/login/oauth/access_token` → GitHub token（`ghu_`，长期）
//! 4. `GET https://api.github.com/copilot_internal/v2/token` → Copilot token（~25~30 min）
//!
//! **密文边界（AGENT.md §5.3）**：
//! - GitHub token 与 Copilot token **只进保险库**（`kind` 分别是 `oauth_gh_token` /
//!   `oauth_copilot_token`），**不回显、不进日志、不进 IPC**；
//! - `device_code`（能换出 token 的凭据）**不出后端**——IPC 只回 `flow_id` + `user_code`
//!   + `verification_uri`。
//!
//! **风险**：④ 的端点是**社区观察到的内部端点**，GitHub **未公开承诺稳定** → 可能随时变更。
//!
//! **合规**：走 GitHub **官方** device flow；**不做**指纹伪造 / 验证码绕过 / 签名伪造
//! （AGENT.md §5.7）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use serde::Serialize;

use crate::vault::crypto::SecretBytes;
use crate::vault::Vault;

/// VS Code Copilot 的**公开** client_id（**不是密钥**，用户无须填）。
///
/// 社区实现一致（opencode / corvus / xcodeai / zeroclaw / synaps-core）。
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// 最小权限 scope。
pub const SCOPE: &str = "read:user";

/// ① 取设备码。
pub const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
/// ③ 轮询授权结果。
pub const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
/// ④ 换取 Copilot token（**社区观察到的内部端点**）。
pub const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// OAuth 端点集合（**生产默认指向真实 GitHub**；测试指向本地 mock —— **不出网**）。
///
/// 为什么做成可注入：e2e 既要**不碰真实 GitHub**（测试纪律 + 密文红线），
/// 又要**真的走一遍** device flow 的请求与解析 → **只换端点，不换代码路径**。
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// ① 取设备码。
    pub device_code_url: String,
    /// ③ 轮询授权结果。
    pub access_token_url: String,
    /// ④ 换 Copilot token。
    pub copilot_token_url: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            device_code_url: DEVICE_CODE_URL.to_string(),
            access_token_url: ACCESS_TOKEN_URL.to_string(),
            copilot_token_url: COPILOT_TOKEN_URL.to_string(),
        }
    }
}

impl Endpoints {
    /// 三个端点都指向**同一个本地 mock 上游**（**测试用**）。
    ///
    /// 路径与真实 GitHub **保持一致** → mock 按路径分发即可。
    pub fn local(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            device_code_url: format!("{base}/login/device/code"),
            access_token_url: format!("{base}/login/oauth/access_token"),
            copilot_token_url: format!("{base}/copilot_internal/v2/token"),
        }
    }
}

/// 出站 API 默认域名（④ 的响应里若带 `endpoints.api`，**以它为准**）。
pub const DEFAULT_API_BASE: &str = "https://api.githubcopilot.com";

/// GitHub 没给 `interval` 时的兜底轮询间隔（秒）。
pub const DEFAULT_INTERVAL_SECS: u64 = 5;
/// GitHub 没给 `expires_in` 时的兜底有效期（秒）。
pub const DEFAULT_EXPIRES_IN_SECS: u64 = 900;
/// 过期前多久就重取（秒）——02 §6.2「过期前 5min 自动重取」。
pub const REFRESH_SKEW_SECS: i64 = 300;

/// 保险库里 GitHub token 的 `credential.kind`。
pub const GH_TOKEN_KIND: &str = "oauth_gh_token";
/// 保险库里 Copilot token 的 `credential.kind`。
pub const COPILOT_TOKEN_KIND: &str = "oauth_copilot_token";

/// 设备授权请求的 User-Agent（GitHub 对无 UA 的请求会拒）。
const USER_AGENT: &str = "GitHubCopilotChat/0.26.7";

/// 可选：**覆盖**内置的 `github.com` 候选 IP 池（逗号分隔，如 `1.2.3.4,5.6.7.8`）。
///
/// 边缘 IP **会变** → 变了用这个环境变量顶，**不用改代码**。
pub const ENV_GITHUB_IPS: &str = "LLMR_GITHUB_IPS";

/// `github.com` 的**候选 IP 池**（公网边缘地址；本机实测能服务 OAuth POST 的优先在前）。
///
/// **为什么是「池 + 轮换」而不是「钉一个」**（本机实测 2026-11-27，详见 02 §6.2.1）：
/// - 同一个 IP 的成功率**随时间翻转**：`20.27.177.113` 21:15 时 5/5 通、21:47 时 0/5；
/// - **失败模式因请求方法而异**：`20.200.245.247` 的 `GET /login/device` 5/5 通，
///   但 `POST /login/oauth/access_token` **0/3**；
/// - 系统 DNS 连续 16 次解析**全是同一个 A 记录**（ISP 侧有缓存）→ 动态发现候选**不可行**；
/// - 钉死单个 IP 会把设备码的 **900 s** 寿命全耗在一条坏路径上。
///
/// **兜底**：`ClientPool` 的**最后一个 client 走普通 DNS** → 池内 IP 全失效时仍有一条路。
const GITHUB_IP_POOL: [&str; 15] = [
    // ── 2026-11-27 22:0x 实测 POST `/login/oauth/access_token` 2/2 通的 ──
    "20.205.243.166",
    "20.200.245.249",
    "57.155.223.240",
    "20.247.166.64",
    "20.27.177.113",
    // ── 同族的其他边缘地址（当时不通，但**好坏会随时间翻转**，轮换才撞得到）──
    "20.205.243.168",
    "20.200.245.245",
    "20.200.245.247",
    "20.200.245.250",
    "20.27.177.116",
    "20.201.28.148",
    "20.201.28.151",
    "20.26.156.210",
    "20.26.156.215",
    "20.248.137.48",
];

/// 设备授权会话（① 的响应）。
#[derive(Debug, Clone)]
pub struct DeviceFlow {
    /// **机密**：能换出 token 的凭据，**只留在后端内存**。
    pub device_code: String,
    /// 给用户看的短码（**可以**进 IPC）。
    pub user_code: String,
    /// 用户要打开的验证网址。
    pub verification_uri: String,
    /// 建议轮询间隔（秒）。
    pub interval: u64,
    /// 设备码有效期（秒）。
    pub expires_in: u64,
}

/// ③ 轮询的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// 用户还没在浏览器确认 → 等 `interval` 秒再轮询。
    Pending,
    /// 轮询太快 → 间隔 **+5 s**。
    SlowDown,
    /// 设备码过期（`expires_in` 用完）→ 让用户重新发起。
    Expired,
    /// 用户拒绝授权。
    Denied,
    /// 授权成功（**长期** GitHub token）。
    Authorized(String),
}

/// ④ 换到的 Copilot token。
#[derive(Debug, Clone)]
pub struct CopilotToken {
    /// 短期 Copilot token（~25~30 min）。
    pub token: String,
    /// 过期时刻（Unix 秒）。
    pub expires_at: i64,
    /// 该账号实际要用的 API 域名。
    pub api_base: String,
}

/// 构造一个出站 HTTP 客户端（**只给 OAuth 端点用**）。
///
/// - `connect_timeout` **3 s**、单请求总超时 **8 s**：GitHub OAuth 正常 < 2 s
///   → 失败一次代价小，能在设备码寿命（900 s）内多试很多次；
/// - `pin` 为 `Some` 时把 `github.com` **钉到该 IP**（见 [`ClientPool`]）。
fn build_client(pin: Option<std::net::SocketAddr>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(8));
    if let Some(addr) = pin {
        builder = builder.resolve("github.com", addr);
    }
    builder
        .build()
        .map_err(|err| format!("构造 HTTP 客户端失败：{err}"))
}

/// 候选 IP 列表：环境变量优先，否则用内置池。
fn candidate_ips() -> Vec<String> {
    candidate_ips_from(std::env::var(ENV_GITHUB_IPS).ok())
}

/// 从环境变量的原始值解析候选 IP（**拆出来是为了单测不用改进程级环境变量**）。
fn candidate_ips_from(raw: Option<String>) -> Vec<String> {
    match raw {
        Some(text) => text
            .split(',')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        None => GITHUB_IP_POOL.iter().map(|item| item.to_string()).collect(),
    }
}

/// 出站客户端池：每个候选 IP 一个 client，**轮换**使用。
///
/// **为什么要池**（本机实测 2026-11-27，详见 02 §6.2.1）：本机到 `github.com` 的链路
/// 时好时坏，钉死单个 IP 会把设备码的 900 s 寿命全耗在一条坏路径上。
///
/// **兜底**：池子**最后一个 client 走普通 DNS** → 池内 IP 全部失效时仍有一条路。
pub struct ClientPool {
    clients: Vec<reqwest::Client>,
    next: std::sync::atomic::AtomicUsize,
}

impl ClientPool {
    /// 构造池子：候选 IP 各一个 client，**末尾追加一个走普通 DNS 的**（兜底）。
    fn build() -> Result<Self, String> {
        let mut clients = Vec::new();
        for raw in candidate_ips() {
            let Ok(ip) = raw.parse::<std::net::IpAddr>() else {
                continue;
            };
            let addr = std::net::SocketAddr::new(ip, 443);
            if let Ok(client) = build_client(Some(addr)) {
                clients.push(client);
            }
        }
        // 兜底 client：走普通 DNS（池内 IP 全失效时仍有一条路）
        clients.push(build_client(None)?);
        Ok(Self {
            clients,
            next: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// 取下一个 client（**轮换**：每次重试换一个候选 IP）。
    fn next_client(&self) -> &reqwest::Client {
        let len = self.clients.len().max(1);
        let idx = self
            .next
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % len;
        &self.clients[idx]
    }

    /// 池子大小（**含**末尾那个走普通 DNS 的兜底 client）。
    pub fn len(&self) -> usize {
        self.clients.len()
    }
}

static POOL: OnceLock<Result<ClientPool, String>> = OnceLock::new();

/// 全局客户端池（进程内复用，避免每次请求都重建 TLS 配置）。
pub fn pool() -> Result<&'static ClientPool, String> {
    POOL.get_or_init(|| ClientPool::build())
        .as_ref()
        .map_err(|err| err.clone())
}

/// 发一次请求；**网络层失败（超时 / 连不上）时重试**。
///
/// **为什么需要**：见 [`ClientPool`]——本机到 `github.com` 的链路时好时坏，
/// 命中坏路径时单次请求必然超时。GitHub 的三个端点操作都是**幂等**的
/// （取设备码 / 轮询 / 换 token）→ **重试安全**。
///
/// **每次重试换一个候选 IP**（轮换，见 [`ClientPool`]）。
///
/// **只重试网络层错误**（超时 / 连不上）；HTTP 状态码由调用方判定，**不重试**
/// ——**例外**：`retry_on_status` 指定的那些状态码。
///
/// **为什么要给状态码开口子**：GitHub 的 OAuth **业务错误全是 HTTP 200 + JSON `error`**
/// （`authorization_pending` / `slow_down` / `expired_token` / `access_denied` 都实测过是 200）。
/// 所以 ①②③ 上**任何非 2xx 都意味着「这个边缘节点没正确服务该端点」**（实测会随机回 400/403，
/// 且**同一时刻换个节点就正常**）→ 换下一个候选 IP 重试，而不是当成业务失败。
async fn send_with_retry(
    attempts: u32,
    build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    retry_on_status: fn(reqwest::StatusCode) -> bool,
) -> Result<reqwest::Response, String> {
    let pool = pool()?;
    let mut last = "请求失败".to_string();
    for attempt in 1..=attempts.max(1) {
        let client = pool.next_client();
        match build(client).send().await {
            Ok(response) => {
                let status = response.status();
                if retry_on_status(status) && attempt < attempts {
                    last = format!("GitHub 返回 HTTP {}", status.as_u16());
                    tokio::time::sleep(std::time::Duration::from_millis(400 * u64::from(attempt)))
                        .await;
                    continue;
                }
                return Ok(response);
            }
            Err(err) => {
                let transient = err.is_timeout() || err.is_connect();
                last = http_error(&err);
                if !transient || attempt >= attempts {
                    return Err(last);
                }
                tokio::time::sleep(std::time::Duration::from_millis(400 * u64::from(attempt))).await;
            }
        }
    }
    Err(last)
}

/// ① 发起设备授权。
pub async fn start_device_flow(endpoints: &Endpoints) -> Result<DeviceFlow, String> {
    let body = form_body(&[("client_id", CLIENT_ID), ("scope", SCOPE)]);
    let response = send_with_retry(
        5,
        |client| {
            client
                .post(&endpoints.device_code_url)
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .body(body.clone())
        },
        // ① 的业务响应永远是 200 + JSON；**非 2xx = 这个节点没服务好该端点** → 换候选 IP
        |status| !status.is_success(),
    )
    .await
    .map_err(|err| format!("发起设备授权失败：{err}"))?;
    let status = response.status();    let text = response
        .text()
        .await
        .map_err(|err| format!("读取设备码响应失败：{}", http_error(&err)))?;
    if !status.is_success() {
        return Err(format!("GitHub 返回 HTTP {}", status.as_u16()));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| format!("解析设备码响应失败：{err}"))?;
    Ok(DeviceFlow {
        device_code: value
            .get("device_code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "GitHub 响应里没有 device_code".to_string())?
            .to_string(),
        user_code: value
            .get("user_code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "GitHub 响应里没有 user_code".to_string())?
            .to_string(),
        verification_uri: value
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "GitHub 响应里没有 verification_uri".to_string())?
            .to_string(),
        interval: value
            .get("interval")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_INTERVAL_SECS),
        expires_in: value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_EXPIRES_IN_SECS),
    })
}

/// ③ 轮询一次授权结果（**一次调用只轮询一次**；间隔由前端按 `interval` 控制）。
pub async fn poll_device_flow(
    endpoints: &Endpoints,
    device_code: &str,
) -> Result<PollOutcome, String> {
    let body = form_body(&[
        ("client_id", CLIENT_ID),
        ("device_code", device_code),
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:device_code",
        ),
    ]);
    let response = send_with_retry(
        5,
        |client| {
            client
                .post(&endpoints.access_token_url)
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .body(body.clone())
        },
        // ③ 的业务响应（`authorization_pending` / `slow_down` / `expired_token` /
        // `access_denied` / 成功）**实测全是 HTTP 200**；非 2xx = 节点没服务好 → 换候选 IP
        |status| !status.is_success(),
    )
    .await
    .map_err(|err| format!("轮询授权结果失败：{err}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format!("读取授权响应失败：{}", http_error(&err)))?;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);

    // 成功：只有这一处会拿到 token
    if let Some(token) = value.get("access_token").and_then(|v| v.as_str()) {
        if !token.is_empty() {
            return Ok(PollOutcome::Authorized(token.to_string()));
        }
    }
    match value.get("error").and_then(|v| v.as_str()) {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => Ok(PollOutcome::SlowDown),
        Some("expired_token") => Ok(PollOutcome::Expired),
        Some("access_denied") => Ok(PollOutcome::Denied),
        // 只回 error 码，**不回显响应体**（可能含敏感信息）
        Some(other) => Err(format!("GitHub 返回错误：{other}")),
        None => {
            if status.is_success() {
                Err("GitHub 响应里既没有 token 也没有 error".to_string())
            } else {
                Err(format!("GitHub 返回 HTTP {}", status.as_u16()))
            }
        }
    }
}

/// ④ 用 GitHub token 换 Copilot token。
pub async fn fetch_copilot_token(
    endpoints: &Endpoints,
    gh_token: &str,
) -> Result<CopilotToken, String> {
    let response = send_with_retry(
        5,
        |client| {
            client
                .get(&endpoints.copilot_token_url)
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::AUTHORIZATION, format!("token {gh_token}"))
                .header(reqwest::header::USER_AGENT, USER_AGENT)
        },
        // ④ 的 **401 / 403 是业务错误**（token 无效 / 没有订阅）→ **不能重试**；
        // 只有 5xx 才当网络问题重试（且 ④ 的主机是 `api.github.com`，**不在钉 IP 范围内**）
        |status| status.is_server_error(),
    )
    .await
    .map_err(|err| format!("换取 Copilot token 失败：{err}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| format!("读取 Copilot token 响应失败：{}", http_error(&err)))?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "GitHub token 无效或已过期——请重新设备授权".to_string(),
            403 => "该账号没有可用的 GitHub Copilot 订阅".to_string(),
            other => format!("换取 Copilot token 失败：HTTP {other}"),
        });
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| format!("解析 Copilot token 响应失败：{err}"))?;
    let token = value
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Copilot token 响应里没有 token".to_string())?
        .to_string();
    // `expires_at` 是 **float** Unix 秒
    let expires_at = value
        .get("expires_at")
        .and_then(|v| v.as_f64())
        .map(|v| v as i64)
        .ok_or_else(|| "Copilot token 响应里没有 expires_at".to_string())?;
    let api_base = value
        .get("endpoints")
        .and_then(|v| v.get("api"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .to_string();
    Ok(CopilotToken {
        token,
        expires_at,
        api_base,
    })
}

/// 设备授权会话的**内存**存储（`device_code` 不出后端）。
pub struct FlowStore {
    inner: Mutex<HashMap<String, DeviceFlow>>,
}

static FLOWS: OnceLock<FlowStore> = OnceLock::new();

/// 全局会话表（进程内；应用退出即清空——**这是特性不是缺陷**）。
pub fn flows() -> &'static FlowStore {
    FLOWS.get_or_init(|| FlowStore {
        inner: Mutex::new(HashMap::new()),
    })
}

impl FlowStore {
    /// 存一个会话，返回 `flow_id`（**不是** `device_code`）。
    pub fn insert(&self, flow: DeviceFlow) -> String {
        let id = crate::vault::new_id("flow");
        if let Ok(mut map) = self.inner.lock() {
            map.insert(id.clone(), flow);
        }
        id
    }

    /// 取一个会话（**克隆**，不交出所有权——授权成功后由调用方决定是否删除）。
    pub fn get(&self, flow_id: &str) -> Option<DeviceFlow> {
        self.inner.lock().ok().and_then(|map| map.get(flow_id).cloned())
    }

    /// 结束一个会话（授权成功 / 过期 / 拒绝后清掉）。
    pub fn remove(&self, flow_id: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(flow_id);
        }
    }
}

/// Copilot token 的**进程内缓存**（避免每次出站都解密；过期前 5 min 重取）。
///
/// 缓存里是**明文 token**——这是短生命周期凭据的正常驻留方式
/// （与 `CallContext.credential` 同性质），**不进日志、不进 IPC**。
pub struct TokenCache {
    inner: Mutex<Option<CopilotToken>>,
}

static CACHE: OnceLock<Arc<TokenCache>> = OnceLock::new();

/// 全局 token 缓存（**进程内共享**）。
///
/// 用 `Arc` 包一层：出站适配器（T21）要**持有**这个缓存（跨请求复用，
/// 否则每次出站都得重换 token），而单测要**独立实例**（并行用例互不污染）。
pub fn cache() -> &'static Arc<TokenCache> {
    CACHE.get_or_init(|| Arc::new(TokenCache::new()))
}

impl TokenCache {
    /// 新建一个**独立**的缓存（生产用全局那个；**单测用独立实例**避免互相干扰）。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// 缓存里的 token（**可能已过期**）。
    pub fn peek(&self) -> Option<CopilotToken> {
        self.inner.lock().ok().and_then(|guard| guard.clone())
    }

    /// 写入缓存。
    pub fn put(&self, token: CopilotToken) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some(token);
        }
    }

    /// 清空缓存（断开连接 / 重新授权时）。
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }

    /// 缓存是否**还能用**（离过期至少还有 `REFRESH_SKEW_SECS`）。
    pub fn is_fresh(&self, now: i64) -> bool {
        self.peek()
            .map(|token| now < token.expires_at - REFRESH_SKEW_SECS)
            .unwrap_or(false)
    }
}

/// 把 GitHub token（长期）写进保险库；**同 kind 的旧凭据删掉**（不留孤儿）。
pub fn store_github_token(vault: &Vault, channel_id: &str, token: &str) -> Result<String, String> {
    delete_kind(vault, channel_id, GH_TOKEN_KIND)?;
    let secret = SecretBytes::from_slice(token.as_bytes());
    vault
        .put_credential(channel_id, GH_TOKEN_KIND, &secret)
        .map_err(|err| err.to_string())
}

/// 把 Copilot token（短期，带过期时间）写进保险库。
pub fn store_copilot_token(
    vault: &Vault,
    channel_id: &str,
    token: &CopilotToken,
) -> Result<String, String> {
    delete_kind(vault, channel_id, COPILOT_TOKEN_KIND)?;
    let secret = SecretBytes::from_slice(token.token.as_bytes());
    vault
        .put_credential_expiring(channel_id, COPILOT_TOKEN_KIND, &secret, Some(token.expires_at))
        .map_err(|err| err.to_string())
}

/// 读出渠道下的 GitHub token（**解密只在这一刻发生**）。
pub fn load_github_token(vault: &Vault, channel_id: &str) -> Result<Option<String>, String> {
    let meta = find_kind(vault, channel_id, GH_TOKEN_KIND)?;
    let Some(meta) = meta else { return Ok(None) };
    let secret = vault
        .get_credential(&meta.id)
        .map_err(|err| err.to_string())?;
    Ok(Some(String::from_utf8_lossy(secret.as_bytes()).to_string()))
}

/// 读出渠道下的 Copilot token 元信息（**只有 id 与过期时间，没有明文**）。
pub fn copilot_token_meta(vault: &Vault, channel_id: &str) -> Result<Option<(String, Option<i64>)>, String> {
    let meta = find_kind(vault, channel_id, COPILOT_TOKEN_KIND)?;
    Ok(meta.map(|m| (m.id, m.expires_at)))
}

/// 断开连接：删掉两条凭据 + 清缓存。
pub fn disconnect(vault: &Vault, channel_id: &str) -> Result<(), String> {
    delete_kind(vault, channel_id, GH_TOKEN_KIND)?;
    delete_kind(vault, channel_id, COPILOT_TOKEN_KIND)?;
    cache().clear();
    Ok(())
}

/// 取一个**可用的** Copilot token：缓存命中就用；否则用保险库里的 GitHub token 重取。
///
/// `cache` **显式传入**（生产传全局 `cache()`）——单测要**独立实例**，否则
/// 并行跑的用例会互相污染缓存、测不出「到底有没有重取」。
///
/// **为什么收 `&Arc<RwLock<Vault>>` 而不是 `&Vault`**：`std::sync::RwLockReadGuard`
/// **不是 `Send`** → 调用方若在 `await` 期间持有读锁，这个 future 就失去 `Send`
/// （`#[async_trait]` 要求 `Send`）→ 编译不过。故保险库的读写**只在同步段里**做，
/// **读锁用完立刻放**，绝不跨 `await`。
pub async fn ensure_token(
    endpoints: &Endpoints,
    vault: &Arc<RwLock<Vault>>,
    channel_id: &str,
    cache: &TokenCache,
) -> Result<CopilotToken, String> {
    let now = now_secs();
    if cache.is_fresh(now) {
        if let Some(token) = cache.peek() {
            return Ok(token);
        }
    }
    // ── 同步段：取 GitHub token（读锁用完即放）
    let gh_token = {
        let guard = vault.read().unwrap_or_else(|err| err.into_inner());
        load_github_token(&guard, channel_id)?
    }
    .ok_or_else(|| "还没有 GitHub token——先做一次设备授权".to_string())?;

    // ── 异步段：换 Copilot token（**不持锁**）
    let token = fetch_copilot_token(endpoints, &gh_token).await?;

    // ── 同步段：回写保险库 + 缓存
    {
        let guard = vault.read().unwrap_or_else(|err| err.into_inner());
        store_copilot_token(&guard, channel_id, &token)?;
    }
    cache.put(token.clone());
    Ok(token)
}

/// 渠道下某 kind 的凭据元信息（**不含明文**）。
fn find_kind(vault: &Vault, channel_id: &str, kind: &str) -> Result<Option<crate::vault::CredentialMeta>, String> {
    let items = vault
        .list_credentials(Some(channel_id))
        .map_err(|err| err.to_string())?;
    Ok(items.into_iter().find(|m| m.kind == kind))
}

/// 删掉渠道下某 kind 的**全部**凭据。
fn delete_kind(vault: &Vault, channel_id: &str, kind: &str) -> Result<(), String> {
    if let Some(meta) = find_kind(vault, channel_id, kind)? {
        let _ = vault.delete_credential(&meta.id);
    }
    Ok(())
}

/// 构造 `application/x-www-form-urlencoded` 请求体。
fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&url_encode(key));
        out.push('=');
        out.push_str(&url_encode(value));
    }
    out
}

/// 极简百分号编码（**只够表单用**：空格 → `+`，其余保留字符集之外转义）。
fn url_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// reqwest 错误 → 不含敏感信息的一句话。
fn http_error(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "请求超时".to_string()
    } else if err.is_connect() {
        "连接失败".to_string()
    } else if let Some(status) = err.status() {
        format!("HTTP {}", status.as_u16())
    } else {
        "请求失败".to_string()
    }
}

/// 当前 Unix 秒。
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// IPC 视图：设备授权会话（**不含 `device_code`**）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFlowView {
    /// 会话 id（**不是** `device_code`）。
    pub flow_id: String,
    /// 给用户输入的短码。
    pub user_code: String,
    /// 验证网址。
    pub verification_uri: String,
    /// 建议轮询间隔（秒）。
    pub interval: u64,
    /// 设备码有效期（秒）。
    pub expires_in: u64,
}

impl DeviceFlowView {
    /// 会话 → IPC 视图（**丢掉 `device_code`**）。
    pub fn of(flow_id: &str, flow: &DeviceFlow) -> Self {
        Self {
            flow_id: flow_id.to_string(),
            user_code: flow.user_code.clone(),
            verification_uri: flow.verification_uri.clone(),
            interval: flow.interval,
            expires_in: flow.expires_in,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_body_is_url_encoded() {
        let body = form_body(&[("client_id", CLIENT_ID), ("scope", SCOPE)]);
        assert!(body.starts_with("client_id=Iv1.b507a08c87ecfe98&scope=read%3Auser"), "{body}");
    }

    #[test]
    fn the_device_flow_view_never_carries_the_device_code() {
        let flow = DeviceFlow {
            device_code: "SECRET-DEVICE-CODE".to_string(),
            user_code: "ABCD-1234".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            interval: 5,
            expires_in: 900,
        };
        let view = DeviceFlowView::of("flow_1", &flow);
        let json = serde_json::to_string(&view).expect("序列化");
        assert!(!json.contains("SECRET-DEVICE-CODE"), "{json}");
        assert!(json.contains("ABCD-1234"), "{json}");
    }

    #[test]
    fn the_cache_is_fresh_until_five_minutes_before_expiry() {
        let cache = TokenCache {
            inner: Mutex::new(None),
        };
        cache.put(CopilotToken {
            token: "t".to_string(),
            expires_at: 1_000,
            api_base: DEFAULT_API_BASE.to_string(),
        });
        assert!(cache.is_fresh(600), "离过期 400s 应仍算新鲜");
        assert!(!cache.is_fresh(700), "离过期 300s 应判为需重取");
        assert!(!cache.is_fresh(2_000), "已过期");
    }

    #[test]
    fn the_flow_store_keeps_the_device_code_server_side() {
        let store = FlowStore {
            inner: Mutex::new(HashMap::new()),
        };
        let flow = DeviceFlow {
            device_code: "SECRET-DEVICE-CODE".to_string(),
            user_code: "WXYZ-9876".to_string(),
            verification_uri: "https://github.com/login/device".to_string(),
            interval: 5,
            expires_in: 900,
        };
        let id = store.insert(flow);
        assert!(!id.contains("SECRET-DEVICE-CODE"));
        assert_eq!(store.get(&id).map(|f| f.user_code), Some("WXYZ-9876".to_string()));
        store.remove(&id);
        assert!(store.get(&id).is_none());
    }

    #[tokio::test]
    async fn the_network_layer_retries_transient_failures() {
        // 本机实测：`github.com` 的链路时好时坏，命中坏路径时单次请求必然超时
        // → 网络层错误必须重试（三个端点操作都幂等），且**每次重试换一个候选 IP**。
        // 借一个回环端口再放掉 → 连接被拒（网络层错误）
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
        let port = listener.local_addr().expect("取本机地址").port();
        drop(listener);

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let err = send_with_retry(
            3,
            |client| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                client.get(format!("http://127.0.0.1:{port}/nope"))
            },
            |status| !status.is_success(),
        )
        .await
        .expect_err("连不上应当报错");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "网络层错误应重试到 3 次"
        );
        assert_eq!(err, "连接失败");
    }

    #[test]
    fn the_client_pool_keeps_a_dns_fallback_and_rotates() {
        // 池子 = 候选 IP 各一个 + **末尾一个走普通 DNS 的兜底**
        let pool = pool().expect("构造客户端池");
        assert!(pool.len() >= 2, "池子至少要有兜底 client");
        // 轮换：连续取两个应当不是同一个（除非池里只有兜底那一个）
        let first = pool.next_client() as *const reqwest::Client;
        let second = pool.next_client() as *const reqwest::Client;
        if pool.len() > 1 {
            assert_ne!(first, second, "每次重试应当换一个候选 IP");
        }
    }

    #[tokio::test]
    async fn a_bare_http_error_is_retried_on_another_candidate() {
        // 「坏边缘节点」：第 1 次回 HTTP 400。OAuth 的**业务错误实测全是 200 + JSON**，
        // 所以裸 4xx = 这个节点没服务好该端点 → **换候选 IP 重试**（而不是当成业务失败）。
        let (base, mock) = testing::start().await;
        let endpoints = Endpoints {
            device_code_url: format!("{base}/login/device/code"),
            access_token_url: format!("{base}/flaky/login/oauth/access_token"),
            copilot_token_url: format!("{base}/copilot_internal/v2/token"),
        };
        let outcome = poll_device_flow(&endpoints, "MOCK-DEVICE-CODE")
            .await
            .expect("应当重试到成功");
        assert_eq!(outcome, PollOutcome::Pending);
        assert_eq!(mock.poll_count(), 2, "第 1 次的 400 应当被换 IP 重试掉");
    }

    #[test]
    fn the_env_var_overrides_the_builtin_ip_pool() {
        // 候选 IP 来自环境变量时，内置池被**完全替换**
        let ips = candidate_ips_from(Some("203.0.113.7, 203.0.113.8 ,".to_string()));
        assert_eq!(
            ips,
            vec!["203.0.113.7".to_string(), "203.0.113.8".to_string()]
        );
        // 没给环境变量 → 用内置池
        assert_eq!(candidate_ips_from(None).len(), GITHUB_IP_POOL.len());
    }
}

/// **本地 mock 上游**（**测试专用**）。
///
/// **不出网**：绑 `127.0.0.1:0`，路径与真实 GitHub **一致** → 代码路径完全相同，
/// 只是把「真实 GitHub」换成「本机 mock」。
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::header::AUTHORIZATION;
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::Router;
    use serde_json::json;

    /// mock 记录到的请求（**只记请求，不记响应里的 token**）。
    #[derive(Default)]
    pub struct MockUpstream {
        /// 已经轮询了几次（决定这次返回什么）。
        polls: AtomicUsize,
        /// ④ 被调了几次（换 Copilot token）。
        copilot_token_calls: AtomicUsize,
        /// 设备码请求体。
        device_code_body: Mutex<Option<String>>,
        /// 每次轮询的请求体。
        poll_bodies: Mutex<Vec<String>>,
        /// ④ 请求带的 Authorization 头。
        auth_header: Mutex<Option<String>>,
    }

    impl MockUpstream {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// 轮询次数。
        pub fn poll_count(&self) -> usize {
            self.polls.load(Ordering::SeqCst)
        }

        /// ④（换 Copilot token）被调了几次。
        pub fn copilot_token_calls(&self) -> usize {
            self.copilot_token_calls.load(Ordering::SeqCst)
        }

        /// 设备码请求体。
        pub fn device_code_body(&self) -> String {
            self.device_code_body.lock().unwrap().clone().unwrap_or_default()
        }

        /// 全部轮询请求体。
        pub fn poll_bodies(&self) -> Vec<String> {
            self.poll_bodies.lock().unwrap().clone()
        }

        /// ④ 请求的 Authorization 头。
        pub fn auth_header(&self) -> String {
            self.auth_header.lock().unwrap().clone().unwrap_or_default()
        }
    }

    /// 起一个本地 mock 上游，返回（base_url, 记录器）。
    pub async fn start() -> (String, Arc<MockUpstream>) {
        let recorder = MockUpstream::new();
        let router = Router::new()
            .route("/login/device/code", post(device_code))
            .route("/login/oauth/access_token", post(access_token))
            .route("/copilot_internal/v2/token", get(copilot_token))
            // 错误分支：GitHub token 无效（401）/ 没有订阅（403）
            .route("/bad-token/copilot_internal/v2/token", get(bad_token))
            .route("/no-subscription/copilot_internal/v2/token", get(no_subscription))
            // 终态分支：设备码过期 / 用户拒绝
            .route("/expired/login/oauth/access_token", post(always_expired))
            .route("/denied/login/oauth/access_token", post(always_denied))
            // 「坏边缘节点」分支：第 1 次回 HTTP 400（不是 OAuth 业务错误），第 2 次起正常
            .route("/flaky/login/oauth/access_token", post(flaky_edge))
            .with_state(Arc::clone(&recorder));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定回环端口");
        let addr = listener.local_addr().expect("取本机地址");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (format!("http://{addr}"), recorder)
    }

    /// ① 设备码。
    async fn device_code(
        State(rec): State<Arc<MockUpstream>>,
        body: Bytes,
    ) -> axum::Json<serde_json::Value> {
        *rec.device_code_body.lock().unwrap() = Some(String::from_utf8_lossy(&body).into_owned());
        axum::Json(json!({
            "device_code": "MOCK-DEVICE-CODE",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "interval": 5,
            "expires_in": 900,
        }))
    }

    /// 「坏边缘节点」：第 1 次回 HTTP 400，第 2 次起正常。
    ///
    /// 模拟本机实测到的现象——**同一个 IP 在不同时刻一个能服务该端点、一个不能**
    /// （会随机回 400/403；OAuth 的业务错误实测**全是 HTTP 200 + JSON `error`**）。
    async fn flaky_edge(State(rec): State<Arc<MockUpstream>>) -> axum::response::Response {
        if rec.polls.fetch_add(1, Ordering::SeqCst) == 0 {
            axum::http::StatusCode::BAD_REQUEST.into_response()
        } else {
            axum::Json(json!({"error": "authorization_pending"})).into_response()
        }
    }

    /// ③ 轮询：第 1 次 pending、第 2 次 slow_down、第 3 次授权成功。
    async fn access_token(
        State(rec): State<Arc<MockUpstream>>,
        body: Bytes,
    ) -> axum::Json<serde_json::Value> {
        rec.poll_bodies
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(&body).into_owned());
        match rec.polls.fetch_add(1, Ordering::SeqCst) {
            0 => axum::Json(json!({"error": "authorization_pending"})),
            1 => axum::Json(json!({"error": "slow_down"})),
            _ => axum::Json(json!({
                "access_token": "MOCK-GH-TOKEN",
                "token_type": "bearer",
                "scope": "read:user",
            })),
        }
    }

    /// ④ 换 Copilot token。
    async fn copilot_token(
        State(rec): State<Arc<MockUpstream>>,
        headers: HeaderMap,
    ) -> axum::Json<serde_json::Value> {
        rec.copilot_token_calls.fetch_add(1, Ordering::SeqCst);
        *rec.auth_header.lock().unwrap() = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string());
        axum::Json(json!({
            "token": "MOCK-COPILOT-TOKEN",
            "expires_at": crate::vault::now_secs() + 1800,
            "endpoints": {"api": "https://api.githubcopilot.com"},
        }))
    }

    /// 终态：设备码过期。
    async fn always_expired() -> axum::Json<serde_json::Value> {
        axum::Json(json!({"error": "expired_token"}))
    }

    /// 终态：用户拒绝授权。
    async fn always_denied() -> axum::Json<serde_json::Value> {
        axum::Json(json!({"error": "access_denied"}))
    }

    /// ④ 的 401 分支（GitHub token 无效）。
    async fn bad_token() -> axum::http::StatusCode {
        axum::http::StatusCode::UNAUTHORIZED
    }

    /// ④ 的 403 分支（账号没有 Copilot 订阅）。
    async fn no_subscription() -> axum::http::StatusCode {
        axum::http::StatusCode::FORBIDDEN
    }
}

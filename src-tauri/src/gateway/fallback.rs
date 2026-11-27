//! 候选链消费与首字节前重试（02 §2.1 ④⑧、§3.3、§4.3）。
//!
//! 流程：
//! 1. 依次取候选（首选 + 组内按 priority 排序的备选）；
//! 2. **熔断中**的候选直接跳过；
//! 3. 发送失败 / 429 / 5xx → 记熔断状态 → 取下一候选（**首字节前**才允许重试）；
//! 4. 400 且错误体含 `stream_options` → P-4 学习式兼容：关闭该渠道注入后重试同一候选；
//! 5. 401/403/其它 4xx → 不可重试，直接回传；
//! 6. 2xx → 组装响应（P-7 `X-Routed-Via`、P-6 响应头透传）。

use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::Value;

use crate::gateway::circuit::{CircuitBreaker, OpenReason};
use crate::gateway::errors::{ErrorKind, GatewayError};
use crate::gateway::inbound::openai::ChatRequest;
use crate::gateway::outbound::{
    mentions_stream_options, OutboundClient, RateLimitInfo, StreamUsageDowngrades, UpstreamBody,
    UpstreamResponse,
};
use crate::gateway::router::{RouteCandidate, RoutePlan};
use crate::gateway::stream;
use crate::gateway::usage::{
    extract_completion_text, extract_prompt_text, parse_reported_usage_from_response,
    resolve_tokens, TokenEstimator, UsageContext, UsageTap, PROTOCOL_OPENAI,
};
use crate::providers;
use crate::providers::adapter::{CallContext, OpenAIRequest, OutgoingCall, ProviderAdapter};
use crate::providers::presets::minimax;
use crate::store::quota::{default_account_id, learn_rate_limit, RateLimitObservation};
use crate::store::usage::{UsageRecord, UsageSink, UsageStatus};
use crate::store::Store;
use crate::vault::Vault;

/// 熔断粒度键：`(channel_id, upstream_model)`（02 §2.3）。
pub use crate::gateway::circuit::circuit_key;

/// 转发器。
#[derive(Clone)]
pub struct Forwarder {
    /// 出站客户端。
    pub client: OutboundClient,
    /// 熔断器。
    pub circuit: Arc<CircuitBreaker>,
    /// R-4 注入降级状态（P-4 学习式兼容）。
    pub downgrades: Arc<StreamUsageDowngrades>,
    /// 加密保险库（凭证解密）。
    pub vault: Arc<RwLock<Vault>>,
    /// 数据层（限额自学习回写 `account.quota_json`，02 §8.2）。
    pub store: Arc<Store>,
    /// 用量落库出口（一条记录 = 一次出站尝试，02 §8.1）。
    pub usage_sink: Arc<dyn UsageSink>,
    /// token 估算器（上游没回 usage 时用；T12 换 tiktoken-rs）。
    pub estimator: Arc<dyn TokenEstimator>,
    /// 适配器构造依赖（**OAuth 型渠道的凭证供给**；02 §6.2.2）。
    pub adapter_deps: providers::AdapterDeps,
}

impl std::fmt::Debug for Forwarder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forwarder").finish_non_exhaustive()
    }
}

impl Forwarder {
    /// 构造转发器。
    pub fn new(
        client: OutboundClient,
        circuit: Arc<CircuitBreaker>,
        downgrades: Arc<StreamUsageDowngrades>,
        vault: Arc<RwLock<Vault>>,
        store: Arc<Store>,
        usage_sink: Arc<dyn UsageSink>,
        estimator: Arc<dyn TokenEstimator>,
    ) -> Self {
        // OAuth 型渠道的会话：**生产默认**（真实 GitHub 端点 + 全局 token 缓存）。
        // 测试用 `.with_adapter_deps(...)` 换成**本地 mock 端点 + 独立缓存**。
        let adapter_deps = providers::AdapterDeps {
            copilot: providers::copilot::CopilotSession::production(Arc::clone(&vault)),
        };
        Forwarder {
            client,
            circuit,
            downgrades,
            vault,
            store,
            usage_sink,
            estimator,
            adapter_deps,
        }
    }

    /// 换掉适配器依赖（**测试用**：把 OAuth 端点指向本地 mock）。
    pub fn with_adapter_deps(mut self, deps: providers::AdapterDeps) -> Self {
        self.adapter_deps = deps;
        self
    }

    /// 按候选链转发一次调用。
    pub async fn forward(
        &self,
        plan: &RoutePlan,
        request: &ChatRequest,
        inbound_headers: &HeaderMap,
        client_key_id: Option<&str>,
    ) -> Result<Response, GatewayError> {
        if self.vault.read().unwrap_or_else(|e| e.into_inner()).is_locked() {
            return Err(GatewayError::new(
                ErrorKind::ApiError,
                "保险库处于锁定态：请先在应用内解锁主密码后再发起请求",
            ));
        }

        let mut retried: Vec<String> = Vec::new();
        let mut last_error: Option<GatewayError> = None;
        // 冷却期内被跳过的候选里，若有"限流冷却"（而非连续失败熔断），
        // 客户端应该收到 429 而不是笼统的 502 —— 限流原因是需要被看见的。
        let mut rate_limit_skip: Option<(i64, String)> = None;

        for candidate in &plan.candidates {
            let key = circuit_key(&candidate.channel_id, &candidate.upstream_model);
            // 用量采集上下文（02 §8.1）：一条记录 = 一次出站尝试
            let usage_ctx = self.usage_context(client_key_id, request, candidate);

            // 熔断中的候选直接跳过
            if !self.circuit.allow(&key) {
                if let Some((remaining, reason)) = self.circuit.cooldown(&key) {
                    if reason == OpenReason::RateLimit {
                        rate_limit_skip = Some((remaining, candidate.channel_name.clone()));
                    }
                }
                retried.push(format!("{}（熔断中，已跳过）", candidate.channel_name));
                continue;
            }

            // 没有适配器的 provider（登录态型尚未实现）直接跳过
            let Some(adapter) = providers::adapter_for(&candidate.provider, &self.adapter_deps)
            else {
                retried.push(format!("{}（该渠道类型尚未接入，已跳过）", candidate.channel_name));
                continue;
            };

            // 凭证解密（一次候选只解一次）
            //
            // **OAuth 型渠道不在这里解密**（02 §6.2.2）：它们的凭证**由 adapter 自己供给**
            // （Copilot token 由 adapter 出站前 `ensure_token()` 取）。
            // 理由：GitHub token 与 Copilot token **不能互换**——沿用「按 kind 选一条」
            // 的旧口径，token 过期时会**误取 GitHub token** 当 Bearer → 必然 401。
            let credential = if is_oauth_kind(&candidate.channel_kind) {
                None
            } else {
                match &candidate.credential_id {
                    Some(id) => {
                        let guard = self.vault.read().unwrap_or_else(|e| e.into_inner());
                        match guard.get_credential(id) {
                            Ok(bytes) => Some(bytes),
                            Err(err) => {
                                last_error = Some(
                                    GatewayError::new(
                                        ErrorKind::ApiError,
                                        format!("凭证 {id} 解密失败: {err}"),
                                    )
                                    .with_channel(candidate.channel_name.clone()),
                                );
                                retried.push(candidate.channel_name.clone());
                                break;
                            }
                        }
                    }
                    None => {
                        last_error = Some(
                            GatewayError::new(
                                ErrorKind::ApiError,
                                format!("渠道 {} 没有可用凭证", candidate.channel_name),
                            )
                            .with_channel(candidate.channel_name.clone()),
                        );
                        retried.push(candidate.channel_name.clone());
                        break;
                    }
                }
            };

            let mut attempt = 0u8;
            loop {
                attempt += 1;
                // 出站调用构造失败（凭证不可用 / 渠道配置错误）→ **这个候选发不出去**，
                // 换下一个候选（而不是整条链直接失败）——OAuth 型渠道的凭证取不到时
                // 应当能退到别的渠道（02 §6.2.2）。
                let (call, injection_enabled) = match self
                    .build_call(candidate, adapter.as_ref(), request, inbound_headers, credential.clone())
                    .await
                {
                    Ok(built) => built,
                    Err(err) => {
                        retried.push(candidate.channel_name.clone());
                        last_error = Some(err.with_channel(candidate.channel_name.clone()));
                        break;
                    }
                };

                let attempt_started = Instant::now();
                let send = self.client.send(&call, request.stream).await;
                let response = match send {
                    Ok(response) => response,
                    Err(err) => {
                        // 连接层失败：可重试（首字节前）
                        self.circuit.on_failure(&key);
                        retried.push(candidate.channel_name.clone());
                        self.record_attempt(
                            &usage_ctx,
                            UsageStatus::Error,
                            Some(ErrorKind::UpstreamError),
                            attempt_started,
                        );
                        last_error = Some(
                            GatewayError::upstream(err.to_string())
                                .with_channel(candidate.channel_name.clone()),
                        );
                        break;
                    }
                };

                let status = response.status;
                if status == StatusCode::TOO_MANY_REQUESTS {
                    // 429：冷却参数进熔断器（Retry-After / x-ratelimit-*）
                    let info = RateLimitInfo::parse(&response.headers);
                    // 实际生效的冷却秒数（受用户设置的上限约束）
                    let cooldown = self.circuit.on_rate_limited(&key, info.cooldown_hint());
                    // 限额自学习（02 §8.2）：上游报的上限写进 account.quota_json
                    self.learn_quota(&candidate.channel_id, &info);
                    retried.push(candidate.channel_name.clone());
                    self.record_attempt(
                        &usage_ctx,
                        UsageStatus::Error,
                        Some(ErrorKind::RateLimitError),
                        attempt_started,
                    );
                    let err = GatewayError::rate_limited(format!(
                        "上游 {} 限流（429），冷却 {}s",
                        candidate.channel_name, cooldown
                    ))
                    .with_channel(candidate.channel_name.clone())
                    .with_retry_after(cooldown.max(1) as u64);
                    last_error = Some(err);
                    break;
                }

                if status.is_server_error() {
                    // 5xx：可重试，并计入熔断
                    self.circuit.on_failure(&key);
                    retried.push(candidate.channel_name.clone());
                    self.record_attempt(
                        &usage_ctx,
                        UsageStatus::Error,
                        Some(ErrorKind::UpstreamError),
                        attempt_started,
                    );
                    last_error = Some(
                        GatewayError::upstream(format!(
                            "上游 {} 返回 {}",
                            candidate.channel_name, status
                        ))
                        .with_channel(candidate.channel_name.clone()),
                    );
                    break;
                }

                if status.is_client_error() {
                    let body = buffered_body(&response);
                    // P-4 学习式兼容：注入 stream_options 被拒 → 关闭该渠道注入并重试
                    if status == StatusCode::BAD_REQUEST
                        && injection_enabled
                        && attempt < 2
                        && mentions_stream_options(&body)
                    {
                        self.downgrades.disable(&candidate.channel_id);
                        // 这也是一次真实的出站尝试 → 记一条（02 §8.1）
                        self.record_attempt(
                            &usage_ctx,
                            UsageStatus::Error,
                            Some(ErrorKind::InvalidRequestError),
                            attempt_started,
                        );
                        continue;
                    }
                    // 不可重试：原样回传（状态码 + 统一错误结构）
                    let err = upstream_client_error(status, &body, &candidate.channel_name);
                    self.record_attempt(
                        &usage_ctx,
                        UsageStatus::Error,
                        Some(err.error.kind),
                        attempt_started,
                    );
                    return Err(err);
                }

                // 2xx：成功
                self.circuit.on_success(&key);
                return self
                    .build_success_response(candidate, response, &retried, &usage_ctx, attempt_started)
                    .await;
            }
        }

        if let Some(err) = last_error {
            return Err(err.with_retried(retried));
        }

        // 没有候选真正发出去过：若是因为限流冷却被全部跳过，
        // 回 429 + Retry-After（客户端据此等待），而不是笼统的 502。
        if let Some((remaining, channel)) = rate_limit_skip {
            return Err(
                GatewayError::rate_limited(format!(
                    "上游 {channel} 限流冷却中，{remaining}s 后可重试"
                ))
                .with_channel(channel)
                .with_retried(retried)
                .with_retry_after(remaining.max(1) as u64),
            );
        }

        Err(GatewayError::upstream("没有可用候选：全部候选被跳过或熔断中").with_retried(retried))
    }

    /// 构造出站调用（应用 R-1~R-8 改写）。
    async fn build_call(
        &self,
        candidate: &RouteCandidate,
        adapter: &dyn ProviderAdapter,
        request: &ChatRequest,
        inbound_headers: &HeaderMap,
        credential: Option<crate::vault::crypto::SecretBytes>,
    ) -> Result<(OutgoingCall, bool), GatewayError> {
        let row = providers::ChannelRow {
            id: candidate.channel_id.clone(),
            name: candidate.channel_name.clone(),
            kind: candidate.channel_kind.clone(),
            provider: candidate.provider.clone(),
            base_url: candidate.channel_base_url.clone(),
            meta_json: candidate.channel_meta_json.clone(),
        };
        let mut config = providers::config_from_row(&row).map_err(|err| {
            GatewayError::new(ErrorKind::ApiError, format!("渠道配置错误: {err}"))
        })?;

        // P-4：该渠道已降级 → 本次会话不再注入 stream_options
        // 注意：只有**流式**请求才会注入 R-4；非流式请求即使渠道声明支持，
        // 也不会带 stream_options，此时上游报 400 不能算作"注入被拒"。
        let injection_enabled = request.stream
            && config.inject_stream_usage
            && !self.downgrades.is_disabled(&candidate.channel_id);
        config.inject_stream_usage = injection_enabled;

        let ctx = CallContext {
            channel: config,
            upstream_model: candidate.upstream_model.clone(),
            credential,
        };
        let call = adapter
            .complete(
                &ctx,
                OpenAIRequest {
                    body: request.raw.clone(),
                    stream: request.stream,
                    inbound_headers: Some(inbound_headers.clone()),
                },
            )
            .await
            .map_err(|err| {
                GatewayError::new(ErrorKind::ApiError, format!("出站请求构造失败: {err}"))
            })?;
        Ok((call, injection_enabled))
    }

    /// 用量采集上下文（02 §8.1）。
    fn usage_context(
        &self,
        client_key_id: Option<&str>,
        request: &ChatRequest,
        candidate: &RouteCandidate,
    ) -> UsageContext {
        UsageContext {
            client_key_id: client_key_id.map(|id| id.to_string()),
            channel_id: candidate.channel_id.clone(),
            // API Key 型渠道用默认账号（02 §8.2）
            account_id: default_account_id(&candidate.channel_id),
            model_alias: request.model.clone(),
            upstream_model: candidate.upstream_model.clone(),
            protocol: PROTOCOL_OPENAI.to_string(),
            prompt_text: extract_prompt_text(&request.raw),
        }
    }

    /// 记一次**失败**的出站尝试（不估 token：失败的调用没有产出）。
    fn record_attempt(
        &self,
        ctx: &UsageContext,
        status: UsageStatus,
        error_kind: Option<ErrorKind>,
        started: Instant,
    ) {
        self.usage_sink.record(UsageRecord {
            ts: crate::vault::now_secs(),
            client_key_id: ctx.client_key_id.clone(),
            channel_id: ctx.channel_id.clone(),
            account_id: ctx.account_id.clone(),
            model_alias: ctx.model_alias.clone(),
            upstream_model: ctx.upstream_model.clone(),
            protocol: ctx.protocol.clone(),
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            estimated: false,
            ttft_ms: None,
            duration_ms: started.elapsed().as_millis() as i64,
            status,
            error_kind: error_kind.map(|kind| kind.as_str().to_string()),
        });
    }

    /// 限额自学习（02 §8.2）：把上游报的上限写进 `account.quota_json`。
    fn learn_quota(&self, channel_id: &str, info: &RateLimitInfo) {
        let observation = RateLimitObservation {
            limit_requests: info.limit_requests,
            limit_tokens: info.limit_tokens,
            retry_after_secs: info.retry_after.map(|d| d.as_secs() as i64),
            reset_requests_secs: info.reset_requests.map(|d| d.as_secs() as i64),
            reset_tokens_secs: info.reset_tokens.map(|d| d.as_secs() as i64),
            remaining_requests: info.remaining_requests,
            remaining_tokens: info.remaining_tokens,
        };
        let result = learn_rate_limit(&self.store, channel_id, &observation);
        if let Err(err) = result {
            // 学习失败不影响请求（限额自学习只是给调度提供依据）
            eprintln!("[quota] 限额自学习失败: {err}");
        }
    }

    /// 组装成功响应（P-6 响应头透传 + P-7 `X-Routed-Via`）。
    async fn build_success_response(
        &self,
        candidate: &RouteCandidate,
        response: UpstreamResponse,
        retried: &[String],
        usage_ctx: &UsageContext,
        started: Instant,
    ) -> Result<Response, GatewayError> {
        let routed_via = ascii_safe(&format!(
            "{}/{}",
            candidate.channel_name, candidate.upstream_model
        ));

        match response.body {
            UpstreamBody::Buffered(bytes) => {
                // P-3：MiniMax 可能 HTTP 200 但业务错误（解析 base_resp.status_code）
                if response.status == StatusCode::OK {
                    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                        if let Some(business_error) = minimax::classify(&value) {
                            let body = serde_json::json!({
                                "error": {
                                    "message": business_error.message,
                                    "type": business_error.err_type,
                                    "code": business_error.code,
                                    "channel": candidate.channel_name,
                                    "retried": retried,
                                }
                            });
                            self.record_non_stream_success(usage_ctx, Some(&value), started);
                            return Ok(Response::builder()
                                .status(business_error.http_status)
                                .header("x-routed-via", routed_via)
                                .body(Body::from(body.to_string()))
                                .expect("构造响应"));
                        }
                    }
                }
                let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();
                self.record_non_stream_success(usage_ctx, parsed.as_ref(), started);
                let builder = Response::builder()
                    .status(response.status)
                    .header("x-routed-via", routed_via);
                let builder = copy_response_headers(builder, &response.headers);
                Ok(builder
                    .body(Body::from(bytes))
                    .expect("构造响应"))
            }
            UpstreamBody::Stream(upstream) => {
                let event = stream::error_event(
                    format!("上游 {} 流式响应中断", candidate.channel_name),
                    "upstream_error",
                    None,
                    Some(&candidate.channel_name),
                    retried,
                );
                // 旁路 tee：转发字节的同时扫 SSE 帧找 usage、记 TTFT（02 §8.1）
                let tap = UsageTap::new(
                    upstream,
                    usage_ctx.clone(),
                    Arc::clone(&self.estimator),
                    Arc::clone(&self.usage_sink),
                    started,
                );
                let builder = Response::builder()
                    .status(response.status)
                    .header("x-routed-via", routed_via);
                let builder = copy_response_headers(builder, &response.headers);
                Ok(builder
                    .body(stream::into_body(Box::pin(tap), event))
                    .expect("构造响应"))
            }
        }
    }

    /// 记一次**成功**的非流式出站尝试（上游 usage 优先，否则估算）。
    fn record_non_stream_success(
        &self,
        ctx: &UsageContext,
        body: Option<&Value>,
        started: Instant,
    ) {
        let upstream = body.and_then(parse_reported_usage_from_response);
        let completion = body.map(extract_completion_text).unwrap_or_default();
        let (tokens, estimated) = resolve_tokens(
            upstream,
            self.estimator.as_ref(),
            &ctx.upstream_model,
            &ctx.prompt_text,
            &completion,
        );
        self.usage_sink.record(UsageRecord {
            ts: crate::vault::now_secs(),
            client_key_id: ctx.client_key_id.clone(),
            channel_id: ctx.channel_id.clone(),
            account_id: ctx.account_id.clone(),
            model_alias: ctx.model_alias.clone(),
            upstream_model: ctx.upstream_model.clone(),
            protocol: ctx.protocol.clone(),
            prompt_tokens: tokens.prompt_tokens,
            completion_tokens: tokens.completion_tokens,
            cache_read_tokens: tokens.cache_read_tokens,
            cache_write_tokens: tokens.cache_write_tokens,
            reasoning_tokens: tokens.reasoning_tokens,
            estimated,
            // 非流式没有"首个 token"的概念
            ttft_ms: None,
            duration_ms: started.elapsed().as_millis() as i64,
            status: UsageStatus::Success,
            error_kind: None,
        });
    }
}

/// 渠道是不是 **OAuth 型**（凭证由 adapter 自己供给，网关不解密）。
///
/// 02 §6.2.2：OAuth 型渠道的凭证供给与刷新**归 adapter 管**——网关只解 API Key 型凭证。
pub fn is_oauth_kind(kind: &str) -> bool {
    crate::providers::adapter::ChannelKind::parse(kind)
        == Some(crate::providers::adapter::ChannelKind::OAuth)
}

/// 把值转成 HTTP 头安全的 ASCII。
///
/// 渠道名是用户自定义的，可能是中文；HTTP 头值必须是 ASCII，
/// 否则客户端用 `HeaderValue::to_str()` 读不出来。这里对非 ASCII 字节做百分号编码
/// （`健康` → `%E5%81%A5%E5%BA%B7`），ASCII 部分原样保留。
pub fn ascii_safe(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b' '..=b'~' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 复制上游响应头（去掉 hop-by-hop 与 content-length，02 §4.3 P-6）。
fn copy_response_headers(
    mut builder: axum::http::response::Builder,
    headers: &HeaderMap,
) -> axum::http::response::Builder {
    for (name, value) in headers.iter() {
        let lower = name.as_str().to_ascii_lowercase();
        if crate::providers::openai_compat::is_hop_by_hop(&lower) || lower == "content-length" {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
}

/// 取缓冲的响应体（非流式或错误响应）。
fn buffered_body(response: &UpstreamResponse) -> Vec<u8> {
    match &response.body {
        UpstreamBody::Buffered(bytes) => bytes.clone(),
        UpstreamBody::Stream(_) => Vec::new(),
    }
}

/// 上游 4xx → 统一错误结构（保留上游状态码）。
fn upstream_client_error(
    status: StatusCode,
    body: &[u8],
    channel_name: &str,
) -> GatewayError {
    let text = String::from_utf8_lossy(body);
    let (message, code) = extract_upstream_message(&text);
    let kind = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ErrorKind::AuthenticationError,
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
            ErrorKind::InvalidRequestError
        }
        _ => ErrorKind::ApiError,
    };
    let mut err = GatewayError::new(kind, message)
        .with_channel(channel_name.to_string())
        .with_status(status);
    if let Some(code) = code {
        err = err.with_code(code);
    }
    err
}

/// 从上游错误体里抽 message / code（尽量保留上游语义）。
fn extract_upstream_message(text: &str) -> (String, Option<String>) {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        let error = value.get("error");
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                value
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .or_else(|| {
                value
                    .get("base_resp")
                    .and_then(|r| r.get("status_msg"))
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            });
        let code = error
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                value
                    .get("base_resp")
                    .and_then(|r| r.get("status_code"))
                    .and_then(|c| c.as_i64())
                    .map(|c| format!("upstream_{c}"))
            });
        if let Some(message) = message {
            return (message, code);
        }
    }
    let trimmed = text.trim();
    let message = if trimmed.is_empty() {
        "上游返回空错误体".to_string()
    } else if trimmed.len() > 500 {
        format!("{}…", &trimmed[..500])
    } else {
        trimmed.to_string()
    };
    (message, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_key_is_scoped_by_channel_and_model() {
        assert_ne!(circuit_key("c1", "m1"), circuit_key("c1", "m2"));
        assert_ne!(circuit_key("c1", "m1"), circuit_key("c2", "m1"));
    }

    #[test]
    fn extracts_message_and_code_from_openai_style_error() {
        let (message, code) = extract_upstream_message(
            r#"{"error":{"message":"invalid api key","code":"invalid_api_key"}}"#,
        );
        assert_eq!(message, "invalid api key");
        assert_eq!(code.as_deref(), Some("invalid_api_key"));
    }

    #[test]
    fn extracts_message_from_minimax_base_resp() {
        let (message, code) = extract_upstream_message(
            r#"{"base_resp":{"status_code":1004,"status_msg":"invalid api key"}}"#,
        );
        assert_eq!(message, "invalid api key");
        assert_eq!(code.as_deref(), Some("upstream_1004"));
    }

    #[test]
    fn falls_back_to_raw_text_for_non_json_error() {
        let (message, code) = extract_upstream_message("upstream exploded");
        assert_eq!(message, "upstream exploded");
        assert_eq!(code, None);
    }

    #[test]
    fn empty_error_body_gets_placeholder_message() {
        let (message, _) = extract_upstream_message("   ");
        assert_eq!(message, "上游返回空错误体");
    }

    #[test]
    fn client_error_keeps_upstream_status_and_maps_type() {
        let err = upstream_client_error(
            StatusCode::UNAUTHORIZED,
            br#"{"error":{"message":"bad key"}}"#,
            "DeepSeek 官方",
        );
        assert_eq!(err.error.kind, ErrorKind::AuthenticationError);
        assert_eq!(err.error.channel.as_deref(), Some("DeepSeek 官方"));
        assert_eq!(err.error.message, "bad key");
    }

    #[test]
    fn bad_request_maps_to_invalid_request_error() {
        let err = upstream_client_error(
            StatusCode::BAD_REQUEST,
            br#"{"error":{"message":"bad param"}}"#,
            "c1",
        );
        assert_eq!(err.error.kind, ErrorKind::InvalidRequestError);
    }
}

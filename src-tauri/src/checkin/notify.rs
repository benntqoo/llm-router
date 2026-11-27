//! 签到结果通知（T19；02 §7.5）。
//!
//! **落点**：`checkin::scheduler::run_site` 之后（**结果已落 `checkin_result` 之后**）
//! → 一处覆盖**全部四条执行路径**：应用内每分钟 tick / UI「立即执行」/
//! CLI `--checkin-once` / **Windows 计划任务（应用关闭时）**。
//!
//! **通道**：
//! - **系统通知**——用 [`notify_rust`]（Windows 通知中心 / macOS / Linux DBus）。
//!   **刻意不用 Tauri 通知插件**：CLI 与计划任务路径**没有 Tauri app handle**，
//!   用插件就只能在应用开着时通知，**覆盖不了「应用关闭时也通知」这条 M3 判据**；
//! - **Webhook**——企微群机器人 / 飞书群机器人 / Telegram Bot 三种格式。
//!
//! **密文红线**：webhook 地址与 bot token **进保险库**（`kind = 'webhook'`），
//! `app_setting` 只存**凭据 id**；**报错信息不回显地址**（企微/飞书的 URL 里带 key，
//! Telegram 的 URL 里带 bot token）→ 所有错误都归一成不含地址的一句话。

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::checkin::reward::Reward;
use crate::store::settings;
use crate::store::Store;
use crate::vault::crypto::SecretBytes;
use crate::vault::Vault;

/// 通知总开关的设置键（`"1"` / `"0"`；缺失 = **开**）。
pub const KEY_ENABLED: &str = "notify_enabled";
/// 「成功也通知」的设置键（缺失 = 开）。
pub const KEY_SUCCESS: &str = "notify_success";
/// 「系统通知（弹窗）」的设置键（缺失 = 开）——用户可以只要群消息、不要弹窗。
pub const KEY_SYSTEM: &str = "notify_system";
/// webhook 类型的设置键（`wecom` / `lark` / `telegram`；空 = 不用 webhook）。
pub const KEY_WEBHOOK_KIND: &str = "notify_webhook_kind";
/// webhook 凭据 id 的设置键（**只存 id**；地址在保险库里）。
pub const KEY_WEBHOOK_CREDENTIAL: &str = "notify_webhook_credential_id";
/// Telegram 的 `chat_id`（不是密钥，是目标会话）。
pub const KEY_TELEGRAM_CHAT_ID: &str = "notify_telegram_chat_id";

/// 保险库里 webhook 凭据的 `kind`。
pub const CREDENTIAL_KIND: &str = "webhook";

/// webhook 凭据挂的**占位渠道名**（`credential.channel_id` 是 NOT NULL 外键）。
pub const CHANNEL_NAME: &str = "通知";

/// 系统通知里显示的应用名。
pub const APP_NAME: &str = "LLM Router";

/// 通知配置（**给 UI 看的视图**：没有地址，只有「配没配」）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyConfig {
    /// 总开关（关 = 一律不发）。
    pub enabled: bool,
    /// 系统通知（弹窗）开关。
    pub system_enabled: bool,
    /// 成功也通知。
    pub notify_success: bool,
    /// webhook 类型（`wecom` / `lark` / `telegram` / `None`）。
    pub webhook_kind: Option<String>,
    /// webhook 是否已配置（**只有 id，没有地址**）。
    pub webhook_configured: bool,
    /// Telegram 的 `chat_id`。
    pub telegram_chat_id: Option<String>,
}

/// 运行期配置（**内部用**：带凭据 id 与地址的来源）。
#[derive(Debug, Clone, Default)]
pub struct NotifySettings {
    pub enabled: bool,
    pub system_enabled: bool,
    pub notify_success: bool,
    pub webhook_kind: Option<String>,
    /// webhook 凭据 id（**地址永远从保险库现取**）。
    pub webhook_credential_id: Option<String>,
    pub telegram_chat_id: Option<String>,
}

/// 通知配置输入（**IPC 用**）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyConfigInput {
    pub enabled: bool,
    pub system_enabled: bool,
    pub notify_success: bool,
    /// `wecom` / `lark` / `telegram` / `None`（`None` = 不用 webhook）。
    pub webhook_kind: Option<String>,
    /// webhook 地址：
    /// - `None` → **不动**（保持原样）；
    /// - `Some("")` → **清除**（删凭据 + 清设置）；
    /// - `Some(url)` → **设置**（进保险库；旧凭据删掉，不留孤儿）。
    pub webhook_url: Option<String>,
    pub telegram_chat_id: Option<String>,
}

/// 一条通知的内容（**纯数据**，便于单测）。
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

/// 一个通道的发送结果（**脱敏**：不含地址、不含凭据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResult {
    /// 通道名（`系统通知` / `webhook`）。
    pub channel: String,
    pub ok: bool,
    /// 一句话结果（**不含地址**）。
    pub detail: String,
}

/// 读运行期配置（**读不到就按默认**：总开关开、成功也通知开、webhook 不配）。
pub fn load_settings(store: &Store) -> NotifySettings {
    let flag = |key: &str| -> bool {
        match settings::get_setting(store, key) {
            Ok(Some(raw)) => raw.trim() == "1" || raw.trim().eq_ignore_ascii_case("true"),
            _ => true,
        }
    };
    let text = |key: &str| -> Option<String> {
        settings::get_setting(store, key)
            .ok()
            .flatten()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
    };
    NotifySettings {
        enabled: flag(KEY_ENABLED),
        system_enabled: flag(KEY_SYSTEM),
        notify_success: flag(KEY_SUCCESS),
        webhook_kind: text(KEY_WEBHOOK_KIND),
        webhook_credential_id: text(KEY_WEBHOOK_CREDENTIAL),
        telegram_chat_id: text(KEY_TELEGRAM_CHAT_ID),
    }
}

/// 运行期配置 → UI 视图（**只暴露「配没配」，不回显地址**）。
pub fn config_view(settings: &NotifySettings) -> NotifyConfig {
    NotifyConfig {
        enabled: settings.enabled,
        system_enabled: settings.system_enabled,
        notify_success: settings.notify_success,
        webhook_kind: settings.webhook_kind.clone(),
        webhook_configured: settings.webhook_credential_id.is_some(),
        telegram_chat_id: settings.telegram_chat_id.clone(),
    }
}

/// 保存配置（**webhook 地址进保险库**，设置表只存 id）。
pub fn save_settings(
    store: &Store,
    vault: &Arc<RwLock<Vault>>,
    input: &NotifyConfigInput,
) -> Result<NotifyConfig, String> {
    settings::set_setting(store, KEY_ENABLED, if input.enabled { "1" } else { "0" })
        .map_err(|err| err.to_string())?;
    settings::set_setting(
        store,
        KEY_SYSTEM,
        if input.system_enabled { "1" } else { "0" },
    )
    .map_err(|err| err.to_string())?;
    settings::set_setting(
        store,
        KEY_SUCCESS,
        if input.notify_success { "1" } else { "0" },
    )
    .map_err(|err| err.to_string())?;

    // webhook 类型：空串 / `none` = 不用
    let kind = input
        .webhook_kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty() && !kind.eq_ignore_ascii_case("none"))
        .map(|kind| kind.to_string());
    match &kind {
        Some(kind) => settings::set_setting(store, KEY_WEBHOOK_KIND, kind)
            .map_err(|err| err.to_string())?,
        None => {
            let _ = settings::set_setting(store, KEY_WEBHOOK_KIND, "");
        }
    }

    // Telegram chat_id
    let chat_id = input
        .telegram_chat_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| id.to_string());
    match &chat_id {
        Some(id) => settings::set_setting(store, KEY_TELEGRAM_CHAT_ID, id)
            .map_err(|err| err.to_string())?,
        None => {
            let _ = settings::set_setting(store, KEY_TELEGRAM_CHAT_ID, "");
        }
    }

    // webhook 地址：None = 不动；Some("") = 清除；Some(url) = 设置（进保险库）
    if let Some(url) = input.webhook_url.as_deref() {
        let url = url.trim();
        let old_id = settings::get_setting(store, KEY_WEBHOOK_CREDENTIAL)
            .ok()
            .flatten()
            .filter(|id| !id.trim().is_empty());
        if url.is_empty() {
            if let Some(old_id) = old_id.as_deref() {
                let guard = vault.read().map_err(|err| err.to_string())?;
                let _ = guard.delete_credential(old_id);
            }
            let _ = settings::set_setting(store, KEY_WEBHOOK_CREDENTIAL, "");
        } else {
            let guard = vault.read().map_err(|err| err.to_string())?;
            let channel_id = guard
                .ensure_channel(CHANNEL_NAME)
                .map_err(|err| err.to_string())?;
            let secret = SecretBytes::from_slice(url.as_bytes());
            let new_id = guard
                .put_credential(&channel_id, CREDENTIAL_KIND, &secret)
                .map_err(|err| err.to_string())?;
            // 旧凭据删掉（不留孤儿凭据）
            if let Some(old_id) = old_id.as_deref() {
                if old_id != new_id {
                    let _ = guard.delete_credential(old_id);
                }
            }
            settings::set_setting(store, KEY_WEBHOOK_CREDENTIAL, &new_id)
                .map_err(|err| err.to_string())?;
        }
    }

    Ok(config_view(&load_settings(store)))
}

/// 构造通知文案（**纯函数**，单测直接打）。
///
/// **必含站点名与原因**（M3 验收判据：失败通知含站点名与原因）——
/// 正文第一行就是站点名，**任何只显示正文的通道**（企微文本消息）也带得上。
pub fn build_notification(
    site_name: &str,
    ok: bool,
    class: &str,
    message: &str,
    reward: Option<&Reward>,
) -> Notification {
    let title = if ok {
        format!("签到成功 · {site_name}")
    } else {
        format!("签到失败 · {site_name}")
    };
    let mut body = format!("站点：{site_name}\n分类：{class}\n");
    body.push_str(if ok { "结果：" } else { "原因：" });
    body.push_str(message);
    if let Some(reward) = reward {
        body.push_str(&format!("\n本次到账：+{} {}", reward.amount, reward.unit));
    }
    Notification { title, body }
}

/// webhook 请求体（**纯函数**，单测直接打）。
///
/// - 企微：`{"msgtype":"text","text":{"content":…}}`
/// - 飞书：`{"msg_type":"text","content":{"text":…}}`
/// - Telegram：`{"chat_id":…,"text":…}`
pub fn webhook_body(kind: &str, chat_id: Option<&str>, text: &str) -> Value {
    match kind {
        "wecom" => json!({"msgtype": "text", "text": {"content": text}}),
        "lark" => json!({"msg_type": "text", "content": {"text": text}}),
        "telegram" => json!({"chat_id": chat_id.unwrap_or_default(), "text": text}),
        _ => json!({}),
    }
}

/// 注册 AUMID（**每个进程只做一次**）。
///
/// Windows 10 1607+ 的 toast 要求**调用进程拥有该 AUMID**，否则系统**静默丢弃**
/// （API 仍返回成功——真机踩过：日志写「已发送」，屏幕上什么都没有）。
fn ensure_aumid_once() -> Result<(), String> {
    static READY: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    READY
        .get_or_init(|| {
            crate::checkin::system_notify::ensure_aumid(crate::checkin::system_notify::AUMID)
                .map(|_| ())
        })
        .clone()
}

/// 发系统通知（**独立于 Tauri 运行时**）。
fn send_system(notification: &Notification) -> ChannelResult {
    // 先注册 AUMID（注册失败也要继续发——大不了被系统丢掉）
    let aumid = ensure_aumid_once();
    let mut toast = notify_rust::Notification::new();
    #[cfg(target_os = "windows")]
    {
        // 用**我们自己的 AUMID**（默认是 PowerShell 的——非 powershell 进程冒用会被丢）
        toast.app_id(crate::checkin::system_notify::AUMID);
    }
    let outcome = toast
        .appname(APP_NAME)
        .summary(&notification.title)
        .body(&notification.body)
        .show();
    match outcome {
        Ok(_) => ChannelResult {
            channel: "系统通知".to_string(),
            ok: true,
            detail: match &aumid {
                Ok(()) => "已发送".to_string(),
                Err(err) => format!("已发送（但 AUMID 注册失败：{err}）"),
            },
        },
        Err(err) => ChannelResult {
            channel: "系统通知".to_string(),
            ok: false,
            detail: format!("失败：{err}"),
        },
    }
}

/// 把 `reqwest` 的错误归一成**不含地址**的一句话（**密文红线**：地址里有 key / token）。
fn redact_request_error(err: &reqwest::Error) -> String {
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

/// 发 webhook（**地址从保险库现取**，只在这一瞬间存在于内存）。
async fn send_webhook(
    client: &reqwest::Client,
    kind: &str,
    credential_id: &str,
    chat_id: Option<&str>,
    vault: &Arc<RwLock<Vault>>,
    notification: &Notification,
) -> ChannelResult {
    let channel = format!("webhook（{kind}）");
    let url = {
        let guard = match vault.read() {
            Ok(guard) => guard,
            Err(err) => {
                return ChannelResult {
                    channel,
                    ok: false,
                    detail: format!("保险库不可用：{err}"),
                };
            }
        };
        match guard.get_credential(credential_id) {
            Ok(bytes) => String::from_utf8_lossy(bytes.as_bytes()).to_string(),
            Err(_) => {
                return ChannelResult {
                    channel,
                    ok: false,
                    detail: "保险库里没有这条 webhook 凭据".to_string(),
                };
            }
        }
    };
    let url = url.trim().to_string();
    if url.is_empty() {
        return ChannelResult {
            channel,
            ok: false,
            detail: "webhook 地址是空的".to_string(),
        };
    }
    // 企微 / 飞书只显示正文 → **标题也要带上**（失败通知要含站点名）
    let text = format!("{}\n{}", notification.title, notification.body);
    let body = webhook_body(kind, chat_id, &text);
    // reqwest **没开 `json` feature**（02 §1.3 选型）→ 自己序列化 + 手动带 content-type
    let payload = match serde_json::to_string(&body) {
        Ok(payload) => payload,
        Err(err) => {
            return ChannelResult {
                channel,
                ok: false,
                detail: format!("序列化请求体失败：{err}"),
            };
        }
    };
    let response = client
        .post(&url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload)
        .send()
        .await;
    match response {
        Err(err) => ChannelResult {
            channel,
            ok: false,
            detail: redact_request_error(&err),
        },
        Ok(response) => {
            let status = response.status().as_u16();
            if response.status().is_success() {
                ChannelResult {
                    channel,
                    ok: true,
                    detail: format!("已发送（HTTP {status}）"),
                }
            } else {
                ChannelResult {
                    channel,
                    ok: false,
                    detail: format!("HTTP {status}"),
                }
            }
        }
    }
}

/// 发通知（**各通道互不影响**：一个失败不挡另一个）。
pub async fn send(
    client: &reqwest::Client,
    settings: &NotifySettings,
    vault: &Arc<RwLock<Vault>>,
    notification: &Notification,
) -> Vec<ChannelResult> {
    let mut results = Vec::new();
    if settings.system_enabled {
        results.push(send_system(notification));
    }
    if let (Some(kind), Some(credential_id)) = (
        settings.webhook_kind.as_deref(),
        settings.webhook_credential_id.as_deref(),
    ) {
        results.push(
            send_webhook(
                client,
                kind,
                credential_id,
                settings.telegram_chat_id.as_deref(),
                vault,
                notification,
            )
            .await,
        );
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reward(amount: i64) -> Reward {
        Reward::new(amount, "magicube", json!({"total_amount": amount}))
    }

    #[test]
    fn a_failure_notification_carries_the_site_name_and_the_reason() {
        let notification = build_notification(
            "ModelScope（魔搭）",
            false,
            "credential_invalid",
            "ModelScope 登录态无效（令牌过期或没填）",
            None,
        );
        assert_eq!(notification.title, "签到失败 · ModelScope（魔搭）");
        assert!(notification.body.contains("ModelScope（魔搭）"), "{}", notification.body);
        assert!(
            notification.body.contains("ModelScope 登录态无效（令牌过期或没填）"),
            "{}",
            notification.body
        );
        assert!(notification.body.contains("credential_invalid"), "{}", notification.body);
    }

    #[test]
    fn a_success_notification_carries_the_reward() {
        let notification = build_notification(
            "ModelScope（魔搭）",
            true,
            "success",
            "ModelScope 签到成功",
            Some(&reward(200)),
        );
        assert_eq!(notification.title, "签到成功 · ModelScope（魔搭）");
        assert!(notification.body.contains("本次到账：+200 magicube"), "{}", notification.body);
    }

    #[test]
    fn the_webhook_payload_shape_follows_each_provider() {
        let text = "签到失败 · ModelScope（魔搭）\n原因：登录态无效";
        let wecom = webhook_body("wecom", None, text);
        assert_eq!(wecom["msgtype"], "text");
        assert_eq!(wecom["text"]["content"], text);
        let lark = webhook_body("lark", None, text);
        assert_eq!(lark["msg_type"], "text");
        assert_eq!(lark["content"]["text"], text);
        let telegram = webhook_body("telegram", Some("-100123"), text);
        assert_eq!(telegram["chat_id"], "-100123");
        assert_eq!(telegram["text"], text);
        // 未知类型 → 空体（不该被当成合法通道）
        assert_eq!(webhook_body("unknown", None, text), json!({}));
    }

    #[tokio::test]
    async fn request_errors_never_leak_the_webhook_address() {
        // 地址里有 key / token → 错误信息里不能出现
        // （用**回环地址**：单测不出网，别真去打企微）
        let url = "http://127.0.0.1:9/webhook/send?key=SECRET-KEY";
        let err = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect_err("应当连接失败");
        let redacted = redact_request_error(&err);
        assert!(!redacted.contains("SECRET-KEY"), "{redacted}");
        assert!(!redacted.contains("127.0.0.1"), "{redacted}");
    }
}

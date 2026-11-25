//! MiniMax 共用口径（02 §4.2）：两个 profile 共享业务错误码映射。
//!
//! 要点：
//! 1. MiniMax 可能返回 **HTTP 200 但业务错误**，必须解析 `base_resp.status_code`（P-3）。
//! 2. `base_resp.status_code` 的映射动作见下表；P-3 的响应改写接线属 T6。
//! 3. GroupId **严禁**进 body / header（两处官方均不认），只允许进 URL query（R-5）。

/// MiniMax 业务错误码 → 统一错误结构（02 §4.4）的映射结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusinessError {
    /// 上游原始码（`base_resp.status_code`）
    pub upstream_code: i64,
    /// 映射后的 HTTP 状态码
    pub http_status: u16,
    /// 统一错误结构中的 `type`
    pub err_type: &'static str,
    /// 是否可重试（交给 T6 的 fallback / 熔断使用）
    pub retryable: bool,
    /// 统一错误结构中的 `code`（如 `minimax_1004`）
    pub code: String,
    /// 人类可读提示
    pub message: String,
}

/// 业务错误码映射（02 §4.2 表）。
///
/// | status_code | 含义 | 映射 |
/// |---|---|---|
/// | 1000 | 未知错误 | 500 upstream_error |
/// | 1001 | 引擎异常 | 502，可重试 |
/// | 1004 | 鉴权失败（国内站常见诱因：缺 GroupId） | 401 + 提示检查 GroupId |
/// | 1008 | 余额不足 | 402，不重试 |
/// | 1027 / 1039 | 限流 | 429，可重试 |
/// | 其他 | — | 502 |
pub fn map_business_error(status_code: i64) -> BusinessError {
    let (http_status, err_type, retryable, message): (u16, &'static str, bool, String) = match status_code
    {
        1000 => (
            500,
            "upstream_error",
            false,
            format!("MiniMax 未知错误（status_code={status_code}）"),
        ),
        1001 => (
            502,
            "upstream_error",
            true,
            format!("MiniMax 引擎异常（status_code={status_code}），可重试"),
        ),
        1004 => (
            401,
            "authentication_error",
            false,
            "MiniMax 鉴权失败（status_code=1004）：请检查 GroupId 配置，而非重新生成 key".to_string(),
        ),
        1008 => (
            402,
            "api_error",
            false,
            "MiniMax 余额不足（status_code=1008），请充值后再试".to_string(),
        ),
        1027 | 1039 => (
            429,
            "rate_limit_error",
            true,
            format!("MiniMax 限流（status_code={status_code}），可重试"),
        ),
        other => (
            502,
            "upstream_error",
            false,
            format!("MiniMax 上游错误（status_code={other}）"),
        ),
    };

    BusinessError {
        upstream_code: status_code,
        http_status,
        err_type,
        retryable,
        code: format!("minimax_{status_code}"),
        message,
    }
}

/// 从 MiniMax 响应体中解析 `base_resp.status_code`。
///
/// 缺失或非数值时返回 `None`（视为非 MiniMax 业务错误体）。
pub fn extract_status_code(body: &serde_json::Value) -> Option<i64> {
    body.get("base_resp")
        .and_then(|r| r.get("status_code"))
        .and_then(|c| c.as_i64())
}

/// 从 MiniMax 响应体中解析 `base_resp.status_msg`。
pub fn extract_status_msg(body: &serde_json::Value) -> Option<String> {
    body.get("base_resp")
        .and_then(|r| r.get("status_msg"))
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// 是否是"MiniMax 业务错误"（`base_resp.status_code != 0`）。
///
/// 02 §4.2 要点 3：MiniMax 可能 HTTP 200 但业务错误。
pub fn is_business_error(body: &serde_json::Value) -> bool {
    matches!(extract_status_code(body), Some(code) if code != 0)
}

/// 判定并映射：正常返回 `None`，业务错误返回映射结果。
pub fn classify(body: &serde_json::Value) -> Option<BusinessError> {
    let code = extract_status_code(body)?;
    if code == 0 {
        return None;
    }
    Some(map_business_error(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_code_1000_maps_to_500_upstream_error() {
        let e = map_business_error(1000);
        assert_eq!(e.http_status, 500);
        assert_eq!(e.err_type, "upstream_error");
        assert!(!e.retryable);
        assert_eq!(e.code, "minimax_1000");
    }

    #[test]
    fn status_code_1001_maps_to_502_retryable() {
        let e = map_business_error(1001);
        assert_eq!(e.http_status, 502);
        assert_eq!(e.err_type, "upstream_error");
        assert!(e.retryable);
    }

    #[test]
    fn status_code_1004_maps_to_401_with_group_id_hint() {
        let e = map_business_error(1004);
        assert_eq!(e.http_status, 401);
        assert_eq!(e.err_type, "authentication_error");
        assert!(e.message.contains("GroupId"), "提示必须指向 GroupId：{}", e.message);
        assert!(e.message.contains("而非重新生成 key"));
        assert!(!e.retryable);
    }

    #[test]
    fn status_code_1008_maps_to_402_not_retryable() {
        let e = map_business_error(1008);
        assert_eq!(e.http_status, 402);
        assert_eq!(e.err_type, "api_error");
        assert!(!e.retryable);
        assert!(e.message.contains("充值"));
    }

    #[test]
    fn status_codes_1027_and_1039_map_to_429_retryable() {
        for code in [1027, 1039] {
            let e = map_business_error(code);
            assert_eq!(e.http_status, 429, "code={code}");
            assert_eq!(e.err_type, "rate_limit_error", "code={code}");
            assert!(e.retryable, "code={code}");
            assert_eq!(e.code, format!("minimax_{code}"));
        }
    }

    #[test]
    fn unknown_status_code_maps_to_502() {
        let e = map_business_error(7777);
        assert_eq!(e.http_status, 502);
        assert_eq!(e.err_type, "upstream_error");
        assert_eq!(e.code, "minimax_7777");
    }

    #[test]
    fn extracts_status_code_from_successful_body() {
        let body = serde_json::json!({
            "id": "x",
            "choices": [],
            "usage": {"prompt_tokens": 26, "completion_tokens": 223},
            "base_resp": {"status_code": 0, "status_msg": ""}
        });
        assert_eq!(extract_status_code(&body), Some(0));
        assert!(!is_business_error(&body));
        assert!(classify(&body).is_none());
    }

    #[test]
    fn http_200_with_business_error_is_detected() {
        // 02 §4.2 要点 3：HTTP 200 但业务错误
        let body = serde_json::json!({
            "choices": [],
            "base_resp": {"status_code": 1004, "status_msg": "invalid api key"}
        });
        assert!(is_business_error(&body));
        let e = classify(&body).expect("应识别为业务错误");
        assert_eq!(e.http_status, 401);
        assert_eq!(e.upstream_code, 1004);
    }

    #[test]
    fn non_minimax_body_is_not_business_error() {
        let body = serde_json::json!({"error": {"message": "boom"}});
        assert_eq!(extract_status_code(&body), None);
        assert!(!is_business_error(&body));
        assert!(classify(&body).is_none());
    }

    #[test]
    fn status_msg_is_extracted_when_present() {
        let body = serde_json::json!({"base_resp": {"status_code": 1001, "status_msg": "engine error"}});
        assert_eq!(extract_status_msg(&body).as_deref(), Some("engine error"));
        let body = serde_json::json!({"base_resp": {"status_code": 0, "status_msg": ""}});
        assert_eq!(extract_status_msg(&body), None);
    }
}

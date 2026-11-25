//! 敏感信息脱敏。
//!
//! 依据 02 §5.2「日志中间件全局脱敏 `Authorization` / `api_key` / `token` 键值」：
//! 本模块提供可复用的脱敏函数，供日志中间件与 IPC 层使用。

/// 敏感键名（小写比较）。命中的键其值一律替换为 `***`。
pub const SENSITIVE_KEYS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api_key",
    "apikey",
    "api-key",
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "bearer",
    "secret",
    "secret_key",
    "password",
    "passwd",
    "cookie",
    "set-cookie",
    "client_secret",
];

/// 判断键名是否敏感。
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.trim().to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|k| *k == lower)
}

/// 脱敏后的占位符。
pub const MASK: &str = "***";

/// 脱敏文本中形如 `"key": "value"` 的敏感键值。
///
/// 先尝试按 JSON 解析（结构化场景最可靠），失败则退化为文本扫描。
pub fn redact(input: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(input) {
        let mut redacted = value;
        redact_value(&mut redacted);
        return serde_json::to_string(&redacted).unwrap_or_else(|_| MASK.to_string());
    }
    redact_text(input)
}

/// 递归脱敏 JSON 值中的敏感键。
pub fn redact_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *val = serde_json::Value::String(MASK.to_string());
                } else {
                    redact_value(val);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_value(item);
            }
        }
        _ => {}
    }
}

/// 文本扫描脱敏（非 JSON 内容，如 HTTP 报文片段）。
pub fn redact_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while cursor < input.len() {
        let rest = &input[cursor..];
        let Some(quote) = rest.find('"') else {
            out.push_str(rest);
            return out;
        };
        let after_quote = &rest[quote + 1..];
        let Some((key, key_close)) = read_quoted(after_quote) else {
            out.push_str(rest);
            return out;
        };
        // 键的收尾引号绝对位置
        let key_end = cursor + quote + 1 + key_close;
        let after_key = &input[key_end + 1..];

        if is_sensitive_key(&key) {
            let trimmed = after_key.trim_start();
            if let Some(colon) = trimmed.strip_prefix(':') {
                let value_area = colon.trim_start();
                if value_area.starts_with('"') {
                    if let Some((_, value_close)) = read_quoted(&value_area[1..]) {
                        let value_end = key_end
                            + 1
                            + (after_key.len() - trimmed.len())
                            + 1
                            + (colon.len() - value_area.len())
                            + 1
                            + value_close;
                        out.push_str(&input[cursor..key_end + 1]);
                        out.push_str(": \"");
                        out.push_str(MASK);
                        out.push('"');
                        cursor = value_end + 1;
                        continue;
                    }
                }
            }
        }

        out.push_str(&input[cursor..key_end + 1]);
        cursor = key_end + 1;
    }

    out
}

/// 读取 `s[0..]` 处引号包裹的内容，返回 `(内容, 收尾引号相对偏移)`。
fn read_quoted(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut content = String::new();
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                let next = s.get(i + 1..)?;
                let ch = next.chars().next()?;
                content.push(ch);
                i += 1 + ch.len_utf8();
            }
            b'"' => return Some((content, i)),
            _ => {
                let ch = s.get(i..)?.chars().next()?;
                content.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    None
}

/// 展示用掩码：保留首尾各 4 位，中间用 `•` 代替。
///
/// 仅用于**用户主动回看后**的界面显示；列表视图不使用此函数，
/// 避免由明文派生出可关联信息。
pub fn mask_display(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        // 太短则整体遮蔽，且不暴露长度
        return "•".repeat(4);
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}{}{tail}", "•".repeat(chars.len() - 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_json_sensitive_keys() {
        let input = r#"{"api_key":"sk-abcdef123456","model":"deepseek-chat"}"#;
        let output = redact(input);
        assert!(!output.contains("sk-abcdef123456"), "明文不得残留：{output}");
        assert!(output.contains("\"api_key\":\"***\""), "应替换为掩码：{output}");
        assert!(output.contains("deepseek-chat"), "非敏感字段应保留：{output}");
    }

    #[test]
    fn redacts_authorization_header() {
        let input = r#"{"headers":{"Authorization":"Bearer sk-live-xyz","Accept":"application/json"}}"#;
        let output = redact(input);
        assert!(!output.contains("sk-live-xyz"));
        assert!(output.contains("Bearer") == false || output.contains("***"));
    }

    #[test]
    fn redacts_plain_text_payload() {
        let input = "POST /v1/chat/completions headers={\"x-api-key\":\"sk-plain-999\",\"content-type\":\"application/json\"}";
        let output = redact(input);
        assert!(!output.contains("sk-plain-999"), "明文不得残留：{output}");
        assert!(output.contains("application/json"), "非敏感内容应保留");
    }

    #[test]
    fn leaves_non_sensitive_text_untouched() {
        let input = "plain log line without secrets";
        assert_eq!(redact(input), input);
    }

    #[test]
    fn mask_display_hides_middle() {
        let masked = mask_display("sk-1234567890abcdef");
        assert!(masked.starts_with("sk-1"));
        assert!(masked.ends_with("cdef"));
        assert!(!masked.contains("56789"));
        assert_eq!(mask_display("short"), "••••");
    }

    #[test]
    fn is_sensitive_key_is_case_insensitive() {
        assert!(is_sensitive_key("Authorization"));
        assert!(is_sensitive_key("API_KEY"));
        assert!(!is_sensitive_key("model"));
    }
}

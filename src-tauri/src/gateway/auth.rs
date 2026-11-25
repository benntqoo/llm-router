//! 入站鉴权（02 §3.1）与 client key 管理。
//!
//! - client key 明文形如 `llmr_` + 32 字节随机 base62，**只在创建时展示一次**；
//! - 落库只存 SHA-256 哈希 + 前 8 位前缀；
//! - 传输：`Authorization: Bearer <key>` 或 `x-api-key`；
//! - `Host` 头必须是回环地址（防 DNS rebinding）。

use std::sync::Arc;

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use crate::gateway::errors::GatewayError;
use crate::store::Store;

/// client key 前缀。
pub const CLIENT_KEY_PREFIX: &str = "llmr_";
/// client key 随机部分长度（字节）。
pub const CLIENT_KEY_RANDOM_BYTES: usize = 32;
/// key_prefix 保存的前缀长度。
pub const KEY_PREFIX_LEN: usize = 8;

/// 鉴权通过的 client key 信息。
#[derive(Debug, Clone)]
pub struct AuthenticatedClient {
    pub id: String,
    pub label: String,
    pub key_prefix: String,
    pub rpm_limit: Option<i64>,
    pub daily_tokens: Option<i64>,
    pub expires_at: Option<i64>,
}

/// 生成新的 client key 明文。
pub fn generate_client_key() -> String {
    let raw = rand::random::<[u8; CLIENT_KEY_RANDOM_BYTES]>();
    format!("{CLIENT_KEY_PREFIX}{}", base62_encode(&raw))
}

/// 计算 client key 的 SHA-256 哈希（十六进制小写）。
pub fn hash_client_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 取展示用前缀。
pub fn key_prefix(key: &str) -> String {
    key.chars().take(KEY_PREFIX_LEN).collect()
}

/// base62 编码（字符集 `0-9A-Za-z`，无填充）。
///
/// 用于把 32 字节随机数编码成短字符串；实现为「把字节数组当大端整数，
/// 反复除以 62 取余」。
pub fn base62_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut digits: Vec<u8> = Vec::new();
    let mut value: Vec<u8> = bytes.to_vec();
    // 去掉前导零
    let first_non_zero = value.iter().position(|b| *b != 0).unwrap_or(value.len());
    value.drain(..first_non_zero);
    while !value.is_empty() {
        let mut remainder: u32 = 0;
        let mut quotient: Vec<u8> = Vec::with_capacity(value.len());
        for &byte in value.iter() {
            let current = (remainder << 8) | u32::from(byte);
            quotient.push((current / 62) as u8);
            remainder = current % 62;
        }
        digits.push(ALPHABET[remainder as usize]);
        // 去掉商的前导零
        let first_non_zero = quotient.iter().position(|b| *b != 0).unwrap_or(quotient.len());
        value = quotient[first_non_zero..].to_vec();
    }
    if digits.is_empty() {
        return (ALPHABET[0] as char).to_string();
    }
    digits.reverse();
    String::from_utf8(digits).expect("base62 字符集为 ASCII")
}

/// 校验 Host 头：必须是回环地址（允许带端口）。
pub fn check_host(headers: &HeaderMap) -> Result<(), GatewayError> {
    let Some(host) = headers.get(axum::http::header::HOST) else {
        return Err(GatewayError::invalid_request("缺少 Host 头"));
    };
    let host = host.to_str().unwrap_or_default();
    let hostname = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let hostname = hostname.trim_start_matches('[').trim_end_matches(']');
    let ok = hostname == "127.0.0.1"
        || hostname == "localhost"
        || hostname == "::1"
        || hostname.starts_with("127.");
    if ok {
        Ok(())
    } else {
        Err(GatewayError::forbidden(format!(
            "Host 校验失败：{host}（网关只接受回环地址）"
        )))
    }
}

/// 从请求头取出 client key 明文。
pub fn extract_client_key(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
        let value = value.to_str().ok()?;
        let trimmed = value.trim();
        if let Some(rest) = trimmed.strip_prefix("Bearer ") {
            let key = rest.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("bearer ") {
            let key = rest.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    if let Some(value) = headers.get("x-api-key") {
        let key = value.to_str().ok()?.trim();
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }
    None
}

/// 校验 client key 并返回其信息。
pub fn authenticate(
    store: &Arc<Store>,
    headers: &HeaderMap,
) -> Result<AuthenticatedClient, GatewayError> {
    let Some(key) = extract_client_key(headers) else {
        return Err(GatewayError::unauthorized(
            "缺少 client key：请在 Authorization: Bearer 或 x-api-key 头中提供",
        ));
    };
    let hash = hash_client_key(&key);
    let row = store
        .with_conn(|conn| -> crate::store::Result<Option<AuthenticatedClient>> {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row(
                    "SELECT id, label, key_prefix, rpm_limit, daily_tokens, expires_at
                     FROM client_key WHERE key_hash = ?1",
                    [hash.as_str()],
                    |row| {
                        Ok(AuthenticatedClient {
                            id: row.get(0)?,
                            label: row.get(1)?,
                            key_prefix: row.get(2)?,
                            rpm_limit: row.get(3)?,
                            daily_tokens: row.get(4)?,
                            expires_at: row.get(5)?,
                        })
                    },
                )
                .optional()?)
        })
        .map_err(|e| GatewayError::new(
            crate::gateway::errors::ErrorKind::ApiError,
            format!("client key 查询失败: {e}"),
        ))?;

    let Some(client) = row else {
        return Err(GatewayError::unauthorized("client key 无效"));
    };
    if let Some(expires_at) = client.expires_at {
        if expires_at <= crate::vault::now_secs() {
            return Err(GatewayError::unauthorized("client key 已过期"));
        }
    }
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_has_prefix_and_long_body() {
        let key = generate_client_key();
        assert!(key.starts_with(CLIENT_KEY_PREFIX));
        assert_eq!(key.len(), CLIENT_KEY_PREFIX.len() + 43);
        assert_ne!(key, generate_client_key(), "每次生成必须不同");
    }

    #[test]
    fn hash_is_sha256_hex_and_stable() {
        let hash = hash_client_key("llmr_test");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hash, hash_client_key("llmr_test"));
        assert_ne!(hash, hash_client_key("llmr_test2"));
    }

    #[test]
    fn base62_roundtrip_encoding_is_reversible_by_length() {
        // 32 字节 → 43~44 个 base62 字符（2^256 < 62^43 < 2^256 的关系）
        let raw: [u8; 32] = [7u8; 32];
        let encoded = base62_encode(&raw);
        assert!(encoded.len() >= 43 && encoded.len() <= 44);
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(base62_encode(&[0u8; 32]).len(), 1);
        assert_eq!(base62_encode(&[]), "0");
    }

    #[test]
    fn key_prefix_is_first_eight_chars() {
        // "llmr_" 占 5 位 → 前 8 位 = "llmr_abc"
        assert_eq!(key_prefix("llmr_abcdefghijk"), "llmr_abc");
    }

    #[test]
    fn extract_client_key_from_bearer_or_x_api_key() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer llmr_abc".parse().unwrap(),
        );
        assert_eq!(
            extract_client_key(&headers),
            Some("llmr_abc".to_string())
        );

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "llmr_xyz".parse().unwrap());
        assert_eq!(
            extract_client_key(&headers),
            Some("llmr_xyz".to_string())
        );

        let headers = HeaderMap::new();
        assert_eq!(extract_client_key(&headers), None);
    }

    #[test]
    fn host_check_accepts_loopback_only() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "127.0.0.1:8787".parse().unwrap());
        assert!(check_host(&headers).is_ok());

        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "localhost:8787".parse().unwrap());
        assert!(check_host(&headers).is_ok());

        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, "evil.example.com".parse().unwrap());
        assert!(check_host(&headers).is_err());

        let headers = HeaderMap::new();
        assert!(check_host(&headers).is_err());
    }
}

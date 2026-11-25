//! 加密原语：AES-256-GCM、Argon2id 派生、敏感内存零化。
//!
//! 依据 02 §5.1 密钥层次：
//! ```text
//! OS 原生保护（DPAPI / Keychain）或 Argon2id(主密码)
//!   └── KEK ──wrap── MasterKey (32B 随机)
//!                 └── AES-256-GCM 加密每条 credential（独立 12B nonce）
//! ```

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// AES-256 密钥长度。
pub const KEY_LEN: usize = 32;
/// GCM nonce 长度。
pub const NONCE_LEN: usize = 12;

/// 加密错误。
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("加解密失败: {0}")]
    Aead(String),

    #[error("密钥派生失败: {0}")]
    Argon2(String),

    #[error("非法参数: {0}")]
    InvalidParams(String),
}

/// 敏感字节缓冲：离开作用域时自动清零内存。
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// 从切片构造（复制一份）。
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    /// 生成指定长度的随机字节。
    pub fn random(len: usize) -> Self {
        let mut buf = vec![0u8; len];
        rand::rng().fill_bytes(&mut buf);
        Self(buf)
    }

    /// 只读访问。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// 长度。
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 复制为普通 `Vec`（调用方负责后续清零）。
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.clone()
    }

    /// 转为 UTF-8 字符串（用于回看；副本存在于调用方内存中）。
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl std::fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBytes(len={})", self.0.len())
    }
}

/// 生成随机 nonce。
pub fn random_nonce() -> Vec<u8> {
    let mut nonce = vec![0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// AES-256-GCM 加密，返回 `(密文, nonce)`。
///
/// `aad`（附加认证数据）不加密但参与认证，用于把密文绑定到记录标识，
/// 防止不同记录之间搬运密文。
pub fn encrypt(
    key: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    if key.len() != KEY_LEN {
        return Err(CryptoError::InvalidParams(format!(
            "密钥长度必须为 {KEY_LEN}，实际 {}",
            key.len()
        )));
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = random_nonce();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| CryptoError::Aead(e.to_string()))?;
    Ok((ciphertext, nonce))
}

/// AES-256-GCM 解密。
pub fn decrypt(
    key: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<SecretBytes, CryptoError> {
    if key.len() != KEY_LEN {
        return Err(CryptoError::InvalidParams(format!(
            "密钥长度必须为 {KEY_LEN}，实际 {}",
            key.len()
        )));
    }
    if nonce.len() != NONCE_LEN {
        return Err(CryptoError::InvalidParams(format!(
            "nonce 长度必须为 {NONCE_LEN}，实际 {}",
            nonce.len()
        )));
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|e| CryptoError::Aead(e.to_string()))?;
    Ok(SecretBytes(plaintext))
}

/// Argon2id 派生 KEK。
///
/// 02 §5.1 指定参数：m=64MiB、t=3、p=1（`m_cost` 以 KiB 计）。
pub fn derive_kek_argon2id(
    password: &str,
    salt: &[u8],
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
) -> Result<SecretBytes, CryptoError> {
    use argon2::{Algorithm, Argon2, Params, Version};

    let params = Params::new(m_cost, t_cost, p_cost, None)
        .map_err(|e| CryptoError::Argon2(format!("参数非法: {e}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| CryptoError::Argon2(format!("派生失败: {e}")))?;
    Ok(SecretBytes(out.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = SecretBytes::random(KEY_LEN);
        let plaintext = b"sk-test-do-not-log-123456";
        let aad = b"cred_1";

        let (ciphertext, nonce) = encrypt(key.as_bytes(), plaintext, aad).expect("加密失败");
        assert_ne!(ciphertext, plaintext.to_vec(), "密文不得等于明文");
        assert_eq!(nonce.len(), NONCE_LEN);

        let decrypted = decrypt(key.as_bytes(), &nonce, &ciphertext, aad).expect("解密失败");
        assert_eq!(decrypted.as_bytes(), plaintext);
    }

    #[test]
    fn nonce_is_unique_per_encryption() {
        let key = SecretBytes::random(KEY_LEN);
        let (_, n1) = encrypt(key.as_bytes(), b"same", b"a").expect("加密失败");
        let (_, n2) = encrypt(key.as_bytes(), b"same", b"a").expect("加密失败");
        assert_ne!(n1, n2, "同一密钥下 nonce 不得重复");
    }

    #[test]
    fn wrong_key_fails() {
        let key = SecretBytes::random(KEY_LEN);
        let other = SecretBytes::random(KEY_LEN);
        let (ciphertext, nonce) = encrypt(key.as_bytes(), b"secret", b"a").expect("加密失败");
        assert!(decrypt(other.as_bytes(), &nonce, &ciphertext, b"a").is_err());
    }

    #[test]
    fn wrong_aad_fails() {
        let key = SecretBytes::random(KEY_LEN);
        let (ciphertext, nonce) = encrypt(key.as_bytes(), b"secret", b"cred_1").expect("加密失败");
        assert!(decrypt(key.as_bytes(), &nonce, &ciphertext, b"cred_2").is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = SecretBytes::random(KEY_LEN);
        let (mut ciphertext, nonce) = encrypt(key.as_bytes(), b"secret", b"a").expect("加密失败");
        ciphertext[0] ^= 0xff;
        assert!(decrypt(key.as_bytes(), &nonce, &ciphertext, b"a").is_err());
    }

    #[test]
    fn argon2id_derives_deterministic_key() {
        // 单测用小参数（1MiB）以控制耗时；生产参数为 m=64MiB,t=3,p=1（02 §5.1）
        let salt = b"0123456789abcdef";
        let k1 = derive_kek_argon2id("pw", salt, 1024, 3, 1).expect("派生失败");
        let k2 = derive_kek_argon2id("pw", salt, 1024, 3, 1).expect("派生失败");
        let k3 = derive_kek_argon2id("other", salt, 1024, 3, 1).expect("派生失败");
        assert_eq!(k1.as_bytes(), k2.as_bytes());
        assert_ne!(k1.as_bytes(), k3.as_bytes());
        assert_eq!(k1.len(), KEY_LEN);
    }

    #[test]
    fn secret_bytes_debug_does_not_leak_content() {
        let secret = SecretBytes::from_slice(b"sk-super-secret");
        let text = format!("{secret:?}");
        assert!(!text.contains("sk-super-secret"), "Debug 输出不得包含明文");
        assert!(text.contains("len=15"));
    }
}

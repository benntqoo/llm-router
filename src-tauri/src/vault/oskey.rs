//! 主密钥的 OS 原生保护层。
//!
//! - Windows：DPAPI `CryptProtectData`（绑定当前用户，不使用 `CRYPTPROTECT_LOCAL_MACHINE`）；
//! - macOS：Keychain 通用密码项（`kSecClassGenericPassword`）。
//!
//! 保护对象是 **KEK**（32B 随机），KEK 再包裹主密钥；这样 vault.bin 的格式在两个平台一致，
//! 且主密钥在任何情况下都不以明文落盘。

/// OS 原生保护相关错误。
#[derive(Debug, thiserror::Error)]
pub enum OsKeyError {
    #[error("当前平台不支持 OS 原生密钥保护")]
    UnsupportedPlatform,

    #[error("OS 原生保护调用失败: {0}")]
    Os(String),
}

/// Keychain 中存放 KEK 的服务名。
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "com.llmrouter.app";
/// Keychain 中存放 KEK 的账号名。
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "vault-kek";
/// Keychain 模式下 `protect` 返回的标记（真正的密钥存在 Keychain 里）。
#[cfg(target_os = "macos")]
const KEYCHAIN_MARKER: &[u8] = b"keychain:v1";

/// 用 OS 原生能力保护一段字节，返回可安全落盘的 blob。
#[cfg(windows)]
pub fn protect(data: &[u8]) -> Result<Vec<u8>, OsKeyError> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|e| OsKeyError::Os(format!("CryptProtectData 失败: {e}")))?;

        let blob = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(blob)
    }
}

/// 解开 OS 原生保护过的 blob。
#[cfg(windows)]
pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, OsKeyError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: blob.len() as u32,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|e| OsKeyError::Os(format!("CryptUnprotectData 失败: {e}")))?;

        let data = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(data)
    }
}

/// 用 OS 原生能力保护一段字节（macOS：写入 Keychain）。
#[cfg(target_os = "macos")]
pub fn protect(data: &[u8]) -> Result<Vec<u8>, OsKeyError> {
    use security_framework::passwords::set_generic_password;

    set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, data)
        .map_err(|e| OsKeyError::Os(format!("Keychain 写入失败: {e}")))?;
    Ok(KEYCHAIN_MARKER.to_vec())
}

/// 解开 OS 原生保护过的 blob（macOS：从 Keychain 读取）。
#[cfg(target_os = "macos")]
pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, OsKeyError> {
    use security_framework::passwords::get_generic_password;

    if !blob.starts_with(KEYCHAIN_MARKER) {
        return Err(OsKeyError::Os("Keychain 标记无效".to_string()));
    }
    get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .map_err(|e| OsKeyError::Os(format!("Keychain 读取失败: {e}")))
}

/// 其它平台不支持。
#[cfg(not(any(windows, target_os = "macos")))]
pub fn protect(_data: &[u8]) -> Result<Vec<u8>, OsKeyError> {
    Err(OsKeyError::UnsupportedPlatform)
}

/// 其它平台不支持。
#[cfg(not(any(windows, target_os = "macos")))]
pub fn unprotect(_blob: &[u8]) -> Result<Vec<u8>, OsKeyError> {
    Err(OsKeyError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_unprotect_round_trip() {
        let secret = b"kek-material-32-bytes-long-abc";
        let blob = protect(secret).expect("保护失败");
        assert_ne!(blob.as_slice(), secret, "blob 不得等于原文");
        let restored = unprotect(&blob).expect("解保护失败");
        assert_eq!(restored, secret);
    }

    #[test]
    fn unprotect_rejects_garbage() {
        assert!(unprotect(b"not-a-real-dpapi-blob").is_err());
    }
}

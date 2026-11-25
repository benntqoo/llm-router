//! T2 加密保险库 —— 端到端验收测试（对齐任务卡 T2 验收判据）。
//!
//! 判据：
//! 1. 写入一条假 key 后，`vault.bin` 无明文；
//! 2. 重启（重新从磁盘加载）后仍可解密；
//! 3. 数据库同样不得出现明文。

use std::sync::Arc;

use llm_router_lib::store::Store;
use llm_router_lib::vault::crypto::SecretBytes;
use llm_router_lib::vault::Vault;

/// 假 key（用于检验明文是否泄漏）。
const FAKE_KEY: &[u8] = b"sk-fake-acceptance-key-0123456789abcdef";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn acceptance_vault_file_has_no_plaintext_and_restart_can_decrypt() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault_path = dir.path().join("vault.bin");

    // ── 首次运行：写入假 key
    let credential_id = {
        let vault = Vault::open(store.clone(), &vault_path, None).expect("打开保险库");
        let channel = vault.ensure_channel("DeepSeek 官方").expect("渠道");
        vault
            .put_credential(&channel, "api_key", &SecretBytes::from_slice(FAKE_KEY))
            .expect("写入凭证")
    };

    // 判据 1：vault.bin 不得含明文
    let vault_bytes = std::fs::read(&vault_path).expect("读取 vault.bin");
    assert!(
        !contains(&vault_bytes, FAKE_KEY),
        "vault.bin 出现明文密钥"
    );
    assert!(
        !contains(&vault_bytes, b"sk-fake"),
        "vault.bin 出现疑似密钥前缀"
    );
    assert!(
        !contains(&vault_bytes, b"Bearer "),
        "vault.bin 出现 Bearer 头"
    );

    // 判据 1b：数据库同样不得含明文
    let db_bytes = std::fs::read(store.path()).expect("读取数据库");
    assert!(!contains(&db_bytes, FAKE_KEY), "数据库出现明文密钥");
    assert!(!contains(&db_bytes, b"sk-fake"), "数据库出现疑似密钥前缀");

    // ── 模拟重启：重新从磁盘加载保险库
    let reopened = Vault::open(store, &vault_path, None).expect("重启后打开保险库");
    let revealed = reopened
        .get_credential(&credential_id)
        .expect("重启后解密凭证");
    assert_eq!(
        revealed.as_bytes(),
        FAKE_KEY,
        "重启后解密结果必须与原文一致"
    );

    // 元信息不得携带任何密文/明文
    let meta = reopened
        .list_credentials(None)
        .expect("列出凭据")[0]
        .clone();
    let json = serde_json::to_string(&meta).expect("序列化元信息");
    assert!(!json.contains("sk-fake"), "元信息不得含明文");
    assert!(!json.contains("ciphertext"), "元信息不得含密文字段");
}

#[test]
fn acceptance_each_credential_gets_unique_ciphertext() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Vault::open(store, dir.path().join("vault.bin"), None).expect("打开保险库");
    let channel = vault.ensure_channel("Kimi").expect("渠道");

    vault
        .put_credential(&channel, "api_key", &SecretBytes::from_slice(FAKE_KEY))
        .expect("写入 1");
    vault
        .put_credential(&channel, "api_key", &SecretBytes::from_slice(FAKE_KEY))
        .expect("写入 2");

    let rows: Vec<(String, Vec<u8>, Vec<u8>)> = vault
        .store()
        .with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, ciphertext, nonce FROM credential ORDER BY created_at")?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ));
            }
            Ok(out)
        })
        .expect("读取密文");

    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].1, rows[1].1, "相同明文的密文必须不同");
    assert_ne!(rows[0].2, rows[1].2, "每条记录必须使用独立 nonce");
}

#[test]
fn acceptance_rotation_keeps_data_readable_and_writes_new_version() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault_path = dir.path().join("vault.bin");

    let id = {
        let vault = Vault::open(store.clone(), &vault_path, None).expect("打开保险库");
        let channel = vault.ensure_channel("MiniMax 国内").expect("渠道");
        vault
            .put_credential(&channel, "api_key", &SecretBytes::from_slice(FAKE_KEY))
            .expect("写入凭证")
    };

    let mut vault = Vault::open(store.clone(), &vault_path, None).expect("打开保险库");
    let new_version = vault.rotate_master_key(None).expect("轮换主密钥");
    assert_eq!(new_version, 2);

    let vault_bytes = std::fs::read(&vault_path).expect("读取 vault.bin");
    assert!(!contains(&vault_bytes, FAKE_KEY), "轮换后 vault.bin 仍不得含明文");

    // 轮换后重启仍可解密
    let reopened = Vault::open(store, &vault_path, None).expect("重启后打开保险库");
    let revealed = reopened.get_credential(&id).expect("解密");
    assert_eq!(revealed.as_bytes(), FAKE_KEY);
}

/// 手工验收辅助：在指定目录种一条凭据，用于界面联调。
///
/// 用法：
/// ```text
/// LLMR_SEED_DIR=<目录> cargo test --test vault_e2e -- --ignored seed_demo_credential --nocapture
/// ```
#[test]
#[ignore]
fn seed_demo_credential() {
    let dir = std::env::var("LLMR_SEED_DIR").expect("需要 LLMR_SEED_DIR 环境变量");
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("创建目录");
    let store = Arc::new(Store::open(dir.join("store.db")).expect("打开库"));
    let vault_path = dir.join("vault.bin");
    let vault = Vault::open(store, &vault_path, None).expect("打开保险库");
    let channel = vault.ensure_channel("DeepSeek 官方").expect("渠道");
    let id = vault
        .put_credential(
            &channel,
            "api_key",
            &SecretBytes::from_slice(FAKE_KEY),
        )
        .expect("写入凭证");
    println!("已写入凭据 {id} 到 {}", vault_path.display());
}

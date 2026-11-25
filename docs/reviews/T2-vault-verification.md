# T2 验收留档 — 加密保险库

| 项 | 值 |
|---|---|
| 任务 | T2 加密保险库（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-25 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 结论 | **通过**（判据逐条实测见 §1；另有一项需头头裁决的口径问题见 §6） |

## 1. 判据对照

任务卡原文：*目标：02 §5 全量。DPAPI/Keychain 主密钥、AES-256-GCM、Argon2id 可选模式、IPC 脱敏、回看命令。验收：写入一条假 key 后 `strings vault.bin` 无明文；重启可解密；30s 自动遮蔽生效。*

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | DPAPI/Keychain 主密钥 | ✅（Windows 实测） | `vault::oskey::tests::protect_unprotect_round_trip` 走真实 `CryptProtectData`/`CryptUnprotectData` |
| 2 | AES-256-GCM | ✅ | 每条凭据独立 12B nonce + AAD=凭据 id；改密文/换密钥/换 AAD 均认证失败 |
| 3 | Argon2id 可选模式 | ✅ | m=64MiB/t=3/p=1；错误主密码 → `PasswordMismatch`；启用后重启进入锁定态 |
| 4 | IPC 脱敏 | ✅ | 除回看/明文导出外，命令只回元信息；`vault::redact` 6 个单测覆盖 JSON/文本脱敏 |
| 5 | 回看命令 | ✅ | `vault_reveal_secret` 显式返回明文，界面 30s 自动遮蔽 |
| 6 | `strings vault.bin` 无明文 | ✅ | 字节级扫描：`vault.bin` 无任何明文命中（§2.3） |
| 7 | 重启可解密 | ✅ | 端到端用例：写入 → 重新从磁盘加载 → 解密结果与原文一致 |
| 8 | 30s 自动遮蔽生效 | ✅ | 真机界面实测：明文 + 倒计时 → 33s 后明文消失（§2.4 前后对比截图） |

## 2. 实测记录

### 2.1 Rust 测试

```
$ cd src-tauri && cargo test
test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 3 passed; 0 failed; ...                                      # tests/vault_e2e.rs 端到端
```

关键用例：

| 用例 | 覆盖点 |
|---|---|
| `vault::oskey::tests::protect_unprotect_round_trip` | 真实 DPAPI 往返（Windows 当前用户绑定） |
| `vault::tests::credential_round_trip_and_no_plaintext_in_vault_file` | 写入→解密一致；vault.bin 与数据库均无明文 |
| `vault::tests::reopen_after_restart_decrypts` | 重启（重新加载）后可解密 |
| `vault::tests::rotate_master_key_reencrypts_and_old_key_still_works` | 轮换后新旧都能解，重启后仍可解 |
| `vault::tests::password_mode_requires_password_and_rejects_wrong_one` | 锁定态 / 错误密码 / `unlock()` 路径 |
| `acceptance_*`（3 个） | 端到端验收判据：文件无明文、密文唯一性、轮换后数据可读 |

### 2.2 前端测试

```
$ node --experimental-strip-types --test src/vault/reveal.test.ts
# tests 5 / # pass 5 / # fail 0
```

覆盖：初始遮蔽、30s 内可见、第 30 秒到期即遮蔽、剩余秒数递减、重复回看重新计时。

### 2.3 文件明文扫描（AGENT.md §9 的 Windows 等价写法）

本机无 `sqlite3` CLI 与 Unix `strings`/`grep`，改用 Python 直读二进制做模式扫描（脚本见 §7 附注）。

```
--- vault.bin (596 bytes) ---
  无明文命中
--- store.db (126976 bytes) ---
  HIT 'api_key' @1421: b' NOT NULL,           -- api_key|oauth_gh_token|oauth_copilot_tok'
  HIT 'api_key' @3346: b' NOT NULL,           -- api_key | oauth | session\n  provider    '
  HIT 'api_key' @12270: b'e5e0be711DeepSeek 官方api_keycustom...'
  HIT 'api_key' @61358: b'815eb31b07afc44e5e0be711api_key\x9c\x99\t\xad\x04\xaa[\xf17\x15...'
```

命中全部是 **`credential.kind` 的取值 `'api_key'` 与建表 DDL 注释**，不是密钥明文；
`sk-` / `Bearer ` / 假密钥全文均**零命中**。

> 该结果引出 §6 的验收口径问题，需头头裁决。

`vault.bin` 实际内容（全部为密文）：

```json
{
  "format_version": 1,
  "key_version": 1,
  "key": {
    "kdf": { "type": "os", "os_blob": "AQAAANCMnd8BFdERjHoAwE/Cl+sBAAAA0UtcM161MU23b/1z8xNXXw..." },
    "kek_nonce": "Q+gDlFlmHA+EDo4/",
    "wrapped_key": "GG+HmP47x+YKC8Sr7NUQyRz4Z4JXGGZ10H23vxxzT0as4AOUYEwct2+qfxxDmK3q"
  }
}
```

### 2.4 界面实测：回看 + 30s 自动遮蔽

真机（`npm run tauri dev`）操作步骤：Tab 聚焦到凭据行的「显示」→ 回车。

| 时刻 | 现象 | 证据 |
|---|---|---|
| t+2s | 显示明文 `sk-fake-acceptance-key-0123456789abcdef`，旁注「28s 后自动遮蔽」，出现「隐藏」按钮 | `assets/T2-reveal-30s.png` |
| t+35s | 明文消失，隐藏按钮消失，只剩元信息 | `assets/T2-reveal-masked.png` |
| — | 保险库面板全貌（状态/录入/列表/导出/主密码模式） | `assets/T2-vault-panel.png` |

回看后重新扫描 `vault.bin` 与 `store.db`：**仍无明文命中**（明文只存在于进程内存与界面，不落盘）。

### 2.5 默认路径落盘

```
$ llm-router.exe        # 不带任何环境变量
默认数据目录 %APPDATA%\llm-router\:
  store.db   4096
  vault.bin  596      ← 首次启动自动创建
扫描 vault.bin → 无明文命中
```

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/vault/crypto.rs` | AES-256-GCM 封装（独立 nonce + AAD 绑定）、Argon2id 派生、`SecretBytes` 零化 |
| `src-tauri/src/vault/oskey.rs` | Windows DPAPI（绑定当前用户）/ macOS Keychain；非支持平台直接报错 |
| `src-tauri/src/vault/redact.rs` | 敏感键脱敏（JSON 递归 + 文本扫描）、展示掩码 |
| `src-tauri/src/vault/mod.rs` | `vault.bin` 格式、凭证存取、主密钥轮换、加密备份导出、主密码锁定/解锁 |
| `src-tauri/src/commands/vault.rs` | 12 个 IPC 命令（含回看、明文导出二次确认） |
| `src/vault/reveal.ts` + `reveal.test.ts` | 30s 遮蔽逻辑（纯函数）+ 5 个单测 |
| `src/VaultPanel.tsx` | 保险库面板：录入/回看/删除/导出/主密码模式 |
| `src-tauri/tests/vault_e2e.rs` | 端到端验收用例（3 个）+ 界面联调用种入工具 |

## 4. 密钥层次（落地口径）

```text
OS 原生保护(DPAPI/Keychain) 或 Argon2id(主密码)  ──► KEK(32B)
    └── AES-256-GCM wrap ──► MasterKey(32B 随机)
        └── AES-256-GCM（独立 12B nonce，AAD=凭据 id）──► credential.ciphertext
```

**与 02 §5.1 图示的差异（已实现，需知悉）**：图示为「OS 原生保护 wraps MasterKey」一层；
实现改为「OS 原生保护 wraps KEK，KEK 再 wrap MasterKey」两层。原因：macOS Keychain
以「独立密码项」存储，不适合直接承载任意长度 blob，引入 KEK 层可让两个平台的
`vault.bin` 格式完全一致。主密钥在两种模式下都不会明文落盘，性质与图示一致。

## 5. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| 主密码模式的启动语义 | 启用主密码后，**下次启动**保险库处于锁定态（应用正常启动并提示解锁），而不是拒绝启动。理由：应用本体不含密钥，锁定态更符合"锁住数据而非锁死应用" |
| 错误主密码的识别 | Argon2id 对任何输入都能派生出密钥，错误密码表现为 GCM 认证失败；已统一映射为 `PasswordMismatch` |
| 占位渠道 | `credential.channel_id` 有外键约束，录入凭据时会按名称 upsert 一条 `provider='custom'` 的渠道行；**T8/T9 接管渠道管理后应由正规渠道 CRUD 取代** |
| 主密钥轮换顺序 | 先落盘（新密钥为主、旧密钥记为 prev）再逐条重加密；中途崩溃时残留记录按各自 `key_version` 选对应密钥解密，不会锁死 |
| 回看明文经 IPC 返回前端 | 这是 02 §5.2 明确要求的能力（前端永不收到明文，**除非显式调用回看命令**）；实现仅在回看与明文导出两条路径放行 |
| 明文导出 | 必须显式 `confirm=true`，否则命令直接拒绝、不做任何解密；界面另有勾选确认与警示文案 |
| 加密备份导出 | 复制 `vault.bin` + `store.db`（全为密文）到 `%APPDATA%\llm-router\backup\vault-<时间戳>` |
| 日志中间件 | `vault::redact` 已就绪（JSON/文本脱敏），**尚未接入网关日志**——网关日志随 T3 一并接入 |
| 单连接串行化 | 沿用 T1 的 `Arc<Mutex<Connection>>`；axum 处理函数内访问数据库仍需 `spawn_blocking`（约束已写在 `store::Store` 注释里） |

## 6. 需头头裁决：安全验收命令的正则口径（AGENT.md §9）

AGENT.md §9 的验收命令是：

```
1. sqlite3 <store.db> ".dump" | grep -iE "sk-|Bearer |api_key"   # 无明文命中
```

实测（§2.3）表明：数据库里**必然**会出现 `api_key` 字样——一处是建表 DDL 注释，
一处是 `credential.kind` 的取值。按字面执行这条命令会**恒定命中**，无法区分
"DDL 注释/枚举值"与"真实密钥明文"，M1 验收会被误判为不通过。

建议（择一，需裁决）：

1. 把正则收窄为密钥形态：`grep -iE "sk-|Bearer |api[_-]?key[\"']?\s*[:=]\s*[\"'][A-Za-z0-9_-]{16,}"`；
2. 或只扫数据行、排除 `sqlite_master`：`.dump` 后过滤 `CREATE TABLE` 行；
3. 或在 §9 注明"`api_key` 命中需人工判读（DDL 注释与 `kind` 枚举除外）"。

**未擅自修改 AGENT.md**，按 AGENT.md §6-3 走提案流程。

## 7. 未验证 / 未做

- **macOS Keychain 分支未验证**：本机为 Windows，`security-framework` 代码路径**未编译验证**
  （需 macOS 环境或 `cargo check --target x86_64-apple-darwin`）。Windows DPAPI 路径为实测通过。
- **日志中间件未接入**：脱敏函数已实现并有单测，但网关/Tauri 日志的全局脱敏随 T3 接入。
- **崩溃转储防护**：release profile 已有 `panic = "abort"`；敏感结构已用 `zeroize` 零化。
  完整的"禁止 core dump 含敏感页"在 Windows 上无直接等价手段（02 §5.2 已注明），
  按"关键结构 zeroize"方案落地。
- **凭证导出 `.env` 格式**：本次只实现 JSON 明文导出，`.env` 格式按需再加。

## 8. 附注：本机验收命令的等价写法

```powershell
# 替代 `strings vault.bin | grep -iE "sk-|Bearer "`：
python <scan.py> "$env:APPDATA\llm-router\vault.bin"     # 二进制模式扫描，输出命中上下文

# 替代 `sqlite3 store.db ".dump" | grep ...`：
python -c "import sqlite3; ..."                          # Python sqlite3 模块导出数据行
```

扫描脚本要点：按字节查找 `sk-` / `Bearer ` / 假密钥全文等模式，命中时打印上下文；
命中 `api_key` 需人工判读（见 §6）。

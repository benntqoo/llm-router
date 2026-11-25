# T1 验收留档 — 数据层（SQLite WAL + 迁移框架 + 全量建表）

| 项 | 值 |
|---|---|
| 任务 | T1 数据层（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-25 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 结论 | **通过**（判据逐条实测见 §1） |

## 1. 判据对照

任务卡原文：*目标：SQLite WAL + migration 框架 + 全部建表（02 §9 与 01 §4.4 的 DDL 全量）。验收：`cargo test` 覆盖 migrate up/down；空库启动自动建表。*

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | SQLite + WAL | 通过 | `PRAGMA journal_mode` = `wal`（文件库实测）；驱动 `rusqlite 0.43`（`bundled`，无 CGO、无系统 sqlite 依赖） |
| 2 | migration 框架 | 通过 | `store/migrations.rs`：版本表 `schema_migrations` + up/down 编排 + 单迁移事务化 |
| 3 | 全部建表（01 §4.4 DDL 全量 + 02 §9 修订） | 通过 | 11 张业务表全部存在（见 §2.2 表清单） |
| 4 | `cargo test` 覆盖 migrate up/down | 通过 | 15 passed / 0 failed（10 个 store 相关用例，见 §2.1） |
| 5 | 空库启动自动建表 | 通过 | 空目录启动应用 → 自动建库并迁移到 v4（见 §2.3） |

## 2. 实测记录

### 2.1 单测

```
$ cd src-tauri && cargo test
test store::tests::migrate_up_creates_all_v1_tables ... ok
test store::tests::migrate_up_is_idempotent ... ok
test store::tests::migrate_down_drops_tables_step_by_step ... ok
test store::tests::migrate_down_to_rolls_back_partially ... ok
test store::tests::file_store_uses_wal_mode ... ok
test store::tests::foreign_keys_are_enforced ... ok
test store::tests::model_alias_is_globally_unique ... ok
test store::tests::account_has_meta_json_column_for_session_channels ... ok
test store::tests::opening_empty_path_creates_database_with_tables ... ok
test store::tests::default_db_path_is_under_app_data_dir ... ok
（另有 T0 的 5 个 gateway 用例）

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

覆盖要点：up 建齐全部表 / up 幂等 / down 逐条回滚到底（版本倒序）/ down_to 部分回滚后可重新 up / 文件库 WAL / 外键约束生效 / 别名全局唯一 / `account.meta_json` 存在（02 §9）/ 空路径自动建库建表。

### 2.2 空库启动后的实际库结构

应用启动（`LLMR_STORE_PATH` 指向一个不存在的文件）后，用 Python `sqlite3` 模块读回（**本机无 `sqlite3` CLI**，等价替代见 T0 留档 §4）：

```
tables : ['account', 'channel', 'checkin_result', 'checkin_site', 'client_key',
          'credential', 'model', 'price', 'route_group', 'route_member',
          'schema_migrations', 'sqlite_sequence', 'usage_log']
indexes: ['idx_model_alias', 'idx_usage_ts']
journal_mode: wal
migrations: [(1, 'core_channel_model_route'), (2, 'credentials_account_client_key'),
             (3, 'usage_and_price'), (4, 'checkin')]
account cols: ['id', 'channel_id', 'label', 'quota_json', 'enabled', 'meta_json']
credential cols: ['id', 'channel_id', 'kind', 'ciphertext', 'nonce', 'key_version',
                  'expires_at', 'created_at', 'updated_at']
route_group cols: ['id', 'alias', 'strategy']
```

与 01 §4.4 DDL + 02 §9 修订逐列一致，其中三处 02 §9 修订已落地：

| 修订 | 落地情况 |
|---|---|
| `credential.kind` 增加 `oauth_gh_token` / `oauth_copilot_token` / `session_workbuddy` | ✅ 列注释已更新（取值为应用层约束） |
| `account` 增加 `meta_json`（WorkBuddy 登录态路径引用） | ✅ 已加列，单测覆盖 |
| `route_group.strategy` 增 `sticky_session` | ✅ 取值为应用层约束，DDL 注释与 01 文档同步 |

> 注：`PRAGMA foreign_keys` 按连接生效、不落盘，外部工具读到的 `0` 不代表未开启；Rust 侧连接在 `Store::from_connection` 中显式开启，单测 `foreign_keys_are_enforced` 已验证外键确实拦截非法写入。

### 2.3 空库启动自动建表

```
$ LLMR_STORE_PATH=<临时目录>/store.db llm-router.exe     # 目标文件不存在
[store] 数据库 <临时目录>/store.db（schema v4）
```

随后读取该文件 → 12 张表 + 2 个索引齐备（§2.2）。应用启动即建库建表，无需任何手工步骤。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/store/schema.rs` | **DDL 唯一权威载体**：01 §4.4 全量 + 02 §9 修订，每表一个常量 |
| `src-tauri/src/store/migrations.rs` | 迁移框架：版本表、up/down 编排、单迁移事务化、`migrate_up` / `migrate_down` / `migrate_down_to` |
| `src-tauri/src/store/mod.rs` | `Store`（打开、PRAGMA、迁移入口）、路径解析（`%APPDATA%\llm-router\store.db`，`LLMR_STORE_PATH` 可覆盖）、10 个单测 |
| `src-tauri/src/store/error.rs` | `StoreError`（Io / Sqlite / Migration） |
| `src-tauri/src/lib.rs` | 启动时打开库并迁移到最新版本；`Store` 进 `AppState` |

**迁移分 4 版**（而非一次性建齐）是为了让 down 有语义化粒度：

| 版本 | 名称 | 内容 |
|---|---|---|
| 1 | `core_channel_model_route` | channel / model / route_group / route_member |
| 2 | `credentials_account_client_key` | credential / account / client_key |
| 3 | `usage_and_price` | usage_log / price |
| 4 | `checkin` | checkin_site / checkin_result |

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| 迁移框架自研，未引第三方 crate | 需要精确控制 up/down 语义与事务边界；`refinery` 等依赖未评估，保持依赖面小 |
| 单连接 + `Mutex` 串行化 | 单进程本地应用够用；**axum 处理函数内访问数据库必须走 `spawn_blocking`**，否则会阻塞 tokio 工作线程（T3 接网关时按此约束实现） |
| `credential.kind` / `route_group.strategy` 未加 CHECK 约束 | 取值集合由应用层校验；加 CHECK 会让后续新增取值需要改表，代价大于收益 |
| `usage_log` 未按渠道/模型/client_key 建索引 | 01 §4.4 DDL 只定义了 `idx_usage_ts`；四维聚合的索引需求留到 T7/T14 与 02 §8 限额自学习一并定 |
| 库路径 | `%APPDATA%\llm-router\store.db`，与 02 §5 的 `vault.bin` 同目录（T2 复用 `store::app_data_dir()`） |
| `store.db-wal` / `store.db-shm` | WAL 边车文件，已被 `.gitignore` 的 `*.db-wal` / `*.db-shm` 覆盖 |

## 5. 未做

- 渠道/模型/别名/路由的 CRUD 与查询（属 T5 路由、T8 预设、T9 UI）
- 密文读写（属 T2 保险库；本层只存 `credential.ciphertext` / `nonce` 列）
- `usage_log` 落库写入（属 T7）

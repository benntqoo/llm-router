# T11 验收留档 — M1 安全验收（02 §10.2 四条判据）

| 项 | 值 |
|---|---|
| 任务 | T11 安全验收（03 §2 任务卡） |
| 依据 | `docs/planning/02-technical-design.md` §10.2（v0.10）、§5.2；`AGENT.md` §5 / §9；`docs/planning/03-delivery-plan.md` T11 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `41fd1de`（日志脱敏中间件 `gateway/logging.rs` + 安全验收执行器 `scripts/security_check.py`）；文档提交另计 |
| 结论 | **通过**（四条判据全过）。过程中发现并补齐一处**规范与实现的缺口**：02 §5.2 承诺的「日志中间件全局脱敏」此前**从未接入**（T2 §7 就记着"随 T3 接入"，T3 已过但一直没接）——本轮补上并纳入判据 3 的证据 |

> 本文件改动范围：仅记录 T11 的验收判据、实测证据、交付物与取舍；**不含**后续任务的结论。

## 1. 判据对照

任务卡验收原文：「目标：02 §10.2 四条判据全过。验收：判据记录进 `docs/reviews/security-check-m1.md`。」

| # | 判据（02 §10.2 原文） | 实测 | 证据 |
|---|---|---|---|
| 1 | `sqlite3 store.db ".dump" \| grep -iE "sk-\|api_key\|Bearer"` → **无明文命中** | ✅ | §2.1：dump 199 行 + 主库/WAL/SHM 原始字节全扫 → **明文凭证命中 0 条**；结构性命中 26 条全是 DDL 注释与 `kind='api_key'` 枚举值（逐条可核） |
| 2 | `vault.bin` 用十六进制查看 → **无明文 key 片段** | ✅ | §2.2：596 字节原始字节 + 十六进制视图 + 可打印片段三重扫描 → **0 命中** |
| 3 | 日志文件 `grep Authorization` → **全部脱敏为 `***`** | ✅ | §2.3：3 条含敏感键的日志行全部为 `"authorization":"***"`；两个已知明文凭证**都不在** stdout/stderr |
| 4 | 监听 `0.0.0.0` 且无 client key → **启动拒绝并告警** | ✅ | §2.4：进程退出 + 明确告警文案；端口**未监听**；反向用例（有 client key）正常启动 |

## 2. 实测记录

### 2.1 判据 1：库内无明文凭证

被测对象：**已实际使用的临时库**（T10 验证库 `%TEMP%\llmr-t10p-verify\store.db`，
含 1 条渠道 + 1 条模型 + **1 条已录密钥的凭据** + 1 把 client key + 53 条 usage_log 流水）。
不是空库——空库扫不出任何东西。

扫描面（三层）：

| 层 | 内容 | 大小 |
|---|---|---|
| 逻辑 dump | `.dump` 等价内容（`sqlite_master` + 全表 INSERT） | 199 行 |
| 原始字节 | `store.db` 主库 | 4 096 B |
| 原始字节 | `store.db-wal`（**只扫主库会漏掉 WAL 里的新数据**） | 943 512 B |
| 原始字节 | `store.db-shm` | 32 768 B |

结果：

```
明文凭证命中：0 条（必须为 0）
结构性命中（DDL 注释 / 字段名 / 枚举值，非明文）：26 条
  [note] store.db dump: kind  TEXT NOT NULL,  -- api_key | oauth | session
  [note] store.db dump: INSERT INTO "channel" VALUES(...,'api_key','deepseek','http://127.0.0.1:...
  [note] store.db dump: kind  TEXT NOT NULL,  -- api_key|oauth_gh_token|oauth_copilot_token|session_workbuddy
  [note] store.db dump: INSERT INTO "credential" VALUES('cred_...','ch_...','api_key',X'A4F1...
```

**分类口径**（本轮新增，见 §4）：命中项分两类——

- **明文凭证命中**（判 FAIL）：命中真实密钥值（用 `--secret` 传入的已知明文比对）
  或匹配凭证形态（`sk-…` / `llmr_…` / `Bearer …`）。**必须为 0**。
- **结构性命中**（不计失败）：DDL 注释、列名、`credential.kind`/`channel.kind` 的枚举值。
  逐条列出供人工核对，不静默忽略。

库内两条关键行的实际形态（dump 原文）：

```sql
-- client key：只存 SHA-256 与前 8 位前缀，无明文
INSERT INTO "client_key" VALUES('ck_7f99…','pi',
  '5ec45c9de64ac3100fa6b6bebbe5103fc3cf19f74368825c93478974ec68419e','llmr_ukh',…);

-- 渠道密钥：AES-256-GCM 密文 + 独立 nonce，无明文
INSERT INTO "credential" VALUES('cred_619c…','ch_5d79…','api_key',
  X'A4F15CA0BA7ADFC45C728D11BF9E33FCE20FA5448663F4482D01D6D617A193',
  X'248C8E39AE433B6B14F6649B',1,NULL,…);
```

### 2.2 判据 2：保险库文件无明文

`vault.bin`（596 B）三层扫描：

| 扫描方式 | 命中 |
|---|---|
| 原始字节（latin-1 视图） | 0 |
| 十六进制视图（找明文的 hex 片段） | 0 |
| 可打印片段（≥12 连续可打印字符）里匹配凭证形态 | 0 |

保险库文件只存 `{"kdf":…, "wrapped_key":…, "key_version":…}`；**渠道密钥密文本身在数据库的
`credential.ciphertext` 里**（同样是密文）。两处都不含明文。

### 2.3 判据 3：日志脱敏

**先说结论：这条判据在本次之前是没有机制支撑的。** 02 §5.2 写的是
「IPC 边界：…；**日志中间件全局脱敏** `Authorization` / `api_key` / `token` 键值」，
T2 §7 也写着「脱敏函数已实现并有单测，但网关/Tauri 日志的全局脱敏随 T3 接入」——
**T3 早已通过，但这个中间件一直没接**：全仓库只有 `vault::redact` 的**单测**在调用它，
生产代码零调用点，也没有 `axum` 中间件层。

也就是说，判据 3 当时能"通过"只是因为**日志里本来就不打请求头**——
不泄漏靠的是"没记"，不是"记了但脱敏"。任何人将来往日志里加一行 header dump 就会直接泄漏。

**本轮补上机制**（补的是 02 §5.2 已经写明的规范，不是新增设计）：

- 新增 `src-tauri/src/gateway/logging.rs`：axum 中间件 `request_log`，
  每个入站请求记一行 `方法 路径 状态码 headers={脱敏后的请求头}`，
  敏感键（`authorization` / `x-api-key` / `api_key` / `token` / `cookie` …）
  的**值**一律经 `vault::redact` 替换为 `***`；请求体不落日志。
- 挂载点：`gateway::server::router()` 的 `.layer(axum::middleware::from_fn(logging::request_log))`。
- 单测 3 条（`authorization` / `x-api-key`+`cookie` / 非敏感头保留）锁住语义。

**实测**（应用实跑，三条鉴权路径各打一次）：

```
[gateway] POST /v1/chat/completions -> 401 Unauthorized headers={"authorization":"***",…}
[gateway] POST /v1/chat/completions -> 401 Unauthorized headers={"authorization":"***",…}
[gateway] POST /v1/chat/completions -> 200 OK        headers={"authorization":"***",…}
```

| 断言 | 结果 |
|---|---|
| 含敏感键的日志行 | 3 行，**全部** `"authorization":"***"` |
| client key 明文是否出现在 stdout/stderr | **否** |
| 渠道密钥明文是否出现在 stdout/stderr | **否** |
| stderr | 长度 0（无异常输出） |
| 401 请求是否打到上游 | **否**（上游计数只 +1，来自那条 200） |

### 2.4 判据 4：绑定策略

**负例（判据本体）**：`LLMR_GATEWAY_ADDR=0.0.0.0:8791` + **空库**（无 client key）→

```
thread 'main' panicked at …tauri-2.12.7\src\app.rs:1444:11:
Failed to setup app: error encountered during setup hook:
拒绝绑定非回环地址 0.0.0.0:8791：改绑必须先存在有效 client key（AGENT.md §5-4），否则本机将变成开放代理
```

| 断言 | 结果 |
|---|---|
| 进程是否退出 | **是**（`HasExited=True`） |
| 端口 8791 是否监听 | **否** |
| 告警是否说清原因与依据 | **是**（指到 AGENT.md §5-4） |

**正例（反向确认策略语义）**：同一个地址 + **有** client key 的库 → 正常启动并监听
（`/healthz` 返回 `{"status":"ok",…}`）。策略是"**有** client key 才允许改绑"，
不是"一律禁止 0.0.0.0"。

## 3. 可复跑的执行器

四条判据固化为 `scripts/security_check.py`（**只读**，不修改库与保险库）：

```powershell
python scripts/security_check.py `
    --store "$env:APPDATA\llm-router\store.db" `
    --vault "$env:APPDATA\llm-router\vault.bin" `
    --log <应用 stdout 日志> --log <应用 stderr 日志> `
    --secret "<渠道 key 明文>" --secret "<client key 明文>" `
    --dump-out <dump.sql> `
    --bind-check "<llm-router.exe>"
```

退出码：`0` = 全过 / `1` = 有明文凭证命中 / `2` = 参数或环境问题。
`--secret` 只用于比对，**不打印、不落盘**（与 02 §3.1 口径一致）。

本轮实测输出（节选）：

```
判据 1 store.db 明文扫描：dump 199 行 + 主库/WAL/SHM 原始字节
判据 2 vault.bin 原始字节 + 十六进制视图扫描
判据 3 日志扫描：含敏感键的行 3 行，其中 3 行已脱敏
判据 4 绑定策略：0.0.0.0 + 无 client key ⇒ 拒绝启动

明文凭证命中：0 条（必须为 0）
结构性命中（DDL 注释 / 字段名 / 枚举值，非明文）：26 条
结论：四条判据全过
```

## 4. 口径细化（对应 T2 §6 的待裁决项）

AGENT.md §9 / 02 §10.2 判据 1 的原始正则 `sk-|api_key|Bearer` **必然命中** DDL 注释与
`credential.kind` 的取值——T2 §6 提过三个备选（收窄为密钥形态 / 只扫数据行 / 注明人工判读），
一直在等裁决。本轮**没有擅自改 AGENT.md**（按 §6-3 走提案流程），而是：

- 在 **02 §10.2** 补了「执行口径（T11）」小节：把"明文凭证命中 = 0"与
  "结构性命中需人工判读"写成**可执行的分类规则**，并指到 `scripts/security_check.py`；
- 实现上等于**T2 §6 提案 1（收窄为密钥形态）+ 提案 3（人工判读需说明）**的合并。

**裁决落地（头头 2026-11-26，选"改为指向 `scripts/security_check.py`"）**：
`AGENT.md` §9 已改写——**保留四条判据的语义**，但**执行统一走只读执行器**，
并写明"明文凭证命中必须为 0 / 结构性命中逐条列出"这条分类口径（不再出现两套并行口径）。
同时清掉了原命令里三处与现实不符的地方：正则恒定命中 `api_key`、
`strings vault.bin`（Unix 工具，Windows 没有）、`grep logs/`（应用不落日志文件，日志走 stdout）。

## 4.1 §9 改写后的执行留档（2026-11-26）

按**新 §9 的命令**实跑两处：

| 对象 | 结果 |
|---|---|
| **用户真实库** `%APPDATA%\llm-router\`（`store.db` 126 976 B / `vault.bin` 596 B） | 判据 1–2 扫描 dump 120 行 + 主库原始字节 → **明文凭证命中 0**，结构性命中 6 条（全是 DDL 注释）；判据 3 无日志文件；**退出码 0**。注：该库当前 `channel`/`credential`/`client_key`/`usage_log` **均为 0 行**（用户还没配渠道），所以"无明文"是**空库意义上的**，有意义的证据见下一行 |
| **已录密钥的验证库**（T12 验证库：1 渠道 + 1 凭据 + 1 client key + 55 条 usage_log，含 WAL） | 判据 1 dump 200 行 + 主库/**WAL/SHM 原始字节** → **明文凭证命中 0**，结构性命中 26 条；判据 2 → 0；判据 3 含敏感键的行 1 行**已脱敏**；判据 4 绑定策略**拒绝启动**；**退出码 0，结论"四条判据全过"** |

## 5. 回归

```
cargo test → 291 项全绿（241 单元 + 7 channel_e2e + 5 gateway_e2e + 6 outbound_e2e
                        + 11 router_e2e + 13 stream_e2e + 5 usage_e2e + 3 vault_e2e）
             0 失败，2 ignored，编译告警 0
```

其中新增 3 条为 `gateway::logging` 的脱敏单测。`npm run build` 未受影响（本轮无前端改动）。

## 6. 产物清单

| 文件 | 状态 |
|---|---|
| `src-tauri/src/gateway/logging.rs` | **新增**——请求日志中间件（02 §5.2 的全局脱敏），含 3 条单测 |
| `src-tauri/src/gateway/server.rs` | 改动——路由表挂 `.layer(from_fn(logging::request_log))` |
| `src-tauri/src/gateway/mod.rs` | 改动——注册 `pub mod logging;` |
| `scripts/security_check.py` | **新增**——02 §10.2 四条判据的可复跑执行器（只读） |
| `docs/reviews/security-check-m1.md` | **新增**（本文件） |
| `docs/planning/02-technical-design.md` | §10.2 补「执行口径（T11）」；§1.2 补 `scripts/security_check.py`；§13 记 v0.10 |
| `docs/planning/03-delivery-plan.md` | T11 任务卡补「落地」行 |
| `HANDOFF.md` | §4 执行入口推进到 T11；§8 账本更新 |

## 7. 未验证 / 范围外

| # | 事项 | 状态 | 说明 |
|---|---|---|---|
| 1 | **macOS Keychain 分支** | 未验证 | 本机是 Windows，DPAPI 路径实测通过；macOS `security-framework` 代码路径未编译验证（T2 §7 沿用） |
| 2 | **AGENT.md §9 正则原文** | ✅ 已裁决并落地 | 头头 2026-11-26 裁决"改为指向 `scripts/security_check.py`"：§9 保留四条判据语义、执行统一走只读执行器，并写明"明文凭证命中必须为 0 / 结构性命中逐条列出"的分类口径；原命令里三处与现实不符之处（正则恒命中 `api_key`、`strings` 是 Unix 工具、`grep logs/` 而应用不落日志文件）一并清掉。执行留档见 §4.1 |
| 3 | 崩溃转储防护 | 部分 | release profile 有 `panic = "abort"`，敏感结构用 `zeroize` 零化；**未做**真实崩溃转储取证 |
| 4 | Tauri 侧（非网关）日志 | 未覆盖 | 本轮中间件只挂在 axum 网关路由上；Tauri 框架自身的日志不经这里（目前它也不打凭证） |

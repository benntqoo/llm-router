# T25 验收留档 — 签到通用化：声明式站点 + 站点目录 + 积分余额

| 项 | 值 |
|---|---|
| 日期 | 2026-11-28 |
| 卡片 | `docs/planning/03-delivery-plan.md` §2「T25 签到通用化」 |
| 设计口径 | 02 **§7.6**（§13 **v0.34**） |
| 提案 | `docs/reviews/proposal-t25-t26.md`（头头 2026-11-28 拍板：声明式 spec + 目录导入导出 + 余额查询） |
| 结论 | ✅ **验收通过**（7 条判据逐条实测全绿；真机渲染确认） |
| 回归 | Rust **517 通过 / 0 失败 / 4 忽略**（lib 421 + e2e 96）；前端 **71/71**；`npm run build` 38 modules / gzip 69.24 kB |

---

## 1. 为什么做这一张卡（问题定义）

头头诉求：「很多 agent harness 都可以有签到领积分 或是平台可以领积分使用」。

现状盘点（**已验证**，逐条带位置）：

| 现状 | 证据 |
|---|---|
| 引擎层**已是通用**的 HTTP 重放（注入 / 断言 / 8 类 `ReplayClass`） | `src-tauri/src/checkin/replay.rs:345`（`evaluate`）、`:469`（`inject`）、`:35`（`ReplayClass`） |
| 模板模型 `SiteTemplate` **本来就通用** | `src-tauri/src/checkin/har.rs:289` |
| 但**内置站点插件只有 2 个** | `src-tauri/src/checkin/plugin.rs:133`（`builtin()`）、`:144`（`builtin_catalog()`） |
| 用户自建**只有 HAR 导入一条路**（抓包 + 逐条标变量 + **无断言编辑 UI**） | `src/checkin/CheckinPanel.tsx`（导入向导）、T16/T17 留档已知限制 |
| **HAR 重放恒拿不到积分** | `src-tauri/src/checkin/replay.rs:141`（注释：HAR 重放恒为 `None`） |
| **没有「当前余额」概念**（只有「本次拿到」快照） | `src-tauri/src/checkin/reward.rs:13` |

**结论**：框架不缺，缺的是**「轻量接入通道」**——新增一家平台的边际成本要从「写 Rust + 发版」降到「填一份 JSON」。
→ **spec → 现有 `SiteTemplate` → 现有重放引擎**，注入 / 断言 / 分类 / 落库**一行不改**。

---

## 2. 做了什么

### 2.1 后端

| 新增/改动 | 内容 |
|---|---|
| `src-tauri/src/checkin/spec.rs`（**新**，约 700 行含单测） | `SpecSite` 模型 + **校验** + **编译成现有 `SiteTemplate`** + **积分/余额路径提取**；`SpecSiteConfig`（复用 `PluginSiteConfig` 的 `credentialId`/`channelId`/`accountId` 字段名，老代码不用改） |
| `src-tauri/src/checkin/catalog.rs`（**新**） | 目录文件模型（`{version, sites[]}`）+ 解析 + 导出 + **内置目录**（只收已实测条目 + 一个空白模板） |
| `src-tauri/src/checkin/replay.rs` | 抽出 `send()` + 新增 `replay_with_body()`（**同时交出响应体**，供提取积分）；**分类与断言语义一字未改** |
| `src-tauri/src/checkin/scheduler.rs` | 分派点加第三支：`plugin == "spec"` → 编译 → 现有引擎 → 取 `reward` |
| `src-tauri/src/commands/checkin_catalog.rs`（**新**） | **5 个 IPC**：`checkin_catalog_list` / `checkin_add_spec_site` / `checkin_import_catalog` / `checkin_export_catalog` / `checkin_balance` |
| `src-tauri/src/commands/checkin.rs` | ① `SiteSummary` 增 `spec` 摘要字段；② **`checkin_set_credential` 修一个真 bug**：原实现用 `PluginSiteConfig::to_json()` 回写，**会把声明式定义整个抹掉** → 改为按插件类型分别回写 |
| `src-tauri/src/lib.rs` | 注册 5 个新 IPC（**IPC 总数 58 → 63**） |
| **schema** | **未改**（复用 `checkin_site` 的 `plugin`/`config_json`/`schedule`/`enabled` 四个字段） |

### 2.2 前端

| 文件 | 内容 |
|---|---|
| `src/checkin/types.ts` | `SpecSite` / `SpecSummary` / `CatalogEntry` / `CatalogImportSummary` / `BalanceSummary` |
| `src/checkin/spec.ts`（**新**） | 纯函数：占位符扫描 / 断言规则解析 / 表单→spec / JSON 解析 / 前置校验 / 摘要 |
| `src/checkin/CatalogPanel.tsx`（**新**） | 「站点目录」区块：列目录 / 用模板 / 新增声明式站点 / 导入 / 导出 / 查余额 / 我的声明式站点 |
| `src/checkin/CheckinPanel.tsx` | 引入并渲染 `<CatalogPanel />` |
| `src/checkin/plugin.ts` | `pluginLabel("spec")` → 「声明式站点（T25）」 |

---

## 3. 判据逐条实测（7 条全绿）

### 判据① 一份 spec 指向本地 mock → 建站 → 执行成功并**拿到积分** ✅

`src-tauri/tests/checkin_spec_e2e.rs` → `spec_site_signs_in_and_extracts_the_reward_everywhere`：

- `checkin_add_spec_site_of(&state, &spec, Some("SECRET-TOKEN"))` → 站点 `plugin = "spec"`、**默认 `enabled = false`**（02 §7.3）；
- 执行 → `outcome.ok == true`、`class == Success`；
- **登录态真的发出去了**：mock 收到 `Authorization: Bearer SECRET-TOKEN`；`{{today}}` 被替换（查询参数带 `day=`）；
- **积分 400 point**：`reward.amount == 400`、`reward.unit == "point"`；
- **三处落点**：`checkin_result.reward_json` 含 `"amount":400`；`account.quota_json` 快照 `points == 400`；
- **响应体里的字符串不进 `reward_json`**（mock 返回体带 `SHOULD-NOT-PERSIST` → 断言**不出现**）。

### 判据② 断言不成立 → 判 `assert_failed`（**不静默当成功**） ✅

`a_failing_json_path_assertion_is_not_silently_a_success`：mock 端点返回 `{"data":{"ok":false}}`，spec 断言 `data.ok == true`
→ `outcome.ok == false`、`class == AssertFailed`、`reward == None`，且结果**落库 `ok = 0`**。

### 判据③ 缺凭据 → 判 `credential_missing` 且**上游请求数不增加** ✅

`a_site_without_a_credential_never_reaches_the_upstream`：**不填登录态**建站（= 从别的机器导进来的目录）→ 执行
→ `class == CredentialMissing`，**mock 命中数 == 0**（上游一个请求都没发）。

> 实现要点：`token` 变量**没有凭据也照样放进模板** → 注入阶段判 `credential_missing`；
> 若不放，注入会变成「未知占位符」的 `template_invalid`，**两者的排查方向完全不同**。

### 判据④ 目录导入：非法条目**带原因不静默丢弃**、默认 `enabled = 0` ✅

`importing_a_catalog_reports_bad_entries_and_is_idempotent`：目录 = 1 条合法 + 1 条 `ftp://` URL
→ `imported.len() == 1`、`errors.len() == 1`，错误里带站点名与原因（`contains("http")`）；导入的站点 `enabled == false`。

### 判据⑤ 导出 → 再导入**站点数一致**（幂等）；导出**不含凭据明文** ✅

- 同上用例：**再导入一次** → 站点总数仍为 **1**（同 spec id 视为更新）；
- `exporting_a_catalog_never_leaks_the_credential`：导出文本含 `credentialId`（id），**不含 `SECRET-TOKEN`**。

### 判据⑥ 零明文 ✅

`the_declaration_never_carries_the_plaintext_credential`：站点 `config_json` 只含 `credentialId`；
**逐字节扫** `store.db` / `store.db-wal` / `store.db-shm` / `vault.bin` → **均无登录态明文**。

### 判据⑦ 余额可查，且与「本次拿到」**严格区分** ✅

`the_balance_is_a_separate_number_from_the_reward`：签到拿到 **400**，查余额得 **1200**（两个不同的数）；
`balance.message` 含「当前余额」；**余额不写进额度池**（额度池仍是「本次拿到」的 400）→ 不与限额自学习字段打架。

---

## 4. 真机验证（2026-11-28）

- **真实应用启动**（`cargo build --bin llm-router` + vite dev server）：日志 `[store] 数据库 … (schema v8)`、`[vault] 保险库 …`、`[gateway] 监听 http://127.0.0.1:8787/v1`，签到调度日志正常（ModelScope / MiniMax Code「跳过（本周期待已过）」）。
- **界面渲染**：签到面板出现「**站点目录（T25 · 声明式站点）**」区块——目录条目表（ModelScope / MiniMax Code = 内置插件，**通用 HTTP 签到（空白模板）** = 声明式模板 + 「用这个模板」按钮）、「新增声明式站点」表单（完整 JSON 框 + id/名称/方法/URL/登录态/断言/积分路径/余额路径）、目录导入导出区。
- 截图：`docs/reviews/app-ui-t25-catalog.png`。

> **环境注记（别再踩）**：
> ① `cargo build` 的 **debug** 构建走 **devUrl**（`tauri.conf.json` → `http://localhost:1420`），**不是** `npm run build` 的产物 → 要看真界面必须**先起 vite（端口 1420，`vite.config.ts` 写死）**；
> ② 本机 `SetCursorPos` 被环境屏蔽 → **UI 自动化点不动按钮**，只能靠「改代码 → vite HMR → 截图」取证（本轮用此法拿到两张真机图）；
> ③ 截图脚本 `scripts/take_screenshot.ps1`（本次新增）：启动应用 → `AttachThreadInput` 抢前台（**不用鼠标**）→ 截全屏。

---

## 5. 真机揪出并修掉的 2 个真 bug（e2e 逮到的，不是猜的）

1. **目录导入绕过了校验**：原实现 `checkin_import_catalog_of` 直接调 `upsert_spec_site`，而校验写在 `checkin_add_spec_site_of` 里
   → **非法条目（`ftp://`）照样建站**。修法：校验下沉到 `upsert_spec_site`，**所有建站路径都过这一道**。
2. **余额模板缺 `token` 变量**：余额请求**默认沿用签到请求的头**（含 `{{token}}`），但占位符扫描只看了余额自己那几个字段
   → 注入阶段报「未知占位符 `{{token}}`」。修法：`balance_placeholders()` 按**实际生效的头**（为空则沿用签到头）扫描。

另修一处**会抹数据的隐患**：`checkin_set_credential` 用 `PluginSiteConfig::to_json()` 回写 `config_json`
→ **声明式站点的 `spec` 字段会被整个抹掉**（目录导入后再填登录态就废了）。已按插件类型分支回写（e2e `setting_the_login_state_keeps_the_declaration_intact` 守住这条）。

---

## 6. 边界（守住没做）

- **不做**验证码绕过 / 指纹伪造 / 签名伪造（AGENT.md §5.7）——撞验证码 = 失败并通知。
- **不做**表达式 / 脚本化 spec：占位符**只有** `{{token}}` 与 `{{today}}`（后端逐字段扫描，别的名字直接拒）。
- **不内置未实测的 endpoint**：内置目录只收 ModelScope / MiniMax Code（**已实测**）+ 一个**空白模板**；「听说的」平台（智谱 / 硅基流动 / 讯飞…）**不进目录**，等实测后再收（进 backlog）。
- **不改重放引擎的分类与断言语义**、**不改 schema**、不引第三方依赖。
- 不做多账号切换、不做「签到后自动兑换/下单」类业务动作（**只签到与查余额**）。

---

## 7. 已知限制（留档）

1. **声明式站点拿不到「已领过」的业务语义**——通用 spec 只能靠状态码 + JSON 路径断言判断成败；
   「今天已领过」需要平台特有的业务字段（MiniMax 的 `status == 3`、ModelScope 的当日流水差值）→ **这类站点仍应走内置插件**。
2. **内置目录只有 2 个已实测条目**——新平台的接入质量取决于用户自己写的 spec；**内置目录要靠「实测后回填」慢慢长**。
3. **余额查询与签到请求共用同一凭据**（spec 只有一个 `{{token}}` 槽）。
4. **前端校验只是提前拦一道**，后端仍会全量校验（前端过了后端拒 → 以后端为准）。
5. 声明式站点**不支持多账号**（一个站点一个凭据槽）。

---

## 8. 证据索引

| 证据 | 位置 |
|---|---|
| 提案与方案对比 | `docs/reviews/proposal-t25-t26.md` |
| 设计口径 | 02 §7.6 / §13 v0.34 |
| e2e（8 条） | `src-tauri/tests/checkin_spec_e2e.rs` |
| 后端单测 | `src-tauri/src/checkin/spec.rs`、`src-tauri/src/checkin/catalog.rs` |
| 前端单测（8 条） | `src/checkin/spec.test.ts` |
| 真机截图 | `docs/reviews/app-ui-t25-catalog.png` |
| 回归数 | Rust 517/0/4；前端 71/71；build 38 modules / gzip 69.24 kB |

# T18 站点插件 ×2 —— 验收留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2「**T18 站点插件 ×2**」
> 口径：`docs/planning/02-technical-design.md` §7.4（+ §13 v0.19 / **v0.20 真机修订**）
> 日期：2026-11-27　·　分支：`dev` → `main`
> 声明：本卡只动「站点插件」这一层（插件机制 / 两个内置站点 / 浏览器辅轨 / 积分回写 / UI 入口），
> **不含**通知（T19）、批量编排 UI、辅轨分发形态。

---

## 1. 结论

**已验证（带证据）**：两个站点插件**都跑通了真实链路**，但**「真签到成功」这一步仍缺头头的登录态**。

| 项 | 状态 | 证据 |
|---|---|---|
| 插件机制（`plugin` 分派 / 8 类共用语义 / 内置带正确断言） | ✅ 已验证 | §4.1 单测 + §4.3 e2e |
| ModelScope 插件打**真实** ModelScope 接口并正确分类 | ✅ 已验证 | §4.2 ① |
| MiniMax Code 插件**走辅轨**打真实站点并正确分类 | ✅ 已验证 | §4.2 ② |
| 浏览器辅轨（Playwright + 系统 Chrome + 持久化 profile）可用 | ✅ 已验证 | §4.2 ⑤⑥ |
| 登录态明文零落盘（库 / WAL / SHM / 保险库四层） | ✅ 已验证 | §4.2 ③ |
| 积分三处落点（返回值 / `reward_json` / `account.quota_json`） | ✅ 已验证（mock 上游） | §4.1 e2e |
| 积分回写对**真实站点** | ❌ **未验证**（无登录态） | §5 |
| 两站**真签到成功** + 魔粒字段路径 + 签到入口 DOM | ❌ **未验证**（无登录态） | §5 |

**一句话**：链路、断言、分类、密文边界、辅轨、积分回写**全部打通并实测**；
剩下的是「拿真实登录态跑一次真签到」——这一步**需要头头提供登录态**（§6 提问）。

---

## 2. 改了什么（范围）

**后端（Rust）**

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/checkin/mod.rs` | 改 | 挂 `plugin` / `reward` / `sidecar` / `sites` 四个模块 |
| `src-tauri/src/checkin/plugin.rs` | **新** | `SitePlugin` trait（`id`/`display_name`/`credential_hint`/`needs_sidecar`/`default_schedule`/`run`/`login`）+ `PluginContext` + `PluginSiteConfig` + `builtin()` / `builtin_catalog()` / `parse_config()` |
| `src-tauri/src/checkin/reward.rs` | **新** | `Reward { amount, unit, raw }` + `sanitize_raw`（**raw 只留数值与布尔**——字符串可能带 csrf/token 一类密文） |
| `src-tauri/src/checkin/sidecar.rs` | **新** | 辅轨 **CLI 协议**客户端：`Sidecar::default_instance()`（env → exe 同级 → 上溯 3 级 → cwd 探测 sidecar 目录）/ `run(action,args,timeout)` / `health()` |
| `src-tauri/src/checkin/sites/mod.rs` | **新** | 内置插件目录 |
| `src-tauri/src/checkin/sites/modelscope.rs` | **新** | ModelScope 插件：`evaluate_signin`（**断言 JSON 业务字段 `Success`**）+ `extract_reward`（候选路径可被站点配置覆盖） |
| `src-tauri/src/checkin/sites/minimax_code.rs` | **新** | MiniMax Code 插件：走辅轨 + `CheckinReport` 契约 + `class_for` 失败码映射 + `login` 交接 |
| `src-tauri/src/checkin/scheduler.rs` | 改 | `run_site` **按 `plugin` 分派**（`har` → 重放引擎；内置 → 插件；未知 → `template_invalid`）；`record` 写 `reward_json`；新增 `write_back_reward`（回写失败**不回滚签到**） |
| `src-tauri/src/checkin/replay.rs` | 改 | `ReplayOutcome` 加 `reward` 字段；`ReplayEngine::client()`（内置插件复用同一套超时口径） |
| `src-tauri/src/store/account.rs` | **新** | 积分回写：`write_reward` / `read_reward`——**合并写**（只改 `points`/`unit`/`source`/`updated_at`，**不动**限额字段） |
| `src-tauri/src/store/mod.rs` | 改 | 挂 `account` 模块 |
| `src-tauri/src/commands/checkin.rs` | 改 | **7 个新 IPC**（见 §3）；`SiteSummary` 扩 4 个字段（`credentialReady` / `needsSidecar` / `credentialHint` / `reward`） |
| `src-tauri/src/lib.rs` | 改 | 注册 7 个新 IPC |
| `src-tauri/Cargo.toml` | 改 | `tokio` 加 `process` feature（辅轨要拉起 Node 进程） |

**辅轨（Node）**

| 文件 | 性质 | 内容 |
|---|---|---|
| `sidecar/index.mjs` | **新** | 正式辅轨：`health` / `login` / `checkin` 三个动作，**stdout 只吐一行 JSON**，用完即退 |
| `sidecar/package.json` | **新** | `playwright@1.58.0` 依赖（`node_modules` 被 gitignore 覆盖） |
| `sidecar/probe-*.mjs` | 新（探测用） | 接口面 / 页面结构探测脚本，**不是**运行时代码的一部分 |

**前端**

| 文件 | 性质 | 内容 |
|---|---|---|
| `src/checkin/plugin.ts` | **新** | 纯函数：登录态三态判定 / 中文说明 / 积分描述 / 插件展示名（`node --test` 可跑） |
| `src/checkin/plugin.test.ts` | **新** | 6 条单测 |
| `src/checkin/types.ts` | 改 | `SiteSummary` 扩 4 字段 + 新增 `RewardSnapshot` / `BuiltinSite` / `SidecarHealth` / `LoginSummary` |
| `src/checkin/CheckinPanel.tsx` | 改 | 新增「**内置站点插件**」区块（清单 / 添加 / 填登录态 / 弹浏览器登录 / 积分显示）+ 调度表加「登录态」列 |

**文档**：02 §7.4（v0.19）+ §7.4 §4 与 §13 v0.19 的**实测订正**（v0.20）、
03 T18 落地行、HANDOFF §4 第 21 步 + §8 账本行、本留档、`.workbuddy/memory/2026-11-27.md`。

---

## 3. IPC 面（7 个新增）

| IPC | 作用 | 密文边界 |
|---|---|---|
| `checkin_builtin_sites` | 内置插件清单（UI「添加站点」） | — |
| `checkin_add_builtin_site` | 建内置站点（**默认不启用**，02 §7.3） | 建站即建**占位渠道 + 默认账号**（见 §2 的归属口径） |
| `checkin_set_credential` | 填登录态（`cookie` 必需 / `token` 可选） | **明文只进保险库**，不回显、不进日志 |
| `checkin_plugin_login` | 登录交接（拉起有头浏览器） | 应用**不读**浏览器 profile 里的凭证 |
| `checkin_sidecar_health` | 辅轨探活 | 只报「有/没有」，不报任何凭据内容 |
| `checkin_site_reward` | 该站点的积分快照 | 只回数量与单位 |

---

## 4. 证据

### 4.1 自动化测试

**Rust 单测 21 条**（T17 的 348 → **369**）：

- `checkin::reward` 3 条（`raw` 只留数值/布尔、**字符串被丢弃**、嵌套递归、JSON 稳定）
- `checkin::plugin` 3 条（**清单默认 cron 与插件默认值必须一致**——见 §4.3 ②；未知插件查不到；脏配置退回默认值）
- `checkin::sidecar` 2 条（截断；profile 目录优先取环境变量）
- `checkin::sites::modelscope` 6 条（**匿名 401 → `credential_invalid`**；**2xx 但 `Success=false` → 断言不符**（不是状态码问题）；无 `Success` 字段不算成功；429/500 分类；非 JSON 响应；积分候选路径顺序与显式路径）
- `checkin::sites::minimax_code` 2 条（失败码 → 8 类映射；辅轨输出契约反序列化）
- `store::account` 5 条（写进 `quota_json`；**合并写不动限额字段**；账号不存在报错；坏 JSON 被替换成合法对象；无快照读回 `None`）

**Rust e2e 9 条**（`src-tauri/tests/checkin_plugin_e2e.rs`，mock 上游跑全链）：

1. `modelscope_plugin_signs_in_and_writes_the_reward_everywhere`——**上游真的收到了保险库里的 Cookie**（断言 `seen_cookie == "csrf_session=SECRET-COOKIE"`）；积分 400 三处落点一致（返回值 / `reward_json` / `account.quota_json`）
2. `modelscope_plugin_reports_an_invalid_credential`——401 → `credential_invalid`，**失败后不再查魔粒**
3. `signin_still_succeeds_when_the_balance_cannot_be_read`——**渐进增强**：查不到魔粒 → **签到仍然成功**，只是没积分
4. `a_missing_credential_is_reported_before_any_request`——没填登录态 → **上游请求数 0**
5. `an_unknown_plugin_is_reported_instead_of_being_skipped`——未知插件 → `template_invalid`（不静默跳过）
6. `the_minimax_code_plugin_fails_cleanly_without_the_sidecar`——辅轨不可用 → **明确失败，不假装跑过了**
7. `builtin_site_creation_wires_channel_account_and_default_schedule`——渠道 / 账号 / 默认 cron / 未启用
8. `the_login_state_never_lands_in_the_site_row`——**站点行里没有明文**，真值在保险库
9. `adding_the_same_builtin_site_twice_reuses_the_same_row`——同名站点复用同一行

**前端单测**：checkin 目录 **29/29**（23 → 29，新增 6 条 `plugin.test.ts`）；全部前端 **44/44**。

**构建**：`npm run build` 通过 —— **32 modules**（+1），`gzip 62.73 kB`（T17 是 31 / 61.45 kB）。

**全量回归**：`cargo test --all-targets` → **444 项全绿**（369 单元 + 75 e2e），**4 ignored**，0 失败，**0 编译告警**。

### 4.2 真机验收（真实应用 + 真实上游）

> 环境：Windows 11 · Node **v22.16.0** · 系统 Chrome（`channel:"chrome"`，**不下载 150MB chromium**）·
> 应用数据 `%APPDATA%\llm-router`（`store.db` / `vault.bin` / `browser-profile`）

**① ModelScope 插件打真实接口**（UI 自动化：添加站点 → 填 Cookie → 保存 → 立即执行）

- 输入的是**故意填错的 Cookie**（`csrf_session=FAKE-T18-PROBE-VALUE`）；
- 应用返回：**「ModelScope（魔搭）」签到失败（credential_invalid）：ModelScope 登录态无效（Cookie 过期或没填）——重新从浏览器抄一次 Cookie　耗时 152 ms**
- → **真打到了 `www.modelscope.cn`（152 ms 往返）**，断言与分类在**真实响应**上正确；
- 落库一行：`checkin_result` id=5 `ok=0` `message=ModelScope 登录态无效（Cookie 过期或没填）——重新从浏览器抄一次 Cookie`。

**② MiniMax Code 插件走辅轨打真实站点**（UI 自动化：添加站点 → 立即执行）

- 应用返回：**「MiniMax Code（网页版）」签到失败（credential_invalid）：没登录——请先点「登录」在弹出的浏览器里登录一次　耗时 2741 ms**
- → **Rust → Node 辅轨 → Playwright → 系统 Chrome → 真实 `agent.minimax.cn`** 全链打通（2741 ms 含浏览器启动）；
- 落库：`checkin_result` id=6 / id=7 `ok=0` `message=没登录——请先点「登录」在弹出的浏览器里登录一次`。

**③ 密文边界（四层扫描，`scripts/t18_scan.py`）**

```
=== 明文扫描（'csrf_session=FAKE-T18-PROBE-VALUE'）===
  命中层数：0  （0 命中 ✓）
```

扫描对象：`store.db`（143360 B）/ `store.db-wal`（49472 B）/ `store.db-shm`（32768 B）/ `vault.bin`（596 B）。

**④ 辅轨探活**：`{"nodeVersion":"v22.16.0","browserAvailable":true,"browserChannel":"chrome","profileDir":"C:\\Users\\Ben\\AppData\\Roaming\\llm-router\\browser-profile","loggedIn":null}`
（**优先系统 Chrome**，不强制下载 chromium；`health` 用**临时 profile** 探活，避免与正在跑的签到抢 profile 锁。）

**⑤ 登录交接（`login` 动作）**：**有头**浏览器被拉起并停在真实站点页面（任务栏出现「MiniMax Code: 常规积分」窗口）——
这是头头后续「在弹出的浏览器里登录一次」这条 UX 的落点。

**⑥ 站点落库与归属**（`checkin_site` / `account` 实查）：

```
site_182212017abbf1808375126911d9c85d  ModelScope（魔搭）    plugin=modelscope    schedule=0 9 * * *   enabled=0
site_be349863212e1e94368062b134468038  MiniMax Code（网页版） plugin=minimax_code  schedule=20 8 * * *  enabled=0
account: acct_default_ch_379eefca7099c2edfc6bb96a4f47c705  channel=ch_379eefca…  quota_json={}
```

配置键：`credentialId` / `tokenCredentialId` / `channelId` / `accountId` / `baseUrl` / `pageUrl` / `rewardPath`。

**截图**：`docs/reviews/app-ui-t18-plugins.png`（插件区 + 调度表 + 登录态 + 执行记录）、
`docs/reviews/app-ui-t18-sidecar-login.png`（辅轨弹出的真实站点浏览器窗口）。

### 4.3 真机揪出并修掉的 4 个 bug

| # | 现象 | 根因 | 修法 | 回归 |
|---|---|---|---|---|
| ① | 辅轨一跑就报 `Cannot access 'looksLoggedOut' before initialization` | 局部 `const looksLoggedOut = await looksLoggedOut(page)` **同名遮蔽**（TDZ） | 局部变量改名 `loggedOut` | 真机复测通过 |
| ② | MiniMax Code 新站的默认 cron 是 `0 9 * * *`（应为 `20 8 * * *`） | 插件**没覆盖 trait 的 `default_schedule`** → 落到 trait 默认值 | 两个插件各自 `fn default_schedule()` 返回本插件常量 | 加**清单 vs 插件一致性**单测 + e2e 断言 |
| ③ | `login` 动作**页面还没渲染完就报「登录完成」**（假阳性） | `DOMContentLoaded` 时 React 还没渲染出按钮 → 「登录按钮数 == 0」被误判成已登录 | 等 `networkidle` + **连续两次读到 0** + 最短停留 5 s | 真机复测：未登录时不再误报成功 |
| ④ | `checkin` 动作在页面未渲染时可能误判登录态 | 同上（1.5 s 固定等待不可靠） | 共用 `waitForRender()` | 真机复测：稳定报 `not_logged_in` |

---

## 5. 未验证项（**需头头登录态**）

| # | 未验证项 | 需要什么 | 解锁后要回填什么 |
|---|---|---|---|
| ① | ModelScope **真签到成功** | 一次 `modelscope.cn` 登录态的 **Cookie 请求头** | 成功响应体形状 + **魔粒字段路径**（当前是候选路径 `data.points`/`data.balance`/`data.magicubes`/`data.total`/`points`/`balance`，站点配置 `reward.path` 可覆盖） |
| ② | MiniMax Code **真签到成功** | 在弹出的浏览器里登录一次（登录态存进 profile） | **签到入口的 DOM 选择器**（当前是文案候选：每日签到/立即签到/去签到/签到/领取奖励/每日奖励/领积分/立即领取/领取）+ `/signin/claim` 响应体形状 |
| ③ | 积分回写对**真实站点** | ①②任一成功即可 | `account.quota_json` 的真实 `points` 值 + `updated_at` |
| ④ | MiniMax Code **连签天数 / 每日 400 积分**（任务卡写的「第 4/7 天各 1000」） | ② | 与任务卡数字对账（**任务卡那句是未实测的口径**） |

> 02 §7.4 §5 的「未验证项」清单与本表一致；②的 DOM 选择器与①的字段路径在实测后**收敛成一条**并回填 02。

---

## 6. 已知限制

1. **辅轨依赖 Node 运行时 + `sidecar/` 的 `npm install`**——「开箱即用」的打包分发形态**不在本卡**（02 §7.4 §5 已写明）。
2. **MiniMax Code 走浏览器** → 关掉应用时靠**系统计划任务**兜底（T17）唤起 `--checkin-once`，此时会**再拉一次浏览器**（约 3~5 s）。
3. **ModelScope 的开放平台 Bearer token 是可选的**——没有它**签到仍然成功**，只是拿不到魔粒（渐进增强，不因查不到积分判失败）。
4. **积分是「本次拿到了多少」的快照**，**不是账户余额真值**（余额由取数方负责）；`quota_json` 里与限额字段**并存**，互不覆盖。
5. **`reward.raw` 只留数值与布尔**——站点响应里若有字符串型积分说明，会被丢掉（防密文落库）。
6. **浏览器 profile 里的凭证由浏览器自己加密保管**（DPAPI/Keychain），**应用不读、不导、不入库、不进 IPC**（AGENT.md §5.3 约束的是我们的存储路径）。
7. **不做任何指纹伪装**（AGENT.md §5.7）——辅轨不加反检测开关、不改 UA、不隐藏自动化特征；**不绕过验证码**（撞到验证码 = 失败并通知）。
8. **站点删除不清理内置站点的凭据**——`checkin_delete_site` 只在 `config_json` 是 HAR 模板时清理凭据；内置站点的凭据留在保险库（**下次建同名站点会复用渠道**）。**已记 backlog**。

---

## 7. 回归与提交

| 项 | 结果 |
|---|---|
| `cargo test --all-targets` | **444 项全绿**（369 单元 + 75 e2e），4 ignored，0 失败，0 编译告警 |
| `npm run build` | 通过（32 modules，gzip 62.73 kB） |
| 前端单测 | checkin **29/29**；全部 **44/44** |
| 编译告警 | `cargo check --all-targets` **0 warning** |

提交号见 HANDOFF §8 账本行。

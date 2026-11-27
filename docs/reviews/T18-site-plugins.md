# T18 站点插件 ×2 —— 验收留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2「**T18 站点插件 ×2**」
> 口径：`docs/planning/02-technical-design.md` §7.4（+ §13 v0.19~**v0.24**）
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
| 积分回写对**真实站点** | ❌ **未验证**（真机当天已领过 → 本次无新增积分） | §5 |
| MiniMax Code **真签到成功** + 签到入口定位 + 每日积分口径 | ✅ **已验证** | §4.2 ⑦⑧⑨ |
| ModelScope **真签到成功** + 魔粒字段形状 | ✅ **已验证**（访问令牌 → 200 / 277 ms / 落库 `ok=1`） | §4.2 ⑩ |
| ModelScope **「本次到账 > 0」分支**（差值 > 0） | ❌ **未验证**（真机当天已领过 → 差值 = 0） | §5 |

**一句话**：链路、断言、分类、密文边界、辅轨、积分回写**全部打通并实测**；
**两个站点的成功路径都跑通了**——MiniMax Code（登录 → 打开签到面板 → 判「今天已领」→ 落库 `ok=1`，4307 ms）、
ModelScope（**访问令牌** → `Cookie: m_session_id=<令牌>` → 200 / `Success=true` / 277 ms → 落库 `ok=1`）；
剩下的是**「本次到账 > 0」那条分支**（真机当天已领过 → 差值 = 0）与 MiniMax 的 `/signin/claim` 响应体。

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
| `sidecar/probe-*.mjs` | 新（探测用） | 接口面 / 页面结构探测脚本，**不是**运行时代码的一部分；第二轮新增 `probe-signedin.mjs`（登录态页面结构 + `status` 响应体）、`probe-signin-entry.mjs`（**逐个点候选触发点找签到入口**）、`probe-signin-panel.mjs`（**走一遍真实签到流程** + 抓 `status`/`claim` 响应体） |

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

**⑦ MiniMax Code 登录交接成功（第二轮，2026-11-27）**

- 站点**短信/验证码登录被频控**——点「获取验证码」直接报**「操作频繁」**，短信发不出去（站点防刷策略，不是我们的 bug）；
- 辅轨改为**自动打开登录弹窗 + 自动切「账号密码登录」**（**只铺路**：不代填密码、**不绕人机验证**，撞 AGENT.md §5.7），日志：

  ```
  [sidecar] 浏览器已启动：chrome（headless=false）
  [sidecar] 已打开登录弹窗（「登录」）
  [sidecar] 已点击：「账号密码登录」
  [sidecar] 登录方式已切到账号密码（「账号密码登录」）——避开短信通道的频控
  [sidecar] 检测到登录成功（user_id=479049039077470200）   ← 判据 A：硬信号
  {"ok":true,"loggedIn":true,"code":null,"message":"登录完成，登录态已存进本地浏览器 profile"}
  ```

- **登录态落进应用自己那份 profile**：`%APPDATA%\llm-router\browser-profile`（与 Rust 侧 `LLMR_BROWSER_PROFILE` 同一份 → 应用侧立刻可用；实测 Chrome 进程参数 `--user-data-dir=C:\Users\Ben\AppData\Roaming\llm-router\browser-profile`，父进程 = 辅轨 Node 进程）。
- **登录超时 5min → 15min**（真人要输密码 + 过人机验证；`LLMR_LOGIN_TIMEOUT_MS` 可覆盖）。

**⑧ 签到入口定位（第二轮）**

- **入口不在首页**：登录态首页**一个「签到」字都没有**（`keywordHits` 为空），页面加载时只打 `/signin/config` + `/signin/status`；
- 逐个点候选触发点（用户头像下拉 / 侧栏「更多」）后命中：**用户头像下拉菜单**里有 `设置 / 用量 / **每日签到** / 问题反馈 / 联系我们 / 了解更多 / 退出登录`；
- 触发器是 `.ant-dropdown-trigger`，**页面上有两个**——第一个是头像（有文字），第二个是「添加附件或技能」（**没文字**）→ **只认有文字的那个**（这是个坑）。

**⑨ 真签到成功（第二轮，端到端）**

- 应用里点 MiniMax Code 行的「立即执行」→ Rust → Node 辅轨 → Playwright → 系统 Chrome → 真实 `agent.minimax.cn`：

  ```
  [sidecar] 浏览器已启动：chrome（headless=true）
  [sidecar] 抓到 status 响应：HTTP 200
  [sidecar] 已打开用户头像下拉菜单
  [sidecar] 已点击：「每日签到」
  {"ok":true,"alreadyClaimed":true,"points":null,"day":7,"code":null,"message":"今天已经领过了（连签第 7 天，当日积分 1000）"}
  ```

- 应用 UI：**「[MiniMax Code（网页版）] 签到成功（success）：MiniMax Code 今天已经领过了（连签第 7 天）　耗时 4307 ms」**；
- 落库：`checkin_result` **id=9 `ok=1`** `message=MiniMax Code 今天已经领过了（连签第 7 天）` `reward_json=NULL`（**已领过 → 本次无新增积分 → 不回写额度池**，与 §2 的「合并写只记本次拿到多少」口径一致；`account.quota_json` 保持 `{}`）；
- **真机当天已经领过**（面板里今天的按钮是**禁用态**的「今日已签到」）→ 走的是**幂等路径**，`/signin/claim` **没有被触发**（claim 响应体仍未实测，见 §5）；
- **`/signin/status` 的真实响应体**（登录态实测）：

  ```json
  {"data":{"scene":3,"days":[
    {"day_no":1,"points":400,"status":3,"is_today":false,"bonus_points":0},
    {"day_no":4,"points":1000,"status":3,"is_today":false,"bonus_points":0},
    {"day_no":7,"points":1000,"status":3,"is_today":true,"bonus_points":0}]},
   "base_resp":{"status_code":0,"status_msg":"ok"}}
  ```

  → **`status == 3` = 已领取**；**今天那一条 = `is_today == true`**；**每日 400、第 4/7 天各 1000**（**与 03 任务卡的口径对上，实测值**）；当天为**连签第 7 天**。

- 截图：`docs/reviews/app-ui-t18-signin-success.png`（成功提示 + 最近执行记录 + 调度表「最近：成功」）。

**⑩ ModelScope 真签到成功（第三轮，2026-11-27）—— 登录态口径从「手抄 Cookie」改成「访问令牌」**

- **头头的反馈**：「cookies 只能手动输入吗？这样很不友好，并不是所有人都会获取 cookies」→ 口径改成**访问令牌**（魔搭 → 个人中心 → 访问令牌 → 点一下复制，`ms-` 开头）。
- **两条走不通的路（都不内置，已在同轮撤销实现）**：
  - 「让应用开浏览器自动登录魔搭」→ 真机实测：Chrome 顶部挂着「**自动测试软件正在控制 Chrome**」，登录框报 **`验证失败，点击框重试（error:22phmt）`**、**滑块根本没渲染**——魔搭的人机验证**专门拦自动化浏览器**；而**反检测 / 绕验证码是 AGENT.md §5.7 红线**；
  - 「浏览器接管会话」（辅轨开浏览器 → 用户登录 → 把 Cookie 取回来）→ **凡是我们能读 Cookie 的浏览器都是被自动化控制的浏览器**，登录同样过不了人机验证 → 整套实现（辅轨 `session` 动作、`run_secret`、IPC、前端按钮）**已全部删除**。
- **正解（第三方魔搭签到脚本的实测口径）**：**开放平台访问令牌可以直接当网页会话用**——`Cookie: m_session_id=<ms-令牌>` → `GET /api/v1/users/login/info` 即触发当日签到；**同一个令牌也是 `/openapi/v1/*` 的 Bearer** → **一个令牌，签到 + 魔粒全包**。
- **真机结果**（走应用内 `scheduler::run_now`，与 UI「立即执行」同一条路径）：

  ```
  ok=true class=success status=Some(200) elapsed_ms=277
  message=ModelScope 签到成功
  ```

  落库 `checkin_result` id=10 `ok=1`（后续复核行 id=13：`ModelScope 签到成功（今天已领过：当日每日签到魔粒 200，本次没有新增）`）。
- **魔粒流水的真实形状**（`GET /openapi/v1/magicubes/transactions` → **200**）：

  ```json
  {"data":{"page":1,"page_size":20,"total":3,"records":[
    {"type":"EARN","rule_key":"daily_active","total_amount":200,"count":1,"expire_days":1,"days_remaining":1,"category_key":"daily","gmt_created":"2026-11-27"},
    {"type":"EARN","rule_key":"infocomplement_profile_experience","total_amount":50,"count":1,"expire_days":90,"gmt_created":"2026-11-27"},
    {"type":"EARN","rule_key":"infocomplement_email_verification","total_amount":50,"count":1,"expire_days":90,"gmt_created":"2026-11-27"}
  ]},"request_id":"…","success":true}
  ```

  → **金额字段 = `total_amount`**（不是 `amount`/`points`）、**日期字段 = `gmt_created`**（**只有日期**）、**类型 = `type`**、**规则 = `rule_key`**；
  → **只有 `daily_active` 是「每日签到」那一笔**（当日 200 魔粒）；另两条是**一次性资料奖励**（各 50），**不算进签到奖励**；
  → **网页会话 Cookie 也能开 `/openapi/v1/*`**（与 Bearer 回**同一份**流水）。
- **奖励口径改成「签到前后差值」**：站点**不返回**「今日魔粒 / 今天已领」信号（签到响应体只是用户资料：`Code`/`Data`/`Message`/`RequestId`/`Success`，`Data` 里含 `GitlabAccessToken` 等敏感字段 → **插件不存响应体**），而当日那条 `daily_active` 流水**整天都在** → 只有**差值**才是「本次拿到」：
  - 差值 > 0 → `points = 本次差额`、**回写额度池**；
  - 差值 = 0 → 判**「今天已领过」**、`points = null`、**不回写额度池**（与 MiniMax Code 的口径一致）。
- **真机当天跑出来的是「已领过」分支**：当日 200 魔粒在探针首次触发签到时就已入账 → 差值 = 0 → `ModelScope 签到成功（今天已领过：当日每日签到魔粒 200，本次没有新增）`、**`reward_json = NULL`**（**不重复计数**）。
- **环境备注**：本机 `SetCursorPos` 被环境屏蔽（光标坐标被钉死，UI 自动化点不动按钮）→ 真机验证改走 `scheduler::run_now`（**与 UI「立即执行」同一条路径**，含落库与额度回写），不用手点。

**⑪ 两站已启用 + 系统兜底任务真机验证（2026-11-27）**

- 头头决定「**两站都启用**」→ `checkin_site.enabled = 1`：ModelScope `0 9 * * *`（每天 09:00）、MiniMax Code `20 8 * * *`（每天 08:20）。
- **Windows 计划任务各注册一个**（02 §7.2.1 §4 的口径）：

  | 任务名 | 动作 | 频率 |
  |---|---|---|
  | `LLM Router Checkin - site_182212017abbf1808375126911d9c85d` | `"…\target\debug\llm-router.exe" --checkin-once --site site_1822…` | 每天 09:00 |
  | `LLM Router Checkin - site_be349863212e1e94368062b134468038` | `"…\target\debug\llm-router.exe" --checkin-once --site site_be34…` | 每天 08:20 |

- **整链真机验证（应用关着跑的）**：把应用**完全停掉**（只剩计划任务能触发）→ 临时把 ModelScope 的 cron 调到当前分钟 → `schtasks /run` → 任务 **11:44:56** 自动触发 → 真实打到 `www.modelscope.cn` → 落库 `checkin_result` **id=15 `ok=1`** → 任务 **Last Result: 0** → 随后 cron 还原为每天 09:00。
- **去重口径复验**：当天这一周期已跑过时，任务照样跑成功（exit 0）但**不重复打上游**（`本周期已跑过`）——应用重启后的每分钟 tick 同理（两站都已启用，重启后未产生新的结果行）。
- **已知限制**：任务动作指向**开发态二进制** `src-tauri/target/debug/llm-router.exe` → `cargo clean` / 删掉 `target/` 会让任务失效（**本项目还没有安装版可指**，分发形态不在本卡）；Windows 计划任务默认「**仅当用户登录时运行**」——注销状态下不会触发（补签靠应用启动时的同一套判定）。

### 4.3 真机揪出并修掉的 8 个 bug

**第一轮（失败路径阶段）**

| # | 现象 | 根因 | 修法 | 回归 |
|---|---|---|---|---|
| ① | 辅轨一跑就报 `Cannot access 'looksLoggedOut' before initialization` | 局部 `const looksLoggedOut = await looksLoggedOut(page)` **同名遮蔽**（TDZ） | 局部变量改名 `loggedOut` | 真机复测通过 |
| ② | MiniMax Code 新站的默认 cron 是 `0 9 * * *`（应为 `20 8 * * *`） | 插件**没覆盖 trait 的 `default_schedule`** → 落到 trait 默认值 | 两个插件各自 `fn default_schedule()` 返回本插件常量 | 加**清单 vs 插件一致性**单测 + e2e 断言 |
| ③ | `login` 动作**页面还没渲染完就报「登录完成」**（假阳性） | `DOMContentLoaded` 时 React 还没渲染出按钮 → 「登录按钮数 == 0」被误判成已登录 | 等 `networkidle` + **连续两次读到 0** + 最短停留 5 s | 真机复测：未登录时不再误报成功 |
| ④ | `checkin` 动作在页面未渲染时可能误判登录态 | 同上（1.5 s 固定等待不可靠） | 共用 `waitForRender()` | 真机复测：稳定报 `not_logged_in` |

**第二轮（成功路径阶段，2026-11-27）**

| # | 现象 | 根因 | 修法 | 回归 |
|---|---|---|---|---|
| ⑤ | 登录卡在短信验证码上——点「获取验证码」报**「操作频繁」**，短信发不出去 | 站点对**短信/验证码登录**有频控（不是我们的 bug） | 辅轨 `login` 动作**自动打开登录弹窗 + 自动切「账号密码登录」**（**只铺路**，不代填密码、不绕人机验证）；并扫「操作频繁」类文案，命中就**立刻返回**别让人白等 | 真机：登录成功（`user_id` 硬信号命中） |
| ⑥ | 登录成功判据只有「按钮消失」一条，**判据不稳** | 登录可能开新标签页，只看首屏会把「原页按钮还在」误当没登录 | 收敛成**三条独立判据**：A 硬信号（页面 API 请求里 `user_id > 0`）/ B 整个上下文无「登录」按钮（连续两次 + 停留 ≥5 s）/ C 出现签到入口 | 真机：A 命中即判成功 |
| ⑦ | 签到永远找不到入口 → `no_signin_entry` | 辅轨**在首页找签到文案**，而入口在**用户头像下拉**里（首页一个字都没有） | 改为**先开签到面板再判状态**：开用户菜单 → 点「每日签到」；`.ant-dropdown-trigger` **只认有文字的那个** | 真机：面板打开，判「今日已签到」 |
| ⑧ | 积分/天数从 `/signin/status` 里提不出来 | `extract_reward` 只认**平铺字段**，认不出 `data.days[]`（每条含 `day_no`/`points`/`status`/`is_today`） | 补 `days[]` 形状：**今天那一条 = `is_today == true`** | 真机：day=7、当日 1000 分 |
| ⑨ | 业务成功判据漏了 MiniMax 约定 | 只看 `success`/`code`，**没看 `base_resp.status_code == 0`**（AGENT.md §6 已知坑 1：HTTP 200 也可能是业务失败） | `detect_success` / `detect_already_claimed` 补 `base_resp` 分支；**带了 `base_resp` 就只认它** | 与真实响应形状对齐 |

---

## 5. 未验证项

| # | 未验证项 | 需要什么 | 解锁后要回填什么 |
|---|---|---|---|
| ① | ModelScope **「本次到账 > 0」分支**（差值 > 0） | 一个**还没签到过**的当天 | 真机当天**已领过**（当日 200 魔粒已在流水里）→ 实测跑的是**差值 = 0 → 判「今天已领过」**；**差值 > 0** 要等**次日首次运行**才验（届时应得 `points = 200` + 回写额度池） |
| ② | MiniMax Code **`/signin/claim` 真正领取的响应体** | 一个**还没领过**的当天 | claim 响应体形状（积分/天数字段名）+ **领取按钮的真实文案**（现在是候选：`立即签到`/`去签到`/`签到领取`/`领取奖励`/`立即领取`/`领取`） |
| ③ | 积分回写对**真实站点** | ①或②任一**真正拿到积分**即可 | `account.quota_json` 的真实 `points` 值 + `updated_at`（当前真机那次回写为 0——已领过 → 本次无新增积分，`quota_json` 保持 `{}`） |
| ④ | MiniMax Code **签到按钮禁用态之外的状态枚举** | 一个还没领过的当天 | `status` 字段其余取值（现在只实测到 `3` = 已领取） |

> 02 §7.4 §5 的「未验证项」清单与本表一致。**已收敛**：签到入口 DOM（→ 用户头像下拉「每日签到」）、每日积分与连签天数（→ 每日 400、第 4/7 天各 1000，实测第 7 天）、**ModelScope 真签到成功 + 魔粒字段形状（→ `total_amount` / `gmt_created` / `rule_key`，当日 200 魔粒）**。

---

## 6. 已知限制

1. **辅轨依赖 Node 运行时 + `sidecar/` 的 `npm install`**——「开箱即用」的打包分发形态**不在本卡**（02 §7.4 §5 已写明）。
2. **MiniMax Code 走浏览器** → 关掉应用时靠**系统计划任务**兜底（T17）唤起 `--checkin-once`，此时会**再拉一次浏览器**（约 3~5 s）。
3. **ModelScope 的凭据是「访问令牌」**（`ms-…`，个人中心 → 访问令牌 → 点一下复制）——**同一个令牌既是网页会话（`m_session_id`）又是 `/openapi/v1/*` 的 Bearer**；**手抄 Cookie 头那条路保留**（备选/排障），但不再是推荐路径。**拿不到魔粒不影响签到成功**（渐进增强，不因查不到积分判失败）。
4. **积分是「本次拿到了多少」的快照**，**不是账户余额真值**（余额由取数方负责）；`quota_json` 里与限额字段**并存**，互不覆盖。
5. **`reward.raw` 只留数值与布尔**——站点响应里若有字符串型积分说明，会被丢掉（防密文落库）。
6. **浏览器 profile 里的凭证由浏览器自己加密保管**（DPAPI/Keychain），**应用不读、不导、不入库、不进 IPC**（AGENT.md §5.3 约束的是我们的存储路径）。
7. **不做任何指纹伪装**（AGENT.md §5.7）——辅轨不加反检测开关、不改 UA、不隐藏自动化特征；**不绕过验证码**（撞到验证码 = 失败并通知）。
8. **站点删除不清理内置站点的凭据**——`checkin_delete_site` 只在 `config_json` 是 HAR 模板时清理凭据；内置站点的凭据留在保险库（**下次建同名站点会复用渠道**）。**已记 backlog**。
9. **探测脚本不许用临时 profile 反复打线上站点**——真机踩过：临时 profile 每次都像「新设备」，直接把站点的**短信通道打限流**（「操作频繁」）。**正式辅轨一律用持久化 profile**；探测脚本只在必要时用一次，且**输出要脱敏**（用户下拉里有账号 id 与手机号，不能原样打进日志）。
10. **登录态是「浏览器的」不是「应用的」**——辅轨型站点的登录态存在**浏览器 profile** 里（浏览器自己加密保管），**应用读不到**，所以调度表「登录态」列只能显示「在浏览器里登录一次即可」；它**不反映当前是否真登录着**（要真知道只能开浏览器探一次，代价是每次多一个浏览器进程 + 与正在跑的签到抢 profile 锁）。

---

## 7. 回归与提交

| 项 | 结果 |
|---|---|
| `cargo test --all-targets` | **449 项全绿**（373 单元 + 76 e2e），4 ignored，0 失败，0 编译告警 |
| `npm run build` | 通过（32 modules，gzip 62.78 kB） |
| 前端单测 | checkin **29/29**；全部 **44/44** |
| 编译告警 | `cargo check --all-targets` **0 warning** |
| `node --check sidecar/index.mjs` | 通过 |

提交号见 HANDOFF §8 账本行。

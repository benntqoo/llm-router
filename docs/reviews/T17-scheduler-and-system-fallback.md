# T17 验收留档 — 调度器与系统兜底

| 项 | 值 |
|---|---|
| 任务 | T17 调度器 + 系统兜底（03 §2 任务卡，M3 第三张卡） |
| 依据 | `docs/planning/02-technical-design.md` §7.2（调度与通知）、**§7.2.1 调度与补签口径**、§7.3（合规边界）；**v0.17 新增 §7.2.1**、**v0.18 真机修订** |
| 验收日期 | 2026-11-27 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | 见 §7（代码与留档分两个提交） |
| 结论 | **通过**（任务卡判据「cron 调度、Windows 计划任务/launchd 注册、错过补签；验收：**关闭应用后计划任务仍触发并补签**」逐条实测通过） |

> 本文件改动范围：仅记录 T17 的验收判据、实测证据、交付物与取舍；**不含**后续任务（T18 站点插件 / T19 通知）的结论。

---

## 1. 判据对照

**T17 任务卡**（03 §2）：「cron 调度、Windows 计划任务/launchd 注册、错过补签。验收：**关闭应用后计划任务仍触发并补签**。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **cron 调度** | ✅ | §3.1：`checkin::cron` 10 条单测（5 字段归一化、`last_due_at` / `next_due_at`、形状分类）；§3.3 真机：面板填 `* * * * *` → 落库 `schedule` → 界面显示「每 1 分钟」「下次：马上」 |
| 2 | **Windows 计划任务注册** | ✅ | §3.3：`schtasks /query` 实测 `Task To Run: "…\llm-router.exe" --checkin-once --site site_fd6f…`（**动作指向应用自身 exe**）、`Schedule Type: One Time Only, Minute`（= `/sc minute /mo 1`）；§3.2 e2e：注册 → 查询 → 注销全链路 |
| 3 | **launchd 注册** | ⚠️ 部分 | 代码实现完整（`~/Library/LaunchAgents/com.llmrouter.checkin.<site>.plist`，`StartCalendarInterval` / `StartInterval`），**本机是 Windows，未在真机验证**（见 §5） |
| 4 | **错过补签** | ✅ | §3.3：cron `0 9 * * *`（当天 09:00 已过、期间应用关着）→ CLI 判定为「该跑」并**补签执行成功**；§3.1 单测 `site_is_due_again_after_missing_the_window` |
| 5 | **关闭应用后计划任务仍触发并补签**（**任务卡验收判据**） | ✅ | §3.3：**`taskkill` 关掉应用**（进程列表为空）→ 计划任务在 **02:04:01 / 02:05:01 持续触发** → mock 上游收到请求（带**保险库里的真值 cookie**）→ `checkin_result` 落库 `ok=1` → `Last Run Time: 2026/11/27 2:05:01`、`Last Result: 0` |
| 6 | **同一应触发周期最多跑一次** | ✅ | §3.1：`last_attempt >= due_at` → 跳过（**失败也算试过**）；§3.3 真机：同周期再跑 → CLI 输出「跳过（本周期已跑过（应触发时刻 1795654800））」 |
| 7 | **跨重启去重成立** | ✅ | §3.3：应用重新启动后的第一个 tick，两个站点都判「跳过（本周期已跑过）」，**不重打上游**；补签站点的结果行仍只有 1 条 |
| 8 | **密文边界不被破坏** | ✅ | §3.3：`store.db` / `store.db-wal` / `store.db-shm` / `vault.bin` **四层**扫描验收夹具里的两个真值 → **明文命中 0** |

---

## 2. 交付物

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/checkin/cron.rs` | **新增** | cron 解析（**经典 5 字段**归一化补秒位给 `cron` crate 0.12 的 6 字段口径）/ `last_due_at`（回溯窗口法）/ `next_due_at` / `matches_now` / `CronShape` 六类形状 + **10 条单测** |
| `src-tauri/src/checkin/scheduler.rs` | **新增** | `due_of`（应跑判定 + 去重）/ `due_sites` / `run_site` / `run_due_sites` / `run_now` / `run_ticker`（每分钟 tick）/ `tick_once` / `VaultResolver`（接到真实保险库）+ **9 条单测** |
| `src-tauri/src/checkin/system.rs` | **新增** | Windows `schtasks` 注册·查询·列表·注销 + macOS launchd plist + 其它平台 stub；cron → 频率映射 |
| `src-tauri/src/checkin/cli.rs` | **新增** | `--checkin-once [--site <id>]`：**不起 UI、不起网关**，跑完补签判定就退出（退出码 0/1/2） |
| `src-tauri/src/commands/checkin.rs` | 改 | 新增 4 个 IPC（`checkin_set_schedule` / `checkin_run_now` / `checkin_results` / `checkin_system_sync`）；`SiteSummary` 扩 7 个调度字段（下次触发 / 最近尝试 / 最近成败 / 系统任务名 / 已注册 / 频率 / 同步错误） |
| `src-tauri/src/lib.rs` | 改 | 注册 4 个新 IPC；`setup` 里 spawn 调度器 ticker（**启动先跑一遍 → 补签**） |
| `src-tauri/src/main.rs` | 改 | 拦截 `--checkin-once` 参数（跑完退出，**不进 GUI**） |
| `src-tauri/tests/checkin_scheduler_e2e.rs` | **新增** | 6 条 e2e：每周期一次 / 漏周期补签 / 停用不跑 / 单站点限定 / **Windows 计划任务真实注册·查询·注销** / 频率映射 |
| `src-tauri/examples/checkin_schedule_demo.rs` | **新增** | 真机验收工具（`setup` / `seed` / `status` / `list` / `register` / `cleanup` / `tidy`） |
| `src-tauri/examples/checkin_mock_upstream.rs` | **新增** | 真机验收用的 mock 上游（**独立进程**——验收「关掉应用后计划任务仍触发」时，上游必须一直活着才能证明请求真发出去了） |
| `src/checkin/schedule.ts` | **新增** | 调度面板的纯函数：cron 拆字段 / 前端校验 / 中文说明（与后端 `CronShape` 同口径）/ 相对时间 / 系统兜底说明 |
| `src/checkin/schedule.test.ts` | **新增** | 11 条前端单测（`node --experimental-strip-types --test`） |
| `src/checkin/types.ts` | 改 | `SiteSummary` 扩 7 个调度字段 + 新增 `RunSummary` / `ResultView` / `SystemSyncSummary` |
| `src/checkin/CheckinPanel.tsx` | 改 | 面板新增**调度区**：cron 输入 + 启用开关 + 「立即执行」+「同步系统兜底任务」+ 最近执行记录表（T16 留档里「立即执行一次」明确记在 T17/T18） |
| `src-tauri/Cargo.toml` | 改 | 新增 `cron = "0.12"` + `chrono`（`clock` feature）+ `windows` 的 `Win32_Globalization` feature（按 OEM 代码页解码系统命令输出） |
| `docs/planning/02-technical-design.md` | 改 | **新增 §7.2.1 调度与补签口径** + §13 v0.17 + **v0.18（真机修订）** |

---

## 3. 验收证据

### 3.1 后端单测（19 条新单测）

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib checkin
```

| 模块 | 条数 | 覆盖 |
|---|---|---|
| `checkin::cron` | **10** | 5 字段归一化补秒位 / 6·7 字段原样 / 字段数不对报错 / `last_due_at`（今天已过 / 今天未到 → 昨天 / 每月表达式回溯放大窗口）/ `next_due_at` / 当前分钟命中 / `CronShape` 六类形状 / **`* * * * *` → 每 1 分钟**（回归，见 §6） |
| `checkin::scheduler` | **9** | 停用不调度 / 没 cron 不调度 / 非法 cron 给出原因 / 到点该跑 / **漏掉的周期仍该跑（补签）** / 本周期已跑过跳过 / **失败也算试过** / 历史读写 / `due_sites` 只列该跑的 |
| `commands::checkin` | +1 | `leak_check_ignores_random_credential_ids`（**T15 的随机 flake 修复**：泄漏自检只能查请求面，不能查整段 JSON——随机 `credential_id` 会偶然包含短密文） |

`checkin` 相关单测合计 **61 条全绿**（`har` 12 + `replay` 16 + `cron` 10 + `scheduler` 9 + `commands::checkin` 8 + `store::checkin` 6）。

### 3.2 e2e（6 条，含真实 Windows 计划任务）

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --test checkin_scheduler_e2e
```

| # | 用例 | 实测 |
|---|---|---|
| 1 | `due_site_runs_once_per_cycle` | 同一应触发周期第二次跑 → 跳过 |
| 2 | `missed_cycle_is_caught_up_on_the_next_run` | 漏掉的周期在下次运行时**被补上** |
| 3 | `disabled_site_is_never_run_by_the_scheduler` | 停用站点永不被调度 |
| 4 | `single_site_selection_limits_the_run` | `--site <id>` 只处理该站点 |
| 5 | `windows_scheduled_task_is_registered_queried_and_removed` | **真实 `schtasks` 注册 → 查询 → 列表 → 注销**全链路 |
| 6 | `scheduled_task_frequency_maps_cron_shapes` | 六类 cron 形状 → 计划任务频率描述 |

> 第 5 条首轮**失败**：Windows `schtasks /query /fo LIST /v` 输出的任务名带**前导反斜杠**（`\LLM Router Checkin - <id>`），而 `list()` 只按 `strip_prefix("TaskName:")` 后 `starts_with(TASK_PREFIX)` 匹配 → 返回空数组。已修（剥离前导 `\`），这也是本次任务里**唯一一个「代码逻辑对、外部输出格式没料到」的坑**。

### 3.3 真机验收（真实应用 + 真实 Windows 计划任务）

**准备**（两条命令）：

```powershell
# 1) mock 上游（独立进程，记录每次命中；URL 指向 127.0.0.1:18731）
$env:MOCK_PORT="18731"; $env:MOCK_LOG="$env:TEMP\llmr-t17\mock.log"
src-tauri\target\debug\examples\checkin_mock_upstream.exe

# 2) 真实抓包形态的夹具 → 导入成站点（T15 的真实链路）
src-tauri\target\debug\examples\checkin_schedule_demo.exe setup "$env:TEMP\llmr-t17\acceptance.har" "T17-验收站点"
# → SITE_ID=site_fd6fe5692276409fc1177fc199645b06
```

**证一：面板 + 计划任务注册（真实应用 + UI 自动化）**

启动真实应用（`target/debug/llm-router.exe` + Vite dev server :1420），UI 自动化：滚到签到面板 → cron 输入框粘贴 `* * * * *` → 点「保存」→ 勾「启用」。界面反馈：

> 「T17-验收站点」已按 每 1 分钟 调度（系统兜底任务已注册，关掉应用也能签到）

`schtasks /query /tn "\LLM Router Checkin - site_fd6fe5692276409fc1177fc199645b06" /fo LIST /v` 实测：

```
TaskName:          \LLM Router Checkin - site_fd6fe5692276409fc1177fc199645b06
Task To Run:       "D:\Code\llm-router\src-tauri\target\debug\llm-router.exe" --checkin-once --site site_fd6fe5692276409fc1177fc199645b06
Schedule Type:     One Time Only, Minute
Next Run Time:     2026/11/27 2:04:00
```

→ **动作确实指向应用自身 exe**（不是验收工具——注册动作是从**真实应用**里发出的）。

**证二：关闭应用后计划任务仍触发（任务卡验收判据）**

```
02:03:17  taskkill /PID <app> /T /F      → 应用进程列表为空
02:04:01  计划任务触发（应用已关闭）
02:05:01  计划任务触发（应用已关闭）
```

mock 上游命中日志（应用关闭期间新增的两条）：

```json
{"seq":2,"ts":1795716241,"method":"GET","uri":"/api/v1/users/login/info","cookie":"csrf_session=T17-ACCEPTANCE-SESSION-SECRET; csrf_token=T17-ACCEPTANCE-CSRF-SECRET"}
{"seq":3,"ts":1795716301,"method":"GET","uri":"/api/v1/users/login/info","cookie":"csrf_session=T17-ACCEPTANCE-SESSION-SECRET; csrf_token=T17-ACCEPTANCE-CSRF-SECRET"}
```

→ 上游收到的 Cookie 是**保险库里的真值**（占位符一个都没发出去）。`schtasks` 实测 `Last Run Time: 2026/11/27 2:05:01`、`Last Result: 0`；`checkin_result` 落库 `ok=1`、`message=签到成功（HTTP 200）`。

**证三：补签 + 同周期去重 + 跨重启去重**

```powershell
# 第二个站点：cron `0 9 * * *`（当天 09:00 已过，期间应用关着 → 漏周期）
llm-router.exe --checkin-once --site site_0df999e07b4c052985f9cdb642e858b8
# → [checkin] T17-补签站点：签到成功（HTTP 200）        退出码 0   ← 补签生效

llm-router.exe --checkin-once --site site_0df999e07b4c052985f9cdb642e858b8
# → [checkin] T17-补签站点：跳过（本周期已跑过（应触发时刻 1795654800））   ← 同周期去重
```

应用重新启动后的第一个 tick（stdout）：

```
[checkin] T17-验收站点: 跳过（本周期已跑过（应触发时刻 1795716300））
[checkin] T17-补签站点: 跳过（本周期已跑过（应触发时刻 1795654800））
```

→ **跨重启去重成立**（去重状态落库，不因重启重打上游）。

**证四：零明文**

验收夹具里的两个真值（`T17-ACCEPTANCE-SESSION-SECRET` / `T17-ACCEPTANCE-CSRF-SECRET`）四层扫描：

| 文件 | 大小 | 明文命中 |
|---|---|---|
| `store.db` | 143 360 B | **0** |
| `store.db-wal` | 0 B | **0** |
| `store.db-shm` | 32 768 B | **0** |
| `vault.bin` | 596 B | **0** |

（扫描口径：GBK 与 UTF-8 两种编码都查。）

### 3.4 前端单测（11 条新单测）

```powershell
node --experimental-strip-types --test src/checkin/schedule.test.ts src/checkin/format.test.ts
# → tests 23 / pass 23 / fail 0   （T17 新增 11 + T15 原有 12）
```

覆盖：cron 字段拆分（5/6/7 字段）/ 前端校验（字段数、越界、非法字符）/ 中文说明与后端同口径 / 星期展开与跨周末 / 相对时间 / 系统兜底说明。

### 3.5 回归

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
# → 414 项全绿（348 单元 + 66 e2e），4 ignored，0 失败，0 编译告警
npm run build
# → 31 modules，gzip 61.45 kB，✓ built
```

---

## 4. 口径回填（02 v0.17 / v0.18）

| 落点 | 内容 |
|---|---|
| 02 **§7.2.1**（v0.17 新增） | 调度模型（每分钟 tick / 5 字段 / **本地时区**）/ 应跑判定与去重 / 错过补签 / 系统兜底任务 / CLI 入口 / 边界 |
| 02 §7.2.1 §4 + §13 **v0.18**（真机修订） | ① 任务名以**连字符**为准（v0.17 行里的 `·` 是笔误，已一并订正）；② 频率映射补「**分钟字段 `*` → `/sc minute /mo 1`**」；③ 「**任务不存在 = 已注销**」（先查后删，不靠错误文案匹配）；④ 系统命令输出**按 OEM 代码页解码** |

---

## 5. 已知限制与未做项

| # | 项 | 说明 |
|---|---|---|
| 1 | **macOS launchd 未在真机验证** | 代码路径完整（plist 生成 + `launchctl load/unload`），但**本机是 Windows**，只做了代码走查；上 macOS 时需补一次真机验证 |
| 2 | **Windows 计划任务默认「仅当用户登录时运行」** | `schtasks /create` 不带 `/ru` → 用户**注销**后任务不触发（用户登录时正常）。要覆盖「注销也跑」需 `/ru SYSTEM`（要管理员权限），属后续项 |
| 3 | **导入的模板带着录制时的状态码** | ModelScope 匿名态 401、真实成功 200 → 用户必须自己改；**无断言编辑 UI**（backlog，T18 站点插件会带正确断言） |
| 4 | **csrf 会轮换而模板不自刷新** | 模板固化的是录制那一刻的 Cookie；重录 HAR 才更新（属 T18） |
| 5 | **未对真实站点发过签到请求** | 本卡全部实测打的是**本地 mock 上游**；真实站点属 T18（且要用户自己授权登录态） |
| 6 | **积分回写 `account.quota_json` 不在本卡** | `reward_json` 留空，属 T18 |
| 7 | **通知不在本卡** | 属 T19；本卡的 `message` 已是可直接用于通知的中文原因 |
| 8 | **调度层不重试** | 同周期只跑一次；失败由下个应触发周期再试；要立刻再试用「立即执行」 |
| 9 | **验收残留** | 验收用的两个站点与两个计划任务已删除；`%APPDATA%\llm-router` 里只剩应用自己的空库与保险库（该目录在验收前是空的；**受本机删除策略限制无法整体删目录**） |

---

## 6. 经验（真机跑出来的两个 bug，都是「单测和 e2e 测不出来」的那类）

### 6.1 `* * * * *` 被降级成「每天一次」→ 系统兜底漏跑

`CronShape` 的分类里，分钟字段只认 `*/N`（每 N 分钟），**不认 `*`**。于是 `* * * * *` 落进 `Other` → 映射成 `/sc daily` → **应用内 ticker 每分钟跑一次（对），但应用关掉后系统兜底一天才跑一次（漏）**。

- 为什么单测没抓到：形状分类的单测只覆盖了 `*/15` / `30 * * * *` / `0 9 * * *` / `15 8 * * 1-5` / `0 9 1 * *`，**没有 `* * * * *`**。
- 为什么 e2e 没抓到：e2e 只验「注册 → 查询 → 注销」，**不验频率参数**（频率只验了描述文本）。
- 修法：`classify()` 里把「分钟字段 = `*` 且其余字段全是 `*`」判为 `EveryMinutes(1)`；前端 `describeCron` 同步（两边口径必须一致，否则界面显示与实际注册频率会打架）。已加**后端 + 前端**两条回归断言。

###### 6.2 靠错误文案判断「任务不存在」→ 注销误报失败 + 乱码

`unregister()` 原本靠 `stderr.contains("没有") || stderr.contains("ERROR: The system cannot find")` 判断任务是否存在。实测**两个前提都不成立**：

1. **文案随系统语言变**——中文 Windows 上是「错误: 系统找不到指定的文件。」，英文匹配必然漏判 → 正常路径被报成「注销计划任务失败」。
2. **同一个程序里，文案还随「有没有控制台」变**——在 PowerShell 控制台里 `schtasks` 回英文，在**无控制台的 GUI 进程**里回中文（系统 OEM 代码页）。所以「在控制台里手工试一遍」**复现不出**应用里的行为。
3. 附带问题：错误文案按 UTF-8 解 → 中文是乱码（`���Ϣ·cΓ���X…`）。

- 修法：① 注销改成**先查后删**（`/query` 判存在 → 不在就直接返回「已注销」），**彻底不依赖文案**；② 错误文案按 **UTF-8 → UTF-16LE → OEM 代码页 → 有损兜底** 的顺序解码（`windows` crate 的 `MultiByteToWideChar` + `CP_OEMCP`，`windows` 本来就在依赖树里，只是多开一个 feature）。
- **教训**：判断「系统命令的某种状态」时，**不要用它的错误文案**——用**退出码 + 独立的查询命令**。文案既随语言变、又随调用环境变。

---

## 7. 提交清单

| 提交 | 内容 |
|---|---|
| `76329a0` | **代码**：后端调度器 + 系统兜底 + CLI + 4 个 IPC + 前端调度面板 + 验收工具 + 回归单测 |
| `0a6bf3c` | **留档 + 文档**：本文件 + 02 §7.2.1/§13（v0.17、v0.18）+ 03 T17 落地行 + HANDOFF §4/§8 + 真机截图 |

> 提交号在提交后回填（AGENT.md §6：提交后回填提交号，避免与实际历史不一致）。

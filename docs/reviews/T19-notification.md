# T19 通知 —— 验收留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2「**T19 通知**」
> 口径：`docs/planning/02-technical-design.md` §7.5（+ §13 v0.25）
> 日期：2026-11-27　·　分支：`dev` → `main`
> 声明：本卡只动「通知」这一层（系统通知 / webhook / 配置落点 / UI 区块）；
> **不含**邮件、短信、钉钉、企微应用消息、通知历史与已读状态、模板自定义、@人。

---

## 1. 结论

**已验证（带证据）**：通知链路（文案、通道、密文边界、落点）全部打通；**Windows 计划任务路径
（应用完全关闭）真机把一条失败通知送进了 webhook 接收器**。

| 判据（03 T19 卡片） | 状态 | 证据 |
|---|---|---|
| 失败通知含**站点名与原因** | ✅ 已验证 | §4.1 单测 ① · §4.2 ②③ |
| 成功通知含**站点名与本次到账** | ✅ 已验证 | §4.1 单测 ② · §4.1 e2e ① |
| 通知落点覆盖**四条执行路径**（含**应用关闭时**的计划任务） | ✅ 已验证（真机计划任务 → webhook 真送达） | §4.2 ③ |
| webhook 地址**四层零明文**（库 / WAL / SHM / 保险库） | ✅ 已验证（**真机全库**扫描） | §4.2 ④ |
| 通知失败**不影响签到结果落库** | ✅ 已验证 | §4.1 e2e ④ |
| **系统通知横幅肉眼可见** | ❌ **未验证**（本机取不到 shell 合成层的证据） | §5 ① |
| **真实企微 / 飞书 / Telegram 群机器人** | ❌ **未验证**（只验了本地接收器 + 真实 HTTP 路径 + 真实请求体形状） | §5 ② |

**一句话**：通知这条链**代码通、文案对、密文边界干净、落点一处覆盖四条路径**，并且**在应用完全关闭的情况下
由 Windows 计划任务真机发出过一条带站点名与原因的失败通知**；剩下两项要头头参与才敢盖章——
「屏幕横幅是否肉眼可见」与「真实群机器人」。

---

## 2. 改了什么（范围）

**后端（Rust）**

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/checkin/notify.rs` | **新** | `NotifyConfig` / `NotifySettings` / `NotifyConfigInput` / `Notification` / `ChannelResult`；`load_settings` / `config_view` / `save_settings`（webhook 地址进保险库 `kind='webhook'`，挂占位渠道「通知」；`app_setting` **只存凭据 id**）；`build_notification`（标题 `签到成功/失败 · {站点名}`，正文首行 `站点：{站点名}` + 分类 + 原因 + 本次到账）；`webhook_body`（企微 / 飞书 / Telegram 三种请求体形状）；`send_system`（notify-rust）；`send_webhook`（reqwest **没开 `json` feature** → 手动序列化 + 手动带 `CONTENT_TYPE`）；`redact_request_error`（错误归一成**不含地址**的一句话）；`send`（**各通道互不影响**，一个失败不挡另一个） |
| `src-tauri/src/checkin/system_notify.rs` | **新** | **Windows AUMID 注册**：COM 建开始菜单快捷方式 + 写 `System.AppUserModel.ID`（`PKEY_AppUserModel_ID`）→ `IPersistFile::Save` 落盘；非 Windows 直接 `Ok(None)` |
| `src-tauri/src/checkin/scheduler.rs` | 改 | `run_site` 在 `record` + `write_back_reward` **之后**调 `notify_result`（**结果已落库才通知**）；新增 `notify_result`（总开关 / 成功可关 / **失败永远通知** / 通知失败只记日志、**不回滚签到**） |
| `src-tauri/src/commands/notify.rs` | **新** | 3 个 IPC（见 §3）；`notify_test` **不受总开关限制**（用户点了就是要试通道） |
| `src-tauri/src/checkin/mod.rs` / `src-tauri/src/commands/mod.rs` / `src-tauri/src/lib.rs` | 改 | 挂 `notify` + `system_notify` 两个模块；注册 3 个 IPC |
| `src-tauri/Cargo.toml` | 改 | `notify-rust = "4.11"`；`windows` 加 5 个 COM/Shell feature（`Win32_System_Com` / `…_StructuredStorage` / `Win32_System_Variant` / `Win32_UI_Shell` / `…_PropertiesSystem`） |
| `src-tauri/tests/checkin_notify_e2e.rs` | **新** | 5 条 e2e（§4.1） |
| `src-tauri/tests/checkin_plugin_e2e.rs` | 改 | 测试环境写 `notify_enabled=0`（**e2e 不弹 toast**） |

**前端（React / TypeScript）**

| 文件 | 性质 | 内容 |
|---|---|---|
| `src/checkin/notify.ts` | **新** | `WEBHOOK_KINDS`（企微 / 飞书 / Telegram / 不使用）/ `needsChatId` / `webhookPlaceholder` / `webhookUrlToSend`（**留空 = 不动**已配置的地址）/ `describeNotifyState`（一句话说清后果） |
| `src/checkin/CheckinPanel.tsx` | 改 | 「**结果通知（T19）**」区块：开启通知 / 系统通知（弹窗）/ 成功也通知 / webhook 类型 / 地址（**password 输入、不回显**、留空=不动、清除按钮）/ Telegram `chat_id` / 保存 / 测试通知 |
| `src/checkin/types.ts` | 改 | `NotifyConfig` / `NotifyConfigInput` / `NotifyChannelResult` |
| `src/checkin/notify.test.ts` | **新** | 5 条前端单测 |

**依赖决策**：`notify-rust` 而**不是** Tauri 通知插件——CLI `--checkin-once` 与 **Windows 计划任务路径没有 Tauri app handle**，
用插件就只能在应用开着时通知，**覆盖不了「应用关闭时也通知」这条 M3 判据**（§4.2 ③ 已真机证明这条路径能发）。

---

## 3. IPC 面（3 个新增）

| IPC | 入参 | 出参 | 密文边界 |
|---|---|---|---|
| `notify_get_config` | — | `NotifyConfig` | 只回「**配没配**」，**不回显地址** |
| `notify_set_config` | `NotifyConfigInput` | `NotifyConfig` | 地址进保险库（`kind='webhook'`）；`app_setting` 只存凭据 id |
| `notify_test` | — | `Vec<ChannelResult>` | 通道结果**只有一句话**，不含地址 |

`NotifyConfigInput.webhook_url` 三态：`None` = **不动** / `Some("")` = **清除**（删凭据 + 清设置，不留孤儿凭据）/ `Some(url)` = **设置**。

---

## 4. 证据

### 4.1 自动化测试

| 层 | 数量 | 覆盖 |
|---|---|---|
| Rust 单测（`checkin::notify`） | 4 | 失败通知含**站点名与原因** / 成功通知含**本次到账** / 三种 webhook 请求体形状 / **错误信息不泄地址**（用回环 `127.0.0.1:9`——**单测不出网**，别真去打企微） |
| Rust e2e（`checkin_notify_e2e.rs`） | 5 | 成功通知含站点名 / 失败通知含站点名与原因 / **webhook 地址零明文**（库 + WAL + SHM + 保险库**四层**扫描）/ **webhook 坏了不影响签到落库** / 关掉开关**不发** |
| 前端单测（`notify.test.ts`） | 5 | 只有 Telegram 要 `chat_id` / 输入框按类型给提示 / **留空 = 不动**已配置地址 / 状态一句话说清后果 / 类型清单覆盖三种群机器人 + 不使用 |

**全量回归**：`cargo test --all-targets` **458 项全绿**（0 失败 / 4 ignored / 0 编译告警）；前端 **49/49**；
`npm run build` 通过（33 modules，gzip 64.24 kB）。

### 4.2 真机验收（真实应用数据 + 真实 HTTP）

**① 系统通知走通 API**：`scheduler::run_site` 跑完打印 `[notify] 系统通知：已发送`。

**② webhook 真实 HTTP 路径**（本地接收器：`POST /hook` 记 body、`GET /api/v1/users/login/info` 回 401）
收到**企微形状**的原文：

```json
{"msgtype":"text","text":{"content":"签到失败 · ModelScope（魔搭）\n站点：ModelScope（魔搭）\n分类：credential_invalid\n原因：ModelScope 登录态无效（令牌过期或没填）——去魔搭「个人中心 → 访问令牌」重新复制一个"}}
```

→ **站点名 ✓ / 分类 ✓ / 原因 ✓ / 不含任何密文 ✓**（企微 / 飞书只显示正文，所以正文首行必须自带站点名）。

**③ Windows 计划任务路径（应用完全关闭）真机送达**——本卡最关键的一条：

1. 应用**没运行**（`Get-Process llm-router` 空）；
2. 计划任务 `LLM Router Checkin - site_182212017abbf1808375126911d9c85d`（动作 `"…\llm-router.exe" --checkin-once --site site_…`）；
3. 临时把站点 cron 调到当前分钟（`13 12 * * *`）→ `schtasks /run`；
4. `Last Run Time: 2026/11/27 12:13:38` → 接收器**同时收到**上面那条通知体；
5. `checkin_result` 新增 **id=19**（`ok=0`，消息含分类 `credential_invalid`）；
6. cron 还原 `0 9 * * *`。

→ **「应用关闭时也能通知」这条 M3 判据有真机证据**（不是靠读代码推的）。

> 顺带确认的退出码语义（T17 的契约，别误读）：计划任务 `Last Result = 1` = **签到失败**；
> `0` = 成功；`2` = 环境问题（库 / 保险库 / 引擎）。**`Last Result: 1` 不是「任务挂了」**。

**④ webhook 地址四层零明文**（**真机库**上跑，webhook 真实存在时）：

```
python scripts/security_check.py --store "$env:APPDATA\llm-router\store.db" \
       --vault "$env:APPDATA\llm-router\vault.bin" --secret "<webhook 完整地址>"
→ 明文凭证命中：0 条（必须为 0）
→ 结论：四条判据全过        退出码 0
```

**⑤ AUMID 注册落地**：开始菜单出现 `LLM Router.lnk`（1134 B）——Windows toast 的前置条件已就位。

### 4.3 真机揪出并修掉的问题

| # | 现象 | 根因 | 修法 | 回归 |
|---|---|---|---|---|
| ① | `notify-rust` 报「已发送」但**屏幕什么都没有** | Windows 10 1607+ 的 toast 要求**调用进程拥有该 AUMID**；**非 powershell 进程冒用 PowerShell 的 AUMID 会被 OS 静默丢弃**（API 仍返回成功——日志说成功、屏幕什么都没有） | 自建 AUMID（`LLM Router`）+ COM 注册开始菜单快捷方式 + 写 `System.AppUserModel.ID`；进程内 `OnceLock` **只注册一次** | 开始菜单快捷方式已生成；横幅可见性仍待目视确认（§5 ①） |
| ② | 企微 / 飞书的错误信息里带出**地址**（地址里含 key / bot token） | reqwest 错误默认把 URL 拼进消息 | `redact_request_error` 归一成「请求超时 / 连接失败 / HTTP `<code>` / 请求失败」四类，**零地址** | 单测断言错误里既没有 key 也没有 host |
| ③ | e2e 跑起来**真的弹系统通知**（测试环境不该打扰人） | e2e 用的真 `app_setting` | `checkin_plugin_e2e.rs` 的 `app_state()` 写 `notify_enabled=0`；通知 e2e 里 `system_enabled=false`（系统通知那条路**留到真机验**） | 全量回归不再弹 toast |

---

## 5. 未验证项

| # | 未验证项 | 需要什么 | 解锁后要回填什么 |
|---|---|---|---|
| ① | **系统通知横幅肉眼可见** | 头头在触发后**看一眼屏幕** | 本机 GDI（`ImageGrab`）与 DXGI（`dxcam`）都取不到 shell 合成的横幅，系统通知库也不记我们这类「无历史条目」的 toast；**本机连 PowerShell 原生 toast 同样不显示**，疑似系统层面（专注助手 / 通知设置）拦截。代码侧已修掉一个**真实**根因（AUMID 静默丢弃），**横幅可见性仍待目视确认** |
| ② | **真实企微 / 飞书 / Telegram 群机器人** | 头头给一个**群机器人地址**（**当密文进保险库**，不经聊天记录留存） | 三种格式在真实群的接收端行为（企微文本消息有长度上限、Telegram 必填 `chat_id`）+ 真实返回体 |
| ③ | **多群 / 多通道同时发** | 不在本卡 | webhook 现在**只有一个地址**；要同时发多个群得改配置结构（backlog） |
| ④ | **调度器自身异常时的告警** | 不在本卡 | 通知只覆盖**签到结果**；调度器压根没跑起来（保险库锁着等）→ **没人通知** |

---

## 6. 已知限制

1. **Windows 上必须有自己的 AUMID**——所以会建一个开始菜单快捷方式（`LLM Router.lnk`）；**卸载应用后这条快捷方式会留下**（待清理，backlog）。
2. **失败通知不可关**（AGENT.md §5.7「撞到验证码 = 失败并通知」在这里落地）；**成功通知可关**（`notify_success`，默认开）。
3. **通知失败只记日志、不重试、不回滚签到**——与 §7.4「写回失败不回滚签到」同口径。
4. **通知内容零敏感信息**：只有分类、中文原因、站点名与本次到账数；**不带**凭据 / Cookie / 令牌 / 完整响应体。
5. **只有一种 webhook 地址**（三种格式共用一个配置）——见 §5 ③。
6. **Telegram 需要额外的 `chat_id`**（单独一项设置，**不是密钥**，可明文存 `app_setting`）。
7. **测试通知不受总开关限制**（用户点了就是要试通道），但**不会发真实签到结果**。
8. **计划任务指向开发态二进制** `target/debug/llm-router.exe` → `cargo clean` 会让兜底任务失效（T17 已知限制，本卡未变）。
9. **系统通知只覆盖签到结果**，调度器自身异常不告警（§5 ④）。

---

## 7. 回归与提交

| 项 | 结果 |
|---|---|
| `cargo test --all-targets` | **458 项全绿**（0 失败、4 ignored、0 编译告警） |
| `cargo check --all-targets` | **0 warning** |
| 前端单测 | **49/49** |
| `npm run build` | 通过（33 modules，gzip **64.24 kB**） |
| 提交 | **口径与任务卡** `464f42c`（02 §7.5 + §13 v0.25 + 03 T19 卡片）· **代码** `4b6f0ee`（通知模块 + 3 个 IPC + 前端区块 + 测试）· **留档与账本** `581de44`（本文件 + HANDOFF） |

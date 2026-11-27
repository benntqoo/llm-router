# HANDOFF — 开发工作交接

| 项 | 值 |
|---|---|
| 交接日期 | 2026-11-25 |
| 交接对象 | 任意接手实际开发的 agent 会话（MiniMax Code / 其他） |
| 项目阶段 | M3 开发中（M1/M2 里程碑已汇报；M3 第一张卡 T15 已完成）；进度账本见 §8 |
| 本文件改动范围 | 每次仅更新 §4 执行入口与 §8 账本；代码提交号见账本行 |

---

## 0. 给接手 agent 的第一段话

先按顺序读完：`AGENT.md`（规范红线）→ `docs/planning/03-delivery-plan.md`（任务卡，你的工作清单）→ `docs/planning/02-technical-design.md`（技术设计，实现依据）。

然后从 **T0** 开始。每完成一个任务，验收判据实测留档到 `docs/reviews/`。遇到本文件 §5/§6 的未验证项，先实测再实现。发现任何文档矛盾或前提失效：**停下上报，不要自行裁决**（AGENT.md §2/§6）。

---

## 1. 项目与产品定义

**LLM Router**（仓库 `D:/Code/llm-router`）：本地 AI 网关桌面应用。

- 用户在应用内加密录入各 LLM 渠道凭证（DeepSeek/Kimi/GLM/MiniMax/百炼/硅基流动 + GitHub Copilot OAuth + WorkBuddy 登录态），可随时回看（解决各平台"密钥只显示一次"）。
- 任何 OpenAI 兼容客户端只需填一次 `http://127.0.0.1:<port>/v1` + 本地 client key。
- 附带：HAR 模板式自动签到、token 用量与成本统计。
- **明确不做**：修改 harness 配置文件、协议转换（v1）、多用户/云同步、风控规避。完整边界见 02 §0。

## 2. 决策时间线（已锁定，不许重开）

| 日期 | 决策 |
|---|---|
| 2026-11-24 | 技术栈 = Tauri 2 + Rust + React；v1 含全部 5 模块（网关/保险库/签到/反代/用量）；密钥加密 = OS 原生 + 可选主密码；模型命名 = 别名制（用户自定义，支持 route_group 编排） |
| 2026-11-24 | 与开源竞品关系 = 自研替代（非编排层）；不做配置文件注入 |
| 2026-11-25 | Harness 反代 v1 仅 **GitHub Copilot + WorkBuddy**；**Anthropic 入站不进 v1**（pi 已验证支持 OpenAI 兼容，前提成立）；MiniMax 需处理 group_id（见 02 §4.2 口径） |
| 2026-11-25 | FreeLLMAPI（58k★，MIT）调研：路线定为 **B 保持 Tauri 自研**，将其列为头号参考实现（`docs/reviews/freellmapi-analysis.md`）；X-Routed-Via / 限额自学习 / sticky_session 三处设计已回填 02（v0.2）；**维持"永不修改 harness 配置文件"红线**，不做一键配置向导 |

## 3. 文档索引

| 文件 | 内容 | 何时读 |
|---|---|---|
| `AGENT.md` | 规范红线、流程、验收命令 | 开工前 |
| `docs/planning/01-architecture-and-feasibility.md` | 竞品调研（CCR/OmniRoute/9router/TraeWorkAssistant）、可行性、DDL 基础 | 需要决策依据时 |
| `docs/planning/02-technical-design.md` | **实现依据**：v1 边界、架构、反代流程、鉴权、转换规则 R-1~R-8/P-1~P-6、MiniMax 口径、适配器规格、实测矩阵 | 写每个任务前 |
| `docs/planning/03-delivery-plan.md` | **任务卡** T0–T24：目标/产出/验收/依赖、6 周排期、降级预案 | 每天开工时 |
| `.workbuddy/memory/2026-11-24.md` | 决策与调研过程记录（含全部证据来源） | 追溯某结论出处时 |

## 4. 执行入口

```
第 1 步：~~T0 仓库初始化~~ ✅（提交 `c98bd69`，验收留档 `docs/reviews/T0-scaffold-verification.md`）
第 2 步：~~T1 数据层~~ ✅（提交 `2bf7df5`，验收留档 `docs/reviews/T1-datastore-verification.md`）
第 3 步：~~T2 加密保险库~~ ✅（提交 `ac93d35`，验收留档 `docs/reviews/T2-vault-verification.md`）
第 4 步：~~T3 网关骨架 + 入站鉴权~~ ✅（提交 `359b2ce`，验收留档 `docs/reviews/T3-gateway-auth-verification.md`）
第 5 步：~~T4 出站适配器~~ ✅（提交 `364974f`，验收留档 `docs/reviews/T4-outbound-adapter-verification.md`）
第 6 步：~~T5 路由与别名~~ ✅（提交 `eb3fddd`，验收留档 `docs/reviews/T5-routing-alias-verification.md`）
第 7 步：~~T6 流式管道 + fallback + 熔断~~ ✅（提交 `ad50996`，验收留档 `docs/reviews/T6-stream-fallback-circuit-verification.md`）
第 8 步：~~T6 追加：熔断落库 + UI 展示 + 429/Retry-After 口径~~ ✅（头头裁决后落地，留档 `docs/reviews/T6-circuit-persistence-verification.md`）；同批追加二（熔断时长可配 + 手动解除）、追加三（429 冷却上限可配，提交 `96be851`）也已落地
第 9 步：~~T7 usage 采集~~ ✅（提交 `59053b2`，留档 `docs/reviews/T7-usage-capture.md`）
第 10 步：~~T8 A 类预设 + 自定义渠道~~ ✅（提交 `a340a29`，留档 `docs/reviews/T8-preset-and-custom-channel.md`）
第 11 步：~~T9 UI 第一轮~~ ✅（提交 `e70df27`，留档 `docs/reviews/T9-ui-first-round.md`）
第 12 步：~~T10 实测矩阵（A 类 7 列 + pi 接入）~~ 🟡 **pi 实连已通过**（留档 `docs/reviews/T10-channel-matrix-and-pi.md`）；**A 类 7 列真实矩阵经头头决定挂起**（先不测真实上游，见 §8 账本）
第 13 步：每任务完成 → 验收留档 docs/reviews/<任务号>-*.md
第 14 步：~~T11 安全验收~~ ✅（02 §10.2 四条判据全过；补上了 §5.2 承诺但一直没接的日志脱敏中间件；留档 `docs/reviews/security-check-m1.md`）
第 15 步：M1 全绿（03 §1 判据）→ 向头头汇报，再进 M2 —— **M1 判据 ②③④ 已验；① 的真实 DeepSeek 部分按头头决定挂起**，汇报件 `docs/reviews/M1-milestone-report.md`
第 16 步：~~T12 本地估算器~~ ✅（产线默认换成 tiktoken-rs；"无 usage 走估算路径"e2e + 真机双证；留档 `docs/reviews/T12-token-estimator.md`）；~~T13 价目表与成本换算~~ ✅（三价换算 + 47 行预填牌价 + 真实上游实测；留档 `docs/reviews/T13-price-and-cost.md`）；~~T14 聚合与图表 UI~~ ✅（四维聚合 + SVG 图表 + 用量面板；M2 判据全过；留档 `docs/reviews/T14-usage-aggregation.md`）—— **M2 三张卡齐了**，下一步进 M3（可签到）
第 17 步：**M2 里程碑汇报** —— 判据（03 §1）一条拆成 6 个子项，**逐条实测全绿**（流式请求 ✓ / tokens ✓ / TTFT ✓ / tokens·s⁻¹ ✓ / 估算成本 ✓ / `estimated` 标记 ✓），汇报件 `docs/reviews/M2-milestone-report.md`；**未做项**：价目表编辑 UI（另排一张卡）
第 18 步：~~T15 HAR 导入与模板化~~ ✅（M3 第一张卡；判据「导入一个真实 HAR 并标记 2 个变量成功」——**Playwright 实抓的真实 HAR**（脱敏成可提交夹具）+ **真机 UI 自动化**双证；留档 `docs/reviews/T15-har-import-and-template.md`，截图 `app-ui-t15-checkin.png` / `app-ui-t15-fields.png`）
第 19 步：~~T16 重放引擎~~ ✅（M3 第二张卡；判据「变量注入、请求重放、响应断言（状态码 + JSON 路径）、错误分类；单测 3 场景」——**单测 16 + e2e 4（用 T15 的真实产物打真 mock 上游）+ 真机验收工具 5 场景**四证；留档 `docs/reviews/T16-replay-engine.md`）→ 下一步 **T17 调度器 + 系统兜底**（cron 调度、Windows 计划任务 / launchd 注册、错过补签；依赖 T16 ✅）
第 20 步：~~T17 调度器 + 系统兜底~~ ✅（M3 第三张卡；判据「关闭应用后计划任务仍触发并补签」——**真机四证**：真实应用 UI 填 cron → 勾启用 → **真实 Windows 计划任务注册**（动作实测指向应用自身 exe）→ **`taskkill` 关掉应用** → 任务在 02:04:01 / 02:05:01 **持续触发**（每次带保险库真值 cookie、上游命中、结果落库 `ok=1`、`Last Result: 0`）；**补签**（cron `0 9 * * *` 当天已过 → 补签执行成功）/ **同周期去重** / **跨重启去重** / **四层零明文**；**单测 19 + e2e 6（含真实 schtasks 注册·查询·注销）+ 前端单测 11**；留档 `docs/reviews/T17-scheduler-and-system-fallback.md`，截图 `app-ui-t17-schedule.png`）→ 下一步 **T18 站点插件 ×2**（ModelScope + MiniMax Code）
第 21 步：~~T18 站点插件 ×2~~ ✅ **失败路径 + MiniMax Code 成功路径已验收通过；ModelScope 成功路径待头头的网页会话 Cookie**（M3 第四张卡；**内置插件机制**（`checkin_site.plugin` 分派：`har` → 重放引擎 / `modelscope` / `minimax_code` → 内置插件）+ **浏览器辅轨**（`sidecar/` Node + Playwright + **系统 Chrome** + 持久化 profile；CLI 协议，stdout 吐 JSON）+ **积分回写 `account.quota_json`**（合并写，不动限额字段）+ **7 个新 IPC** + 前端「内置站点插件」区块；**真机四证**：① ModelScope **打真实 `www.modelscope.cn`**（错误 Cookie → HTTP 401 → `credential_invalid`，152 ms）② MiniMax Code **走辅轨打真实 `agent.minimax.cn`**（Rust → Node → Playwright → Chrome 全链，`not_logged_in`，2741 ms）③ **四层扫描明文命中 0** ④ 辅轨探活（chrome / Node v22.16.0）；**真机揪出并修掉 4 个 bug**（TDZ / 插件没覆盖 trait 默认 cron / `login` 渲染前假阳性 / `checkin` 渲染前误判）；**Rust 单测 21 + e2e 9 + 前端单测 29**；留档 `docs/reviews/T18-site-plugins.md`，截图 `app-ui-t18-plugins.png` / `app-ui-t18-sidecar-login.png`）→ **成功路径第二轮**（头头给了 MiniMax Code 登录态）：**短信登录被站点频控**（「操作频繁」）→ 辅轨改为**自动打开登录弹窗 + 自动切「账号密码登录」**（**只铺路**，不代填密码、不绕人机验证），**登录成功**（`user_id` 硬信号）；**签到入口不在首页**，在**用户头像下拉**的「每日签到」；**端到端真签到跑通**（UI「签到成功（success）：MiniMax Code 今天已经领过了（连签第 7 天）」，**4307 ms**，落库 `ok=1`）；**每日 400、第 4/7 天各 1000 实测对上任务卡**；**「今天已领」判成功（幂等）**；**下一张卡 = T19 通知**；**T18 剩余 = ModelScope 的 Cookie**（贴到应用「站点调度 → ModelScope（魔搭）」行的「网页会话 Cookie」输入框点保存，明文只进保险库）
第 22 步：~~T19 通知~~ ✅（M3 第五张卡；判据「失败通知含站点名与原因」——**真机四证**：① 本地接收器收到**企微形状**原文（**站点名 + 分类 + 原因**、**零密文**）② **Windows 计划任务路径（应用完全关闭）真机送达**：`schtasks /run` → 接收器**同时**收到通知体 + `checkin_result` 新增 id=19 → **「应用关闭时也能通知」有真机证据**（`Last Result = 1` = **签到失败**，不是任务挂了；T17 退出码契约 0/1/2）③ **真机全库四层扫描**（带 webhook 明文做 `--secret`）→ **明文凭证命中 0 条、退出码 0** ④ AUMID 注册落地（开始菜单 `LLM Router.lnk`）；**新增** `checkin::notify` + `checkin::system_notify` + `commands::notify`（**3 个 IPC**）+ 前端「结果通知（T19）」区块；**系统通知用 `notify-rust`**（**刻意不用 Tauri 通知插件**——CLI 与计划任务路径没有 app handle）；**落点 = `scheduler::run_site` 之后**（结果已落库才通知）→ 一处覆盖四条路径；**失败永远通知**、成功可关、通知失败不影响签到结果；**webhook 地址进保险库**、设置表只存凭据 id、**错误不回显地址**；**真机揪出并修掉**：① **toast「已发送」但屏幕什么都没有**（根因：**非 powershell 进程冒用 PowerShell 的 AUMID 会被 OS 静默丢弃** → 自建 AUMID + COM 注册开始菜单快捷方式）② reqwest 错误带出地址 → 统一脱敏；**Rust 单测 4 + e2e 5 + 前端单测 5**；回归 **458 项全绿** + 前端 49/49 + build 64.24 kB；留档 `docs/reviews/T19-notification.md`）→ **M3 五张卡齐了**；**两项未验证**：系统通知横幅肉眼可见（本机连 PowerShell 原生 toast 也不显示，待目视）+ 真实群机器人（待头头给地址，当密文进保险库）——见 §5 第 7 项
第 23 步：**M3 里程碑汇报** —— 判据（03 §1）一条拆成 **6 个子项**，**逐条实测全绿**（① 指定 cron 触发两个站点签到 ② 成功写 `checkin_result` ③ 成功通知 ④ 人为改坏模板后判失败并落库 ⑤ 失败通知含站点名与原因 ⑥ 系统计划任务在应用关闭时仍触发）；**全程真站点 + 真凭据 + 真计划任务**，无手工数据；汇报件 `docs/reviews/M3-milestone-report.md`；**未做项**：真实群机器人（待头头给地址）+ ModelScope「差值 > 0」分支（等次日首次运行）→ 下一步 **M4 可反代**（T20 / T21 / T22 / T23 / T24）
```

排期基准 6 周（03 §4）。滞后处理按 03 §5 降级预案，不擅自砍范围。

## 5. 未验证项（开工会撞到，先测再做）

| # | 项 | 影响任务 | 验证方法 |
|---|---|---|---|
| 1 | **WorkBuddy 上游 API host 与登录态字段**。本机已实测：登录态在 `~/.workbuddy/app/users/<64hex>/`，设备标识 `~/.workbuddy/device-id`（UUID 可读写）；内部字段与 API host 未知 | T22（抓包）→ T23 | 代理抓 WorkBuddy 桌面端一次对话；产出 `docs/reviews/workbuddy-protocol.md`（**只记字段名，严禁记录 token 值**）。发现签名/加密 → 触发降级（仅保活+额度查看） |
| 2 | **MiniMax 国内站 GroupId**：第三方口径分裂（有带/不带）。官方国际站确认不带 | T8/T10 | 三种口径逐一实测（不带/带 GroupId query/旧版路径），结论回填 02 §4.2 预设 |
| 3 | ~~**pi CLI 的自定义 provider 配置格式**~~ | T10 | ✅ **已实测**（2026-11-26）：配置在 `~/.pi/agent/models.json`（provider 定义）+ `~/.pi/agent/settings.json`（默认 provider/model）；`api` 取 `openai-completions`；`apiKey` 用 `$ENV_NAME` 插值（`$$`/`$!` 转义、`!command` 取命令输出）。权威口径见 02 §6.5，依据是 pi 1.6.1 **自带**的 `docs/models.md` 与 `schemas/models.schema.json`；片段生成器 `scripts/pi_models.py` |
| 4 | DeepSeek/Kimi/GLM/百炼/硅基流动 base_url（常识级，未逐一到控制台复核） | T8/T10 | 各渠道控制台文档核对一遍 |
| 5 | 国产渠道 `tools`（tool_calls）兼容性参差 | T10 | 实测矩阵 02 §10.1，结果决定预设标 `stable`/`partial` |
| 6 | **T18 剩余的成功路径**：① ModelScope 的**「本次到账 > 0」分支**（差值 > 0）——真机当天**已领过**（当日 `daily_active` 200 魔粒已在流水里）→ 实测跑的是**差值 = 0 → 判「今天已领过」**；**差值 > 0** 要等**次日首次运行**才验（届时应得 `points = 200` + 回写额度池）；② MiniMax Code 的 **`/signin/claim` 真正领取响应体**与**领取按钮真实文案**（真机当天**已经领过**，只验到幂等路径；`status` 字段只实测到 `3` = 已领取）；③ **积分回写对真实站点**（两站真机那次都是「已领过」→ 本次无新增积分，`quota_json` 保持 `{}`） | T18 收尾 / T19 | ① **不用头头再做什么**——次日站点首次运行后核对 `checkin_result.reward_json` 与 `account.quota_json` 即可；② 需一个**还没领过**的当天。实测后把口径**收敛成一条**回填 02 §7.4，并补 `docs/reviews/T18-site-plugins.md` §5 |
| 7 | **T19 只剩一项未验证**：**真实企微 / 飞书 / Telegram 群机器人**——本轮只验了**本地接收器 + 真实 HTTP 路径 + 真实请求体形状**（企微 `{"msgtype":"text","text":{"content":…}}`），**没打过真实群**。（**系统通知横幅已于 2026-11-27 15:36 目视确认弹出** → 判据闭环；**真根因**：不是代码问题，而是 **Windows 全局通知总开关关着**——`HKCU\…\PushNotifications\ToastEnabled = 0` → 事件日志 `Id=3150 … PolicyReason [GlobalSettingDisabled]`，**所有**应用的 toast 都被拦（别的应用同样中招，这解释了「连 PowerShell 原生 toast 也不显示」）；修法：打开开关 + **重启 `WpnUserService_<sid>`**（服务会缓存策略，不重启当场重发仍被拦）→ 投递恢复；代码侧已加**如实报告**（开关关着就报失败，不再谎报「已发送」）） | T19 收尾 / M3 判据 | 头头给一个**群机器人地址**（**当密文进保险库**，不经聊天记录留存）→ 贴到「签到 → 结果通知」的地址框保存 → 触发一次签到看群里是否收到。实测后回填 `docs/reviews/T19-notification.md` §5 与 02 §7.5 §6 |

## 6. 已知坑（前人踩过，别再踩）

1. **MiniMax：HTTP 200 但业务错误**。错误判定必须解析 `base_resp.status_code`，不能只看 HTTP 状态码（02 §4.2 映射表）。
2. **MiniMax 1004 ≠ key 错**。常见诱因是国内站缺 GroupId，报 401。UI 提示文案要引导查 GroupId 而非重新生成 key。
3. **CCR 的教训（本项目差异化依据）**：竞品把 key 明文存 SQLite（`config.sqlite` plain JSON，`api_keys.encryption` 只接受 `plain`——formal.ai 实测 + greenlitbooks 审计）。本项目的安全验收四命令（AGENT.md §9）一条不过就不算 M1 完成。
4. **流式 usage**：不注入 `stream_options.include_usage` 就拿不到 usage，统计模块会瞎（MiniMax 例外，其流式自带，注入多余字段有 400 风险——P-4 学习式兼容）。
5. **重试边界**：已向客户端写出首字节后严禁重试（会产生重复内容），只在首字节前重试（02 §2.1 ⑧）。
6. **竞品迭代快**：CCR/OmniRoute 日更。不要回头去"参考它们的最新功能"扩 v1 范围——范围变更必须走提案流程（AGENT.md §6-3）。
7. **参考实现纪律**：实现路由（T5/T6）与用量（T7/T13）前，先读 FreeLLMAPI 对应源码（`server/src/providers/openai-compat.ts`、路由与 analytics 文档见其 `docs/en/architecture/`）。**抄设计不抄代码**（它是 Node，我们是 Rust）。02 v0.2 的 P-7 / 限额自学习 / sticky_session 三处即为已验证收益。

## 7. 上报规则（何时必须停下问头头）

- 任务卡与 02 设计出现矛盾，或设计缺口超出 R-1~R-8 改写清单；
- §5 未验证项实测结果与文档假设不符（尤其 T22 抓包发现签名/加密）；
- 任何想新增/删除功能范围的念头；
- 实测中拿到真实 token/cookie 类数据（只记字段结构，原始值就地销毁，不留档）。

其余按 AGENT.md §6 流程正常推进，无需事事请示。

---

## 8. 交接清单与待办账本

**规划阶段已交付**

- [x] `AGENT.md` — agent 规范（角色/红线/流程/文档/Git/安全验收）
- [x] `HANDOFF.md` — 本文件
- [x] `docs/planning/01-architecture-and-feasibility.md` — 可行性与竞品
- [x] `docs/planning/02-technical-design.md` — 技术设计（版本与变更记录见其 §13）
- [x] `docs/planning/03-delivery-plan.md` — 开发计划 T0–T24

**开发账本（唯一；状态词：✅完成 / 🔴待办 / 🟡挂起 / ⏳验证点）**

| 状态 | 事项 | 依据 / 指针 |
|---|---|---|
| ✅ | T0 仓库初始化 | 提交 `c98bd69`（分支 `dev`）；留档 `docs/reviews/T0-scaffold-verification.md` |
| ✅ | T1 数据层（SQLite WAL + 迁移框架 + v1 全量建表） | 提交 `2bf7df5`；留档 `docs/reviews/T1-datastore-verification.md` |
| ✅ | T2 加密保险库（DPAPI/Keychain + AES-256-GCM + Argon2id + 30s 回看遮蔽） | 提交 `ac93d35`；留档 `docs/reviews/T2-vault-verification.md` |
| ✅ | T3 网关骨架 + 入站鉴权（client key/Host/限流/统一错误结构） | 提交 `359b2ce`；留档 `docs/reviews/T3-gateway-auth-verification.md` |
| ✅ | T4 出站适配器（ProviderAdapter trait + openai_compat R-1~R-8 + 7 个渠道预设 + MiniMax 错误码映射） | 提交 `364974f`；留档 `docs/reviews/T4-outbound-adapter-verification.md` |
| ✅ | T5 路由与别名（别名解析 + priority/round_robin/cost/sticky_session 四策略 + 未命中附可用别名 + fallback 候选链） | 提交 `eb3fddd`；留档 `docs/reviews/T5-routing-alias-verification.md` |
| ✅ | T6 流式管道 + fallback + 熔断（reqwest 出站转发 + SSE 零缓冲直通 + 首字节前重试 + 断连 abort 上游 + 熔断状态机 + 429 冷却 + P-3/P-4/P-7） | 提交 `ad50996`；留档 `docs/reviews/T6-stream-fallback-circuit-verification.md` |
| ✅ | T6 追加：熔断状态**落库 + 重启恢复**（`circuit_state` 表，schema v5）+ UI 展示（`circuit_status`）+ 限流冷却期回 429 + `Retry-After` | 头头 2026-11-26 裁决；留档 `docs/reviews/T6-circuit-persistence-verification.md`；提案 `docs/reviews/proposal-circuit-persistence.md` |
| ✅ | T6 追加二：熔断**时长可配置**（默认 60s / 最小 1s，`app_setting` 表，schema v6）+ **手动解除熔断**（`circuit_reset`） | 头头 2026-11-26 裁决；同上留档 §2.5 |
| ✅ | T6 追加三：**429 冷却上限可配置**（默认 60s / 最小 1s，`app_setting.rate_limit_cooldown_cap_secs`；冷却 = `min(max(退避, Retry-After), 上限)`，回给客户端的 `Retry-After` 为实际生效值） | 头头 2026-11-26 裁决；提交 `96be851`；同上留档 §2.7；提案裁决五 |
| ✅ | T7 usage 采集（流式**旁路 tee** 解析 usage 帧 + 记 TTFT + 非流式直读 + `estimated` 估算入口 + `usage_log` 落库（增 `account_id`，schema v7）+ per `(channel, account)` 的 RPM/RPD/TPM/TPD 计数 + 429 限额自学习回写 `quota_json`） | 提交 `59053b2`；留档 `docs/reviews/T7-usage-capture.md`；02 §8.1/§8.2（v0.6） |
| ✅ | T7 账号口径（**头头 2026-11-26 认可**）：API Key 型渠道按需补建默认账号 `acct_default_<channel_id>`（"账号"在 v1 是统一概念，后续接账号池不改采集层）。02 §8.2 口径不变 | T7 留档 `docs/reviews/T7-usage-capture.md` §4 |
| ✅ | T8 A 类预设 + 自定义渠道（02 §6.1 的 7 个 profile 全部接入 UI：每个预设带一条**默认模型建议值**（公开常识值，**未实测、可改**）→ 选中预设后**仅剩填 key 一步**；自定义 OpenAI 兼容渠道表单：base_url / 路径模板 / 鉴权样式（`bearer`｜`custom_header`）/ GroupId / include_usage 开关 / 额外固定头 / 模型别名 → 保存后别名**立即**出现在 `GET /v1/models`） | 提交 `a340a29`；留档 `docs/reviews/T8-preset-and-custom-channel.md`；截图 `docs/reviews/app-ui-channel.png`；02 §6.1.1/§6.1.2/§9（v0.7） |
| ✅ | T9 UI 第一轮（渠道列表/编辑 + **模型别名多行管理** + 渠道启用/禁用 + 优先级；密钥录入与回看（**30s 自动遮蔽**）且与渠道面板**联动一致**；**client key 管理**（创建 + 标签/限额编辑）；**`/v1/models` 预览**（与网关同源）+ 客户端接入片段） | 提交 `e70df27`；留档 `docs/reviews/T9-ui-first-round.md`；截图 `docs/reviews/app-ui-t9-channels.png` / `app-ui-t9-preview.png`；02 §6.6（v0.8）；**判据实测：从零配置 DeepSeek 渠道 + client key = 7.1 秒（上限 3 分钟）** |
| 🟡 | **T10 实测矩阵（A 类 7 列 + pi 接入）**：**pi 实连已通过**（官方脚本装 Pi 1.6.1；`~/.pi/agent/models.json` 片段由 `scripts/pi_models.py` 从应用库直读生成；一轮带工具调用的对话经真网关成功——模型出 `read` tool_call → pi 执行真实工具 → 结果回传；网关 2 次请求 + 2 条 `usage_log status=success estimated=0`；上游侧看到的是渠道密钥）。**矩阵侧执行器已建成**（`scripts/channel_matrix.py` 经网关跑 6 项，预设名单由 `cargo run --example presets` 导出），在 mock 上**经真网关 6 项全绿**。**A 类 7 列真实数据仍缺真实 API Key**（本机无任何 A 类渠道密钥，`channel` 表为空），未测单元格一律标「· 未测」 | 留档 `docs/reviews/T10-channel-matrix-and-pi.md`；矩阵产物（框架自测）`docs/reviews/channel-matrix-2026-11-26-mock.md`；02 §6.5/§10.1/§1.2（v0.9） |
| 🟡 | **T10 剩余项：A 类 7 列真实矩阵 + 预设 `stable`/`partial` 标注**。**头头 2026-11-26 决定「先不测真实上游」**，本项按**挂起**收口（非阻塞）：未测单元格一律标「· 未测」，**不猜测渠道能力**。解除条件 = 拿到任一 A 类渠道真实 API Key；到位后 `python scripts/channel_matrix.py --store "$env:APPDATA/llm-router/store.db"` 覆盖 `docs/reviews/channel-matrix-<日期>.md`，再按全绿/部分绿回填预设标注。**连带影响**：03 §1 的 M1 判据 ①「与 **DeepSeek** 完成一轮带工具调用的多轮对话」目前只有 **mock 上游**版本，真实 DeepSeek 版同样挂起 | 头头 2026-11-26 决定；执行器 `scripts/channel_matrix.py` 已就绪；留档 `docs/reviews/T10-channel-matrix-and-pi.md` §5 |
| ✅ | **T11 安全验收**（02 §10.2 四条判据全过）：① 库内明文（dump + 主库 + WAL/SHM 三层扫描，**明文凭证命中 0**）；② `vault.bin` 三重扫描 0 命中；③ 日志敏感键**全部脱敏为 `***`**（本轮补上了 §5.2 承诺但一直没接的 `gateway::logging::request_log` 中间件）；④ 绑定策略（无 client key ⇒ 拒绝启动且端口不监听；有 client key ⇒ 允许改绑）。四条判据固化为只读执行器 `scripts/security_check.py` | 留档 `docs/reviews/security-check-m1.md`；02 §10.2 执行口径 + §1.2（v0.10）；回归 `cargo test` 291 项全绿 |
| ✅ | **T12 本地估算器**（M2 第一张卡）：产线默认估算器由**字符启发式**换成 **tiktoken-rs**（`gateway::estimator::TiktokenEstimator`；编码表 crate 内嵌、运行期不联网）；编码选择 o200k 家族走 `o200k_base`、其余（含全部国产模型）走 `cl100k_base`；`is_approximation(model)` 把"国产模型只能近似"写成可查询事实。口径细化：**上游缺一项 → 只补缺的那一项**（"缺失"= 没有该键，值为 0 不算缺失），上游已给的精确值**一律不被覆盖**（R7），估算值一律 `estimated=1`、不参与预算告警。验收判据"无 usage 渠道强制走估算路径"**e2e + 真机双证通过** | 留档 `docs/reviews/T12-token-estimator.md`；02 §8.1 估算入口细化 + §13（v0.11）；回归 `cargo test` 299 项全绿；代价：二进制 24.2 → 32.2 MB |
| ✅ | **T13 价目表与成本换算**（M2 第二张卡）：新增 `store::price`（查表 / `*` 通配匹配 / **三价换算** / CRUD / 播种 / 改价后重算历史）与 `commands::price`（IPC 列/改/删/重算/试算）；`insert_usage` **落库时**算出 `cost_micros`；v8 迁移播种 **47 行**预填牌价（`deepseek`/`kimi`/`glm`/`dashscope`/`minimax_cn`/`siliconflow` 六家，**取自厂商官方页面**，快照 2026-11-26，`INSERT OR IGNORE` 不覆盖用户改过的价）。口径（02 §8.3）：缓存命中有独立档价**只计一次**（它已含在 `prompt_tokens` 里）、缓存写按输入价、**总额**四舍五入一次、**`0`（免费）与 `NULL`（未配置）必须可区分**、估算行照样算成本、改价后历史一并重算 | 留档 `docs/reviews/T13-price-and-cost.md`；02 §8.3 + §13（v0.13）；证据：**单测 12 条 + e2e 真网关全链（三价同时参与，`cost_micros == Some(108)`）+ 真机**（真实二进制 → schema v8 → 查库 47 行）+ **真实上游**（OpenRouter 免费模型真上游，真实 token 数 → `cost_micros` 与**独立复算**逐笔一致：536/64/491/509 micros）四证；回归 `cargo test` **321 项全绿**（e2e 6→7），0 失败 0 告警；**已知限制**：多档定价（分段/峰谷/priority）三价表达不了、`minimax_intl` 按 USD 计价不折算 → 不预填、OpenRouter 免费档 429 限流重；提交 `33dbe34`（真实上游实测夹具 `7c8d168` + 夹具假绿修复 `902af5d`，默认 ignore） |
| ✅ | **T14 聚合与图表 UI**（M2 收尾卡）：新增 `store::analytics`（四维聚合：日 / 渠道 / 模型 / 客户端，每桶 13 项指标——请求数 / 三态计数 / 五类 token / 总 token / **已知成本合计** / **成本未知行数** / 估算行数 / TTFT P50·P95 / 输出速度；分位用**最近秩法**；输出速度按耗时加权，**耗时为 0 → NULL**）+ `commands::usage`（`usage_aggregate` / `usage_recent` 两个 IPC）；新增 `src/usage/`（**手写 SVG** 条形图 + 用量统计面板：维度 / 时间窗 / 度量切换 + KPI + 聚合表 + 最近请求明细），**不引第三方图表库**（gzip 56.38 kB，包体不变）。口径细化（02 §8.4）：**「已知成本合计为 0」不等于「不花钱」**（未配牌价的行单独计数）、**合计桶不受桶数上限影响**、**可空列 COALESCE 成 (未记录)**、**聚合在后端、前端只做展示**（同源纪律） | 留档 `docs/reviews/T14-usage-aggregation.md`；02 §8.4 + §13（v0.14）；证据：**后端单测 17 条 + e2e 真网关全链 1 条**（流式+非流式 → 四维聚合逐项对账，速度按该行自己的耗时复算）+ **前端单测 10 条**（`node --test`，含「成本 NULL → 未知 ≠ ¥0」）+ **真机**（真实应用 → 真网关 → mock 上游 5 次请求 → 5 行流水 → 面板显示 tokens 229 / TTFT P50·P95 = 186·201 ms / 44.9 tok·s⁻¹ / 成本 ¥0.000221 元（另有 1 行未知）/ 来源「精确」；**四个维度逐一截图** `docs/reviews/app-ui-usage*.png`）四证；回归 `cargo test` **340 项全绿**（e2e 7→8），0 失败 0 告警，`npm run build` 通过；提交 `9820bb1`（后端 + 口径）+ `60fe9aa`（前端）+ `3251e62`（**顺带修掉一条既有随机失败断言**：client key 长度恒为 43 → 实为 42~43）+ `b598d63`（留档）；**已知限制**：**价目表编辑 UI 不在本卡**（任务卡只写聚合 + 图表，M2 判据只需成本可显示），另排一张卡 |
| ✅ | **T15 HAR 导入与模板化**（M3 第一张卡）：`checkin::har`（自写 HAR 1.2 子集解析，**响应体与响应头一律丢弃**）+ `store::checkin`（`checkin_site` 读写）+ `commands::checkin`（4 个 IPC）+ 前端 `src/checkin/` 导入向导四步。**密文边界**：导入视图敏感值一律 `***`（原值不出 Rust 进程）→ 保存只回传**路径 + 标记清单**、Rust 重新读盘（导入态不驻留内存）→ 敏感取值进保险库 `credential`（`kind=cookie`/`access_token`）→ **落库前自检模板不得残留原值**；**未标记的敏感字段自动补齐为 `auto_*`**（**Cookie 子项一律按密文**）；**归属渠道** = 方案 A（头头 2026-11-27 裁决）。证据：单测 24 + e2e 3（**Playwright 1.58.0 实抓** ModelScope 的真实 HAR 脱敏夹具）+ 前端单测 12 + **真机四证**（真实应用 + 真实抓包文件 + UI 自动化：标 2 个 → 落库 8 变量、`enabled=0`、**库/WAL/SHM/vault 四层明文命中 0**）。回归 `cargo test --all-targets` **367 项全绿** | 留档 `docs/reviews/T15-har-import-and-template.md`；提交 `2402fd9`（代码）、`9f0684b`（留档 + 文档）；截图 `app-ui-t15-checkin.png` / `app-ui-t15-fields.png`；02 §7.1.1/§7.1.2 + §13（v0.15）；**已知限制**：csrf 会轮换而模板只固化录制时的 Cookie（重录才更新，属 T16/T18）、base64 请求体不改写、JSON 路径断言属 T16、**未对真实站点发过签到请求**（属 T18） |
| ✅ | **T16 重放引擎**（M3 第二张卡）：新增 `checkin::replay`（`inject` 变量注入 → `ReplayEngine::replay` 重放 → `check_assertion` 断言 → `evaluate` + `refine_by_status` 分类）+ `store::checkin` 的 `checkin_result` 读写。**注入**：敏感变量按 `credential_id` 从保险库解密、非敏感用 `default`，替换在**值内部**；**缺凭据不发请求**（`credential_missing`）、**未知占位符拒绝重放**（`template_invalid`）。**构造**：**query 以模板数组为准重建**（HAR 的 url 与 queryString 重复，先剥再重建）、**剔除 `Host`/`Content-Length`**、连接 10s/整请求 30s、**引擎不重试**（属 T17）。**断言**：`assert.status` 是**期望状态码** + 新增可选 `assert.jsonPaths`（点号路径 + 数组下标），**旧模板兼容**。**分类 8 类**：`success`/`assert_failed`/`network_error`/`credential_missing`/`template_invalid`/`credential_invalid`(401·403)/`rate_limited`(429)/`http_error`，带「可重试」标记（沿用 02 §3.3）；**不猜验证码**。**落库**：`ok=1` 仅当 `success`；**响应体不进库**（内存留 512B 片段）。证据：单测 16 + **e2e 4**（用 T15 的真实产物打真 mock 上游）+ **真机验收工具** `cargo run --example checkin_replay_demo`（5 场景实测：成功/断言失败/401/连不上/缺凭据；**上游收到的是保险库真值**、缺凭据与网络失败时**上游请求数不增加**、**库+保险库明文命中 0**）四证。回归 `cargo test --all-targets` **388 项全绿** | 留档 `docs/reviews/T16-replay-engine.md`；提交 `67ef3c4`（代码）、`9a36505`（留档 + 文档）；02 §7.1.3 + §13（v0.16）；**已知限制**：导入模板带着**录制时的状态码**（ModelScope 匿名态 401 vs 成功 200）→ **用户必须自己改**（无断言编辑 UI，backlog）；csrf 不自刷新（重录才更新）；**无 UI 触发入口**（属 T17/T18）；**未对真实站点发过签到请求**（属 T18） |
| ✅ | **T17 调度器 + 系统兜底**（M3 第三张卡）：新增 `checkin::cron`（**经典 5 字段**归一化补秒位 + `last_due_at`/`next_due_at` + `CronShape` 六类形状）+ `checkin::scheduler`（`due_of` 去重判定 / `run_due_sites` / `run_ticker` 每分钟 tick / `run_now`）+ `checkin::system`（Windows `schtasks` 注册·查询·列表·注销 + macOS launchd plist）+ `checkin::cli`（`--checkin-once [--site <id>]`：**不起 UI、不起网关**，跑完退出）；`commands::checkin` 增 4 个 IPC（`checkin_set_schedule`/`checkin_run_now`/`checkin_results`/`checkin_system_sync`）；前端 `src/checkin/schedule.ts`（纯函数）+ 面板**调度区**（cron 输入 / 启用开关 / 立即执行 / 同步系统兜底 / 最近执行记录）。**调度**：每分钟 tick（对齐整分钟 +3s），`cron` crate 解析，**本地时区**解释；**去重**：`due_at` = 最近一次应触发时刻（≤ now），`last_attempt` = 最近一次尝试（**成败都算**）的 `checkin_result.ts`，`last_attempt >= due_at` → 跳过（**落库 ⇒ 跨重启成立**）；**失败不在同周期重打上游**；**补签**：应用启动 / tick / 计划任务唤起**三处同一套判定**；**系统兜底**：每个「启用 + 有 cron」站点注册**一个**计划任务 `LLM Router Checkin - <site_id>`（动作 `"<exe>" --checkin-once --site <site_id>`），cron → `/sc minute /mo N`（含 `*` → 每 1 分钟）/`hourly`/`daily`/`weekly`，**未识别表达式退回每天一次**。**真机揪出并修掉两个 bug**：① `* * * * *` 曾被归进"未识别 → 每天一次"→ **兜底漏跑**；② 注销靠**匹配 schtasks 错误文案**判"任务不存在"，而文案**随系统语言与有无控制台而变** → 漏判成失败（改成**先查后删**；顺带把系统命令输出改成按 **OEM 代码页**解码，不再乱码）。证据：单测 19 + **e2e 6**（含**真实 Windows 计划任务**注册·查询·注销）+ 前端单测 11 + **真机四证**（真实应用 UI 自动化 → 计划任务注册 → **关掉应用后 02:04:01 / 02:05:01 持续触发**（上游收到**保险库真值 cookie**、结果落库 `ok=1`、`Last Result: 0`）→ 补签 / 同周期去重 / 跨重启去重 / **四层明文命中 0**）。回归 `cargo test --all-targets` **414 项全绿** | 留档 `docs/reviews/T17-scheduler-and-system-fallback.md`；提交 `76329a0`（代码）、`0a6bf3c`（留档 + 文档）；截图 `app-ui-t17-schedule.png`；02 §7.2.1 + §13（v0.17 / **v0.18 真机修订**）；**已知限制**：**导入模板带录制时的状态码**（ModelScope 匿名态 401 vs 成功 200）→ 用户必须自己改（**无断言编辑 UI**，backlog，T18 带正确断言）；csrf 不自刷新（重录才更新）；**未对真实站点发过签到请求**（属 T18）；Windows 计划任务默认**「仅当用户登录时运行」**（注销则不触发）；**macOS launchd 分支未在真机验证**（本机 Windows） |
| ✅ | **M2 里程碑状态：判据全绿**（03 §1 的一条判据拆成 6 个子项，**逐条实测通过**：① 流式请求 ② tokens ③ TTFT ④ tokens·s⁻¹ ⑤ 估算成本 ⑥ `usage_log.estimated` 标记）。M2 三张卡 **T12 / T13 / T14** 全部完成并各自留档；T12 补上了"上游没回 usage → `estimated=1`"的反向证据，T13 把成本换算推进到**真实上游**实测，T14 提供呈现载体。**汇报件见 `docs/reviews/M2-milestone-report.md`**（M2 结论的唯一权威载体） | 03 §1 判据；T12 `docs/reviews/T12-token-estimator.md`、T13 `docs/reviews/T13-price-and-cost.md`、T14 `docs/reviews/T14-usage-aggregation.md`；真机四张截图 `docs/reviews/app-ui-usage*.png`；**未做项**：价目表编辑 UI（任务卡只写聚合 + 图表，M2 判据只需成本可显示）→ 另排一张卡；提交 `706007a` |
| ✅ | **M3 里程碑状态：判据全绿**（03 §1 的一条判据拆成 6 个子项，**逐条实测通过**：① 指定 cron 触发两个站点签到 ② 成功写 `checkin_result` ③ 成功通知 ④ 人为改坏模板后判失败并落库 ⑤ 失败通知含站点名与原因 ⑥ 系统计划任务在应用关闭时仍触发）。M3 五张卡 **T15 / T16 / T17 / T18 / T19** 全部完成并各自留档；**全程真站点 + 真凭据 + 真计划任务**，无手工数据——成功路径实测：真实 ModelScope **200 / 328 ms** → 落库 `id=20 ok=1` → webhook 收到「签到成功 · ModelScope（魔搭）」→ 系统通知库 `Id=15757`；失败路径实测：计划任务路径（**应用完全关闭**）→ 接收器同时收到「签到失败 … 分类：credential_invalid / 原因：…」→ **头头目视确认横幅**。**汇报件见 `docs/reviews/M3-milestone-report.md`**（M3 结论的唯一权威载体） | 03 §1 判据；T15 / T16 / T17 / T18 / T19 五份留档；真机常态：两站已启用、各挂一个 Windows 计划任务（每天 **08:20 / 09:00**）；**未验证项见 §5 第 7 项**（真实群机器人）+ §4.2 ①（ModelScope 差值 > 0 分支）；提交 `9e2e330`（T19 收尾）+ 本次（M3 汇报） |
| 🟡 | **M1 里程碑状态：判据 ②③④ 已验通过；① 的真实 DeepSeek 部分挂起**（头头决定先不测真实上游，与 T10 真实矩阵同一前提）。**汇报件见 `docs/reviews/M1-milestone-report.md`** | 03 §1 判据；解除条件 = 拿到任一 A 类渠道真实 API Key（一把钥匙同时解锁 T10 真实矩阵与 M1 判据 ①） |
| ✅ | **AGENT.md §9 安全验收正则口径**（头头 2026-11-26 裁决：改为指向执行器）。§9 保留四条判据语义，**执行统一走只读执行器** `scripts/security_check.py`，并写明"明文凭证命中必须为 0 / 结构性命中逐条列出"的分类口径；原命令三处与现实不符之处（正则恒命中 `api_key`、`strings` 是 Unix 工具、`grep logs/` 而应用不落日志文件）一并清掉 | 留档 `docs/reviews/security-check-m1.md` §4 + §4.1（含在**用户真实库**与**已录密钥的验证库**上的实跑留档，退出码 0）；02 §10.2 执行口径 |
| ✅ | **02 §6.1 标题"8 profile"** → 已按事实改为 **"7 profile"**（表体 7 行；T4/T8/UI/T8 任务卡均按 7 个落地，03 T8 卡片写的也是"7 个 profile"） | 头头 2026-11-26 裁决；02 §6.1 |
| ✅ | **分支命名**（头头 2026-11-26 裁决："main 指向 dev 最新并推远端，dev 也推"）：本地 `master` → **`main`**（指向 dev 最新 = 当前可发布状态），`dev` 一并推上远端并各自设上游；旧远端 `master` **已删除**；GitHub 仓库**默认分支改为 `main`**。现与 AGENT.md §8 的 `main`（可发布）+ `dev`（日常）**完全一致** | 实操记录：`git branch -m master main` → `git branch -f main dev` → `git push -u origin main` → `git push -u origin dev` → `gh api -X PATCH repos/benntqoo/llm-router -f default_branch=main`（GitHub **不允许删当前默认分支**，必须先切默认分支）→ `git push origin --delete master` |
| ✅ | **任务编号交叉引用对齐**（头头 2026-11-26 批准提案）：以 03 任务卡为唯一真值源，改 02 §6.1.1「T9/T10→T10」、§6.2「T17→T21」、§6.3 三处 + §11 风险表「T18a→T22」；另修提案未列的同源问题——§6.5 指向**不存在文件**的坏引用（`T10-pi-integration.md` → `T10-channel-matrix-and-pi.md`）、`presets/mod.rs` 注释里的旧编号。**代码行为零变更**（仅一条注释） | 提案与执行清单 `docs/reviews/proposal-task-numbering.md` §6；02 §13（v0.12）；提交 `3dab928` |
| ✅ | **统一错误体 `available_models` 字段回填 02 §4.4**：02 §2.1 ③ 要求"响应体附可用别名"但 §4.4 示例未列该字段。现补进 JSON 示例并写明**键的可选性**——只在**别名未命中**（`code = "model_not_found"`）时填充，缺失时**整个键不出现**（不出现 `null`）；别名命中但无可用候选的路径**不带**该字段（附了会误导客户端） | 代码事实：`gateway::errors::ErrorBody.available_models`（`skip_serializing_if`）+ `server.rs` 路由错误映射；02 §4.4 + §13（v0.12）；提交 `3dab928` |
| ✅ | **T18 签到站点清单已指定**（头头 2026-11-26 裁决）：**ModelScope（魔搭）+ MiniMax Code（网页版）**。两站覆盖 01 §1.1 模块 C 要求的两种签到形态——ModelScope 走平台**自己的公开接口**（`/api/v1/users/login/info` 触发签到 + `/openapi/v1/magicubes/transactions` 对账，魔粒可用于 API-Inference），MiniMax Code 走**浏览器自动化**（每日 400 积分）。按 AGENT.md §5.7（不做指纹伪造/验证码绕过/签名伪造）**已排除** Qoder CN（要 COSY 签名）与 Loomy（账号域 HMAC-SHA1，AK/SK 为客户端内置常量） | 03 §2 T18 卡片（含合规约束）；02 §13（v0.12）；提交 `3dab928` |
| ✅ | **T18 站点插件 ×2**（M3 第四张卡）：新增 `checkin::plugin`（`SitePlugin` trait + `builtin()` / `builtin_catalog()` + `parse_config()`）+ `checkin::reward`（`Reward{amount,unit,raw}`，**`raw` 只留数值与布尔**——字符串可能是 csrf/token 一类密文）+ `checkin::sidecar`（辅轨 **CLI 协议**客户端：起 Node 跑一个动作、**stdout 吐 JSON**、用完即退）+ `checkin::sites::modelscope`（**断言 JSON 业务字段 `Success`**）+ `checkin::sites::minimax_code`（走辅轨 + 登录交接）+ `store::account`（积分回写 `account.quota_json`，**合并写**：只改 `points`/`unit`/`source`/`updated_at`，**不动**限额字段）；`checkin::scheduler` 按 `plugin` **分派** + 写 `reward_json` + 回写账号；`commands::checkin` 增 **7 个 IPC**；前端「**内置站点插件**」区块 + 调度表「登录态」列 + `src/checkin/plugin.ts` 纯函数。**归属口径**：内置站点建站时建**占位渠道**（`provider='custom'`）→ 取 §8.2 默认账号 `acct_default_<channel_id>`（`credential.channel_id` / `account.channel_id` 都是 NOT NULL 外键）；**登录态明文只进保险库**，不回显、不进日志。**辅轨**：`sidecar/index.mjs`（`health`/`login`/`checkin`，**优先系统 Chrome**（`channel:"chrome"`，**不下载 150MB**）→ 回落自带 chromium，**持久化 profile**；**结果以页面自己的 `/signin/claim` 响应为准**；`AlreadyClaimed`/`claim_result=2` 判「今天已领」≠ 失败）。**真机揪出并修掉 8 个 bug**（第一轮 4 个 + 第二轮 4 个）：辅轨同名变量 TDZ / **插件没覆盖 trait 的 `default_schedule`**（MiniMax 默认 cron 错成 `0 9 * * *`，已加「清单 vs 插件一致性」回归单测）/ **`login` 动作在页面渲染前就误报「登录完成」**（假阳性）/ `checkin` 动作渲染前误判登录态（后两个的修法：等 `waitForSelector("button")` 再判，**不用 `networkidle`**——该站持续请求要等 20 s）。证据：Rust 单测 21 + **e2e 9**（mock 上游全链：**上游真的收到保险库里的 Cookie**、积分三处落点一致、**查不到魔粒时签到仍成功**、**没凭据时上游请求数 0**、**站点行里没有明文**）+ 前端单测 29 + **真机四证**（**ModelScope 打真实 `www.modelscope.cn`**：错误 Cookie → HTTP 401 → `credential_invalid`，152 ms；**MiniMax Code 走辅轨打真实 `agent.minimax.cn`**：Rust → Node → Playwright → Chrome 全链，`not_logged_in`，2741 ms；**四层扫描明文命中 0**；辅轨探活 chrome / Node v22.16.0）。**成功路径第二轮真机（头头给了 MiniMax Code 登录态）**：① **短信/验证码登录被站点频控**（点「获取验证码」报「操作频繁」）→ 辅轨 `login` 改为**自动打开登录弹窗 + 自动切「账号密码登录」**（**只铺路**：不代填密码、**不绕人机验证**），**登录成功**（硬信号：页面 API 请求 `user_id` 从 0 变真实 id），**登录态落进应用自己那份持久化 profile**（`%APPDATA%\llm-router\browser-profile`）；② **签到入口不在首页**（首页无任何「签到」文案），在**用户头像下拉**里的「每日签到」（`.ant-dropdown-trigger`，**页面上有两个，只认有文字的那个**）；③ **端到端真签到跑通**：应用点「立即执行」→ Rust → Node → Playwright → 系统 Chrome → 真实 `agent.minimax.cn` → UI 显示「签到成功（success）：MiniMax Code 今天已经领过了（连签第 7 天）」，**4307 ms**，落库 `checkin_result` id=9 `ok=1`；④ **每日积分口径实测对上任务卡**：`/signin/status` 的 `points` = **每日 400、第 4/7 天各 1000**，当天**连签第 7 天**；⑤ `status == 3` = 已领取，**「今天已领」判成功（幂等）**，已领过时报 `points = null`（本次无新增 → **不回写额度池**，避免重复计数）。**第二轮修掉的 bug**：签到在首页找文案必然找不到 / `extract_reward` 认不出 `days[]` 形状（`is_today`/`day_no`）/ 业务成功判据漏 `base_resp.status_code == 0`（AGENT.md §6 已知坑 1）/ 登录超时 5min → **15min**。回归 `cargo test --all-targets` **444 项全绿**（369 单元 + 75 e2e），4 ignored | 留档 `docs/reviews/T18-site-plugins.md`；提交 `9c98976`（代码）、`1a0c16b`（留档 + 文档）；截图 `app-ui-t18-plugins.png` / `app-ui-t18-sidecar-login.png` / **`app-ui-t18-signin-success.png`**；02 §7.4 + §13（v0.19 / **v0.20** / **v0.21** / **v0.22 真机修订**）；**已知限制**：**ModelScope 的真签到成功 + 魔粒字段路径待头头的网页会话 Cookie**；**MiniMax 的 `/signin/claim` 真正领取响应体未实测**（真机当天已领过）→ **积分回写对真实站点也还没实测**；辅轨依赖 Node + `sidecar/` 的 `npm install`（分发形态不在本卡）；**删站点不清内置站点的凭据**（backlog）；**探测脚本不许用临时 profile 反复打线上站点**（会被判成「新设备」并打频控——真机踩过） |
| ✅ | **T18 第三轮（2026-11-27）**：**ModelScope 成功路径已跑通**——登录态口径从「手抄 Cookie」改成「**访问令牌** `ms-…`」（头头反馈：「并不是所有人都会获取 cookies」）。**魔搭开放平台访问令牌可直接当网页会话用**（`Cookie: m_session_id=<令牌>` → `GET /api/v1/users/login/info` 即触发当日签到），且**同一令牌就是 `/openapi/v1/*` 的 Bearer** → **一个令牌，签到 + 魔粒全包**；判别规则（纯函数 + 单测）：含 `=` → 原始 Cookie 头（老路保留）/ 不含 → 访问令牌。**两条走不通的路已记档并撤销实现**：① 应用开浏览器自动登录 → 魔搭人机验证**专门拦自动化浏览器**（Chrome 挂「自动测试软件正在控制 Chrome」、滑块报 `验证失败（error:22phmt）` **根本没渲染**）+ §5.7 红线；② 浏览器接管会话 → 能读 Cookie 的浏览器都是被自动化控制的 → 辅轨 `session` 动作 / `run_secret` / IPC / 前端按钮**全部删除**。**真机**：访问令牌 → **HTTP 200**、`Success=true`、**277 ms** → 落库 `ok=1`（id=10）。**魔粒流水形状收敛**：`data.records[]`、**金额 `total_amount`**、**日期 `gmt_created`**、**类型 `type`**、**规则 `rule_key`**；当日 `daily_active` **200** + 两条资料类一次性奖励 50+50（**不算进签到奖励**）；**Cookie 形态也能开 `/openapi/v1/*`**。**奖励口径改「签到前后差值」**（站点不返回「今日魔粒 / 已领」信号；当日那条流水**整天都在** → 直接取当日总额会让同一天第 2、3 次运行**重复计数**）：> 0 = 本次到账（回写额度池）/ = 0 = **「今天已领过」**（`points=null`、**不回写**，与 MiniMax Code 一致）。**真机当天跑的是已领过分支**（`reward_json=NULL`）；**「差值 > 0」待次日首次运行**。回归 `cargo test --all-targets` **449 项全绿**（373 单元 + 76 e2e），4 ignored，0 失败 0 告警；前端 **44/44**；`npm run build` 通过（32 modules，gzip 62.78 kB） | 留档 `docs/reviews/T18-site-plugins.md` §4.2 ⑩；02 §7.4 §2/§5 + §13（v0.23 / **v0.24**）；03 §2 T18 卡片；提交 `273c355`（代码）、`88da56a`（文档） |
| ✅ | **T19 通知**（M3 第五张卡）：新增 `checkin::notify`（配置 / 文案 / 三种 webhook 请求体 / 系统通知 / 错误脱敏）+ `checkin::system_notify`（**Windows AUMID 注册**）+ `commands::notify`（**3 个 IPC**：`notify_get_config` / `notify_set_config` / `notify_test`）+ 前端「**结果通知（T19）**」区块（`src/checkin/notify.ts` 纯函数 + 5 条单测）。**落点** = `scheduler::run_site` 之后（**结果已落库才通知**）→ 一处覆盖**四条路径**（应用 tick / UI 立即执行 / CLI / **Windows 计划任务，应用关闭时**）；**失败永远通知**（§5.7「撞到验证码 = 失败并通知」）、成功可关（`notify_success`）；**通知失败不影响签到结果**（只记日志、不重试、不回滚）。**通道决策**：系统通知用 **`notify-rust`**——**刻意不用 Tauri 通知插件**（CLI 与计划任务路径**没有 app handle**，用插件覆盖不了「应用关闭时也通知」）。**密文边界**：webhook 地址与 bot token 进**保险库**（`kind='webhook'`，挂占位渠道「通知」），`app_setting` 只存**凭据 id**；IPC 只回「配没配」；reqwest 错误一律归一成**不含地址**的一句话。**真机四证**：① 本地接收器收到**企微形状**原文（**站点名 + 分类 + 原因**、**零密文**）② **计划任务路径真机送达**（`schtasks /run` → 接收器同时收到通知体 + `checkin_result` id=19；`Last Result = 1` = **签到失败**，不是任务挂了——T17 退出码契约 0/1/2）③ **真机全库四层扫描**（webhook 明文做 `--secret`）→ **明文凭证命中 0 条、退出码 0** ④ AUMID 注册落地（开始菜单 `LLM Router.lnk`）。**真机揪出并修掉 3 个问题**：① toast「已发送」但**屏幕什么都没有**——根因是**非 powershell 进程冒用 PowerShell 的 AUMID 会被 OS 静默丢弃**（API 却返回成功）→ 自建 AUMID + COM 注册开始菜单快捷方式 ② reqwest 错误**带出地址** → 统一脱敏 ③ **Windows 全局通知总开关关着时，所有应用的 toast 都被静默丢弃**（`PushNotifications\ToastEnabled = 0` → 事件日志 `Id=3150 … PolicyReason [GlobalSettingDisabled]`；**别的应用同样中招** → 「PowerShell 原生 toast 也不显示」是同一原因）→ 发送前读开关、**如实报失败**（`global_toasts_enabled` + 纯函数 `global_block_reason` + 2 条单测）；另发现**改完开关必须重启 `WpnUserService_<sid>`**（服务缓存策略）。回归 `cargo test --all-targets` **460 项全绿**（0 失败 / 4 ignored / 0 编译告警）；前端 **49/49**；`npm run build` 通过（33 modules，gzip 64.24 kB） | 留档 `docs/reviews/T19-notification.md`；02 §7.5（+ §7.5 §6 静默丢弃口径）+ §13（v0.25 / **v0.26**）；03 §2 T19 卡片；**横幅已目视确认**；**只剩一项未验证见 §5 第 7 项**（真实群机器人）；提交 `464f42c`（口径与任务卡）、`4b6f0ee`（代码）、`581de44`（留档 + 账本） |
| ⏳ | HANDOFF §5 未验证项 | 验证方法见 §5；命中对应任务前必须先实测再实现（含已划掉的项，表格即真值） |

> 未列入本表的待办视为丢失：任何新待办必须入表。

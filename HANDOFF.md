# HANDOFF — 开发工作交接

| 项 | 值 |
|---|---|
| 交接日期 | 2026-11-25 |
| 交接对象 | 任意接手实际开发的 agent 会话（MiniMax Code / 其他） |
| 项目阶段 | M1 开发中；进度账本见 §8 |
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
第 16 步：~~T12 本地估算器~~ ✅（产线默认换成 tiktoken-rs；"无 usage 走估算路径"e2e + 真机双证；留档 `docs/reviews/T12-token-estimator.md`）；下一步 T13 价目表与成本换算 → T14 聚合与图表 UI
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
| 🟡 | **M1 里程碑状态：判据 ②③④ 已验通过；① 的真实 DeepSeek 部分挂起**（头头决定先不测真实上游，与 T10 真实矩阵同一前提）。**汇报件见 `docs/reviews/M1-milestone-report.md`** | 03 §1 判据；解除条件 = 拿到任一 A 类渠道真实 API Key（一把钥匙同时解锁 T10 真实矩阵与 M1 判据 ①） |
| ✅ | **AGENT.md §9 安全验收正则口径**（头头 2026-11-26 裁决：改为指向执行器）。§9 保留四条判据语义，**执行统一走只读执行器** `scripts/security_check.py`，并写明"明文凭证命中必须为 0 / 结构性命中逐条列出"的分类口径；原命令三处与现实不符之处（正则恒命中 `api_key`、`strings` 是 Unix 工具、`grep logs/` 而应用不落日志文件）一并清掉 | 留档 `docs/reviews/security-check-m1.md` §4 + §4.1（含在**用户真实库**与**已录密钥的验证库**上的实跑留档，退出码 0）；02 §10.2 执行口径 |
| ✅ | **02 §6.1 标题"8 profile"** → 已按事实改为 **"7 profile"**（表体 7 行；T4/T8/UI/T8 任务卡均按 7 个落地，03 T8 卡片写的也是"7 个 profile"） | 头头 2026-11-26 裁决；02 §6.1 |
| ✅ | **分支命名**（头头 2026-11-26 裁决："main 指向 dev 最新并推远端，dev 也推"）：本地 `master` → **`main`**（指向 dev 最新 = 当前可发布状态），`dev` 一并推上远端并各自设上游；旧远端 `master` **已删除**；GitHub 仓库**默认分支改为 `main`**。现与 AGENT.md §8 的 `main`（可发布）+ `dev`（日常）**完全一致** | 实操记录：`git branch -m master main` → `git branch -f main dev` → `git push -u origin main` → `git push -u origin dev` → `gh api -X PATCH repos/benntqoo/llm-router -f default_branch=main`（GitHub **不允许删当前默认分支**，必须先切默认分支）→ `git push origin --delete master` |
| 🔴 | 任务编号交叉引用对齐（02 的 `T18a`/`T17`/`T9` 与 03 任务卡不一致） | 提案 `docs/reviews/proposal-task-numbering.md`，待头头裁决 |
| 🔴 | 统一错误体新增 `available_models` 字段是否回填 02 §4.4 | 02 §2.1 ③ 要求"响应体附可用别名"，§4.4 字段表未列；已按可选字段实现，见 `docs/reviews/T5-routing-alias-verification.md` §6 |
| 🔴 | T18 签到站点清单待指定 | `docs/planning/03-delivery-plan.md` §2 T18 |
| ⏳ | HANDOFF §5 五项未验证项 | 验证方法见 §5；命中对应任务前必须先实测再实现 |

> 未列入本表的待办视为丢失：任何新待办必须入表。

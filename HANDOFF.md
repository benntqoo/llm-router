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
第 7 步：~~T6 流式管道 + fallback + 熔断~~ ✅（提交 `ad50996`，验收留档 `docs/reviews/T6-stream-fallback-circuit-verification.md`）；下一步 T7 usage 采集（关键路径 T7→T10）
第 8 步：每任务完成 → 验收留档 docs/reviews/<任务号>-*.md
第 9 步：M1 全绿（03 §1 判据）→ 向头头汇报，再进 M2
```

排期基准 6 周（03 §4）。滞后处理按 03 §5 降级预案，不擅自砍范围。

## 5. 未验证项（开工会撞到，先测再做）

| # | 项 | 影响任务 | 验证方法 |
|---|---|---|---|
| 1 | **WorkBuddy 上游 API host 与登录态字段**。本机已实测：登录态在 `~/.workbuddy/app/users/<64hex>/`，设备标识 `~/.workbuddy/device-id`（UUID 可读写）；内部字段与 API host 未知 | T22（抓包）→ T23 | 代理抓 WorkBuddy 桌面端一次对话；产出 `docs/reviews/workbuddy-protocol.md`（**只记字段名，严禁记录 token 值**）。发现签名/加密 → 触发降级（仅保活+额度查看） |
| 2 | **MiniMax 国内站 GroupId**：第三方口径分裂（有带/不带）。官方国际站确认不带 | T8/T10 | 三种口径逐一实测（不带/带 GroupId query/旧版路径），结论回填 02 §4.2 预设 |
| 3 | **pi CLI 的自定义 provider 配置格式**。pi-ai 库已验证支持 OpenAI 兼容（`createProvider`），但 CLI 层 JSON schema 未见文档 | T10 | 装 pi 实测；产出可复制的接入配置片段，回填 02 §6.5 |
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
| 🔴 | **AGENT.md §9 安全验收正则口径**：`grep -iE "sk-|Bearer |api_key"` 会恒定命中 DDL 注释与 `credential.kind` 取值，需收窄口径 | 提案与三个备选方案见 `docs/reviews/T2-vault-verification.md` §6，待头头裁决 |
| 🔴 | 任务编号交叉引用对齐（02 的 `T18a`/`T17`/`T9` 与 03 任务卡不一致） | 提案 `docs/reviews/proposal-task-numbering.md`，待头头裁决 |
| 🔴 | 02 §6.1 标题写"8 profile"但表体只有 7 行（03 T8 写"7 个 profile"） | T4 已按 7 个落地；是否回填标题待裁决，见 `docs/reviews/T4-outbound-adapter-verification.md` §6 |
| 🔴 | 统一错误体新增 `available_models` 字段是否回填 02 §4.4 | 02 §2.1 ③ 要求"响应体附可用别名"，§4.4 字段表未列；已按可选字段实现，见 `docs/reviews/T5-routing-alias-verification.md` §6 |
| 🔴 | **限流冷却期内回 429 + `Retry-After` 响应头**是否回填 02 §4.4 | §3.3 把 429 单列一类，§2.3 只写"429 触发连接冷却"，未写冷却期内回什么；已实现为 429 + 响应头（错误体字段表未改），见 `docs/reviews/T6-stream-fallback-circuit-verification.md` §6 |
| 🔴 | 熔断/冷却状态是否需要落库与 UI 展示 | 02 §2.3 明确"只存内存、重启清零"，当前按此实现；若要在 UI 显示"某渠道正在冷却"需额外设计 |
| 🔴 | 分支命名：仓库实为 `master` + `origin/master`，AGENT.md §8 要求 `main` + `dev`（已建 `dev`） | 待头头裁决是否重命名 `master` → `main` |
| 🔴 | T18 签到站点清单待指定 | `docs/planning/03-delivery-plan.md` §2 T18 |
| ⏳ | HANDOFF §5 五项未验证项 | 验证方法见 §5；命中对应任务前必须先实测再实现 |

> 未列入本表的待办视为丢失：任何新待办必须入表。

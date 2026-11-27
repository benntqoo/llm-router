# LLM Router — 开发计划与任务拆解（v1）

| 项 | 值 |
|---|---|
| 版本 | v0.1（草案） |
| 日期 | 2026-11-25 |
| 上游文档 | `02-technical-design.md`（技术设计）、`01-architecture-and-feasibility.md`（可行性） |
| 执行方式 | 头头本人用 MiniMax Code 开发；小满只做规划与审查。**本文档未改动任何代码或配置**。 |

---

## 1. 里程碑总览

```
M1 可用内核          M2 可观测           M3 可签到            M4 可反代
(网关+保险库+A类)    (用量+成本)          (HAR+调度+2站点)      (Copilot+WorkBuddy)
────────────────►  ────────────────►  ──────────────────►  ──────────────────►
  约 4–5 周           约 1 周              约 2 周               约 2.5–3.5 周
       │                                                             │
       └── M1 完成即可日常自用，后续里程碑并行推进 ──────────────────────┘
```

| 里程碑 | 验收判据（可执行） |
|---|---|
| **M1** | ① pi 指向 `http://127.0.0.1:<port>/v1`，携带 client key，与 DeepSeek 完成一轮带工具调用的多轮对话；② `sqlite3 store.db ".dump" | grep -iE "sk-|Bearer "` 无明文命中；③ 重启应用后渠道与密钥仍可用；④ 别名冲突时 `/v1/models` 列表无重复 |
| **M2** | 一次流式请求后 UI 显示 tokens / TTFT / tokens·s⁻¹ / 估算成本，且 `usage_log.estimated` 标记正确（上游有 usage 时为 0） |
| **M3** | 指定 cron 触发两个站点签到：成功写 `checkin_result` 并通知；人为改坏模板后失败通知含原因；系统计划任务在应用关闭时仍触发 |
| **M4** | ① Copilot OAuth 全流程走通，30min 后自动刷新 token，流式对话经网关返回；② WorkBuddy 登录态可作为上游完成对话；③ 两渠道默认关闭，首次启用有 ToS 确认弹窗 |

M4 中 WorkBuddy 存在协议不确定性（T22 抓包），允许降级交付：仅"保活 + 额度查看"，转发降为 v2（见 §5 风险缓冲）。

---

## 2. 任务拆解

> 卡片格式：**目标 / 产出 / 验收 / 依赖**。文件路径以 `02-technical-design.md` §1.2 目录结构为准。

### M1 — 可用内核

**T0 仓库初始化** `0.5d` · 依赖：无
- 目标：工程可跑。git init、Tauri 2 + React + TS 脚手架、axum 内嵌服务 hello world、`cargo test` / `npm run tauri dev` 双通。
- 产出：`src-tauri/`、`src/` 骨架；CI 留空。
- 验收：`npm run tauri dev` 打开窗口，`curl 127.0.0.1:<port>/healthz` 返回 200。

**T1 数据层** `1d` · 依赖 T0
- 目标：SQLite WAL + migration 框架 + 全部建表（02 §9 与 01 §4.4 的 DDL 全量）。
- 验收：`cargo test` 覆盖 migrate up/down；空库启动自动建表。

**T2 加密保险库** `2d` · 依赖 T1
- 目标：02 §5 全量。DPAPI/Keychain 主密钥、AES-256-GCM、Argon2id 可选模式、IPC 脱敏、回看命令。
- 验收：写入一条假 key 后 `strings vault.bin` 无明文；重启可解密；30s 自动遮蔽生效。

**T3 网关服务骨架 + 入站鉴权** `1.5d` · 依赖 T1
- 目标：axum 服务、`/v1/chat/completions`、`/v1/models`、client key 校验（SHA-256）、统一错误结构（02 §4.4）、Host 校验、限流。
- 验收：无 key 返回 401 统一结构；`/v1/models` 返回空别名列表。

**T4 出站适配器** `2d` · 依赖 T2
- 目标：`ProviderAdapter` trait + `openai_compat.rs` 通用实现（URL/头/body.model 改写，R-1~R-8 全部规则）+ 预设数据结构。
- 验收：单测覆盖每个改写规则（R-1/R-3/R-5/R-7 必测）；MiniMax 出站 URL 按 §4.2 口径拼接。

**T5 路由与别名** `1d` · 依赖 T3
- 目标：alias 唯一索引、route_group 四策略（priority/round_robin/cost/**sticky_session**）、未命中返回可用别名。
- 验收：route_group 命中/单别名命中/未命中三条路径单测通过；sticky_session 对同一 client_key+模型连续请求返回同渠道（多账号池场景）。

**T6 流式管道 + fallback + 熔断** `3d` · 依赖 T4 T5
- 目标：SSE 零缓冲直通、首字节前重试、客户端断连 abort 上游、熔断状态机、P-4 学习式兼容、P-7 `X-Routed-Via` 响应头注入、429 响应 `Retry-After`/`x-ratelimit-*` 解析进冷却参数。
- 验收：mock 上游可复现"首字节后失败不重试""断连取消上游""3 连败熔断""429 冷却时长遵循 Retry-After"四个场景（用内存 mock server 测试）。
- 追加（2026-11-26 头头裁决）：熔断状态**落库 + 重启恢复**（新增 `circuit_state` 表，02 §2.3/§9）+ 应用内 UI 展示（`circuit_status`）；限流冷却期内回 **429 + `Retry-After`**（02 §4.4）；熔断**时长可配置**（默认 60s、最小 1s，新增 `app_setting` 表）+ **手动解除熔断**。
- 追加二（2026-11-26 头头裁决）：**429 冷却上限可配置**（默认 60s、最小 1s，`app_setting.rate_limit_cooldown_cap_secs`）——冷却 = `min(max(指数退避, 上游 Retry-After), 上限)`，退避封顶同用该上限；回给客户端的 `Retry-After` 是**实际生效**的冷却秒数。

**T7 usage 采集** `1.5d` · 依赖 T6
- 目标：tee 解析 usage 帧、R-4 注入、estimated 估算入口（估算器本身在 T12）、`usage_log` 落库、per `(channel, account)` 的 RPM/RPD/TPM/TPD 计数与 `quota_json` 回写（限额自学习，02 §8）。
- 验收：流式/非流式各一条用例，usage_log 字段与上游返回一致；429 触发后 quota_json 记录更新。
- 落地（2026-11-26）：一条 `usage_log` = **一次出站尝试**（fallback 试 N 个候选 = N 行；未到达任何候选的失败不写）；流式走**旁路 tee**（转发字节的同时在副本上扫 SSE 帧找 usage 帧、记 TTFT，不缓冲全量），非流式直读响应体 `usage`；上游没回 usage 才走估算入口（`estimated=1`，失败行不估 token；估算器本体在 T12）；`usage_log` 增 `account_id`（schema v7，幂等补列），API Key 型渠道按需补建默认账号 `acct_default_<channel_id>`；四个窗口（RPM/TPM 近 60s、RPD/TPD 按 UTC 日界）从 `usage_log` 派生；429 的 `x-ratelimit-limit-*` 学进 `account.quota_json`（**只收紧不放宽**，`last_rate_limit` 记上游报的事实）。口径见 02 §8.1/§8.2（v0.6），验收留档 `docs/reviews/T7-usage-capture.md`。

**T8 A 类预设 + 自定义渠道** `1.5d` · 依赖 T4
- 目标：02 §6.1 的 7 个 profile 数据 + "自定义 OpenAI 兼容渠道"表单（base_url/路径模板/鉴权样式/group_id）。
- 验收：预设渠道选中后仅剩"填 key"一步；自定义渠道可保存并出现在 `/v1/models`。
- 落地（2026-11-26）：7 个 profile 全部接入 UI（DeepSeek/Kimi/GLM/MiniMax 国际/MiniMax 国内/百炼/硅基流动），每个预设带一条**默认模型建议值**（公开常识值，**未实测、可改**）→ 选预设即预填 base_url / 路径模板 / 鉴权样式 / include_usage 注入，**只剩填 key**；自定义渠道表单字段落到 `channel` 行 + `channel.meta_json`（base_url / 路径模板 / 鉴权样式 `bearer`｜`custom_header` / GroupId / include_usage 开关 / 额外固定头 / 模型别名），保存后别名**立即**出现在 `GET /v1/models`；密钥不进渠道行，一律走保险库；表单里删掉的模型**置 disabled 不物理删除**（保 `route_member` 编排）；`meta_json` 走**整份替换**语义，字段 `null` 统一解释为"不覆盖"。口径见 02 §6.1.1/§6.1.2/§9（v0.7），验收留档 `docs/reviews/T8-preset-and-custom-channel.md`。

**T9 UI 第一轮** `3d` · 依赖 T1–T8（可部分并行）
- 目标：渠道列表/编辑、密钥录入与回看（30s 遮蔽）、模型别名管理、client key 管理、`/v1/models` 预览。
- 验收：从零配置一个 DeepSeek 渠道 + client key ≤ 3 分钟。
- 落地（2026-11-26）：渠道面板支持**模型别名多行管理**（增删行 / 逐条启用）、渠道启用开关与优先级；保险库面板的密钥录入改为**下拉选已有渠道**（按 `channel_id` 挂载，不再按名字兜底建占位渠道），回看维持 **30s 自动遮蔽**；client key 支持**标签与限额**（rpm / 每日 tokens）编辑；新增「模型预览」面板——**与网关 `GET /v1/models` 同源**（同一条查询，另写一份必然漂移）+ 客户端接入片段；跨面板数据变更用**事件总线 + 5s 轮询兜底**广播，避免"渠道说密钥已录入、保险库说 0 条凭据"的自相矛盾；02 → v0.8（§6.5 注明 pi 实连归 T10、新增 §6.6 UI 面板口径）。**验收判据实测：从零配置 DeepSeek 渠道 + client key = 7.1 秒**（脚本驱动，含自动化等待；人工路径仅 1 次粘贴 + 3 次点击 + 1 次滚动，上限 3 分钟）；验收留档 `docs/reviews/T9-ui-first-round.md`。

**T10 实测矩阵（A 类 7 列 + pi 接入）** `1.5d` · 依赖 T9
- 目标：跑 02 §10.1 矩阵前七列；用 pi 实连并产出接入文档片段（02 §6.5 补全）。
- 验收：矩阵填写完整并落库到 `docs/reviews/channel-matrix-*.md`；pi 一轮带工具调用对话成功。
- 落地（2026-11-26）：**pi 实连通过**——官方 `pi.dev/install.ps1` 装 managed Pi **1.6.1**（系统 Node v22.16.0 不满足 ≥22.19.0，用**脚本自带的** `Install-NodeStandalone` 装独立 Node v22.23.5，**不动用户 Node 环境**）；`scripts/pi_models.py` 从应用库直读别名生成 `~/.pi/agent/models.json`（`apiKey` 写 `$LLMR_CLIENT_KEY`，**不留明文**），`pi --list-models llm-router` 正常列出别名；**一轮带工具调用的对话经真网关成功**（模型出 `read` tool_call → pi 执行真实工具 → 结果回传 → 模型收尾；网关 **2 次请求**、**2 条 `usage_log`** `status=success estimated=0`；上游侧看到的是**渠道密钥**而非 client key）。02 §6.5 按 pi 自带 schema 与官方文档**重写**（不再是"未验证"占位）。矩阵侧：`scripts/channel_matrix.py` 执行器建成（经网关跑 6 项，预设名单由 `cargo run --example presets` 导出，**不在 Python 侧抄**），在本地 mock 上**经真网关 6 项全绿**，产物 `docs/reviews/channel-matrix-2026-11-26-mock.md`。**A 类 7 列真实矩阵仍待真实 API Key**（本机无任何 A 类渠道密钥，`channel` 表为空），未测单元格一律标「· 未测」，**不猜测渠道能力**。验收留档 `docs/reviews/T10-channel-matrix-and-pi.md`。（**2026-11-26 头头决定：先不测真实上游**——该部分按**挂起**收口，非阻塞；连带 M1 判据 ① 的「真实 DeepSeek 对话」同样挂起，解除条件 = 拿到任一 A 类渠道真实 key。）

**T11 安全验收** `0.5d` · 依赖 T2 T3
- 目标：02 §10.2 四条判据全过。
- 验收：判据记录进 `docs/reviews/security-check-m1.md`。
- 落地（2026-11-26）：**四条判据全过**。① 库内明文：逻辑 dump（199 行）+ `store.db` 主库 + **`-wal`/`-shm` 原始字节**三层扫描 → **明文凭证命中 0 条**（结构性命中 26 条全是 DDL 注释与 `kind='api_key'` 枚举值）；被测库是**已实际录过密钥**的库（1 渠道 + 1 凭据 + 1 client key + 53 条 usage_log），不是空库。② `vault.bin`（596 B）原始字节 + 十六进制视图 + 可打印片段三重扫描 → 0 命中。③ 日志：3 条含敏感键的日志行**全部** `"authorization":"***"`，两个已知明文凭证都不在 stdout/stderr；401 请求不打上游。④ 绑定策略：`0.0.0.0` + 无 client key ⇒ **进程退出 + 端口不监听 + 明确告警**；反向用例（有 client key）正常启动。**过程中补齐一处规范缺口**：02 §5.2 承诺的「日志中间件全局脱敏」**此前从未接入**（T2 §7 记着"随 T3 接入"，T3 已过但一直没接，`vault::redact` 只被自己的单测调用），本轮新增 `gateway::logging::request_log`（敏感键值一律 `***`、请求体不落日志）并挂到 `gateway::server::router()`。四条判据固化为**只读**执行器 `scripts/security_check.py`（命中项分"明文凭证命中 / 结构性命中"两类，对应 T2 §6 提案 1+3；**AGENT.md §9 原文未改**，正式口径仍待裁决）。回归 `cargo test` → **291 项全绿**（新增 3 条脱敏单测），0 失败 0 告警。02 → v0.10（§10.2 补执行口径、§1.2 补脚本）。验收留档 `docs/reviews/security-check-m1.md`。

> M1 合计 ≈ 18.5d（约 4 周，含缓冲）。

### M2 — 可观测

**T12 本地估算器** `1d` · 依赖 T7
- tiktoken-rs 包装；国产模型标 estimated=1。验收：无 usage 渠道强制走估算路径。
- 落地（2026-11-26）：**验收通过**（e2e + 真机双证）。新增 `gateway::estimator::TiktokenEstimator`（**tiktoken-rs 0.12.1**，编码表 crate 内嵌、运行期不联网），**产线默认估算器**由字符启发式（字符数/4）换成 tiktoken（`CharRatioEstimator` 保留为轻量回退与单测基准）。编码选择：**o200k 家族**（gpt-4o/gpt-4.1/gpt-5/o1/o3/o4/gpt-oss）走 `o200k_base`，**其余全部**（含全部国产模型）走 `cl100k_base`；`is_approximation(model)` 把「国产模型只能近似」写成可查询事实（供 T13 成本换算与 T14 UI 取用），估算值**一律 `estimated=1`**、不参与预算告警（R7），上游已给的精确值**一律不被覆盖**。口径细化回填 02 §8.1（v0.11）：估算入口分**四种情形**——上游给全（原样用，`estimated=0`）/ 上游没回（全估，`estimated=1`）/ 上游**缺一项**（**只补缺的那一项**，另一项仍用上游精确值，`estimated=1`；"缺失"= usage 对象里没有该键，**值为 0 不算缺失**）/ 出站失败（不估 token）。e2e：升级"无 usage 走估算路径"用例（落库数**精确等于** tiktoken 计数，且**不等于**字符启发式计数）+ 新增"上游缺一项只补缺失侧"用例；既有 5 条行为不变。**真机**：应用实跑 → 真网关 → **不返 usage** 的 mock 上游 ⇒ 落库 `prompt_tokens=21 completion_tokens=12 estimated=1 status=success`（同提示词字符启发式只会给 ~8 ⇒ 确实走的是 tiktoken），而上游给了 usage 的行仍是 `estimated=0`。回归 `cargo test` → **299 项全绿**（新增 7 单测 + 1 e2e），0 失败 0 告警。代价：调试版二进制 24.2 MB → **32.2 MB**（内嵌 BPE 表）。验收留档 `docs/reviews/T12-token-estimator.md`。

**T13 价目表与成本换算** `1d` · 依赖 T7
- `price` 表 + 预填国产牌价 + `cost_micros` 换算。验收：单测覆盖"输入+输出+缓存读"三价换算。
- 落地（2026-11-26）：**验收通过**。新增 `store::price`（查表 / `*` 通配匹配 / 三价换算 / CRUD / 播种 / 改价后重算历史）与 `commands::price`（IPC 列/改/删/重算/试算）；`insert_usage` **落库时**算出 `cost_micros`（**没命中价目 → NULL（未知），不是 0**）；v8 迁移 `INSERT OR IGNORE` 播种 **47 行**预填牌价（覆盖 `deepseek`/`kimi`/`glm`/`dashscope`/`minimax_cn`/`siliconflow` 六家，**取自厂商官方页面**，快照 2026-11-26，不覆盖用户改过的价）。口径细化回填 02 §8.3（v0.13）：缓存命中有独立档价**只计一次**（它已含在 `prompt_tokens` 里）、缓存写按输入价、**总额**四舍五入一次、`0`（免费）与 `NULL`（未配置）必须可区分、估算行照样算成本、改价后历史一并重算。证据：**单测 12 条 + e2e 真网关全链 1 条**（三价同时参与，`cost_micros == Some(108)`）+ **真机**（真实二进制启动 → schema v8 → 查库 47 行价目）+ **真实上游**（OpenRouter 免费模型真上游，真实 token 数 → `cost_micros` 与**独立复算**逐笔一致：536 / 64 / 491 / 509 micros）四证。回归 `cargo test` → **321 项全绿**（e2e 6→7），0 失败 0 告警。验收留档 `docs/reviews/T13-price-and-cost.md`。**已知限制**：分段/峰谷/priority 等多档价无法用"每模型一组三价"表达；`minimax_intl` 按 USD 计价不做汇率折算 → 不预填；OpenRouter 免费档限流重（429），真实上游实测要按"择期可复现"看。

**T14 聚合与图表 UI** `3d` · 依赖 T12 T13
- 四维聚合 SQL + React 图表（按天/渠道/模型/客户端）。验收：M2 判据。
- 落地（2026-11-26）：**验收通过**。新增 `store::analytics`（四维聚合：日 / 渠道 / 模型 / 客户端，每桶 13 项指标——请求数 / 三态计数 / 五类 token / 总 token / **已知成本合计** / **成本未知行数** / 估算行数 / TTFT P50·P95 / 输出速度；分位用**最近秩法**；输出速度按耗时加权，**耗时为 0 → NULL**）+ `commands::usage`（`usage_aggregate` / `usage_recent` 两个 IPC）；新增 `src/usage/`（手写 **SVG** 条形图 + 用量统计面板：维度 / 时间窗 / 度量切换 + KPI + 聚合表 + 最近请求明细），**不引第三方图表库**（包体不变，gzip 56.38 kB）。口径细化回填 02 §8.4（v0.14）：**「已知成本合计为 0」不等于「不花钱」**（未配牌价的行单独计数）、**合计桶不受桶数上限影响**、**可空列 COALESCE 成 (未记录)**。证据：**后端单测 17 条 + e2e 真网关全链 1 条**（流式+非流式 → 四维聚合逐项对账，速度按该行自己的耗时复算）+ **前端单测 10 条**（`node --test`，含「成本 NULL → 未知 ≠ ¥0」）+ **真机**（真实应用 → 真网关 → mock 上游 5 次请求 → 5 行流水 → 面板显示 tokens 229 / TTFT P50·P95 = 186·201 ms / 44.9 tok·s⁻¹ / 成本 ¥0.000221 元（另有 1 行未知）/ 来源「精确」；**四个维度逐一截图**）四证。回归 `cargo test` → **340 项全绿**（e2e 7→8），0 失败 0 告警；`npm run build` 通过。验收留档 `docs/reviews/T14-usage-aggregation.md`。**顺带修掉一条既有随机失败断言**（client key 长度恒为 43 → 实为 42~43，概率约 1.2% 随机失败）。**已知限制**：**价目表编辑 UI 不在本卡**（任务卡只写聚合 + 图表；M2 判据只需成本可显示），另排一张卡。

> M2 合计 ≈ 5d。可与 M3 并行。

### M3 — 可签到

**T15 HAR 导入与模板化** `2.5d` · 依赖 T1
- HAR 1.2 解析、变量标记向导、模板存储。验收：导入一个真实 HAR 并标记 2 个变量成功。
- 落地（2026-11-27）：**验收通过**。新增 `checkin::har`（**自写 HAR 1.2 子集解析**：只取 request 的 method/url/headers/queryString/postData 与 response.status，**响应体与响应头一律丢弃**——实测签到响应的 `Set-Cookie` 带 csrf 密文）+ `store::checkin`（`checkin_site` 读写）+ `commands::checkin`（4 个 IPC：导入/保存/列/删，各带可测 `_of` 版本）+ 前端 `src/checkin/`（导入向导四步）。**密文边界**（AGENT.md §5.3）：导入视图里敏感值一律 `***`（**响应密文与请求原值都不出 Rust 进程**）→ 保存时前端只回传**路径 + 标记清单**、Rust 重新读盘解析（导入态不驻留内存）→ 敏感取值写保险库 `credential`（`kind = cookie` / `access_token`）、模板里只留占位符 → **落库前自检模板不得残留被标记字段原值**（命中即拒绝保存）。**未标记的敏感字段自动补齐为 `auto_*` 变量**（留在模板里=明文落库，丢掉=重放必失效）；**Cookie 子项一律按密文处理**（实测 `cna`/`ssxmod_itna` 这类名字不含关键词的子项同样是会话态）。**归属渠道**（头头 2026-11-27 裁决方案 A）：标站点的凭据挂在一个渠道上（选既有或按站点名新建 `provider=custom`）。证据：**单测 24 条**（解析/掩码/零明文/重录替换/删站点连带清凭据/渠道解析）+ **e2e 3 条**（**Playwright 1.58.0 实抓** ModelScope 的真实 HAR 脱敏成可提交夹具：12 条 entry、entry 0 = `GET /api/v1/users/login/info` **401**、**Cookie 8 项且不带 csrf**——csrf 是服务端下发后续请求才带）+ **前端单测 12 条** + **真机四证**（真实应用 + 真实抓包文件 + UI 自动化：导入 → 标 2 个 → 保存 → 提示「已保存：8 个变量」；库内 `checkin_site` 1 行 `enabled=0`，模板 Cookie 头整条占位符化，8 条凭据 `kind=cookie`；**`store.db`/`-wal`/`-shm`/`vault.bin` 四层扫描明文命中 0**）四证。回归 `cargo test --all-targets` → **367 项全绿**（311 单元 + 56 e2e），4 ignored，0 失败 0 告警；`npm run build` 通过（30 modules，gzip 58.92 kB，**新增运行时依赖 `@tauri-apps/plugin-dialog`**——原生文件选择框）。口径见 02 §7.1.1/§7.1.2 + §13（v0.15），验收留档 `docs/reviews/T15-har-import-and-template.md`（真机截图 `app-ui-t15-checkin.png` / `app-ui-t15-fields.png`）。**已知限制**：csrf 会轮换而模板只固化录制那一刻的 Cookie（重录 HAR 才更新，属 T16/T18）；base64 请求体不改写；JSON 路径断言属 T16；**未对真实站点发过签到请求**（属 T18）。

**T16 重放引擎** `2d` · 依赖 T15
- 变量注入、请求重放、响应断言（状态码 + JSON 路径）、错误分类。验收：单测 3 场景（成功/断言失败/网络失败）。
- 落地（2026-11-27）：**验收通过**。新增 `checkin::replay`（`inject` 变量注入 → `ReplayEngine::replay` 重放 → `check_assertion` 断言 → `evaluate` + `refine_by_status` 错误分类）+ `store::checkin` 的 `checkin_result` 读写（`insert_result` / `list_results`）。**变量注入**：敏感变量按 `credential_id` 从保险库解密、非敏感变量用 `default`，替换发生在**值内部**（整个头 / Cookie 子项 / query / JSON body 叶子）；**缺凭据不发请求**（`credential_missing`）；**未知占位符拒绝重放**（`template_invalid`）。**请求构造**：**query 以模板 `request.query` 数组为准重建**（HAR 的 `url` 自带 query 与 `queryString` 重复，先剥 URL 再重建，否则参数发两遍）、**剔除 `Host` / `Content-Length`**（由客户端按实际 URL 与请求体生成）、连接 10s / 整请求 30s、**引擎自己不重试**（重试与冷却属 T17）。**断言**：`assert.status` 是**期望状态码**（不是"录制时的状态码"）+ 新增可选 `assert.jsonPaths`（点号路径 + 数组下标 + `equals`），二者「与」关系，**旧模板兼容**（新字段一律可选）。**错误分类** 8 类：`success` / `assert_failed` / `network_error` / `credential_missing` / `template_invalid` / `credential_invalid`(401·403) / `rate_limited`(429) / `http_error`，每类带「可重试」标记（沿用 02 §3.3 判据）；**不猜验证码**（不内置验证码识别）。**结果落库**：`ok = 1` 仅当 `success`，`message` 是可直接用于通知的中文原因；**响应体不进库**（只在内存留 512 字节片段，防 csrf / token 密文落库）；`reward_json` 留空（积分回写属 T18 / T17）。证据：**单测 16 条**（注入 / 断言 / 分类 / 未知占位符 / query 重建 / Host 剔除）+ **e2e 4 条**（真 mock 上游跑全链，用的是 **T15 的真实产物**：真实抓包夹具 → 真实模板 → 真实保险库）+ **真机验收**（`cargo run --example checkin_replay_demo`：一条命令走完「真实抓包 → T15 存模板 → T16 重放 → 落结果 → 零明文核对」；5 场景实测：成功 `success` / JSON 路径不符 `assert_failed` / 上游 401 `credential_invalid` / 连不上 `network_error`（可重试）/ 缺凭据 `credential_missing`；**上游收到的 Cookie 是保险库里的真值**（占位符一个没发出去）、**缺凭据与网络失败时上游请求数不增加**、`checkin_result` 5 行 `ok` 标记一致、**库 + 保险库明文命中 0**）四证。回归 `cargo test --all-targets` → **388 项全绿**（328 单元 + 60 e2e），4 ignored，0 失败，**0 编译告警**；`npm run build` 通过（30 modules，gzip 58.92 kB，**T16 零前端改动**）。口径见 02 §7.1.3 + §13（v0.16），验收留档 `docs/reviews/T16-replay-engine.md`。**已知限制**：导入的模板带着**录制时的状态码**（ModelScope 匿名态 = 401，真实成功是 200）→ **用户必须自己改**（无断言编辑 UI，已记 backlog，T18 站点插件会带正确断言）；csrf 会轮换而模板不自己刷新（重录 HAR 才更新）；**无 UI 触发入口**（「立即执行一次」属 T17 / T18）；**未对真实站点发过签到请求**（属 T18）。

**T17 调度器 + 系统兜底** `2d` · 依赖 T16
- cron 调度、Windows 计划任务/launchd 注册、错过补签。验收：关闭应用后计划任务仍触发并补签。
- 落地（2026-11-27）：**验收通过**。新增 `checkin::cron`（**经典 5 字段**归一化补秒位 + `last_due_at`/`next_due_at` + `CronShape` 六类形状）、`checkin::scheduler`（`due_of` 去重判定 / `run_due_sites` / `run_ticker` 每分钟 tick / `run_now`）、`checkin::system`（Windows `schtasks` 注册·查询·列表·注销 + macOS launchd plist + 其它平台 stub）、`checkin::cli`（`--checkin-once [--site <id>]`：**不起 UI、不起网关**，跑完退出）；`commands::checkin` 增 4 个 IPC（`checkin_set_schedule` / `checkin_run_now` / `checkin_results` / `checkin_system_sync`），`SiteSummary` 扩 7 个调度字段；前端 `src/checkin/schedule.ts`（纯函数：cron 拆字段 / 校验 / 中文说明 / 相对时间）+ 签到面板**调度区**（cron 输入 + 启用开关 + 「立即执行」+「同步系统兜底任务」+ 最近执行记录表）。**调度模型**：每分钟 tick（对齐整分钟 +3s 偏移），`cron` crate 解析，**本地时区** `chrono::Local` 解释；**去重**：`due_at` = 最近一次应触发时刻（≤ now），`last_attempt` = 最近一次尝试（**成败都算**）的 `checkin_result.ts`，`last_attempt >= due_at` → 本周期跳过（**落库去重 ⇒ 跨重启成立**）；**失败不在同周期重打上游**（要立刻再试用「立即执行」）；**补签**：应用启动 / 每分钟 tick / 计划任务唤起**三处走同一套判定**；**系统兜底**：每个「启用 + 有 cron」的站点注册**一个**计划任务 `LLM Router Checkin - <site_id>`（动作 `"<exe>" --checkin-once --site <site_id>`），cron → `/sc minute /mo N`（含 `*` → 每 1 分钟）/`hourly`/`daily`/`weekly` 映射，**未识别表达式退回每天一次**（宁可多跑不可漏跑）；macOS 用 launchd plist（`StartCalendarInterval`/`StartInterval`）。**真机实测揪出并修掉两个真 bug**：① `* * * * *` 被归进"未识别 → 每天一次"，会把每分钟降级成每天 → **兜底漏跑**（已修 + 回归单测）；② 注销靠**匹配 schtasks 错误文案**判"任务不存在"，而文案随系统语言变（英文 `ERROR: The system cannot find the file specified.` / 中文 `错误: 系统找不到指定的文件。`）→ **漏判成注销失败**（已改成**先查后删**；顺带把系统命令输出改成按 **OEM 代码页**解码，无控制台 GUI 进程里的 GBK 输出不再出乱码）。证据：**单测 20 条**（cron 9 + scheduler 9 + 判定口径）+ **e2e 6 条**（`tests/checkin_scheduler_e2e.rs`：每周期一次 / 漏周期补签 / 停用不跑 / 单站点限定 / **Windows 计划任务真实注册·查询·注销** / 频率映射）+ **前端单测 11 条**（`node --test`）+ **真机四证**（真实应用 + 真实 UI 自动化填 cron → 勾启用 → **真实计划任务注册**（动作实测指向应用自身 exe）→ **关掉应用** → 计划任务在 02:04:01 / 02:05:01 **持续触发**，每次都带**保险库里的真值 cookie**，上游命中、结果落库 `ok=1`、`Last Result: 0`；**补签**：cron `0 9 * * *`（当天 09:00 已过）在应用关闭时由 CLI **补签执行**成功；**去重**：同周期再跑 → "跳过（本周期已跑过）"；**跨重启**：应用启动的首次 tick 对两个站点都判跳过，**不重打**；**库/WAL/SHM/vault 四层扫描明文命中 0**）四证。回归 `cargo test --all-targets` → **414 项全绿**（348 单元 + 66 e2e），4 ignored，0 失败 **0 编译告警**；`npm run build` 通过（31 modules，gzip 61.45 kB）。口径见 02 §7.2.1 + §13（v0.17 / **v0.18 真机修订**），验收留档 `docs/reviews/T17-scheduler-and-system-fallback.md`（真机截图 `app-ui-t17-schedule.png`）。**已知限制**：**导入的模板带着录制时的状态码**（ModelScope 匿名态 401 vs 成功 200）→ 用户必须自己改（**无断言编辑 UI**，backlog，T18 站点插件会带正确断言）；csrf 会轮换而模板不自刷新（重录 HAR 才更新）；**未对真实站点发过签到请求**（属 T18）；Windows 计划任务默认**「仅当用户登录时运行」**（用户注销则不触发）；macOS launchd 分支**未在真机验证**（本机是 Windows）。

**T18 站点插件 ×2** `2d` · 依赖 T16 T17
- **站点清单已指定（头头 2026-11-26 裁决）：ModelScope（魔搭）+ MiniMax Code（网页版）**。
- 两站正好覆盖 01 §1.1 模块 C 要求的两种签到形态：
  - **ModelScope**——平台**自己的公开接口**，`GET /api/v1/users/login/info` 触发当日签到、
    `GET /openapi/v1/magicubes/transactions` 对账；不逆向客户端、不构造签名。
    领到的魔粒可用于 ModelScope 的 API-Inference，是本项目"签到的额度直接进同一渠道池"最直接的一站。
  - **MiniMax Code（网页版）**——每日 400 积分（第 4/7 天各 1000），浏览器自动化型：
    复用登录态点账户菜单的「每日签到」。
- **合规约束（AGENT.md §5.7）**：签到撞到验证码 = 失败并通知；**不做**指纹伪造、验证码绕过、签名伪造。
  据此已排除的候选：Qoder CN（签到链要 COSY 签名）、Loomy（账号域 HMAC-SHA1，AK/SK 为客户端安装包内置常量）。
- 验收：M3 判据。
- 落地（2026-11-27）：**失败路径 + MiniMax Code 成功路径已验收通过；ModelScope 成功路径待头头的网页会话 Cookie**
- 落地（2026-11-27，第三轮）：**ModelScope 成功路径已验收通过**（登录态口径从「手抄 Cookie」改成「**访问令牌** `ms-…`」）。头头反馈：「cookies 只能手动输入吗？……并不是所有人都会获取 cookies」→ 魔搭的**开放平台访问令牌可直接当网页会话用**（`Cookie: m_session_id=<令牌>` → `GET /api/v1/users/login/info` 即触发当日签到），且**同一个令牌就是 `/openapi/v1/*` 的 Bearer** → **一个令牌，签到 + 魔粒全包**；判别规则（纯函数 + 单测）：密文**含 `=`** → 原始 Cookie 头（老路保留），**不含 `=`** → 访问令牌。**两条走不通的路（都不内置，实现已全部撤销）**：①「让应用开浏览器自动登录魔搭」——真机实测 Chrome 挂「自动测试软件正在控制 Chrome」、滑块报 `验证失败（error:22phmt）` **根本没渲染**（魔搭人机验证**专门拦自动化浏览器**），而**反检测 / 绕验证码撞 AGENT.md §5.7 红线**；②「浏览器接管会话」——凡是我们能读 Cookie 的浏览器都是被自动化控制的浏览器，登录同样过不了人机验证 → 辅轨 `session` 动作 / `run_secret` / IPC / 前端按钮**已全部删除**。**真机证据**：访问令牌 → **HTTP 200**、`Success=true`、`Message="success"`、**277 ms** → 落库 `checkin_result` id=10 `ok=1`。**魔粒流水真实形状已收敛**（`GET /openapi/v1/magicubes/transactions` → 200）：`data.records[]`，**金额 = `total_amount`**、**日期 = `gmt_created`**（只有日期）、**类型 = `type`**、**规则 = `rule_key`**；当日实测三条 → `daily_active` **200**（每日签到那一笔）+ `infocomplement_profile_experience` 50 + `infocomplement_email_verification` 50（**一次性资料奖励，不算进签到奖励**）；**网页会话 Cookie 也能开 `/openapi/v1/*`**（与 Bearer 回同一份流水）。**奖励口径改为「签到前后差值」**——站点**不返回**「今日魔粒 / 已领」信号（签到响应体只是用户资料，含 `GitlabAccessToken` 等敏感字段 → **插件不存响应体**），而当日那条 `daily_active` **整天都在** → 直接取当日总额会把**同一天的第 2、3 次运行**当成新到账、**额度池重复计数**；故取**差值**：> 0 = 本次到账（回写额度池），= 0 = 判**「今天已领过」**（`points=null`、**不回写**，与 MiniMax Code 口径一致）。真机当天跑的是**已领过分支**（`ModelScope 签到成功（今天已领过：当日每日签到魔粒 200，本次没有新增）`，`reward_json=NULL`）；**「差值 > 0」分支待次日首次运行**（02 §7.4 §5）。回归 `cargo test --all-targets` → **449 项全绿**（373 单元 + 76 e2e），4 ignored，0 失败 0 编译告警；前端 **44/44**；`npm run build` 通过（32 modules，gzip 62.78 kB）。。新增 `checkin::plugin`（`SitePlugin` trait + `builtin()` / `builtin_catalog()` + `parse_config()`）、`checkin::reward`（`Reward { amount, unit, raw }`，**`raw` 只留数值与布尔**——字符串可能是 csrf/token 一类密文）、`checkin::sidecar`（辅轨 **CLI 协议**客户端：起 Node 进程跑一个动作、**stdout 吐 JSON**、用完即退）、`checkin::sites::modelscope`（`evaluate_signin` **断言 JSON 业务字段 `Success`** + `extract_reward` 候选路径）、`checkin::sites::minimax_code`（走辅轨 + `class_for` 失败码映射 + 登录交接）、`store::account`（积分回写 `account.quota_json`，**合并写**：只改 `points`/`unit`/`source`/`updated_at`，**不动**限额字段）；`checkin::scheduler` 按 `plugin` **分派**（`har` → 重放引擎 / 内置 → 插件 / 未知 → `template_invalid`）并把 `reward` 写进 `checkin_result.reward_json` + 回写账号；`commands::checkin` 增 **7 个 IPC**（`checkin_builtin_sites` / `checkin_add_builtin_site` / `checkin_set_credential` / `checkin_plugin_login` / `checkin_sidecar_health` / `checkin_site_reward`）；前端新增「**内置站点插件**」区块 + 调度表「登录态」列 + `src/checkin/plugin.ts` 纯函数。**归属口径**：内置站点建站时建**占位渠道**（`provider='custom'`）→ 取 §8.2 默认账号 `acct_default_<channel_id>`（`credential.channel_id` / `account.channel_id` 都是 NOT NULL 外键）；**登录态明文只进保险库**，不回显、不进日志。**辅轨**：`sidecar/index.mjs`（`health` / `login` / `checkin` 三动作，**优先系统 Chrome**（`channel:"chrome"`，**不下载 150MB**）→ 回落自带 chromium，**持久化 profile**；**结果以页面自己的 `/signin/claim` 响应为准**，不解析 DOM 里的数字；`AlreadyClaimed`/`claim_result=2` 判「今天已领」≠ 失败）。证据：**Rust 单测 21 条**（reward 3 + plugin 3 + sidecar 2 + modelscope 6 + minimax_code 2 + account 5）+ **e2e 9 条**（`tests/checkin_plugin_e2e.rs`，mock 上游跑全链：**上游真的收到保险库里的 Cookie**、积分 400 三处落点一致、**查不到魔粒时签到仍成功**、**没凭据时上游请求数 0**、**站点行里没有明文**）+ **前端单测 29/29**（新增 6 条）+ **真机四证**（① **ModelScope 打真实 `www.modelscope.cn`**：错误 Cookie → **HTTP 401** → 判 `credential_invalid`，**152 ms**；② **MiniMax Code 走辅轨打真实 `agent.minimax.cn`**：Rust → Node → Playwright → 系统 Chrome 全链，判 `not_logged_in`，**2741 ms**；③ **四层扫描明文命中 0**（`store.db` / `-wal` / `-shm` / `vault.bin`）；④ **辅轨探活** `{"browserChannel":"chrome","nodeVersion":"v22.16.0"}`）四证。**真机揪出并修掉 4 个 bug**：辅轨同名变量 TDZ / **插件没覆盖 trait 的 `default_schedule`**（MiniMax 默认 cron 错成 `0 9 * * *`，已加「清单 vs 插件一致性」回归单测）/ **`login` 动作在页面渲染前就误报「登录完成」**（假阳性）/ `checkin` 动作渲染前误判登录态（后两个的修法：等 `waitForSelector("button")` 再判，**不用 `networkidle`**——该站持续请求要等 20 s）。回归 `cargo test --all-targets` → **444 项全绿**（369 单元 + 75 e2e），4 ignored，0 失败 **0 编译告警**；`npm run build` 通过（32 modules，gzip 62.73 kB）。口径见 02 §7.4 + §13（v0.19 / **v0.20 真机修订**），验收留档 `docs/reviews/T18-site-plugins.md`（截图 `app-ui-t18-plugins.png` / `app-ui-t18-sidecar-login.png`）。**成功路径真机回填（2026-11-27，第二轮）**：头头提供登录态后**两站各走一半**——① **MiniMax Code 登录交接打通**：站点**短信/验证码登录被频控**（点「获取验证码」报「操作频繁」，短信根本发不出去）→ 辅轨改为**自动打开登录弹窗 + 自动切「账号密码登录」**（**只铺路**：不代填密码、**不绕人机验证**，撞 AGENT.md §5.7），**登录成功**（硬信号：页面自己的 API 请求里 `user_id` 从 0 变成真实 id），**登录态落进应用自己那份持久化 profile**（`%APPDATA%\llm-router\browser-profile` → 应用侧立刻可用）；② **签到入口定位并收敛成一条**：**入口不在首页**（首页一个字都没有「签到」），在**用户头像下拉**里的「每日签到」（触发器 `.ant-dropdown-trigger`，**页面上有两个，只认有文字的那个**）；③ **「每日签到」路径真机跑通**：Rust → Node → Playwright → 系统 Chrome → 真实 `agent.minimax.cn` → 落库 `ok=1`，UI 显示「签到成功（success）：MiniMax Code 今天已经领过了（连签第 7 天）」，**4307 ms**；④ **每日积分口径实测对上任务卡**：`/signin/status` 的 `points` 字段 = **每日 400、第 4/7 天各 1000**（**实测值，不再是任务卡的推测**），实测当天为**连签第 7 天、当日 1000 分**；⑤ **`status == 3` = 已领取**，**「今天已领」判成功（幂等）**；已领过时报 `points = null`（本次**无新增**积分 → **不回写额度池**，避免重复计数），当日积分与连签天数写进回执文案。**真机揪出并修掉 4 个 bug（第二轮）**：辅轨 `checkin` 动作**直接在首页找签到文案必然找不到**（入口在用户菜单里）→ 改为**先开签到面板、再判状态**；`/signin/status` 的 `days[]` 形状没被 `extract_reward` 认出来（它只找平铺字段）→ 补 `is_today` / `day_no` 形状；**业务成功判据漏了 `base_resp.status_code == 0`**（AGENT.md §6 已知坑 1：HTTP 200 也可能是业务失败）→ 补上，且「**带了 `base_resp` 就只认它**」；**登录超时 5min 太紧**（真人要输密码 + 过人机验证）→ 放宽到 **15min**。回归 `cargo test --all-targets` → **444 项全绿**（369 单元 + 75 e2e），4 ignored，0 失败 **0 编译告警**；前端 **44/44**；`npm run build` 通过（32 modules，gzip 62.73 kB）。**已知限制**：**ModelScope 的真签到成功与魔粒字段路径仍待头头的网页会话 Cookie**（02 §7.4 §5）；**MiniMax Code 的 `/signin/claim` 真正领取响应体未实测**（真机当天**已经领过**，面板是禁用态「今日已签到」，**没触发过 claim**）→ **积分回写对真实站点也还没实测**；辅轨依赖 Node 运行时 + `sidecar/` 的 `npm install`（**分发形态不在本卡**）；**删站点不清内置站点的凭据**（backlog）；**探测脚本不许用临时 profile 反复打线上站点**（会被站点判成「新设备」并打频控——真机踩过，直接把短信通道打限流）。

**T19 通知** `1d` · 依赖 T16
- 系统通知 + webhook（企微/飞书/Telegram 任选）。验收：失败通知含站点名与原因。
- **卡片补全（2026-11-27，头头批准开卡前补）**：口径见 02 §7.5（§13 v0.25）。
  - **触发**：落点 = `scheduler::run_site` 之后（**结果已落库之后**）→ 一处覆盖**四条路径**（应用 tick / UI 立即执行 / CLI `--checkin-once` / **Windows 计划任务，应用关闭时**）；**成功与失败都通知**（失败**永远**通知 = §5.7「撞到验证码 = 失败并通知」；成功可关）。
  - **内容**：标题 `签到成功/失败 · {站点名}`，正文含**分类 + 中文原因**（+ 成功时的本次到账）；**零敏感信息**（不带凭据 / 响应体）。
  - **通道**：系统通知（`notify-rust`——**CLI 与计划任务路径没有 Tauri app handle**，刻意不用 Tauri 通知插件）+ webhook（**企微 / 飞书 / Telegram** 三种群机器人格式）。
  - **配置**：`app_setting` 只存非密文项（总开关 / 成功也通知 / 类型 / **凭据 id** / Telegram chat_id）；**webhook 地址与 bot token 进保险库**（`kind='webhook'`，挂占位渠道「通知」），**零明文**、不回显 IPC。
  - **失败**：通知发不出去**不影响签到结果**（只记日志、不重试、不回显 URL）。
  - **验收判据（本卡）**：① 成功通知含**站点名**与本次到账；② 失败通知含**站点名与原因**；③ **应用关闭时**（计划任务路径）也能通知；④ webhook 地址在库 / WAL / SHM / 保险库**四层扫描零明文**；⑤ 通知失败**不影响** `checkin_result` 落库。
  - **边界（不做）**：邮件 / 短信 / 钉钉 / 企微**应用**消息、通知历史与已读状态、模板自定义、@人。
- **落地（2026-11-27）**：**M3 第五张卡**（M3 判据的最后一环）。新增 `checkin::notify`（配置 / 文案 / 三种 webhook 请求体 / 系统通知 / 错误脱敏）+ `checkin::system_notify`（**Windows AUMID 注册**）+ `commands::notify`（**3 个 IPC**：`notify_get_config` / `notify_set_config` / `notify_test`）+ 前端「**结果通知（T19）**」区块。**落点 = `scheduler::run_site` 之后**（结果已落库才通知）→ 一处覆盖四条路径；**失败永远通知**、成功可关；**通知失败不影响签到结果**（只记日志、不重试）。**通道决策**：系统通知用 **`notify-rust`**（**刻意不用 Tauri 通知插件**——CLI 与计划任务路径**没有 app handle**，用插件就覆盖不了「应用关闭时也通知」）。**密文边界**：webhook 地址 / bot token 进**保险库**（`kind='webhook'`，挂占位渠道「通知」），`app_setting` 只存**凭据 id**；IPC 只回「配没配」；**reqwest 错误一律归一成不含地址的一句话**。**真机证据**：① 本地接收器（`POST /hook` + 上游 401）收到**企微形状**原文——含**站点名 + 分类 + 原因**、**零密文**；② **Windows 计划任务路径（应用完全关闭）真机送达**：`schtasks /run` → `Last Run Time 12:13:38` → 接收器**同时**收到通知体 + `checkin_result` 新增 id=19 → **判据 ③ 有真机证据**（计划任务 `Last Result = 1` = **签到失败**，不是任务挂了；T17 退出码契约 0/1/2）；③ **真机全库四层扫描**（`scripts/security_check.py` 带 webhook 明文做 `--secret`）→ **明文凭证命中 0 条、退出码 0**；④ AUMID 注册落地（开始菜单 `LLM Router.lnk`）。**真机揪出并修掉**：① **toast「已发送」但屏幕什么都没有** → 根因是**非 powershell 进程冒用 PowerShell 的 AUMID 会被 OS 静默丢弃**（API 仍返回成功）→ 自建 AUMID + COM 注册开始菜单快捷方式 + 写 `System.AppUserModel.ID`；② reqwest 错误**带出地址** → 统一脱敏。回归 `cargo test --all-targets` → **458 项全绿**（0 失败 / 4 ignored / 0 编译告警）；前端 **49/49**；`npm run build` 通过（33 modules，gzip 64.24 kB）。**第二轮真机排查（头头反馈「屏幕上没有」）**：查事件日志 `Microsoft-Windows-PushNotification-Platform/Operational` → `Id=3150` / `PolicyReason [GlobalSettingDisabled]` → **不是代码问题**：Windows 的**全局通知总开关**关着（`HKCU\…\PushNotifications\ToastEnabled = 0`）→ **所有**应用的 toast 都被拦（别的应用同样中招 → 「连 PowerShell 原生 toast 也不显示」是**同一原因**，此前误判成我们 AUMID 的问题）。修法：打开开关 + **重启 `WpnUserService_<sid>`**（服务**缓存**策略，不重启当场重发仍被拦）→ 事件日志变成 `Id=3153 Toast … is delivered to LLM Router on session 2`，且通知库 `wpndatabase.db` 里**出现我们的 payload**（**被丢弃的 toast 从不入库** → 「库里有记录」= 投递成功的硬信号）。**代码侧补「如实报告」**：发送前读全局开关，关着就报「Windows 全局通知已关闭（设置 → 系统 → 通知）——请打开后再试」，**不再谎报「已发送」**（`system_notify::global_toasts_enabled` + 纯函数 `global_block_reason` + 2 条单测；两个场景真机实测）。回归 `cargo test --all-targets` → **460 项全绿**（0 失败 / 4 ignored / 0 编译告警）；前端 **49/49**；`npm run build` 通过（33 modules，gzip 64.24 kB）。**横幅已目视确认弹出**（2026-11-27 15:36 发出「LLM Router 通知测试」→ 头头确认屏幕右下角出现横幅）→ **T19 的判据全闭环**；**只剩一项未验证**：**真实企微 / 飞书 / Telegram 群机器人**（待头头给地址，当密文进保险库）。留档 `docs/reviews/T19-notification.md`。

> M3 合计 ≈ 9.5d（约 2 周）。

### M4 — 可反代（harness 内置额度）

**T20 Copilot OAuth** `2d` · 依赖 T2
- device flow、`ghu_` token 存 vault、copilot token 换取与 30min 刷新（02 §6.2 四步）。
- 验收：全流程单测 + 手动一次真实授权。

**T21 Copilot 出站适配 + 实测** `2d` · 依赖 T20 T6
- R-6 伪装头（以参考仓库源码核实固化）、出站转发、实测矩阵第 8 列。
- 验收：经网关流式对话成功；token 过期后自动续。

**T22 WorkBuddy 协议确认（抓包）** `2–3d` · 依赖 无（可与 T20 并行）
- 目标：确认 ① `~/.workbuddy/app/users/<hash>/` 内登录态字段；② 上游 API host 与鉴权头；③ device-id 是否参与签名。
- 方法：F12/代理抓 WorkBuddy 桌面端一次对话，记录到 `docs/reviews/workbuddy-protocol.md`（**脱敏记录字段名，不记录 token 值**）。
- 验收：产出可实现的协议记录；若发现请求签名/加密，触发 §5 降级决策。

**T23 WorkBuddy adapter** `3–5d` · 依赖 T22 T6
- 登录态只读扫描 + 转发 + 失效标记。验收：M4 判据②。

**T24 合规弹窗与开关** `0.5d` · 依赖 T20 T23
- 首次启用 Copilot/WorkBuddy 强制 ToS 确认；默认关闭。验收：M4 判据③。

> M4 合计 ≈ 11.5–14.5d（约 2.5–3.5 周；WorkBuddy 有 ±3d 不确定性）。

---

## 3. 依赖关系图

```
T0 ─► T1 ─┬─► T2 ─► T4 ─┐
          │             ├─► T6 ─► T7 ─┬─► T12 ─► T14
          └─► T3 ─► T5 ─┘            └─► T13 ─┘
                T4 ─► T8 ─► T9 ─► T10
                T2,T3 ─► T11
T1 ─► T15 ─► T16 ─┬─► T18
                  ├─► T17
                  └─► T19
T2 ─► T20 ─► T21 ─┐
T22 ─► T23 ───────┴─► T24
```

关键路径：T0→T1→T2→T4→T6→T7→T10（M1 主链，约 11.5d 纯工期）。

---

## 4. 排期建议

| 周 | 目标 |
|---|---|
| W1 | T0 T1 T2 T3（骨架+数据+保险库+入站） |
| W2 | T4 T5 T8（适配器+路由+预设）；并行 T15（HAR 导入） |
| W3 | T6 T9（流式管道+UI）；并行 T16 T17（重放+调度） |
| W4 | T7 T10 T11（usage+实测+安全验收）→ **M1 达成**；并行 T18 T19 |
| W5 | T12 T13 T14 → **M2**；并行 T20 T22 |
| W6 | T21 T23 T24 → **M3 收尾 + M4** |

总计 ≈ 6 周（单人 + AI 辅助，含缓冲）。WorkBuddy 若走降级路径，M4 减 3–5 天。

---

## 5. 风险缓冲与降级预案

| 风险 | 触发条件 | 预案 |
|---|---|---|
| WorkBuddy 协议有签名/加密（T22 发现） | 抓包显示请求含动态签名 | M4 降级：仅保活+额度查看，转发延至 v2；工期减 3–5d |
| 国产渠道 tools 兼容差（T10） | 矩阵多列红 | 预设标 partial + UI 明示；不阻塞 M1（网关本身不受影响） |
| Copilot 伪装头失效（T21） | 官方风控拦截 | 更新常量值；仍失败则 Copilot 降为"仅模型目录浏览"，额度反代延后 |
| MiniMax 国内 GroupId 实测不符（T10） | 拼接后仍 1004 | 按 §4.2 三种口径逐一实测（无/带/旧版路径），将结论固化回预设 |
| 单周进度滞后 >2 任务 | 连续两周 | 砍 T14 图表（降为表格 UI）、砍 T19 webhook（保系统通知） |

---

## 6. 明确不做与 v2 预留（引用）

见 `02-technical-design.md` §0.2 与 §12。执行中若出现"顺手做掉"的诱惑（Anthropic 入站、登录态切换器、指纹伪造），一律记入 backlog 不动工。

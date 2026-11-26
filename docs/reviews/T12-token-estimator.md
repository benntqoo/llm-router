# T12 验收留档 — 本地估算器（tiktoken-rs）

| 项 | 值 |
|---|---|
| 任务 | T12 本地估算器（03 §2 任务卡，M2 第一张卡） |
| 依据 | `docs/planning/01-architecture-and-feasibility.md` §5.5（tokenizer 选型、R7）；`docs/planning/02-technical-design.md` §8.1 估算入口（v0.11）、§1.3 技术选型；`docs/planning/03-delivery-plan.md` T12 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `00cb3be`（`gateway::estimator::TiktokenEstimator` + `usage.rs` 缺项补估 + 产线默认切换 + e2e + tiktoken-rs 依赖）；文档提交另计 |
| 结论 | **通过**（任务卡验收判据「无 usage 渠道强制走估算路径」实测通过；e2e + 真机双证） |

> 本文件改动范围：仅记录 T12 的验收判据、实测证据、交付物与取舍；**不含**后续任务的结论。

## 1. 判据对照

任务卡验收原文：「tiktoken-rs 包装；国产模型标 `estimated=1`。验收：无 usage 渠道强制走估算路径。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **tiktoken-rs 包装** | ✅ | §2：新增 `gateway::estimator::TiktokenEstimator`（tiktoken-rs 0.12.1），**产线默认估算器**由 `CharRatioEstimator`（字符数 / 4）换成它；7 条单测锁住编码选择与计数 |
| 2 | **国产模型标 `estimated=1`** | ✅ | §3：口径写进 02 §8.1（v0.11）；**任何本地估算值一律 `estimated=1`**，国产模型更是必须（tiktoken 与其真实分词不一致）；`is_approximation()` 把这条规则写进代码供 T13/T14 取用 |
| 3 | **无 usage 渠道强制走估算路径** | ✅ | §4：e2e（`usage_e2e`）断言落库数**精确等于** tiktoken 计数、且**不等于**字符启发式计数；§5 真机复现同一条路径（上游不返 usage → `estimated=1`，`prompt_tokens=21` 而字符启发式只会给 ~8） |

## 2. 实现

### 2.1 依赖

| 项 | 值 |
|---|---|
| crate | `tiktoken-rs` **0.12.1**（crates.io；`Cargo.lock` checksum `2aeff724…4cfa`） |
| 编码表 | crate **内嵌** `assets/*.tiktoken`（cl100k_base 1.6 MB / o200k_base 3.6 MB），**运行期不联网** |
| 引入方式 | `cargo add tiktoken-rs`；`cargo fetch` 在本机**网络超时**（300s 未完成），改为**直连 static.crates.io 下载 `.crate` 放进 cargo 缓存**再 `cargo build --offline`，sha256 与 `Cargo.lock` 一致 |
| 体积代价 | 调试版 `llm-router.exe` **24.2 MB → 32.2 MB**（内嵌 BPE 表，+8 MB） |

### 2.2 模块

新增 `src-tauri/src/gateway/estimator.rs`：

| 公开项 | 作用 |
|---|---|
| `Encoding` | `Cl100kBase` / `O200kBase` |
| `encoding_for(model)` | 按上游模型名选编码：**o200k 家族**（gpt-4o / gpt-4.1 / gpt-5 / o1 / o3 / o4 / gpt-oss）走 o200k；**其余全部**（含全部国产模型）走 cl100k |
| `is_approximation(model)` | 该模型是否**只能近似**估算：只有 OpenAI 家族接近精确，**国产模型与自定义兼容渠道一律视为近似**（01 §5.5 规则 3）。T13 的成本换算、T14 的 UI 展示据此提示"这是估算值" |
| `TiktokenEstimator` | 实现 `TokenEstimator`；计数口径 `count_with_special_tokens`（特殊标记也算，与 OpenAI 侧一致） |

### 2.3 口径细化（先改文档后改码）

02 §8.1 的「估算入口」原本只有一句话，本轮**细化为四种情形**（02 → v0.11）：

| 情形 | 处理 | `estimated` |
|---|---|---|
| 上游给了完整 usage | 原样用，不覆盖 | `0` |
| 上游没回 usage | tiktoken 估算输入与输出 | `1` |
| 上游回了 usage 但**缺其中一项** | **只补缺的那一项** | `1` |
| 出站失败 | 不估 token | `0` |

关键定义：**「缺失」= usage 对象里没有这个键；值为 0 不算缺失**（0 可能是合法的真实值，比如空回复）。

> 02 §8.1 原句「`prompt_tokens` / `completion_tokens` —— 上游 `usage`；**缺失时由估算器补**」在 T7 只做到了"整体缺失才估"，
> 本轮把"部分缺失"补齐：`ReportedUsage` 记录了**哪些项是上游真给的**，只对缺的那一侧调用估算器。

## 3. 「国产模型标 estimated=1」怎么落地的

01 §5.5 的原文是：「对国产模型只能近似，必须标 `estimated=1`。绝不用估算值冒充精确值。」

落地方式（三层）：

1. **行为层**：估算路径产出的任何一行都带 `estimated=1`——上游没回 usage 时是**必须**，
   国产模型因为 tiktoken 与其真实分词不一致更是**必须**（不存在"国产模型可以用估算值冒充精确"的空间）。
2. **代码层**：`is_approximation(model)` 把"只能近似"这件事变成可查询的事实，
   而不是散落在注释里；T13/T14 直接取用。
3. **护栏层**：上游已给的精确值**一律不被估算覆盖**（R7）；`estimated=1` 的行不参与预算告警。

## 4. e2e 证据（`src-tauri/tests/usage_e2e.rs`）

夹具已换成 **产线同款** `TiktokenEstimator`（原先是字符启发式）。

| 用例 | 断言 |
|---|---|
| `acceptance_success_without_upstream_usage_is_estimated`（升级） | 落库 `estimated=1`；`prompt_tokens` **精确等于** tiktoken 对该提示词的计数；且该值**不等于**字符启发式的计数（用例提示词含 emoji —— 字符启发式按 4 字符 1 token，tiktoken 里 emoji 是多个 token，两者必然不同 ⇒ "确实换了实现"测得出来） |
| `acceptance_partial_upstream_usage_estimates_only_missing_side`（新增） | 上游 usage 只给 `prompt_tokens=31` ⇒ 落库 `prompt_tokens=31`（**不被覆盖**）、`completion_tokens>0`（**由估算器补**）、`estimated=1`（含估算值的行必须标估算） |
| 既有 5 条 | 全绿（上游给全 usage ⇒ `estimated=0`、流式 tee、429 限额自学习、失败行不估 token 等**行为不变**） |

## 5. 真机证据

环境：应用实跑（`llm-router.exe`）→ 真网关 `127.0.0.1:8787` → **不返回 usage** 的 mock 上游
（`%TEMP%\llmr-t12-verify\mock_nousage.py`，响应体里**故意不带** `usage` 字段），
渠道与密钥来自 T10 验证库的副本（`%TEMP%\llmr-t12-verify\`）。

请求：`model=deepseek-chat`，提示词 `🌈估算🌈路径🌈验证🌈token🌈`（16 字符）。

| 项 | 实测 |
|---|---|
| HTTP | 200，响应体**不含** `usage` |
| 上游命中 | 1 次（确实打到了上游） |
| 落库行 | `prompt_tokens=21` `completion_tokens=12` **`estimated=1`** `status=success` |
| 对账：字符启发式会给多少 | 提示词 16 字符 → 字符启发式约 **8**；**实际落库 21** ⇒ 走的确实是 tiktoken |
| 对账：上游给了 usage 的行 | 同库既有行 `prompt_tokens=11 completion_tokens=5 **estimated=0**` ⇒ 精确值路径**未被影响** |
| 日志脱敏 | 该次请求日志行 `"authorization":"***"`；两个明文凭证**都不在**日志里 |
| 二进制体积 | 24.2 MB → **32.2 MB**（内嵌 BPE 表） |

## 6. 回归

```
cargo test → 299 项全绿（248 单元 + 7 channel_e2e + 5 gateway_e2e + 6 outbound_e2e
                       + 11 router_e2e + 13 stream_e2e + 6 usage_e2e + 3 vault_e2e）
             0 失败，2 ignored，编译告警 0
```

其中新增 7 条 `gateway::estimator` 单测 + 1 条 `usage_e2e` 用例。
本轮无前端改动，`npm run build` 不受影响。

## 7. 产物清单

| 文件 | 状态 |
|---|---|
| `src-tauri/src/gateway/estimator.rs` | **新增**——`TiktokenEstimator` + 编码选择 + 国产近似判定（7 条单测） |
| `src-tauri/src/gateway/usage.rs` | 改动——新增 `ReportedUsage`（记录哪些项是上游真给的）、`resolve_tokens` 支持**只补缺的那一项** |
| `src-tauri/src/gateway/fallback.rs` | 改动——非流式路径改用 `parse_reported_usage_from_response` |
| `src-tauri/src/gateway/mod.rs` | 改动——注册 `pub mod estimator;` |
| `src-tauri/src/lib.rs` / `src/gateway/server.rs` | 改动——产线默认估算器 `CharRatioEstimator` → `TiktokenEstimator` |
| `src-tauri/tests/usage_e2e.rs` | 改动——夹具换产线同款估算器；升级 1 条 + 新增 1 条验收用例 |
| `src-tauri/Cargo.toml` / `Cargo.lock` | 改动——引入 `tiktoken-rs` 0.12.1 |
| `docs/planning/02-technical-design.md` | §8.1 估算入口细化为四种情形；§13 记 v0.11 |
| `docs/planning/03-delivery-plan.md` | T12 任务卡补「落地」行 |
| `HANDOFF.md` | §4 执行入口、§8 账本更新 |

## 8. 未验证 / 遗留

| # | 事项 | 状态 | 说明 |
|---|---|---|---|
| 1 | **tiktoken 计数与国产模型真实分词的偏差** | 结构性偏差，无法消除 | v1 没有国产模型的 tokenizer；只能近似并标 `estimated=1`（01 §5.5 已定口径） |
| 2 | `is_approximation` 目前**只被单测与文档引用** | 待 T13 消费 | T13 成本换算要据此提示"估算值"，且估算值不参与预算告警（R7） |
| 3 | BPE 表**首次解析有一次性开销** | 已接受 | 懒加载单例，首次估算时解析（cl100k 1.6 MB / o200k 3.6 MB）；落在请求路径上但只一次 |
| 4 | 二进制 +8 MB | 已接受 | 内嵌 BPE 表的代价；桌面应用可接受 |
| 5 | `CharRatioEstimator` 仍保留 | 有意保留 | 作为**轻量回退**与既有单测的基准；**不是**产线默认 |

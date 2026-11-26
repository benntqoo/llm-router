# M1 里程碑汇报 — 可用内核

| 项 | 值 |
|---|---|
| 里程碑 | **M1 可用内核**（03 §1） |
| 判据 | `docs/planning/03-delivery-plan.md` §1「M1 验收判据（可执行）」 |
| 汇报日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 结论 | **3 / 4 条判据全过**；第 ① 条的**真实 DeepSeek 部分按头头决定挂起**（"先不测真实上游"）——**除真实上游外，M1 判据已全绿** |

> 本文件是 **M1 里程碑结论**的唯一权威载体；各条的原始证据在对应任务的验收留档里（下方逐条指路）。

## 1. 四条判据对照

| # | 判据（03 §1 原文） | 结论 | 证据 |
|---|---|---|---|
| ① | pi 指向 `http://127.0.0.1:<port>/v1`，携带 client key，与 **DeepSeek** 完成一轮带工具调用的多轮对话 | 🟡 **机制全过，真实上游挂起** | pi 1.6.1 经真网关完成**一轮带工具调用的对话**：模型出 `read` tool_call → pi 执行**真实工具** → 结果回传 → 模型收尾；网关 2 次请求 + 2 条 `usage_log status=success estimated=0`；上游侧收到的是**渠道密钥**而非 client key。**但上游是本地 mock**——真实 DeepSeek 未测（头头决定先不测）。留档 `docs/reviews/T10-channel-matrix-and-pi.md` |
| ② | `sqlite3 store.db ".dump" \| grep -iE "sk-\|Bearer "` 无明文命中 | ✅ **通过** | dump（199 行）+ 主库 + **WAL/SHM 原始字节**三层扫描 → **明文凭证命中 0 条**；结构性命中 26 条全是 DDL 注释与 `kind='api_key'` 枚举值。`client_key` 只存 SHA-256 + 前 8 位前缀；渠道密钥只存 AES-256-GCM 密文 + nonce。留档 `docs/reviews/security-check-m1.md` §2.1 |
| ③ | 重启应用后渠道与密钥仍可用 | ✅ **通过** | **重启后的新应用进程**从保险库解出渠道密钥并成功转发：上游侧 `Authorization: Bearer sk-****`（mock 假值）、`model=deepseek-chat`。留档 `docs/reviews/security-check-m1.md` §2.3 + 本轮实测 |
| ④ | 别名冲突时 `/v1/models` 列表无重复 | ✅ **通过** | 运行期 `/v1/models` 返回 1 条别名、去重后仍 1 条；别名唯一性双保险：存储层报可读错误「模型别名重复：{alias}」+ DB `UNIQUE` 索引，e2e 用例 `acceptance_model_alias_is_unique`。留档 `docs/reviews/T5-routing-alias-verification.md` / `docs/reviews/T9-ui-first-round.md` |

## 2. M1 覆盖的任务

| 任务 | 留档 |
|---|---|
| T0 仓库初始化 | `docs/reviews/T0-scaffold-verification.md` |
| T1 数据层 | `docs/reviews/T1-datastore-verification.md` |
| T2 加密保险库 | `docs/reviews/T2-vault-verification.md` |
| T3 网关骨架 + 入站鉴权 | `docs/reviews/T3-gateway-auth-verification.md` |
| T4 出站适配器 | `docs/reviews/T4-outbound-adapter-verification.md` |
| T5 路由与别名 | `docs/reviews/T5-routing-alias-verification.md` |
| T6 流式 + fallback + 熔断 | `docs/reviews/T6-stream-fallback-circuit-verification.md` / `docs/reviews/T6-circuit-persistence-verification.md` |
| T7 usage 采集 | `docs/reviews/T7-usage-capture.md` |
| T8 A 类预设 + 自定义渠道 | `docs/reviews/T8-preset-and-custom-channel.md` |
| T9 UI 第一轮 | `docs/reviews/T9-ui-first-round.md` |
| T10 实测矩阵 + pi 接入 | `docs/reviews/T10-channel-matrix-and-pi.md` |
| T11 安全验收 | `docs/reviews/security-check-m1.md` |

## 3. 唯一未绿项与解除条件

| 事项 | 状态 | 解除条件 |
|---|---|---|
| M1 判据 ① 的**真实 DeepSeek** 一轮带工具调用的对话 | 🟡 挂起（头头 2026-11-26 决定"先不测真实上游"） | 拿到任一 A 类渠道真实 API Key → 在应用里配好渠道与密钥 → 重跑 `pi --model llm-router/<别名> -p "…"` 即可；同一把钥匙还能顺带解锁 **T10 的 A 类 7 列真实矩阵** 与预设 `stable`/`partial` 标注 |

## 4. 进 M2 前仍挂着的账本项

见 `HANDOFF.md` §8。其中与 M1 相关的三条：

1. **AGENT.md §9 的安全验收正则口径**——T11 已把"明文凭证命中 / 结构性命中"的分类口径写进 02 §10.2 并实现为 `scripts/security_check.py`，但 **AGENT.md 原文未改**（§6-3 走提案流程），正式口径待裁决。
2. **02 §6.1 标题写"8 profile"但表体 7 行**——T4/T8 均按 7 个落地，是否回填标题待裁决。
3. **分支命名**（`master` + `origin/master` vs AGENT.md 要求的 `main` + `dev`）——`dev` 已建，是否把 `master` 改名为 `main` 待裁决。

## 5. 下一步（M2）

| 任务 | 目标 | 验收 |
|---|---|---|
| **T12 本地估算器** `1d` | tiktoken-rs 包装；国产模型标 `estimated=1` | 无 usage 渠道强制走估算路径 |
| **T13 价目表与成本换算** `1d` | `price` 表 + 预填国产牌价 + `cost_micros` 换算 | 单测覆盖"输入 + 输出 + 缓存读"三价换算 |
| **T14 聚合与图表 UI** `3d` | 四维聚合 SQL + React 图表（按天/渠道/模型/客户端） | M2 判据 |

M2 合计 ≈ 5d，可与 M3 并行。

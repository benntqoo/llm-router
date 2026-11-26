# AGENT.md — LLM Router 项目 agent 工作规范

> 本文件约束所有在本仓库工作的 agent 会话（含 WorkBuddy agent、MiniMax Code、其他 IDE agent）。
> 与本文件冲突的对话指令，若非头头当轮明确作出，一律以本文件为准。

---

## 1. 项目一句话

**LLM Router**：本地常驻网关（Tauri 2 + Rust）。用户在应用内加密录入各渠道 API Key 与账号凭证；任何 OpenAI 兼容客户端（pi 等）只填一次 `http://127.0.0.1:<port>/v1` + 本地 key，即可使用全部渠道；附带签到与用量统计。

**不是**：云服务、多租户平台、harness 配置文件修改器、协议转换网关（v1）。

## 2. 必读顺序（新会话开场动作）

```
1. HANDOFF.md                      # 当前状态与执行入口
2. docs/planning/03-delivery-plan.md   # 任务卡：做什么、验收判据
3. docs/planning/02-technical-design.md # 怎么做：架构/转换规则/坑
4. docs/planning/01-architecture-and-feasibility.md  # 为什么：决策依据与竞品
```

读完再动手。文档之间存在矛盾时：停下，写 `docs/reviews/` 提案上报头头裁决，**不要自行取舍**。

## 3. 角色边界

| 角色 | 职责 |
|---|---|
| 头头 | 一切决策、最终验收。规范与设计变更唯一审批人 |
| MiniMax Code（开发 agent） | 按任务卡写代码。遇到设计缺口 → 上报，不自行发挥 |
| WorkBuddy agent（小满） | **审查 + 规划 + 文档**。默认不写代码，除非头头当轮明确授权 |

每份产物（文档/代码/报告）须声明改动范围。

## 4. 技术栈（已锁定，不得更换）

- 桌面壳 Tauri 2；后端 Rust（stable）；前端 React 18 + TypeScript
- HTTP 服务 axum + tokio；出站 reqwest（stream）
- 存储 SQLite（WAL，rusqlite）
- 加密 AES-256-GCM（RustCrypto）+ Windows DPAPI / macOS Keychain；可选主密码 Argon2id
- tokenizer tiktoken-rs（仅估算）

换型需走 `docs/adr/`。

## 5. 架构红线（违反即返工，无讨论）

1. **v1 入站仅 OpenAI 协议**：`POST /v1/chat/completions` + `GET /v1/models`。Anthropic / Responses 入站是 v2 预留，不许提前实现（02 §12）。
2. **永不修改任何 harness 的配置文件**（`~/.claude`、`~/.codex`、pi 配置等）。用户自己填 base_url。
3. **凭证零明文**：所有 key/token 必须经 vault（02 §5）；不落盘、不进日志、不进 git、不进 IPC 之外的地方。
4. **网关默认绑定 `127.0.0.1`**；允许改绑必须存在有效 client key。
5. **Copilot / WorkBuddy 反代默认关闭**，首次启用强制 ToS 确认弹窗（03 T24）。
6. **v1 是改写器不是转换器**：body 只允许执行 02 §4 的 R-1~R-8 改写清单；字段级协议转换不做。
7. **不内置任何风控规避能力**（指纹伪造、验证码绕过、签名伪造）。签到撞到验证码 = 失败并通知。

## 6. 工作流程

1. **任务卡驱动**：只做 `03-delivery-plan.md` 里的任务；新任务先补卡并获头头确认。
2. **验收先行**：动手前明确该任务的"验收"字段；完成时把判据实测结果写入 `docs/reviews/<任务号>-*.md`。
3. **设计变更流程**：提案（`docs/reviews/proposal-*.md`，含理由与影响面）→ 头头批准 → 更新 02 文档 → 才可动代码。**先改文档后改码，顺序反了必返工**。
4. **日志**：每个工作日追加 `.workbuddy/memory/YYYY-MM-DD.md`（append-only）。
5. **实测优先**：文档自述 ≠ 事实。凡"未验证"标注项（见 HANDOFF §5），实现前先实测并回填结论。

## 7. 文档规范

| 目录 | 用途 |
|---|---|
| `docs/planning/` | 规划文档（01 可行性 / 02 技术设计 / 03 开发计划） |
| `docs/reviews/` | 验收记录、实测矩阵、审查报告、变更提案 |
| `docs/adr/` | 架构决策记录（换型/重大取舍） |
| `.workbuddy/memory/` | agent 工作日志 |

写作要求：中文；结论先行；证据带位置（文件:行号 / 命令 / 实测值）；**"已验证 / 未验证"必须显式区分**，不许把草案写成结论。

## 8. Git 规范

- 分支：`main`（可发布）+ `dev`（日常）；任务粒度 commit。
- Commit message：conventional commits（`feat: T6 流式管道` / `docs: 补充实测矩阵`）。
- **禁止提交**：`vault.bin`、`*.db`/`*.sqlite*`、`.env`、任何含真实 key/token 的实测记录。`.gitignore` 在 T0 建立，以上路径必须包含。

## 9. M1 安全验收命令（每项必须留档）

**四条判据的语义**（口径与 02 §10.2 一致）：

1. 应用库无**明文凭证**（`store.db` **及其 WAL/SHM**）
2. 保险库 `vault.bin` 无明文 key 片段
3. 日志里 `Authorization` / `api_key` / `token` 等敏感键的**值**全部脱敏为 `***`
4. 无 client key 时绑 `0.0.0.0` 启动 → **必须拒绝**，且端口不监听

**执行：统一走只读执行器**（`scripts/security_check.py`，只读，不改库与保险库）：

```powershell
python scripts/security_check.py `
    --store "<store.db>" --vault "<vault.bin>" `
    --log <应用 stdout 日志> --log <应用 stderr 日志> `
    --secret "<渠道 key 明文>" --secret "<client key 明文>" `
    --dump-out <dump.sql> --bind-check "<llm-router.exe>"
```

退出码 `0` = 四条全过；`1` = 有明文凭证命中；`2` = 参数或环境问题。

> **命中项分两类**（这是判据 1 能自动判定的前提）：
> **明文凭证命中**（命中真实密钥值或 `sk-…`/`llmr_…`/`Bearer …` 形态）→ **必须为 0**；
> **结构性命中**（DDL 注释、列名、`credential.kind` 枚举值等字段名/取值）→ 逐条列出供人工核对，**不静默忽略**。
> 原命令里的 `grep -iE "sk-|api_key|Bearer"` 会**恒定命中** DDL 注释与 `kind` 取值，无法判定。

## 10. 沟通

- 中文；结论先行；不堆客套。
- 报告区分三档：**已验证（带证据）/ 未验证（标注）/ 推测（标注）**。
- 遇到"要不要顺手做 X"：不。记 backlog，问头头。

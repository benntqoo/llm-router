# T10 验收留档 — 实测矩阵（A 类 7 列框架 + pi 实连接入片段）

| 项 | 值 |
|---|---|
| 任务 | T10 实测矩阵（A 类 7 列 + pi 接入）（03 §2 任务卡；关键路径 T9→T10） |
| 依据 | `docs/planning/02-technical-design.md` §6.5 / §10.1（v0.9）；`docs/planning/03-delivery-plan.md` T10 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `30e99c7`（矩阵执行框架 + pi 片段生成器 + 预设导出器）、`c637a08`（`.gitignore` 补 Python 缓存）、`fc8c5a7`（矩阵产物 `--note` 前提标注 + mock 自测产物）；文档提交另计 |
| 结论 | **部分通过**：**pi 实连接入（任务卡 2 条验收判据中的 1 条）完全通过**；**矩阵框架建成并在真网关上自测通过**，但 **A 类 7 列真实数据仍缺 API Key** —— 头头 2026-11-26 决定「先不测真实上游」，该项按**挂起**收口（非阻塞，见 §5） |

> 本文件改动范围：仅记录 T10 的验收判据、实测证据、交付物与取舍；**不含**后续任务的结论。

## 1. 判据对照

任务卡验收原文：「矩阵填写完整并落库到 `docs/reviews/channel-matrix-*.md`；pi 一轮带工具调用对话成功。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **pi 一轮带工具调用对话成功** | ✅ | §3：pi 1.6.1 经**真网关**发起一轮对话，模型出 `read` tool_call → **pi 执行了真实工具**（读到了目标文件内容）→ 结果回传 → 模型收尾；网关侧 **2 次请求**、**2 条 `usage_log`**（`status=success estimated=0`） |
| 2 | **矩阵填写完整并落库到 `docs/reviews/channel-matrix-*.md`** | ⚠️ 部分 | §4：矩阵**执行器**建成，6 项判定在真网关上全绿，产物已落库（`channel-matrix-2026-11-26-mock.md`）；但 **A 类 7 列的真实单元格仍为空**——本机无任何 A 类渠道 API Key，无从实测（§5 阻塞项） |
| 3 | **接入文档片段（02 §6.5 补全）** | ✅ | §2.3：02 §6.5 已按 pi 1.6.1 **自带 schema 与官方文档**重写（安装 / 配置文件位置 / JSON 片段 / 验证命令 / 实连结论），不再是"未验证"占位 |

## 2. 交付物

### 2.1 矩阵执行框架

| 文件 | 作用 |
|---|---|
| `scripts/channel_matrix.py` | 02 §10.1 前 7 列的 6 项执行器，产出 `docs/reviews/channel-matrix-<日期>.md`（主矩阵 + 逐项证据 + `usage_log` 落库证据）。新增 `--note` 用于在产物顶部标注"本次是 mock 自测"这类前提 |
| `src-tauri/examples/presets.rs` | 导出 A 类预设清单（`cargo run --example presets`），供脚本引用——**预设名单不在 Python 侧抄**，抄了必然漂移 |
| `scripts/pi_models.py` | 生成 `~/.pi/agent/models.json` / `settings.json`；别名清单**从应用库直读**（与 `GET /v1/models` 同源），`apiKey` 写成 `$LLMR_CLIENT_KEY`，**文件里不留明文** |

框架的三条设计取舍：

1. **必须经网关打，不直连上游**：矩阵判据是"这个预设能不能标 stable / partial"，
   那是**预设 + 网关改写（R-1~R-8）之后的联合属性**，不是渠道裸接口的属性。
2. **脚本只读应用库**：client key 只从 `--client-key` / 环境变量 `LLMR_CLIENT_KEY` 取，
   **不打印、不落盘**（与 02 §3.1「明文只显示一次」口径一致）。
3. **Python 内部用 `subprocess` 调 cargo**：绕开 PowerShell 5.1 的 `>` 重定向输出 UTF-16LE
   会把 JSON 写坏这个坑。

### 2.2 pi 接入片段（权威载体 = 02 §6.5）

生成物（本机）：

```json
{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "llm-router": {
      "name": "LLM Router (本地网关)",
      "baseUrl": "http://127.0.0.1:8787/v1",
      "api": "openai-completions",
      "apiKey": "$LLMR_CLIENT_KEY",
      "models": [
        { "id": "deepseek-chat", "name": "deepseek-chat", "contextWindow": 128000, "maxTokens": 8192 }
      ]
    }
  }
}
```

### 2.3 02 §6.5 的补全要点

依据是 **pi 1.6.1 安装目录内自带的权威材料**（不是二手博客）：
`~/.pi/agent/install/releases/1.6.1/node_modules/@earendil-works/pi-coding-agent/`
下的 `docs/custom-provider.md`、`docs/models.md`、`docs/cli.md`、`schemas/models.schema.json`。

- 配置文件位置：`~/.pi/agent/models.json`（provider 定义）+ `~/.pi/agent/settings.json`（默认 provider/model）。
- `api` 只能是 `openai-completions`（v1 入站仅 OpenAI 协议，§0.2）。
- `apiKey` 用 `$NAME` / `${NAME}` 环境变量插值，`$$` → 字面 `$`、`$!` → 字面 `!`、`!command` → 取命令输出。
- provider 可选字段：`name` / `baseUrl` / `apiKey` / `api` / `headers` / `compat` / `authHeader` / `models` / `modelOverrides`；
  model 条目**只强制 `id`**，其余有默认值。
- 别名清单**不要手抄**，用 `scripts/pi_models.py` 从应用库直读生成。

## 3. pi 实连实测

### 3.1 安装

| 项 | 结果 |
|---|---|
| 官方安装脚本 | `https://pi.dev/install.ps1`（71114 字符）下载后执行，**managed install 成功** |
| pi 版本 | **1.6.1**，装到 `%USERPROFILE%\.pi\agent\bin\pi.cmd` |
| Node | 脚本要求 **Node ≥ 22.19.0**；本机系统 Node 是 v22.16.0，**不满足** |
| 绕过办法 | 用**官方脚本自带的** `Install-NodeStandalone`（从同一脚本里摘出函数、去掉末尾 `Invoke-PiInstaller` 调用）装独立 Node **v22.23.5** 到 `%LOCALAPPDATA%\pi-node\current`（走 nodejs.org/dist + SHASUMS256 校验 + 解压，**不动用户既有 Node 环境**），再把该目录放到 `PATH` 前端重跑官方脚本 |
| 无交互终端的影响 | 脚本在 `[Console]::IsInputRedirected` 为真时：跳过自带的 Node 安装、跳过 Git Bash 安装、action 自动取默认 —— 官方脚本本身支持非交互，无需改脚本 |

### 3.2 配置与发现

```
$ pi --list-models llm-router
provider    model          context  max-out  thinking  images
llm-router  deepseek-chat  128K     8.2K     no        no
```

`--list-models` **不打网关**（只读本地 `models.json`）。

### 3.3 一轮带工具调用的对话（核心验收）

命令（`-p` 单轮模式）：

```
pi --model llm-router/deepseek-chat --no-session --mode json -p "请用read工具读取README.md并回答marker"
```

事件时间线（`--mode json` 的 JSONL，逐条核对）：

| 步 | 事件 | 内容 |
|---|---|---|
| 1 | `agent_start` / `turn_start` | 第 1 轮请求发出 |
| 2 | `message_start(role=assistant)` | `stopReason=pending`，开始收流 |
| 3 | `message_end(role=assistant)` | **`stopReason=toolUse`**，`toolCalls=[{id:"call_mock_1", name:"read", arguments:{"path":"README.md"}}]`，`usage={input:11, output:5}` |
| 4 | `tool_execution_start` | pi **真的去执行了工具**：`{"toolName":"read","args":{"path":"README.md"}}` |
| 5 | `tool_execution_end` | 工具结果 = 目标文件真实内容（含 `marker: LLMR-PI-TOOL-OK`） |
| 6 | `message_start/End(role=toolResult)` | 结果作为 `tool` 角色回灌进对话 |
| 7 | `message_start/End(role=assistant)` | 第 2 轮：`stopReason=stop`，正文「工具已执行完成，结果如上。」，`usage={input:11, output:5}` |
| 8 | `turn_end` / `agent_end` / `agent_settled` | `aborted=false`，进程退出码 0 |

### 3.4 网关侧证据

| 证据 | 值 |
|---|---|
| 网关请求数 | **2 次**（第 1 次出 tool_call，第 2 次带 tool 结果收尾） |
| 网关收到的入站 | `POST /v1/chat/completions`，`stream=true`，`model=deepseek-chat`，`tools=[read, bash, edit, write]`，消息角色序列 `system → user → assistant → tool` |
| **密钥改写生效** | 上游侧看到的 `Authorization` 是**渠道密钥**（`Bearer sk-t10-****`，本轮自造的 mock 假值），**不是**入站 client key —— 网关出站换密钥这条链路被真实客户端验证 |
| 用量落库（T7） | 2 条 `usage_log`：`model_alias=deepseek-chat protocol=openai status=success estimated=0 prompt_tokens=11 completion_tokens=5`（`estimated=0` ⇒ 采到的是上游真 usage，不是估算） |
| 流式 tool_call 透传 | pi **总是流式**（`stream=true`），其 `delta.tool_calls` 帧经网关透传后被 pi 正确解析成工具调用 —— 这是「网关是改写器不是转换器」在真实客户端上的直接证据 |

### 3.5 复现步骤

```powershell
# 1) 装 pi（官方脚本；Node 不满足时先用脚本自带的 Install-NodeStandalone）
# 2) 生成配置片段（别名从应用库直读，apiKey 走环境变量）
python scripts/pi_models.py --store "$env:APPDATA/llm-router/store.db" `
    --out "$env:USERPROFILE/.pi/agent/models.json" `
    --settings-out "$env:USERPROFILE/.pi/agent/settings.json" --write
# 3) 验证
$env:LLMR_CLIENT_KEY = "<client key 明文>"
pi --list-models llm-router
pi --model llm-router/<别名> -p "用一句话回答：1+1 等于几？"
```

## 4. 矩阵框架自测

命令：

```powershell
$env:LLMR_CLIENT_KEY = "<client key>"
python scripts/channel_matrix.py --store "<临时库>" --out docs/reviews/channel-matrix-2026-11-26-mock.md `
    --note "本次是**矩阵框架自测**：上游是本地 mock，不是真实渠道。"
```

结果（6 项全绿，mock 上游经**真网关**）：

| 项 | 结果 | 证据 |
|---|---|---|
| 非流式 chat | ✔ | HTTP 200，响应含 `choices[0].message.content` |
| 流式 chat | ✔ | 3 个 SSE 帧，增量文本 9 字 |
| tool_calls | ✔ | 返回 1 个 tool_call（`get_weather`，`arguments={"city":"llm-router"}`） |
| usage 返回 | ✔ | `usage={prompt:11, completion:5}` |
| reasoning 字段 | ✔ | 含 `reasoning_content`（17 字） |
| 多轮上下文 | ✔ | 第二轮答出代号「紫罗兰七三」 |

`usage_log` 落库证据：`alias=deepseek-chat upstream=deepseek-chat status=success estimated=0 prompt=11 completion=5`。

**产物**：矩阵表头与 02 §10.1 的 7 列同构；未配置的渠道标「· 未测」而不是留空，
避免"看起来测过"的歧义。

## 5. 未完成项与阻塞

**头头 2026-11-26 决定：「先不测真实上游」。** 下列两项按**挂起**收口（不再是阻塞），
本卡以「框架 + pi 完成、真实矩阵挂起」状态交付。

| # | 事项 | 状态 | 说明 | 解除条件 |
|---|---|---|---|---|
| 1 | **A 类 7 列真实矩阵** | 🟡 挂起（头头决定） | 本机**无任何 A 类渠道真实 API Key**：环境变量里没有、`~/.pi/agent/auth.json` 是空对象、用户真实库 `%APPDATA%\llm-router\store.db` 的 `channel` 表为空 | 拿到任一 A 类渠道真实 key（贴 7 个 / 只给部分均可）；到位后重跑 `scripts/channel_matrix.py` 即覆盖 `docs/reviews/channel-matrix-<日期>.md` |
| 2 | 预设 `stable` / `partial` 标注 | 🟡 挂起（随 1） | 标注判据来自真实矩阵结果 | 同上 |
| 3 | **连带影响**：03 §1 的 M1 判据 ①「与 **DeepSeek** 完成一轮带工具调用的多轮对话」 | 🟡 挂起（随 1） | 目前只有 **mock 上游**版本（机制、网关改写、流式 tool_call、密钥替换、用量落库均已验证），**真实 DeepSeek** 未测 | 同上 |

> 按 AGENT.md 的红线与「设计变更须走 提案 → 头头批准 → 先改文档后改码」，
> **没有真实 key 时不猜测任何渠道的能力**，矩阵里对应单元格一律留「· 未测」。

## 6. 踩坑记录（留给下一个人）

1. **官方安装脚本在无交互终端下会跳过自带的 Node 安装**，直接用系统 Node → 版本不够就报
   `error: Pi requires Node.js 22.19.0 or newer.`。解法是用脚本自带的同一个 `Install-NodeStandalone` 函数
   先装独立 Node，**不要改官方脚本**。
2. **pi 的 `pi.cmd` 自带 Node 定位**：`%LOCALAPPDATA%\pi-node\current\node.exe` 存在就用它，
   所以装完 standalone Node 后不需要改 PATH 也能跑 pi。
3. **`Start-Process -ArgumentList` 传中文提示会按空格拆成多个参数** → pi 把一句话当成多条提示，
   跑出多轮 agent。验证脚本里提示词**不要含空格**，或改用 `subprocess.run([...])` 传数组。
4. **mock 上游原来只会发非流式 tool_call**，而 pi **总是流式**（`stream=true`）→ 工具调用不触发。
   mock 需按 OpenAI 的 `delta.tool_calls` 帧形态补流式 tool_call。
5. **PowerShell 5.1 的 `>` 重定向输出是 UTF-16LE**：JSON 产物要用 Python 落盘，别用它。
6. `Get-Content` 读无 BOM 的 UTF-8 中文文件显示乱码**属正常**（文件本身是对的），
   校验用 Python 按 UTF-8 读。

## 7. 产物清单

| 文件 | 状态 |
|---|---|
| `scripts/channel_matrix.py` | 新增（本次追加 `--note`） |
| `scripts/pi_models.py` | 新增 |
| `src-tauri/examples/presets.rs` | 新增 |
| `docs/reviews/channel-matrix-2026-11-26-mock.md` | 新增（**框架自测**，非真实渠道数据） |
| `docs/reviews/T10-channel-matrix-and-pi.md` | 新增（本文件） |
| `docs/planning/02-technical-design.md` | §6.5 重写、§10.1 补执行方式与当前状态、§1.2 补 `scripts/` 目录 |
| `docs/planning/03-delivery-plan.md` | T10 任务卡补「落地」行 |
| `HANDOFF.md` | §4 / §5 / §8 更新 |

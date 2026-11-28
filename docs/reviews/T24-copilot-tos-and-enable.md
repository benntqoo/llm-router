# T24 合规确认与渠道启用开关（Copilot 半边）—— 验收留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2 **T24**
> 设计：`docs/planning/02-technical-design.md` **§6.8**（+ §13 **v0.35**）
> 提案：头头 2026-11-28 拍板「**做 Copilot 半边**」（WorkBuddy 半边随 T23 延后到付费后）
> 日期：2026-11-28

## 1. 摘要

| 项 | 结果 |
|---|---|
| 目标 | 让 Copilot 渠道**能被安全地启用**：用户显式确认 ToS 之后，渠道可开、模型出现在 `GET /v1/models`、可被网关路由；**未确认则永远开不了** |
| 后端 | 新增 `providers::compliance`（受限清单 + 声明 + 闸门）、`store::channels` 的 `accept_channel_tos` / `set_channel_enabled`、`channel_set_enabled` + `copilot_tos_accept` 两个 IPC（**63 → 65**） |
| 前端 | 新增 `src/copilot/tos.ts`（纯函数）、`src/ui/Modal.tsx`（自写模态框）、Copilot 面板「渠道启用（T24）」区块 |
| 数据 | **不改 schema、不加迁移**——确认记录落 `channel.meta_json` 两个**服务端管理**的字段 |
| 判据 | **8 条逐条实测全绿**（详见 §4） |
| 回归 | Rust **539 通过 / 0 失败 / 4 忽略**（26 个测试二进制）；前端 **79/79**；build **40 modules / gzip 70.01 kB** |
| 边界 | **WorkBuddy 半边不做**（等合规凭证）；不引第三方依赖；不新增迁移；不做验证码/指纹/签名类规避 |

## 2. 问题与口径

**问题（AGENT.md §5.5）**：「Copilot / WorkBuddy 反代**默认关闭**，首次启用强制 ToS 确认弹窗」。
但 T20 / T21 只做了「取凭证 + 模型目录」，**启用入口一直空着** → 渠道恒为 `enabled = 0`，
模型**永不**出现在 `GET /v1/models`，**M4 判据③ 对 Copilot 无法闭环**。

**三条硬口径**（02 §6.8.1）：

1. **默认关闭**：受限渠道创建即 `enabled = 0`；
2. **首次启用强制确认**：没有**有效**的确认记录 → **后端拒绝启用**；
3. **关闭永远不需要确认**（安全方向不该设门槛）。

**合规不靠前端**——这是本卡最核心的一条：前端弹窗只是「知情」环节，
**后端每个「把渠道置为启用」的入口都要过闸门**，否则通用渠道表单里的一个勾选框就能绕过。

## 3. 实现

### 3.1 后端

**`src-tauri/src/providers/compliance.rs`（新）** —— 纯数据 + 纯函数，无 IO：

| 项 | 值 |
|---|---|
| `RESTRICTED_PROVIDERS` | `["copilot"]`（WorkBuddy 后置 → 届时加一行） |
| `COPILOT_TOS_STATEMENT` | 「将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。」（**逐字进弹窗**） |
| `COPILOT_TOS_VERSION` | `copilot-tos-2026-11-28`（**改文案就要升版本** → 旧确认失效） |
| `ensure_can_enable(provider, accepted_at, accepted_version)` | 不通过就返回一条**指路**的错误（「请先在『GitHub Copilot 授权』面板确认 ToS 后再启用」） |
| `acceptance_is_valid(...)` | 「没确认过」或「声明版本对不上」都算**没确认过** |

**`store::channels`**：

- `ChannelMeta` 加两个**服务端管理**的字段：`tos_accepted_at` / `tos_statement_version`；
- `save_channel` 写 `meta_json` 时**用库里的旧值覆盖入参里的同名字段** → 前端设置/清除都无效；
- `save_channel` 在 `enabled = true` 时过闸门；
- 新增 `accept_channel_tos`（**只记录，不启用**）、`set_channel_enabled`、`load_tos_acceptance`。

**IPC（63 → 65）**：`channel_set_enabled(channel_id, enabled)`、`copilot_tos_accept()`；
`copilot_status` 视图加 `tos_accepted_at` + `tos_required`（**由后端裁定**「是否还需要确认」，
版本判断只有一份实现，前端不重复实现）。

### 3.2 前端

- **`src/copilot/tos.ts`（新，纯函数）**：`needsTosConfirmation` / `describeEnableState` / `describeEnableEffect` / `formatAcceptedAt`
  + 声明原文与版本号（**与后端逐字一致**）。
- **`src/ui/Modal.tsx`（新）**：自写模态框（**不引第三方依赖**）；点遮罩 / 按 `Esc` → 取消。
- **Copilot 面板**：新增「渠道启用（T24）」区块（开关 + 状态说明）。
  点开启且未确认 → **弹模态框**（声明原文 + 「我已阅读并确认」勾选 + 「确认并启用」/「取消」）；
  **取消 → 渠道保持关闭，且不记录任何东西**。确认 → 先 `copilot_tos_accept`（记录）→ 再 `channel_set_enabled(true)`（启用）。
  **关闭** → 直接关（不需要确认）。

## 4. 验收判据逐条实测

| # | 判据 | 怎么验的 | 结果 |
|---|---|---|---|
| ① | **绕过前端直接启用 → 被拒** | e2e：走**存储层** `save_channel`（连 IPC 都不经过）+ `channel_set_enabled` 两条路都试 | ✅ 报错「尚未确认 ToS —— 请先在『GitHub Copilot 授权』面板确认 ToS 后再启用」；**渠道仍然是关闭的**（拒绝无副作用） |
| ② | **伪造的确认不算数** | e2e：入参里塞 `tos_accepted_at` + 正确版本号 | ✅ 照样被拒（闸门只认**库里的**值） |
| ③ | **确认后可启用，模型出现在 `/v1/models`** | e2e：真网关 `GET /v1/models` | ✅ 启用前 **0 个别名**；确认 → 启用后 `gpt-4o-mini-2024-07-18` 出现在列表里 |
| ④ | **确认与启用分成两步** | e2e：`copilot_tos_accept` 之后渠道**仍然是关闭的** | ✅ 确认只记录，不启用（便于审计「什么时候确认的」） |
| ⑤ | **保存渠道表单不抹掉确认记录** | e2e + store 单测：确认后用**不带** tos 字段的 meta 保存 | ✅ 记录还在，渠道保持启用 |
| ⑥ | **关闭永远不需要确认** | e2e + store 单测 | ✅ 随时可关；关掉后模型从 `/v1/models` 下线 |
| ⑦ | **声明版本不匹配 = 没确认过** | store 单测：把版本号改成旧的再启用 | ✅ 被拒，报「ToS 声明已更新，需要重新确认」 |
| ⑧ | **删渠道 → 确认记录跟着消失** | e2e：删掉后重建同名渠道 | ✅ 新渠道**不继承**旧确认（记录跟着渠道走，不在全局设置里留残渣） |

**附带验证**：非受限渠道**完全不受影响**（`custom` / `deepseek` 照常启用；对非受限渠道调用
`accept_channel_tos` 明确报错「不是受限渠道，无需 ToS 确认」）。

### 4.1 自动化测试清单

- **Rust 单测 15 条**（新增）：`providers::compliance` **8 条**（受限判定 / 声明原文 / 版本失效 /
  闸门放行与拒绝 / 非受限不拦）+ `store::channels` **7 条**（拒绝启用 / 确认后可启用 /
  伪造与清除无效 / 关闭不需确认 / 删渠道清记录 / 非受限不拦 / 版本失效）。
- **Rust e2e 7 条**（新文件 `src-tauri/tests/compliance_e2e.rs`）：上面 ①~⑧ 的端到端版本
  （含**真网关** `/v1/models` 断言）。
- **前端单测 8 条**（新文件 `src/copilot/tos.test.ts`）：要不要弹框（4 种状态组合）/
  状态说明 / 后果说明 / 声明原文与后端一致 / 时刻格式化。

## 5. 真机取证

截图：`docs/reviews/app-ui-t24-enable.png`（启用区块）、`docs/reviews/app-ui-t24-tos-dialog.png`（ToS 弹窗）。
**两张都是最终代码的真实应用截图**（`cargo build --bin llm-router` + vite devUrl 1420 + 本机真实数据）。

**① 启用区块**（`app-ui-t24-enable.png`）：

- 面板顶部：「已授权（Copilot token 到期 11-28 22:55）。渠道默认关闭——首次启用要先确认服务条款。」
- 「**渠道启用（T24）**」区块 + **未勾选**的「启用 Copilot 渠道」开关 + 状态说明
  「渠道默认关闭——首次启用需要先确认 GitHub Copilot 的服务条款。」
- **同一个画面里的旁证**：上方「模型预览」显示 **0 个别名** —— 正因为 Copilot 渠道还关着，
  它的 31 个模型**一个都没对外发布**（这正是「默认关闭」的语义）。
- 渠道列表里的 Copilot 行标着「**已禁用**」。

**③ 真实数据下「启用 → 对外可见」的实测**（不动用户的真库）：

把本机真实 `store.db` **复制**一份到临时目录，用**与 `list_published_models` 一致的 SQL** 跑前后对比：

| | 对外别名数 |
|---|---|
| 渠道启用前 | **0** |
| 渠道启用后 | **19** |

新增别名实测为 `copilot-search-a` / `copilot-search-b` / `gpt-3.5-turbo` / `gpt-3.5-turbo-0613` / `gpt-4` /
`gpt-4-0125-preview` / `gpt-4-0613` / `gpt-4-o-preview` … `mai-code-2-flash`（**19 条 = T21 真机收录的 19 条**，
与 02 §13 v0.30 记的「真实上游 39 条 → 收录 19 条」一致）。
**改完已在副本里还原成 `enabled = 0`，用户真库未被触碰**（副本已删）。

**② ToS 确认弹窗**（`app-ui-t24-tos-dialog.png`）：

- 标题「启用 Copilot 渠道前请确认服务条款」；
- 高亮框内是**声明原文逐字**：「将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。」；
- 后果说明：「启用后，Copilot 的模型会出现在 `/v1/models`，harness 可以直接用你的 Copilot 额度发起请求。」；
- 「我已阅读并确认上述条款」勾选框 + 「取消」/「确认并启用」按钮（**未勾选时「确认并启用」是禁用的**）。

## 6. 真机阶段揪出的问题

| # | 问题 | 根因 | 处理 |
|---|---|---|---|
| 1 | **通用渠道表单根本存不了 Copilot 渠道** | `commands::channel::check_provider` 只认 7 个预设 + `custom`，`copilot` 不在其中 → 表单保存报「未知的渠道预设 "copilot"」 | **反向印证了设计**：启用开关只能放 Copilot 面板（通用表单里根本改不动它）。**不改 `check_provider`**（超范围，且放开会让通用表单能改 OAuth 渠道，风险更高） |
| 2 | **`PrintWindow` 拍不到 `position: fixed` 的浮层** | 弹窗走 GPU 合成层，`PrintWindow` 只拿到主层 | **新增 `scripts/take_screenshot_screen.ps1`**：屏幕实拍（窗口摆到屏幕左侧避开别的窗口 + 抢前台）。**弹窗类浮层一律用屏幕实拍** |
| 3 | **`.ps1` 改成 UTF-8 无 BOM 后 PowerShell 报语法错误** | PowerShell 5.1 按系统 ANSI（GB2312）读脚本 → 中文注释里的字节**吞掉了 3 个换行** → `}` 结构断裂 | **`.ps1` 一律存成 UTF-8 带 BOM**（`new UTF8Encoding($true)`）；已对本轮两个脚本执行 |
| 4 | **抢前台时好时坏** | 用户自己的 IDE 会一直占焦点 | **三招叠加**：ALT 敲一下（放开前台锁）+ `AllowSetForegroundWindow` + `AttachThreadInput`；抢到前台后 `SendKeys {PGDN}` 才能滚动 |
| 5 | **UI 上出现裸的 markdown 星号** | 文案里写了 `**默认关闭**`，但应用**不渲染 markdown** | 去掉记号（`tos.ts` / `copilot.ts` 两处） |

## 7. 已知限制与未验证项

- **WorkBuddy 半边不做**（T23 延后到付费后，见 HANDOFF §5 第 10 行）。后置时**只加一行清单 + 一段声明**，不改本节结构。
- **不做「撤回确认」**（用户确认过就一直算数，除非删渠道重建）。
- **不做「每启必弹」**：确认过一次后再开关**不再弹框**（02 §6.8.5 的口径）。
- **真实上游未验**：本卡的 e2e 上游是 `127.0.0.1` 的 mock。**「真实 Copilot 账号 + 真实模型目录 + 真实 `/v1/models`」这条链没验过** —— 但**「启用后模型出现在 `/v1/models`」这一点**已在本机真应用上验证（渠道行里 31 个模型、模型预览显示 0 个别名是因为渠道关着），并用**真实数据副本**实测过前后对比（0 → 19 条）。
- **通用渠道面板没有做「受限渠道」提示**：`check_provider` 本来就拒绝 `copilot`（问题 #1），
  通用表单里根本见不到 Copilot 行 → **不需要额外提示**。后端闸门仍然守住了 store 层与新 IPC 两条路。

## 8. 回归

| 项 | 结果 |
|---|---|
| `cargo test --all-targets` | **539 通过 / 0 失败 / 4 忽略**（26 个测试二进制；lib 436 + e2e 103） |
| 前端单测（`node --experimental-strip-types --test`） | **79/79**（新增 8 条） |
| `npm run build`（`tsc && vite build`） | **40 modules / gzip 70.01 kB**，零类型错误 |

## 9. 留档索引

| 内容 | 位置 |
|---|---|
| 设计口径 | 02 **§6.8**（§6.8.1~§6.8.5）+ §13 **v0.35** |
| 任务卡 | 03 §2 **T24 卡片**（目标 / 产出 / 验收判据 / 边界 / 完成状态） |
| M4 判据③ | 03 §1 **M4 里程碑**（判据③ 对 Copilot 已闭环；② 仍待付费后） |
| 后端 | `src-tauri/src/providers/compliance.rs`、`src-tauri/src/store/channels.rs`、`src-tauri/src/commands/{channel,copilot}.rs` |
| 前端 | `src/copilot/{tos.ts,CopilotPanel.tsx,copilot.ts}`、`src/ui/Modal.tsx`、`src/App.css` |
| 测试 | `src-tauri/tests/compliance_e2e.rs`、`src/copilot/tos.test.ts`、`src-tauri/src/store/channels.rs`（单测） |
| 真机截图 | `docs/reviews/app-ui-t24-enable.png`、`docs/reviews/app-ui-t24-tos-dialog.png` |
| 取证工具 | `scripts/take_screenshot.ps1`（窗口抓取）、`scripts/take_screenshot_screen.ps1`（**屏幕实拍，浮层用这个**） |

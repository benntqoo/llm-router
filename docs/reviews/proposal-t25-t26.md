# 提案 T25 / T26 — 签到通用化 + UI 功能分组

| 项 | 值 |
|---|---|
| 日期 | 2026-11-28 |
| 来源 | 头头指令：「现在先完成签到功能，很多 agent harness 都可以有签到领积分 或是平台可以领积分使用，这部份要先实现起来；然后当前 UIUX 就是全挤在一起需要根据功能分类有 tab 可以切换方便快速查看才对；这部份需要重新评估调整开发计划」 |
| 性质 | **设计变更提案**（AGENT.md §6-3：提案 → 头头批准 → 更新 02 → 才可动代码） |
| 涉及卡片 | T25（签到通用化）、T26（UI 功能分组） |
| 阻塞关系 | **两件事都不被 WorkBuddy 付费前提阻塞**（HANDOFF §5 第 10 行只卡 T23/T24 的 WorkBuddy 半边） |

---

## 0. 结论（先行）

1. **签到**：**框架不缺，缺的是「轻量接入通道」**。引擎层（注入 / 重放 / 断言 / 8 类分类）、调度层（cron + 系统兜底）、通知层都已是通用且已验证；真正卡住「很多平台都能签到」的是**新增一家平台的边际成本**——现状要么写 Rust 插件（要发版），要么抓 HAR（要 DevTools + 逐条标变量 + 还没有断言编辑 UI）。
   → **建议新增「声明式站点（spec）+ 站点目录（catalog）」机制**：新增一家平台 = 填一份 JSON（或导入一个目录文件），**不改一行 Rust**。
2. **UI**：8 个面板单页平铺（`src/App.tsx:91-105`），页面纵向极长，找功能靠滚。
   → **建议顶部 tab 分组（4 组）+ 网关状态压缩成常驻状态条**。
3. 两件事**互不依赖**，可并行推进；都不产生新的 schema 变更。

---

## 1. 现状评估（已验证，2026-11-28 实查代码）

| 层 | 现状 | 证据（文件:行号） |
|---|---|---|
| 引擎层 | `inject` 变量注入 → `replay` 发请求 → `check_assertion` 断言 → `evaluate` 分类；**8 类 `ReplayClass`** 统一结果语义 | `src-tauri/src/checkin/replay.rs:469`、`:345`、`:35` |
| 模板模型 | `SiteTemplate`（method/url/headers/query/postData + variables + assert）**已经是通用的 HTTP 模板模型** | `src-tauri/src/checkin/har.rs:289`、`:266` |
| 站点层 | `SitePlugin` trait 可插拔，但 `builtin()` **只认 2 个**（`modelscope` / `minimax_code`），`builtin_catalog()` 也只有 2 项 | `src-tauri/src/checkin/plugin.rs:85`、`:133`、`:144` |
| 用户自建层 | **只有 HAR 导入向导一条路**：抓包 → 标记变量 → 保存；**没有断言编辑 UI**（T16/T17 留档已知限制） | `src/checkin/CheckinPanel.tsx:942`（导入向导）、`docs/reviews/T16-replay-engine.md` |
| 调度层 | cron 调度 + 错过补签 + 同周期去重 + Windows 计划任务 / macOS launchd 兜底 | `src-tauri/src/checkin/scheduler.rs`、`checkin/system.rs` |
| 通知层 | `checkin::notify` + `checkin::system_notify` 覆盖全部执行路径 | `src-tauri/src/checkin/notify.rs`、`system_notify.rs` |
| 积分层 | `Reward{amount, unit, raw}`，`raw` 只保留数值/布尔（密文不过界） | `src-tauri/src/checkin/reward.rs:13` |
| 积分缺口 | **HAR 重放恒为 `reward = None`**——只有内置插件能提取积分 | `src-tauri/src/checkin/replay.rs:141` |
| 分派点 | `scheduler::execute` 按 `site.plugin` 分派（`har` → 重放引擎 / 内置 → `sites::*`） | `src-tauri/src/checkin/scheduler.rs:194-231` |
| 存储层 | `checkin_site` 表已含 `plugin` / `config_json` / `schedule` / `enabled` → **声明式站点可复用，无须改 schema** | `src-tauri/src/store/checkin.rs:20`、`:15` |

**规模**：Rust 75 个源文件、前端 29 个 ts/tsx、IPC 58 个、前端面板 8 个。

---

## 2. 问题定义（把诉求翻译成可验收的技术问题）

### 2.1 「很多 agent harness / 平台都要能签到领积分」

- **表面诉求**：多接几个站点。
- **真问题**：**新增一家平台的边际成本**。现状两种路径：
  - 路径 A（内置插件）：写一个 Rust 模块 + 注册 + 发版 → 边际成本 ≈ 0.5–1 人日，**且每加一家都要发版**；
  - 路径 B（HAR 模板）：用户抓包 + 逐条标变量 + 遇到断言不对还要手改 JSON（**无 UI**）→ 边际成本高、失败率高。
- **目标**：边际成本降到「**填一份 JSON / 导入一个目录文件**」，且**断言与积分提取规则随定义走**（不用用户手改）。
- **附带的第二半**：诉求里的「**平台可以领积分使用**」——现在系统只有「本次拿到了多少」的快照（`Reward`），**没有「当前账户余额」**。余额是「积分能不能用」的直接依据，属于同一张卡的合理范围。

### 2.2 「UI 全挤在一起」

- **表面诉求**：加 tab。
- **真问题**：8 个功能域平铺在一页 → ① 首屏被「网关状态 / 快速开始 / 当前范围」三张卡片占掉；② 找功能靠纵向滚动；③ 无位置感（用户不知道「熔断」和「用量」是两件事还是一件事）。
- **目标**：按**功能域**分组 + tab 切换 + **上次选中项持久化**（重开应用还在原地）+ 网关状态常驻可见（不能因为切 tab 就看不到「网关在不在跑」）。

---

## 3. 方案对比

### 3.1 签到通用化（三选一）

| 维度 | A：扩 trait 多写插件 | **B：声明式 spec + 站点目录** | C：纯手工 HTTP（无目录） |
|---|---|---|---|
| 新增一家平台成本 | 写 Rust + 发版（≈0.5–1 人日） | **填/导一份 JSON（≈2 分钟）** | 填 URL + 断言（≈5 分钟） |
| 要不要改代码 | **要**（每家都改） | **不要** | 不要 |
| 断言表达力 | 最强（可写任意逻辑） | 状态码 + JSON 路径断言（够用） | 同 B |
| 积分提取 | 可做 | **可做**（声明式路径） | 可做 |
| 分发给他人 | 只能随版本 | **目录文件可导出/导入** | 只能手抄 |
| 与现有代码的关系 | 沿用 `SitePlugin` | **复用 `SiteTemplate` + 现有引擎**（注入/断言/分类一行不改） | 同 B |

**建议：选 B，并把 A 保留为「官方高质量站点」的补充路径**（ModelScope / MiniMax Code 这类断言复杂的仍走代码插件；目录里以「内置条目」形式列出，用户不必重新填）。

**B 的关键设计**：`spec`（声明式站点定义）**编译成现有的 `SiteTemplate`**，再交给**已验证的重放引擎**执行——于是「零新引擎、零新分类、零新调度」，风险面只在「编译 + 积分提取 + 目录校验」三处。

### 3.2 UI 分组（三选一）

| 方案 | 分组 | 面板分布 | 评价 |
|---|---|---|---|
| **①（建议）** | 接入 / 凭证 / 运维 / 签到与积分 | 接入 3（渠道·模型目录·Copilot）｜凭证 2（保险库·Client Key）｜运维 2（熔断·用量）｜签到 1 | 4 个 tab 平衡，**默认落在「接入」**（新用户第一件事就是配渠道） |
| ② | 接入 / Harness 反代 / 凭证 / 运维 / 签到 | 接入 2｜Harness 1｜凭证 2｜运维 2｜签到 1 | Copilot 独立成组更醒目，但多一个 tab；**WorkBuddy 半边延后**后该组会长期只有 1 个面板 |
| ③ | 每面板一个 tab（8 个） | 各 1 | 改动最小，但 tab 太多反而成「另一张长列表」 |

**建议 ①**。Copilot 面板本身是「第三方 harness 接入」，归入「接入」语义成立；WorkBuddy 落地后同组再加，**不新增 tab**。

---

## 4. 设计要点（**待批准后才写进 02**）

### 4.1 声明式站点（spec）结构草案

```jsonc
{
  "id": "my-platform",            // 目录内唯一 id
  "name": "我的平台",              // UI 展示名
  "method": "POST",
  "url": "https://example.com/api/checkin",
  "headers": [ { "name": "Authorization", "value": "Bearer {{token}}" } ],
  "query":   [ { "name": "day", "value": "{{today}}" } ],
  "body":    { "mimeType": "application/json", "text": "{\"type\":\"daily\"}" },
  "credentialId": "cred_xxx",     // 敏感值来源 = 保险库里的凭据 id（真实值不出模板）
  "assert":  { "status": 200, "jsonPaths": [ { "path": "data.ok", "equals": true } ] },
  "reward":  { "path": "data.credits", "unit": "point" },        // 可选：本次拿到多少
  "balance": { "method": "GET", "url": "https://example.com/api/balance",
               "path": "data.balance", "unit": "point" }           // 可选：当前余额
}
```

- **占位符**：只有 `{{token}}`（敏感，取保险库）与 `{{today}}`（当天日期，非敏感）两种——**不做任意表达式**（避免变成脚本引擎）。
- **编译**：spec → `SiteTemplate`（`variables` 里 `token` 标记为 sensitive + `credentialId`，`today` 为默认值变量）→ **现有 `inject` / `evaluate` 直接跑**。
- **落库**：`checkin_site.plugin = "spec"`，`config_json` = spec JSON。**零 schema 变更。**

### 4.2 目录（catalog）

- 一个 catalog 文件 = `{"version":1, "sites":[ spec, … ]}`。
- **导入**：逐条校验 → 逐条建站（**默认 `enabled = false`**，与现有「导入的站点默认关」一致）→ 返回导入结果（成功数 / 失败原因）。
- **导出**：把自建站点导出成 catalog 文件（**不含真实凭据**，只有 `credentialId`）→ 可分享给别的 harness 用户。
- **内置目录**：首版只收**已实测**的条目（ModelScope / MiniMax Code 走代码插件）+ 一个**空白模板**（通用 HTTP 签到，用户填 URL 即可）。
  **不虚构 endpoint**：任何「听说的」平台（智谱 / 硅基流动 / 讯飞等）**不进内置目录**，只进 backlog，等实测后再收。

### 4.3 积分：本次拿到 vs 当前余额

| 概念 | 口径 | 落点 |
|---|---|---|
| **本次拿到** | 这一次签到**入账了多少**（`Reward`，`raw` 只留数值/布尔） | `checkin_result.reward_json` + `account.quota_json` |
| **当前余额** | 平台账户**现在有多少**（spec.balance 声明式查询） | 只回前端展示，**不落 `quota_json`**（避免与限额自学习字段打架） |

UI 文案严格区分：「本次拿到 400 积分」 vs 「当前余额 1,200 积分」，**不混用「余额」二字**。

### 4.4 UI 结构

```
┌ 顶栏：LLM Router   [网关状态 ● 运行中  http://127.0.0.1:xxxx/v1] ┐
├ tab：接入 │ 凭证 │ 运维 │ 签到与积分 ───────────────────────────┤
│                                                              │
│  <当前 tab 的面板>                                            │
│                                                              │
├ 底栏：当前范围（折叠）                                          ┘
```

- 「快速开始（从零接入三步）」**移入「接入」组**首卡（它讲的就是配渠道）。
- 「当前范围」**移到底部折叠区**（它是历史说明，不是操作入口）。
- tab 选中项写 `localStorage`（键：`llm-router.activeTab`）→ 重开应用回到原地。
- **一次只渲染当前 tab 的面板**（不做全渲染 + 隐藏）→ 首屏 DOM 显著变小。

---

## 5. 影响面

**后端（新增/改动）**

| 位置 | 改动 |
|---|---|
| `checkin/spec.rs`（**新**） | spec 数据模型 + 校验 + **编译成 `SiteTemplate`** + 积分/余额路径提取 |
| `checkin/catalog.rs`（**新**） | catalog 文件模型 + 导入/导出 + 内置目录 |
| `checkin/scheduler.rs` | 分派点加一支：`plugin == "spec"` → 编译 → 现有引擎 → 取 reward |
| `checkin/plugin.rs` | `pluginLabel` 等前端映射补 `spec`；`PluginSiteConfig` 增 `spec` 相关字段 |
| `commands/checkin.rs` | 新 IPC：`checkin_catalog_list` / `checkin_add_spec_site` / `checkin_import_catalog` / `checkin_export_catalog` / `checkin_balance` |
| `lib.rs` | 注册新 IPC |
| **schema** | **不变**（复用 `checkin_site`） |

**前端**

| 位置 | 改动 |
|---|---|
| `src/tabs.ts`（**新**） | tab 分组定义（纯数据，可单测） |
| `src/App.tsx` | 状态条 + tab 栏 + 按 tab 渲染 + localStorage 记忆 |
| `src/App.css` | tab 栏 / 状态条样式 |
| `src/checkin/CheckinPanel.tsx` | 新增「站点目录」区块（列目录 / 导入 / 导出 / 声明式新增 / 余额查询） |
| `src/checkin/types.ts`、`plugin.ts` | 对齐后端新形状 |

**文档**：02（新增站点目录设计 + UI 分组口径 + §13 版本）、03（T25 / T26 卡片 + 里程碑 + 排期 + 风险）、HANDOFF（§4 执行入口 + §8 账本）。

**不做**：不改 `checkin_site` / `checkin_result` 表结构；不动重放引擎的分类与断言语义；不引入第三方依赖。

---

## 6. 验收判据（草案）

**T25 签到通用化**

1. 一份手写 spec（指向**本地 mock 上游**）→ 建站 → 「立即执行一次」→ 成功并**拿到积分**（`reward.amount` 正确、`raw` 无字符串）。
2. spec 的 JSON 路径断言不成立 → 判 `assert_failed`（**不静默当成功**）。
3. 缺凭据 → 判 `credential_missing`，**且上游请求数不增加**（沿用 T16 口径）。
4. catalog 导入：2 条有效 + 1 条非法 → 导入 2 条、**非法那条带原因不静默丢弃**、**默认 `enabled = false`**。
5. catalog 导出 → 再导入，**站点数一致**（幂等）。
6. **零明文**：spec / catalog 文件 / `store.db` / `vault.bin` 四层扫描**无凭据明文**（`credentialId` 只是 id）。
7. 余额查询：spec 带 `balance` → 查到当前余额；**文案与「本次拿到」严格区分**。

**T26 UI 分组**

8. 4 个 tab 切换，**每个 tab 只渲染自己的面板**（DOM 里不出现其它组的面板）。
9. 切 tab 后刷新应用，**仍停在同一个 tab**（localStorage）。
10. 网关状态**在任意 tab 都可见**（顶栏常驻）。
11. 前端单测 + `npm run build` 通过；Rust 全量回归 0 失败。

---

## 7. 边界（明确不做）

- **不做**验证码绕过 / 指纹伪造 / 签名伪造（AGENT.md §5.7）——spec 撞上验证码 = 失败并通知。
- **不做**任意表达式 / 脚本化的 spec（占位符只有 `{{token}}` 与 `{{today}}`）。
- **不内置未实测的 endpoint**（内置目录只收已实测条目 + 空白模板）。
- **不改任何 harness 配置**（AGENT.md §5.2）。
- **不改 schema**、不动重放引擎的分类与断言语义。
- 不做多账号切换器、不做代理池。

---

## 8. 待头头拍板（两条）

| # | 决策点 | 建议 | 备选 |
|---|---|---|---|
| Q1 | UI 分组方案 | **方案 ①：接入 / 凭证 / 运维 / 签到与积分（4 组）** | 方案 ②（Copilot 独立成组，5 组）｜方案 ③（8 个面板各一组） |
| Q2 | 签到通用化边界 | **方案 B：声明式 spec + 目录导入导出 + 余额查询**；内置目录只收已实测条目 | B′（B 但不做余额查询）｜B″（B 且预置一批未实测的候选平台条目） |

> **批准口径**：头头回复后 → 更新 02（设计）→ 更新 03（卡片）→ 才动代码（AGENT.md §6-3）。

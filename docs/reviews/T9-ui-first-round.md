# T9 验收留档 — UI 第一轮（渠道列表/编辑、密钥回看 30s 遮蔽、别名管理、client key 管理、`/v1/models` 预览）

| 项 | 值 |
|---|---|
| 任务 | T9 UI 第一轮（03 §2 任务卡；关键路径 T9→T10） |
| 依据 | `docs/planning/02-technical-design.md` §5.2 / §6.1 / §6.5 / §6.6（v0.8）；`docs/planning/03-delivery-plan.md` T9 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `e70df27`（`feat: T9 UI 第一轮（渠道列表/编辑 + 别名管理 + 密钥回看联动 + client key 管理 + /v1/models 预览）`）；文档提交另计 |
| 结论 | **通过**（任务卡验收判据实测 **7.1 秒**，远低于 3 分钟上限；五项目标全部落地并留证） |

> 本文件改动范围：仅记录 T9 的验收判据、实测证据、交付物与取舍；**不含**任何后续任务的结论。

## 1. 判据对照

任务卡验收原文：「从零配置一个 DeepSeek 渠道 + client key ≤ 3 分钟。」

| # | 判据 / 目标 | 实测 | 证据 |
|---|---|---|---|
| 1 | **从零配置 ≤ 3 分钟** | ✅ | §2.3：**脚本驱动墙钟 7.1 秒**（含每次操作 220–700ms 的自动化等待）；人工路径只有 **1 次粘贴 + 3 次点击 + 1 次滚动** |
| 2 | **渠道列表 / 编辑** | ✅ | §2.4：渠道列表显示 provider / 名称 / base_url / 模型映射 / 密钥状态；「编辑」把整条渠道（含全部别名、优先级、启用态、meta）载入表单，保存后原地更新 |
| 3 | **密钥录入与回看（30s 遮蔽）** | ✅ | §2.4：录入后保险库显示该渠道的凭据；点「显示」明文可见并带倒计时，**33 秒后再看已自动遮蔽** |
| 4 | **模型别名管理** | ✅ | §2.4：一个渠道挂 **两条**别名（`deepseek-chat` + `deepseek-reasoner`），两条都出现在 `/v1/models`；可增行、删行、逐条启用/禁用 |
| 5 | **client key 管理** | ✅ | §2.4：创建（明文只显示一次 + 一键复制）、**标签与限额编辑**（`pi-主力` / 60 rpm / 200000 tokens/日 落库）、删除 |
| 6 | **`/v1/models` 预览** | ✅ | §2.4：「模型预览」面板列出 2 个别名 + base_url + 接入片段；**与网关同源**（e2e 断言两者集合相等） |
| 7 | 凭证零明文（AGENT.md §5） | ✅ | §2.5：库与 WAL 全文件扫描，API Key 明文与 client key 明文**均无命中** |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 238 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 7 passed;  0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/channel_e2e.rs（T8 + T9 新增 3 条）
test result: ok. 5 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs
test result: ok. 6 passed;  0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/outbound_e2e.rs（T4）
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/router_e2e.rs（T5）
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/stream_e2e.rs（T6）
test result: ok. 5 passed;  0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/usage_e2e.rs（T7）
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs（T2）
```

编译告警：0。前端 `npm run build` 通过（`tsc && vite build`，24 modules）。

> 单元测试 228 → **238**（+10：`published_models` 1 + 渠道名唯一 2 + 预览 IPC 1 + vault 按渠道录入 3 + client key 限额 3）；集成测试 4 → **7**（T9 新增 3 条）。全量 **288** 项，0 失败。

### 2.2 新增用例

| 层 | 用例 | 断言要点 |
|---|---|---|
| 单测 | `published_models_only_include_enabled_entries` | 只有**模型与渠道都启用**的条目对外发布；任一被禁即隐藏 |
| 单测 | `duplicate_channel_name_is_rejected_with_readable_error` | 同名渠道被拒，且错误是**可读中文**（不是 UNIQUE 约束原文） |
| 单测 | `updating_channel_may_keep_its_own_name` | 编辑同一条渠道（名字不变、别名 +1）必须成功 |
| 单测 | `models_preview_lists_published_aliases` | 预览返回 base_url + 别名 + 归属渠道名 |
| 单测 | `put_secret_for_channel_attaches_to_that_channel` | 凭证精确挂到指定渠道，**不新建**渠道 |
| 单测 | `put_secret_for_unknown_channel_is_rejected_without_creating_placeholder` | 未知渠道报错且**渠道数不变**（不造占位渠道） |
| 单测 | `put_secret_for_channel_rejects_empty_secret` | 空密钥 / 空 id 拒绝 |
| 单测 | `update_meta_changes_label_and_limits` | client key 的标签与限额可改并落库 |
| 单测 | `update_meta_rejects_negative_limits_and_unknown_id` | 负限额拒绝；未知 id 返回 false |
| 单测 | `update_meta_falls_back_to_default_label` | 空标签回落「未命名」 |
| e2e | `acceptance_models_preview_matches_gateway_list` | **UI 预览集合 == 网关 `/v1/models` 集合** |
| e2e | `acceptance_one_channel_can_publish_multiple_aliases` | 一个渠道两条别名都在 `/v1/models` |
| e2e | `acceptance_disabled_channel_is_hidden_from_models` | 禁用渠道 → 网关与 UI 预览同时隐藏 |

### 2.3 真机验收：从零配置（**计时**）

环境：应用用 `LLMR_STORE_PATH` / `LLMR_VAULT_PATH` 指向**全新空目录**（`%TEMP%\llmr-t9p3-verify\`，库与保险库都是新建的）；网关 `127.0.0.1:8787`；前端 `npm run dev`（Vite 1420）。UI 操作全部为**真实鼠标键盘**（`SetCursorPos` + `mouse_event` + 剪贴板粘贴）。

**人工路径（5 个动作）**：

| # | 动作 | 说明 |
|---|---|---|
| 1 | 打开应用 | 渠道面板**自动预填** DeepSeek 官方 + `deepseek-chat`（默认预设自动应用），无需选预设 |
| 2 | 在「API Key」框粘贴密钥 | 唯一需要输入的东西 |
| 3 | 点「保存渠道」 | → 提示「已保存渠道「DeepSeek 官方」，别名 deepseek-chat 已出现在 /v1/models（密钥已加密入库）」 |
| 4 | 滚到「Client Key」面板，点「创建 client key」 | 明文只显示一次 |
| 5 | 点「复制」 | 剪贴板拿到明文 key |

**计时结果**：

```
ELAPSED_MS=7149
ELAPSED_SEC=7.1
```

> 计时口径：从"用户开始配置"（应用已打开）到"client key 创建完成"，**含每次点击 220–350ms、每次粘贴 200+150+350ms、每次滚轮 300+500ms 的自动化等待**。也就是说 7.1 秒是**含自动化开销的上界**，人工操作只会更快 —— 距 3 分钟上限有 25 倍余量。

**配置结果（库内）**：

```
channels:      [('deepseek', 'DeepSeek 官方', 'https://api.deepseek.com', 1)]
models:        [('deepseek-chat', 'deepseek-chat', 1)]
credentials:   [('cred_a530ad66738', 'ch_14bdd5e…', 'api_key', 36)]   # 36 字节密文
client_key:    [('ck_20ff5065…', 'pi', 'llmr_xN0')]
```

**配置后立刻可用**（用刚创建的 client key 请求网关）：

```
GET /v1/models -> {"object":"list","data":[
  {"id":"deepseek-chat","object":"model","created":0,"owned_by":"DeepSeek 官方"}]}
```

### 2.4 真机验收：五项目标逐条实测

| 项 | 实测动作与结果 |
|---|---|
| **渠道列表 / 编辑** | 点「编辑」→ 表单载入「DeepSeek 官方 / 优先级 100 / 启用 / 两个别名 / 路径 / 鉴权」→ 出现「取消编辑」按钮、保存按钮变「保存修改」→ 保存后 `channel.id` **不变**（原地更新，不是新建） |
| **密钥回看 30s 遮蔽** | 保险库「已存凭据」显示 `api_key · DeepSeek 官方 · 2026/11/26 12:32:08`；点「显示」→ 明文可见 + 「（30s 后自动遮蔽）」；**33 秒后再看：明文行消失，只剩「显示 / 删除」按钮** |
| **模型别名管理** | 「+ 添加别名」→ 填 `deepseek-reasoner` → 保存 → `/v1/models` 返回两条：`deepseek-chat` 与 `deepseek-reasoner`，`owned_by` 都是「DeepSeek 官方」；库内 `model` 两行 |
| **client key 管理** | 「编辑」→ 标签改 `pi-主力`、rpm `60`、每日 tokens `200000` → 保存 → 库内 `client_key` 行 = `('pi-主力', 60, 200000)`；列表行同步显示 `· 60 rpm · 200000 tokens/日` |
| **`/v1/models` 预览** | 「模型预览」面板：`2 个别名 · base_url http://127.0.0.1:8787/v1`，列出 `deepseek-chat · DeepSeek 官方`、`deepseek-reasoner · DeepSeek 官方`，各有「复制别名」；下方「客户端这样填」给出 base_url / api_key / model 三行片段 |
| **渠道禁用** | 取消勾选「启用该渠道」→ 保存 → `GET /v1/models` 返回 `{"data":[]}`，预览面板同步清空；重新勾选后恢复 |

截图证据：`docs/reviews/app-ui-t9-channels.png`（渠道表单两个别名 + 保存提示 + 已建渠道 + 保险库）、`docs/reviews/app-ui-t9-preview.png`（client key 限额 + 模型预览两个别名）。

### 2.5 凭证零明文复查（AGENT.md §5 / §9 相关项）

扫描对象含 **WAL 文件**（SQLite 的新数据先落在 `store.db-wal` 里，只扫主库会漏）：

```
store.db     size = 4096    | 明文命中: 无
store.db-shm size = 32768   | 明文命中: 无
store.db-wal size = 313152  | 明文命中: ['DeepSeek']      # ← 渠道名，业务字段，非凭证
vault.bin    size = 596     | 明文命中: 无（无 sk- / Bearer / llmr_）
```

- API Key 明文（本轮自造的 mock 假值 `sk-t9-****`）：**全部文件无命中** → 凭证确实由 vault 加密。
- client key 明文：**全部文件无命中** → 只存 SHA-256 哈希（02 §3.1）。
- 唯一命中项 `DeepSeek` 是**渠道名**（`channel.name` / `model.alias`），是业务字段不是凭证 —— 与账本里那条 🔴「AGENT.md §9 正则口径太宽」是同类问题。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/store/channels.rs` | 新增 `PublishedModel` + `list_published_models`（**对外发布别名的唯一查询**）；`save_channel` 增加渠道名唯一自查（可读报错）与 `enabled` / `priority` 的编辑支持 |
| `src-tauri/src/gateway/server.rs` | `GET /v1/models` 改用 `store::channels::list_published_models`（网关与 UI 预览同源） |
| `src-tauri/src/commands/channel.rs` | 新增 `models_preview` IPC（`ModelsPreview { base_url, models }`）与可测版本 `models_preview_of` |
| `src-tauri/src/commands/vault.rs` | 新增 `vault_put_secret_for_channel` / `vault_list_secrets_for_channel`（按 **channel_id** 精确挂载，不再按名字兜底建占位渠道） |
| `src-tauri/src/commands/client_key.rs` | `client_key_list` 拆出可测版本 `client_key_list_of`；新增 `client_key_update_meta`（标签 + rpm + 每日 tokens，负数拒绝） |
| `src-tauri/src/lib.rs` | 注册 4 个新 IPC |
| `src-tauri/tests/channel_e2e.rs` | 新增 3 条 e2e（预览同源 / 多别名 / 禁用隐藏） |
| `src/ChannelPanel.tsx` | **模型别名多行管理**（增行/删行/逐条启用）+ 渠道启用开关 + 优先级 + 保存后进入编辑态 |
| `src/VaultPanel.tsx` | 录入密钥改为**下拉选已有渠道**（不再自由文本）；凭据列表显示归属渠道名；加「刷新」按钮 |
| `src/ClientKeyPanel.tsx` | 标签与限额编辑 + 明文「复制」按钮 |
| `src/ModelsPanel.tsx`（新） | `/v1/models` 预览（与网关同源）+ 接入片段 |
| `src/panels/bus.ts`（新） | 跨面板数据变更通知（轻量 `CustomEvent`，无第三方依赖） |
| `src/App.tsx`、`src/App.css` | 挂载 `ModelsPanel`；新增「快速开始（从零接入三步）」卡片；`.alias-block` / `.steps` 样式 |
| `docs/planning/02-technical-design.md` | §6.5 注明 pi 实连归 T10；**新增 §6.6 应用内 UI 面板口径**（含两条防漂移口径）；§13 变更记录（v0.8） |

无新增依赖。

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| **模型预览与网关同源** | UI 预览不再自己写一份查询，而是复用 `store::channels::list_published_models`（网关也改成调它）。两份查询必然漂移 —— 用户在 UI 看到、客户端却调不到。e2e 用集合相等断言锁住 |
| **跨面板数据变更要广播** | 渠道面板存了密钥，保险库与模型预览必须立刻跟着变。实现：事件总线（`src/panels/bus.ts`）+ 5s 轮询兜底（漏通知也能自愈） |
| **按 channel_id 挂凭证** | 原 `vault_put_secret(channel_name)` 会按名字兜底建占位渠道（名字打错就多一个垃圾渠道）。T9 起 UI 一律走 `vault_put_secret_for_channel(channel_id)`，旧命令保留不动 |
| **保存后进入编辑态** | 新建渠道成功后 `editingId = saved.id`：再改别名、再保存是**更新**同一条渠道，而不是新建同名渠道（那会撞 `UNIQUE(channel.name)`） |
| **渠道名唯一给可读报错** | 后端先自查再写，错误是「渠道名已存在：X」，而不是把 `UNIQUE constraint failed: channel.name` 抛给用户 |
| **别名全局唯一** | 沿用 T8 口径（`idx_model_alias`）：被别家渠道占用直接报错，不自动改名（避免客户端手里的名字失效） |
| **表单里删掉的模型 → 置 disabled** | 沿用 T8 口径：避免物理删除连带删掉 `route_member` 编排关系；disabled 的模型不发布到 `/v1/models` |
| **client key 明文只显示一次** | 沿用 02 §3.1：client key 是本地口令，没有"只显示一次"痛点；UI 给了复制按钮，但**不做回看** |
| **密钥回看仍走 vault** | 沿用 02 §5.2：唯一返回明文的 IPC 是 `vault_reveal_secret`，30s 自动遮蔽（前端计时，到期把明文从状态里移除） |
| **pi 接入片段不写死** | 02 §6.5 的 pi CLI 配置格式**仍未验证**，T9 只给通用三行片段；pi 实连与片段归 **T10** |

### 上报与裁决

1. **02 §6.6 是本轮新固化的两条口径**（模型预览与网关同源、跨面板变更要广播）。它们不是新功能，而是把 T9 落地时踩到的两个坑写成规则，防止后续实现走偏。已回填 02（v0.8）。
2. **默认模型名仍是"未实测"建议值**（02 §6.1.1）—— T9 没动这一条；真实可用性归 T10 实测矩阵。
3. **HANDOFF §5 的未验证项本轮未动**（MiniMax GroupId 三口径、各渠道 base_url 复核、tools 兼容度）—— 都归 T10。

## 5. 未做（范围外）

- **pi CLI 实连与配置片段** —— T10（02 §6.5）
- **A 类 7 列实测矩阵**（含 `tools` 兼容度、`tool_support` 标 `stable`/`partial`）—— T10
- **路由组编排 UI**（`route_group` / `route_member` 编辑）—— 未排期；T5 的数据层已就绪
- **别名重命名的历史映射**（改名后旧别名被置 disabled，不做映射表）—— 未排期
- **用量聚合与图表 UI** —— T14
- **安全验收四条命令成册** —— T11

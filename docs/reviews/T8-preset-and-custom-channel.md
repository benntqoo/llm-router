# T8 验收留档 — A 类预设 + 自定义 OpenAI 兼容渠道

| 项 | 值 |
|---|---|
| 任务 | T8 A 类预设 + 自定义渠道（03 §2 任务卡；关键路径 T8→T10） |
| 依据 | `docs/planning/02-technical-design.md` §6.1 / §6.1.1 / §6.1.2 / §9（v0.7）；`docs/planning/03-delivery-plan.md` T8 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `a340a29`（`feat: T8 A 类预设 + 自定义渠道（7 个 profile 接入 UI + 自定义 OpenAI 兼容渠道表单）`）；文档提交另计 |
| 结论 | **通过**（任务卡两条判据均实测留证，见 §1） |

> 本文件改动范围：仅记录 T8 的验收判据、实测证据、交付物与取舍；**不含**任何后续任务的结论。

## 1. 判据对照

任务卡验收原文：「预设渠道选中后仅剩"填 key"一步；自定义渠道可保存并出现在 `/v1/models`。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **预设渠道选中后仅剩"填 key"一步** | ✅ | §2.3 预设路径：选「DeepSeek 官方」后 base_url / 路径 / 鉴权样式 / include_usage / 渠道名 / 模型别名 / 上游模型名 **全部自动预填**，表单里唯一的空项是 API Key；填完 key 保存成功 |
| 2 | **自定义渠道可保存** | ✅ | §2.3 自定义路径：base_url `http://127.0.0.1:18789` + 路径 `/v1/chat/completions` + 渠道名 `MyProxy` + 别名 `my-alias` + 上游模型 `my-model` + 密钥 → 保存成功（落库见 §2.4） |
| 3 | **自定义渠道出现在 `/v1/models`** | ✅ | §2.3：`GET /v1/models` → `{"id":"my-alias","object":"model","created":0,"owned_by":"MyProxy"}` |
| 4 | 预设渠道保存后也出现在 `/v1/models` | ✅ | §2.3：`GET /v1/models` → `{"id":"deepseek-chat",...,"owned_by":"DeepSeek 官方"}` |
| 5 | 7 个预设 profile 全部接入 UI | ✅ | §2.3：下拉框列出 DeepSeek / Kimi / GLM / MiniMax 国际 / MiniMax 国内 / 阿里百炼 / 硅基流动（截图 `app-ui-channel-presets.png`）；单测 `presets_expose_seven_profiles` |
| 6 | 自定义渠道**真的转发**到上游（不是只落库） | ✅ | §2.3：`POST /v1/chat/completions`（`model=my-alias`）→ 200，mock 上游收到 `/v1/chat/completions` 且 `Authorization: Bearer sk-t8-test-****` |
| 7 | 鉴权样式可切换（任务卡要求的表单字段） | ✅ | e2e `acceptance_custom_channel_forwards_to_upstream`：自定义样式下密钥进 `x-api-key`、`Authorization` 为空 |
| 8 | 非法输入被拒且**不落库** | ✅ | e2e `acceptance_invalid_channel_is_rejected_without_writing`；单测 `invalid_inputs_are_rejected` / `alias_owned_by_another_channel_is_rejected` / `duplicate_alias_in_one_form_is_rejected` |
| 9 | 凭证零明文（AGENT.md §5） | ✅ | §2.5：`store.db` 无 API Key 明文、无 client key 明文；`vault.bin` 无 `sk-` / `Bearer ` / `llmr_` |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 228 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 4 passed;  0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/channel_e2e.rs（T8，本次新增）
test result: ok. 5 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs
test result: ok. 6 passed;  0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/outbound_e2e.rs（T4）
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/router_e2e.rs（T5）
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/stream_e2e.rs（T6）
test result: ok. 5 passed;  0 failed; 0 ignored; 0 measured; 0 filtered out    # tests/usage_e2e.rs（T7）
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs（T2）
```

编译告警：0。前端 `npm run build` 通过（`tsc && vite build`，22 modules）。

> 单元测试 208 → **228**（+20：预设目录 6 + 渠道/模型 CRUD 10 + `meta_json` 覆盖 4）；集成测试新增 4 条（`channel_e2e.rs`）。全量 **275** 项，0 失败。

### 2.2 新增用例

| 层 | 用例 | 断言要点 |
|---|---|---|
| 单测 | `presets_expose_seven_profiles` | 7 个预设全部暴露给前端，且各带默认模型建议值 |
| 单测 | `input_from_preset_fills_defaults` | 预设 → 渠道入参：base_url / 路径 / 鉴权 / 默认模型自动带出 |
| 单测 | `meta_json_can_switch_to_custom_auth_header` | `auth_style=custom_header` + `auth_header_name` 生效 |
| 单测 | `meta_json_rejects_unknown_auth_style` | 未知鉴权样式 → `AdapterError::Config` |
| 单测 | `meta_json_null_values_mean_no_override` | `null` 一律解释为"不覆盖"（表单里的"没填"不是错） |
| 单测 | `save_creates_channel_and_model` / `saved_channel_appears_in_model_table` | 渠道行 + 模型行落库，alias ↔ upstream_id 对应 |
| 单测 | `update_replaces_meta_json_wholesale` | 编辑渠道 = `meta_json` **整份替换**（不与旧值合并） |
| 单测 | `removed_model_is_disabled_not_deleted` | 表单里删掉的模型**置 disabled**，不物理删除（保 `route_member`） |
| 单测 | `alias_owned_by_another_channel_is_rejected` | 别名全局唯一，被别家占用直接报错 |
| 单测 | `duplicate_alias_in_one_form_is_rejected` | 同一表单内重复别名报错 |
| 单测 | `invalid_inputs_are_rejected` | 渠道名为空 / base_url 非 http(s) / 路径不以 `/` 开头 / 零个模型 → 全拒 |
| 单测 | `delete_removes_channel_models_and_credentials` | 删渠道连带删模型与凭证 |
| 单测 | `list_returns_channels_with_models` / `meta_json_round_trips_through_channel_summary` | 列表回读含模型与 `meta_json` 逐字段一致 |
| 单测 | `unknown_provider_is_rejected` / `custom_provider_is_accepted` | provider 必须是已知预设或 `custom` |
| 单测 | `put_key_attaches_credential_to_the_channel` | 密钥走保险库并挂到渠道上（不进渠道行） |
| 单测 | `put_key_rejects_unknown_channel_and_empty_secret` | 空密钥 / 未知渠道 → 拒 |
| 单测 | `delete_removes_the_channel`（IPC 层） | 删除 IPC 生效 |
| e2e | `acceptance_custom_channel_appears_in_models_list` | 自定义渠道保存后出现在 `/v1/models` |
| e2e | `acceptance_preset_channel_saves_with_preset_defaults` | 预设渠道按预设默认值保存并出现在 `/v1/models` |
| e2e | `acceptance_custom_channel_forwards_to_upstream` | 自定义渠道真转发；自定义鉴权样式下密钥进 `x-api-key`、`Authorization` 为空 |
| e2e | `acceptance_invalid_channel_is_rejected_without_writing` | 非法渠道被拒且不落库 |

### 2.3 真机 UI 验收（应用实跑，`127.0.0.1:8787`）

环境：应用用 `LLMR_STORE_PATH` / `LLMR_VAULT_PATH` 指向临时目录（`%TEMP%\llmr-t8p-verify\`）跑，**不污染用户真实库**；mock 上游 `127.0.0.1:18789`（Python 标准库，记录收到的 path 与请求头，`GET /counts` 查看）；前端走 `npm run dev`（Vite 1420）。UI 操作全部为**真实鼠标键盘**（`SetCursorPos` + `mouse_event` + 剪贴板粘贴）。

**自定义渠道路径**

```
[PASS] UI 填写：base_url=http://127.0.0.1:18789 / 路径=/v1/chat/completions /
       渠道名=MyProxy / 模型别名=my-alias / 上游模型名=my-model / API Key=sk-t8-test-****（本轮自造的 mock 假值）
[PASS] 点「保存渠道」→ 提示「已保存渠道「MyProxy」，别名 my-alias 已出现在 /v1/models（密钥已加密入库）」
[PASS] GET /v1/models → {"object":"list","data":[{"id":"my-alias","object":"model","created":0,"owned_by":"MyProxy"}]}
[PASS] POST /v1/chat/completions (model=my-alias) → HTTP 200
[PASS] mock 上游收到 path=/v1/chat/completions，headers={"authorization":"Bearer sk-t8-test-****","x-api-key":""}
       （密钥是本轮自造的 mock 假值，指向 127.0.0.1:18789 的本地假上游，非真实凭证）
```

**预设渠道路径**

```
[PASS] UI 点「预设渠道」→ 下拉框列出 7 个 profile（截图 app-ui-channel-presets.png）
[PASS] 选「DeepSeek 官方 (deepseek)」→ 自动预填：
       base_url=https://api.deepseek.com / 路径=/v1/chat/completions /
       渠道名=DeepSeek 官方 / 模型别名=deepseek-chat / 上游模型名=deepseek-chat /
       鉴权样式=bearer / include_usage 注入=开 / 该预设不带 GroupId
[PASS] 表单唯一空项 = API Key（"仅剩填 key 一步"成立）
[PASS] 填 key → 点「保存渠道」→ 提示「已保存渠道「DeepSeek 官方」，别名 deepseek-chat 已出现在 /v1/models（密钥已加密入库）」
[PASS] GET /v1/models → {"object":"list","data":[
         {"id":"deepseek-chat","object":"model","created":0,"owned_by":"DeepSeek 官方"},
         {"id":"my-alias","object":"model","created":0,"owned_by":"MyProxy"}]}
```

截图证据：`docs/reviews/app-ui-channel.png`（渠道面板：预设预填 + 保存成功提示 + 两条已建渠道）、`docs/reviews/app-ui-channel-presets.png`（7 个预这下拉）。

> **真机抓到并修掉的一个 UX bug**：最初点「预设渠道」只清空表单、不带出预设默认值 → 保存时报"渠道名不能为空"，与"仅剩填 key 一步"直接冲突。修法：切到预设模式时 `setMode("preset") → resetForm() → applyPreset(selectedPreset ?? presets[0])`。**这是只有真机点一遍才会暴露的 bug，单测测不到。**

> 预设渠道**没有**打真实上游（需要真实 DeepSeek key）；"预设的 base_url / 默认模型名是否真的可用"属 02 §10.1 实测矩阵，归 T10。

### 2.4 库内证据（验收后 `channel` / `model` / `credential`）

`channel`（两行）：

| id | provider | name | base_url |
|---|---|---|---|
| `ch_5ff72bd1…` | `deepseek` | DeepSeek 官方 | `https://api.deepseek.com` |
| `ch_a2825037…` | `custom` | MyProxy | `http://127.0.0.1:18789` |

`meta_json`（DeepSeek 预设行，整份替换语义）：

```json
{"path_chat":"/v1/chat/completions","auth_style":"bearer","auth_header_name":null,
 "group_id":null,"supports_groupid_query":false,"inject_stream_usage":true,"extra_headers":{}}
```

`model`（两行，别名 → 上游模型名）：

| id | channel_id | upstream_id | alias | enabled |
|---|---|---|---|---|
| `m_21da6794…` | `ch_a2825037…` | `my-model` | `my-alias` | 1 |
| `m_b139411f…` | `ch_5ff72bd1…` | `deepseek-chat` | `deepseek-chat` | 1 |

`credential`：两行 `kind=api_key`，密文列均为二进制密文（`b'\x94K\xf…` / `b'\x86:i\x9a\x…`）——**密钥不进渠道行**，一律走保险库。

### 2.5 凭证零明文复查（AGENT.md §5 / §9 相关项）

```
store.db size = 143360
contains 'sk-t8-test-****'（本轮假值）  : False
contains 'sk-t8-preset-****'（本轮假值）: False
contains client key plaintext    : False
vault.bin size = 596
vault contains 'sk-'      : False
vault contains 'Bearer '  : False
vault contains 'llmr_'    : False
```

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/providers/presets/mod.rs` | `PresetDef` 增 `default_models`；新增 `PresetView` / `ModelSuggestion` / `PresetDef::view()` / `all_views()`（前端目录）；`ToolSupport::as_str()` |
| `src-tauri/src/providers/presets/*.rs`（7 个） | 各带一条默认模型建议值（DeepSeek `deepseek-chat` / Kimi `moonshot-v1-8k` / GLM `glm-4-flash` / MiniMax 国际 `abab6.5s-chat` / MiniMax 国内 `abab6.5s-chat` / 百炼 `qwen-plus` / 硅基流动 `deepseek-ai/DeepSeek-V3`），注释标明 02 §6.1.1、**未实测、可改** |
| `src-tauri/src/providers/adapter.rs` | `AuthStyle::as_str()` / `AuthStyle::parse()`（`bearer` / `custom_header`） |
| `src-tauri/src/providers/mod.rs` | `apply_meta_overrides` 支持 `meta_json.auth_style`；**所有可选字段的 `null` 统一解释为"不覆盖"**（真机实测发现，见 §4） |
| `src-tauri/src/store/channels.rs`（新） | `ChannelMeta` / `ModelSummary` / `ChannelSummary`（含 `has_credential`）/ `ModelInput` / `ChannelInput` / `validate()` / `save_channel()` / `list_channels()` / `channel_summary()` / `delete_channel()` |
| `src-tauri/src/store/error.rs` | 新增 `StoreError::Invalid(String)`（"数据校验失败: {0}"） |
| `src-tauri/src/store/mod.rs` | 注册 `pub mod channels;` |
| `src-tauri/src/commands/channel.rs`（新） | 5 个 IPC：`channel_presets` / `channel_list` / `channel_save` / `channel_delete` / `channel_put_key`；每个都有不依赖 Tauri `State` 的可测版本 `*_of`；`check_provider()`；`input_from_preset()` |
| `src-tauri/src/commands/mod.rs`、`src-tauri/src/lib.rs` | 模块与 IPC 命令注册 |
| `src-tauri/tests/channel_e2e.rs`（新） | 4 条端到端验收用例（§2.2 表末四行） |
| `src/ChannelPanel.tsx`（新） | 预设 / 自定义双模式表单 + 已建渠道列表（编辑 / 删除）+ 密钥录入 |
| `src/App.tsx`、`src/App.css` | 挂载 `<ChannelPanel />`；`.field` / `select` / `textarea` 样式 |
| `docs/planning/02-technical-design.md` | §6.1.1 默认模型建议值、§6.1.2 自定义渠道表单字段、§9 `meta_json` 增 `auth_style` / `extra_headers`、§13 变更记录（v0.7） |

无新增依赖。

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| **预设 = 纯数据** | 预设不写死渠道；选中预设只是**预填表单**，落库的还是普通 `channel` 行（`provider` = 预设 id）。这样"预设"与"自定义"是同一条链路，编辑/删除逻辑不分叉 |
| **每个预设带一条默认模型建议值** | 任务卡判据是"选中预设后仅剩填 key 一步"——不预填模型名就做不到。默认模型取**公开常识建议值**（02 §6.1.1），**未实测**，UI 与文档均标注可改；实测矩阵归 T9/T10 |
| **`meta_json` 整份替换语义** | 表单状态即真相，不与旧值合并；写 `null` 才能表达"清空字段"。代价：旧版本写进去的字段若新表单没暴露会被抹掉——当前表单已覆盖全部约定字段 |
| **`null` = 不覆盖** | `apply_meta_overrides` 原先对 `null` 报类型错 → 500。表单里"没填"的字段天然是 `null`，**真机实测发现**（不是单测发现的）。现已统一按"不覆盖"处理，并补 1 条单测锁住语义 |
| **表单删掉的模型 → 置 disabled，不物理删除** | `model` 被 `route_member` 引用，物理删除会连带删掉用户的编排关系。disabled 的模型不出现于 `/v1/models`、也不参与路由 |
| **删除渠道会连带删除其模型与凭证** | FK CASCADE；凭证密文在 DB，`vault.bin` 只存主密钥。UI 因此弹确认框 |
| **别名全局唯一** | `idx_model_alias` 约束；被别家渠道占用直接报错（不自动改名，避免客户端手里的名字失效） |
| **密钥不进渠道行** | 渠道 CRUD 与密钥录入分离：`channel_save` 只写渠道与模型，`channel_put_key` 单独走保险库 |
| **新增 IPC 的参数用 camelCase、返回结构用 snake_case** | Tauri IPC **参数**按框架约定 camelCase（`channelId` / `payload`）；**返回结构**沿用仓库既有 `CredentialMeta` 一致的 snake_case 口径；payload 结构体字段统一 snake_case（与 `meta_json` 的库内契约一致） |
| **渠道 CRUD 落在数据层，IPC 层只做包装** | `store/channels.rs` 负责事务与校验，`commands/channel.rs` 只做 provider 校验与 `AppState` 包装 → 每个 IPC 都能脱离 Tauri 测 |
| **`vault.ensure_channel` 的占位渠道逻辑保留** | T2 既有行为，与 T8 的正规 CRUD 并存（注释已写明"T8/T9 接管渠道管理后取代"） |
| **改绑地址的红线未触碰** | 本任务不动网关绑定/Host 校验（AGENT.md §5 红线原样） |

### 上报与裁决

1. **默认模型名按"公开常识建议值"处理，未实测**（02 §6.1.1）。真实可用的模型名 / `base_url` 以 02 §10.1 实测矩阵为准（**T10**）；HANDOFF §5 未验证项 #2 / #4 仍然挂着，本轮**没有**动它们。
2. **`meta_json` 增 `auth_style`（`bearer` / `custom_header`）**：任务卡把"鉴权样式"列为表单字段，原实现不支持 → 属任务卡要求的补齐，已**先改 02（v0.7）再改码**。
3. **渠道列表 / 别名管理 / 路由组编排 UI** 属 T9（任务卡："渠道列表/编辑、密钥录入与回看、模型别名管理、client key 管理、`/v1/models` 预览"）。本轮只做了 T8 需要的最小渠道列表与编辑入口。

## 5. 未做（范围外）

- **路由组编排 UI**（`route_group` / `route_member` 编辑）—— T9
- **别名管理 UI**（多模型批量维护、别名重命名）—— T9；本轮表单一次只维护一个模型
- **client key 管理 UI** —— T9
- **渠道连通性 probe / 出站试跑** —— 未排期
- **`tools`（tool_calls）兼容度实测** —— T10；`tool_support` 目前一律 `unknown`
- **预设 `base_url` / 默认模型的真实可用性验证** —— T10（02 §10.1 实测矩阵）
- **MiniMax 国内站 GroupId 三种口径实测** —— T10（HANDOFF §5 未验证项 #2）
- **多账号 / 账号池** —— 后续任务；"账号"在 v1 是统一概念（T7 已认可口径）

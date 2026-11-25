# T4 验收留档 — 出站适配器

| 项 | 值 |
|---|---|
| 任务 | T4 出站适配器（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-25 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `364974f` |
| 结论 | **通过**（判据逐条实测见 §1） |

## 1. 判据对照

任务卡原文：*目标：`ProviderAdapter` trait + `openai_compat.rs` 通用实现（URL/头/body.model 改写，R-1~R-8 全部规则）+ 预设数据结构。验收：单测覆盖每个改写规则（R-1/R-3/R-5/R-7 必测）；MiniMax 出站 URL 按 §4.2 口径拼接。*

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | `ProviderAdapter` trait | ✅ | `providers/adapter.rs`，签名与 02 §6.4 一致（`kind` / `provider` / `complete` / `probe` / `refresh`） |
| 2 | `openai_compat.rs` 通用实现 | ✅ | `providers/openai_compat.rs`，`build_outgoing_call` 完成 R-1~R-8 |
| 3 | R-1~R-8 **全部规则**有单测 | ✅ | §2.2 逐条对照表（8 条规则全覆盖） |
| 4 | **R-1 必测** | ✅ | `r1_replaces_model_with_upstream_name` |
| 5 | **R-3 必测** | ✅ | `r3_uses_base_url_plus_channel_path`、`r3_strips_trailing_slash_of_base_url` |
| 6 | **R-5 必测** | ✅ | `r5_*` 四条（拼接 / 不拼 / 国际站不拼 / URL encode 且不入 body/header） |
| 7 | **R-7 必测** | ✅ | `r7_strips_hop_by_hop_headers` |
| 8 | **MiniMax 出站 URL 按 §4.2 口径拼接** | ✅ | 单测 `minimax_url_follows_section_4_2` + 端到端 `acceptance_minimax_cn_url_follows_section_4_2`（§2.3） |
| 9 | 预设数据结构 | ✅ | `providers/presets/`：7 个 profile 为纯数据（`PresetDef`），新增渠道 = 填表零代码 |

## 2. 实测记录

### 2.1 测试

```
$ cd src-tauri && cargo test
test result: ok. 110 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   # 单元测试
test result: ok. 4 passed;  0 failed; 1 ignored                                 # tests/gateway_e2e.rs（T3）
test result: ok. 6 passed;  0 failed; 0 ignored                                 # tests/outbound_e2e.rs（T4 新增）
test result: ok. 3 passed;  0 failed; 1 ignored                                 # tests/vault_e2e.rs（T2）
```

T4 新增单元用例集中在 `providers::`（T3 时的单元基线见 `docs/reviews/T3-gateway-auth-verification.md` §2.1）。

```
$ npm run build   # 前端未受影响，回退检查
✓ built in 416ms
```

### 2.2 改写规则逐条覆盖

| 规则 | 含义 | 单测用例 |
|---|---|---|
| R-1 | `body.model` 替换为上游真实模型名 | `r1_replaces_model_with_upstream_name` |
| R-2 | `Authorization: Bearer <凭证>` 替换 | `r2_sets_bearer_authorization`、`r2_missing_credential_is_error` |
| R-3 | URL = `base_url` + 渠道路径模板 | `r3_uses_base_url_plus_channel_path`（GLM 非标路径）、`r3_strips_trailing_slash_of_base_url` |
| R-4 | 流式注入 `stream_options.include_usage`（渠道声明支持才注入） | `r4_injects_include_usage_when_streaming_and_supported`、`r4_does_not_inject_for_non_stream_request`、`r4_not_injected_when_channel_does_not_support`（MiniMax）、`r4_preserves_existing_stream_options_keys` |
| R-5 | `group_id` 非空且支持时追加 `?GroupId=` | `r5_appends_group_id_for_minimax_cn`、`r5_not_appended_without_group_id`、`r5_not_appended_for_international_profile`、`r5_group_id_is_url_encoded_and_not_leaked_into_body_or_headers` |
| R-6 | 追加渠道自定义头（Copilot 伪装头位） | `r6_applies_channel_extra_headers` |
| R-7 | 剥离 hop-by-hop 头 | `r7_strips_hop_by_hop_headers` |
| R-8 | 未知 body 字段透传 | `r8_passes_through_unknown_body_fields` |

其余新增用例：MiniMax 业务错误码映射 9 条（`providers::presets::minimax::tests::*`）、预设注册表 6 条、
注册表与 `meta_json` 合并 12 条。

### 2.3 MiniMax 出站 URL 口径（02 §4.2）

端到端用例走**真实 `channel` 表 → 预设合并 → 出站调用**全链路（`tests/outbound_e2e.rs`）：

```
acceptance_minimax_cn_url_follows_section_4_2   ok
    url    = https://api.minimaxi.com/v1/text/chatcompletion_v2?GroupId=17895xxxxxxx
    method = POST
    body.model = "MiniMax-Text-01"        (alias "mm-alias" 已替换)
    authorization = "Bearer sk-upstream-secret"
    body 无 stream_options                (MiniMax 不注入，R-4)
    connection / keep-alive / transfer-encoding / content-length / host 均已剥离
    accept / content-type 透传

acceptance_minimax_intl_url_has_no_group_id   ok
    url = https://api.minimax.io/v1/text/chatcompletion_v2   (不含 GroupId)

acceptance_group_id_never_leaks_into_body_or_headers   ok
    GroupId 只出现在 URL query，body 与全部 header 均无
```

其它渠道路径（同一端到端文件）：

```
acceptance_glm_non_standard_path           ok
    https://open.bigmodel.cn/api/paas/v4/chat/completions
acceptance_dashscope_compatible_mode_path  ok
    https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
    （百炼声明支持 → 流式注入 include_usage，R-4）
acceptance_outgoing_request_line_matches_target_url  ok
    origin = api.minimaxi.com, path = v1/text/chatcompletion_v2?GroupId=17895xxxxxxx
```

### 2.4 MiniMax 业务错误码映射（02 §4.2 表）

`providers/presets/minimax.rs` 提供映射与判定，单测逐条断言：

| `base_resp.status_code` | 映射 HTTP | `type` | 可重试 | `code` |
|---|---|---|---|---|
| 1000 | 500 | `upstream_error` | 否 | `minimax_1000` |
| 1001 | 502 | `upstream_error` | 是 | `minimax_1001` |
| 1004 | 401 | `authentication_error` | 否 | `minimax_1004` |
| 1008 | 402 | `api_error` | 否 | `minimax_1008` |
| 1027 / 1039 | 429 | `rate_limit_error` | 是 | `minimax_1027` / `minimax_1039` |
| 其他 | 502 | `upstream_error` | 否 | `minimax_<code>` |

- 1004 的提示文案固定为「检查 GroupId 配置，而非重新生成 key」（`status_code_1004_maps_to_401_with_group_id_hint` 断言）。
- **HTTP 200 但业务错误**可被识别（`http_200_with_business_error_is_detected`）：`is_business_error` / `classify` 读 `base_resp.status_code`。

> P-3（响应侧按此表改写状态码与错误体）的**接线属 T6**，本任务只交付映射表与判定函数。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/providers/adapter.rs` | trait 契约（02 §6.4）+ `ChannelConfig` / `CallContext` / `OpenAIRequest` / `OutgoingCall` / `ProbeResult` / `AdapterError` |
| `src-tauri/src/providers/openai_compat.rs` | 通用 OpenAI 兼容实现：R-1~R-8 改写 + `url_encode` + `HOP_BY_HOP_HEADERS` |
| `src-tauri/src/providers/presets/mod.rs` | `PresetDef`（纯数据）+ `to_config` + 预设注册表 |
| `src-tauri/src/providers/presets/{deepseek,kimi,glm,minimax_intl,minimax_cn,dashscope,siliconflow}.rs` | 02 §6.1 的 7 个 profile |
| `src-tauri/src/providers/presets/minimax.rs` | MiniMax 业务错误码映射（§4.2）+ `base_resp` 判定 |
| `src-tauri/src/providers/mod.rs` | 适配器注册表 + `channel.meta_json` 覆盖合并 |
| `src-tauri/tests/outbound_e2e.rs` | 端到端验收用例（6 个） |
| `src-tauri/src/lib.rs` | 新增 `pub mod providers;` |

依赖新增：`async-trait 0.1.92`（trait 的 async 方法）。

## 4. 设计取舍与遗留

| 项 | 说明 |
|---|---|
| **预设数量按 7 个实现** | 02 §6.1 标题写"8 profile"，但表体只有 7 行；03 T8 写"7 个 profile"。按**表体的 7 行**落地，标题与表体的差异未擅自改文档（见 §6） |
| `tool_support` 一律 `Unknown` | 02 §10.1 判据是"全绿才能标 `stable`"，实测矩阵由 T9 执行。**未实测不标 stable/partial**，避免 UI 提前承诺未验证的兼容性 |
| `probe()` 返回 `Unsupported` | trait 契约要求连通性/额度探测，但真实探测需要出站 HTTP 客户端与真实凭证，属 T9 实测矩阵范围。本任务只交付 trait 形状，不塞桩数据 |
| 未引入 `reqwest` | 出站 HTTP 客户端随 T6（流式管道）一起接入；T4 只负责"构造出站调用"，不负责发送 |
| `ChannelConfig` 增 `kind` 字段 | 渠道类型是出站鉴权三型（02 §3.2）的必要输入，放在配置里省去调用方二次查表 |
| `meta_json` 覆盖项 | 按 02 §9 约定实现 `group_id` / `supports_groupid_query` / `path_chat` / `inject_stream_usage`，另接受 `base_url` / `auth_header_name` / `extra_headers` 三个便利覆盖。解析失败**报错而非静默忽略**（避免配置写错却悄悄发到错误上游） |
| `custom` 渠道走通用适配器 | 自定义渠道本质就是"填表出来的 OpenAI 兼容渠道"；但 `kind != api_key` 的未知 provider（如 `copilot`）**不退回**通用适配器，避免 OAuth 渠道被错误地按 Bearer 处理 |
| R-7 的 `content-length` 重算 | 本层只负责**剥离**；重算由 HTTP 客户端按 body 长度完成（T6 接线） |
| R-6 的 Copilot 伪装头 | 头名与取值由 02 §6.2 指向参考仓库源码核实，**属 T17/T21 任务**；本任务只提供 `extra_headers` 机制与单测，不臆造版本串常量 |

## 5. 未做

- Copilot（OAuth 型）与 WorkBuddy（登录态型）适配器 —— 02 §6.2 / §6.3，各自实现 trait，不在 M1 范围
- 真实出站发送、连通性探测、流式管道、熔断与 fallback —— T6
- 响应侧 P-1~P-7（含 `X-Routed-Via`）—— T6
- MiniMax P-3 响应改写接线 —— T6
- P-4 学习式兼容（注入报 400 后降级关闭注入并记入渠道 meta）—— T6；本任务已预留 `meta_json.inject_stream_usage` 这个持久化落点
- 渠道管理 UI（录入渠道 / GroupId 输入框等）—— T8/T9

## 6. 待裁决

| # | 项 | 说明 |
|---|---|---|
| 1 | 02 §6.1 标题"8 profile"与表体 7 行不一致 | 03 T8 写"7 个 profile"，本实现按 7 个落地。是否回填 02 §6.1 标题，待头头裁决（与既有 `docs/reviews/proposal-task-numbering.md` 同类问题） |

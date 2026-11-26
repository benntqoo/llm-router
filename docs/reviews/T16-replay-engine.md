# T16 验收留档 — 重放引擎

| 项 | 值 |
|---|---|
| 任务 | T16 重放引擎（03 §2 任务卡，M3 第二张卡） |
| 依据 | `docs/planning/02-technical-design.md` §7.1（双轨设计：按响应特征判定成败）、§7.1.3（重放引擎口径）、§7.2（结果写 `checkin_result`）、§7.3（合规边界）、§3.3（错误分类与重试）；**v0.16 新增 §7.1.3** |
| 验收日期 | 2026-11-27 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `67ef3c4`（引擎 + 断言结构扩展 + 结果落库 + e2e + 验收工具）；留档与文档另计（见 §7） |
| 结论 | **通过**（任务卡判据「变量注入、请求重放、响应断言（状态码 + JSON 路径）、错误分类；验收：单测 3 场景（成功/断言失败/网络失败）」逐条实测通过） |

> 本文件改动范围：仅记录 T16 的验收判据、实测证据、交付物与取舍；**不含**后续任务（T17 调度器 / T18 站点插件 / T19 通知）的结论。

---

## 1. 判据对照

**T16 任务卡**（03 §2）：「变量注入、请求重放、响应断言（状态码 + JSON 路径）、错误分类。验收：**单测 3 场景（成功/断言失败/网络失败）**。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **变量注入** | ✅ | §3.1：`inject()` 覆盖四种 target（整个头 / Cookie 子项 / query / JSON body 叶子）；**缺凭据不发请求**（`credential_missing`）；**未知占位符拒绝重放**（`template_invalid`） |
| 2 | **请求重放** | ✅ | §3.2 e2e + §3.3 真机：真实模板 → 注入 → 上游**确实收到带真值的 Cookie**（占位符一个都没发出去） |
| 3 | **响应断言（状态码 + JSON 路径）** | ✅ | §3.1：状态码断言、JSON 路径断言（点号路径 + 数组下标 + `equals`）、二者「与」关系；旧模板兼容 |
| 4 | **错误分类** | ✅ | §3.1 + §3.3：8 类（`success` / `assert_failed` / `network_error` / `credential_missing` / `template_invalid` / `credential_invalid` / `rate_limited` / `http_error`），每类带「可重试」标记（沿用 02 §3.3 判据） |
| 5 | **单测 3 场景（成功/断言失败/网络失败）** | ✅ | §3.1 单测 16 条 + §3.2 e2e 4 条（真 mock 上游跑全链）+ §3.3 真机验收工具 5 场景 |
| 6 | **结果落 `checkin_result`** | ✅ | §3.3：`ok = 1` 仅当 `success`；`message` 是可直接用于通知的中文原因 |

---

## 2. 交付物

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/checkin/replay.rs` | **新增** | 重放引擎：`inject`（变量注入）/ `ReplayEngine::replay`（重放）/ `check_assertion`（状态码 + JSON 路径断言）/ `evaluate` + `refine_by_status`（错误分类）/ `substitute`（占位符替换）+ **16 条单测** |
| `src-tauri/src/checkin/har.rs` | 改 | ① `TemplateAssert` 扩展：`status` 改 `Option<i64>` + 新增可选 `jsonPaths`（**向后兼容**：T15 旧模板继续可解析）；② `json_lookup` 提为 `pub`（T16 的 JSON 路径断言复用同一份实现，避免两处路径语义漂移） |
| `src-tauri/src/store/checkin.rs` | 改 | 新增 `checkin_result` 读写：`insert_result` / `list_results` + `ResultRow` + 1 条单测 |
| `src-tauri/tests/checkin_replay_e2e.rs` | **新增** | 4 条 e2e（真 mock 上游跑全链：成功 / 断言失败 / 网络失败 / 缺凭据） |
| `src-tauri/examples/checkin_replay_demo.rs` | **新增** | 真机验收工具：一条命令走完「真实抓包 → T15 存模板 → T16 重放 → 落结果 → 零明文核对」 |
| `docs/planning/02-technical-design.md` | 改 | **新增 §7.1.3 重放引擎口径** + §13 v0.16 |

---

## 3. 验收证据

### 3.1 后端单测（16 条新单测）

`checkin::replay::tests`：

| 用例 | 断言 |
|---|---|
| `injects_cookie_parts_from_the_vault` | Cookie 子项按各自的真实值替换；URL 不带多余 query |
| `missing_credential_stops_before_any_request` | 缺凭据 → `credential_missing`，**不可重试**，消息点名缺哪条凭据 |
| `unknown_placeholder_is_rejected` | 模板里有变量表之外的占位符 → `template_invalid`（不把 `{{x}}` 发出去） |
| `query_is_rebuilt_from_the_array_not_the_url` | URL 自带的 query 被剥掉再按数组重建 —— **同一个参数不能发两遍** |
| `host_and_content_length_headers_are_dropped` | `Host` / `Content-Length` 由客户端生成，模板里录制时的旧值要剔掉 |
| `json_body_leaves_are_replaced` | JSON body 叶子替换，其它字段原样保留，`Content-Type` 补上 |
| `assertion_passes_on_expected_status_and_json_paths` | 状态码 + JSON 路径都过 → 摘要含「签到成功」 |
| `assertion_rejects_wrong_status` | 状态码不符 → 消息同时给出**期望值与实际值** |
| `json_path_assertion_detects_missing_and_mismatched_values` | 路径不存在 / 值不等 / 响应不是 JSON → 三种失败都归 `assert_failed` |
| `json_path_supports_array_indexes` | `data.items.0.id` 这类下标路径可用 |
| `no_assertion_falls_back_to_2xx_or_3xx` | 没配断言时只判 2xx/3xx |
| `upstream_status_drives_the_classification` | 401/403 → `credential_invalid`；429 → `rate_limited`（可重试）；5xx → `http_error`（可重试）；4xx → 不可重试 |
| `failed_assertion_on_2xx_is_assert_failed` | 状态码对但 JSON 路径不符 → `assert_failed`（不是"成功"） |
| `substitute_reports_unknown_placeholders` | 占位符替换与未知占位符报错 |
| `split_query_keeps_the_base_and_the_tail` | URL 拆分 |
| `class_names_round_trip` | 8 个分类的字符串稳定且可反读（落库用） |

另有 `store::checkin::tests::results_round_trip_and_sort_by_time`（结果落库 / 倒序 / 按站点过滤）。

### 3.2 e2e（真 mock 上游，4 条）

`src-tauri/tests/checkin_replay_e2e.rs` —— 用的是 **T15 的真实产物**（真实抓包夹具 → 真实模板 → 真实保险库），只有上游换成内存 mock：

| 用例 | 断言 |
|---|---|
| `replay_succeeds_and_records_the_result` | 状态码 200 + JSON 路径 `data.ok == true` → `success`；**上游收到的 Cookie 是保险库里的真值**（含 `REDACTED-acw_tc` / `REDACTED-cna`，且**不含任何 `{{`**）；结果落库 `ok = 1`，消息里**不含**真值 |
| `assertion_failure_is_classified_and_recorded` | JSON 路径不符 → `assert_failed`（不可重试）；上游 401 → **`credential_invalid`**（比"状态码不符"更可操作）；两次失败都落库 `ok = 0` |
| `network_failure_is_classified_as_retryable` | 连不上 → `network_error`、**可重试**；上游请求数 **0**；落库 `ok = 0` |
| `missing_credential_stops_before_sending_anything` | 凭据被删 → `credential_missing`；上游请求数 **0**（**缺凭据时一个请求都不该发出去**） |

### 3.3 真机验收（`cargo run --example checkin_replay_demo`）

一条命令走完整条链：**真实抓包夹具 → T15 存模板（真实值进保险库）→ T16 取回模板 → 注入 → 重放 → 断言 → 分类 → 落 `checkin_result` → 零明文核对**。库与保险库指向 `%TEMP%\llmr-t16-demo\`，**不污染用户真实库**。

实测输出（节选）：

```
[T15] 模板已存：站点=ModelScope 变量=8 个 ["acw_tc", "cna", "auto__ga_k9cstskfc5", "auto__ga",
      "auto_gcl_au", "auto_modelscope_locale", "auto_ssxmod_itna", "auto_ssxmod_itna2"]

上游收到 Cookie: acw_tc=REDACTED-acw_tc; _ga_K9CSTSKFC5=REDACTED-_ga_K9CSTSKFC5; _ga=REDACTED-_ga;
      _gcl_au=REDACTED-_gcl_au; modelscope-locale=REDACTED-modelscope-locale; cna=REDACTED-cna;
      ssxmod_itna=REDACTED-ssxmod_itna; ssxmod_itna2=REDACTED-ssxmod_itna2

① 成功: ok=true  分类=success            可重试=false 状态=Some(200) 耗时=2ms
   原因: 签到成功（HTTP 200），JSON 路径断言 1 条全过
② 断言失败（JSON 路径不符）: ok=false 分类=assert_failed   可重试=false 状态=Some(200)
   原因: JSON 路径 data.ok：期望 true，实际 false
③ 断言失败（上游 401）: ok=false 分类=credential_invalid 可重试=false 状态=Some(401)
   原因: 上游返回 401：凭证可能已失效（状态码不符：期望 200，实际 401）
④ 网络失败（连不上）: ok=false 分类=network_error      可重试=true  状态=None
   原因: 网络失败：连不上上游（error sending request for url (http://127.0.0.1:55501/...)）

⑤ 缺凭据: ok=false 分类=credential_missing
   原因: 变量 acw_tc 缺少凭据：cred_f60a95c1… 不存在（没录或已删除）
   上游请求数 3 → 3（缺凭据时**不该发出任何请求**）

checkin_result 共 5 行：
  ts=… ok=false message=变量 acw_tc 缺少凭据：cred_f60a95c1… 不存在（没录或已删除）
  ts=… ok=false message=网络失败：连不上上游（…）
  ts=… ok=false message=上游返回 401：凭证可能已失效（状态码不符：期望 200，实际 401）
  ts=… ok=false message=JSON 路径 data.ok：期望 true，实际 false
  ts=… ok=true  message=签到成功（HTTP 200），JSON 路径断言 1 条全过

零明文核对：store.db + vault.bin 明文命中 0 条（响应体与变量真值都不落库）
上游总请求数：3
```

**关键证据**：上游收到的 Cookie 是**保险库里的真值**（不是占位符）；缺凭据与网络失败两个场景**上游请求数不增加**；`checkin_result` 五行的 `ok` 标记与分类一致；`store.db` 与 `vault.bin` 明文命中 **0**。

### 3.4 回归

- `cargo test --all-targets` → **388 项全绿**（**328** 单元 + 60 e2e：channel 7 + checkin 3 + **checkin_replay 4** + gateway 5 + outbound 6 + router 11 + stream 13 + usage 8 + vault 3），4 ignored，**0 失败**。
- 编译告警：`cargo clean -p llm-router` 后 `cargo check --all-targets` → **0 告警 0 错误**。
- `npm run build` → 通过（**30 modules**，gzip 58.92 kB，**T16 零前端改动**，包体不变）。

---

## 4. 口径回填（02 v0.16）

本卡落地前**先改文档后改码**（AGENT.md §6-3），02 新增 **§7.1.3 重放引擎口径**，六节：

1. **变量注入** —— 敏感变量按 `credential_id` 从保险库解密、非敏感变量用 `default`；**缺凭据不发请求**（`credential_missing`）；替换发生在**值内部**；**未知占位符拒绝重放**（`template_invalid`）。
2. **请求构造** —— **query 以模板 `request.query` 数组为准重建**（HAR 的 `url` 自带 query 与 `queryString` 重复，直接拼会发两遍）；**剔除 `Host` 与 `Content-Length`**（由客户端按实际 URL / 请求体生成，留着录制旧值会把请求打歪）；连接 10s / 整请求 30s；**引擎自己不重试**（重试与冷却属 T17）。
3. **响应断言** —— `assert.status` 是**期望状态码**（不是"录制时的状态码"）；新增可选 `assert.jsonPaths`（点号路径 + 数组下标 + `equals`）；状态码与 JSON 路径是「与」关系；**旧模板兼容**（新字段一律可选）。
4. **错误分类** —— 8 类 + 「可重试」标记（沿用 §3.3 判据）；**不猜验证码**（不内置验证码识别，验证码表现为断言不符 → `assert_failed` / `http_error`）。
5. **结果落库** —— 每次重放写一行 `checkin_result`；`ok = 1` 仅当 `success`；**响应体不进库**（只在内存留 512 字节片段，防 csrf / token 密文落库）；`reward_json` 留空（积分回写属 T18 / T17）。
6. **边界** —— T16 只做引擎：无调度 / 无计划任务 / 无通知 / 无浏览器辅轨 / 无 UI 触发入口。

---

## 5. 已知限制与未做项

1. **导入的模板带着"录制时的状态码"** —— T15 导入时 `assert.status` 填的是录制值（ModelScope 匿名态 = **401**），真实签到成功是 **200**。**用户必须自己改成期望值**（目前没有断言编辑 UI）——T18 的站点插件会带正确断言。**这是当前最大的使用门槛**，已记 backlog。
2. **csrf 会轮换，模板不会自己更新** —— 响应头按设计丢弃（T15），模板只固化录制那一刻的 Cookie；服务端下发的新 csrf 只能靠**重录 HAR** 更新（T16 引擎不负责刷新会话）。
3. **没有 UI 触发入口** —— 「立即执行一次」按钮与结果列表属 T17 / T18（任务卡未要求，按 AGENT.md §10 不顺手扩范围）。
4. **无重试** —— 引擎一次一打；重试与 429 冷却属 T17 调度层（分类里已带 `retryable` 标记供其使用）。
5. **不猜验证码** —— 撞到验证码表现为状态码 / 正文与断言不符 → 归 `assert_failed` 或 `http_error`；要单独表达需用户配 JSON 路径断言（AGENT.md §5.7 不做风控规避）。
6. **积分未提取** —— `reward_json` 留空；`{"credits": 400}` 的提取与 `account.quota_json` 回写属 T18 / T17（02 §7.2）。
7. **未测真实站点** —— 本卡**没有对 ModelScope 发过真实签到请求**（无登录态）；所有重放都对 mock 上游做的。

---

## 6. 经验

- **"断言失败"不是一种失败，是四种** —— 只报"状态码不符"对用户没用：上游 401 是**凭证失效**（要重录 HAR）、429 是**被限流**（等冷却再打）、5xx 是**上游挂了**（等会儿再打）、2xx 但 JSON 不对才是**站点行为变了**。所以引擎的分类是**先按状态码细化、再落到可操作的原因**，`retryable` 也跟着状态码走（§3.3 判据）。
- **缺凭据必须在发请求之前就拦住** —— 少一次请求就少一次上游侧的无效打卡；e2e 直接断言"上游请求数不增加"，这条比断言错误消息更防回归。
- **HAR 的 `url` 和 `queryString` 是重复的** —— 两者都保留是 T15 的口径（照抄 HAR），但**重放时不能两个都拼**。规矩：**query 以数组为准，先把 URL 自带的 query 剥掉再重建**。同理 `Host` / `Content-Length` 必须剔掉——留着录制时的旧值会把请求打歪（`Host` 与 URL 不一致时尤其危险）。
- **未知占位符要"拒绝重放"而不是"原样发出去"** —— `{{x}}` 原样发到上游，上游大概率回 4xx，你会误以为是站点挂了；早失败在本地、消息直接点名是哪个占位符。
- **响应体不能落库** —— 失败排障确实想看响应体，但签到接口的响应体可能带 csrf / token。做法：**只在内存里留 512 字节片段**（`body_preview`），落库的 `message` 只带人类可读的原因。
- **`cargo run --example` 的工作目录不一定是 crate 目录** —— 例子里用相对路径读夹具会报"系统找不到指定的路径"；正解是 `env!("CARGO_MANIFEST_DIR")` 拼绝对路径（而 `cargo test` 的工作目录是 crate 目录，所以 e2e 里用相对路径没事）。
- **验收工具要把"上游看到了什么"打出来** —— 打印 mock 收到的 Cookie，才看得出**注入的是保险库里的真值**（而不是占位符没替换掉）。

---

## 7. 提交清单

| 提交 | 内容 |
|---|---|
| `67ef3c4` | `feat: T16 重放引擎（变量注入 + 请求重放 + 状态码/JSON 路径断言 + 错误分类）` |
| 本文件所在提交 | 验收留档 + 02 v0.16 + 03 落地行 + HANDOFF 账本 |

> 文档提交号在**提交后回填**（与 T13/T14/T15 的做法一致）。

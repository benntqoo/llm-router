# T14 验收留档 — 聚合与图表 UI

| 项 | 值 |
|---|---|
| 任务 | T14 聚合与图表 UI（03 §2 任务卡，M2 收尾卡） |
| 依据 | `docs/planning/02-technical-design.md` §8「聚合视图：按 日/渠道/模型/client_key 四维；TTFT P50/P95；输出 tokens/s」（**v0.14 新增 §8.4 口径细化**）、§8.3 成本口径；`docs/planning/03-delivery-plan.md` §1 M2 判据、§2 T14 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `9820bb1`（后端聚合 + 口径 + e2e）；`3251e62`（顺带修掉一条既有随机失败断言）；前端与留档另计 |
| 结论 | **通过**（M2 判据逐条实测通过；单测 + e2e 全链 + 前端单测 + 真机四证） |

> 本文件改动范围：仅记录 T14 的验收判据、实测证据、交付物与取舍；**不含**后续任务的结论。

## 1. 判据对照

**M2 判据**（03 §1）：「一次流式请求后 UI 显示 **tokens / TTFT / tokens·s⁻¹ / 估算成本**，且 `usage_log.estimated` 标记正确（上游有 usage 时为 0）」

**T14 任务卡**（03 §2）：「四维聚合 SQL + React 图表（按天/渠道/模型/客户端）。验收：M2 判据。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **四维聚合**（按天 / 渠道 / 模型 / 客户端） | ✅ | §3.2 e2e 逐维度断言；§3.4 真机**四个维度逐一截图** |
| 2 | **React 图表** | ✅ | §3.4 真机：手写 SVG 条形图（三种度量可切） |
| 3 | UI 显示 **tokens** | ✅ | KPI「总 token **229**（输入 188 · 输出 41）」+ 明细表逐行 |
| 4 | UI 显示 **TTFT** | ✅ | KPI「TTFT P50 / P95 = **186 ms / 201 ms**」；明细表 185 / 186 / 201 ms，**非流式显示「—」** |
| 5 | UI 显示 **tokens·s⁻¹** | ✅ | KPI「输出速度 **44.9 tok/s**」；明细表 36.6 / 36.3 / 34.2 tok/s（流式三行） |
| 6 | UI 显示 **估算成本** | ✅ | KPI「已知成本 **¥0.000221 元**（另有 1 行未配牌价（未知））」——**有真实金额**，且**未知的行不会被当成 0 元**（明细表里 ¥0.00006 / ¥0.000041 元与「未知」并存） |
| 7 | `usage_log.estimated` 标记正确（上游有 usage 时为 0） | ✅ | 明细表「来源」列全部显示**精确**；KPI「估算行 **0**」；库内 `estimated=0` |

## 2. 做了什么摘要

- **新增 `src-tauri/src/store/analytics.rs`**（数据层唯一聚合载体）：
  - `Dim`（`day` / `channel` / `model` / `client`）——`GROUP BY` 表达式**只返回固定字面量**，不接受外部输入（无 SQL 注入面）；可空列 `COALESCE` 成 `(未记录)`；
  - `aggregate()`——每个桶 **13 项指标**（请求数 / 三态计数 / 五类 token / 总 token / 已知成本合计 / **成本未知行数** / 估算行数 / TTFT P50·P95 / 输出速度）+ **不受 limit 影响的合计桶**；
  - `recent_requests()`——最近请求明细（tokens / TTFT / tokens·s⁻¹ / 成本 / `estimated` 标记）；
  - 14 条单测。
- **新增 `src-tauri/src/commands/usage.rs`**：IPC `usage_aggregate` / `usage_recent`（各有 `_of` 可测版本；维度取值非法时报可读中文错）+ 3 条单测。
- **新增 `src/usage/`**（前端）：`types.ts`（IPC 数据形状）、`format.ts`（纯格式化函数，**空值与 0 必须可区分**）、`UsageChart.tsx`（**手写 SVG** 条形图）、`UsagePanel.tsx`（维度 / 时间窗 / 度量切换 + KPI + 图表 + 聚合表 + 明细表）。
- **文档先行**：02 新增 **§8.4 聚合与图表口径** + §13（v0.14）。
- **新增 `src-tauri/examples/seed_usage_demo.rs`**：真机验收工具（种临时库 + 起 mock 上游）。

## 3. 验收证据

### 3.1 后端单测（`store::analytics::tests` 14 条 + `commands::usage::tests` 3 条）

| 用例 | 断言 |
|---|---|
| `aggregates_by_channel_and_resolves_names` | 三个渠道 → 桶正确；**渠道名从 `channel.name` 解析出来**（`DeepSeek 官方`） |
| `groups_by_utc_day` | 按 **UTC 日**分桶（`2024-01-01` / `2024-01-02`），日维度按日期**升序** |
| `groups_by_client_key_with_label` | 桶带 client key 标签（`pi-主力`） |
| `ttft_percentiles_use_nearest_rank` | **最近秩法**：[10,20,30,40] → p50 = 20、p95 = 40 |
| `ttft_is_none_when_no_row_has_it` | 无 TTFT 样本 → `None`（**不是 0**） |
| `output_tps_ignores_failed_rows` | 失败行不计入速度（100 token / 1s = 100 tok/s） |
| `cost_unknown_rows_are_counted_separately` | 已知成本 10_000_000 micros 与「未知行数 1」**分开记** |
| `estimated_rows_are_counted` | 估算行单独计数 |
| `time_range_filters_rows` | `since` 之前的行被排除（**合计也遵守时间范围**） |
| `total_is_not_capped_by_limit` | 桶受 limit 限制、**合计不受** |
| `output_tps_is_none_when_duration_is_zero` | 耗时为 0 → `None`（不是 0，也不是无穷大） |
| `recent_requests_are_sorted_newest_first_with_metrics` | 新的在前；失败行无速度；标签已解析 |
| `empty_table_aggregates_to_zero_total` | 空库 → 合计全 0，TTFT 为 `None` |
| `percentile_handles_empty_and_single` | 分位数边界 |
| `unknown_dimension_is_rejected` / `aggregate_defaults_to_a_thirty_day_window` / `recent_returns_rows_newest_first` | IPC 层：维度非法报错、默认近 30 天、明细倒序 |

### 3.2 e2e 全链（`tests/usage_e2e.rs`）

新增 `acceptance_aggregate_reflects_real_gateway_traffic`：**真网关** → 流式 + 非流式各一次 → 用量落库 → 四维聚合逐项对账。

```
按渠道：c1（非流式渠道）prompt 31 / completion 7 / cache_read 5 / ttft 无 / estimated 0
        c2（流式渠道）  prompt 42 / completion 9 / ttft 有
合计：  requests 2 · prompt 73 · completion 16 · total_tokens 89 · success 2
        cost_micros 0 · cost_unknown_rows 2（没配牌价 → NULL → 未知）
按日：  同一天 → 1 个桶，requests 2
按模型：1 个桶（两个渠道同一上游模型）· 按客户端：1 个桶，标签 = client key 标签
明细：  每行 tokens·s⁻¹ **按该行自己的耗时复算**（tps == 输出 token ÷ 耗时秒）
```

### 3.3 前端单测（`src/usage/format.test.ts` 10 条）

`node --experimental-strip-types --test src/usage/format.test.ts` → **10 项全绿**。覆盖：

- **成本 `NULL` → 「未知」，不等于 `¥0`**（02 §8.3 的 `0`/`NULL` 语义在 UI 侧同样成立）；
- `formatCost(536)` → `¥0.000536 元`、`formatCost(10_000_000)` → `¥10 元`；
- token 千分位（**不做单位缩写**——统计页要精确值）；
- TTFT / 输出速度的 `null` → 「—」；
- 条形图比例归一化（负值截 0、超 1 截 1、最大值为 0 不画条）；
- 坐标轴上限取整到 1/2/5 × 10ⁿ。

### 3.4 真机（真实应用 + 真网关 + mock 上游）

用 `src-tauri/examples/seed_usage_demo.rs`（新）一键准备：种临时库（渠道 + **两个模型** + 凭证 + client key + 一条价目）并起一个 mock 上游（`127.0.0.1:63582`）。**库与保险库都指向 `%TEMP%\llmr-t14-demo2\`，不污染用户真实库。**

演示环境刻意做成**「有价 + 无价」两个模型**，一次把两件事都验实：

| 模型 | 价目 | 用途 |
|---|---|---|
| `demo-model-1`（别名 `demo-chat`） | **配了**：输入 1 元/1M、输出 2 元/1M、缓存命中 0.1 元/1M | 让 UI 显示**真实金额** |
| `demo-model-2`（别名 `demo-chat-2`） | **没配** | 让 UI 显示**「未知」**（02 §8.3 的 `NULL` 语义） |

应用实跑日志：

```
[store] 数据库 %TEMP%\llmr-t14-demo2\store.db（schema v8）
[vault] 保险库 %TEMP%\llmr-t14-demo2\vault.bin（主密钥 v1）
[gateway] 监听 http://127.0.0.1:18787/v1（OpenAI 兼容入站）
```

经**真网关**打 5 次（3 次流式 + 1 次非流式走配价模型 + 1 次非流式走**未配价**模型），库里 5 行流水：

| id | 上游模型 | prompt | completion | cache_read | estimated | ttft_ms | duration_ms | status | cost_micros |
|---|---|---|---|---|---|---|---|---|---|
| 1 | demo-model-1 | 31 | 7 | 5 | **0** | NULL | 1 | success | **41** |
| 2 | demo-model-1 | 42 | 9 | 0 | **0** | **185** | 246 | success | **60** |
| 3 | demo-model-1 | 42 | 9 | 0 | **0** | **201** | 263 | success | **60** |
| 4 | demo-model-1 | 42 | 9 | 0 | **0** | **186** | 248 | success | **60** |
| 5 | **demo-model-2** | 31 | 7 | 5 | **0** | NULL | 0 | success | **NULL（未知）** |

成本独立复算（测试价：输入 1 元/1M、输出 2 元/1M、缓存命中 0.1 元/1M）：

- 第 1 行：可计费输入 = 31 − 5 = 26 → `26×1 + 7×2 + 5×0.1 = 40.5` → **41 micros** ✅
- 第 2–4 行：`42×1 + 9×2 = 60` → **60 micros** ✅
- 第 5 行：没配牌价 → **NULL（未知）**，不是 0 ✅

UI 实测（截图见 `docs/reviews/app-ui-usage*.png`）：

| 面板显示 | 实测值 |
|---|---|
| KPI 请求数 | **5**（失败 0 · 断连 0） |
| KPI 总 token | **229**（输入 188 · 输出 41） |
| KPI 已知成本 | **¥0.000221 元**（= 41 + 60×3 = 221 micros），提示「另有 **1** 行未配牌价（未知）」 |
| KPI TTFT P50 / P95 | **186 ms / 201 ms**（提示「只统计流式请求」） |
| KPI 输出速度 | **44.9 tok/s**（提示「成功行按耗时加权」） |
| KPI 估算行 | **0**（提示「上游没回 usage，由 tiktoken 估算」） |
| 聚合表 | `2026-11-26 · 请求 5 · 失败 0 · 总 token 229 · 输入/输出 188/41 · ¥0.000221 元（1 行未知）· 186ms/201ms · 44.9 tok/s · 估算 0` |
| 明细表 | 时间 / 渠道·模型 / 输入·输出 / 总 token / TTFT / 速度 / 成本 / 来源 / 状态 —— **5 行全 success，来源全「精确」**，成本列**有金额（¥0.00006 / ¥0.000041 元）与「未知」并存** |

**四个维度逐一验证**（同一份数据，只切维度）：

| 维度 | 桶内容 | 截图 |
|---|---|---|
| 按天 | `2026-11-26` → 229 | `app-ui-usage.png` |
| 按渠道 | `演示渠道` → 229 | `app-ui-usage-channel.png` |
| 按模型 | **两个桶**：`demo-model-1` → 191（¥0.000221 元）、`demo-model-2` → 38（**¥0 元（1 行未知）**） | `app-ui-usage-model.png` |
| 按客户端 | `demo-client` → 229（**client key 标签已解析**） | `app-ui-usage-client.png` |

**两处边界在真机上自然出现**：明细表里 `duration_ms = 0` 的那行速度显示「**—**」（不是 0、不是无穷大），`duration_ms = 1` 的那行显示 `7000.0 tok/s`——正是 02 §8.4「耗时为 0 → NULL」的口径；同一张表里，**未配牌价那行成本显示「未知」而不是「¥0」**——正是 02 §8.3 的 `0` / `NULL` 语义。

### 3.5 回归

- `cargo test --offline` → **340 项全绿**（287 单元 + 7 + 5 + 6 + 11 + 13 + **8** e2e + 3），4 ignored，0 失败，**0 编译告警**。（e2e 7→8 = 新增 T14 全链用例；单元 269→287 = 新增 17 条 + 修掉 1 条随机失败断言。）
- `npm run build`（`tsc && vite build`）→ 通过，**27 modules**，gzip **56.38 kB**（**未新增任何运行时依赖**，包体不变）。
- `node --experimental-strip-types --test src/usage/format.test.ts` → 10 项全绿。

## 4. 取舍与已知限制

| 项 | 处理 | 理由 |
|---|---|---|
| 图表实现 | **手写 SVG**，不引第三方图表库 | 与 02 §6.6 事件总线同一取向：轻量、可离线构建、包体不增；图表单独成 `UsageChart.tsx`，**要换实现只动这一个文件** |
| 聚合放后端还是前端 | **后端** | 前端只做展示，**不在 UI 侧另写一份统计口径**（02 §6.6 的同源纪律）；口径改动只改一处 |
| 时间范围默认值 | 近 **30** 天（可选 7 / 30 / 90） | 30 天覆盖常见复盘周期；本地库体积小，不必默认全量 |
| 桶数上限 | 默认 **50**，**合计不受限** | 模型维度可能很多；截断只影响展示，不影响合计数字 |
| 日维度的日界 | **UTC 日** | 与 02 §8.2 的 UTC 日界一致，避免跨时区对不上 |
| `output_tps` 的分母为 0 | **NULL**（显示「—」） | 耗时为 0 时速度无从计算；显示 0 会被读成"没输出"，显示无穷大是错的 |
| `cost_micros` 为 NULL 的行 | **单独计 `cost_unknown_rows`**，已知成本合计里按 0 | 延续 02 §8.3：合计 ¥0 **不等于**"不花钱"（T13 留档 §5 的同一条口径） |
| **价目表编辑 UI** | **本轮不做** | T14 任务卡只写「四维聚合 + 图表」；M2 判据只需成本**可显示**（只读）。价目表的增删改仍走 `price_*` IPC 与 02 §8.3，**另排一张卡** |
| TTFT 只对流式有意义 | 非流式行 `ttft_ms` 为 NULL → 显示「—」 | 非流式没有"首个 token"的概念（02 §8.1） |
| 估算行 | **照常计入**聚合与成本，UI 标「估算」 | 02 §8.1 R7：估算值不参与预算告警，但成本照算 |

## 5. 交付物

| 文件 | 改动 |
|---|---|
| `src-tauri/src/store/analytics.rs` | **新增**——四维聚合 + 最近请求明细（13 项指标 / 最近秩分位 / 加权输出速度）+ 14 条单测 |
| `src-tauri/src/commands/usage.rs` | **新增**——`usage_aggregate` / `usage_recent` 两个 IPC + 3 条单测 |
| `src-tauri/src/store/mod.rs` / `src-tauri/src/commands/mod.rs` / `src-tauri/src/lib.rs` | 改——挂模块、注册 IPC |
| `src-tauri/tests/usage_e2e.rs` | 改——新增 T14 聚合全链用例；mock 加 `slowusage` 场景（延迟 120ms）与 `sseusage` 延迟 60ms，让 TTFT / 输出速度可按公式复算 |
| `src-tauri/src/gateway/auth.rs` | 改——**修掉一条既有随机失败断言**（见 §6）；纯测试改动，零行为变更 |
| `src-tauri/examples/seed_usage_demo.rs` | **新增**——真机验收工具（种临时库：渠道 + **两个模型** + 一条价目 + 凭证 + client key，并起 mock 上游） |
| `src/usage/types.ts` / `format.ts` / `format.test.ts` | **新增**——IPC 数据形状 / 纯格式化函数 + 10 条前端单测 |
| `src/usage/UsageChart.tsx` / `UsagePanel.tsx` | **新增**——手写 SVG 条形图 / 用量统计面板 |
| `src/App.tsx` / `src/App.css` | 改——挂载面板；补 KPI / 表格 / 图表样式 |
| `docs/planning/02-technical-design.md` | 改——新增 §8.4 聚合与图表口径、§13 v0.14 |

## 6. 经验

- **"复算型断言"要同时锚住"调用本身成功"**：T13 那条真实上游用例暴露过——失败时 token 全 0、成本恒为 0，`cost == 独立复算值(0)` 照样通过。本轮写 e2e 时特意让**速度按该行自己的耗时复算**，而不是比一个常量。
- **耗时为 0 是真实存在的**：本地 mock 亚毫秒响应 → `duration_ms = 0` → 速度无从计算。第一版用例直接断言"必须有速度"就红了。**这不是 mock 的毛病，是生产代码该挡住的边界**——分母为 0 时返回 `NULL`，别返回 0 或无穷大。
- **TTFT / 速度这类指标要"有才显示"**：把「没有样本」画成 0，UI 上就和"真的 0 毫秒"混为一谈了。`NULL` → 「—」这条规则和成本的 `NULL` → 「未知」是同一个道理。
- **「合计为 0」必须配一个「有几行未知」**：只显示 ¥0，用户会以为"这次没花钱"。**已知的 0 与未知的 0 在 UI 上必须分开**（延续 T13 的 `0` / `NULL` 口径）。
- **分位数要写清算法**：SQLite 没有分位函数，取原始值在内存里算很容易各写各的。**最近秩法**（第 `ceil(p×n)` 个）写进 02 §8.4，单测锁死 `[10,20,30,40] → p50=20 / p95=40`。
- **顺手修掉的既有 flaky 断言**：`gateway::auth` 里断言 client key 长度**恒等于 43**，但 `base62_encode` 会去前导零 → 32 字节随机数编码后是 **42 或 43**（首字节 < 4 时为 42，概率约 1.2%）。本轮回归随机撞上一次。**随机数相关的长度断言不该写死**——改成区间断言后连跑 40 次全绿。同文件另一条用例的注释算式（`2^256 < 62^43 < 2^256`）也写错了，一并改成 `62^42 < 2^256 < 62^43`。
- **真机验收要让"延迟"可观测**：mock 上游刻意延迟 60ms / 120ms，TTFT 与输出速度才能按公式复算；否则本地 mock 的亚毫秒响应会把一切压成 0。
- **验收工具比验收脚本更值得沉淀**：UI 面板没法手工点（种渠道、录密钥、造 client key），于是把"种临时库 + 起 mock 上游"做成一个 `cargo run --example` —— 以后任何用量/统计相关的真机验收都能一键复现。
- **PS 5.1 读无 BOM 的 .ps1 按 ANSI 解析**：脚本里**写中文注释会把语法搞坏**（实测报"Unexpected token '}'"）。验收脚本一律纯 ASCII。
- **`curl.exe` 在 PowerShell 下传 JSON 会被引号处理弄坏**：后端收到非 JSON → 400（本轮又踩了一次）。正解是**把 body 写进文件再 `-d @file`**，且**必须无 BOM**（`Set-Content -Encoding UTF8` 在 PS 5.1 会写 BOM，同样导致 400）。

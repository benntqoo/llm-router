# T13 验收留档 — 价目表与成本换算（price 表 + `cost_micros`）

| 项 | 值 |
|---|---|
| 任务 | T13 价目表与成本换算（03 §2 任务卡，M2 第二张卡） |
| 依据 | `docs/planning/01-architecture-and-feasibility.md` §4.4（`price` 表 DDL）、§5.5；`docs/planning/02-technical-design.md` §8（v0.13 新增 §8.3 成本换算口径）、§9；`docs/planning/03-delivery-plan.md` T13 任务卡 |
| 验收日期 | 2026-11-26 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | 见 `git log`（价目表模块 + 落库换算 + v8 迁移 + IPC + 单测/e2e）；文档提交另计 |
| 结论 | **通过**（任务卡验收判据「单测覆盖『输入+输出+缓存读』三价换算」实测通过；单测 + e2e 全链 + 真机三证） |

> 本文件改动范围：仅记录 T13 的验收判据、实测证据、交付物与取舍；**不含**后续任务的结论。

## 1. 判据对照

任务卡验收原文：「`price` 表 + 预填国产牌价 + `cost_micros` 换算。验收：单测覆盖『输入+输出+缓存读』三价换算。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **`price` 表** | ✅ | §2：表结构沿用 01 §4.4 DDL（主键 `(provider, model_pattern)`，三档单价）；新增 `store/price.rs` 承载查表 / 匹配 / 换算 / CRUD / 播种 / 历史重算；v8 迁移在开库时播种预填行 |
| 2 | **预填国产牌价** | ✅ | §4：47 行，覆盖 6 家 provider，全部**取自厂商官方页面**（附来源与核验日期） |
| 3 | **`cost_micros` 换算** | ✅ | §3：换算在**落库时**完成（`insert_usage`），改价后历史一并重算；口径写进 02 §8.3（v0.13） |
| 4 | **单测覆盖「输入+输出+缓存读」三价换算** | ✅ | §3：三价各自独立成用例，另加"缓存命中不被重复计费""缓存写按输入价""总额一次舍入""NULL ≡ 0 / 未命中 = NULL"等；**e2e 真网关全链**也断言三价 |

## 2. 做了什么摘要

- **新增 `src-tauri/src/store/price.rs`**（数据层唯一价目载体）：
  - `PriceEntry`（provider / model_pattern / input / output / cache_read 三档单价）；
  - `matches_pattern()`——`*` 通配任意字符串（可在任意位置、可多个），无 `*` 即精确匹配；
  - `lookup()`——命中多条时取**最具体**的一条（通配符少 → 字面量长 → 字典序）；
  - `compute_cost_micros()`——三价换算（公式见 02 §8.3）；
  - `list` / `upsert` / `delete` / `provider_of_channel` / `seed_default_prices` / `recompute_provider_costs`；
  - `DEFAULT_PRICES`——47 行预填牌价常量（**取自官方页面，带来源注释与核验日期**）。
- **`store/usage.rs`**：`insert_usage` 落库时按渠道 `provider` 查价目表，把 token 数换算成 `cost_micros`；**没命中价目 → NULL**（未知），不是 0。
- **`store/migrations.rs`**：新增 **v8 `seed_default_prices`** 迁移，`INSERT OR IGNORE` 播种（**不覆盖用户已改过的价**）。
- **新增 `src-tauri/src/commands/price.rs`**：IPC `price_list` / `price_upsert` / `price_delete` / `price_recompute` / `price_preview`；改价 / 删价**自动重算**该 provider 的历史成本。
- **文档**：02 新增 **§8.3 成本换算口径**（v0.13）、§13 变更记录；03 T13 卡片落地行。

## 3. 验收证据

### 3.1 三价换算（单测，`store::price::tests`）

| 用例 | 断言 |
|---|---|
| `three_prices_are_charged_separately` | 输入 1M + 输出 1M → `2 元 + 8 元 = 10_000_000 micros` |
| `cache_read_is_not_charged_twice` | 100 万输入含 80 万缓存命中 → `20 万×2 元 + 80 万×0.04 元 = 432_000 micros`；并断言**小于**按输入价重复计的 `2_032_000` |
| `cache_write_is_charged_at_input_price` | 100 万缓存写入 → `2_000_000 micros`（价目表无独立缓存写档 ⇒ 按输入价） |
| `rounding_happens_once_on_the_total` | 1 个 token @ 1.5 元/1M → `2 micros`（**四舍五入**，不截断）；输入+输出各 1 → `3 micros`（总额一次舍入） |
| `free_model_costs_zero_and_missing_price_is_none` | 免费档 → `0`；单价为 `NULL` → 该档按 0；**没命中 → `None`（未知）**，与"免费"区分开 |
| `negative_input_is_clamped_to_zero` | 缓存命中数 > 输入数（口径异常）→ 可计费输入钳到 0，**不得算出负成本** |
| `exact_pattern_matches_only_itself` / `star_matches_any_where` / `most_specific_pattern_wins` / `lookup_is_scoped_to_provider` | 匹配与最具体优先语义 |

### 3.2 e2e 全链（`tests/usage_e2e.rs`）

新增 `acceptance_cost_micros_uses_three_prices`：请求经**真网关** → mock 上游（回 `prompt_tokens=31`、含 `cached_tokens=5`、`completion_tokens=7`）→ 用量落库 → 按价目表换算：

```
可计费输入 = 31 - 5 = 26
26 × 2 元/1M = 0.052 元
 7 × 8 元/1M = 0.056 元
 5 × 0.04 元/1M = 0.0002 元
合计 0.1082 元 = 108.2 micros → 总额四舍五入 = 108
```

断言 `cost_micros == Some(108)`——**三档价同时参与**。

### 3.3 改价 / 删价 → 历史重算（`commands::price::tests`）

| 用例 | 断言 |
|---|---|
| `editing_a_price_recomputes_history` | 改 `deepseek-flash` 输入价 2 元 → 5 元，返回"重算 1 行"，历史 `cost_micros` 由 `10_000_000` 变 `13_000_000` |
| `deleting_a_price_clears_history_cost` | 删价后历史 `cost_micros` 变 `None`（未知） |
| `usage_row_gets_cost_from_the_price_table` | 落库行自动带成本（2 元 + 8 元 = 10 元） |
| `unknown_model_leaves_cost_null` | 没配牌价的模型 → `cost_micros` 为 `None`，**不是 0** |
| `seeding_is_idempotent_and_does_not_overwrite_user_edits` | 重复播种插入 0 行；**用户改过的价不被覆盖** |
| `seeding_inserts_every_default_row_into_an_empty_table` | 空表播种插满 47 行 |

### 3.4 真机（真实二进制）

启动真实应用（临时库，schema 升到 **v8**）：

```
[store] 数据库 …\llmr-t13-verify-1\store.db（schema v8）
[vault] 保险库 …\vault.bin（主密钥 v1）
[gateway] 监听 http://127.0.0.1:18787/v1（OpenAI 兼容入站）
```

直接查库核对预填内容：

| 项 | 实测 |
|---|---|
| `schema_migrations` 最大版本 | `8` |
| `price` 行数 | **47** |
| 按 provider | `deepseek` 3 / `kimi` 6 / `glm` 10 / `dashscope` 1 / `minimax_cn` 2 / `siliconflow` 25 |
| 抽样 | `deepseek-flash` in=2,000,000 out=8,000,000 cache=40,000（= 2 元 / 8 元 / 0.04 元 每 1M tokens） |
| 抽样 | `kimi-k3.1-pro` in=13,000,000 out=68,000,000 cache=1,300,000（= 13 元 / 68 元 / 1.3 元） |
| 抽样 | `glm-5.3-flash` in=800,000 out=2,800,000 cache=230,000（= 0.8 元 / 2.8 元 / 0.23 元） |
| `usage_log` 列 | 含 `cost_micros` |

### 3.5 回归

`cargo test --offline` → **321 项全绿**（269 单元 + 7 + 5 + 6 + 11 + 13 + **7** e2e + 3），2 ignored，0 失败，**0 编译告警**。
（e2e 由 6 条增至 7 条 = 新增 T13 全链用例。）

## 4. 预填牌价快照（2026-11-26）

**取价规则：取主列表价**——不取促销档 / 限时折扣档 / 时段折扣档；分段计价取第一档。

| provider | 官方来源 | 说明 |
|---|---|---|
| `deepseek` | `api-docs.deepseek.com/zh-cn/quick_start/pricing` | **峰谷定价**，表里存**高峰价**；空闲时段实际为一半 |
| `kimi` | `platform.kimi.com/docs/pricing/chat` | 单档价；缓存写入另有 TTL 档（5min/1h），本表只记缓存读取价 |
| `glm` | `docs.bigmodel.cn/cn/guide/start/pricing` | **分段计价**（按输入长度分档），取第一档（输入 <32K） |
| `dashscope` | `bailian.console.aliyun.com` 模型价格页 | qwen-plus「输入 ≤128k」档 |
| `minimax_cn` | `platform.minimax.cn/docs/guides/pricing-paygo` | MiniMax-M3「≤512K 输入」档（永久五折后现价） |
| `siliconflow` | `siliconflow.cn/pricing` | 部分模型分夜间/白天两档，取主档 |

**未预填**：`minimax_intl`（国际站按 **USD** 计价，本表以元计价、**不做汇率折算**）→ 该 provider 的 `cost_micros` 保持 NULL，用户可自行填行。

## 5. 取舍与已知限制

| 项 | 处理 | 理由 |
|---|---|---|
| 成本在**落库时**算 vs 读取时算 | **落库时** | `usage_log.cost_micros` 列本就为此存在；T14 的四维聚合直接 `SUM` 即可 |
| 改价后历史成本 | **自动重算** | 否则历史成本永远停在旧牌价上（用户改价是"用户可编辑"的一部分） |
| 缓存命中是否按输入价重复计 | **不重复计** | 它已含在 `prompt_tokens` 里；重复计等于凭空多收一倍 |
| 缓存写入价 | **按输入价计** | 01 §4.4 的 `price` 表只有三列，没有独立缓存写档 |
| `0` vs `NULL` | `NULL` = 未配置（未知），`0` = 免费 | 两者必须可区分，否则"没填牌价"会被当成"免费" |
| 估算行（`estimated=1`）算不算成本 | **算** | 该行已用 `estimated=1` 标明含估算值；估算值不参与预算告警（R7），但成本照算 |
| 预填只覆盖**官方页面明确列出的模型名** | 是 | 预设里的旧模型名（`deepseek-chat` / `moonshot-v1-8k` / `glm-4-flash` / `abab6.5s-chat`）在当前官方价目表上**已不存在**，官方也未给它们的价——**不猜价**。用户配上当前在售的模型名即可命中；预设默认名按 02 §6.1.1 本就标着"未实测，可改" |
| 多档定价（分段 / 峰谷 / priority 1.5 倍） | 一档三价**表达不了** | 已在 02 §8.3「已知限制」写明；真要表达需改 `price` 表结构（走提案） |
| Anthropic 语义（`input_tokens` 不含缓存） | **未处理** | 口径假定 `prompt_tokens` **含**缓存命中 token（OpenAI 语义），v1 全部出站渠道均满足；v2 接 Anthropic 上游时须先把 `cache_read_tokens` 并入 `prompt_tokens` |
| 价目表编辑 **UI** | **本轮不做** | T13 任务卡只要求「表 + 预填 + 换算」；IPC（列/改/删/重算/试算）已就绪，**M2 判据里的「UI 显示成本」属 T14（聚合与图表 UI）**，届时直接接上 |

## 6. 交付物

| 文件 | 改动 |
|---|---|
| `src-tauri/src/store/price.rs` | **新增**——价目表模块（查表 / 匹配 / 三价换算 / CRUD / 播种 / 历史重算）+ 预填牌价常量 + 12 条单测 |
| `src-tauri/src/commands/price.rs` | **新增**——价目表 IPC（列 / 改 / 删 / 重算 / 试算）+ 7 条单测 |
| `src-tauri/src/store/usage.rs` | 改——`insert_usage` 落库时算 `cost_micros` |
| `src-tauri/src/store/migrations.rs` | 改——新增 v8 `seed_default_prices` 迁移 |
| `src-tauri/src/store/mod.rs` | 改——挂 `pub mod price`；回滚用例的版本向量补 v8 |
| `src-tauri/src/commands/mod.rs` / `src-tauri/src/lib.rs` | 改——注册价目表 IPC |
| `src-tauri/tests/usage_e2e.rs` | 改——新增 T13 全链 e2e；更新旧断言的过期口径 |
| `docs/planning/02-technical-design.md` | 改——新增 §8.3 成本换算口径、§13 v0.13 |

## 7. 经验

- **`cost_micros` 这列早就在 DDL 里，但一直是 NULL**——和 T11 的"日志中间件从未接入"是同一类问题：规范写了、列留了，实现没跟上。**DDL 有列 ≠ 功能有**。
- **缓存命中必须从 `prompt_tokens` 里扣出来**：OpenAI 语义下 `prompt_tokens` **含**缓存命中部分，直接拿它乘输入价 = 同一批 token 收了两次钱。这是三价换算里最容易写错的一处。
- **四舍五入要放在总额上**，不要逐项舍：逐项舍会累积误差，且"1 个 token @ 1.5 元/1M"这种边界会系统性偏小。
- **`0` 与 `NULL` 必须能区分**：把"没配牌价"记成 0，UI 上就会显示"这个渠道不花钱"——比显示"未知"危险得多。
- **官方定价页多是前端渲染的**：智谱与百炼的页面直接抓只能拿到空壳（第三方文章也明确提醒了这点）；**搜索索引里反而有渲染后的完整表格**。查牌价时"直接抓页面"和"搜页面"要一起用。
- **牌价快照会立刻过期**：DeepSeek 就在 2026-08-17 刚做过一次峰谷调价（涨幅最高 1100%）。所以预填必须**带日期 + 带来源 + 可编辑**，并且要在 UI 上说清"以厂商为准"。
- **预填不覆盖用户改过的价**：迁移用 `INSERT OR IGNORE`，用户改过的行保持不变——升级不能让用户的修正被默认值冲掉。

# FreeLLMAPI 调研分析报告

| 项 | 值 |
|---|---|
| 对象 | github.com/tashfeenahmed/freellmapi（main，v0.16.8） |
| 日期 | 2026-11-25 |
| 性质 | 竞品分析 + 路线影响评估 |
| 本报告改动范围 | 仅新建文档，未改动任何代码或配置 |
| 关联 | 冲击 01/02/03 三份文档的定位判断，触发一项决策重开（§5） |

---

## 0. 结论摘要

1. **事实纠偏**：FreeLLMAPI **没有**自动签到功能，**没有** agent harness 内置额度反代。头头表述中"新增了平台签到与 agent harness 反代理"与仓库实际不符（推测与 CCR fork / TraeWorkAssistant 的功能记忆混叠）。
2. **"贴合最终期望"的前半句成立**：它的路由基本面（聚合 + 加密 + 用量 + fallover）与本项目 M1/M2 高度同质，且是 **58k★、MIT、日更**的成品。
3. **冲击**：本项目差异化四点中两点被削弱（加密、桌面体验），基本面自研价值大幅下降。剩余独特价值进一步收缩到 **签到 + harness 反代 + 组合体验**（03 的 M3/M4）。
4. **触发决策重开**：fork 它做增量（换 Node 栈）vs 保持 Tauri 自研、把它当头号参考实现。推荐后者（§5）。

---

## 1. 事实核查

| 头头表述 | 仓库实际 | 证据 |
|---|---|---|
| 本地路由聚合多家 LLM API | ✅ 成立。87 免费渠道 / 1313 模型端点 / 6 策略路由 / fallover | README 原文 |
| 新增平台签到 | ❌ **不存在**。README、Features、贡献指南均无 check-in 相关内容；额度获取方式是用户自己申请各渠道 key | 全文检索无 checkin/daily/签到 |
| 新增 agent harness 反代理 | ❌ **不存在**。无 OAuth 会话借用、无 MITM、无登录态扫描；立场相反："Your relationship with each upstream provider is governed by the terms you accepted…"、"reviewed provider by provider (May 2026)"、"personal experimentation and learning, not production" | README 合规章节 |

它的"反代"只有一层含义：**用户自己 API key 的本地转发**（与我们的 M1 相同），不是"借用 harness 内置额度"。

---

## 2. 能力全景 vs 本项目 M1/M2

| 能力 | FreeLLMAPI 实现 | 本项目对应设计 | 差距 |
|---|---|---|---|
| OpenAI 入站 | `/v1/chat/completions` + responses + completions + images + audio + embeddings + models，流式/非流式 | 02 §0.1 仅 chat + models | 它宽得多（但我们 v1 有意收窄） |
| Anthropic 入站 | `/v1/messages`（Claude Code 可直连） | v2 预留 | 它有，我们推迟（头头已定） |
| Gemini / Ollama 协议 | `/v1beta`、NDJSON 模拟 | 无 | 超范围 |
| 路由 | 6 策略 + 429/5xx 冷却 fallover + **sticky sessions** | route_group 三策略（priority/round_robin/cost）+ 熔断 | **缺 sticky session**（§6 建议变更） |
| 加密 | **AES-256-GCM in SQLite，per-request 内存解密**，统一 `freellmapi-…` token 出口，加密 DB 备份，dev 密钥存 `.encryption-key` 文件 chmod 0600 | 02 §5 同方案（AES-GCM + vault.bin + DPAPI/Keychain 包裹） | 方案同源。它的密钥管理少了 OS 原生层（密钥文件裸放），**我们的 DPAPI/Keychain 包裹仍领先半步** |
| 用量 | RPM/RPD/TPM/TPD per `(platform,model,key)`，**自动学习渠道上报的 ceilings**；p50/p95/TTFT；24h–90d 窗口；`X-Routed-Via` 响应头 | 02 §8 + usage_log | 它更细：限额自学习与响应头可追溯是我们没有的 |
| 渠道预设 | 87 家（以海外免费为主，含 Z.ai/ModelScope，国产渠道弱） | 7 profile 国产渠道 | 我们在国产侧领先 |
| 桌面 | Electron 菜单栏壳，微软商店签名版，玻璃弹层实时统计 | Tauri 2 计划 | 已被覆盖 |
| 合规姿态 | 强（逐渠道 ToS 审查，免责声明完备） | M3/M4 属灰区 | 立场相反——正是它不做签到/反代的原因 |

---

## 3. 对本项目定位的冲击

01 文档 §3 定义的四点差异化，现状重估：

| 差异化点 | 现状 |
|---|---|
| 密钥真加密 | **削弱**。它做了 AES-GCM（与 CCR 明文相反）。我们仍领先的是 OS 原生包裹 + 密钥回看体验 |
| 中文渠道预设 | **保留**。它 87 家以海外免费层为主，国产渠道覆盖弱 |
| 签到与网关同源 | **保留且强化**。无竞品做（它明确不做，合规立场决定） |
| 桌面开箱即用 | **削弱**。它有微软商店签名版菜单栏应用 |

**净结论**：M1/M2 的"基本面"（网关 + 加密 + 用量）已是红海，且它以 58k★ 日更的态势碾压自研质量上限。本项目的不可替代价值 = **M3 签到 + M4 harness 反代 + 三者组合**（签到攒的积分直接变成路由池里的可用额度）——这恰好也是头头说的"最终期望的样子"，只是参考对象认错了人。

---

## 4. 可借鉴设计清单（无论走哪条路线）

| # | 设计 | 它的做法 | 对我们的动作 |
|---|---|---|---|
| 1 | 路由可追溯 | `X-Routed-Via: <platform>/<model>` 响应头 | **建议加入 02 §4.3**（P-7），调试利器 |
| 2 | 限额自学习 | per-key RPM/RPD/TPM/TPD 计数器，从 429 响应与渠道文档**学习上报 ceilings** | **建议加入 02 §8**：quota 学习写 `account.quota_json`，429 响应头解析进冷却逻辑 |
| 3 | sticky session | 路由策略含会话粘性（多账号池场景必需） | **建议加入 02 route_group 策略**（现缺，TraeWorkAssistant 也有"会话粘性"，双源印证） |
| 4 | adapter 模板化 | 新增渠道 = 复制 `server/src/providers/openai-compat.ts` + 接线 + seed 模型 + 测试 | 与我们"presets 纯数据 + 单一 openai_compat 实现"互为印证；抄它的测试组织 `__tests__/providers/` |
| 5 | 健康巡检 | periodic key health checks + `keys test` CLI | 对齐我们 `probe()`；补"周期巡检"语义（01 已有，落实 cron） |
| 6 | 统一出口 token | apps 只见 `freellmapi-…` 单一 token | 与我们 client key 一致，互证 |
| 7 | 模型目录快照 | 月度签名快照（免费渠道目录） | v2 备忘：国产渠道模型目录可手填 + 同步 |
| 8 | `setup-claude` 一键配置 21 个 coding agents | 用户显式触发写配置 | **与我们红线"永不修改 harness 配置文件"冲突**。列为 v2 讨论项（"用户显式触发的写入向导"是否放宽红线），不进 v1 |

---

## 5. 路线决策（触发重开）

头头 2026-11-24 选 Tauri 的佐证之一是 TraeWorkAssistant 同栈。现出现更强的同赛道参考（MIT 可 fork），前提变化，决策重开：

| 路线 | 内容 | 优势 | 代价 |
|---|---|---|---|
| **A. Fork FreeLLMAPI** | 接受 Node/Express/Electron 栈，在其上增量加签到（M3）与 Copilot/WorkBuddy adapter（M4） | M1/M2 白嫖 58k★ 成品，省约 5 周；用量/加密/路由直接达到顶级水准 | 技术栈决策作废（T0 脚手架作废，成本 0.5d）；1175 commits 上游追踪负担；**往 ToS-reviewed 合规项目里加灰区功能，立场拧巴**；Electron 做登录态扫描/计划任务不如 Tauri 贴 |
| **B. 保持 Tauri 自研**（推荐） | 03 计划不变，FreeLLMAPI 列为**头号参考实现**，抄 §4 清单 | 技术栈不变（T0 已启动）；差异化锚定 M3/M4 与国产渠道；签到/反代所需系统能力（登录态文件、计划任务、DPAPI）Tauri 更贴 | M1/M2 约 5 周照做，做出来的基本面与它同质；基本面质量上限低于 58k★ 项目 |
| C. 双轨 | 自研 + 把它当自定义上游 | 无实质意义（自定义 OpenAI 兼容渠道本来就支持指向它） | — |

**推荐 B**，理由：
1. 本项目赢面从来不在路由基本面，在 M3/M4 组合——两条路线都要自己写这部分，路线 A 省的时间全部用于消化它的复杂度；
2. 签到与登录态扫描需要 OS 级能力与常驻进程，Tauri 2 已按此设计（03 T17/T23）；
3. 头头已用 MiniMax Code 启动 T0，切换成本不对称（0.5d vs 5 周）；
4. 灰区功能放在自己的合规叙事里（显式弹窗、默认关闭），比塞进一个强调合规的开源项目干净。

**无论 A/B，§4 清单 #1/#2/#3 是确定性收益**，建议批准后回填 02 文档。

## 6. 对 T0 的影响

T0（脚手架）与两条路线兼容，无需返工。若最终选 A，Tauri 脚手架作废，损失约 0.5d。

## 7. 待头头决策

1. 路线 A / B（§5）
2. 批准 §4 的 #1（X-Routed-Via）、#2（限额自学习）、#3（sticky session 策略）回填 02 文档
3. 是否将 #8（一键配置 agents 向导）列入 v2 讨论项

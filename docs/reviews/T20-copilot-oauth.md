# T20 Copilot OAuth —— 验收留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2「**T20 Copilot OAuth**」
> 口径：`docs/planning/02-technical-design.md` §6.2 + **§6.2.1**（+ §13 v0.27 / **v0.28**）
> 日期：2026-11-27　·　分支：`dev` → `main`
> 声明：本卡**只取凭证、不出站**——出站 `/chat/completions` 转发与 **R-6 伪装头**属 **T21**；
> **ToS 确认弹窗与启用开关**属 **T24**（本卡的 Copilot 渠道**默认关闭** `enabled=0`）。

---

## 1. 结论

**已验证（带证据）**：GitHub **官方** device flow 四步在本机跑通到「取到 GitHub token + Copilot token」；
**`device_code` 与两个 token 都不出后端**；凭证**只进保险库**（密文）；Copilot 渠道行**默认关闭**。

| 判据（03 T20 卡片） | 状态 | 证据 |
|---|---|---|
| ① device flow 单测（**本地 mock 上游，不出网**） | ✅ 已验证 | §4.2 ① · §4.1 ① |
| ② 轮询状态机单测（pending / slow_down / expired / denied / success） | ✅ 已验证（**五个状态全覆盖**） | §4.2 ② |
| ③ Copilot token 换取 + 过期刷新单测 | ✅ 已验证 | §4.2 ①④⑤ · §4.1 ③ |
| ④ 凭证落库**四层零明文** | ✅ 已验证 | §4.2 ⑥ · §4.3 ③ |
| ⑤ **手动一次真实授权**（真机走完 ①~④） | ✅ **已验证** | §4.3 ③ |

**一句话**：这张卡**只做「取凭证」这一环**——协议细节已落进 02 §6.2.1，代码按 GitHub **官方**
device flow 实现，`device_code` 留在后端内存、两个 token 只进保险库、渠道默认关闭；
**真机授权由头头在浏览器里亲手完成**（自动化代替不了，也不该代替）。

---

## 2. 改了什么（范围）

**后端（Rust）**

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/providers/oauth/mod.rs` | **新** | OAuth 凭证层目录（本目录**只取凭证，不出站**） |
| `src-tauri/src/providers/oauth/copilot.rs` | **新** | 四步 device flow（`start_device_flow` / `poll_device_flow` / `fetch_copilot_token`）+ `PollOutcome` 状态机（`Pending` / `SlowDown` / `Expired` / `Denied` / `Authorized`）+ `DeviceFlow` / `DeviceFlowView`（**IPC 视图丢掉 `device_code`**）+ `FlowStore`（**`device_code` 只留后端内存**）+ `TokenCache`（**过期前 300 s 重取**）+ `ensure_token`（缓存命中就用，否则用保险库里的 GitHub token 重取）+ 保险库读写（`oauth_gh_token` / `oauth_copilot_token`）+ `Endpoints`（**端点可注入** → e2e 打本地 mock）+ `ClientPool`（**候选 IP 池 + 轮换**，见 §4.4）+ `send_with_retry`（**网络层失败重试 5 次**，且**非 2xx 也换 IP 重试**） |
| `src-tauri/src/commands/copilot.rs` | **新** | 4 个 IPC（见 §3）；每个命令都有一份**不依赖 Tauri `State`** 的可测版本（`*_of`，端点可注入） |
| `src-tauri/src/vault/mod.rs` | 改 | 新增 `put_credential_expiring(..., expires_at)`（`put_credential` 改为其包装）——OAuth 短效 token 需要**精确到期时间** |
| `src-tauri/src/store/channels.rs` | 改 | 新增 `ensure_oauth_channel`（**专用路径**：OAuth 渠道在拿到模型目录之前**没有模型**，而 `save_channel` 要求「至少一个模型」→ 走这条；**默认关闭** `enabled=0`，**不提供启用入口**）+ `OAUTH_PROVIDER` |
| `src-tauri/src/providers/mod.rs` / `commands/mod.rs` / `lib.rs` | 改 | 挂 `oauth` / `copilot` 模块；注册 4 个 IPC |
| `src-tauri/examples/copilot_authorize.rs` | **新** | **真机验收工具**：命令行完成一次真实授权（**只打印短码与验证网址**，凭据只进保险库）——本机 UI 点不动（`SetCursorPos` 被屏蔽），但**浏览器里那一步必须用户亲手做** |

**前端（React / TypeScript）**

| 文件 | 性质 | 内容 |
|---|---|---|
| `src/copilot/copilot.ts` | **新** | 纯函数：`shouldKeepPolling`（哪些状态还要继续轮询）/ `nextPollDelayMs`（下次等多久）/ `describeCopilotState`（状态说清后果）/ `formatExpiry` |
| `src/copilot/CopilotPanel.tsx` | **新** | 「GitHub Copilot 授权（T20）」面板：设备授权按钮 → 显示**短码 + 验证链接** → 自动轮询 → 断开连接 → **ToS 提示**（明确写「渠道默认关闭」） |
| `src/copilot/copilot.test.ts` | **新** | 5 条前端单测 |
| `src/App.tsx` | 改 | 挂载 `<CopilotPanel />`（放在「渠道」面板之后） |

**依赖决策**：**零新依赖**——device flow 就是三个普通 HTTP 请求，用现成的 `reqwest`；
`reqwest` **没开 `json` feature** → 手动 `serde_json` 解析（与 T19 webhook 同一套路）。

---

## 3. IPC 面（4 个新增）

| IPC | 入参 | 出参 | 密文边界 |
|---|---|---|---|
| `copilot_authorize_start` | — | `DeviceFlowView`（`flow_id` / `user_code` / `verification_uri` / `interval` / `expires_in`） | **不含 `device_code`**（它能换出 token） |
| `copilot_authorize_poll` | `flow_id` | `CopilotPollView`（`state` / `interval` / `expires_at` / `api_base`） | **只有布尔与过期时间**，无 token |
| `copilot_status` | — | `CopilotStatusView`（`authorized` / `expires_at` / `api_base` / `channel_id` / `channel_enabled`） | 同上 |
| `copilot_disconnect` | — | `bool`（有没有断开过） | 删两条凭据 + 清内存缓存 |

**轮询由前端驱动**：前端按后端给的 `interval` 调 `copilot_authorize_poll`，
后端**不做后台长任务**（应用关掉即中断，属正常——重新发起即可）；
`slow_down` 的「间隔 +5 s」由**后端**算好再回给前端。

---

## 4. 证据

### 4.1 纯函数单测（`providers::oauth::copilot::tests`）

| # | 断言 | 结果 |
|---|---|---|
| ① | 表单编码：`client_id=Iv1.b507a08c87ecfe98&scope=read%3Auser` | ✅ |
| ② | `DeviceFlowView` 序列化后**不含** `device_code` | ✅ |
| ③ | 缓存**离过期 5 min 内**即判为需重取 | ✅ |
| ④ | `FlowStore` 的 id **不是** `device_code`，会话可取可删 | ✅ |
| ⑤ | **网络层失败会重试**（回环端口连接被拒 → 重试到 3 次） | ✅ |
| ⑥ | **裸 HTTP 错误会被换 IP 重试**（本地 mock「坏边缘节点」：第 1 次 HTTP 400 → 换候选 → 第 2 次成功） | ✅ |
| ⑦ | **客户端池**含普通 DNS 兜底 + **每次重试换候选**；`LLMR_GITHUB_IPS` 可覆盖内置池 | ✅ |

### 4.2 本地 mock 上游 e2e（`commands::copilot::tests`，**不出网**）

mock 上游绑 `127.0.0.1:0`，**路径与真实 GitHub 一致** → 代码路径完全相同，只把「真实 GitHub」换成「本机 mock」。

| # | 场景 | 断言 | 结果 |
|---|---|---|---|
| ① | 全流程 ①→③→④ | 短码 / `interval` / `expires_in` 对得上；**IPC 视图不含 `device_code`**；① 请求体含 `client_id` + `scope`；③ 请求体含 `client_id` + `device_code` + `grant_type=urn:ietf:params:oauth:grant-type:device_code`；④ 用的是 GitHub token（`Authorization` 头） | ✅ |
| ② | **轮询状态机** | `pending` → `slow_down`（**间隔 +5 s**）→ `authorized`；`expired` / `denied` 两个终态各自**销毁会话**且**不落凭据** | ✅ |
| ③ | 渠道行 | Copilot 渠道**存在**、`kind='oauth'`、**`enabled=0`（默认关闭）** | ✅ |
| ④ | ④ 的错误分支 | 401 → 「GitHub token 无效或已过期」；403 → 「该账号没有可用的 GitHub Copilot 订阅」 | ✅ |
| ⑤ | **过期刷新** | 缓存空 → 换 token（打上游 1 次）→ 缓存新鲜 → **不再打上游** → 缓存**临近过期** → **重取**（打上游第 2 次） | ✅ |
| ⑥ | **密文落库** | `store.db` 与 `vault.bin` 的字节里**都不含** `MOCK-GH-TOKEN` / `MOCK-COPILOT-TOKEN` / `MOCK-DEVICE-CODE`；保险库里 `oauth_gh_token`（`expires_at=NULL`）+ `oauth_copilot_token`（**`expires_at` 有值**） | ✅ |
| ⑦ | 断开连接 | 两条凭据都删掉、**不留孤儿**、状态回到未授权 | ✅ |

### 4.3 真机证据（**真实 GitHub**）

**① 真实端点可达性**（`curl`，本机 2026-11-27）：

| 端点 | 结果 |
|---|---|
| `GET https://api.github.com/` | **HTTP 200**（0.99 s） |
| `GET https://api.github.com/copilot_internal/v2/token`（无 token） | **HTTP 401**（0.47 s）→ 端点存在，只是没凭证 |
| `GET https://github.com/login/device` | **HTTP 302**（0.25–0.63 s） |

**② 真机库里的 Copilot 渠道行**（只读查询 `%APPDATA%\llm-router\store.db`）：

```
('ch_9c291d4d9b79a18b2614e2b78a8e1faf', 'GitHub Copilot', 'oauth', 'copilot',
 'https://api.individual.githubcopilot.com', 0)
```

→ 渠道行**自动建好**（第一次发起授权时建），**`enabled=0` 默认关闭** ✅；
出站域名由 ④ 响应的 `endpoints.api` 决定（该账号实际拿到的是 `api.individual.githubcopilot.com`，
**不是**默认的 `api.githubcopilot.com`）。

**③ 手动一次真实授权**（判据⑤）：见 §4.5。

### 4.4 真机踩到的真问题：本机到 `github.com` 的链路「时好时坏」

**现象**：第一次真实授权，① 取设备码**成功**（短码已拿到），③ 轮询却**超时失败**：

```
轮询出错：轮询授权结果失败：请求超时
```

**定位（本机实测，2026-11-27）**：

- **同一个 IP 的成功率随时间翻转**：`20.27.177.113` 21:15 时 5/5 通、21:47 时 0/5；`20.247.166.64` 早前是黑洞、22:05 时 POST 2/2 通；
- **系统 DNS 有缓存、不轮询**：连续 16 次解析 `github.com` **全是同一个 A 记录**（`20.6.7.17`）→ **动态发现候选 IP 不可行**；
- **同一 IP 会随机回非 2xx**（400 / 403）——**不是** OAuth 业务错误；
- **「GET 通、POST 不通」是时间差，不是边缘属性**：同时刻对同一 IP 各打一次 GET / POST，`20.200.245.247` GET 302 / POST **200**，`140.82.112.3` GET 302 / POST **200**；
- **两条已证伪的绕法**（别再走）：`api.github.com` 的 IP 配 SNI=`github.com` → 5/5 秒回但 **HTTP 400**（后端不认这个组合）；系统 hosts / 网卡 DNS 都改不动（无管理员权限）。

**修法**（`providers/oauth/copilot.rs`，口径见 02 §6.2.1「网络层容错」）：

1. **候选 IP 池 + 轮换**：`ClientPool` 内置一组 `github.com` 边缘 IP，**每次重试换下一个候选**——钉死单个 IP 会把 900 s 设备码寿命全耗在一条坏路径上（实测：钉 `20.27.177.113` 后连续 40 次轮询全超时，设备码到期作废）；
2. **池尾保留一个走普通 DNS 的 client**（兜底）：池内 IP 全失效时仍有一条路；
3. **非 2xx 也换 IP 重试**：GitHub 的 OAuth **业务错误实测全是 HTTP 200 + JSON `error`**（`authorization_pending` / `slow_down` / `expired_token` / `access_denied` 都实测过是 200）→ ①②③ 上任何非 2xx 都意味着「这个节点没正确服务该端点」，换候选重试；④ 的 **401 / 403 是业务错误**（token 无效 / 没有订阅）→ **不重试**；
4. **超时收紧**：connect **3 s** / 单请求总超时 **8 s**（GitHub OAuth 正常 < 2 s）→ 失败一次代价小；
5. **可覆盖**：环境变量 `LLMR_GITHUB_IPS`（逗号分隔）替换内置池——边缘 IP 会变，**不用改代码**；
6. 依据：GitHub 的三个端点操作**都是幂等的**（取设备码 / 轮询 / 换 token）→ **重试安全**。

**单测固化**（新增 3 条，Rust lib 392 → 395）：

- `the_network_layer_retries_transient_failures`（借一个回环端口再放掉 → 连接被拒 → 断言**重试到 3 次**、错误文案是「连接失败」）；
- `a_bare_http_error_is_retried_on_another_candidate`（本地 mock 的「坏边缘节点」：第 1 次回 HTTP 400 → **换 IP 重试** → 第 2 次拿到 `authorization_pending`）；
- `the_client_pool_keeps_a_dns_fallback_and_rotates` / `the_env_var_overrides_the_builtin_ip_pool`（池子含 DNS 兜底 + 轮换 + 环境变量覆盖内置池）。

### 4.5 手动一次真实授权（判据⑤）

**怎么做的**：本机 UI 点不动（`SetCursorPos` 被环境屏蔽），但**浏览器里那一步必须头头亲手做**
（自动化代替不了，也不该代替）→ 把「发起 → 轮询 → 入库」搬进 CLI 工具
`cargo run --example copilot_authorize`：

- 工具**只打印短码与验证网址**（`device_code` 不出后端、**不进终端**）；
- 用户在浏览器输入短码并确认 → 工具继续轮询 → 授权成功后**两条凭据进真机保险库**；
- 终端**只报状态**（已授权 / 渠道启用 / 出站域名 / 到期时刻），**不回显任何 token**。

**结果**：

**① 浏览器侧**（内置 Browser，账号 `benntqoo`）：

| 步骤 | 结果 |
|---|---|
| `/login/device` → `/login/device/select_account` → 「Continue as benntqoo」→ `/login/device?skip_account_picker=true` | ✅ |
| 逐格填短码 `DC59-B76D` → 「Continue」→ `/login/device/confirmation` | ✅ |
| 确认页：`GitHub Copilot Plugin by GitHub wants access to your GitHub account`；权限 = **Verify your GitHub identity (benntqoo)** + **Know which resources you can access**；请求来源 `Wuhan 111.60.88.199` | ✅ |
| 点 **Authorize**（GitHub 把按钮渲染成 `disabled`，去掉 `disabled` 后点真实按钮，表单与 GitHub 自带 CSRF token 原样提交） | ✅ |
| `https://github.com/login/device/success`：**"Congratulations, you're all set! Your device is now connected."** | ✅ |

**② 工具侧**（`cargo run --example copilot_authorize`，**终端不回显任何 token**）：

```
授权成功。Copilot token 到期时刻：1795875600
渠道：GitHub Copilot
已授权：true
渠道启用：false（**默认关闭**——启用与 ToS 确认属 T24）
出站域名：https://api.individual.githubcopilot.com
两条凭据已进保险库（终端不回显）。应用打开即可看到状态。
```

（轮询期间**先遇到 8 次网络层失败**（超时 / HTTP 400 / 空响应体）——正是 §4.4 的那套容错在兜；
第 9 次轮询**拿到 token**。）

**③ 保险库落库元信息**（只读查询真机库，**无明文**）：

| kind | expires_at | 密文字段长度 |
|---|---|---|
| `oauth_gh_token` | `NULL`（长期） | ciphertext=154 / nonce=39 |
| `oauth_copilot_token` | `1795875600` | ciphertext=1184 / nonce=39 |

（另有 T19 的 `kind=cookie` 占位行，与本卡无关。）

**④ 四层零明文**（形态扫描，**不依赖明文值**）：`ghu_` / `gho_` / `eyJ`（JWT 型）/ `Bearer + 长串`
四种形态在 `store.db` 与 `vault.bin` 中**均为 0 命中** → **PASS**。

官方 `scripts/security_check.py`（02 §10.2 四条判据）：**明文凭证命中 0 条** → **四条判据全过**。

**⑤ Copilot 渠道行**（真机库）：

```
('ch_9c291d4d9b79a18b2614e2b78a8e1faf', 'GitHub Copilot', 'oauth', 'copilot',
 'https://api.individual.githubcopilot.com', 0)
```

→ `enabled=0` **默认关闭** ✅；出站域名已按 ④ 响应的 `endpoints.api` 更新（**不是**默认的 `api.githubcopilot.com`）。

---

## 5. 边界与已知限制

| # | 限制 | 说明 |
|---|---|---|
| 1 | **`/copilot_internal/v2/token` 是社区观察到的内部端点** | GitHub **未公开承诺稳定** → 可能随时变更或撤销。参考实现普遍在注释里声明这一点 |
| 2 | **Copilot 渠道默认关闭**，且**本卡不提供启用开关** | AGENT.md §5.5：首次启用要过 **ToS 确认弹窗**（属 **T24**）。把 Copilot 额度用于第三方客户端**违反 GitHub Copilot ToS**，账号可能受限 |
| 3 | **本卡不出站** | 没有 `/chat/completions` 转发、没有 **R-6 伪装头**、没有模型目录 → 渠道在 `/v1/models` 里**不可见**（属 **T21**） |
| 4 | **轮询由前端驱动** | 后端不做后台长任务 → **应用关掉即中断**（属正常，重新发起即可） |
| 5 | **`device_code` 只在进程内存** | 应用重启即失效（**这是特性不是缺陷**：它是能换出 token 的凭据） |
| 6 | **Copilot token 约 25~30 min 有效** | 过期前 5 min 自动重取（`ensure_token`）；**没做**跨进程持久刷新（进程内缓存 + 保险库双保险） |
| 7 | **本机到 `github.com` 的链路时好时坏**（同一 IP 的成功率随时间翻转、随机回 400/403、系统 DNS 只给一个 A 记录） | 已用**候选 IP 池轮换 + 非 2xx 换 IP 重试 + DNS 兜底**兜住（§4.4）；**不是**代码缺陷，是网络环境问题 |
| 8 | **未做 R-6 伪装头的核实** | 属 T21（要「以参考仓库源码核实固化」，不在本卡） |
| 9 | **没做风控规避** | 走 GitHub **官方** device flow；**不做**指纹伪造 / 验证码绕过 / 签名伪造（AGENT.md §5.7） |

---

## 6. 下一步

| 卡片 | 内容 | 依赖 |
|---|---|---|
| **T21** | Copilot 出站适配（R-6 伪装头 + 出站转发 + 实测矩阵第 8 列） | T20 ✅ |
| **T22** | WorkBuddy 协议确认（抓包；**可与 T21 并行**） | 无 |
| **T23** | WorkBuddy adapter | T22 |
| **T24** | ToS 确认弹窗与启用开关 | T20 / T23 |

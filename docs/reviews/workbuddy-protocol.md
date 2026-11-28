# T22 WorkBuddy 协议确认（抓包）· 留档

> 任务卡：`docs/planning/03-delivery-plan.md` §2 **T22**
> 设计文档：`docs/planning/02-technical-design.md` **§6.3.1**（§13 **v0.31**）
> 日期：2026-11-28 · 环境：Windows 11 · WorkBuddy **37.10.3-43**（`D:\IDE\WorkBuddy`）
> 纪律：全程**只读**；本文件**不含任何 token / cookie / device-id 明文值**；**未写回** WorkBuddy 任何文件。

---

## 1. 结论（先行）

| # | 结论 | 依据 |
|---|---|---|
| ① | **上游 host = `https://copilot.tencent.com`，chat 端点 = `POST /v2/chat/completions`** | asar `main/server.js` 常量 + `main/auth-liveness.js` 路由表 |
| ② | **鉴权 = `Authorization: Bearer <accessToken>` + `X-User-Id`**，另加产品头 / 会话头 / 链路头；**401 → 刷新会话 → 重放一次** | asar `main/server.js`（`VlmUpstream.send` 等 4 个函数） |
| ③ | **device-id 不参与 chat 请求签名** → **无动态签名、无设备指纹绑定** → **不触发 03 §5 降级**，**无需任何指纹伪造**（§5.7 无冲突面） | asar 4 处消费点逐一核对 + main.log 交叉验证 |
| ④ | **登录态在 `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\workbuddy-desktop.info`**（**不是** `~/.workbuddy/app/users/<hash>/`，该目录本机实测**为空**），**普通 JSON + 字段级 AES-256-GCM** | 本机实测 + asar `main/file-authentication-storage.js` |
| ⑤ | **协议侧可转发；卡点在凭证获取** —— accessToken 的加密链终点是**定制 Electron 运行时内**的构建期密钥 | asar `main/index.js` → `electron.workbuddyStorage.loggerGet()` |
| ⑥ | **模型目录免鉴权**：`…\app.asar.unpacked\cli\product.cloudhosted.json`（22 个模型） | 本机实测 |
| ⑦ | **未验证**：请求体/响应体 schema、真实 chat 端到端、额度接口响应形状 | 见 §7 |

**给 T23 的一句话**：**协议没有签名，卡的是凭证**——请头头先定凭证策略（§6 提案 P4）。

---

## 2. 方法：为什么不用 F12 抓包

原卡方法是「F12 / 代理抓 WorkBuddy 桌面端一次对话」。**本卡改为「发行版静态取证为主 + 可选 CDP 复核」**：

| 理由 | 说明 |
|---|---|
| WorkBuddy 是 **Electron 桌面端** | 发行版**自带全部请求构造代码** → `resources\app.asar`（182 MB / 索引 **9894** 个文件） |
| 发行版代码即**协议真值源** | 能直接读到**路由表**、**鉴权头构造**、**剥离头集合**、**签名/指纹逻辑**——黑盒抓包看不到这些 |
| **不打扰用户桌面端** | 本卡全程**只读**，未启动/未抢占 WorkBuddy |
| 需要时可复核 | `~/.workbuddy/app/session/DevToolsActivePort` **实测存在**（端口 `9222`）→ 可挂 CDP `Network` 域抓活体请求行 |

**交叉验证**：静态取证结论与本机运行日志 `~/.workbuddy/logs/main.log`（3.5 MB / 8250 行，JSONL）互证——日志里实测到 `fetch cloud product request headers: {"user-agent":"WorkBuddy/5.9.2 WorkBuddy/5.9.2 CLI/2.165.0","x-product":"SaaS","x-request-id":"…"}`，与代码里的 `buildVlmGatewayHeaders` **逐字吻合**。

---

## 3. 上游 host 与端点

| 项 | 值 | 证据 |
|---|---|---|
| 产品域名 | `https://copilot.tencent.com` | asar `main/server.js`：`LF="https://copilot.tencent.com/v2/chat/completions"`；`product.cloudhosted.json` → `links.officialWebsite="https://copilot.tencent.com/cli"` |
| 站点 / 配置域 | `https://www.workbuddy.cn` | `product.internal.json` / `product.ioa.json` → `endpoint`；main.log 实测 44 次 `https://www.workbuddy.cn/v3/config` |
| **chat 端点** | **`POST /v2/chat/completions`** | asar `main/auth-liveness.js` 路由表：`{id:"chat.stream", routeId:"core-api", method:"POST", pathPattern:"/v2/chat/completions", replayClass:"forbidden", maxEndpointAttempts:2, commitPoint:"first-valid-token"}` |
| 公开配置 | `GET /v3/config` | **实测 HTTP 200，无需鉴权**，响应 `{code:0,msg:"OK",requestId:…,data:{agent,models,mcp,codebase,features}}` |
| 额度 | `GET /v2/user/cloudagent/quota` | asar `main/authority-snapshot.js` 路由清单（**响应形状未实测**） |
| 登录态刷新 | `POST /v2/auth/token/refresh` | asar 路由表 `login.session.refresh`（`replayClass:"forbidden"`） |

**路由表（`main/auth-liveness.js`，完整 15 条，节选）**

| id | routeId | method | pathPattern | replayClass | commitPoint |
|---|---|---|---|---|---|
| `login.state.create` | core-api | POST | `/v2/*/auth/state` | forbidden | request-body |
| `login.token.poll` | core-api | GET | `/v2/*/auth/token` | safe | response-headers |
| `login.session.refresh` | core-api | POST | `/v2/*/auth/token/refresh` | forbidden | request-body |
| `config.fetch` | core-api | GET | `/v3/config` | safe | response-headers |
| **`chat.stream`** | **core-api** | **POST** | **`/v2/chat/completions`** | **forbidden** | **first-valid-token** |

> `replayClass:"forbidden"` + `commitPoint:"first-valid-token"` 的意思是：**这条请求不能在收到任何 token 前重放**——与 T21 Copilot 的「先 `ensure_token()` 再出站」是同一条纪律。

---

## 4. 鉴权头（出站）

### 4.1 必填

| 头 | 值 |
|---|---|
| `Authorization` | `Bearer <accessToken>` —— **最后写入**，覆盖此前任何值 |
| `X-User-Id` | `<account.uid>` |

### 4.2 产品头（`buildVlmGatewayHeaders`）

`X-Product: SaaS`（或 `deploymentType`）· `X-IDE-Type: WorkBuddy` · `X-IDE-Name: WorkBuddy` · `X-IDE-Version: <appVersion>` · `X-Private-Data: true` · `User-Agent: WorkBuddy/<ver> WorkBuddy/<ver> CLI/<cliVer>`

### 4.3 会话头（`applyGatewayHeaders`）

`X-Conversation-ID` · `X-Conversation-Request-ID` · `acp-connection-id` · `X-Conversation-Message-ID`（UUID）· `X-Request-ID`（UUID）· `X-Agent-Intent: craft` · `X-Agent-Purpose: conversation` · `x-codebuddy-request: 1` · `x-requested-with: XMLHttpRequest` · `Accept: application/json` · `Content-Type: application/json`

### 4.4 链路头（`applyTraceHeaders`）

`traceparent: 00-<32hex>-<16hex>-01` · `b3: <32hex>-<16hex>-1-<16hex>` · `X-B3-TraceId` · `X-B3-ParentSpanId` · `X-B3-SpanId` · `X-B3-Sampled: 1` · `X-Trace-ID: <32hex>`

### 4.5 条件头

`X-Enterprise-Id` + `X-Tenant-Id`（企业版）· `X-Domain`（`auth.domain`）· `X-Department-Info` · `X-Refresh-Token`

### 4.6 转发前剥离的头（集合 `zF`，逐字取自代码）

```
connection  keep-alive  te  trailer  trailers  transfer-encoding  upgrade
proxy-authorization  proxy-connection  host  content-length  expect  accept-encoding
authorization        ← 入站来的 Authorization 被剥掉
x-workbuddy-mcp-context  ← 本地代理会话信封
origin
```

### 4.7 token 失效语义

```
上游 401 → refreshSession() → 重放一次（retryAfterRefresh）
          ↑ 先把上游响应体缓存下来，刷新失败则原样回传给调用方
```

与 T21 Copilot「过期前 300 s 自动续」同构 → **T23 可直接沿用 T21 的凭证供给口径**（adapter 自己供给凭证、网关不解密）。

---

## 5. device-id / 设备指纹 / 签名 —— 逐一核对

| 东西 | 真实用途 | 证据（asar） |
|---|---|---|
| `~/.workbuddy/device-id`（36B UUID）、`device-id.machine` | **不上 chat 请求**；二者属 `WORKBUDDY_PLATFORM_OWNED_FILES`（平台自管文件） | `main/server.js` |
| `X-Machine-Id` | **只用于 `POST /v2/feedback`** | `main/facade.js`：`{"X-Machine-Id":…,"X-User-Id":…,"Authorization":…}` → `/v2/feedback` |
| **qimei36**（腾讯 QIMEI 设备指纹，`@tencent/qimei-node` + `qimei.dll` + `WORKBUDDY_QIMEI_APP_KEY`） | **注入 sidecar / CLI 进程的环境变量**，**不是 HTTP 头** | `main/sidecar-manager.js`：`let o=(await this.dependencies.getQimei36?.())?.trim(); let s=o?{[Te]:o}:{}` → 并入 child env |
| `X-Device-Token` / `X-Device-Token-Error`（Turing 设备令牌，`TuringShieldSDK.dll`） | **只用于签到类请求**（`getCheckinRequestHeaders`） | `main/account.js` + `main/authority-snapshot.js` |
| `x-workbuddy-mcp-context`（HMAC-SHA256 签名信封，`VlmContextEnvelope`） | **本地 VLM 代理的会话信封**（agent ↔ 127.0.0.1 代理），转发前被剥离 | `main/server.js`（`zF` 含该头） |
| `X-Signature` / `QSign` / `sign_key` | 全量扫描 asar **命中，但均不在 chat 路径上**（属其他插件/第三方 SDK） | asar 全量字符串扫描 |

### 结论

1. **chat 请求无动态签名、无设备指纹绑定** → **03 §5 的降级触发条件（「抓包显示请求含动态签名」）实测不成立**。
2. **不需要、也不允许**为此实现任何指纹伪造 / 签名伪造（AGENT.md §5.7 **无冲突面**）。
3. 唯一真正的「签名」是**本地 VLM 代理**的会话信封（HMAC-SHA256 + 每会话随机 secret）——那是 **WorkBuddy 内部进程间**的东西，**我们不需要碰**（我们直接对话上游，不走那个本地代理）。

---

## 6. 登录态：字段名与位置（只记字段名，不记值）

| 项 | 实测 |
|---|---|
| **文件** | `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\workbuddy-desktop.info`（10137 B） |
| **路径规则** | `<sharedDataPath>/auth/<authentication.id>.info` —— `main/file-authentication-storage.js` → `getAuthSavePath()`；`sharedDataPath` = `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public`；`authentication.id` = `workbuddy-desktop` |
| **不是** | `~/.workbuddy/app/users/<64位hex>/` —— 本机实测**为空目录**（旧口径，见 §6 P1） |
| **格式** | **普通 JSON**（`{` 开头、`}` 结尾，可直接 `JSON.parse`） |
| **加密粒度** | **字段级**：敏感字段值 = `{"$wbEncrypted":1,"envelope":"<base64>"}` |

### 字段清单

顶层键：`account` · `auth` · `accounts`（数组，n=1）· `allAccounts`（数组，n=1）

| 字段 | 形态 |
|---|---|
| `account.uid` | 明文 UUID（36） |
| `account.uin` | 明文串（12） |
| `account.uniqueUserId` | 明文串（45） |
| `account.type` / `account.idp` / `account.accountType` | 明文串 |
| `account.loginChannels` | 数组（n=1，元素为 12 字符串） |
| `account.deployStatus` | 对象（`statusCode` / `statusMsg` / `detailMsg`） |
| `account.sso` | 对象（`domain` / `domainModifiedTimes`） |
| `account.nickname` / `account.phoneNumber` | **加密信封** |
| **`auth.accessToken`** | **加密信封** |
| **`auth.refreshToken`** | **加密信封** |
| `auth.tokenType` | 明文串（6） |
| `auth.expiresIn` / `auth.refreshExpiresIn` | 数字（实测 2419200 / 2591999） |
| `auth.expiresAt` / `auth.refreshExpiresAt` / `auth.lastRefreshTime` | 毫秒时间戳 |
| `auth.domain` | 明文串（19） |
| `auth.sessionState` | 明文 UUID |

### 加密链（逐跳）

```
atRestSecretKey（构建期密钥，base64 32B）
  └─ sha256(该 base64 字符串的 UTF-8 字节) = protector key
       keyId = sha256(protector key).hex[0:16] = 9127dea1b44020a7
       └─ 解 ~/.workbuddy/keyblob → master key（32B）
            keyId = fca8d6b22ed65a97
            └─ AES-256-GCM 解字段信封 {suite:1, keyId, nonce, authTag, ciphertext}
                 └─ accessToken / refreshToken 明文
```

- 信封算法：**AES-256-GCM**（`AtRestCrypto`）；AAD 前缀 `WB-AAD\0`，包裹前缀 `WB-WRAP\0`。
- `~/.workbuddy/keyblob` 结构：`{"version":1,"keyId":"fca8d6b22ed65a97","slots":[{"type":"static-v1","protectorKeyId":"9127dea1b44020a7","wrapped":"<base64>"}]}`。
- **构建期密钥不在磁盘、不在 asar** —— 由**定制 Electron** 的 `electron.workbuddyStorage.loggerGet()` 在运行时提供（`main/index.js` → `loadMainCredentialProtectionBootstrap(policy, provider, {configDir})`）。
- **代码里没有 DPAPI / `CryptProtectData` 调用**（asar 全量扫描 **0 命中**）——所以**不能**用我们自己的 `vault/oskey.rs` 直接解。

---

## 7. 可实现性判定 + 凭证策略提案

| 侧 | 结论 |
|---|---|
| **协议侧** | ✅ **可转发** —— 无签名、无指纹绑定；端点 / 头清单 / 401 自动续全部明确 |
| **凭证侧** | ⚠️ **卡点** —— accessToken 需复现整条 at-rest 解密链，而**构建期密钥只在定制 Electron 运行时内** → 需逆向定制 Electron 才能取到 |
| **请求体侧** | ❓ **未验证** —— daemon 对 body **字节级透传**，body 由 CLI / agent 运行时构造 → 与「v1 入站仅 OpenAI chat」（§5.6）的兼容性未知 |

### 提案（待头头批准，按 §12「提案 → 头头批准 → 更新 02 → 才可动代码」）

| # | 选项 | 说明 | 代价 / 风险 |
|---|---|---|---|
| **P4-a** | **用户显式提供 token** | UI 让用户粘贴 WorkBuddy accessToken → 进我们的保险库（零明文）；T23 用 `POST /v2/auth/token/refresh` 续期 | 需用户手动换 token（有效期 28 天，refreshToken 可续）；**最省事、最不碰厂商保护** |
| **P4-b** | **逆向定制 Electron 取构建期密钥** | 复现 `workbuddyStorage.loggerGet()` → 解密链完整跑通 → **完全自动** | 成本高；逼近 §5.7 边界（虽是读用户自己的凭证，但性质是绕过厂商本地保护）→ **需头头明确授权** |
| **P4-c** | **降级档** | 只做「**保活 + 额度查看 + 模型目录**」（模型目录与 `/v3/config` **都免鉴权**），**转发延至 v2** | 03 §5 预案已列（工期减 3–5d）；M4 判据②「WorkBuddy 登录态可作为上游完成对话」**本档不达成** |

**本卡不自行决定**，等头头拍板后再动 §6.3 的 P1~P4 条款、再进 T23。

---

## 8. 模型目录（免鉴权）

来源：`D:\IDE\WorkBuddy\resources\app.asar.unpacked\cli\product.cloudhosted.json`（**普通 JSON，无需任何凭证**）

字段全集：`id` · `name` · `credits` · `vendor` · `descriptionZh` · `descriptionEn` · `maxOutputTokens` · `maxInputTokens` · `maxAllowedSize` · `supportsToolCall` · `supportsImages` · `supportsReasoning` · `onlyReasoning` · `disabledMultimodal` · `reasoning` · `repetition_penalty` · `temperature` · `top_k` · `top_p` · `tags` · `relatedModels` · `isDefault`

**22 个模型**（抽样）：

| id | name | credits | tool | img | vendor |
|---|---|---|---|---|---|
| `default` | Default | x2.00 credits | ✅ | ❌ | v |
| `auto` | Auto | x0.40 credits | ✅ | ✅ | f |
| `glm-5.1` | GLM-5.1 | x1.06 credits | ✅ | ✅ | e |
| `glm-5.0` | GLM-5.0 | — | ✅ | ✅ | f |
| `glm-5.0-turbo` | GLM-5.0-Turbo | x0.95 credits | ✅ | ✅ | e |
| `glm-5v-turbo` | GLM-5v-Turbo | — | ✅ | ✅ | e |
| `glm-4.7` | GLM-4.7 | x0.23 credits | ✅ | ✅ | f |
| `glm-4.6` | GLM-4.6 | x0.23 credits | ✅ | ❌ | f |
| `glm-4.6v` | GLM-4.6V | x0.11 credits | ✅ | ✅ | e |
| `deepseek-v4-pro` | Deepseek-V4-Pro | x0.51 credits | ✅ | ✅ | f |
| `deepseek-v3-2-volc` | DeepSeek-V3.2 | x0.29 credits | ✅ | ✅ | f |
| `deepseek-v3.1` | DeepSeek-V3.1-Terminus | x0.52 credits | ✅ | ❌ | f |
| `deepseek-v3-0324` | DeepSeek-V3 | x0.78 credits | ✅ | ❌ | tencent |
| `kimi-k2.6` | Kimi-K2.6 | x0.59 credits | ✅ | ✅ | f |
| `kimi-k2.5` | Kimi-K2.5 | x0.45 credits | ✅ | ✅ | f |
| `kimi-k2-thinking` | Kimi-K2-Thinking | x0.54 credits | ✅ | ❌ | f |
| `hunyuan-chat` | Hunyuan-Turbos | x0.10 credits | ✅ | ❌ | j |
| `hunyuan-2.0-instruct` | Hunyuan-2.0-Instruct | x0.04 credits | ✅ | ✅ | j |
| `hunyuan-image-v3.0` | Hunyuan Image V3 | x5.00 credits | — | — | — |
| `minimax-m2.5` | minimax-M2.5 | x0.18 credits | ✅ | ❌ | e |
| `minimax-m2.7` | minimax-M2.7 | x0.26 credits | ✅ | ✅ | f |
| `deepseek-v4-pro-exclusive` | Deepseek-V4-Pro | x0.51 credits | ✅ | ✅ | f |

> 另有 `product.internal.json` / `product.ioa.json` / `product.selfhosted.json` 三份变体（`endpoint` / `stagingEndpoint` / 模型子集不同）——**T23 取哪份要按渠道配置定**，本卡只记录位置。

---

## 9. 未验证项（不猜）

| # | 项 | 为什么没验 |
|---|---|---|
| 1 | **请求体 / 响应体 JSON schema**（是否 OpenAI `chat/completions` 兼容、`tools` / `stream` / `usage` 形状） | daemon **字节级透传** body，body 由 CLI / agent 运行时构造 → **要真发一次**才能定 |
| 2 | **真实 chat 端到端**（发出一次并拿到 SSE） | WorkBuddy **当前未运行**（本轮无活体抓包）；且需要先解决凭证 |
| 3 | `/v2/user/cloudagent/quota` **响应形状** | 额度查看降级档要用；需鉴权，本轮未验 |
| 4 | `X-Device-Token`（Turing）在**非签到**场景是否也会附带 | 代码里只见到签到路径调用点 |

---

## 10. 边界遵守自查

| 约束 | 本卡是否遵守 |
|---|---|
| AGENT.md §5.3 凭证零明文 | ✅ 留档只记**字段名 / 形态 / 长度**；未把任何 token / cookie / device-id 值写入本文件、代码或 git |
| AGENT.md §5.5 反代默认关闭 | ✅ 本卡**未写任何产品代码**，未建渠道、未提供启用入口 |
| AGENT.md §5.6 改写器不是转换器 | ✅ 只做**协议记录**，未设计任何字段级转换 |
| AGENT.md §5.7 不内置风控规避能力 | ✅ **未实现任何指纹伪造 / 签名伪造 / 验证码绕过**；实测也**证明不需要** |
| AGENT.md §6-3 先改文档后改码 | ✅ 顺序：03 任务卡 → 02 设计文档（§6.3 提案 + §6.3.1 + §13 v0.31）→ 取证 → 本留档 |
| §12 设计变更走审批 | ✅ §6.3 的 P1~P4 **只列为提案**，**未擅自改写既有条款**；等头头批准才动 |
| 只读 WorkBuddy | ✅ 全程只读；未启动、未抢占 WorkBuddy；未写回任何文件 |

---

## 11. 下一步

1. **头头拍板 P4**（凭证策略：用户显式提供 / 逆向定制运行时 / 降级档）→ 据此改写 02 §6.3 的 P1~P4。
2. **T23 WorkBuddy adapter**：沿用 T21 的凭证供给口径（**adapter 自己供给凭证、网关不解密**）+ `AdapterDeps` 依赖注入（生产真实端点 / 测试本地 mock）。
3. **T24**：ToS 弹窗 + 启用开关（默认关闭）。

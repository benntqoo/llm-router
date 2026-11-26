# T15 验收留档 — HAR 导入与模板化

| 项 | 值 |
|---|---|
| 任务 | T15 HAR 导入与模板化（03 §2 任务卡，M3 第一张卡） |
| 依据 | `docs/planning/02-technical-design.md` §7.1.1（HAR 1.2 子集与模板结构）、§7.1.2（变量标记与密文边界）、§7.3（站点插件默认全关）、§9（`credential.kind` 取值）；**v0.15 新增 §7.1.1 / §7.1.2** |
| 验收日期 | 2026-11-27 |
| 执行 | MiniMax Code（开发 agent） |
| 分支 | `dev` |
| 代码提交 | `2402fd9`（后端 + 前端 + 夹具 + 脚本）；留档与文档另计（见 §6 末尾「提交清单」） |
| 结论 | **通过**（任务卡判据「导入一个真实 HAR 并标记 2 个变量成功」逐条实测通过；单测 + e2e + 前端单测 + 真机四证） |

> 本文件改动范围：仅记录 T15 的验收判据、实测证据、交付物与取舍；**不含**后续任务（T16 重放 / T17 调度 / T18 站点插件 / T19 通知）的结论。

---

## 1. 判据对照

**T15 任务卡**（03 §2）：「HAR 1.2 解析、变量标记向导、模板存储。验收：**导入一个真实 HAR 并标记 2 个变量成功**。」

| # | 判据 | 实测 | 证据 |
|---|---|---|---|
| 1 | **HAR 1.2 解析** | ✅ | §3.2 e2e：真实浏览器抓包夹具解析出 12 条 entry（entry 0 = `GET /api/v1/users/login/info`，匿名态 **401**）；§3.4 真机导入同样 12 条 |
| 2 | **变量标记向导** | ✅ | §3.4 真机：导入 → 选 entry → 标记 **2 个**（`acw_tc` / `cna`）→ 保存成功；截图 `app-ui-t15-fields.png` |
| 3 | **模板存储** | ✅ | §3.4 真机：库内 `checkin_site` 落 **1 行**，`config_json` 是占位符化模板（Cookie 头整条替换为 `{{...}}`） |
| 4 | **密文边界：零明文**（AGENT.md §5.3） | ✅ | §3.2 e2e + §3.4 真机：`store.db` / `-wal` / `-shm` / `vault.bin` 四层扫描 **明文命中 0**；真实取值只存在于保险库密文里 |
| 5 | **未标记的敏感字段不留在模板里**（02 §7.1.2） | ✅ | 真机：标 2 个 → 落库 **8 个**变量（2 显式 + 6 个 `auto_*` 自动补齐），模板里没有裸值 |
| 6 | **导入的站点默认关**（02 §7.3） | ✅ | 库内 `enabled=0`；UI 状态列显示「默认关」 |
| 7 | **归属渠道**（头头 2026-11-27 裁决 · 方案 A） | ✅ | 8 条凭据全部挂在新建渠道 `modelscope-checkin`（`provider=custom`）下；既有渠道也可复用（单测） |

---

## 2. 交付物

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/checkin/har.rs` | **新增** | HAR 1.2 子集解析、敏感候选识别、模板生成（`build_template`）、落库前密文自检 + 13 条单测 |
| `src-tauri/src/checkin/mod.rs` | **新增** | 模块入口（只含 `har`；重放/调度/通知分别属 T16/T17/T19） |
| `src-tauri/src/store/checkin.rs` | **新增** | `checkin_site` 行读写（upsert / list / get / 按名查 / 删）+ 4 条单测 |
| `src-tauri/src/commands/checkin.rs` | **新增** | 4 个 IPC（导入 / 保存模板 / 列站点 / 删站点，各带可测 `_of` 版本）+ 7 条单测 |
| `src-tauri/tests/checkin_e2e.rs` | **新增** | 3 条 e2e（真实抓包脱敏夹具走全链） |
| `src-tauri/tests/fixtures/modelscope-checkin.har` | **新增** | 真实浏览器抓包（Playwright 1.58.0）**脱敏**后的夹具，可进 git |
| `src/checkin/CheckinPanel.tsx` | **新增** | 导入向导四步 UI（选文件 → 选 entry → 标记变量 → 保存） |
| `src/checkin/format.ts` / `types.ts` / `format.test.ts` | **新增** | 纯函数（变量名清洗与校验、entry 过滤、默认选中的 entry）+ 12 条前端单测 |
| `scripts/capture_har.py` / `make_har_fixture.py` | **新增** | 真实抓包脚本 + 脱敏成可提交夹具的脚本 |
| `src-tauri/Cargo.toml` / `package.json` / `capabilities/default.json` | 改 | 新增 `tauri-plugin-dialog` + `@tauri-apps/plugin-dialog`（原生文件选择框）、授权 `dialog:allow-open` |
| `src-tauri/src/lib.rs` / `commands/mod.rs` / `store/mod.rs` | 改 | 挂模块、注册 4 个 IPC、初始化 dialog 插件 |
| `src/App.tsx` / `src/App.css` | 改 | 挂载签到面板 + 样式 |
| `docs/planning/02-technical-design.md` | 改 | **新增 §7.1.1 / §7.1.2** + §9 的 `credential.kind` 取值 + §13 v0.15 |

---

## 3. 验收证据

### 3.1 后端单测（24 条）

| 位置 | 条数 | 覆盖 |
|---|---|---|
| `checkin::har::tests` | 13 | HAR 子集解析 / 敏感值掩码 / **响应头与响应体绝不进入视图** / 标 2 个变量且 Cookie 其余部分保留 / 模板零明文 / 非敏感变量保留默认值 / body 与 query 标记 / 变量名重复与非法 / 目标不存在 / 坏 JSON / Cookie 值里含 `=` / 名字判定大小写不敏感 |
| `store::checkin::tests` | 4 | 站点 id 唯一且带前缀 / upsert 覆盖 / 按名查（重录复用同一行）/ 删除语义 |
| `commands::checkin::tests` | 7 | 导入掩码且丢弃响应密文 / 标 2 个变量后模板无明文 / **凭据密文列不含明文** / 重录替换模板且旧凭据不翻倍 / 删站点连带清凭据 / 渠道不存在与未指定都报错 / 可复用既有渠道 |

### 3.2 e2e（真实抓包脱敏夹具，3 条）

夹具 `src-tauri/tests/fixtures/modelscope-checkin.har` 是 **Playwright 1.58.0 实抓**（Chromium DevTools 协议导出，2026-11-27 抓 ModelScope 首页）经 `scripts/make_har_fixture.py` 脱敏而来：保留 HAR 1.2 真实结构与接口路径，**响应体与全部响应头按 02 §7.1.1 丢弃**，敏感取值换成 `REDACTED-*`（共 130 处）。**夹具扫描**：`sk-*` / `Bearer *` / `llmr_*` / 48 位以上十六进制串 **命中 0** → 可进 git（AGENT.md §8）。

| 用例 | 断言 |
|---|---|
| `real_har_shape_is_imported_with_masked_values` | 12 条 entry；entry 0 = `GET /api/v1/users/login/info`、**401**；导入视图里三个敏感值与响应 `Set-Cookie` 值 **一个都不出现**，但**键名可见**（`csrf_session` 要让用户知道标什么） |
| `marking_two_variables_on_the_checkin_api_produces_a_placeholder_template` | 标 2 个 → 模板 **8** 个变量（2 + 6 个 cookie 子项自动补齐）；模板含 `{{acw_tc}}` / `{{cna}}`、**不含**原值；8 条凭据 `kind = cookie` 且能解密回原值 |
| `the_database_dump_contains_no_plain_variable_value` | 标 `{{cookie}}` / `{{csrf}}` 两个变量 → 模板含占位符 + 自动补齐的 `{{auto_acw_tc}}`；**整个库 dump（含凭据表）搜不到变量原值**；保险库能解出原值；`vault.bin` 原始字节里也没有 |

### 3.3 前端单测（12 条）

`node --experimental-strip-types --test src/checkin/format.test.ts` → **12 项全绿**：变量名清洗与**与后端同规则**的合法性校验、Cookie 子项与整个请求头的 target 键区分、entry 过滤、**默认选中带 `/api/` 的接口请求**、敏感字段计数、站点名默认值（去 `.har` 后缀）。

### 3.4 真机（真实应用 + 真实抓包文件 + UI 自动化）

环境：真实二进制 `src-tauri/target/debug/llm-router.exe` + 真实 Vite 前端；**库与保险库指向临时目录** `%TEMP%\llmr-t15-010914\`（`LLMR_STORE_PATH` / `LLMR_VAULT_PATH`），**不污染用户真实库**；导入的 HAR 就是仓库里的真实抓包夹具。

操作链（UI 自动化，逐步截图核对）：

1. 滚动到「签到站点（T15）」面板 → 粘贴 HAR 绝对路径 → 点「按路径导入」→ 面板提示 **「已解析 modelscope-checkin.har：共 12 条 entry」**；
2. entry 列表 12 行（方法 / URL / 状态 / 敏感字段数），默认选中 entry 0（`GET …/api/v1/users/login/info`，状态 401，敏感字段 8）；
3. 字段表逐行展开：**Cookie 的 8 个子项值全部显示 `***` 且标「（敏感）」**，非敏感头（`Accept` / `Host` / `User-Agent` …）照常显示；
4. 标记 **2 个**变量：`acw_tc`、`cna`（变量名可改名），提示同步从「还有 8 个敏感字段未标」变为「还有 6 个」；
5. 点「保存模板」→ 绿色提示 **「已保存「modelscope-checkin」：8 个变量（敏感取值已进加密保险库，模板里只留占位符）」**。

**截图**：`docs/reviews/app-ui-t15-fields.png`（字段表：掩码值 + 2 个变量已标记）、`docs/reviews/app-ui-t15-checkin.png`（站点列表 + 成功提示 + 12 条 entry 列表）。

**库侧核对**（真机保存后的实际落库）：

```
checkin_site: id=site_50b543d1… name=modelscope-checkin plugin=har enabled=0 schedule=None
              source=modelscope-checkin.har entryCount=12 assert={'status': 401}
              request=GET https://www.modelscope.cn/api/v1/users/login/info
variables: acw_tc (sensitive=1 auto=0 default=None credential=cred_4fde0b14…)
           cna   (sensitive=1 auto=0 default=None credential=cred_41a24a5d…)
           auto__ga_k9cstskfc5 / auto__ga / auto_gcl_au / auto_modelscope_locale
           auto_ssxmod_itna / auto_ssxmod_itna2   （6 个自动补齐，sensitive=1 default=None）
cookie 头模板: acw_tc={{acw_tc}}; _ga_K9CSTSKFC5={{auto__ga_k9cstskfc5}}; _ga={{auto__ga}};
              _gcl_au={{auto_gcl_au}}; modelscope-locale={{auto_modelscope_locale}};
              cna={{cna}}; ssxmod_itna={{auto_ssxmod_itna}}; ssxmod_itna2={{auto_ssxmod_itna2}}
credential: 8 行，kind 全为 cookie，密文长度 28~42 字节，全部挂渠道 ch_ce17ce4f…
channel: ch_ce17ce4f… | modelscope-checkin | custom   （按站点名新建，方案 A）
```

**零明文扫描**（真机库里 `store.db` / `store.db-wal` / `store.db-shm` / `vault.bin` 四个文件逐字节扫 + 全表逻辑 dump 再扫一次）：**明文命中 0 / 0**。

### 3.5 回归

- `cargo test --all-targets` → **367 项全绿**（**311** 单元 + 56 e2e：channel 7 + **checkin 3** + gateway 5 + outbound 6 + router 11 + stream 13 + usage 8 + vault 3），4 ignored，**0 失败**，**0 编译告警**（`cargo clean -p llm-router` 后重编核过）。
- `npm run build`（`tsc && vite build`）→ 通过，**30 modules**，gzip **58.92 kB**（较 T14 的 27 modules / 56.38 kB 多的部分来自**新增运行时依赖 `@tauri-apps/plugin-dialog`** —— 原生文件选择框所需，与 T14「不新增运行时依赖」不同）。
- `node --experimental-strip-types --test src/checkin/format.test.ts` → 12 项全绿。

---

## 4. 口径回填（02 v0.15）

本卡落地前**先改文档后改码**（AGENT.md §6-3），02 新增：

- **§7.1.1 HAR 1.2 子集与模板结构**——只取 `request` 的 method/url/headers/queryString/postData 与 `response.status`；**响应体与响应头一律丢弃**（实测签到响应的 `Set-Cookie` 带 csrf 密文）；模板结构、占位符语法 `{{name}}`、四种替换位置（`header` / `headerPart` / `query` / `body`）、**替换发生在"值内部"**、导入模板 `enabled = 0`。
- **§7.1.2 变量标记与密文边界**——候选自动识别清单（**Cookie 子项一律按密文处理**，不靠名字猜）、**模板零密文**、真实取值进保险库（`kind = cookie` / `access_token`）、**归属渠道**（头头 2026-11-27 裁决方案 A）、落库前自检、IPC 只传掩码值。
- **§9** 补 `credential.kind` 的签到取值；**§13** 记 v0.15。

---

## 5. 已知限制与未做项

1. **csrf 会轮换，模板自己不会更新**——设计如此：响应头被丢弃（02 §7.1.1），模板只固化**录制那一刻**的 Cookie。服务端下发的新 `csrf_session` / `csrf_token` 只能靠**重录 HAR**（同名站点覆盖模板 + 旧凭据自动清理）更新。这是 T16 重放与 T18 站点插件要处理的事。
2. **base64 编码的请求体不改写**——`postData.encoding = base64` 时不做 JSON 路径替换（原样保留）。
3. **断言只有状态码**——JSON 路径断言属 T16（模板里存了录制时的 `status` 作为初值）。
4. **重录 HAR 会整体替换模板**——不是增量合并：同名站点重录后，旧变量的凭据会被删掉（模板更新成功后才删旧凭据，避免中途失败丢凭据）。
5. **自动变量名有 cosmetic 瑕疵**——cookie 名以 `_` 开头时生成 `auto__ga_k9cstskfc5`（双下划线）。合法（`[A-Za-z0-9_-]`）且唯一，只是不好看。
6. **未测真实站点**——本卡只做"导入 + 模板化"，**没有对 ModelScope 发过真实签到请求**（无登录态；且真实上游测试属 T18）。
7. **未做项**：重放（T16）、调度（T17）、通知（T19）、内置站点插件（T18）、价目表编辑 UI（T13 遗留，另排）。

---

## 6. 经验

- **"敏感"不能靠名字猜**——实测 ModelScope 的 Cookie 里有 `cna`、`ssxmod_itna` 这种名字不含任何关键词的子项，但它们同样是会话态数据。规则定成「**Cookie 子项一律按密文处理**」，其它字段才按名字关键词（csrf / token / session / jwt …）判。
- **"用户没标的敏感字段"必须自动补齐，不能留在模板里也不能丢掉**——留在模板里等于把会话凭证明文写进 SQLite；丢掉则重放必失效。自动补成 `auto_*` 变量（用户可在列表里改名）才是唯一出路。真机上这条规则直接把 2 个标记扩成了 8 个变量。
- **"密文边界"要落在**两处**：① IPC 边界掩码（导入视图里敏感值一律 `***`，真实值不出 Rust 进程）；② 落库边界自检（模板序列化后逐个搜敏感原值，命中就拒绝保存）。两处缺一不可——只做①会让"模板构造逻辑写错"直接漏密文。
- **导入态不驻留内存**：保存时前端只回传**路径 + 标记清单**，Rust 重新读盘解析。代价是保存要重读一次 HAR，收益是**导入的敏感值从头到尾没在内存里多留一份**，也天然免疫"前端偷偷缓存原值"。
- **PS 5.1 的 `SendKeys` 打路径会被输入法吃掉**——直接 `SendKeys` 输 `D:\Code\...` 得到的是 `D:\C\\山\\义\\`。正解是 `Set-Clipboard` + `^v` 粘贴。
- **`cargo build` 失败于"应用还在运行"**——`failed to remove file … llm-router.exe`（exe 被占）。上一会话残留的 `llm-router.exe` 进程既挡住了本次编译，也是那次会话中断的现场；复跑测试前先 `taskkill /T /F` 清进程树。
- **真机验收要看**"保存之后库里到底长什么样"**，不能只看 UI 提示**——UI 说"已保存"可能是假绿；把真机库 dump 出来逐项核对（占位符、变量表、凭据 kind、明文命中数）才作数。

---

## 7. 提交清单

| 提交 | 内容 |
|---|---|
| `2402fd9` | `feat: T15 HAR 导入与模板化（自写 HAR 解析 + 变量标记 + 模板存储）`——后端 / 前端 / 夹具 / 脚本 |
| `9f0684b` | 验收留档 + 02 v0.15 + 03 落地行 + HANDOFF 账本 + 真机截图 |

> 文档提交号已回填（本文件所在提交）。

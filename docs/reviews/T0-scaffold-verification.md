# T0 验收留档 — 仓库初始化

| 项 | 值 |
|---|---|
| 任务 | T0 仓库初始化（`docs/planning/03-delivery-plan.md` §2 M1） |
| 验收日期 | 2026-11-25 |
| 执行 | MiniMax Code（开发 agent） |
| 提交 | `c98bd69`（分支 `dev`，代码与配置） |
| 留档 | 本文件 + `docs/reviews/assets/T0-dev-window.png` |
| 结论 | **通过**（判据逐条实测见下） |

## 1. 判据对照

任务卡原文：*目标：工程可跑。git init、Tauri 2 + React + TS 脚手架、axum 内嵌服务 hello world、`cargo test` / `npm run tauri dev` 双通。产出：`src-tauri/`、`src/` 骨架；CI 留空。验收：`npm run tauri dev` 打开窗口，`curl 127.0.0.1:<port>/healthz` 返回 200。*

| # | 判据 | 实测结果 | 证据 |
|---|---|---|---|
| 1 | Tauri 2 + React + TS 脚手架 | 通过 | `src-tauri/`（Tauri 2.12.7）+ `src/`（React 18.3 + TS 6.0 + Vite 8.3）；`npm run build` 退出码 0 |
| 2 | axum 内嵌服务 hello world | 通过 | `src-tauri/src/gateway/server.rs`：`GET /healthz` → 200 |
| 3 | `cargo test` 通 | 通过 | 5 passed / 0 failed（见 §2.1） |
| 4 | `npm run tauri dev` 打开窗口 | 通过 | 进程 `llm-router` 主窗口标题 `LLM Router`；截图 `assets/T0-dev-window.png` |
| 5 | `curl 127.0.0.1:<port>/healthz` 返回 200 | 通过 | HTTP 200 + `{"status":"ok","service":"llm-router","version":"0.1.0"}`（见 §2.3） |
| 6 | 产出 `src-tauri/`、`src/` 骨架 | 通过 | 目录结构按 02 §1.2：`commands/`、`gateway/`、`store/` 等预留位（`gateway/`、`commands/` 已建） |
| 7 | CI 留空 | 通过 | 未创建 `.github/`，按任务卡留空 |
| 8 | `.gitignore` 覆盖敏感路径（HANDOFF §5 入口段） | 通过 | `vault.bin` / `*.db` / `*.sqlite*` / `.env*` / `logs/` 均已写入（AGENT.md §8 硬要求） |

## 2. 实测记录

### 2.1 Rust 单测

```
$ cd src-tauri && cargo test
running 5 tests
test gateway::server::tests::ipv6_loopback_is_allowed ... ok
test gateway::server::tests::default_addr_is_loopback ... ok
test gateway::server::tests::invalid_addr_is_rejected ... ok
test gateway::server::tests::non_loopback_is_rejected ... ok
test gateway::server::tests::healthz_returns_ok_with_json_body ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 2.2 前端构建

```
$ npm run build
> tsc && vite build
vite v8.3.8 building client environment for production...
✓ 17 modules transformed.
dist/index.html                   0.39 kB │ gzip:  0.27 kB
dist/assets/index-BPSZ5B5C.css    1.21 kB │ gzip:  0.60 kB
dist/assets/index-BDt6cz8H.js   142.56 kB │ gzip: 46.67 kB
✓ built in 84ms
```

### 2.3 dev 运行态与健康检查

```
$ npm run tauri dev        # 窗口打开，标题 "LLM Router"

$ Invoke-WebRequest http://127.0.0.1:8787/healthz
HTTP 200
{"status":"ok","service":"llm-router","version":"0.1.0"}

监听端口：
LocalAddress LocalPort
127.0.0.1     8787      # 网关（axum）
::1           1420      # vite dev server
```

### 2.4 红线校验：非回环地址必须拒绝启动（AGENT.md §5-4）

```
$ LLMR_GATEWAY_ADDR=0.0.0.0:8788 llm-router.exe
exited with code 1
thread 'main' panicked at tauri-2.12.7/src/app.rs:1444:11:
Failed to setup app: error encountered during setup hook:
拒绝绑定非回环地址 0.0.0.0:8788：client key 鉴权落地前只允许 127.0.0.1（AGENT.md §5-4）
```

> 实现说明：当前策略比红线更严——client key 鉴权（T3）落地前**任何**非回环地址都拒绝启动，而不是"有 client key 才允许"。T3 完成鉴权后按红线放开。

## 3. 本次交付物

| 路径 | 说明 |
|---|---|
| `src-tauri/src/gateway/server.rs` | axum 路由 + 监听地址解析与红线校验（含 5 个单测） |
| `src-tauri/src/lib.rs` | Tauri 外壳：setup 中绑定网关 + `AppState` + IPC 注册 |
| `src-tauri/src/commands/gateway.rs` | `gateway_status` 命令（前端读网关地址/base_url） |
| `src/App.tsx` / `src/App.css` | T0 状态页：网关运行状态 + 客户端 base_url |
| `src-tauri/Cargo.toml` | 锁定 axum 0.8 / tokio 1 / tauri 2（02 §1.3） |
| `.gitignore` | 敏感路径 + 构建产物 |

## 4. 偏离与说明

| 项 | 说明 |
|---|---|
| React 版本 | 官方脚手架默认 React 19；AGENT.md §4 锁定 React 18，故 `package.json` 固定 `react@^18.3.1` / `@types/react@^18.3.12` |
| 移除 opener 插件 | 模板自带 `tauri-plugin-opener`（用于打开外链），02 §1.2 无此依赖，T0 移除以减小依赖面 |
| `tauri dev` 产物目录 | `dist/` 在 T0 仅作 `frontendDist` 占位（gitignored），由 `npm run build` 覆盖 |
| 验收命令的 Windows 等价写法 | 本机无 `sqlite3` CLI 与 Unix `strings`/`grep`。AGENT.md §9 安全验收四命令需改用等价格式（Python `sqlite3` 模块导出 dump + PowerShell 二进制正则）。**该适配属 T11 范围，T0 未实施** |

## 5. 与本次并行发生的外部变更（不属 T0 范围，未纳入本提交）

2026-11-25 16:38–16:44 期间，另一会话在同一仓库更新了规划文档（`HANDOFF.md`、`docs/planning/01–03`），02 升至 v0.2：新增 P-7 `X-Routed-Via` 响应头、route_group `sticky_session` 策略、限额自学习，并产出 `docs/reviews/freellmapi-analysis.md`。这些变更落在 T4/T5/T6/T7 范围，**不影响 T0 实现**；提交 `c98bd69` 只含 T0 自身文件，上述文档变更仍留在工作区未提交，由其作者提交。

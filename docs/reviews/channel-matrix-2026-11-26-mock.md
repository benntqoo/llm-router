# 渠道实测矩阵 — 2026-11-26

> ⚠️ 本次是**矩阵框架自测**：上游是本地 mock（127.0.0.1:18790），**不是真实渠道**。真实 A 类 7 列矩阵待 API Key 到位后重跑 scripts/channel_matrix.py 覆盖本文件。

| 项 | 值 |
|---|---|
| 矩阵 | 02 §10.1 前 7 列（A 类 API Key 型渠道） |
| 覆盖 | 1 / 7 个 A 类渠道 |
| 网关 | `http://127.0.0.1:8787`（**经网关实测**：测的是「预设 + 网关改写」后的能力） |
| 执行 | `scripts/channel_matrix.py` |
| 生成时间 | 2026-11-26 |

> 判据：全绿才能在预设中标 `stable`；部分绿标 `partial`（UI 明示哪些特性不可用）。
> `✔` 通过 / `✘` 不通过 / `—` 该模型不适用 / `·` 未测（未配置渠道）。

## 1. 主矩阵

| 项 | DeepSeek 官方 | Kimi 官方 | GLM 官方 | MiniMax 国际站 | MiniMax 国内站 | 阿里百炼 | 硅基流动 |
|---|---|---|---|---|---|---|---|
| 非流式 chat | ✔ | · | · | · | · | · | · |
| 流式 chat | ✔ | · | · | · | · | · | · |
| tool_calls | ✔ | · | · | · | · | · | · |
| usage 返回 | ✔ | · | · | · | · | · | · |
| reasoning 字段 | ✔ | · | · | · | · | · | · |
| 多轮上下文 | ✔ | · | · | · | · | · | · |

## 2. 逐项证据

| 渠道 | 别名 | 项 | 结果 | HTTP | 耗时 | 证据 |
|---|---|---|---|---|---|---|
| DeepSeek 官方 | deepseek-chat | 非流式 chat | ✔ | 200 | 20 ms | {"id": "chatcmpl-matrix-mock", "object": "chat.completion", "model": "deepseek-chat", "choices": [{"index": 0, "message": {"role": "assistant", "content": "1+1 等于 2。", "reasoning_content": "（mock 推理过程 |
| DeepSeek 官方 | deepseek-chat | 流式 chat | ✔ | 200 | 2 ms | 3 个 SSE 帧，增量文本 9 字 |
| DeepSeek 官方 | deepseek-chat | tool_calls | ✔ | 200 | 17 ms | 返回 1 个 tool_call：get_weather，arguments {"city": "llm-router"} |
| DeepSeek 官方 | deepseek-chat | usage 返回 | ✔ | 200 | 15 ms | usage={prompt:11, completion:5} |
| DeepSeek 官方 | deepseek-chat | reasoning 字段 | ✔ | 200 | 1 ms | 含 reasoning_content（17 字）：（mock 推理过程）先算，再答。 |
| DeepSeek 官方 | deepseek-chat | 多轮上下文 | ✔ | 200 | 28 ms | 第二轮答出代号（紫罗兰七三） |
| Kimi 官方 | — | 全部 | · | — | — | 未配置渠道（未测） |
| GLM 官方 | — | 全部 | · | — | — | 未配置渠道（未测） |
| MiniMax 国际站 | — | 全部 | · | — | — | 未配置渠道（未测） |
| MiniMax 国内站 | — | 全部 | · | — | — | 未配置渠道（未测） |
| 阿里百炼 | — | 全部 | · | — | — | 未配置渠道（未测） |
| 硅基流动 | — | 全部 | · | — | — | 未配置渠道（未测） |

## 3. 网关侧用量落库（T7 采集链路）

| 渠道 | 网关 usage_log 最近一行 |
|---|---|
| DeepSeek 官方 | alias=deepseek-chat upstream=deepseek-chat status=success estimated=0 prompt=11 completion=5 |
| Kimi 官方 | （未配置渠道，未测） |
| GLM 官方 | （未配置渠道，未测） |
| MiniMax 国际站 | （未配置渠道，未测） |
| MiniMax 国内站 | （未配置渠道，未测） |
| 阿里百炼 | （未配置渠道，未测） |
| 硅基流动 | （未配置渠道，未测） |

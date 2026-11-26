#!/usr/bin/env python3
"""渠道实测矩阵执行器（T10；02 §10.1）。

对**已在应用里配置好的渠道**逐项跑 6 个能力项，产出 02 §10.1 同构的矩阵并落库到
`docs/reviews/channel-matrix-<日期>.md`。

为什么经网关打：矩阵的判据是"这个预设能不能标 stable / partial"——
那是**预设 + 网关改写（R-1~R-8）之后的联合属性**，不是渠道裸接口的属性。
所以必须走 `POST http://127.0.0.1:<port>/v1/chat/completions`，而不是直连上游。

前置（都由 T9 的 UI 提供）：
  1. 应用已启动，且在「渠道」面板配好了渠道并录了密钥（密钥在保险库里，脚本读不到）；
  2. 一把 client key（在「Client Key」面板创建后复制）。

用法（PowerShell）：
  $env:LLMR_CLIENT_KEY = "llmr_xxx"
  python scripts/channel_matrix.py --store "$env:APPDATA/llm-router/store.db"
  python scripts/channel_matrix.py --store ... --channels deepseek,glm --json

安全：本脚本**只读**应用库；client key 只从命令行/环境变量取，**不打印、不落盘**。
"""

from __future__ import annotations

import argparse
import io
import json
import os
import sqlite3
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import date
from typing import Any

# ── 矩阵的 6 个能力项（与 02 §10.1 一一对应） ─────────────────────────────
ITEMS = [
    ("chat_nonstream", "非流式 chat"),
    ("chat_stream", "流式 chat"),
    ("tool_calls", "tool_calls"),
    ("usage_return", "usage 返回"),
    ("reasoning", "reasoning 字段"),
    ("multi_turn", "多轮上下文"),
]

MARK_OK = "✔"
MARK_FAIL = "✘"
MARK_NA = "—"
MARK_SKIP = "·"


def out(msg: str) -> None:
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


# ── 应用库（只读） ────────────────────────────────────────────────────────

@dataclass
class Channel:
    id: str
    name: str
    provider: str
    base_url: str
    models: list[tuple[str, str]] = field(default_factory=list)  # (alias, upstream_id)


def load_channels(store_path: str) -> list[Channel]:
    """从应用库读渠道清单（只读）。密钥读不到也不需要——网关自己解密。"""
    uri = "file:" + store_path.replace("\\", "/") + "?mode=ro"
    conn = sqlite3.connect(uri, uri=True)
    try:
        rows = conn.execute(
            "SELECT id, name, provider, base_url FROM channel WHERE enabled = 1 ORDER BY created_at, id"
        ).fetchall()
        models = conn.execute(
            "SELECT channel_id, alias, upstream_id FROM model WHERE enabled = 1 ORDER BY alias"
        ).fetchall()
    finally:
        conn.close()
    by_channel: dict[str, list[tuple[str, str]]] = {}
    for channel_id, alias, upstream_id in models:
        by_channel.setdefault(channel_id, []).append((alias, upstream_id))
    out_channels = []
    for cid, name, provider, base_url in rows:
        ch = Channel(cid, name, provider, base_url, by_channel.get(cid, []))
        if ch.models:
            out_channels.append(ch)
    return out_channels


def load_presets(manifest: str) -> list[dict[str, Any]] | None:
    """从 Rust 侧导出预设目录（**唯一权威载体**：providers::presets）。

    用 `cargo run --example presets` 现场导出，避免在 Python 里抄一份渠道名单
    （抄了必然漂移）。导出失败不致命——退回"以库里已配置的渠道为准"。
    """
    import subprocess

    cmd = ["cargo", "run", "--quiet", "--example", "presets", "--manifest-path", manifest]
    try:
        proc = subprocess.run(cmd, capture_output=True, timeout=300, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    try:
        data = json.loads(proc.stdout.decode("utf-8", errors="replace"))
    except json.JSONDecodeError:
        return None
    return data if isinstance(data, list) else None


def latest_usage_row(store_path: str, alias: str) -> dict[str, Any] | None:
    """网关侧用量流水最近一行（验证 T7 采集链路是否把这次调用记下来了）。"""
    try:
        uri = "file:" + store_path.replace("\\", "/") + "?mode=ro"
        conn = sqlite3.connect(uri, uri=True)
        try:
            row = conn.execute(
                "SELECT model_alias, upstream_model, status, estimated, prompt_tokens,"
                " completion_tokens, error_kind FROM usage_log WHERE model_alias = ?"
                " ORDER BY id DESC LIMIT 1",
                (alias,),
            ).fetchone()
        finally:
            conn.close()
    except sqlite3.Error:
        return None
    if not row:
        return None
    keys = ["model_alias", "upstream_model", "status", "estimated", "prompt_tokens",
            "completion_tokens", "error_kind"]
    return dict(zip(keys, row))


# ── HTTP（走网关） ────────────────────────────────────────────────────────

def post_json(
    url: str,
    headers: dict[str, str],
    payload: dict[str, Any],
    timeout: float,
) -> tuple[int, dict[str, Any], str]:
    """返回 (状态码, 解析后的 JSON 或 {}, 原始响应体前 400 字)。"""
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=body, method="POST")
    req.add_header("Content-Type", "application/json")
    for k, v in headers.items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            status = resp.status
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", errors="replace")
        status = e.code
    except Exception as e:  # 连接失败 / 超时
        return 0, {}, f"{type(e).__name__}: {e}"
    try:
        return status, json.loads(raw), raw
    except json.JSONDecodeError:
        return status, {}, raw


def post_stream(
    url: str,
    headers: dict[str, str],
    payload: dict[str, Any],
    timeout: float,
) -> tuple[int, list[str], str]:
    """流式：返回 (状态码, SSE data 帧文本列表, 错误信息)。"""
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=body, method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "text/event-stream")
    for k, v in headers.items():
        req.add_header(k, v)
    frames: list[str] = []
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            status = resp.status
            for raw_line in resp:
                line = raw_line.decode("utf-8", errors="replace").strip()
                if line.startswith("data:"):
                    chunk = line[5:].strip()
                    if chunk and chunk != "[DONE]":
                        frames.append(chunk)
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace")[:400]
        return e.code, [], detail
    except Exception as e:
        return 0, [], f"{type(e).__name__}: {e}"
    return status, frames, ""


# ── 6 个能力项 ────────────────────────────────────────────────────────────

def first_text(message: dict[str, Any]) -> str:
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):  # 部分模型返回内容块数组
        parts = [c.get("text", "") for c in content if isinstance(c, dict)]
        return "".join(parts)
    return ""


def check_chat_nonstream(url, headers, alias, timeout) -> dict[str, Any]:
    status, data, raw = post_json(
        url, headers,
        {"model": alias, "messages": [{"role": "user", "content": "用一句话回答：1+1 等于几？"}],
         "stream": False},
        timeout,
    )
    ok = status == 200 and bool(first_text((data.get("choices") or [{}])[0].get("message", {})))
    return {"pass": ok, "status": status, "detail": _brief(data, raw)}


def check_chat_stream(url, headers, alias, timeout) -> dict[str, Any]:
    status, frames, err = post_stream(
        url, headers,
        {"model": alias, "messages": [{"role": "user", "content": "用一句话回答：1+1 等于几？"}],
         "stream": True},
        timeout,
    )
    text = ""
    for frame in frames:
        try:
            chunk = json.loads(frame)
        except json.JSONDecodeError:
            continue
        for choice in chunk.get("choices") or []:
            delta = choice.get("delta") or {}
            piece = delta.get("content")
            if isinstance(piece, str):
                text += piece
    return {"pass": status == 200 and bool(frames) and bool(text.strip()),
            "status": status,
            "detail": f"{len(frames)} 个 SSE 帧，增量文本 {len(text.strip())} 字" + (f"；错误 {err[:120]}" if err else "")}


def check_tool_calls(url, headers, alias, timeout) -> dict[str, Any]:
    tools = [{
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "查询某个城市的天气",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string", "description": "城市名"}},
                "required": ["city"],
            },
        },
    }]
    status, data, raw = post_json(
        url, headers,
        {"model": alias,
         "messages": [{"role": "user", "content": "请调用 get_weather 工具查询北京的天气。"}],
         "tools": tools, "tool_choice": "auto", "stream": False},
        timeout,
    )
    msg = (data.get("choices") or [{}])[0].get("message", {}) or {}
    calls = msg.get("tool_calls") or []
    if calls:
        fn = calls[0].get("function", {}) or {}
        detail = f"返回 {len(calls)} 个 tool_call：{fn.get('name')}"
        if fn.get("arguments"):
            detail += f"，arguments {str(fn.get('arguments'))[:80]}"
        return {"pass": True, "status": status, "detail": detail}
    # 明确拒绝工具也是一种确定结论（不是网关的错）
    err = (data.get("error") or {}).get("message") if isinstance(data.get("error"), dict) else None
    if status in (400, 404, 422) or (err and "tool" in str(err).lower()):
        return {"pass": False, "na": True, "status": status,
                "detail": f"上游明确不支持 tools（HTTP {status}）：{str(err or raw)[:120]}"}
    return {"pass": False, "status": status, "detail": _brief(data, raw) + "（无 tool_calls）"}


def check_usage_return(url, headers, alias, timeout) -> dict[str, Any]:
    status, data, raw = post_json(
        url, headers,
        {"model": alias, "messages": [{"role": "user", "content": "回复“ok”两个字即可"}],
         "stream": False},
        timeout,
    )
    usage = data.get("usage") or {}
    prompt = usage.get("prompt_tokens")
    completion = usage.get("completion_tokens")
    has = isinstance(prompt, int) and isinstance(completion, int)
    return {"pass": bool(has and (prompt or completion)),
            "status": status,
            "detail": (f"usage={{prompt:{prompt}, completion:{completion}}}" if has
                       else f"响应里没有 usage 字段（{_brief(data, raw)}）")}


def check_reasoning(url, headers, alias, timeout) -> dict[str, Any]:
    status, data, raw = post_json(
        url, headers,
        {"model": alias, "messages": [{"role": "user", "content": "算一下 17*23，用一句话给出结果。"}],
         "stream": False},
        timeout,
    )
    msg = (data.get("choices") or [{}])[0].get("message", {}) or {}
    for field_name in ("reasoning_content", "reasoning"):
        value = msg.get(field_name)
        if isinstance(value, str) and value.strip():
            return {"pass": True, "status": status,
                    "detail": f"含 {field_name}（{len(value)} 字）：{value.strip()[:60]}"}
    return {"pass": False, "na": True, "status": status,
            "detail": "该模型不返回 reasoning 字段（非缺陷，矩阵按“—”记）"}


def check_multi_turn(url, headers, alias, timeout) -> dict[str, Any]:
    code = "紫罗兰七三"
    _, first, _ = post_json(
        url, headers,
        {"model": alias, "messages": [{"role": "user", "content": f"请记住这个代号：{code}。只回复“好”。"}],
         "stream": False},
        timeout,
    )
    status, data, raw = post_json(
        url, headers,
        {"model": alias,
         "messages": [{"role": "user", "content": f"请记住这个代号：{code}。只回复“好”。"},
                      {"role": "assistant", "content": "好"},
                      {"role": "user", "content": "我刚才让你记的代号是什么？只回答代号本身。"}],
         "stream": False},
        timeout,
    )
    text = first_text((data.get("choices") or [{}])[0].get("message", {}))
    hit = code in text or ("紫罗兰" in text and "七三" in text)
    return {"pass": bool(hit and status == 200), "status": status,
            "detail": (f"第二轮答出代号（{text.strip()[:40]}）" if hit
                       else f"第二轮没答出代号：{text.strip()[:60] or _brief(data, raw)}")}


def _brief(data: dict[str, Any], raw: str) -> str:
    if data:
        err = data.get("error")
        if isinstance(err, dict):
            return f"error {err.get('type')}: {str(err.get('message'))[:120]}"
        return json.dumps(data, ensure_ascii=False)[:200]
    return raw[:160].replace("\n", " ")


CHECKS = {
    "chat_nonstream": check_chat_nonstream,
    "chat_stream": check_chat_stream,
    "tool_calls": check_tool_calls,
    "usage_return": check_usage_return,
    "reasoning": check_reasoning,
    "multi_turn": check_multi_turn,
}


# ── 主流程 ────────────────────────────────────────────────────────────────

def run_channel(ch: Channel, gateway: str, client_key: str, timeout: float,
                store_path: str) -> dict[str, Any]:
    alias = ch.models[0][0]
    headers = {"Authorization": f"Bearer {client_key}"}
    url = gateway.rstrip("/") + "/v1/chat/completions"
    results: dict[str, Any] = {}
    for item_key, item_name in ITEMS:
        started = time.time()
        try:
            res = CHECKS[item_key](url, headers, alias, timeout)
        except Exception as e:  # 单项异常不中断整列
            res = {"pass": False, "status": 0, "detail": f"{type(e).__name__}: {e}"}
        res["ms"] = int((time.time() - started) * 1000)
        results[item_key] = res
        out(f"    {item_name:<12} {MARK_OK if res['pass'] else (MARK_NA if res.get('na') else MARK_FAIL)}"
            f"  {res.get('detail', '')}")
    results["_usage_row"] = latest_usage_row(store_path, alias)
    results["_alias"] = alias
    return results


def render_markdown(columns: list[dict[str, Any]], gateway: str, generated_at: str,
                    preset_count: int) -> str:
    names = [c["name"] for c in columns]
    header = [
        "| 项 | " + " | ".join(names) + " |",
        "|---|" + "---|" * len(columns),
    ]
    rows = []
    for item_key, item_name in ITEMS:
        cells = []
        for col in columns:
            res = col["results"].get(item_key) if col["results"] else None
            if res is None:
                cells.append(MARK_SKIP)
            elif res["pass"]:
                cells.append(MARK_OK)
            elif res.get("na"):
                cells.append(MARK_NA)
            else:
                cells.append(MARK_FAIL)
        rows.append(f"| {item_name} | " + " | ".join(cells) + " |")

    detail = [
        "| 渠道 | 别名 | 项 | 结果 | HTTP | 耗时 | 证据 |",
        "|---|---|---|---|---|---|---|",
    ]
    for col in columns:
        if not col["results"]:
            detail.append(f"| {col['name']} | — | 全部 | {MARK_SKIP} | — | — | 未配置渠道（未测） |")
            continue
        for item_key, item_name in ITEMS:
            res = col["results"][item_key]
            mark = MARK_OK if res["pass"] else (MARK_NA if res.get("na") else MARK_FAIL)
            detail.append(
                f"| {col['name']} | {col['results']['_alias']} | {item_name} | {mark} | "
                f"{res.get('status', '-')} | {res.get('ms', '-')} ms | {res.get('detail', '')} |"
            )

    usage_rows = [
        "| 渠道 | 网关 usage_log 最近一行 |",
        "|---|---|",
    ]
    for col in columns:
        if not col["results"]:
            usage_rows.append(f"| {col['name']} | （未配置渠道，未测） |")
            continue
        row = col["results"].get("_usage_row")
        if row:
            usage_rows.append(
                f"| {col['name']} | alias={row['model_alias']} upstream={row['upstream_model']} "
                f"status={row['status']} estimated={row['estimated']} "
                f"prompt={row['prompt_tokens']} completion={row['completion_tokens']} |"
            )
        else:
            usage_rows.append(f"| {col['name']} | （无流水记录） |")

    body = [
        f"# 渠道实测矩阵 — {generated_at}",
        "",
        "| 项 | 值 |",
        "|---|---|",
        "| 矩阵 | 02 §10.1 前 7 列（A 类 API Key 型渠道） |",
        f"| 覆盖 | {len([c for c in columns if c['results']])} / {preset_count} 个 A 类渠道 |",
        f"| 网关 | `{gateway}`（**经网关实测**：测的是「预设 + 网关改写」后的能力） |",
        "| 执行 | `scripts/channel_matrix.py` |",
        f"| 生成时间 | {generated_at} |",
        "",
        "> 判据：全绿才能在预设中标 `stable`；部分绿标 `partial`（UI 明示哪些特性不可用）。",
        "> `✔` 通过 / `✘` 不通过 / `—` 该模型不适用 / `·` 未测（未配置渠道）。",
        "",
        "## 1. 主矩阵",
        "",
        *header,
        *rows,
        "",
        "## 2. 逐项证据",
        "",
        *detail,
        "",
        "## 3. 网关侧用量落库（T7 采集链路）",
        "",
        *usage_rows,
        "",
    ]
    return "\n".join(body)


def main() -> int:
    parser = argparse.ArgumentParser(description="渠道实测矩阵执行器（T10）")
    parser.add_argument("--store", required=True, help="应用 store.db 路径（只读打开）")
    parser.add_argument("--gateway", default=os.environ.get("LLMR_GATEWAY", "http://127.0.0.1:8787"))
    parser.add_argument("--client-key", default=os.environ.get("LLMR_CLIENT_KEY", ""))
    parser.add_argument("--out", default="", help="输出 markdown 路径（默认 docs/reviews/channel-matrix-<日期>.md）")
    parser.add_argument("--channels", default="", help="只跑这些 provider（逗号分隔）")
    parser.add_argument("--timeout", type=float, default=90.0)
    parser.add_argument("--manifest", default=os.path.join("src-tauri", "Cargo.toml"),
                        help="Cargo.toml 路径（用于导出预设清单）")
    parser.add_argument("--json", action="store_true", help="额外输出 JSON")
    args = parser.parse_args()

    if not args.client_key:
        out("需要 client key：在「Client Key」面板创建后复制，"
            "用 --client-key 传入或设环境变量 LLMR_CLIENT_KEY。")
        return 2

    channels = load_channels(args.store)
    if args.channels:
        wanted = {c.strip() for c in args.channels.split(",") if c.strip()}
        channels = [c for c in channels if c.provider in wanted or c.name in wanted]
    if not channels:
        out("库里没有可用渠道（enabled=1 且至少有一个启用的模型）。"
            "先在「渠道」面板配置并录密钥。")
        return 3

    # 列 = A 类 7 个预设（未配置的留空标记「未测」）+ 库里额外配置的渠道
    presets = load_presets(args.manifest) or []
    by_provider = {c.provider: c for c in channels}
    columns: list[dict[str, Any]] = []
    for preset in presets:
        ch = by_provider.get(preset.get("provider"))
        columns.append({
            "name": preset.get("name") or preset.get("provider"),
            "provider": preset.get("provider"),
            "channel": ch,
            "results": None,
        })
    for ch in channels:
        if ch.provider not in by_provider or all(c["provider"] != ch.provider for c in columns):
            columns.append({"name": f"{ch.name}（自定义）", "provider": ch.provider,
                            "channel": ch, "results": None})

    out(f"网关 {args.gateway}；A 类预设 {len(presets)} 个；库里已配置 {len(channels)} 个")
    for col in columns:
        ch = col["channel"]
        if not ch:
            out(f"  · {col['name']}：未配置渠道，跳过")
            continue
        out(f"  · {ch.name}（{ch.provider}，{ch.base_url}）")
        col["results"] = run_channel(ch, args.gateway, args.client_key, args.timeout, args.store)

    generated_at = date.today().isoformat()
    out_path = args.out or os.path.join("docs", "reviews", f"channel-matrix-{generated_at}.md")
    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    with io.open(out_path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(render_markdown(columns, args.gateway, generated_at, len(presets) or len(channels)))
    out(f"矩阵已写入 {out_path}")

    if args.json:
        dump = {col["name"]: {k: v for k, v in (col["results"] or {}).items()
                              if not k.startswith("_")} for col in columns}
        print(json.dumps(dump, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

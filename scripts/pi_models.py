#!/usr/bin/env python3
"""生成 pi CLI 的接入配置片段（T10；02 §6.5）。

pi 的自定义 provider 写在 `~/.pi/agent/models.json`：

    {
      "$schema": "https://pi.dev/schemas/models.schema.json",
      "providers": {
        "llm-router": {
          "baseUrl": "http://127.0.0.1:8787/v1",
          "api": "openai-completions",
          "apiKey": "$LLMR_CLIENT_KEY",
          "models": [ { "id": "<别名>", "contextWindow": 128000, "maxTokens": 8192 } ]
        }
      }
    }

模型清单**从应用库直接读**（与 `GET /v1/models` 同一份数据），
所以片段里出现的别名就是客户端真能调到的别名。

用法：
    python scripts/pi_models.py --store "%APPDATA%/llm-router/store.db"
    python scripts/pi_models.py --store ... --out ~/.pi/agent/models.json --write
"""

from __future__ import annotations

import argparse
import io
import json
import os
import sqlite3
import sys

PROVIDER_ID = "llm-router"
DEFAULT_CONTEXT = 128000
DEFAULT_MAX_TOKENS = 8192


def published_models(store_path: str) -> list[dict[str, str]]:
    """对外发布的别名（与 `GET /v1/models` 同源：模型与渠道都启用才算）。"""
    uri = "file:" + store_path.replace("\\", "/") + "?mode=ro"
    conn = sqlite3.connect(uri, uri=True)
    try:
        rows = conn.execute(
            "SELECT m.alias AS alias, c.name AS owned_by FROM model m"
            " JOIN channel c ON c.id = m.channel_id"
            " WHERE m.enabled = 1 AND c.enabled = 1 ORDER BY m.alias"
        ).fetchall()
    finally:
        conn.close()
    return [{"alias": r[0], "owned_by": r[1]} for r in rows]


def build_models_json(models: list[dict[str, str]], base_url: str,
                      api_key_env: str, context: int, max_tokens: int) -> dict:
    return {
        "$schema": "https://pi.dev/schemas/models.schema.json",
        "providers": {
            PROVIDER_ID: {
                "name": "LLM Router (本地网关)",
                "baseUrl": base_url.rstrip("/") + "/v1",
                "api": "openai-completions",
                # 写成 $ENV_NAME → pi 运行时读环境变量，**文件里不留明文密钥**
                "apiKey": f"${api_key_env}",
                "models": [
                    {
                        "id": m["alias"],
                        "name": m["alias"],
                        "contextWindow": context,
                        "maxTokens": max_tokens,
                    }
                    for m in models
                ],
            }
        },
    }


def build_settings_json(models: list[dict[str, str]]) -> dict:
    default_model = models[0]["alias"] if models else ""
    return {
        "defaultProvider": PROVIDER_ID,
        "defaultModel": default_model,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="生成 pi 接入配置片段")
    parser.add_argument("--store", required=True, help="应用 store.db 路径（只读）")
    parser.add_argument("--base-url", default=os.environ.get("LLMR_GATEWAY", "http://127.0.0.1:8787"))
    parser.add_argument("--api-key-env", default="LLMR_CLIENT_KEY",
                        help="apiKey 引用的环境变量名（避免明文落文件）")
    parser.add_argument("--context", type=int, default=DEFAULT_CONTEXT)
    parser.add_argument("--max-tokens", type=int, default=DEFAULT_MAX_TOKENS)
    parser.add_argument("--out", default="", help="输出文件路径（默认打印到 stdout）")
    parser.add_argument("--settings-out", default="", help="顺带写 settings.json")
    parser.add_argument("--write", action="store_true", help="确认写入 --out 指定的文件")
    args = parser.parse_args()

    models = published_models(args.store)
    if not models:
        print("库里没有对外发布的别名——先在「渠道」面板配好渠道与模型。", file=sys.stderr)
        return 3

    models_json = build_models_json(models, args.base_url, args.api_key_env,
                                    args.context, args.max_tokens)
    settings_json = build_settings_json(models)
    payload = json.dumps(models_json, ensure_ascii=False, indent=2)

    if args.out and args.write:
        os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
        with io.open(args.out, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(payload + "\n")
        print(f"已写入 {args.out}")
        if args.settings_out:
            with io.open(args.settings_out, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(json.dumps(settings_json, ensure_ascii=False, indent=2) + "\n")
            print(f"已写入 {args.settings_out}")
        print()
        print("下一步：")
        print(f"  1) 设置环境变量（与 --api-key-env 一致）：set {args.api_key_env}=<client key>")
        print(f"  2) 列出模型：pi --list-models {PROVIDER_ID}")
        print(f"  3) 发一句话：pi --model {PROVIDER_ID}/{models[0]['alias']} -p \"用一句话回答：1+1 等于几？\"")
        return 0

    print(payload)
    print()
    print("--- settings.json（可选：设默认 provider/model）---")
    print(json.dumps(settings_json, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

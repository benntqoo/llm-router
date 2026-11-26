"""Turn a real HAR capture into a sanitized, committable fixture (T15).

Usage:
    python scripts/make_har_fixture.py <in.har> <out.har> [--all-entries]

The output keeps the real HAR 1.2 *shape* (log/creator/entries, per-entry
startedDateTime, timings, cache, request/response objects) but:

- keeps only entries whose URL contains ``/api/`` (a check-in HAR is about the
  API call, not about the page's static assets);
- drops the response body and **all** response headers (a real check-in
  response carries ``Set-Cookie: csrf_session=...``);
- replaces every sensitive request value with a fixed fake placeholder.

The result contains no real credential, so it can live in git
(AGENT.md section 8 forbids committing real keys/tokens).

The capture itself is done by ``scripts/capture_har.py``.
"""

import json
import sys

SENSITIVE_PATTERNS = (
    "cookie",
    "set-cookie",
    "authorization",
    "x-csrf-token",
    "csrf-token",
    "csrf",
    "token",
    "session",
    "jwt",
    "acw_tc",
    "x-xsrf-token",
)


def is_sensitive(name: str) -> bool:
    lower = name.lower()
    return any(pattern in lower for pattern in SENSITIVE_PATTERNS)


def sanitize_value(name: str, value: str) -> str:
    if not is_sensitive(name):
        return value
    if name.lower() == "cookie":
        parts = []
        for pair in value.split(";"):
            pair = pair.strip()
            if not pair:
                continue
            key = pair.split("=", 1)[0].strip()
            parts.append(f"{key}=REDACTED-{key}")
        return "; ".join(parts)
    return f"REDACTED-{name}"


def sanitize_entry(entry: dict) -> dict:
    request = entry.get("request", {})
    headers = []
    for header in request.get("headers", []):
        name = header.get("name", "")
        headers.append(
            {"name": name, "value": sanitize_value(name, header.get("value", ""))}
        )
    query = []
    for param in request.get("queryString", []):
        name = param.get("name", "")
        query.append(
            {"name": name, "value": sanitize_value(name, param.get("value", ""))}
        )
    new_request = {
        "method": request.get("method", ""),
        "url": request.get("url", ""),
        "httpVersion": request.get("httpVersion", "HTTP/1.1"),
        "cookies": [],
        "headers": headers,
        "queryString": query,
        "headersSize": -1,
        "bodySize": request.get("bodySize", 0),
    }
    if request.get("postData"):
        post = request["postData"]
        new_request["postData"] = {
            "mimeType": post.get("mimeType", ""),
            "text": post.get("text", ""),
        }
    return {
        "startedDateTime": entry.get("startedDateTime", ""),
        "time": entry.get("time", 0),
        "request": new_request,
        "response": {
            "status": entry.get("response", {}).get("status", 0),
            "statusText": entry.get("response", {}).get("statusText", ""),
            "httpVersion": entry.get("response", {}).get("httpVersion", "HTTP/1.1"),
            "cookies": [],
            "headers": [],
            "content": {"size": 0, "mimeType": "application/json"},
            "contentSize": 0,
            "_comment": "响应体与响应头已按 02 7.1.1 丢弃（Set-Cookie 带 csrf 密文）",
        },
        "cache": {},
        "timings": entry.get("timings", {"send": 0, "wait": 0, "receive": 0}),
    }


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    src, dst = sys.argv[1], sys.argv[2]
    keep_all = "--all-entries" in sys.argv[3:]

    with open(src, "r", encoding="utf-8") as f:
        har = json.load(f)

    entries = har.get("log", {}).get("entries", [])
    if not keep_all:
        entries = [e for e in entries if "/api/" in e.get("request", {}).get("url", "")]
    entries = entries[:12]

    out = {
        "log": {
            "version": har.get("log", {}).get("version", "1.2"),
            "creator": har.get("log", {}).get("creator", {"name": "captured", "version": "1"}),
            "pages": [],
            "entries": [sanitize_entry(e) for e in entries],
        }
    }
    with open(dst, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
    print(f"wrote {dst}: {len(out['log']['entries'])} entries")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

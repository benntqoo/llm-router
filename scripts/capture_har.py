"""Capture a real HAR 1.2 file with Playwright (T15 acceptance fixture).

Usage:
    python scripts/capture_har.py <out.har> <url> [full|minimal|omit]

The HAR is written by the browser itself (Chromium DevTools protocol), so the
file is a genuine browser export, not a hand-written approximation.

Note: the captured HAR may contain session cookies. It is a local temp artifact
and must never be committed as-is (AGENT.md section 5.3 / section 8).
"""

import json
import os
import sys

from playwright.sync_api import sync_playwright


def summarize(har_path: str) -> None:
    with open(har_path, "r", encoding="utf-8") as f:
        har = json.load(f)
    entries = har.get("log", {}).get("entries", [])
    lines = [
        "HAR version: %s" % har.get("log", {}).get("version"),
        "entries: %d" % len(entries),
    ]
    for e in entries:
        req = e.get("request", {})
        resp = e.get("response", {})
        url = req.get("url", "")
        lines.append(
            "ENTRY %s %s %s | req_hdrs=%s"
            % (
                req.get("method"),
                resp.get("status"),
                url.split("?", 1)[0][:110],
                ",".join(h.get("name", "") for h in req.get("headers", [])),
            )
        )
    with open(har_path + ".summary.txt", "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines[:3]))


def main() -> int:
    out_path = sys.argv[1]
    url = sys.argv[2]
    mode = sys.argv[3] if len(sys.argv) > 3 else "full"
    parent = os.path.dirname(out_path)
    if parent:
        os.makedirs(parent, exist_ok=True)

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(
            record_har_path=out_path,
            record_har_mode=mode,
            ignore_https_errors=True,
        )
        page = context.new_page()
        page.goto(url, wait_until="networkidle", timeout=90000)
        page.wait_for_timeout(3000)
        context.close()
        browser.close()

    summarize(out_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

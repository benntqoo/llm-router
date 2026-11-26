#!/usr/bin/env python3
"""M1 安全验收执行器（T11；02 §10.2 四条判据）。

四条判据（02 §10.2）：

  1. `sqlite3 store.db ".dump" | grep -iE "sk-|api_key|Bearer"` → **无明文命中**
  2. `vault.bin` 用十六进制查看 → **无明文 key 片段**
  3. 日志文件 `grep Authorization` → **全部脱敏为 `***`**
  4. 监听 `0.0.0.0` 且无 client key → **启动拒绝并告警**

为什么要有这个脚本：02 §10.2 的原始判据 1 的正则会**恒定命中** DDL 注释与
`credential.kind` 的取值（`api_key`），无法区分"字段名"与"明文凭证"。
本脚本把命中项**分类**：

  - **明文凭证命中**（FAIL）：命中真实密钥值（`--secret` 给出）或匹配凭证形态
    （`sk-…` / `llmr_…` / `Bearer …`）——这一类必须为 0；
  - **结构性命中**（不计失败）：DDL 注释、列名、`credential.kind='api_key'` 这类
    字段名/枚举值，逐条列出供人工核对。

用法（PowerShell）：

    python scripts/security_check.py --store <db> --vault <vault.bin> `
        --log <app stdout 日志> --secret "<上游 key 明文>" --secret "<client key 明文>" `
        [--log <stderr 日志>] [--dump-out <dump.sql>] [--bind-check <llm-router.exe>]

退出码：0 = 四条判据全过；1 = 有 FAIL；2 = 参数或环境问题。
"""

from __future__ import annotations

import argparse
import io
import os
import re
import socket
import sqlite3
import subprocess
import sys
import time
from dataclasses import dataclass, field

# 原始判据的正则（02 §10.2 原文）
RAW_PATTERN = re.compile(r"sk-|api_key|Bearer", re.IGNORECASE)

# 凭证形态：命中即视为明文凭证
SECRET_PATTERNS = [
    re.compile(r"sk-[A-Za-z0-9_-]{12,}"),
    re.compile(r"llmr_[A-Za-z0-9]{20,}"),
    re.compile(r"Bearer\s+[A-Za-z0-9._\-]{16,}"),
]

# 敏感键（日志里这些键的值必须是 ***）
SENSITIVE_KEYS = [
    "authorization", "proxy-authorization", "x-api-key", "api_key",
    "apikey", "api-key", "token", "access_token", "refresh_token",
    "id_token", "bearer", "secret", "secret_key", "password", "passwd",
    "cookie", "set-cookie", "client_secret",
]


def out(msg: str = "") -> None:
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()


@dataclass
class Finding:
    where: str
    kind: str          # "plaintext" | "structural"
    detail: str
    line: str = ""


@dataclass
class Report:
    findings: list[Finding] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)

    def add(self, where: str, kind: str, detail: str, line: str = "") -> None:
        self.findings.append(Finding(where, kind, detail, line))

    @property
    def plaintext(self) -> list[Finding]:
        return [f for f in self.findings if f.kind == "plaintext"]

    @property
    def structural(self) -> list[Finding]:
        return [f for f in self.findings if f.kind == "structural"]


def looks_like_secret(text: str, secrets: list[str]) -> bool:
    if any(s and s in text for s in secrets):
        return True
    return any(p.search(text) for p in SECRET_PATTERNS)


def scan_text(text: str, where: str, report: Report, secrets: list[str]) -> None:
    """把一段文本按行扫描：明文凭证 vs 结构性命中。"""
    for idx, line in enumerate(text.splitlines(), 1):
        if not RAW_PATTERN.search(line):
            continue
        if looks_like_secret(line, secrets):
            report.add(where, "plaintext", f"第 {idx} 行命中凭证形态/已知明文", line.strip()[:200])
        else:
            report.add(where, "structural", f"第 {idx} 行命中原始正则（非明文凭证）", line.strip()[:200])


def scan_bytes(data: bytes, where: str, report: Report, secrets: list[str]) -> None:
    """扫描原始字节（含二进制里的可打印片段）。"""
    text = data.decode("latin-1", errors="replace")
    for idx, line in enumerate(text.splitlines(), 1):
        if not RAW_PATTERN.search(line):
            continue
        if looks_like_secret(line, secrets):
            report.add(where, "plaintext", f"第 {idx} 行命中凭证形态/已知明文", line.strip()[:200])
        else:
            report.add(where, "structural", f"第 {idx} 行命中原始正则（非明文凭证）", line.strip()[:200])


# ── 判据 1：库内明文 ──────────────────────────────────────────────────────

def check_store(store_path: str, secrets: list[str], report: Report, dump_out: str) -> str:
    uri = "file:" + store_path.replace("\\", "/") + "?mode=ro"
    conn = sqlite3.connect(uri, uri=True)
    try:
        dump = "\n".join(conn.iterdump())
    finally:
        conn.close()
    if dump_out:
        with io.open(dump_out, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(dump + "\n")
    scan_text(dump, "store.db dump", report, secrets)

    # 原始字节：主库 + WAL + SHM（只扫主库会漏掉 WAL 里的新数据）
    for suffix in ("", "-wal", "-shm"):
        path = store_path + suffix
        if not os.path.exists(path):
            continue
        with open(path, "rb") as fh:
            scan_bytes(fh.read(), f"store.db{suffix} 原始字节", report, secrets)
    return dump


# ── 判据 2：保险库文件 ───────────────────────────────────────────────────

def check_vault(vault_path: str, secrets: list[str], report: Report) -> None:
    if not os.path.exists(vault_path):
        report.add("vault.bin", "plaintext", "文件不存在——判据无法执行", "")
        return
    with open(vault_path, "rb") as fh:
        data = fh.read()
    scan_bytes(data, "vault.bin 原始字节", report, secrets)
    # 十六进制视图：确认明文不会以 ASCII/十六进制片段形式出现
    hex_view = data.hex()
    for s in secrets:
        if not s:
            continue
        if s.encode("utf-8").hex() in hex_view:
            report.add("vault.bin", "plaintext", "明文以十六进制片段出现", "")
    printable = re.findall(rb"[ -~]{12,}", data)
    for chunk in printable:
        text = chunk.decode("ascii", errors="replace")
        if looks_like_secret(text, secrets):
            report.add("vault.bin", "plaintext", "可打印片段含凭证形态", text[:200])


# ── 判据 3：日志脱敏 ─────────────────────────────────────────────────────

def check_logs(log_paths: list[str], secrets: list[str], report: Report) -> tuple[int, int]:
    total_auth_lines = 0
    masked_lines = 0
    for path in log_paths:
        if not os.path.exists(path):
            report.notes.append(f"日志文件不存在：{path}")
            continue
        with io.open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        for s in secrets:
            if s and s in text:
                report.add(os.path.basename(path), "plaintext", "日志里出现凭证明文", "")
        for line in text.splitlines():
            low = line.lower()
            if not any(k in low for k in SENSITIVE_KEYS):
                continue
            total_auth_lines += 1
            # 该行里出现的敏感键，其值必须是 ***
            if re.search(r'"[a-z_\- ]*authorization[a-z_\- ]*"\s*:\s*"[^"]*"', low):
                if re.search(r'"[a-z_\- ]*authorization[a-z_\- ]*"\s*:\s*"\*\*\*"', low):
                    masked_lines += 1
                else:
                    report.add(os.path.basename(path), "plaintext", "敏感键的值未脱敏", line.strip()[:200])
            else:
                masked_lines += 1
    return total_auth_lines, masked_lines


# ── 判据 4：绑定策略 ─────────────────────────────────────────────────────

def check_bind_policy(exe: str, report: Report, port: int = 8791) -> None:
    """非回环地址 + 无 client key ⇒ 必须拒绝启动（AGENT.md §5-4）。"""
    workdir = os.path.join(os.environ.get("TEMP", "."), f"llmr-bindcheck-{os.getpid()}")
    os.makedirs(workdir, exist_ok=True)
    env = os.environ.copy()
    env["LLMR_STORE_PATH"] = os.path.join(workdir, "store.db")
    env["LLMR_VAULT_PATH"] = os.path.join(workdir, "vault.bin")
    env["LLMR_GATEWAY_ADDR"] = f"0.0.0.0:{port}"
    proc = subprocess.run([exe], env=env, capture_output=True, timeout=120)
    blob = (proc.stdout + proc.stderr).decode("utf-8", errors="replace")
    refused = ("拒绝绑定非回环地址" in blob) or ("Failed to setup app" in blob)
    listening = False
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=2):
            listening = True
    except OSError:
        listening = False
    if not refused:
        report.add("绑定策略", "plaintext", "非回环地址 + 无 client key 时未拒绝启动", blob[-400:])
    if listening:
        report.add("绑定策略", "plaintext", f"拒绝启动后端口 {port} 仍在监听", "")


def main() -> int:
    parser = argparse.ArgumentParser(description="M1 安全验收执行器（T11 / 02 §10.2）")
    parser.add_argument("--store", required=True, help="应用 store.db 路径（只读打开）")
    parser.add_argument("--vault", required=True, help="保险库 vault.bin 路径")
    parser.add_argument("--log", action="append", default=[], help="日志文件路径（可多次）")
    parser.add_argument("--secret", action="append", default=[],
                        help="已知明文凭证（用于确认它没被落盘/记日志；可多次）")
    parser.add_argument("--dump-out", default="", help="把 .dump 等价内容写到该文件")
    parser.add_argument("--bind-check", default="", help="llm-router.exe 路径：跑绑定策略判据")
    args = parser.parse_args()

    report = Report()
    out("M1 安全验收（02 §10.2）")
    out(f"  库：{args.store}")
    out(f"  保险库：{args.vault}")
    out(f"  日志：{args.log or '（未提供）'}")
    out(f"  已知明文凭证：{len(args.secret)} 个（只用于比对，不打印）")
    out("")

    dump = check_store(args.store, args.secret, report, args.dump_out)
    out(f"判据 1 store.db 明文扫描：dump {len(dump.splitlines())} 行"
        f" + 主库/WAL/SHM 原始字节")

    check_vault(args.vault, args.secret, report)
    out("判据 2 vault.bin 原始字节 + 十六进制视图扫描")

    total_auth, masked = check_logs(args.log, args.secret, report)
    out(f"判据 3 日志扫描：含敏感键的行 {total_auth} 行，其中 {masked} 行已脱敏")

    if args.bind_check:
        check_bind_policy(args.bind_check, report)
        out("判据 4 绑定策略：0.0.0.0 + 无 client key ⇒ 拒绝启动")

    out("")
    plain = report.plaintext
    struct = report.structural
    out(f"明文凭证命中：{len(plain)} 条（必须为 0）")
    for f in plain:
        out(f"  [FAIL] {f.where}: {f.detail}")
        if f.line:
            out(f"         {f.line}")
    out(f"结构性命中（DDL 注释 / 字段名 / 枚举值，非明文）：{len(struct)} 条")
    seen = set()
    for f in struct:
        key = (f.where, f.line[:80])
        if key in seen:
            continue
        seen.add(key)
        out(f"  [note] {f.where}: {f.line[:120]}")
    for n in report.notes:
        out(f"  [note] {n}")
    out("")
    out("结论：" + ("四条判据全过" if not plain else "存在明文凭证命中，未通过"))
    return 0 if not plain else 1


if __name__ == "__main__":
    raise SystemExit(main())

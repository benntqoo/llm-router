"""
T18 真机验收脚本（只读）：查签到结果行 + 扫描明文。

- 读 `checkin_result` 最近几行（证明插件结果真落库）；
- 四层扫描 `store.db` / `store.db-wal` / `store.db-shm` / `vault.bin`：**登录态明文必须 0 命中**。

用法：python t18_scan.py <明文串>
"""
import json
import os
import sqlite3
import sys

APP_DIR = os.path.join(os.environ["APPDATA"], "llm-router")
DB = os.path.join(APP_DIR, "store.db")
VAULT = os.path.join(APP_DIR, "vault.bin")


def recent_results(limit=10):
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    try:
        rows = conn.execute(
            "SELECT id, site_id, ts, ok, reward_json, message "
            "FROM checkin_result ORDER BY id DESC LIMIT ?",
            (limit,),
        ).fetchall()
        sites = conn.execute("SELECT id, name, plugin, config_json FROM checkin_site").fetchall()
        accounts = conn.execute("SELECT id, channel_id, quota_json FROM account").fetchall()
        return rows, sites, accounts
    finally:
        conn.close()


def scan(secret: str):
    """四层扫描明文（只读）。"""
    hits = []
    targets = [
        ("store.db", DB),
        ("store.db-wal", DB + "-wal"),
        ("store.db-shm", DB + "-shm"),
        ("vault.bin", VAULT),
    ]
    needle = secret.encode("utf-8")
    for name, path in targets:
        if not os.path.exists(path):
            continue
        with open(path, "rb") as handle:
            data = handle.read()
        if needle in data:
            hits.append(name)
    return hits


def main():
    rows, sites, accounts = recent_results()
    print("=== checkin_result 最近 10 行 ===")
    for row in rows:
        print(f"  id={row[0]} site={row[1]} ts={row[2]} ok={row[3]} reward={row[4]} msg={row[5]}")
    print("=== checkin_site ===")
    for site in sites:
        config = json.loads(site[3]) if site[3] else {}
        print(f"  {site[0]} name={site[1]} plugin={site[2]} enabled-config-keys={sorted(config.keys())}")
        print(f"      credential_id={config.get('credentialId')} token={config.get('tokenCredentialId')} account={config.get('accountId')}")
    print("=== account ===")
    for account in accounts:
        print(f"  {account[0]} channel={account[1]} quota_json={account[2]}")

    secret = sys.argv[1] if len(sys.argv) > 1 else ""
    if secret:
        hits = scan(secret)
        print(f"=== 明文扫描（{secret!r}）===")
        print(f"  命中层数：{len(hits)}  {hits if hits else '（0 命中 ✓）'}")


if __name__ == "__main__":
    main()

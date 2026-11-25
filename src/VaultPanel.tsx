import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  HIDDEN,
  RevealState,
  isRevealing,
  remainingSeconds,
  revealNow,
  visibleSecret,
} from "./vault/reveal";

type VaultStatus = {
  path: string;
  key_version: number;
  locked: boolean;
  password_mode: boolean;
  credential_count: number;
};

type CredentialMeta = {
  id: string;
  channel_id: string;
  kind: string;
  created_at: number;
  updated_at: number;
  expires_at: number | null;
  key_version: number;
};

function formatTime(ts: number): string {
  return new Date(ts * 1000).toLocaleString();
}

export default function VaultPanel() {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [credentials, setCredentials] = useState<CredentialMeta[]>([]);
  const [channelName, setChannelName] = useState("DeepSeek 官方");
  const [secretInput, setSecretInput] = useState("");
  const [unlockPassword, setUnlockPassword] = useState("");
  const [masterPassword, setMasterPassword] = useState("");
  const [revealed, setRevealed] = useState<RevealState>(HIDDEN);
  const [revealedId, setRevealedId] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [exportConfirm, setExportConfirm] = useState(false);
  const [exportedPlain, setExportedPlain] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const s = await invoke<VaultStatus>("vault_status");
      setStatus(s);
      const list = await invoke<CredentialMeta[]>("vault_list_secrets", {
        channelName: null,
      });
      setCredentials(list);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 倒计时：仅在有展示中的明文时运行
  useEffect(() => {
    if (revealed.expireAt === null) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, [revealed.expireAt]);

  // 到期即遮蔽：明文从状态中移除
  useEffect(() => {
    if (revealed.expireAt !== null && now >= revealed.expireAt) {
      setRevealed(HIDDEN);
      setRevealedId(null);
    }
  }, [now, revealed]);

  const visible = useMemo(() => visibleSecret(revealed, now), [revealed, now]);
  const revealing = isRevealing(revealed, now);
  const seconds = remainingSeconds(revealed, now);

  const fail = (e: unknown) => {
    setError(String(e));
    setMessage(null);
  };

  const handleSave = async () => {
    if (!secretInput.trim()) {
      setError("请输入要保存的密钥");
      return;
    }
    try {
      await invoke<CredentialMeta>("vault_put_secret", {
        channelName: channelName.trim(),
        kind: "api_key",
        secret: secretInput,
      });
      setSecretInput("");
      setError(null);
      setMessage("已加密保存");
      await refresh();
    } catch (e) {
      fail(e);
    }
  };

  const handleReveal = async (id: string) => {
    try {
      const secret = await invoke<string>("vault_reveal_secret", { id });
      const stamp = Date.now();
      setNow(stamp);
      setRevealed(revealNow(secret, stamp));
      setRevealedId(id);
      setError(null);
    } catch (e) {
      fail(e);
    }
  };

  const handleHide = () => {
    setRevealed(HIDDEN);
    setRevealedId(null);
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke<boolean>("vault_delete_secret", { id });
      if (revealedId === id) {
        handleHide();
      }
      setError(null);
      setMessage("已删除");
      await refresh();
    } catch (e) {
      fail(e);
    }
  };

  const handleExportPlain = async () => {
    if (!exportConfirm) {
      setError("导出明文需二次确认：请先勾选确认框");
      return;
    }
    try {
      const ids = credentials.map((c) => c.id);
      const json = await invoke<string>("vault_export_plaintext", {
        ids,
        confirm: true,
      });
      setExportConfirm(false);
      setExportedPlain(json);
      setError(null);
      setMessage(`已导出 ${ids.length} 条明文（仅在内存中，请立即保存并关闭）`);
    } catch (e) {
      fail(e);
    }
  };

  const handleExportBackup = async () => {
    try {
      const files = await invoke<string[]>("vault_export_encrypted_backup", {
        targetDir: "",
      });
      setError(null);
      setMessage(`已导出加密备份：${files.join("、")}`);
    } catch (e) {
      fail(e);
    }
  };

  const handleUnlock = async () => {
    try {
      await invoke<boolean>("vault_unlock", { password: unlockPassword });
      setUnlockPassword("");
      setError(null);
      setMessage("保险库已解锁");
      await refresh();
    } catch (e) {
      fail(e);
    }
  };

  const handleTogglePasswordMode = async () => {
    try {
      if (status?.password_mode) {
        await invoke<boolean>("vault_disable_password_mode");
        setMessage("已关闭主密码模式（主密钥改由系统原生保护）");
      } else {
        await invoke<boolean>("vault_enable_password_mode", {
          password: masterPassword,
        });
        setMasterPassword("");
        setMessage("已启用主密码模式：下次启动需输入主密码解锁");
      }
      await refresh();
    } catch (e) {
      fail(e);
    }
  };

  return (
    <section className="card">
      <h2>加密保险库</h2>

      {status && (
        <dl className="status">
          <dt>保险库文件</dt>
          <dd>
            <code>{status.path}</code>
          </dd>
          <dt>主密钥版本</dt>
          <dd>
            v{status.key_version}
            {status.password_mode ? "（主密码模式）" : "（系统原生保护）"}
            {status.locked ? " · 已锁定" : ""}
          </dd>
          <dt>凭证条数</dt>
          <dd>{status.credential_count}</dd>
        </dl>
      )}

      {status?.locked && (
        <div className="row-inline">
          <input
            type="password"
            value={unlockPassword}
            onChange={(e) => setUnlockPassword(e.target.value)}
            placeholder="主密码"
          />
          <button type="button" onClick={handleUnlock}>
            解锁
          </button>
        </div>
      )}

      <h3>录入密钥</h3>
      <div className="row-inline">
        <input
          value={channelName}
          onChange={(e) => setChannelName(e.target.value)}
          placeholder="渠道名"
        />
        <input
          type="password"
          value={secretInput}
          onChange={(e) => setSecretInput(e.target.value)}
          placeholder="API Key（加密后落库）"
        />
        <button type="button" onClick={handleSave}>
          保存
        </button>
      </div>

      <h3>已存凭据</h3>
      {credentials.length === 0 && <p className="muted">暂无凭据。</p>}
      <ul className="cred-list">
        {credentials.map((c) => (
          <li key={c.id}>
            <div className="cred-row">
              <span className="cred-meta">
                <code>{c.kind}</code> · {c.id.slice(0, 12)}… · {formatTime(c.created_at)}
              </span>
              <span className="cred-actions">
                <button type="button" onClick={() => handleReveal(c.id)}>
                  显示
                </button>
                {revealedId === c.id && (
                  <button type="button" onClick={handleHide}>
                    隐藏
                  </button>
                )}
                <button type="button" onClick={() => handleDelete(c.id)}>
                  删除
                </button>
              </span>
            </div>
            {revealedId === c.id && (
              <div className="cred-secret">
                {revealing ? visible : "••••••••"}
                {revealing && <em>（{seconds}s 后自动遮蔽）</em>}
              </div>
            )}
          </li>
        ))}
      </ul>

      <h3>导出</h3>
      <div className="row-inline">
        <label className="checkbox">
          <input
            type="checkbox"
            checked={exportConfirm}
            onChange={(e) => setExportConfirm(e.target.checked)}
          />
          我确认导出明文（明文可被其他程序读取，请立即妥善保存）
        </label>
        <button type="button" onClick={handleExportPlain}>
          导出明文
        </button>
        <button type="button" onClick={handleExportBackup}>
          导出加密备份
        </button>
      </div>
      {exportedPlain && (
        <div className="export-plain">
          <pre>{exportedPlain}</pre>
          <button type="button" onClick={() => setExportedPlain(null)}>
            清除明文
          </button>
        </div>
      )}

      <h3>主密码模式</h3>
      <div className="row-inline">
        <input
          type="password"
          value={masterPassword}
          onChange={(e) => setMasterPassword(e.target.value)}
          placeholder="主密码（Argon2id, m=64MiB/t=3/p=1）"
        />
        <button type="button" onClick={handleTogglePasswordMode}>
          {status?.password_mode ? "关闭主密码模式" : "启用主密码模式"}
        </button>
      </div>

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

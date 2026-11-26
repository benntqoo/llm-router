import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type CreatedClientKey = {
  id: string;
  key: string;
  key_prefix: string;
};

type ClientKeyMeta = {
  id: string;
  label: string;
  key_prefix: string;
  created_at: number;
  expires_at: number | null;
  rpm_limit: number | null;
  daily_tokens: number | null;
};

function formatTime(ts: number): string {
  return new Date(ts * 1000).toLocaleString();
}

/** 把空字符串换成 undefined（= 不限）。 */
function toLimit(raw: string): number | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  const n = Number(trimmed);
  if (!Number.isFinite(n) || n < 0) return null;
  return Math.floor(n);
}

export default function ClientKeyPanel() {
  const [keys, setKeys] = useState<ClientKeyMeta[]>([]);
  const [label, setLabel] = useState("pi");
  const [created, setCreated] = useState<CreatedClientKey | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState("");
  const [editRpm, setEditRpm] = useState("");
  const [editTokens, setEditTokens] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setKeys(await invoke<ClientKeyMeta[]>("client_key_list"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleCreate = async () => {
    try {
      const result = await invoke<CreatedClientKey>("client_key_create", {
        label: label.trim() || "未命名",
      });
      setCreated(result);
      setError(null);
      setMessage("已创建：明文只显示这一次，请立即复制保存");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const startEdit = (k: ClientKeyMeta) => {
    setEditingId(k.id);
    setEditLabel(k.label);
    setEditRpm(k.rpm_limit === null ? "" : String(k.rpm_limit));
    setEditTokens(k.daily_tokens === null ? "" : String(k.daily_tokens));
    setMessage(null);
    setError(null);
  };

  const handleSaveEdit = async (id: string) => {
    try {
      await invoke<boolean>("client_key_update_meta", {
        id,
        label: editLabel,
        rpmLimit: toLimit(editRpm),
        dailyTokens: toLimit(editTokens),
        expiresAt: null,
      });
      setEditingId(null);
      setError(null);
      setMessage("已保存");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke<boolean>("client_key_delete", { id });
      if (editingId === id) setEditingId(null);
      setError(null);
      setMessage("已删除");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <section className="card">
      <h2>Client Key</h2>
      <p className="muted">
        客户端（pi 等）填入的本地口令。只存 SHA-256 哈希，明文创建后不可再查看。
      </p>

      <div className="row-inline">
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="用途标签，如 pi"
        />
        <button type="button" onClick={handleCreate}>
          创建 client key
        </button>
      </div>

      {created && (
        <div className="export-plain">
          <p className="ok">新 key（仅显示一次）：</p>
          <pre>{created.key}</pre>
          <div className="row-inline">
            <button type="button" onClick={() => setCreated(null)}>
              我已保存，隐藏
            </button>
            <button
              type="button"
              onClick={() => navigator.clipboard?.writeText(created.key)}
            >
              复制
            </button>
          </div>
        </div>
      )}

      {keys.length === 0 ? (
        <p className="muted">暂无 client key。</p>
      ) : (
        <ul className="cred-list">
          {keys.map((k) => (
            <li key={k.id}>
              <div className="cred-row">
                <span className="cred-meta">
                  <code>{k.key_prefix}…</code> · {k.label} · {formatTime(k.created_at)}
                  {k.rpm_limit ? ` · ${k.rpm_limit} rpm` : ""}
                  {k.daily_tokens ? ` · ${k.daily_tokens} tokens/日` : ""}
                </span>
                <span className="cred-actions">
                  <button type="button" onClick={() => startEdit(k)}>
                    {editingId === k.id ? "收起" : "编辑"}
                  </button>
                  <button type="button" onClick={() => handleDelete(k.id)}>
                    删除
                  </button>
                </span>
              </div>
              {editingId === k.id && (
                <div className="row-inline alias-row">
                  <label className="field">
                    <span>标签</span>
                    <input
                      type="text"
                      value={editLabel}
                      onChange={(e) => setEditLabel(e.target.value)}
                    />
                  </label>
                  <label className="field">
                    <span>每分钟请求上限（留空 = 不限）</span>
                    <input
                      type="number"
                      value={editRpm}
                      onChange={(e) => setEditRpm(e.target.value)}
                    />
                  </label>
                  <label className="field">
                    <span>每日 token 上限（留空 = 不限）</span>
                    <input
                      type="number"
                      value={editTokens}
                      onChange={(e) => setEditTokens(e.target.value)}
                    />
                  </label>
                  <button type="button" onClick={() => handleSaveEdit(k.id)}>
                    保存
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

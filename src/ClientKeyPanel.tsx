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

/**
 * client key 面板（T3 的最小前端接入）。
 *
 * T9 将把它并入完整 UI；此处只保证 T3 的 client key 功能在应用里可用。
 */
export default function ClientKeyPanel() {
  const [keys, setKeys] = useState<ClientKeyMeta[]>([]);
  const [label, setLabel] = useState("pi");
  const [created, setCreated] = useState<CreatedClientKey | null>(null);
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

  const handleDelete = async (id: string) => {
    try {
      await invoke<boolean>("client_key_delete", { id });
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
          <button type="button" onClick={() => setCreated(null)}>
            我已保存，隐藏
          </button>
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
                </span>
                <span className="cred-actions">
                  <button type="button" onClick={() => handleDelete(k.id)}>
                    删除
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

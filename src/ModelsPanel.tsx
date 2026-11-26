import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { onDataChanged } from "./panels/bus";

type PublishedModel = {
  alias: string;
  owned_by: string;
};

type ModelsPreview = {
  base_url: string;
  models: PublishedModel[];
};

/**
 * 模型预览面板（T9）。
 *
 * 数据源与网关 `GET /v1/models` **同源**（`store::channels::list_published_models`），
 * 所以这里列出的别名 = 客户端真正能调到的模型。
 */
export default function ModelsPanel() {
  const [preview, setPreview] = useState<ModelsPreview | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setPreview(await invoke<ModelsPreview>("models_preview"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 渠道一变，预览立刻跟着变（别名增删 / 渠道禁用都要立刻反映）
  useEffect(() => {
    const unsubscribe = onDataChanged(() => {
      void refresh();
    });
    const timer = window.setInterval(() => {
      void refresh();
    }, 5000);
    return () => {
      unsubscribe();
      window.clearInterval(timer);
    };
  }, [refresh]);

  const baseUrl = preview?.base_url ?? "";
  const sampleAlias = preview?.models[0]?.alias ?? "<模型别名>";

  const snippet = [
    `base_url = ${baseUrl}`,
    `api_key  = <你的 client key>`,
    `model    = ${sampleAlias}`,
  ].join("\n");

  const copy = async (text: string, what: string) => {
    try {
      await navigator.clipboard?.writeText(text);
      setMessage(`已复制${what}`);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <section className="card">
      <h2>模型预览</h2>
      <p className="muted">
        这里列出的别名 = <code>GET /v1/models</code> 会返回的别名（与网关同源）。
        客户端把 <code>model</code> 设成其中之一即可调用。
      </p>

      <div className="row-inline">
        <button type="button" onClick={refresh}>
          刷新
        </button>
        {preview && (
          <span className="muted">
            {preview.models.length} 个别名 · base_url <code>{preview.base_url}</code>
          </span>
        )}
      </div>

      {preview && preview.models.length === 0 && (
        <p className="muted">
          还没有对外发布的模型——去「渠道」面板建一个渠道并至少加一条模型别名。
        </p>
      )}

      {preview && preview.models.length > 0 && (
        <ul className="cred-list">
          {preview.models.map((m) => (
            <li key={m.alias}>
              <div className="cred-row">
                <span className="cred-meta">
                  <code>{m.alias}</code> · {m.owned_by}
                </span>
                <span className="cred-actions">
                  <button type="button" onClick={() => copy(m.alias, "别名")}>
                    复制别名
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}

      <h3>客户端这样填</h3>
      <div className="export-plain">
        <pre>{snippet}</pre>
      </div>
      <div className="row-inline">
        <button type="button" onClick={() => copy(snippet, "接入片段")}>
          复制接入片段
        </button>
      </div>

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

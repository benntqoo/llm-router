import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { notifyDataChanged } from "./panels/bus";

type ModelSuggestion = {
  upstream_id: string;
  alias: string;
};

type PresetView = {
  provider: string;
  name: string;
  base_url: string;
  chat_path: string;
  kind: string;
  auth_style: string;
  inject_stream_usage: boolean;
  supports_groupid_query: boolean;
  tool_support: string;
  default_models: ModelSuggestion[];
};

type ModelSummary = {
  id: string;
  alias: string;
  upstream_id: string;
  enabled: boolean;
};

type ChannelSummary = {
  id: string;
  name: string;
  kind: string;
  provider: string;
  base_url: string;
  enabled: boolean;
  priority: number;
  meta: {
    path_chat: string | null;
    auth_style: string | null;
    auth_header_name: string | null;
    group_id: string | null;
    supports_groupid_query: boolean | null;
    inject_stream_usage: boolean | null;
    extra_headers: Record<string, string>;
  };
  models: ModelSummary[];
  has_credential: boolean;
};

type ModelRow = {
  alias: string;
  upstream_id: string;
  enabled: boolean;
};

type ChannelInput = {
  id: string | null;
  name: string;
  provider: string;
  base_url: string;
  kind: string | null;
  enabled: boolean;
  priority: number | null;
  meta: ChannelSummary["meta"];
  models: ModelRow[];
};

const CUSTOM_PROVIDER = "custom";

/** 解析"每行一条 k=v"的额外头输入。 */
function parseHeaders(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const idx = trimmed.indexOf("=");
    if (idx <= 0) continue;
    const key = trimmed.slice(0, idx).trim();
    const value = trimmed.slice(idx + 1).trim();
    if (key) out[key] = value;
  }
  return out;
}

export default function ChannelPanel() {
  const [presets, setPresets] = useState<PresetView[]>([]);
  const [channels, setChannels] = useState<ChannelSummary[]>([]);
  const [mode, setMode] = useState<"preset" | "custom">("preset");
  const [editingId, setEditingId] = useState<string | null>(null);

  const [presetProvider, setPresetProvider] = useState("");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [chatPath, setChatPath] = useState("/v1/chat/completions");
  const [authStyle, setAuthStyle] = useState<"bearer" | "custom_header">("bearer");
  const [authHeaderName, setAuthHeaderName] = useState("");
  const [groupId, setGroupId] = useState("");
  const [supportsGroupId, setSupportsGroupId] = useState(false);
  const [injectUsage, setInjectUsage] = useState(true);
  const [extraHeaders, setExtraHeaders] = useState("");
  const [priority, setPriority] = useState(100);
  const [enabled, setEnabled] = useState(true);
  const [rows, setRows] = useState<ModelRow[]>([]);
  const [secret, setSecret] = useState("");

  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<ChannelSummary[]>("channel_list");
      setChannels(list);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    invoke<PresetView[]>("channel_presets")
      .then((list) => {
        setPresets(list);
        if (list.length > 0) {
          setPresetProvider(list[0].provider);
          applyPreset(list[0]);
        }
      })
      .catch((e) => setError(String(e)));
    void refresh();
  }, [refresh]);

  /** 选中预设：base_url / 路径 / 鉴权 / include_usage 全部预填，只剩密钥与模型名待确认。 */
  function applyPreset(preset: PresetView) {
    setPresetProvider(preset.provider);
    setName((prev) => (prev.trim() ? prev : preset.name));
    setBaseUrl(preset.base_url);
    setChatPath(preset.chat_path);
    setAuthStyle(preset.auth_style === "custom_header" ? "custom_header" : "bearer");
    setAuthHeaderName("");
    setGroupId("");
    setSupportsGroupId(preset.supports_groupid_query);
    setInjectUsage(preset.inject_stream_usage);
    setExtraHeaders("");
    const first = preset.default_models[0];
    setRows((prev) =>
      prev.length > 0
        ? prev
        : first
          ? [{ alias: first.alias, upstream_id: first.upstream_id, enabled: true }]
          : [],
    );
  }

  const selectedPreset = useMemo(
    () => presets.find((p) => p.provider === presetProvider) ?? null,
    [presets, presetProvider],
  );

  function resetForm() {
    setEditingId(null);
    setName("");
    setBaseUrl("");
    setChatPath("/v1/chat/completions");
    setAuthStyle("bearer");
    setAuthHeaderName("");
    setGroupId("");
    setSupportsGroupId(false);
    setInjectUsage(true);
    setExtraHeaders("");
    setPriority(100);
    setEnabled(true);
    setRows([]);
    setSecret("");
  }

  function loadForEdit(channel: ChannelSummary) {
    setMode(channel.provider === CUSTOM_PROVIDER ? "custom" : "preset");
    setEditingId(channel.id);
    setPresetProvider(
      channel.provider === CUSTOM_PROVIDER ? "" : channel.provider,
    );
    setName(channel.name);
    setBaseUrl(channel.base_url);
    setChatPath(channel.meta.path_chat ?? "/v1/chat/completions");
    setAuthStyle(
      channel.meta.auth_style === "custom_header" ? "custom_header" : "bearer",
    );
    setAuthHeaderName(channel.meta.auth_header_name ?? "");
    setGroupId(channel.meta.group_id ?? "");
    setSupportsGroupId(channel.meta.supports_groupid_query ?? false);
    setInjectUsage(channel.meta.inject_stream_usage ?? true);
    setExtraHeaders(
      Object.entries(channel.meta.extra_headers ?? {})
        .map(([k, v]) => `${k}=${v}`)
        .join("\n"),
    );
    setPriority(channel.priority);
    setEnabled(channel.enabled);
    setRows(
      channel.models.map((m) => ({
        alias: m.alias,
        upstream_id: m.upstream_id,
        enabled: m.enabled,
      })),
    );
    setSecret("");
  }

  function updateRow(index: number, patch: Partial<ModelRow>) {
    setRows((prev) =>
      prev.map((row, i) => (i === index ? { ...row, ...patch } : row)),
    );
  }

  function removeRow(index: number) {
    setRows((prev) => prev.filter((_, i) => i !== index));
  }

  function addRow() {
    setRows((prev) => [
      ...prev,
      { alias: "", upstream_id: "", enabled: true },
    ]);
  }

  const handleSave = async () => {
    if (!name.trim()) {
      setError("渠道名不能为空");
      return;
    }
    if (!baseUrl.trim()) {
      setError("base_url 不能为空");
      return;
    }
    const models = rows.filter((r) => r.alias.trim() || r.upstream_id.trim());
    if (models.length === 0) {
      setError("至少要有一个模型别名（否则渠道不会出现在 /v1/models）");
      return;
    }
    if (authStyle === "custom_header" && !authHeaderName.trim()) {
      setError("自定义鉴权样式需要填写头名（例如 x-api-key）");
      return;
    }
    const provider =
      mode === "custom" ? CUSTOM_PROVIDER : presetProvider || CUSTOM_PROVIDER;
    const payload: ChannelInput = {
      id: editingId,
      name: name.trim(),
      provider,
      base_url: baseUrl.trim(),
      kind: "api_key",
      enabled,
      priority,
      meta: {
        path_chat: chatPath.trim() || null,
        auth_style: authStyle,
        auth_header_name:
          authStyle === "custom_header" ? authHeaderName.trim() : null,
        group_id: groupId.trim() || null,
        supports_groupid_query: supportsGroupId,
        inject_stream_usage: injectUsage,
        extra_headers: parseHeaders(extraHeaders),
      },
      models: models.map((m) => ({
        alias: m.alias.trim(),
        upstream_id: m.upstream_id.trim(),
        enabled: m.enabled,
      })),
    };
    try {
      const saved = await invoke<ChannelSummary>("channel_save", { payload });
      // 保存后进入"编辑态"：再改别名、再点保存是**更新**同一条渠道，
      // 而不是新建一条同名渠道（那会撞 UNIQUE(channel.name)）
      setEditingId(saved.id);
      if (secret.trim()) {
        await invoke("channel_put_key", {
          channelId: saved.id,
          secret: secret.trim(),
        });
      }
      const aliasNames = payload.models.map((m) => m.alias).join("、");
      setSecret("");
      setError(null);
      setMessage(
        `已保存渠道「${saved.name}」，别名 ${aliasNames} 已出现在 /v1/models` +
          (secret.trim() ? "（密钥已加密入库）" : ""),
      );
      await refresh();
      // 保险库面板与模型预览面板要同步（否则一个说"密钥已录入"、另一个说"0 条凭据"）
      notifyDataChanged();
    } catch (e) {
      setError(String(e));
      setMessage(null);
    }
  };

  const handleDelete = async (channel: ChannelSummary) => {
    const ok = window.confirm(
      `删除渠道「${channel.name}」？其模型与已录入的密钥会一并删除。`,
    );
    if (!ok) return;
    try {
      await invoke<boolean>("channel_delete", { id: channel.id });
      if (editingId === channel.id) resetForm();
      setError(null);
      setMessage(`已删除渠道「${channel.name}」`);
      await refresh();
      notifyDataChanged();
    } catch (e) {
      setError(String(e));
      setMessage(null);
    }
  };

  return (
    <section className="card">
      <h2>渠道</h2>
      <p className="muted">
        预设渠道：选中后 base_url / 路径 / 鉴权样式都已填好，只剩填密钥；
        自定义渠道：任何 OpenAI 兼容端点，填表即可（无需改代码）。
        保存后模型别名立即出现在 <code>GET /v1/models</code>。
      </p>

      <h3>新增 / 编辑渠道</h3>
      <div className="row-inline">
        <button
          type="button"
          onClick={() => {
            setMode("preset");
            resetForm();
            // 切到预设模式：立刻预填当前预设的默认值（否则表单是空的，
            // 与"选中预设后仅剩填 key 一步"的预期不符）
            const preset = selectedPreset ?? presets[0];
            if (preset) applyPreset(preset);
          }}
        >
          预设渠道
        </button>
        <button
          type="button"
          onClick={() => {
            setMode("custom");
            resetForm();
          }}
        >
          自定义渠道
        </button>
        {editingId && (
          <button type="button" onClick={resetForm}>
            取消编辑
          </button>
        )}
      </div>

      {mode === "preset" ? (
        <>
          <div className="row-inline">
            <label className="field">
              <span>预设</span>
              <select
                value={presetProvider}
                onChange={(e) => {
                  const preset = presets.find((p) => p.provider === e.target.value);
                  if (preset) applyPreset(preset);
                }}
              >
                {presets.map((p) => (
                  <option key={p.provider} value={p.provider}>
                    {p.name}（{p.provider}）
                  </option>
                ))}
              </select>
            </label>
            {selectedPreset && (
              <span className="muted">
                {selectedPreset.base_url}
                {selectedPreset.chat_path}
              </span>
            )}
          </div>
          {selectedPreset && (
            <p className="muted">
              base_url、路径模板、鉴权样式、include_usage 注入均按预设预填；
              {selectedPreset.supports_groupid_query
                ? "该预设支持 GroupId（国内站可在下方补填）。"
                : "该预设不带 GroupId。"}
            </p>
          )}
        </>
      ) : (
        <>
          <div className="row-inline">
            <label className="field">
              <span>base_url</span>
              <input
                type="text"
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                placeholder="https://api.example.com"
              />
            </label>
            <label className="field">
              <span>路径模板</span>
              <input
                type="text"
                value={chatPath}
                onChange={(e) => setChatPath(e.target.value)}
                placeholder="/v1/chat/completions"
              />
            </label>
          </div>
          <div className="row-inline">
            <label className="field">
              <span>鉴权样式</span>
              <select
                value={authStyle}
                onChange={(e) =>
                  setAuthStyle(
                    e.target.value === "custom_header"
                      ? "custom_header"
                      : "bearer",
                  )
                }
              >
                <option value="bearer">Bearer（Authorization）</option>
                <option value="custom_header">自定义头</option>
              </select>
            </label>
            {authStyle === "custom_header" && (
              <label className="field">
                <span>鉴权头名</span>
                <input
                  type="text"
                  value={authHeaderName}
                  onChange={(e) => setAuthHeaderName(e.target.value)}
                  placeholder="x-api-key"
                />
              </label>
            )}
          </div>
          <div className="row-inline">
            <label className="field">
              <span>GroupId（MiniMax 国内站）</span>
              <input
                type="text"
                value={groupId}
                onChange={(e) => setGroupId(e.target.value)}
                placeholder="可留空"
              />
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={supportsGroupId}
                onChange={(e) => setSupportsGroupId(e.target.checked)}
              />
              追加 ?GroupId=
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={injectUsage}
                onChange={(e) => setInjectUsage(e.target.checked)}
              />
              注入 stream_options.include_usage
            </label>
          </div>
          <label className="field block">
            <span>额外固定头（每行一条 key=value）</span>
            <textarea
              value={extraHeaders}
              onChange={(e) => setExtraHeaders(e.target.value)}
              placeholder={"x-foo=bar"}
              rows={2}
            />
          </label>
        </>
      )}

      <div className="row-inline">
        <label className="field">
          <span>渠道名</span>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="DeepSeek 官方"
          />
        </label>
        <label className="field">
          <span>优先级（越小越优先）</span>
          <input
            type="number"
            value={priority}
            onChange={(e) => setPriority(Number(e.target.value) || 0)}
          />
        </label>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(e) => setEnabled(e.target.checked)}
          />
          启用该渠道
        </label>
      </div>

      <div className="alias-block">
        <div className="row-inline">
          <strong>模型别名</strong>
          <button type="button" onClick={addRow}>
            + 添加别名
          </button>
          <span className="muted">
            别名是客户端看到的名字（全局唯一）；上游模型名是真实模型。
          </span>
        </div>
        {rows.length === 0 && (
          <p className="muted">还没有别名。至少添加一条，否则渠道不会对外发布。</p>
        )}
        {rows.map((row, index) => (
          <div className="row-inline alias-row" key={index}>
            <label className="field">
              <span>别名</span>
              <input
                type="text"
                value={row.alias}
                onChange={(e) => updateRow(index, { alias: e.target.value })}
                placeholder="deepseek-chat"
              />
            </label>
            <label className="field">
              <span>上游模型名</span>
              <input
                type="text"
                value={row.upstream_id}
                onChange={(e) => updateRow(index, { upstream_id: e.target.value })}
                placeholder="deepseek-chat"
              />
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={row.enabled}
                onChange={(e) => updateRow(index, { enabled: e.target.checked })}
              />
              启用
            </label>
            <button type="button" onClick={() => removeRow(index)}>
              删除
            </button>
          </div>
        ))}
        {mode === "preset" && (
          <p className="muted">
            默认模型是「未实测」的建议值（02 §6.1.1），可改成你实际可用的模型名。
          </p>
        )}
      </div>

      <div className="row-inline">
        <label className="field">
          <span>API Key（加密后入库）</span>
          <input
            type="password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder="sk-…"
          />
        </label>
        <button type="button" onClick={handleSave}>
          {editingId ? "保存修改" : "保存渠道"}
        </button>
      </div>

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}

      <h3>已建渠道</h3>
      {channels.length === 0 && <p className="muted">还没有渠道。</p>}
      <ul className="cred-list">
        {channels.map((c) => (
          <li key={c.id}>
            <div className="cred-row">
              <span className="cred-meta">
                <code>{c.provider}</code> · {c.name} · <code>{c.base_url}</code>
                {c.enabled ? "" : " · 已禁用"}
              </span>
              <span className="cred-actions">
                <button type="button" onClick={() => loadForEdit(c)}>
                  编辑
                </button>
                <button type="button" onClick={() => handleDelete(c)}>
                  删除
                </button>
              </span>
            </div>
            <div className="cred-meta">
              模型：
              {c.models.length === 0
                ? "（无）"
                : c.models
                    .map(
                      (m) =>
                        `${m.alias} → ${m.upstream_id}${m.enabled ? "" : "（已禁用）"}`,
                    )
                    .join("；")}
              {" · "}
              {c.has_credential ? "密钥已录入" : "未录密钥"}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

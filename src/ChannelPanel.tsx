import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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

type ModelInput = {
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
  models: ModelInput[];
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
  const [alias, setAlias] = useState("");
  const [upstreamId, setUpstreamId] = useState("");
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
    setAlias((prev) => (prev.trim() ? prev : (first?.alias ?? "")));
    setUpstreamId((prev) => (prev.trim() ? prev : (first?.upstream_id ?? "")));
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
    setAlias("");
    setUpstreamId("");
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
    const first = channel.models[0];
    setAlias(first?.alias ?? "");
    setUpstreamId(first?.upstream_id ?? "");
    setSecret("");
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
    if (!alias.trim() || !upstreamId.trim()) {
      setError("模型别名与上游模型名都要填");
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
      enabled: true,
      priority: 100,
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
      models: [
        {
          alias: alias.trim(),
          upstream_id: upstreamId.trim(),
          enabled: true,
        },
      ],
    };
    try {
      const saved = await invoke<ChannelSummary>("channel_save", { payload });
      if (secret.trim()) {
        await invoke("channel_put_key", {
          channelId: saved.id,
          secret: secret.trim(),
        });
      }
      setSecret("");
      setError(null);
      setMessage(
        `已保存渠道「${saved.name}」，别名 ${alias.trim()} 已出现在 /v1/models` +
          (secret.trim() ? "（密钥已加密入库）" : ""),
      );
      await refresh();
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

      <h3>新增渠道</h3>
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
          {mode === "preset" && selectedPreset && (
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
          <span>模型别名（客户端看到的名字）</span>
          <input
            type="text"
            value={alias}
            onChange={(e) => setAlias(e.target.value)}
            placeholder="deepseek-chat"
          />
        </label>
        <label className="field">
          <span>上游模型名</span>
          <input
            type="text"
            value={upstreamId}
            onChange={(e) => setUpstreamId(e.target.value)}
            placeholder="deepseek-chat"
          />
        </label>
      </div>
      {mode === "preset" && (
        <p className="muted">
          默认模型是「未实测」的建议值（02 §6.1.1），可改成你实际可用的模型名。
        </p>
      )}

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

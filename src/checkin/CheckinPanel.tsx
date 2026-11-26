import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import {
  defaultSiteName,
  entryLine,
  filterEntries,
  isValidVarName,
  sensitiveCount,
  suggestEntryIndex,
  suggestVarName,
  targetKey,
  targetLabel,
} from "./format";
import type {
  ChannelOption,
  FieldView,
  HarImport,
  MarkedVariable,
  SaveTemplateInput,
  SiteSummary,
} from "./types";

/**
 * 签到站点面板（T15；02 §7.1.1 / §7.1.2）。
 *
 * 导入向导四步：**选 HAR 文件 → 选一条 entry → 标记变量 → 保存模板**。
 *
 * 密文边界（AGENT.md §5.3）：
 * - 导入回来的**敏感值已经是掩码**（后端在 IPC 边界就掩掉了），前端拿不到原值；
 * - 保存时前端只回传**路径 + 标记清单**，后端重新解析并把真实值写进保险库；
 * - 未标记的敏感字段由后端**自动补齐为变量**（名字带 `auto_` 前缀），
 *   否则它们会以明文留在模板里。
 */
export default function CheckinPanel() {
  const [sites, setSites] = useState<SiteSummary[]>([]);
  const [channels, setChannels] = useState<ChannelOption[]>([]);
  const [harPath, setHarPath] = useState<string | null>(null);
  const [pathInput, setPathInput] = useState("");
  const [imported, setImported] = useState<HarImport | null>(null);
  const [keyword, setKeyword] = useState("");
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [marks, setMarks] = useState<Record<string, MarkedVariable>>({});
  const [siteName, setSiteName] = useState("");
  const [channelChoice, setChannelChoice] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refreshSites = useCallback(async () => {
    try {
      const rows = await invoke<SiteSummary[]>("checkin_list_sites");
      setSites(rows);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshSites();
    invoke<ChannelOption[]>("channel_list")
      .then((rows) => setChannels(rows))
      .catch(() => setChannels([]));
  }, [refreshSites]);

  const filtered = useMemo(
    () => (imported ? filterEntries(imported.entries, keyword) : []),
    [imported, keyword],
  );

  const selected = useMemo(
    () => imported?.entries.find((entry) => entry.index === selectedIndex) ?? null,
    [imported, selectedIndex],
  );

  const importHar = useCallback(async (path: string) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<HarImport>("checkin_import_har", { path });
      setHarPath(path);
      setImported(result);
      setSiteName(defaultSiteName(result.source));
      setSelectedIndex(suggestEntryIndex(result.entries));
      setMarks({});
      setKeyword("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const pickHar = useCallback(async () => {
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "HAR 文件", extensions: ["har"] }],
      });
      if (typeof picked === "string") {
        await importHar(picked);
      }
    } catch (e) {
      setError(String(e));
    }
  }, [importHar]);

  const toggleMark = useCallback((field: FieldView) => {
    const key = targetKey(field.target);
    setMarks((current) => {
      if (current[key]) {
        const next = { ...current };
        delete next[key];
        return next;
      }
      const name = suggestVarName(field.target);
      return {
        ...current,
        [key]: { name: name || "变量", target: field.target },
      };
    });
  }, []);

  const renameMark = useCallback((key: string, name: string) => {
    setMarks((current) => {
      const existing = current[key];
      if (!existing) {
        return current;
      }
      return { ...current, [key]: { ...existing, name } };
    });
  }, []);

  const save = useCallback(async () => {
    setError(null);
    setNotice(null);
    const name = siteName.trim();
    if (!name) {
      setError("站点名不能为空");
      return;
    }
    if (!harPath || selectedIndex === null) {
      setError("先选一条 entry");
      return;
    }
    const variables = Object.values(marks);
    if (variables.length === 0) {
      setError("至少标记一个变量");
      return;
    }
    const bad = variables.find((variable) => !isValidVarName(variable.name));
    if (bad) {
      setError(`变量名不合法：${bad.name}（只允许字母、数字、下划线、连字符）`);
      return;
    }
    const duplicated = variables
      .map((variable) => variable.name)
      .filter((value, index, all) => all.indexOf(value) !== index);
    if (duplicated.length > 0) {
      setError(`变量名重复：${duplicated.join("、")}`);
      return;
    }
    const input: SaveTemplateInput = {
      path: harPath,
      entryIndex: selectedIndex,
      siteName: name,
      channelId: channelChoice === "" ? null : channelChoice,
      channelName: channelChoice === "" ? name : null,
      variables,
    };
    setBusy(true);
    try {
      const summary = await invoke<SiteSummary>("checkin_save_template", { input });
      setNotice(
        `已保存「${summary.name}」：${summary.template?.variableCount ?? 0} 个变量` +
          "（敏感取值已进加密保险库，模板里只留占位符）",
      );
      setMarks({});
      await refreshSites();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [siteName, harPath, selectedIndex, marks, channelChoice, refreshSites]);

  const removeSite = useCallback(
    async (id: string) => {
      setError(null);
      try {
        await invoke<boolean>("checkin_delete_site", { id });
        await refreshSites();
      } catch (e) {
        setError(String(e));
      }
    },
    [refreshSites],
  );

  const unmarkedSensitive = useMemo(() => {
    if (!selected) {
      return 0;
    }
    return selected.fields.filter(
      (field) => field.sensitive && !marks[targetKey(field.target)],
    ).length;
  }, [selected, marks]);

  return (
    <section className="card">
      <h2>签到站点（T15：HAR 导入与模板化）</h2>
      <p className="muted">
        浏览器 F12 录制一次签到请求 → 导出 HAR → 在这里导入 → 选一条请求 →
        标记变量 → 保存成站点模板。模板里只留 <code>{"{{变量名}}"}</code>{" "}
        占位符，敏感取值（cookie / csrf / token）进加密保险库（02 §7.1.2）。
      </p>

      {error && <p className="error">{error}</p>}
      {notice && <p className="ok">{notice}</p>}

      {sites.length > 0 && (
        <table className="usage-table">
          <thead>
            <tr>
              <th>站点</th>
              <th>插件</th>
              <th>请求</th>
              <th>变量</th>
              <th>状态</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {sites.map((site) => (
              <tr key={site.id}>
                <td>{site.name}</td>
                <td>{site.plugin}</td>
                <td>
                  {site.template ? (
                    <span className="checkin-url">
                      {site.template.method} {site.template.url}
                    </span>
                  ) : (
                    "—"
                  )}
                </td>
                <td>
                  {site.template
                    ? site.template.variableNames.join("、")
                    : "—"}
                </td>
                <td>{site.enabled ? "已启用" : "默认关"}</td>
                <td>
                  <button onClick={() => void removeSite(site.id)}>删除</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>导入向导</h3>
      <div className="row-inline">
        <button onClick={() => void pickHar()} disabled={busy}>
          导入 HAR 文件
        </button>
        {harPath && <span className="muted">{harPath}</span>}
      </div>
      <div className="row-inline">
        <input
          type="text"
          value={pathInput}
          placeholder="或直接粘贴 HAR 文件路径"
          onChange={(e) => setPathInput(e.target.value)}
        />
        <button
          onClick={() => {
            const path = pathInput.trim();
            if (path) {
              void importHar(path);
            }
          }}
          disabled={busy || pathInput.trim() === ""}
        >
          按路径导入
        </button>
      </div>

      {imported && (
        <>
          <p className="muted">
            已解析 <code>{imported.source}</code>：共 {imported.entryCount}{" "}
            条 entry
          </p>

          <div className="field">
            <label htmlFor="checkin-keyword">搜索 entry（方法 / URL / 状态码）</label>
            <input
              id="checkin-keyword"
              type="text"
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              placeholder="例如 login/info"
            />
          </div>

          <table className="usage-table">
            <thead>
              <tr>
                <th>#</th>
                <th>方法</th>
                <th>URL</th>
                <th>状态</th>
                <th>敏感字段</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {filtered.slice(0, 50).map((entry) => (
                <tr key={entry.index}>
                  <td>{entry.index}</td>
                  <td>{entry.method}</td>
                  <td>
                    <span className="checkin-url">{entry.url}</span>
                  </td>
                  <td>{entry.status}</td>
                  <td>{sensitiveCount(entry.fields)}</td>
                  <td>
                    <button
                      className={entry.index === selectedIndex ? "active" : ""}
                      onClick={() => {
                        setSelectedIndex(entry.index);
                        setMarks({});
                      }}
                    >
                      选它
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {filtered.length > 50 && (
            <p className="muted">
              共 {filtered.length} 条，只显示前 50 条——用上面的搜索框缩小范围。
            </p>
          )}

          {selected && (
            <>
              <h3>
                选中的请求：{entryLine(selected)}
              </h3>
              <p className="muted">
                标成变量的字段，模板里会换成占位符；敏感取值会写进保险库。
                {unmarkedSensitive > 0 && (
                  <>
                    {" "}
                    还有 <strong>{unmarkedSensitive}</strong>{" "}
                    个敏感字段未标——保存时会自动补成 <code>auto_*</code>{" "}
                    变量（否则它们会以明文留在模板里）。
                  </>
                )}
              </p>
              <table className="usage-table">
                <thead>
                  <tr>
                    <th>位置</th>
                    <th>值</th>
                    <th>变量名</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {selected.fields.map((field) => {
                    const key = targetKey(field.target);
                    const marked = marks[key];
                    return (
                      <tr key={key}>
                        <td>
                          {targetLabel(field.target)}
                          {field.sensitive && (
                            <span className="muted"> （敏感）</span>
                          )}
                        </td>
                        <td>
                          <span className="checkin-value">
                            <code>{field.value}</code>
                          </span>
                        </td>
                        <td>
                          {marked ? (
                            <input
                              type="text"
                              value={marked.name}
                              onChange={(e) =>
                                renameMark(key, e.target.value)
                              }
                            />
                          ) : (
                            <span className="muted">—</span>
                          )}
                        </td>
                        <td>
                          <button onClick={() => toggleMark(field)}>
                            {marked ? "取消标记" : "标记为变量"}
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>

              <div className="field">
                <label htmlFor="checkin-site-name">站点名</label>
                <input
                  id="checkin-site-name"
                  type="text"
                  value={siteName}
                  onChange={(e) => setSiteName(e.target.value)}
                />
              </div>

              <div className="field">
                <label htmlFor="checkin-channel">归属渠道（变量的密文挂在这里）</label>
                <select
                  id="checkin-channel"
                  value={channelChoice}
                  onChange={(e) => setChannelChoice(e.target.value)}
                >
                  <option value="">按站点名新建一个渠道</option>
                  {channels.map((channel) => (
                    <option key={channel.id} value={channel.id}>
                      {channel.name}
                    </option>
                  ))}
                </select>
              </div>

              <button onClick={() => void save()} disabled={busy}>
                保存模板
              </button>
            </>
          )}
        </>
      )}
    </section>
  );
}

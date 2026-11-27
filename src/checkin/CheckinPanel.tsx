import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import {
  defaultSiteName,
  entryLine,
  filterEntries,
  isValidVarName,
  sensitiveCount,
  shortUrl,
  suggestEntryIndex,
  suggestVarName,
  targetKey,
  targetLabel,
} from "./format";
import {
  credentialState,
  describeCredentialState,
  describeReward,
  pluginLabel,
} from "./plugin";
import {
  describeCron,
  describeSystemTask,
  formatClock,
  formatRelative,
  validateCron,
} from "./schedule";
import {
  describeNotifyState,
  needsChatId,
  webhookPlaceholder,
  webhookUrlToSend,
  WEBHOOK_KINDS,
} from "./notify";
import type {
  BuiltinSite,
  ChannelOption,
  FieldView,
  HarImport,
  LoginSummary,
  MarkedVariable,
  NotifyChannelResult,
  NotifyConfig,
  NotifyConfigInput,
  ResultView,
  RunSummary,
  SaveTemplateInput,
  SidecarHealth,
  SiteSummary,
  SystemSyncSummary,
} from "./types";

/**
 * 签到站点面板（T15 HAR 导入与模板化 · T17 调度器与系统兜底 · T18 站点插件）。
 *
 * 三块：
 * 1. **内置站点插件**（T18）：ModelScope / MiniMax Code——**自带正确的断言**，
 *    用户只提供登录态；MiniMax Code 走**浏览器辅轨**（弹一次浏览器登录）；
 * 2. **站点调度**（T17）：给站点设 cron / 启用开关 / 立即执行一次 /
 *    同步系统兜底任务 + 看最近执行记录；
 * 3. **导入向导**（T15）：**选 HAR 文件 → 选一条 entry → 标记变量 → 保存模板**。
 *
 * 密文边界（AGENT.md §5.3）：
 * - 导入回来的**敏感值已经是掩码**（后端在 IPC 边界就掩掉了），前端拿不到原值；
 * - 保存时前端只回传**路径 + 标记清单**，后端重新解析并把真实值写进保险库；
 * - 未标记的敏感字段由后端**自动补齐为变量**（名字带 `auto_` 前缀），
 *   否则它们会以明文留在模板里；
 * - 内置站点的登录态同样**只进保险库**，不回显、不进日志。
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
  // ── T17 调度 ──
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [results, setResults] = useState<ResultView[]>([]);
  const [systemSummary, setSystemSummary] = useState<SystemSyncSummary | null>(null);
  const [busySite, setBusySite] = useState<string | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  // ── T18 站点插件 ──
  const [builtins, setBuiltins] = useState<BuiltinSite[]>([]);
  /** 登录态输入框的草稿（`siteId` → 明文；**只活在内存里**，提交后清空）。 */
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string>>({});
  const [sidecarHealth, setSidecarHealth] = useState<SidecarHealth | null>(null);
  // ── T19 通知 ──
  const [notifyConfig, setNotifyConfig] = useState<NotifyConfig | null>(null);
  /** webhook 地址草稿（**只活在内存里**；保存后清空，地址永不回显）。 */
  const [webhookDraft, setWebhookDraft] = useState("");
  const [chatIdDraft, setChatIdDraft] = useState("");
  /** 「清除 webhook」的待办（点「清除」才置位）。 */
  const [clearWebhook, setClearWebhook] = useState(false);

  const refreshNotify = useCallback(async () => {
    try {
      const config = await invoke<NotifyConfig>("notify_get_config");
      setNotifyConfig(config);
      setChatIdDraft(config.telegramChatId ?? "");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const saveNotify = useCallback(async () => {
    if (!notifyConfig) {
      return;
    }
    setBusy(true);
    try {
      const input: NotifyConfigInput = {
        enabled: notifyConfig.enabled,
        systemEnabled: notifyConfig.systemEnabled,
        notifySuccess: notifyConfig.notifySuccess,
        webhookKind: notifyConfig.webhookKind || null,
        // 空输入 = 不动；点过「清除」= 传空串
        webhookUrl: clearWebhook ? "" : webhookUrlToSend(webhookDraft),
        telegramChatId: chatIdDraft.trim() || null,
      };
      const saved = await invoke<NotifyConfig>("notify_set_config", { input });
      setNotifyConfig(saved);
      setWebhookDraft("");
      setClearWebhook(false);
      setNotice("通知配置已保存。");
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [notifyConfig, webhookDraft, chatIdDraft, clearWebhook]);

  const testNotify = useCallback(async () => {
    setBusy(true);
    try {
      const results = await invoke<NotifyChannelResult[]>("notify_test");
      const summary = results
        .map((item) => `${item.channel}：${item.ok ? "已发送" : item.detail}`)
        .join("；");
      setNotice(summary || "没有可用的通知通道。");
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const refreshSites = useCallback(async () => {
    try {
      const rows = await invoke<SiteSummary[]>("checkin_list_sites");
      setSites(rows);
      // 草稿跟库里的一致（用户正在改的除外——草稿存在 drafts 里，改完才写回）
      setDrafts((current) => {
        const next: Record<string, string> = {};
        for (const row of rows) {
          next[row.id] = current[row.id] ?? row.schedule ?? "";
        }
        return next;
      });
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const refreshResults = useCallback(async () => {
    try {
      const rows = await invoke<ResultView[]>("checkin_results", {
        id: null,
        limit: 20,
      });
      setResults(rows);
    } catch {
      setResults([]);
    }
  }, []);

  useEffect(() => {
    void refreshSites();
    void refreshResults();
    invoke<ChannelOption[]>("channel_list")
      .then((rows) => setChannels(rows))
      .catch(() => setChannels([]));
    invoke<BuiltinSite[]>("checkin_builtin_sites")
      .then((rows) => setBuiltins(rows))
      .catch(() => setBuiltins([]));
    invoke<SidecarHealth>("checkin_sidecar_health")
      .then((row) => setSidecarHealth(row))
      .catch(() => setSidecarHealth(null));
    void refreshNotify();
  }, [refreshSites, refreshResults, refreshNotify]);

  // 相对时间要自己走（不然「下次：3 分钟后」会一直停在旧值）
  useEffect(() => {
    const timer = window.setInterval(
      () => setNow(Math.floor(Date.now() / 1000)),
      30_000,
    );
    return () => window.clearInterval(timer);
  }, []);

  const filtered = useMemo(
    () => (imported ? filterEntries(imported.entries, keyword) : []),
    [imported, keyword],
  );

  const selected = useMemo(
    () => imported?.entries.find((entry) => entry.index === selectedIndex) ?? null,
    [imported, selectedIndex],
  );

  const siteNameOf = useMemo(() => {
    const map = new Map<string, string>();
    for (const site of sites) {
      map.set(site.id, site.name);
    }
    return map;
  }, [sites]);

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
        await refreshResults();
      } catch (e) {
        setError(String(e));
      }
    },
    [refreshSites, refreshResults],
  );

  /** 合并后端返回的站点摘要（保留别的站点的行）。 */
  const applySummary = useCallback((summary: SiteSummary) => {
    setSites((current) => {
      const index = current.findIndex((site) => site.id === summary.id);
      if (index < 0) {
        return [...current, summary];
      }
      const next = [...current];
      next[index] = summary;
      return next;
    });
  }, []);

  /** 保存 cron / 启用开关（两者一起提交，后端据此同步系统兜底任务）。 */
  const saveSchedule = useCallback(
    async (site: SiteSummary, enabled: boolean) => {
      setError(null);
      setNotice(null);
      const draft = (drafts[site.id] ?? "").trim();
      const problem = validateCron(draft);
      if (problem) {
        setError(problem);
        return;
      }
      setBusySite(site.id);
      try {
        const summary = await invoke<SiteSummary>("checkin_set_schedule", {
          id: site.id,
          schedule: draft === "" ? null : draft,
          enabled,
        });
        applySummary(summary);
        setDrafts((current) => ({
          ...current,
          [site.id]: summary.schedule ?? "",
        }));
        if (summary.systemTaskError) {
          setError(`系统兜底任务没注册上：${summary.systemTaskError}`);
        } else {
          setNotice(
            enabled && summary.schedule
              ? `「${summary.name}」已按 ${describeCron(summary.schedule)} 调度` +
                (summary.systemTaskRegistered
                  ? "（系统兜底任务已注册，关掉应用也能签到）"
                  : "（系统兜底任务没注册上——见下方提示）")
              : `「${summary.name}」已停用`,
          );
        }
      } catch (e) {
        setError(String(e));
      } finally {
        setBusySite(null);
      }
    },
    [drafts, applySummary],
  );

  /** 立即执行一次（忽略「本周期已跑过」的去重）。 */
  const runNow = useCallback(
    async (site: SiteSummary) => {
      setError(null);
      setNotice(null);
      setBusySite(site.id);
      try {
        const summary = await invoke<RunSummary>("checkin_run_now", { id: site.id });
        setNotice(
          `「${summary.siteName}」${summary.ok ? "签到成功" : "签到失败"}` +
            `（${summary.class}）：${summary.message}` +
            `　耗时 ${summary.elapsedMs} ms`,
        );
        await refreshSites();
        await refreshResults();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusySite(null);
      }
    },
    [refreshSites, refreshResults],
  );

  /** 同步系统兜底任务（按当前站点状态全量对齐）。 */
  const syncSystem = useCallback(async () => {
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const summary = await invoke<SystemSyncSummary>("checkin_system_sync");
      setSystemSummary(summary);
      if (summary.errors.length > 0) {
        setError(`系统兜底同步有 ${summary.errors.length} 处失败：${summary.errors.join("；")}`);
      } else {
        setNotice(
          `系统兜底已同步：注册 ${summary.registered.length} 个、注销 ${summary.unregistered.length} 个`,
        );
      }
      await refreshSites();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [refreshSites]);

  const unmarkedSensitive = useMemo(() => {
    if (!selected) {
      return 0;
    }
    return selected.fields.filter(
      (field) => field.sensitive && !marks[targetKey(field.target)],
    ).length;
  }, [selected, marks]);

  // ── T18 站点插件 ──

  /** 添加一个内置站点（默认不启用——02 §7.3）。 */
  const addBuiltin = useCallback(
    async (pluginId: string) => {
      setError(null);
      setNotice(null);
      setBusy(true);
      try {
        const summary = await invoke<SiteSummary>("checkin_add_builtin_site", {
          pluginId,
        });
        setNotice(
          `已添加「${summary.name}」：${summary.credentialHint ?? ""}` +
            "（默认不启用——填好登录态后再勾启用）",
        );
        await refreshSites();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [refreshSites],
  );

  /** 保存登录态（**明文只往后台送一次**，提交后清空输入框）。 */
  const saveCredential = useCallback(
    async (site: SiteSummary, which: "cookie" | "token") => {
      setError(null);
      setNotice(null);
      const draft = (secretDrafts[site.id] ?? "").trim();
      if (!draft) {
        setError(which === "cookie" ? "登录态不能为空" : "token 不能为空");
        return;
      }
      setBusySite(site.id);
      try {
        const summary = await invoke<SiteSummary>("checkin_set_credential", {
          id: site.id,
          which,
          secret: draft,
        });
        applySummary(summary);
        setSecretDrafts((current) => ({ ...current, [site.id]: "" }));
        setNotice(
          `「${summary.name}」的${which === "cookie" ? "网页 Cookie" : "开放平台 token"}已存进加密保险库`,
        );
      } catch (e) {
        setError(String(e));
      } finally {
        setBusySite(null);
      }
    },
    [secretDrafts, applySummary],
  );

  /** 拉起浏览器做登录交接（MiniMax Code：用户在浏览器里登录一次）。 */
  const pluginLogin = useCallback(
    async (site: SiteSummary) => {
      setError(null);
      setNotice(null);
      setBusySite(site.id);
      try {
        const summary = await invoke<LoginSummary>("checkin_plugin_login", { id: site.id });
        setNotice(summary.message);
      } catch (e) {
        setError(String(e));
      } finally {
        setBusySite(null);
      }
    },
    [],
  );

  /** 已建的内置站点（`plugin !== "har"`）。 */
  const builtinSites = useMemo(
    () => sites.filter((site) => site.plugin !== "har"),
    [sites],
  );

  return (
    <section className="card">
      <h2>签到站点（T15 模板化 · T17 调度 · T18 站点插件）</h2>
      <p className="muted">
        内置站点插件自带<b>正确的断言</b>，用户只要提供登录态；
        上半部分是调度（cron / 启用 / 立即执行 / 系统兜底），下半部分是 HAR 导入向导。
        调度口径见 02 §7.2.1：cron 写<b>经典 5 字段</b>（分 时 日 月 周），
        按本地时区解释；应用关着期间漏掉的周期会自动补签。
      </p>

      {error && <p className="error">{error}</p>}
      {notice && <p className="ok">{notice}</p>}

      <h3>内置站点插件（T18）</h3>
      {sidecarHealth && (
        <p className="muted">
          浏览器辅轨：{sidecarHealth.browserAvailable ? "可用" : "不可用"}
          {sidecarHealth.browserAvailable &&
            `（${sidecarHealth.browserChannel}，Node ${sidecarHealth.nodeVersion}）`}
          　profile：{sidecarHealth.profileDir}
        </p>
      )}
      {builtins.length === 0 ? (
        <p className="muted">后端没返回内置插件清单。</p>
      ) : (
        <table className="usage-table">
          <thead>
            <tr>
              <th>插件</th>
              <th>要准备什么</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {builtins.map((item) => {
              const added = builtinSites.some((site) => site.plugin === item.id);
              return (
                <tr key={item.id}>
                  <td>
                    <strong>{item.name}</strong>
                    <div className="muted">默认 cron：{item.defaultSchedule}</div>
                  </td>
                  <td>
                    <div>{item.credentialHint}</div>
                    {item.needsSidecar && (
                      <div className="muted">
                        需要 Node 运行时 + sidecar 目录（开发态：cd sidecar && npm install）
                      </div>
                    )}
                  </td>
                  <td>
                    <button onClick={() => void addBuiltin(item.id)} disabled={busy}>
                      {added ? "已添加（可再点一次）" : "添加"}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <h3>站点调度</h3>
      {sites.length === 0 ? (
        <p className="muted">
          还没有签到站点——先在上面添加一个内置站点，或用下面的导入向导建一个。
        </p>
      ) : (
        <table className="usage-table">
          <thead>
            <tr>
              <th>站点</th>
              <th>登录态</th>
              <th>调度（cron）</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {sites.map((site) => {
              const draft = drafts[site.id] ?? site.schedule ?? "";
              const pending = busySite === site.id;
              const credState = credentialState(site);
              return (
                <tr key={site.id}>
                  <td>
                    <strong>{site.name}</strong>
                    <div className="muted">
                      {site.template
                        ? `${site.template.method} ${shortUrl(site.template.url, 56)}`
                        : `插件：${pluginLabel(site.plugin)}`}
                    </div>
                    <div className="muted">
                      {site.template
                        ? `变量：${site.template.variableNames.join("、") || "—"}`
                        : site.credentialHint ?? "—"}
                    </div>
                    {site.reward && (
                      <div className="muted">积分：{describeReward(site.reward)}</div>
                    )}
                  </td>
                  <td>
                    <div className="muted">{describeCredentialState(credState)}</div>
                    {credState === "sidecar" && (
                      <button
                        onClick={() => void pluginLogin(site)}
                        disabled={pending}
                      >
                        登录（弹浏览器）
                      </button>
                    )}
                    {credState !== "har" && credState !== "sidecar" && (
                      <>
                        <div className="row-inline">
                          <input
                            type="password"
                            value={secretDrafts[site.id] ?? ""}
                            placeholder="访问令牌 ms-…（或网页会话 Cookie）"
                            onChange={(e) =>
                              setSecretDrafts((current) => ({
                                ...current,
                                [site.id]: e.target.value,
                              }))
                            }
                          />
                          <button
                            onClick={() => void saveCredential(site, "cookie")}
                            disabled={pending}
                          >
                            保存
                          </button>
                        </div>
                        <div className="muted">
                          {site.credentialReady
                            ? "登录态已填（存加密保险库）——可以再填一次覆盖"
                            : "还没填：魔搭「个人中心 → 访问令牌 → 新建 → 复制」，把 ms- 开头的令牌粘进来（不用 F12）"}
                        </div>
                      </>
                    )}
                  </td>
                  <td>
                    <div className="row-inline">
                      <input
                        type="text"
                        value={draft}
                        placeholder="分 时 日 月 周，例如 0 9 * * *"
                        onChange={(e) =>
                          setDrafts((current) => ({
                            ...current,
                            [site.id]: e.target.value,
                          }))
                        }
                      />
                      <button
                        onClick={() => void saveSchedule(site, site.enabled)}
                        disabled={pending}
                      >
                        保存
                      </button>
                    </div>
                    <div className="muted">{describeCron(draft)}</div>
                    <div className="muted">
                      {describeSystemTask(
                        site.systemTaskRegistered,
                        site.systemTaskFrequency,
                      )}
                    </div>
                    {site.systemTaskError && (
                      <div className="error">{site.systemTaskError}</div>
                    )}
                  </td>
                  <td>
                    <label className="checkbox">
                      <input
                        type="checkbox"
                        checked={site.enabled}
                        disabled={pending}
                        onChange={(e) =>
                          void saveSchedule(site, e.target.checked)
                        }
                      />
                      启用
                    </label>
                    <div className="muted">
                      下次：{formatRelative(site.nextRunAt, now)}
                    </div>
                    <div className="muted">
                      最近：
                      {site.lastAttemptAt === null
                        ? "—"
                        : `${formatClock(site.lastAttemptAt)}（${
                            site.lastOk ? "成功" : "失败"
                          }）`}
                    </div>
                  </td>
                  <td>
                    <button
                      onClick={() => void runNow(site)}
                      disabled={pending}
                    >
                      立即执行
                    </button>
                    <button onClick={() => void removeSite(site.id)}>删除</button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <div className="row-inline">
        <button onClick={() => void syncSystem()} disabled={busy}>
          同步系统兜底任务
        </button>
        <span className="muted">
          应用关着也能签到：每个「启用 + 有 cron」的站点注册一个系统计划任务
          （Windows 计划任务 / macOS launchd），触发时走同一套「该跑没跑」判定。
        </span>
      </div>
      {systemSummary && (
        <p className="muted">
          上次同步：注册 {systemSummary.registered.length} 个
          {systemSummary.registered.map((name) => `（${name}）`).join("")} ·
          注销 {systemSummary.unregistered.length} 个
          {systemSummary.errors.length > 0 &&
            ` · 出错 ${systemSummary.errors.length} 处：${systemSummary.errors.join("；")}`}
        </p>
      )}

      {results.length > 0 && (
        <>
          <h3>最近执行记录</h3>
          <table className="usage-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>站点</th>
                <th>结果</th>
                <th>说明</th>
              </tr>
            </thead>
            <tbody>
              {results.map((row) => (
                <tr key={row.id}>
                  <td>{formatClock(row.ts)}</td>
                  <td>{siteNameOf.get(row.siteId) ?? row.siteId}</td>
                  <td className={row.ok ? "ok" : "error"}>
                    {row.ok ? "成功" : "失败"}
                  </td>
                  <td>{row.message ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}

      <h3>结果通知（T19）</h3>
      {notifyConfig === null ? (
        <p className="muted">通知配置没读出来。</p>
      ) : (
        <>
          <p className="muted">
            每次签到（<b>应用开着、关着都一样</b>——计划任务那条路也走同一段代码）
            都会发结果通知：<b>失败永远通知</b>（含站点名与原因），成功可关。
            webhook 地址与 token <b>只进加密保险库</b>——不落库、不回显、不进日志。
          </p>
          <div className="muted">
            {describeNotifyState(
              notifyConfig.enabled,
              notifyConfig.systemEnabled,
              notifyConfig.notifySuccess,
              notifyConfig.webhookConfigured,
            )}
          </div>
          <div className="row-inline">
            <label className="checkbox">
              <input
                type="checkbox"
                checked={notifyConfig.enabled}
                disabled={busy}
                onChange={(e) =>
                  setNotifyConfig({ ...notifyConfig, enabled: e.target.checked })
                }
              />
              开启通知
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={notifyConfig.systemEnabled}
                disabled={busy}
                onChange={(e) =>
                  setNotifyConfig({ ...notifyConfig, systemEnabled: e.target.checked })
                }
              />
              系统通知（弹窗）
            </label>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={notifyConfig.notifySuccess}
                disabled={busy}
                onChange={(e) =>
                  setNotifyConfig({ ...notifyConfig, notifySuccess: e.target.checked })
                }
              />
              成功也通知
            </label>
          </div>
          <div className="row-inline">
            <label>
              webhook 类型
              <select
                value={notifyConfig.webhookKind ?? ""}
                disabled={busy}
                onChange={(e) =>
                  setNotifyConfig({
                    ...notifyConfig,
                    webhookKind: e.target.value || null,
                  })
                }
              >
                {WEBHOOK_KINDS.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}
                  </option>
                ))}
              </select>
            </label>
            <div className="muted">
              {WEBHOOK_KINDS.find(
                (item) => item.value === (notifyConfig.webhookKind ?? ""),
              )?.hint ?? ""}
            </div>
          </div>
          {(notifyConfig.webhookKind ?? "") !== "" && (
            <div className="row-inline">
              <label>
                webhook 地址
                <input
                  type="password"
                  value={clearWebhook ? "" : webhookDraft}
                  placeholder={webhookPlaceholder(notifyConfig.webhookKind)}
                  disabled={busy || clearWebhook}
                  onChange={(e) => {
                    setWebhookDraft(e.target.value);
                    setClearWebhook(false);
                  }}
                />
              </label>
              <div className="muted">
                {clearWebhook
                  ? "保存后会清除已配置的地址。"
                  : notifyConfig.webhookConfigured
                    ? "已配置（地址不回显；留空 = 保持不变）。"
                    : "还没配置。"}
              </div>
              {notifyConfig.webhookConfigured && !clearWebhook && (
                <button onClick={() => setClearWebhook(true)} disabled={busy}>
                  清除
                </button>
              )}
            </div>
          )}
          {needsChatId(notifyConfig.webhookKind) && (
            <div className="row-inline">
              <label>
                Telegram chat_id
                <input
                  type="text"
                  value={chatIdDraft}
                  placeholder="-1001234567890"
                  disabled={busy}
                  onChange={(e) => setChatIdDraft(e.target.value)}
                />
              </label>
            </div>
          )}
          <div className="row-inline">
            <button onClick={() => void saveNotify()} disabled={busy}>
              保存
            </button>
            <button onClick={() => void testNotify()} disabled={busy}>
              测试通知
            </button>
          </div>
        </>
      )}

      <h3>导入向导</h3>
      <p className="muted">
        浏览器 F12 录制一次签到请求 → 导出 HAR → 在这里导入 → 选一条请求 →
        标记变量 → 保存成站点模板。模板里只留 <code>{"{{变量名}}"}</code>{" "}
        占位符，敏感取值（cookie / csrf / token）进加密保险库（02 §7.1.2）。
      </p>

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

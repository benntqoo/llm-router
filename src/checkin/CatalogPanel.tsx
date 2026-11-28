import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  describeSpec,
  emptyForm,
  formToSpec,
  parseSpecJson,
  validateSpec,
  type SpecForm,
} from "./spec";
import type {
  BalanceSummary,
  CatalogEntry,
  CatalogImportSummary,
  SiteSummary,
  SpecSite,
} from "./types";

/**
 * 站点目录（T25；02 §7.6）。
 *
 * 三件事：
 * 1. **列目录**：内置目录只收**已实测**的条目（ModelScope / MiniMax Code 走代码插件）
 *    + 一个**空白模板**（通用 HTTP 签到，填 URL 即可接一家新平台）；
 * 2. **加站点**：声明式定义（表单 or 直接粘 JSON）→ **不用改代码、不用发版**；
 * 3. **导入/导出目录**：目录文件可分享给别人（**导出不含凭据明文**）。
 *
 * 密文边界（AGENT.md §5.3）：登录态只提交给后端进保险库，**前端不回显、不留副本**。
 */
export default function CatalogPanel({
  sites,
  onChanged,
}: {
  sites: SiteSummary[];
  onChanged?: () => void;
}) {
  const [entries, setEntries] = useState<CatalogEntry[]>([]);
  const [form, setForm] = useState<SpecForm>(emptyForm());
  const [jsonDraft, setJsonDraft] = useState("");
  const [importDraft, setImportDraft] = useState("");
  const [exported, setExported] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [balances, setBalances] = useState<Record<string, BalanceSummary>>({});

  const refresh = useCallback(async () => {
    try {
      const rows = await invoke<CatalogEntry[]>("checkin_catalog_list");
      setEntries(rows);
    } catch (err) {
      setError(`读取站点目录失败：${String(err)}`);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const specSites = sites.filter((site) => site.plugin === "spec");

  const submit = async () => {
    setError(null);
    setNotice(null);

    // 有 JSON 就以 JSON 为准（**更完整的入口**），否则用表单
    let spec: SpecSite | undefined;
    if (jsonDraft.trim()) {
      const parsed = parseSpecJson(jsonDraft);
      if (parsed.error || !parsed.spec) {
        setError(`声明式定义有问题：${parsed.error ?? "解析失败"}`);
        return;
      }
      spec = parsed.spec;
    } else {
      spec = formToSpec(form);
    }

    const problem = validateSpec(spec);
    if (problem) {
      setError(`声明式定义有问题：${problem}`);
      return;
    }

    setBusy(true);
    try {
      await invoke<SiteSummary>("checkin_add_spec_site", {
        spec,
        secret: form.secret.trim() ? form.secret : null,
      });
      setNotice(`已保存站点「${spec.name}」——默认不启用，确认无误后到「站点调度」勾上启用并设 cron。`);
      setForm(emptyForm());
      setJsonDraft("");
      onChanged?.();
    } catch (err) {
      setError(`保存失败：${String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const importCatalog = async () => {
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const result = await invoke<CatalogImportSummary>("checkin_import_catalog", {
        content: importDraft,
      });
      const failed = result.errors.length;
      setNotice(
        `导入完成：成功 ${result.imported.length} 个` +
          (failed > 0 ? `，失败 ${failed} 个（${result.errors
            .map((item) => `${item.name || item.id}：${item.reason}`)
            .join("；")}）` : ""),
      );
      onChanged?.();
    } catch (err) {
      setError(`导入失败：${String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const exportCatalog = async () => {
    setError(null);
    setNotice(null);
    try {
      const text = await invoke<string>("checkin_export_catalog");
      setExported(text);
      setNotice("已导出当前自建站点（不含凭据明文，只带凭据 id）。");
    } catch (err) {
      setError(`导出失败：${String(err)}`);
    }
  };

  const checkBalance = async (site: SiteSummary) => {
    setError(null);
    try {
      const balance = await invoke<BalanceSummary>("checkin_balance", { id: site.id });
      setBalances((prev) => ({ ...prev, [site.id]: balance }));
    } catch (err) {
      setError(`查余额失败（${site.name}）：${String(err)}`);
    }
  };

  const useTemplate = (entry: CatalogEntry) => {
    const spec = entry.spec;
    if (!spec) {
      return;
    }
    setJsonDraft(JSON.stringify(spec, null, 2));
    setForm(emptyForm());
    setNotice(`已把模板「${entry.name}」填进下面的 JSON 框，改完点「保存站点」即可。`);
  };

  return (
    <>
      <h3>站点目录（T25 · 声明式站点）</h3>
      <p className="muted">
        <b>接一家新的签到平台不用改代码、不用发版</b>：填一份声明式定义（URL + 断言 + 积分字段路径）即可。
        占位符只有两个：<code>{"{{token}}"}</code>（登录态，只进加密保险库）与{" "}
        <code>{"{{today}}"}</code>（当天日期）。
        口径见 02 §7.6。
      </p>

      {error && <p className="error">{error}</p>}
      {notice && <p className="ok">{notice}</p>}

      <table className="usage-table">
        <thead>
          <tr>
            <th>目录条目</th>
            <th>类型</th>
            <th>说明</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => (
            <tr key={entry.id}>
              <td>
                <strong>{entry.name}</strong>
                <div className="muted">默认 cron：{entry.defaultSchedule}</div>
              </td>
              <td>{entry.kind === "builtin" ? "内置插件" : "声明式模板"}</td>
              <td>
                <div>{entry.summary}</div>
                <div className="muted">要准备：{entry.credentialHint}</div>
              </td>
              <td>
                {entry.kind === "template" ? (
                  <button onClick={() => useTemplate(entry)} disabled={busy}>
                    用这个模板
                  </button>
                ) : (
                  <span className="muted">在上面的「内置站点插件」里添加</span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h3>新增声明式站点</h3>
      <div className="field">
        <label>完整定义（JSON，可选——填了就以它为准）</label>
        <textarea
          rows={8}
          value={jsonDraft}
          placeholder='{"id":"my-platform","name":"我的平台","method":"POST","url":"https://example.com/api/checkin","headers":[{"name":"Authorization","value":"Bearer {{token}}"}],"assert":{"status":200},"reward":{"path":"data.credits","unit":"point"}}'
          onChange={(event) => setJsonDraft(event.target.value)}
        />
      </div>
      <div className="row-inline">
        <div className="field">
          <label>站点 id（小写字母/数字/-/_）</label>
          <input
            type="text"
            value={form.id}
            placeholder="my-platform"
            onChange={(event) => setForm({ ...form, id: event.target.value })}
          />
        </div>
        <div className="field">
          <label>站点名</label>
          <input
            type="text"
            value={form.name}
            placeholder="我的平台"
            onChange={(event) => setForm({ ...form, name: event.target.value })}
          />
        </div>
        <div className="field">
          <label>方法</label>
          <select
            value={form.method}
            onChange={(event) => setForm({ ...form, method: event.target.value })}
          >
            <option value="GET">GET</option>
            <option value="POST">POST</option>
          </select>
        </div>
      </div>
      <div className="field">
        <label>签到 URL（可用 {"{{token}}"} 与 {"{{today}}"}）</label>
        <input
          type="text"
          value={form.url}
          placeholder="https://example.com/api/checkin"
          onChange={(event) => setForm({ ...form, url: event.target.value })}
        />
      </div>
      <div className="field">
        <label>登录态（只进加密保险库，不回显）</label>
        <input
          type="password"
          value={form.secret}
          placeholder="token / Cookie"
          onChange={(event) => setForm({ ...form, secret: event.target.value })}
        />
      </div>
      <div className="row-inline">
        <div className="field">
          <label>期望状态码</label>
          <input
            type="text"
            value={form.assertStatus}
            onChange={(event) => setForm({ ...form, assertStatus: event.target.value })}
          />
        </div>
        <div className="field">
          <label>JSON 路径断言（每行一条：路径 或 路径=期望值）</label>
          <input
            type="text"
            value={form.jsonPaths}
            placeholder="data.ok=true"
            onChange={(event) => setForm({ ...form, jsonPaths: event.target.value })}
          />
        </div>
      </div>
      <div className="row-inline">
        <div className="field">
          <label>「本次拿到」字段路径</label>
          <input
            type="text"
            value={form.rewardPath}
            placeholder="data.credits"
            onChange={(event) => setForm({ ...form, rewardPath: event.target.value })}
          />
        </div>
        <div className="field">
          <label>单位</label>
          <input
            type="text"
            value={form.rewardUnit}
            onChange={(event) => setForm({ ...form, rewardUnit: event.target.value })}
          />
        </div>
      </div>
      <div className="row-inline">
        <div className="field">
          <label>「当前余额」查询 URL（可留空）</label>
          <input
            type="text"
            value={form.balanceUrl}
            placeholder="https://example.com/api/balance"
            onChange={(event) => setForm({ ...form, balanceUrl: event.target.value })}
          />
        </div>
        <div className="field">
          <label>余额字段路径</label>
          <input
            type="text"
            value={form.balancePath}
            placeholder="data.balance"
            onChange={(event) => setForm({ ...form, balancePath: event.target.value })}
          />
        </div>
        <div className="field">
          <label>余额单位</label>
          <input
            type="text"
            value={form.balanceUnit}
            onChange={(event) => setForm({ ...form, balanceUnit: event.target.value })}
          />
        </div>
      </div>
      <div className="row-inline">
        <button onClick={() => void submit()} disabled={busy}>
          保存站点
        </button>
      </div>

      <h3>目录导入 / 导出</h3>
      <div className="field">
        <label>粘贴一份目录（JSON：version + sites 数组）</label>
        <textarea
          rows={5}
          value={importDraft}
          onChange={(event) => setImportDraft(event.target.value)}
        />
      </div>
      <div className="row-inline">
        <button onClick={() => void importCatalog()} disabled={busy || !importDraft.trim()}>
          导入目录
        </button>
        <button onClick={() => void exportCatalog()} disabled={busy}>
          导出我的目录
        </button>
      </div>
      {exported && (
        <>
          <p className="muted">导出的目录（可以直接分享给别人；只有凭据 id，没有明文）：</p>
          <div className="export-plain">
            <pre>{exported}</pre>
          </div>
        </>
      )}

      {specSites.length > 0 && (
        <>
          <h3>我的声明式站点</h3>
          <table className="usage-table">
            <thead>
              <tr>
                <th>站点</th>
                <th>定义</th>
                <th>当前余额</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {specSites.map((site) => (
                <tr key={site.id}>
                  <td>
                    <strong>{site.name}</strong>
                    <div className="muted">{site.credentialReady ? "登录态已填" : "登录态没填"}</div>
                  </td>
                  <td>
                    <div className="muted checkin-url">
                      {site.spec
                        ? describeSpec({
                            id: site.spec.id,
                            name: site.spec.id,
                            method: site.spec.method,
                            url: site.spec.url,
                            reward: site.spec.rewardPath
                              ? { path: site.spec.rewardPath }
                              : undefined,
                            balance: site.spec.balancePath
                              ? { url: "", path: site.spec.balancePath }
                              : undefined,
                          })
                        : "—"}
                    </div>
                  </td>
                  <td>
                    {balances[site.id] ? (
                      <span>
                        当前余额 {balances[site.id].amount} {balances[site.id].unit}
                      </span>
                    ) : site.spec?.balancePath ? (
                      <span className="muted">未查询</span>
                    ) : (
                      <span className="muted">这个站点没配余额查询</span>
                    )}
                  </td>
                  <td>
                    {site.spec?.balancePath && (
                      <button onClick={() => void checkBalance(site)} disabled={busy}>
                        查余额
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </>
  );
}

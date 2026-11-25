import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type CircuitStatusRow = {
  channel_id: string;
  channel_name: string;
  upstream_model: string;
  state: "open" | "half_open";
  reason: "failures" | "rate_limit" | null;
  failures: number;
  rate_limit_hits: number;
  open_until: number | null;
  remaining_secs: number | null;
};

type CircuitSettings = {
  open_duration_secs: number;
  rate_limit_cooldown_cap_secs: number;
};

/** 刷新间隔（毫秒）。熔断冷却最长 60s，5s 一次足够看清变化。 */
const REFRESH_MS = 5000;

function stateLabel(state: string): string {
  return state === "open" ? "熔断中" : "半开（等探测）";
}

function reasonLabel(reason: string | null): string {
  if (reason === "rate_limit") return "上游限流 429";
  if (reason === "failures") return "连续失败";
  return "—";
}

/**
 * 熔断状态面板（02 §2.3）。
 *
 * 展示当前正在冷却的 `(渠道, 上游模型)`：状态、原因、剩余秒数；
 * 可手动解除熔断，也可调整熔断时长与 429 冷却上限（默认 60s，最小 1 秒）。
 * 熔断状态落库（`circuit_state` 表），重启后恢复，这里读的是运行中的熔断器。
 */
export default function CircuitPanel() {
  const [rows, setRows] = useState<CircuitStatusRow[]>([]);
  const [settings, setSettings] = useState<CircuitSettings | null>(null);
  const [durationInput, setDurationInput] = useState("60");
  const [capInput, setCapInput] = useState("60");
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [status, current] = await Promise.all([
        invoke<CircuitStatusRow[]>("circuit_status"),
        invoke<CircuitSettings>("circuit_settings_get"),
      ]);
      setRows(status);
      setSettings(current);
      setDurationInput(String(current.open_duration_secs));
      setCapInput(String(current.rate_limit_cooldown_cap_secs));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => {
      void refresh();
    }, REFRESH_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const handleSaveSettings = async () => {
    const openSeconds = Number.parseInt(durationInput.trim(), 10);
    const capSeconds = Number.parseInt(capInput.trim(), 10);
    if (!Number.isFinite(openSeconds) || openSeconds < 1) {
      setError("熔断时长必须是大于等于 1 的整数秒");
      return;
    }
    if (!Number.isFinite(capSeconds) || capSeconds < 1) {
      setError("429 冷却上限必须是大于等于 1 的整数秒");
      return;
    }
    try {
      const saved = await invoke<CircuitSettings>("circuit_settings_set", {
        openDurationSecs: openSeconds,
        rateLimitCooldownCapSecs: capSeconds,
      });
      setSettings(saved);
      setDurationInput(String(saved.open_duration_secs));
      setCapInput(String(saved.rate_limit_cooldown_cap_secs));
      setError(null);
      setMessage(
        `已保存：熔断时长 ${saved.open_duration_secs} 秒，429 冷却上限 ${saved.rate_limit_cooldown_cap_secs} 秒（对之后新打开的熔断/冷却生效）`,
      );
    } catch (e) {
      setError(String(e));
    }
  };

  const handleReset = async (row: CircuitStatusRow) => {
    try {
      const existed = await invoke<boolean>("circuit_reset", {
        channelId: row.channel_id,
        upstreamModel: row.upstream_model,
      });
      setError(null);
      setMessage(
        existed
          ? `已解除 ${row.channel_name} / ${row.upstream_model} 的熔断`
          : "该条记录已不在熔断中",
      );
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <section className="card">
      <h2>熔断状态</h2>
      <p className="muted">
        连续 3 次失败 → 熔断；上游 429 → 按 Retry-After 冷却（不超过冷却上限）。
        冷却期间该渠道会被跳过。
      </p>

      {error && <p className="error">读取熔断状态失败：{error}</p>}
      {!error && !loaded && <p className="muted">正在读取…</p>}

      {!error && loaded && rows.length === 0 && (
        <p className="muted">当前没有渠道处于熔断或冷却中。</p>
      )}

      {rows.length > 0 && (
        <ul className="cred-list">
          {rows.map((row) => (
            <li key={`${row.channel_id}:${row.upstream_model}`}>
              <div className="cred-row">
                <span className="cred-meta">
                  <code>{row.channel_name}</code> · <code>{row.upstream_model}</code> ·{" "}
                  {stateLabel(row.state)} · {reasonLabel(row.reason)}
                  {row.remaining_secs !== null && ` · 剩余 ${row.remaining_secs}s`}
                  {row.reason === "failures" && ` · 连续失败 ${row.failures} 次`}
                  {row.reason === "rate_limit" && ` · 429 命中 ${row.rate_limit_hits} 次`}
                </span>
                <span className="cred-actions">
                  <button type="button" onClick={() => void handleReset(row)}>
                    解除
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}

      <h3>熔断设置</h3>
      <p className="muted">
        连续失败触发的熔断时长（秒），最小 1 秒，默认 60 秒。上游 429 触发的冷却上限
        （秒），同样最小 1 秒、默认 60 秒。保存后立即生效，但只作用于之后新打开的
        熔断/冷却；已在冷却中的不受影响。
      </p>
      <div className="row-inline">
        <input
          type="number"
          min={1}
          step={1}
          value={durationInput}
          onChange={(e) => setDurationInput(e.target.value)}
          placeholder="熔断时长（秒）"
        />
        <input
          type="number"
          min={1}
          step={1}
          value={capInput}
          onChange={(e) => setCapInput(e.target.value)}
          placeholder="429 冷却上限（秒）"
        />
        <button type="button" onClick={() => void handleSaveSettings()}>
          保存设置
        </button>
        <span className="muted">
          {settings
            ? `当前生效：熔断 ${settings.open_duration_secs} 秒 / 429 冷却上限 ${settings.rate_limit_cooldown_cap_secs} 秒`
            : ""}
        </span>
      </div>
      <p className="muted">
        429 冷却时长 = min(max(指数退避, 上游 Retry-After), 上限)；
        上限小于上游要求的等待时间时，会提前重试上游。
      </p>

      <div className="row-inline">
        <button type="button" onClick={() => void refresh()}>
          刷新
        </button>
        <span className="muted">每 {REFRESH_MS / 1000} 秒自动刷新。</span>
      </div>

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

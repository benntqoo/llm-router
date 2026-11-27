import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  describeCopilotState,
  nextPollDelayMs,
  shouldKeepPolling,
  type CopilotPollView,
  type CopilotStatus,
  type DeviceFlowView,
} from "./copilot";

/**
 * GitHub Copilot 授权面板（T20；02 §6.2）。
 *
 * **只做取凭证**：GitHub 官方 device flow → 用户在浏览器输入短码 → 轮询 →
 * 换 Copilot token 存保险库。
 *
 * - `device_code` **不出后端**（IPC 只回 `flow_id` / `user_code` / 验证网址）；
 * - 两个 token **只进保险库**，面板只显示「授权了没 / 到期时间」；
 * - 渠道**默认关闭**——启用开关与 ToS 确认弹窗属 T24（AGENT.md §5.5）。
 */
export default function CopilotPanel() {
  const [status, setStatus] = useState<CopilotStatus | null>(null);
  const [flow, setFlow] = useState<DeviceFlowView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    try {
      const current = await invoke<CopilotStatus>("copilot_status");
      setStatus(current);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    return () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
      }
    };
  }, [refresh]);

  const pollOnce = useCallback(
    async (flowId: string) => {
      try {
        const view = await invoke<CopilotPollView>("copilot_authorize_poll", { flowId });
        if (shouldKeepPolling(view.state)) {
          timerRef.current = setTimeout(() => {
            void pollOnce(flowId);
          }, nextPollDelayMs(view));
          return;
        }
        setFlow(null);
        if (view.state === "authorized") {
          setMessage(
            `授权成功。Copilot token 到期 ${
              view.expiresAt ? new Date(view.expiresAt * 1000).toLocaleString() : "—"
            }`,
          );
        } else if (view.state === "expired") {
          setError("设备码已过期——请重新发起设备授权");
        } else if (view.state === "denied") {
          setError("授权被拒绝");
        }
        await refresh();
      } catch (e) {
        setError(String(e));
        setFlow(null);
      }
    },
    [refresh],
  );

  const handleStart = async () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const view = await invoke<DeviceFlowView>("copilot_authorize_start");
      setFlow(view);
      setMessage(
        `请在浏览器打开验证页并输入短码 ${view.userCode}（点下面链接）`,
      );
      await pollOnce(view.flowId);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleDisconnect = async () => {
    setBusy(true);
    try {
      const removed = await invoke<boolean>("copilot_disconnect");
      setMessage(removed ? "已断开连接（两条凭据已从保险库删除）" : "当前没有已授权的连接");
      setError(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card">
      <h2>GitHub Copilot 授权（T20）</h2>
      <p className="muted">
        用 GitHub 官方「设备码」流程授权：点「设备授权」拿到短码 → 在浏览器输入短码并确认
        → 本页自动轮询 → 授权成功后凭据只进加密保险库（应用不显示、不导出 token）。
      </p>

      {status && <p className="muted">{describeCopilotState(status)}</p>}

      {flow && (
        <div className="row-inline">
          <code>{flow.userCode}</code>
          <a href={flow.verificationUri} target="_blank" rel="noreferrer">
            打开验证页
          </a>
          <span className="muted">
            等待你在浏览器确认…（每 {flow.interval} 秒自动查一次，
            {Math.round(flow.expiresIn / 60)} 分钟内有效）
          </span>
        </div>
      )}

      <div className="row-inline">
        <button type="button" disabled={busy} onClick={() => void handleStart()}>
          设备授权
        </button>
        <button type="button" disabled={busy} onClick={() => void handleDisconnect()}>
          断开连接
        </button>
        <button type="button" onClick={() => void refresh()}>
          刷新状态
        </button>
      </div>

      <p className="muted">
        <strong>合规提示</strong>：把 Copilot 额度用于第三方客户端违反 GitHub Copilot
        ToS，账号可能受限。本卡只做「取凭证」，渠道默认关闭；启用前需在设置里确认 ToS
        （该确认属 T24）。Copilot token 约 25~30 分钟有效，过期自动重取。
      </p>

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

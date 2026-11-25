import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import CircuitPanel from "./CircuitPanel";
import ClientKeyPanel from "./ClientKeyPanel";
import VaultPanel from "./VaultPanel";
import "./App.css";

type GatewayStatus = {
  running: boolean;
  addr: string;
  base_url: string;
  note: string;
};

function App() {
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<GatewayStatus>("gateway_status")
      .then(setStatus)
      .catch((err) => setError(String(err)));
  }, []);

  return (
    <main className="container">
      <header className="header">
        <h1>LLM Router</h1>
        <p className="subtitle">本地 AI 网关 · Tauri 2 + Rust + axum</p>
      </header>

      <section className="card">
        <h2>网关状态</h2>
        {error && <p className="error">读取网关状态失败：{error}</p>}
        {!error && !status && <p className="muted">正在连接本地网关…</p>}
        {status && (
          <dl className="status">
            <dt>运行状态</dt>
            <dd>
              <span className={status.running ? "dot ok" : "dot bad"} />
              {status.running ? "运行中" : "未运行"}
            </dd>

            <dt>监听地址</dt>
            <dd>
              <code>{status.addr}</code>
            </dd>

            <dt>客户端 base_url</dt>
            <dd>
              <code>{status.base_url}</code>
            </dd>
          </dl>
        )}
      </section>

      <section className="card">
        <h2>当前范围</h2>
        <p className="muted">{status?.note ?? "正在读取…"}</p>
        <p className="muted">
          已完成 T0 骨架 / T1 数据层 / T2 加密保险库 / T3 入站鉴权 / T4 出站适配器 /
          T5 路由别名 / T6 流式管道与熔断；后续：T7 用量 → T8 预设 → T9 UI → T10 实测矩阵
        </p>
      </section>

      <VaultPanel />

      <ClientKeyPanel />

      <CircuitPanel />
    </main>
  );
}

export default App;

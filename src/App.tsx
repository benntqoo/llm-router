import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ChannelPanel from "./ChannelPanel";
import CircuitPanel from "./CircuitPanel";
import ClientKeyPanel from "./ClientKeyPanel";
import ModelsPanel from "./ModelsPanel";
import UsagePanel from "./usage/UsagePanel";
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
        <h2>快速开始（从零接入三步）</h2>
        <ol className="steps">
          <li>
            「渠道」面板 → 选预设（DeepSeek / Kimi / GLM …）→ 只填 API Key →
            保存。别名会自动出现在 <code>/v1/models</code>。
          </li>
          <li>
            「加密保险库」面板 → 选中刚建的渠道 → 回看 / 轮换密钥（显示 30 秒后自动遮蔽）。
          </li>
          <li>
            「Client Key」面板 → 创建一把本地口令（明文只显示一次，复制保存）→
            「模型预览」面板复制接入片段，填进客户端。
          </li>
        </ol>
      </section>

      <section className="card">
        <h2>当前范围</h2>
        <p className="muted">{status?.note ?? "正在读取…"}</p>
        <p className="muted">
          已完成 T0 骨架 / T1 数据层 / T2 加密保险库 / T3 入站鉴权 / T4 出站适配器 /
          T5 路由别名 / T6 流式管道与熔断 / T7 用量采集 / T8 预设与自定义渠道 /
          T9 UI 第一轮 / T11 安全验收 / T12 本地估算器 / T13 价目表与成本换算；
          T10 实测矩阵的执行器已建（A 类 7 列真实数据待真实渠道 key）。
        </p>
      </section>

      <ChannelPanel />

      <VaultPanel />

      <ClientKeyPanel />

      <ModelsPanel />

      <CircuitPanel />

      <UsagePanel />
    </main>
  );
}

export default App;

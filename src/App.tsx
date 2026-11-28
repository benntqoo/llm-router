import { useEffect, useState, type ComponentType } from "react";
import { invoke } from "@tauri-apps/api/core";
import ChannelPanel from "./ChannelPanel";
import CheckinPanel from "./checkin/CheckinPanel";
import CircuitPanel from "./CircuitPanel";
import ClientKeyPanel from "./ClientKeyPanel";
import CopilotPanel from "./copilot/CopilotPanel";
import ModelsPanel from "./ModelsPanel";
import UsagePanel from "./usage/UsagePanel";
import VaultPanel from "./VaultPanel";
import {
  readStoredTab,
  storeTab,
  TABS,
  type PanelId,
  type TabId,
} from "./tabs";
import "./App.css";

type GatewayStatus = {
  running: boolean;
  addr: string;
  base_url: string;
  note: string;
};

/** 面板 → 组件（**组件映射留在这一层**：`tabs.ts` 只管分组数据，好单测）。 */
const PANEL_COMPONENTS: Record<PanelId, ComponentType> = {
  channel: ChannelPanel,
  models: ModelsPanel,
  copilot: CopilotPanel,
  vault: VaultPanel,
  clientKey: ClientKeyPanel,
  circuit: CircuitPanel,
  usage: UsagePanel,
  checkin: CheckinPanel,
};

function App() {
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  // **选中项持久化**（02 §6.7）：重开应用回到原地；未知值回落到默认 tab
  const [tab, setTab] = useState<TabId>(() => readStoredTab());

  useEffect(() => {
    invoke<GatewayStatus>("gateway_status")
      .then(setStatus)
      .catch((err) => setError(String(err)));
  }, []);

  useEffect(() => {
    storeTab(tab);
  }, [tab]);

  const current = TABS.find((item) => item.id === tab) ?? TABS[0];

  return (
    <main className="container">
      <header className="header">
        <div>
          <h1>LLM Router</h1>
          <p className="subtitle">本地 AI 网关 · Tauri 2 + Rust + axum</p>
        </div>
        {/* **网关状态常驻顶栏**（02 §6.7）：切到任何 tab 都看得见「网关在不在跑」 */}
        <div className="statusbar">
          {error ? (
            <span className="error">读取网关状态失败：{error}</span>
          ) : !status ? (
            <span className="muted">正在连接本地网关…</span>
          ) : (
            <>
              <span>
                <span className={status.running ? "dot ok" : "dot bad"} />
                {status.running ? "运行中" : "未运行"}
              </span>
              <code>{status.addr}</code>
              <code>{status.base_url}</code>
            </>
          )}
        </div>
      </header>

      {/* 功能分组（4 组）：接入 / 凭证 / 运维 / 签到与积分 */}
      <nav className="tabs" role="tablist">
        {TABS.map((item) => (
          <button
            key={item.id}
            role="tab"
            aria-selected={item.id === current.id}
            className={item.id === current.id ? "tab active" : "tab"}
            title={item.hint}
            onClick={() => setTab(item.id)}
          >
            {item.label}
          </button>
        ))}
      </nav>

      {error && <p className="error">读取网关状态失败：{error}</p>}

      {/* 「快速开始」讲的就是配渠道 → 放在「接入」组首卡 */}
      {current.id === "access" && (
        <section className="card">
          <h2>快速开始（从零接入三步）</h2>
          <ol className="steps">
            <li>
              「渠道」面板 → 选预设（DeepSeek / Kimi / GLM …）→ 只填 API Key →
              保存。别名会自动出现在 <code>/v1/models</code>。
            </li>
            <li>
              「凭证」组的「加密保险库」面板 → 选中刚建的渠道 → 回看 / 轮换密钥（显示 30 秒后自动遮蔽）。
            </li>
            <li>
              「凭证」组的「Client Key」面板 → 创建一把本地口令（明文只显示一次，复制保存）→
              「接入」组的「模型预览」面板复制接入片段，填进客户端。
            </li>
          </ol>
        </section>
      )}

      {/* **一次只渲染当前 tab 的面板**（不做「全渲染 + CSS 隐藏」） */}
      {current.panels.map((panel) => {
        const Panel = PANEL_COMPONENTS[panel];
        return <Panel key={panel} />;
      })}

      {/* 「当前范围」是历史说明、不是操作入口 → 放底部折叠区 */}
      <section className="card">
        <details>
          <summary className="muted">当前范围（点开看）</summary>
          <p className="muted">{status?.note ?? "正在读取…"}</p>
          <p className="muted">
            已完成 T0 骨架 / T1 数据层 / T2 加密保险库 / T3 入站鉴权 / T4 出站适配器 /
            T5 路由别名 / T6 流式管道与熔断 / T7 用量采集 / T8 预设与自定义渠道 /
            T9 UI 第一轮 / T11 安全验收 / T12 本地估算器 / T13 价目表与成本换算 /
            T14 聚合与图表 UI / T15 HAR 导入与模板化；
            T10 实测矩阵的执行器已建（A 类 7 列真实数据待真实渠道 key）。
          </p>
        </details>
      </section>
    </main>
  );
}

export default App;

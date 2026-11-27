/**
 * Copilot 授权相关的纯函数（T20；02 §6.2）。
 *
 * 放这里是为了**能单测**（`node --test`）：面板只做渲染与状态，
 * 「轮询要不要继续 / 下次等多久 / 状态怎么说清」这类判断走纯函数。
 */

/** 授权状态（后端 `CopilotPollView.state`）。 */
export type CopilotPollState =
  | "pending"
  | "slow_down"
  | "expired"
  | "denied"
  | "authorized";

/** 授权状态总览（后端 `CopilotStatusView`）。 */
export type CopilotStatus = {
  authorized: boolean;
  expiresAt: number | null;
  apiBase: string | null;
  channelId: string | null;
  channelEnabled: boolean;
};

/** 设备授权会话（后端 `DeviceFlowView`；**不含** `device_code`）。 */
export type DeviceFlowView = {
  flowId: string;
  userCode: string;
  verificationUri: string;
  interval: number;
  expiresIn: number;
};

/** 一次轮询的结果（后端 `CopilotPollView`）。 */
export type CopilotPollView = {
  state: CopilotPollState;
  interval: number;
  expiresAt: number | null;
  apiBase: string | null;
};

/** 这两种状态**还要继续轮询**（其余都是终态）。 */
export function shouldKeepPolling(state: CopilotPollState): boolean {
  return state === "pending" || state === "slow_down";
}

/**
 * 下次轮询前等多久（毫秒）。
 *
 * 后端已经在 `slow_down` 时把间隔 **+5s** 算好了，这里只做下限保护。
 */
export function nextPollDelayMs(view: CopilotPollView): number {
  return Math.max(1, view.interval) * 1000;
}

/** 授权状态一句话说清后果。 */
export function describeCopilotState(status: CopilotStatus): string {
  if (!status.authorized) {
    return status.channelId
      ? "还没授权——点「设备授权」按提示在浏览器里输入短码。"
      : "还没授权——点「设备授权」开始（GitHub 官方设备码流程）。";
  }
  const until = formatExpiry(status.expiresAt);
  const enabled = status.channelEnabled
    ? "渠道已启用。"
    : "渠道**默认关闭**（启用与 ToS 确认属 T24）。";
  return `已授权${until ? `（Copilot token 到期 ${until}）` : ""}。${enabled}`;
}

/** 过期时刻 → 人话（**空** = 未知）。 */
export function formatExpiry(expiresAt: number | null): string {
  if (!expiresAt) {
    return "";
  }
  const date = new Date(expiresAt * 1000);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 模型目录里的一条模型（后端 `CopilotModelView`；**别名 = id**）。 */
export type CopilotModel = {
  id: string;
  name: string;
  contextLen: number | null;
  supportsToolCalls: boolean | null;
  supportsVision: boolean | null;
};

/** 模型目录刷新结果（后端 `CopilotModelsView`）。 */
export type CopilotModelsView = {
  models: CopilotModel[];
  added: number;
  updated: number;
  disabled: number;
  skipped: string[];
};

/**
 * 模型目录刷新结果 → 人话。
 *
 * **为什么把「跳过」也说清**：被跳过的别名是**别的渠道占着**（或用户改过别名），
 * 用户需要知道**为什么这个模型没进来**，否则会以为刷新失败。
 */
export function describeModelSync(view: CopilotModelsView): string {
  const parts = [`共 ${view.models.length} 个模型`];
  if (view.added > 0) {
    parts.push(`新增 ${view.added}`);
  }
  if (view.updated > 0) {
    parts.push(`更新 ${view.updated}`);
  }
  if (view.disabled > 0) {
    parts.push(`禁用 ${view.disabled}（上游已没有）`);
  }
  if (view.skipped.length > 0) {
    parts.push(`跳过 ${view.skipped.length}（别名已被占用：${view.skipped.join("、")}）`);
  }
  return parts.join("，");
}

/** 一个模型的中文说明（上下文长度 / 能力）。 */
export function describeModel(model: CopilotModel): string {
  const bits: string[] = [];
  if (model.contextLen) {
    bits.push(`上下文 ${Math.round(model.contextLen / 1000)}K`);
  }
  if (model.supportsToolCalls) {
    bits.push("工具调用");
  }
  if (model.supportsVision) {
    bits.push("图片输入");
  }
  return bits.length > 0 ? bits.join(" / ") : "—";
}

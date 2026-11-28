/**
 * ToS 合规确认与渠道启用（T24；02 §6.8）。
 *
 * 放这里是为了**能单测**（`node --test`）：面板只做渲染与状态，
 * 「这句话怎么说清 / 这次点击要不要先弹确认框」这类判断走纯函数。
 *
 * **声明原文与后端逐字一致**（`providers::compliance::COPILOT_TOS_STATEMENT`）——
 * 两边不一致会让用户看到的和后端记录的对不上号。
 */

/**
 * Copilot 的 ToS 声明原文（**必须逐字进弹窗**；02 §6.2 / §6.8.2）。
 */
export const COPILOT_TOS_STATEMENT =
  "将 Copilot 额度用于第三方客户端违反 GitHub Copilot ToS，账号可能受限。用户显式确认后才启用。";

/**
 * 声明版本（**改文案就要升版本** → 旧确认失效 → 要重新确认）。
 */
export const COPILOT_TOS_VERSION = "copilot-tos-2026-11-28";

/** 启用与确认状态（后端 `CopilotStatusView` 里的相关字段）。 */
export type TosStatus = {
  /** 渠道是否已启用。 */
  channelEnabled: boolean;
  /** ToS 确认时刻（Unix 秒；`null` = 没确认过）。 */
  tosAcceptedAt: number | null;
  /**
   * **是否还需要过一次确认**（**后端裁定**：没确认过 **或** 声明版本已改）。
   */
  tosRequired: boolean;
};

/**
 * 这次的「开启」点击要不要**先弹 ToS 确认框**。
 *
 * - 已启用 → 不用（这次是关）；
 * - 还没确认过（或声明改过）→ **要弹**；
 * - 已确认过 → 直接启用。
 */
export function needsTosConfirmation(status: TosStatus): boolean {
  if (status.channelEnabled) {
    return false;
  }
  return status.tosRequired || status.tosAcceptedAt === null;
}

/**
 * 启用开关旁边的一句话状态（**说清后果**，别让用户猜）。
 */
export function describeEnableState(status: TosStatus): string {
  if (status.channelEnabled) {
    return "渠道已启用——它的模型会出现在 `/v1/models`，harness 可以直接用。";
  }
  if (status.tosRequired || status.tosAcceptedAt === null) {
    return "渠道默认关闭——首次启用需要先确认 GitHub Copilot 的服务条款。";
  }
  return "渠道已关闭——你已确认过服务条款，可以直接启用。";
}

/**
 * 确认框里那句「你将做什么」的提示（**后果说在前面**）。
 */
export function describeEnableEffect(): string {
  return "启用后，Copilot 的模型会出现在 `/v1/models`，harness 可以直接用你的 Copilot 额度发起请求。";
}

/** 确认时刻 → 人话（**空** = 没确认过）。 */
export function formatAcceptedAt(acceptedAt: number | null): string {
  if (!acceptedAt) {
    return "";
  }
  const date = new Date(acceptedAt * 1000);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

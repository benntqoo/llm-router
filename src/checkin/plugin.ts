/**
 * 站点插件的纯函数（T18；02 §7.4）。
 *
 * 这些函数**不碰 Tauri**、不碰 DOM —— 可以用 `node --test` 直接跑单测
 * （与 `src/checkin/schedule.ts` 同一路子）。
 *
 * 关键语义：
 * - **登录态分三类**：HAR 模板（变量在模板里）、辅轨型（浏览器里登录一次，
 *   不用手填）、接口型（手填 Cookie / token）。UI 按这三类给不同的入口；
 * - **积分显示的是「本次拿到了多少」**（额度快照），**不是账户余额真值**——
 *   措辞上不能写成「余额」。
 */

import { formatClock } from "./schedule.ts";

/** 登录态的三类来源。 */
export type CredentialState =
  /** HAR 模板：敏感变量在模板里（导入向导管）。 */
  | "har"
  /** 辅轨型：在浏览器里登录一次，不用手填。 */
  | "sidecar"
  /** 接口型：还没填登录态。 */
  | "missing"
  /** 接口型：登录态已填。 */
  | "ready";

/** 判断一个站点的登录态属于哪一类。 */
export function credentialState(site: {
  plugin: string;
  needsSidecar: boolean;
  credentialReady: boolean;
}): CredentialState {
  if (site.plugin === "har") {
    return "har";
  }
  if (site.needsSidecar) {
    return "sidecar";
  }
  return site.credentialReady ? "ready" : "missing";
}

/** 登录态的一行中文说明。 */
export function describeCredentialState(state: CredentialState): string {
  switch (state) {
    case "har":
      return "由 HAR 模板自带变量";
    case "sidecar":
      return "在浏览器里登录一次即可，不用手填凭据";
    case "missing":
      return "还没填登录态——填了才能签到";
    case "ready":
      return "登录态已填（存在加密保险库里）";
  }
}

/** 积分单位的中文名。 */
export function unitLabel(unit: string): string {
  switch (unit) {
    case "magicube":
      return "魔粒";
    case "point":
      return "积分";
    default:
      return unit || "积分";
  }
}

/** 积分快照的中文描述（**是「本次拿到」，不是「余额」**）。 */
export function describeReward(
  reward: { points: number; unit: string; updatedAt: number } | null | undefined,
): string {
  if (!reward) {
    return "还没有积分记录";
  }
  return `本次拿到 ${reward.points} ${unitLabel(reward.unit)}（${formatClock(
    reward.updatedAt,
  )}）`;
}

/** 内置插件的展示名（后端也会给名字，这里给的是**兜底**）。 */
export function pluginLabel(pluginId: string): string {
  switch (pluginId) {
    case "har":
      return "HAR 模板";
    case "modelscope":
      return "ModelScope（魔搭）";
    case "minimax_code":
      return "MiniMax Code（网页版）";
    case "spec":
      return "声明式站点（T25）";
    default:
      return pluginId;
  }
}

/** 插件是不是内置站点插件（`har` 之外的都是）。 */
export function isBuiltinPlugin(pluginId: string): boolean {
  return pluginId !== "har" && pluginId !== "";
}

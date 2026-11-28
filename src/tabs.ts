/**
 * 功能分组（T26；02 §6.7）。
 *
 * **纯数据、不碰 React** → 可以用 `node --test` 直接跑单测
 * （与 `src/checkin/schedule.ts` 同一路子）。
 *
 * 为什么要分组：8 个面板单页平铺 → 首屏被说明卡片占掉、找功能靠纵向滚动。
 * 头头 2026-11-28 拍板 **4 组**：接入 / 凭证 / 运维 / 签到与积分。
 */

/** tab 标识。 */
export type TabId = "access" | "credentials" | "ops" | "checkin";

/** 面板标识。 */
export type PanelId =
  | "channel"
  | "models"
  | "copilot"
  | "vault"
  | "clientKey"
  | "circuit"
  | "usage"
  | "checkin";

/** 一个 tab 的定义。 */
export type TabDef = {
  id: TabId;
  /** tab 上的名字。 */
  label: string;
  /** 一句话说明（鼠标悬停可见）。 */
  hint: string;
  /** 该组包含的面板（**按渲染顺序**）。 */
  panels: PanelId[];
};

/** 全部 tab（**顺序 = 界面上的顺序**）。 */
export const TABS: TabDef[] = [
  {
    id: "access",
    label: "接入",
    hint: "渠道 · 模型目录 · Copilot 反代",
    panels: ["channel", "models", "copilot"],
  },
  {
    id: "credentials",
    label: "凭证",
    hint: "加密保险库 · Client Key",
    panels: ["vault", "clientKey"],
  },
  {
    id: "ops",
    label: "运维",
    hint: "熔断状态 · 用量统计",
    panels: ["circuit", "usage"],
  },
  {
    id: "checkin",
    label: "签到与积分",
    hint: "自动签到 · 站点目录 · 积分余额",
    panels: ["checkin"],
  },
];

/** 默认 tab（新用户第一件事是配渠道 → 落在「接入」）。 */
export const DEFAULT_TAB: TabId = "access";

/** 选中项持久化的 localStorage 键。 */
export const TAB_STORAGE_KEY = "llm-router.activeTab";

/** 取某个 tab 的定义（**没有就是 `undefined`**）。 */
export function tabById(id: string | null | undefined): TabDef | undefined {
  return TABS.find((tab) => tab.id === id);
}

/**
 * 把（可能来自 localStorage 的）字符串解析成合法的 tab id。
 *
 * **未知值回落到默认 tab**——localStorage 里的旧值、手改过的值都不能让界面空掉。
 */
export function resolveTab(raw: string | null | undefined): TabId {
  const found = tabById(raw);
  return found ? found.id : DEFAULT_TAB;
}

/** 读上次选中的 tab（**读不到就用默认值**）。 */
export function readStoredTab(): TabId {
  try {
    if (typeof localStorage === "undefined") {
      return DEFAULT_TAB;
    }
    return resolveTab(localStorage.getItem(TAB_STORAGE_KEY));
  } catch {
    return DEFAULT_TAB;
  }
}

/** 记下这次选中的 tab（**存不了就算了，不报错**）。 */
export function storeTab(id: TabId): void {
  try {
    if (typeof localStorage === "undefined") {
      return;
    }
    localStorage.setItem(TAB_STORAGE_KEY, id);
  } catch {
    // 隐私模式等场景下 localStorage 可能不可用——**不影响使用**
  }
}

/** 某个面板归在哪个 tab（给面板自己反查用）。 */
export function tabOfPanel(panel: PanelId): TabId {
  for (const tab of TABS) {
    if (tab.panels.includes(panel)) {
      return tab.id;
    }
  }
  return DEFAULT_TAB;
}

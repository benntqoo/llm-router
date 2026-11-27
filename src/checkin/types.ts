/**
 * 签到导入向导的 IPC 数据形状（与后端 `checkin::har` / `commands::checkin` 一一对应）。
 */

/** 变量的位置（与后端 `VarTarget` 对齐）。 */
export type VarTarget =
  | { kind: "header"; field: string }
  | { kind: "headerPart"; field: string; part: string }
  | { kind: "query"; field: string }
  | { kind: "body"; field: string };

/** 一个可标记的字段（敏感值在后端已掩码）。 */
export type FieldView = {
  target: VarTarget;
  value: string;
  sensitive: boolean;
  suggested: boolean;
};

/** HAR 的一条 entry。 */
export type EntryView = {
  index: number;
  method: string;
  url: string;
  status: number;
  fields: FieldView[];
};

/** 一次导入的结果。 */
export type HarImport = {
  source: string;
  entryCount: number;
  entries: EntryView[];
};

/** 用户标记好的一个变量。 */
export type MarkedVariable = {
  name: string;
  target: VarTarget;
};

/** 保存模板的入参。 */
export type SaveTemplateInput = {
  path: string;
  entryIndex: number;
  siteName: string;
  channelId: string | null;
  channelName: string | null;
  variables: MarkedVariable[];
};

/** 模板摘要。 */
export type TemplateSummary = {
  source: string;
  method: string;
  url: string;
  status: number;
  variableCount: number;
  variableNames: string[];
};

/** 签到站点摘要（T17 扩展：调度信息；T18 扩展：插件信息）。 */
export type SiteSummary = {
  id: string;
  name: string;
  plugin: string;
  enabled: boolean;
  schedule: string | null;
  template: TemplateSummary | null;
  /** 下一次应触发时刻（Unix 秒；没有 cron 则为 `null`）。 */
  nextRunAt: number | null;
  /** 最近一次尝试（不论成败）的时刻（Unix 秒）。 */
  lastAttemptAt: number | null;
  /** 最近一次尝试是否成功。 */
  lastOk: boolean | null;
  /** 系统兜底任务名（Windows 计划任务 / macOS launchd）。 */
  systemTask: string | null;
  /** 系统兜底任务是否已注册。 */
  systemTaskRegistered: boolean;
  /** 系统兜底任务的频率描述。 */
  systemTaskFrequency: string | null;
  /** 系统兜底同步时的错误（注册失败等）。 */
  systemTaskError: string | null;
  /** 内置插件的登录态是否已填（HAR 模板恒为 `false`）。 */
  credentialReady: boolean;
  /** 内置插件是否需要浏览器辅轨。 */
  needsSidecar: boolean;
  /** 内置插件的凭据提示（HAR 模板为 `null`）。 */
  credentialHint: string | null;
  /** 该站点归属账号的积分快照。 */
  reward: RewardSnapshot | null;
};

/** 账号额度快照里的签到积分（**「本次拿到了多少」，不是账户余额真值**）。 */
export type RewardSnapshot = {
  points: number;
  unit: string;
  source: string;
  updatedAt: number;
};

/** 一个内置站点插件（UI 的「添加站点」清单项）。 */
export type BuiltinSite = {
  id: string;
  name: string;
  /** 凭据提示（告诉用户要准备什么）。 */
  credentialHint: string;
  /** 是否需要浏览器辅轨。 */
  needsSidecar: boolean;
  /** 默认 cron。 */
  defaultSchedule: string;
};

/** 辅轨探活结果。 */
export type SidecarHealth = {
  nodeVersion: string;
  browserAvailable: boolean;
  browserChannel: string;
  profileDir: string;
  loggedIn: boolean | null;
};

/** 登录交接的摘要。 */
export type LoginSummary = {
  ok: boolean;
  message: string;
};

/** 一次执行的摘要（「立即执行一次」的返回）。 */
export type RunSummary = {
  siteId: string;
  siteName: string;
  ok: boolean;
  /** 错误分类（`success` / `assert_failed` / …）。 */
  class: string;
  message: string;
  status: number | null;
  elapsedMs: number;
  retryable: boolean;
};

/** 一条签到结果的展示视图。 */
export type ResultView = {
  id: number;
  siteId: string;
  ts: number;
  ok: boolean;
  rewardJson: string | null;
  message: string | null;
};

/** 系统兜底同步的摘要。 */
export type SystemSyncSummary = {
  /** 已注册的任务名。 */
  registered: string[];
  /** 已注销的任务名。 */
  unregistered: string[];
  /** 出错的站点（站点名 + 原因）。 */
  errors: string[];
  /** 当前平台是否支持系统兜底。 */
  supported: boolean;
};

/** 渠道摘要（只取向导要用的字段）。 */
export type ChannelOption = {
  id: string;
  name: string;
  provider: string;
};

/** 通知配置（T19；**没有地址**——地址在加密保险库里，不回显）。 */
export type NotifyConfig = {
  /** 总开关（关 = 一律不发）。 */
  enabled: boolean;
  /** 系统通知（弹窗）开关。 */
  systemEnabled: boolean;
  /** 成功也通知（失败永远通知）。 */
  notifySuccess: boolean;
  /** webhook 类型（`wecom` / `lark` / `telegram` / `null` = 不用）。 */
  webhookKind: string | null;
  /** webhook 是否已配置。 */
  webhookConfigured: boolean;
  /** Telegram 的 chat_id。 */
  telegramChatId: string | null;
};

/** 保存通知配置的入参（**地址只在这次提交里出现一次**）。 */
export type NotifyConfigInput = {
  enabled: boolean;
  systemEnabled: boolean;
  notifySuccess: boolean;
  webhookKind: string | null;
  /**
   * webhook 地址：
   * - `null` → **不动**（保持原样）；
   * - `""` → **清除**；
   * - 有值 → **设置**（进保险库）。
   */
  webhookUrl: string | null;
  telegramChatId: string | null;
};

/** 一个通知通道的发送结果（**脱敏**：不含地址）。 */
export type NotifyChannelResult = {
  channel: string;
  ok: boolean;
  detail: string;
};

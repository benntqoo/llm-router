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

/** 签到站点摘要（T17 扩展：调度信息）。 */
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

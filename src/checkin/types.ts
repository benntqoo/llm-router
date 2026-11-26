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

/** 签到站点摘要。 */
export type SiteSummary = {
  id: string;
  name: string;
  plugin: string;
  enabled: boolean;
  schedule: string | null;
  template: TemplateSummary | null;
};

/** 渠道摘要（只取向导要用的字段）。 */
export type ChannelOption = {
  id: string;
  name: string;
  provider: string;
};

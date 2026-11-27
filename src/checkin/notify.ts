/**
 * 通知相关的纯函数（T19；02 §7.5）。
 *
 * 放这里是为了**能单测**（`node --test`）：面板只做渲染与状态，
 * 「哪个 webhook 类型要 chat_id / 输入框该显示什么提示」这类判断走纯函数。
 */

/** webhook 类型清单（UI 的下拉项）。 */
export const WEBHOOK_KINDS: { value: string; label: string; hint: string }[] = [
  { value: "", label: "不使用 webhook", hint: "只发系统通知" },
  {
    value: "wecom",
    label: "企业微信群机器人",
    hint: "群机器人设置里复制 Webhook 地址（地址里带 key，当密文存）",
  },
  {
    value: "lark",
    label: "飞书群机器人",
    hint: "群机器人设置里复制 Webhook 地址（地址里带 key，当密文存）",
  },
  {
    value: "telegram",
    label: "Telegram Bot",
    hint: "https://api.telegram.org/bot<token>/sendMessage（token 在地址里，另填 chat_id）",
  },
];

/** 这个 webhook 类型**要不要 chat_id**（Telegram 要，别家不要）。 */
export function needsChatId(kind: string | null | undefined): boolean {
  return kind === "telegram";
}

/** webhook 输入框的 placeholder（按类型给不同的提示）。 */
export function webhookPlaceholder(kind: string | null | undefined): string {
  switch (kind) {
    case "wecom":
      return "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=…";
    case "lark":
      return "https://open.feishu.cn/open-apis/bot/v2/hook/…";
    case "telegram":
      return "https://api.telegram.org/bot<token>/sendMessage";
    default:
      return "先选一个 webhook 类型";
  }
}

/**
 * 保存时该给后端传什么地址（**纯函数**）：
 * - 输入框留空 → `null` = **不动**（已配置的地址不该被空输入清掉）；
 * - 填了新值 → 原样传（后端进保险库）。
 */
export function webhookUrlToSend(draft: string): string | null {
  const trimmed = draft.trim();
  return trimmed === "" ? null : trimmed;
}

/** 通知总开关关掉时，UI 该显示什么（**一句话说清后果**）。 */
export function describeNotifyState(
  enabled: boolean,
  systemEnabled: boolean,
  notifySuccess: boolean,
  webhookConfigured: boolean,
): string {
  if (!enabled) {
    return "通知已关闭——签到照跑，但不通知（失败也不再提醒）。";
  }
  const channels: string[] = [];
  if (systemEnabled) {
    channels.push("系统通知");
  }
  if (webhookConfigured) {
    channels.push("webhook");
  }
  if (channels.length === 0) {
    return "已开启，但还没有可用通道——勾上「系统通知」或配一个 webhook。";
  }
  const success = notifySuccess ? "成功也通知" : "只通知失败";
  return `已开启：${channels.join(" + ")}（${success}）。`;
}

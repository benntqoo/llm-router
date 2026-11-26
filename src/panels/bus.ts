/**
 * 面板之间的数据变更通知（T9）。
 *
 * 渠道面板改了渠道/密钥，保险库与模型预览面板必须**立刻**跟着变，
 * 否则会出现"渠道显示密钥已录入，保险库却显示 0 条凭据"这种自相矛盾。
 *
 * 机制：浏览器原生 `CustomEvent`，不引第三方依赖。
 * 面板在挂载时订阅，在收到通知后重新拉数据；另外还有 5s 轮询兜底
 * （防止漏掉通知，例如数据被别的路径改动）。
 */

/** 数据变更事件名。 */
export const DATA_CHANGED = "llmr:data-changed";

/** 广播"数据变了"。 */
export function notifyDataChanged(): void {
  window.dispatchEvent(new CustomEvent(DATA_CHANGED));
}

/** 订阅"数据变了"。返回取消订阅函数。 */
export function onDataChanged(callback: () => void): () => void {
  const handler = () => callback();
  window.addEventListener(DATA_CHANGED, handler);
  return () => window.removeEventListener(DATA_CHANGED, handler);
}

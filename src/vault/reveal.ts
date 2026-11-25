/**
 * 回看遮蔽逻辑（纯函数，便于单测）。
 *
 * 规则（02 §5.2）：点击"显示"后解密展示，30 秒自动遮蔽。
 */

/** 回看展示时长：30 秒。 */
export const REVEAL_TIMEOUT_MS = 30_000;

/** 回看状态。 */
export type RevealState = {
  /** 当前展示的明文；null 表示已遮蔽 */
  secret: string | null;
  /** 到期时间戳（毫秒）；null 表示当前没有展示中的明文 */
  expireAt: number | null;
};

/** 初始状态：完全遮蔽。 */
export const HIDDEN: RevealState = { secret: null, expireAt: null };

/** 开始回看：从当前时刻起 30 秒内可见。 */
export function revealNow(secret: string, now: number): RevealState {
  return { secret, expireAt: now + REVEAL_TIMEOUT_MS };
}

/** 是否已到期。 */
export function isExpired(state: RevealState, now: number): boolean {
  return state.expireAt !== null && now >= state.expireAt;
}

/** 剩余秒数（向上取整；已遮蔽或已到期返回 0）。 */
export function remainingSeconds(state: RevealState, now: number): number {
  if (state.secret === null || state.expireAt === null) {
    return 0;
  }
  const left = state.expireAt - now;
  if (left <= 0) {
    return 0;
  }
  return Math.ceil(left / 1000);
}

/**
 * 当前应当显示的文本。
 *
 * 到期后返回空串（调用方据此遮蔽），未到期返回明文。
 */
export function visibleSecret(state: RevealState, now: number): string {
  if (state.secret === null || isExpired(state, now)) {
    return "";
  }
  return state.secret;
}

/** 是否处于"正在展示明文"的状态。 */
export function isRevealing(state: RevealState, now: number): boolean {
  return visibleSecret(state, now).length > 0;
}

import assert from "node:assert/strict";
import test from "node:test";

import {
  HIDDEN,
  REVEAL_TIMEOUT_MS,
  isExpired,
  isRevealing,
  remainingSeconds,
  revealNow,
  visibleSecret,
} from "./reveal.ts";

test("初始状态完全遮蔽", () => {
  assert.equal(visibleSecret(HIDDEN, 0), "");
  assert.equal(isRevealing(HIDDEN, 0), false);
  assert.equal(remainingSeconds(HIDDEN, 0), 0);
});

test("回看后 30 秒内可见", () => {
  const now = 1_000_000;
  const state = revealNow("sk-demo-secret", now);
  assert.equal(visibleSecret(state, now), "sk-demo-secret");
  assert.equal(isRevealing(state, now), true);
  assert.equal(remainingSeconds(state, now), 30);
});

test("第 30 秒到期后自动遮蔽", () => {
  const now = 1_000_000;
  const state = revealNow("sk-demo-secret", now);
  const atDeadline = now + REVEAL_TIMEOUT_MS;
  assert.equal(isExpired(state, atDeadline), true);
  assert.equal(visibleSecret(state, atDeadline), "");
  assert.equal(isRevealing(state, atDeadline), false);
  assert.equal(remainingSeconds(state, atDeadline), 0);
});

test("剩余秒数随时间递减", () => {
  const now = 0;
  const state = revealNow("secret", now);
  assert.equal(remainingSeconds(state, 0), 30);
  assert.equal(remainingSeconds(state, 5_000), 25);
  assert.equal(remainingSeconds(state, 29_999), 1);
  assert.equal(remainingSeconds(state, 30_000), 0);
});

test("重复回看会重新计时", () => {
  const now = 0;
  const first = revealNow("a", now);
  const second = revealNow("b", now + 10_000);
  assert.equal(visibleSecret(second, now + 10_000), "b");
  assert.equal(remainingSeconds(second, now + 10_000), 30);
  assert.equal(remainingSeconds(first, now + 10_000), 20);
});

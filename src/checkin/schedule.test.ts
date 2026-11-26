import assert from "node:assert/strict";
import test from "node:test";

import {
  cronFields,
  describeCron,
  describeSystemTask,
  expandWeekdays,
  formatClock,
  formatRelative,
  validateCron,
  weekdayLabel,
} from "./schedule.ts";

test("cron 字段拆分：5 字段直接拆，6/7 字段丢掉秒位", () => {
  assert.deepEqual(cronFields("0 9 * * *"), {
    minute: "0",
    hour: "9",
    dom: "*",
    month: "*",
    dow: "*",
  });
  assert.deepEqual(cronFields("0 30 8 * * 1-5"), {
    minute: "30",
    hour: "8",
    dom: "*",
    month: "*",
    dow: "1-5",
  });
  assert.deepEqual(cronFields("0 0 9 * * * 2027"), {
    minute: "0",
    hour: "9",
    dom: "*",
    month: "*",
    dow: "*",
  });
  assert.equal(cronFields("0 9 *"), null);
  assert.equal(cronFields(""), null);
});

test("cron 校验：字段数不对要报错", () => {
  assert.equal(validateCron("0 9 * * *"), null);
  assert.equal(validateCron("*/15 * * * *"), null);
  assert.equal(validateCron("15 8 * * 1-5"), null);
  assert.equal(validateCron("0 30 8 * * 1-5"), null);
  assert.ok(validateCron("0 9 *")?.includes("5~7"));
  assert.ok(validateCron("a b c d e f g h")?.includes("5~7"));
});

test("cron 校验：空串算合法（空 = 不调度）", () => {
  assert.equal(validateCron(""), null);
  assert.equal(validateCron("   "), null);
});

test("cron 校验：越界的数字要拦下来", () => {
  assert.ok(validateCron("60 9 * * *")?.includes("分钟"));
  assert.ok(validateCron("0 24 * * *")?.includes("小时"));
  assert.ok(validateCron("0 9 32 * *")?.includes("日"));
  assert.ok(validateCron("0 9 * 13 *")?.includes("月"));
  assert.ok(validateCron("0 9 * * 8")?.includes("星期"));
});

test("cron 校验：非法字符要拦下来", () => {
  assert.ok(validateCron("0 9 * * abc") !== null);
  assert.ok(validateCron("0 9 * * ")?.includes("5~7"));
});

test("cron 说明：与后端 CronShape 同口径", () => {
  assert.equal(describeCron("*/15 * * * *"), "每 15 分钟");
  assert.equal(describeCron("* * * * *"), "每 1 分钟", "每分钟不能被降级成自定义");
  assert.equal(describeCron("30 * * * *"), "每小时第 30 分钟");
  assert.equal(describeCron("0 9 * * *"), "每天 09:00");
  assert.equal(describeCron("15 8 * * 1-5"), "每周 周一至周五 08:15");
  assert.equal(describeCron("0 9 1 * *"), "每月 1 日 09:00");
  // 认不出来的不猜
  assert.ok(describeCron("0 9 1 11 *").startsWith("自定义"));
  assert.equal(describeCron(""), "未设置调度");
  assert.equal(describeCron(null), "未设置调度");
  // 6 字段（秒位在前）也要能说明
  assert.equal(describeCron("0 0 9 * * *"), "每天 09:00");
});

test("星期展开：区间 / 列表 / 跨周末", () => {
  assert.deepEqual(expandWeekdays("1-5"), [1, 2, 3, 4, 5]);
  assert.deepEqual(expandWeekdays("1,3,5"), [1, 3, 5]);
  assert.deepEqual(expandWeekdays("0"), [0]);
  assert.deepEqual(expandWeekdays("7"), [7]);
  // 5-0 = 周五、周六、周日
  assert.deepEqual(expandWeekdays("5-0"), [5, 6, 7, 0]);
});

test("星期说明：连续区间合并成「至」", () => {
  assert.equal(weekdayLabel("1-5"), "周一至周五");
  assert.equal(weekdayLabel("0"), "周日");
  assert.equal(weekdayLabel("7"), "周日", "cron 里 7 和 0 都是周日");
  assert.equal(weekdayLabel("6"), "周六");
  assert.equal(weekdayLabel("1,3,5"), "周一、周三、周五");
});

test("时间显示：空值是破折号，格式是 MM-DD HH:mm:ss", () => {
  assert.equal(formatClock(null), "—");
  assert.equal(formatClock(undefined), "—");
  const ts = Math.floor(Date.now() / 1000);
  assert.match(formatClock(ts), /^\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
});

test("相对时间：过去 / 将来 / 刚刚", () => {
  const now = 1_000_000;
  assert.equal(formatRelative(null, now), "—");
  assert.equal(formatRelative(now - 30, now), "刚刚");
  assert.equal(formatRelative(now - 180, now), "3 分钟前");
  assert.equal(formatRelative(now + 180, now), "3 分钟后");
  assert.equal(formatRelative(now - 7200, now), "2 小时前");
  assert.equal(formatRelative(now + 7200, now), "2 小时后");
  assert.equal(formatRelative(now - 86400 * 3, now), "3 天前");
});

test("系统兜底说明", () => {
  assert.equal(describeSystemTask(false, null), "系统兜底：未注册");
  assert.equal(describeSystemTask(true, null), "系统兜底：已注册");
  assert.equal(describeSystemTask(true, "每天 09:00"), "系统兜底：已注册（每天 09:00）");
});

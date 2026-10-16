import assert from "node:assert/strict";
import { test } from "node:test";
import { dateOnly, hourMinute, setLocale, timeOfDay, timeTicks } from "./time.ts";

test("time axis labels drop seconds on whole minutes and date each new day", () => {
  setLocale("en-GB");
  const noon = new Date(2026, 9, 9, 12, 0, 0).getTime() / 1000;
  const sixHours = [0, 1, 2, 3].map((i) => noon + i * 6 * 3600);
  const labels = timeTicks(sixHours);
  assert.equal(labels[0], `${hourMinute(sixHours[0])}\n${dateOnly(sixHours[0])}`);
  assert.equal(labels[1], hourMinute(sixHours[1]));
  assert.equal(labels[2], `${hourMinute(sixHours[2])}\n${dateOnly(sixHours[2])}`);
  // Within one day there is no date, and seconds show where ticks need them.
  assert.deepEqual(timeTicks([noon, noon + 30]), [timeOfDay(noon), timeOfDay(noon + 30)]);
});

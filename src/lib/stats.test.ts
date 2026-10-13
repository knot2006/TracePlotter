import assert from "node:assert/strict";
import { test } from "node:test";
import {
  breaches,
  jitterSeries,
  lossSeries,
  lowerBound,
  mos,
  mtrReport,
  parseSpan,
  spanLabel,
  stats,
} from "./stats.ts";

test("time spans are parsed from short forms and written out in words", () => {
  assert.equal(parseSpan("30s"), 30);
  assert.equal(parseSpan(" 5 min "), 300);
  assert.equal(parseSpan("1.5h"), 5400);
  assert.equal(parseSpan("2 Days"), 172_800);
  assert.equal(parseSpan("15"), 900);
  assert.equal(parseSpan("3s"), 10);
  assert.equal(parseSpan("all"), 0);
  assert.equal(parseSpan("soon"), null);
  assert.equal(parseSpan("5x"), null);
  assert.equal(parseSpan(""), null);
  assert.equal(spanLabel(30), "30 seconds");
  assert.equal(spanLabel(60), "1 minute");
  assert.equal(spanLabel(5400), "90 minutes");
  assert.equal(spanLabel(7200), "2 hours");
  assert.equal(spanLabel(0), "All");
  assert.equal(parseSpan(spanLabel(600)), 600);
});

test("lossSeries gives each probe the loss of its time bucket", () => {
  assert.deepEqual(lossSeries([10, 11, 12, 13, 14], [5, null, 5, 5, null], 2), [50, 50, 0, 0, 100]);
});

test("jitterSeries is a running average of the change between replies", () => {
  assert.deepEqual(jitterSeries([10, null, 14, 11, null]), [null, null, 4, 4 + (3 - 4) / 16, null]);
  // One spike in a steady series moves it by a sixteenth of the spike, then it decays.
  const spiked = jitterSeries([10, 10, 10, 90, 10, 10]);
  assert.deepEqual(spiked.slice(0, 4), [null, 0, 0, 5]);
  assert.ok(spiked[5]! < spiked[4]!);
});

test("lowerBound finds the first time at or after the cutoff", () => {
  assert.equal(lowerBound([1, 2, 4, 8], 3), 2);
  assert.equal(lowerBound([1, 2, 4, 8], 4), 2);
  assert.equal(lowerBound([1, 2, 4, 8], 0), 0);
  assert.equal(lowerBound([1, 2, 4, 8], 9), 4);
  assert.equal(lowerBound([], 1), 0);
});

test("mtr report has one aligned row per hop", () => {
  const lines = mtrReport("Title", [
    { ttl: 1, addr: "10.0.0.1", name: "gw", rtts: [10, null, 20, 14, null] },
    { ttl: 2, addr: null, name: null, rtts: [null] },
  ]).split("\n");
  assert.equal(lines[0], "Title");
  assert.deepEqual(lines[3].trim().split(/\s+/), [
    "1", "10.0.0.1", "40.0%", "5", "-", "14.7", "10.0", "20.0", "8.0", "1.00", "gw",
  ]);
  assert.deepEqual(lines[4].trim().split(/\s+/), ["2", "*", "100.0%", "1", "-", "-", "-", "-", "-", "-"]);
  assert.equal(lines[2].indexOf("Loss%") + 5, lines[3].indexOf("40.0%") + 5);
});

test("mos falls with latency and loss", () => {
  const good = mos(stats([20, 20, 20]))!;
  assert.ok(Math.abs(good - 4.39) < 0.01, `good link scored ${good}`);
  assert.ok(mos(stats([300, 300, 300]))! < good);
  assert.equal(mos(stats([20, null, null, null, 20])), 1);
  assert.equal(mos(stats([null])), null);
});

test("breaches looks at the latest probes only", () => {
  const calm = Array(10).fill(20);
  assert.equal(breaches([...Array(10).fill(500), ...calm], 100, 0), false);
  assert.equal(breaches([...calm, ...Array(10).fill(500)], 100, 0), true);
  assert.equal(breaches([...Array(8).fill(20), null, null], 0, 20), true);
  assert.equal(breaches([...Array(9).fill(20), null], 0, 20), false);
  assert.equal(breaches(Array(10).fill(null), 0, 0), false);
});

test("stats over replies and losses", () => {
  assert.deepEqual(stats([10, null, 20, 14, null]), {
    sent: 5,
    lossPct: 40,
    last: null,
    avg: (10 + 20 + 14) / 3,
    min: 10,
    max: 20,
    jitter: (10 + 6) / 2,
  });
});

test("stats with nothing received", () => {
  assert.deepEqual(stats([null, null]), {
    sent: 2,
    lossPct: 100,
    last: null,
    avg: null,
    min: null,
    max: null,
    jitter: null,
  });
  assert.equal(stats([]).lossPct, 0);
});

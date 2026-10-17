import assert from "node:assert/strict";
import { test } from "node:test";
import {
  alertProbes,
  alignLines,
  breaches,
  jitterSeries,
  latencyAxis,
  lossSeries,
  lowerBound,
  mos,
  niceCeil,
  parseSpan,
  rangeGraph,
  spanLabel,
  spanShort,
  stretchWidth,
  stats,
  tallyStats,
  textTable,
  thin,
  zoomSpan,
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
  assert.equal(spanShort(5400), "90m");
  assert.equal(spanShort(0), "all");
  assert.equal(parseSpan(spanShort(21600)), 21600);
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

test("niceCeil rounds an axis end up to a round number", () => {
  assert.equal(niceCeil(27), 30);
  assert.equal(niceCeil(18.75), 20);
  assert.equal(niceCeil(105), 150);
  assert.equal(niceCeil(63.6), 70);
  assert.equal(niceCeil(82.9), 90);
  assert.equal(niceCeil(0.45), 0.5);
  assert.equal(niceCeil(30), 30);
  assert.equal(niceCeil(950), 1000);
  assert.equal(niceCeil(0), 1);
});

test("textTable aligns each column to its widest cell", () => {
  const table = textTable(
    "Title",
    [{ label: "Hop" }, { label: "IP", left: true }, { label: "Loss" }],
    [
      ["1", "10.0.0.1", "0.0%"],
      ["RT", "Round trip", "12.5%"],
    ],
  );
  assert.equal(table, ["Title", "", "Hop  IP           Loss", "  1  10.0.0.1     0.0%", " RT  Round trip  12.5%", ""].join("\n"));
});

test("mos falls with latency and loss", () => {
  const good = mos(stats([20, 20, 20]))!;
  assert.ok(Math.abs(good - 4.39) < 0.01, `good link scored ${good}`);
  assert.ok(mos(stats([300, 300, 300]))! < good);
  assert.equal(mos(stats([20, null, null, null, 20])), 1);
  assert.equal(mos(stats([null])), null);
});

test("breaches looks at the latest probes only, once there are enough of them", () => {
  const calm = Array(10).fill(20);
  assert.equal(breaches([...Array(10).fill(500), ...calm], 10, 100, 0), false);
  assert.equal(breaches([...calm, ...Array(10).fill(500)], 10, 100, 0), true);
  assert.equal(breaches([...Array(8).fill(20), null, null], 10, 0, 20), true);
  assert.equal(breaches([...Array(9).fill(20), null], 10, 0, 20), false);
  assert.equal(breaches(Array(10).fill(null), 10, 0, 0), false);
  assert.equal(breaches([null], 10, 0, 5), false);
  assert.equal(breaches([...Array(29).fill(20), null], 30, 0, 5), false);
});

test("an alert window covers its seconds at the tab's interval", () => {
  assert.equal(alertProbes(30, 1000), 30);
  assert.equal(alertProbes(30, 2500), 12);
  assert.equal(alertProbes(1, 10_000), 1);
});

test("stats over replies and losses", () => {
  assert.deepEqual(stats([10, null, 20, 14, null]), {
    sent: 5,
    received: 3,
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
    received: 0,
    lossPct: 100,
    last: null,
    avg: null,
    min: null,
    max: null,
    jitter: null,
  });
  assert.equal(stats([]).lossPct, 0);
});

test("thin averages each run, keeps its lowest and highest, gaps and breaks", () => {
  const data = (ys: (number | null)[]) => ({
    xs: ys.map((_, i) => i),
    ys,
    js: ys.map((y) => (y === null ? null : y * 2)),
    ls: ys.map(() => 0),
  });
  const short = data([1, 2, 3]);
  assert.deepEqual(thin(short, new Set(), 0), { ...short, lo: short.ys, hi: short.ys });
  // Stretches of 4 s, each point at its first time.
  const long = thin(data([3, 9, 2, 6, 5, 1, null, 6]), new Set(), 4);
  assert.deepEqual(long.xs, [0, 4]);
  assert.deepEqual(long.ys, [5, 4]);
  assert.deepEqual(long.lo, [2, 1]);
  assert.deepEqual(long.hi, [9, 6]);
  assert.deepEqual(long.js, [10, 8]);
  // A run of only lost probes stays lost.
  assert.deepEqual(thin(data([null, null, null, null, 5, 5, 5, 5]), new Set(), 4).ys, [null, 5]);
  // A break point is kept, lost, and cuts the stretch it falls in.
  const broken = thin(data([1, 3, 0, 3, 4, 5, 6, 7]), new Set([2]), 2);
  assert.deepEqual(broken.xs, [0, 2, 3, 4, 6]);
  assert.deepEqual(broken.ys, [2, null, 3, 4.5, 6.5]);
});

test("thin keeps its stretches in place while the period moves on", () => {
  const ys = [5, 9, 1, 7, 3, 8, 2, 6, 4, 10];
  const at = (from: number) => ({
    xs: ys.slice(from).map((_, i) => from + i),
    ys: ys.slice(from),
    js: ys.slice(from).map(() => null),
    ls: ys.slice(from).map(() => 0),
  });
  // One round later the first stretch has lost a probe, but the others hold the same ones.
  const before = thin(at(0), new Set(), 3);
  const after = thin(at(1), new Set(), 3);
  assert.deepEqual(after.xs.slice(1), before.xs.slice(1));
  assert.deepEqual(after.ys.slice(1), before.ys.slice(1));
});

test("thin with a radius blends neighbours by triangle weights, not across a break", () => {
  const xs = Array.from({ length: 12 }, (_, i) => i);
  // A wave that repeats every two probes; runs of one keep it, the blend flattens it.
  const ys = xs.map((i) => (i % 2 ? 20 : 10));
  const data = { xs, ys, js: ys.map(() => null), ls: ys.map(() => 0) };
  const blended = thin(data, new Set(), 2, 2).ys;
  assert.ok(blended.slice(2, -2).every((y) => y !== null && Math.abs(y - 15) < 2.6), String(blended));
  // The band keeps the extremes of each run.
  assert.deepEqual(thin(data, new Set(), 2, 2).lo.slice(0, 2), [10, 10]);
  // A break stops the blend: the points before it do not see the high values after it.
  const steps = { ...data, ys: [10, 10, 10, 10, 10, null, 50, 50, 50, 50, 50, 50] };
  // Stretches of 2 s: points at 0, 2, 4, the break at 5, then 6, 8, 10.
  const split = thin(steps, new Set([5]), 2, 3);
  assert.deepEqual(split.xs, [0, 2, 4, 5, 6, 8, 10]);
  assert.deepEqual(split.ys, [10, 10, 10, null, 50, 50, 50]);
});

test("tallies from the history files read like samples, and graph with breaks where none were kept", () => {
  const tally = (t: number, rtts: number[], sent = rtts.length) => ({
    t,
    sent,
    received: rtts.length,
    sum: rtts.reduce((a, b) => a + b, 0),
    min: rtts.length ? Math.min(...rtts) : null,
    max: rtts.length ? Math.max(...rtts) : null,
    jitter_sum: 2,
    jitter_count: 1,
    last: rtts.at(-1) ?? null,
  });
  assert.deepEqual(tallyStats(tally(0, [10, 20], 4)), {
    sent: 4,
    received: 2,
    lossPct: 50,
    last: 20,
    avg: 15,
    min: 10,
    max: 20,
    jitter: 2,
  });
  // Minutes 0, 1 and 3: minute 2 is missing, so a break sits right after minute 1.
  const { data, breaks } = rangeGraph([tally(0, [10, 30]), tally(60_000, [20], 2), tally(180_000, [40])], 60, 120, 0, 1);
  assert.deepEqual(data.xs, [0, 60, 120, 180]);
  assert.deepEqual(data.ys, [20, 20, null, 40]);
  assert.deepEqual(data.lo, [10, 20, null, 40]);
  assert.deepEqual([...breaks], [120]);
  // Loss over two-minute stretches: one of four probes in the first.
  assert.deepEqual(data.ls, [25, 25, null, 0]);
});

test("the latency axis starts near the lowest reply only when 0 would leave much empty", () => {
  // Replies from 245 to 475 ms: below 245 would be empty.
  assert.deepEqual(latencyAxis(245, 475), [150, 550]);
  // A steady line keeps a margin of a tenth of its latency on both sides.
  assert.deepEqual(latencyAxis(305, 312), [270, 350]);
  // Replies near 0 keep it, and so do small wobbles on small latencies.
  assert.deepEqual(latencyAxis(1, 30), [0, 40]);
  assert.deepEqual(latencyAxis(10, 30), [0, 40]);
  assert.deepEqual(latencyAxis(null, null), [0, 1]);
  assert.deepEqual(latencyAxis(95, 120), [80, 135]);
  // A steady near hop stays on an axis from 0.
  assert.deepEqual(latencyAxis(1, 2), [0, 2.5]);
});

test("a stretch is about a pixel, whole minutes from a minute up, never under the interval", () => {
  assert.equal(stretchWidth(7 * 86_400, 1500, 1), 420);
  assert.equal(stretchWidth(3600, 1500, 1), 3);
  assert.equal(stretchWidth(600, 1500, 2.5), 2.5);
  assert.equal(stretchWidth(90, 1500, 0.5), 1);
});

test("a stretch missing at an interval wider than the stretch is no break", () => {
  // A 2.5 s trace on 1 s stretches: points 2 or 3 s apart are its normal pace.
  const tally = (t: number) => ({ t, sent: 1, received: 1, sum: 10, min: 10, max: 10, jitter_sum: 0, jitter_count: 0, last: 10 });
  const steady = rangeGraph([0, 2000, 5000, 7000, 10_000].map(tally), 1, 1, 0, 2.5);
  assert.equal(steady.breaks.size, 0);
  // A minute without probes is.
  assert.equal(rangeGraph([0, 2000, 62_000].map(tally), 1, 1, 0, 2.5).breaks.size, 1);
});

test("lines with points at different times line up, holding their value where they have none", () => {
  const { xs, ys } = alignLines([
    new Map([
      [0, 1],
      [2, null],
    ]),
    new Map([
      [1, 5],
      [2, 6],
    ]),
  ]);
  assert.deepEqual(xs, [0, 1, 2]);
  assert.deepEqual(ys, [
    [1, 1, null],
    [null, 5, 6],
  ]);
});

test("Ctrl and the wheel step through round time spans, out to everything", () => {
  assert.equal(zoomSpan(600, false, 86_400), 300);
  assert.equal(zoomSpan(600, true, 86_400), 900);
  // From a span between steps, to the next one either way.
  assert.equal(zoomSpan(700, false, 86_400), 600);
  assert.equal(zoomSpan(700, true, 86_400), 900);
  // Never under 10 seconds.
  assert.equal(zoomSpan(10, false, 86_400), 10);
  // Out to the whole history is everything; in from everything is the step below the history.
  assert.equal(zoomSpan(3600, true, 5000), 0);
  assert.equal(zoomSpan(0, true, 5000), 0);
  assert.equal(zoomSpan(0, false, 5000), 3600);
});

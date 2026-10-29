import assert from "node:assert/strict";
import { test } from "node:test";
import { correlateIncidents, groupEvents, incidents, restoreEvents, type TraceEvent } from "./events.ts";

test("restoring a trace keeps saved alert decisions inside replayed history", () => {
  const old: TraceEvent = { t: 5, kind: "length", from: 2, to: 3 };
  const route: TraceEvent = { t: 20, kind: "hop", ttl: 2, from: "a", to: "b" };
  const alert: TraceEvent = { t: 15, kind: "alert", text: "slow" };
  const recovery: TraceEvent = { t: 25, kind: "recover", text: "ok" };
  assert.deepEqual(restoreEvents([old, alert, route, recovery], [route], 10), [old, alert, route, recovery]);
});

test("cross-target incidents group only overlapping observed windows", () => {
  const groups = correlateIncidents([
    { target: "a", from: 10, to: null, observedUntil: 20, text: "paused before recovery" },
    { target: "b", from: 15, to: 25, observedUntil: 30, text: "slow" },
    { target: "c", from: 40, to: 50, observedUntil: 60, text: "loss" },
  ]);
  assert.equal(groups.length, 2);
  assert.equal(groups[0].to, 25);
  assert.deepEqual(groups[0].entries.map((e) => e.target), ["a", "b"]);
});

test("incident durations pair alerts and recoveries without inventing missing starts", () => {
  assert.deepEqual(incidents([
    { t: 1, kind: "recover", text: "old recovery" },
    { t: 10, kind: "alert", text: "slow" },
    { t: 12, kind: "alert", text: "still slow" },
    { t: 40, kind: "recover", text: "ok" },
    { t: 50, kind: "alert", text: "loss" },
  ]), [
    { from: 10, to: 40, text: "slow" },
    { from: 50, to: null, text: "loss" },
  ]);
});

test("a re-routing spread over two rounds is one entry", () => {
  const events: TraceEvent[] = [
    { t: 100, kind: "length", from: 8, to: 9 },
    { t: 100, kind: "hop", ttl: 7, from: "a", to: "b" },
    { t: 101, kind: "hop", ttl: 5, from: "c", to: "d" },
    { t: 101, kind: "hop", ttl: 7, from: "b", to: "e" },
  ];
  assert.deepEqual(groupEvents(events), [
    {
      t: 100,
      kind: "route",
      length: { from: 8, to: 9 },
      hops: [
        { ttl: 5, from: "c", to: "d" },
        { ttl: 7, from: "a", to: "e" },
      ],
    },
  ]);
});

test("changes that undo themselves, later changes and alerts", () => {
  const events: TraceEvent[] = [
    { t: 100, kind: "hop", ttl: 3, from: "a", to: "b" },
    { t: 102, kind: "hop", ttl: 3, from: "b", to: "a" },
    { t: 200, kind: "hop", ttl: 3, from: "a", to: "b" },
    { t: 201, kind: "alert", text: "slow" },
    { t: 202, kind: "length", from: 9, to: 8 },
  ];
  assert.deepEqual(groupEvents(events), [
    { t: 200, kind: "route", length: null, hops: [{ ttl: 3, from: "a", to: "b" }] },
    { t: 201, kind: "alert", text: "slow" },
    { t: 202, kind: "route", length: { from: 9, to: 8 }, hops: [] },
  ]);
});

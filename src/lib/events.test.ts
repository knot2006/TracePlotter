import assert from "node:assert/strict";
import { test } from "node:test";
import { groupEvents, type TraceEvent } from "./events.ts";

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

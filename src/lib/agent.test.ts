import assert from "node:assert/strict";
import { test } from "node:test";
import { findTrace, getHops, getLatencyHistory, getRouteChanges, listTargets } from "./agent.ts";
import type { Hop, Trace } from "./trace.ts";

/** A hop with one probe a second from t = 1000, `null` = lost. */
function hop(ttl: number, addr: string, rtts: (number | null)[]): Hop {
  return { ttl, addr, name: null, times: rtts.map((_, i) => 1000 + i), rtts };
}

function trace(sessionId: string, host: string, hops: Hop[]): Trace {
  const target = { ...hops[hops.length - 1], name: host };
  return {
    id: 0,
    runId: 0,
    loaded: true,
    paused: false,
    config: { sessionId, host, protocol: "icmp", port: 0, intervalMs: 1000, alertLatencyMs: 0, alertLossPct: 0 },
    addr: target.addr!,
    hops,
    error: null,
    alerting: false,
    events: [
      { t: 1001, kind: "hop", ttl: 2, from: "10.0.0.2", to: "10.0.0.9" },
      { t: 1003, kind: "alert", text: "Last 10 probes: 50.0 ms average, 0% loss" },
    ],
    pathLength: hops.length,
    target,
    lengths: [],
    pathEnd: hops.length,
    routeNotified: 0,
    diskStart: 0,
  };
}

const home = trace("aaa", "router.lan", [hop(1, "192.168.1.1", [1, 1, 1, 1])]);
const dns = trace("bbb", "dns.example", [hop(1, "192.168.1.1", [1, 1, 1, 1]), hop(2, "10.0.0.9", [10, null, 30, 20])]);
const both = [home, dns];

test("a target is found by id, host or address, and a wrong one lists the choices", () => {
  assert.equal(findTrace(both, "bbb"), dns);
  assert.equal(findTrace(both, "DNS.example"), dns);
  assert.equal(findTrace(both, "10.0.0.9"), dns);
  assert.equal(findTrace([home], undefined), home);
  assert.throws(() => findTrace(both, undefined), /aaa \(router.lan\)/);
  assert.throws(() => findTrace(both, "nowhere"), /No open target nowhere/);
});

test("targets and hops carry the window's statistics", async () => {
  const [, second] = (await listTargets(both, {})).targets;
  assert.equal(second.round_trip.sent, 4);
  assert.equal(second.round_trip.loss_pct, 25);
  assert.equal(second.round_trip.avg_ms, 20);
  assert.equal(second.from, "1970-01-01T00:16:40.000Z");
  const { route } = await getHops(both, { target: "bbb" });
  assert.deepEqual(
    route.map((h) => [h.hop, h.address, h.loss_pct, h.max_ms]),
    [
      [1, "192.168.1.1", 0, 1],
      [2, "10.0.0.9", 25, 30],
    ],
  );
  // Only the newest minute: probes are one a second, so all four still count.
  assert.equal((await getHops(both, { target: "bbb", minutes: 1 })).route[1].sent, 4);
});

test("route changes come newest first, grouped as the window shows them", () => {
  const { events, total } = getRouteChanges(both, { target: "bbb" });
  assert.equal(total, 2);
  assert.equal(events[0].kind, "alert");
  assert.deepEqual(events[1], {
    time: "1970-01-01T00:16:41.000Z",
    kind: "route",
    hop_count: null,
    hops: [{ hop: 2, from: "10.0.0.2", to: "10.0.0.9" }],
  });
});

test("history is cut into slices that cover every probe once", async () => {
  const { slices } = await getLatencyHistory(both, { target: "bbb", slices: 2, minutes: 0 });
  assert.equal(slices.length, 2);
  assert.deepEqual(
    slices.map((s) => [s.sent, s.loss_pct, s.avg_ms]),
    [
      [2, 50, 10],
      [2, 0, 25],
    ],
  );
  assert.equal((await getLatencyHistory(both, { target: "bbb", hop: 1 })).slices[0].max_ms, 1);
  await assert.rejects(getLatencyHistory(both, { target: "bbb", hop: 5 }), /no hop 5/);
});

test("a period the window no longer holds is read from the history files", async () => {
  // The files begin an hour before the window's oldest round.
  const older = { ...dns, diskStart: 1000 - 3600 };
  const asked: number[][] = [];
  const tally = { sent: 100, received: 90, sum: 900, min: 5, max: 20, jitter_sum: 10, jitter_count: 5, last: 9 };
  const history = async (_trace: unknown, from: number, to: number, width: number, graphed: number[]) => {
    asked.push([from, to, width, graphed.length]);
    return [{ ttl: 0, total: tally, stretches: [{ ...tally, t: (1000 - 3600) * 1000 }] }];
  };
  const [target] = (await listTargets([older], { minutes: 0 }, history)).targets;
  assert.equal(target.round_trip.sent, 100);
  assert.equal(target.round_trip.loss_pct, 10);
  assert.equal(target.from, new Date((1000 - 3600) * 1000).toISOString());
  assert.equal(asked[0][3], 0);
  const { slices } = await getLatencyHistory([older], { minutes: 0, slices: 10 }, history);
  assert.deepEqual([slices.length, slices[0].avg_ms], [1, 10]);
  // Within the window (it holds 4 s), the window answers.
  assert.equal((await listTargets([older], { minutes: 0.05 }, history)).targets[0].round_trip.sent, 4);
});

import assert from "node:assert/strict";
import { test } from "node:test";
import { agentProbe, findTrace, getHops, getLatencyHistory, getRouteChanges, listTargets } from "./agent.ts";
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

test("targets and hops carry the window's statistics", () => {
  const [, second] = listTargets(both, {}).targets;
  assert.equal(second.round_trip.sent, 4);
  assert.equal(second.round_trip.loss_pct, 25);
  assert.equal(second.round_trip.avg_ms, 20);
  assert.equal(second.from, "1970-01-01T00:16:40.000Z");
  const { route } = getHops(both, { target: "bbb" });
  assert.deepEqual(
    route.map((h) => [h.hop, h.address, h.loss_pct, h.max_ms]),
    [
      [1, "192.168.1.1", 0, 1],
      [2, "10.0.0.9", 25, 30],
    ],
  );
  // Only the newest minute: probes are one a second, so all four still count.
  assert.equal(getHops(both, { target: "bbb", minutes: 1 }).route[1].sent, 4);
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

test("history is cut into slices that cover every probe once", () => {
  const { slices } = getLatencyHistory(both, { target: "bbb", slices: 2, minutes: 0 });
  assert.equal(slices.length, 2);
  assert.deepEqual(
    slices.map((s) => [s.sent, s.loss_pct, s.avg_ms]),
    [
      [2, 50, 10],
      [2, 0, 25],
    ],
  );
  assert.equal(getLatencyHistory(both, { target: "bbb", hop: 1 }).slices[0].max_ms, 1);
  assert.throws(() => getLatencyHistory(both, { target: "bbb", hop: 5 }), /no hop 5/);
});

test("start_trace arguments are checked and never probe faster than once a second", () => {
  assert.deepEqual(agentProbe({ host: " Example.COM " }, true), {
    host: "example.com",
    protocol: "winicmp",
    port: 0,
    intervalMs: 1000,
    note: undefined,
  });
  const fast = agentProbe({ host: "h", protocol: "TCP", interval_ms: 200 }, false);
  assert.equal(fast.protocol, "tcp");
  assert.equal(fast.port, 443);
  assert.equal(fast.intervalMs, 1000);
  assert.match(fast.note!, /once a second/);
  // Rounded up to an interval the toolbar offers.
  assert.equal(agentProbe({ host: "h", interval_ms: 3000 }, false).intervalMs, 5000);
  assert.equal(agentProbe({ host: "h", interval_ms: 600_000 }, false).intervalMs, 10000);
  assert.throws(() => agentProbe({}, false), /needs a host/);
  assert.throws(() => agentProbe({ host: "h", protocol: "winicmp" }, false), /not available/);
  assert.throws(() => agentProbe({ host: "h", protocol: "constructor" }, true), /not available/);
  assert.throws(() => agentProbe({ host: "h", protocol: "udp", port: "abc" }, true), /between 1 and 65535/);
  assert.throws(() => agentProbe({ host: "h", protocol: "tcp", port: 70000 }, true), /between 1 and 65535/);
});

test("windows end at the trace's latest round, so a hop that left the route has no recent probes", () => {
  const left = hop(3, "10.0.0.3", [5, 5]);
  left.times = [100, 101];
  const longer = { ...dns, hops: [...dns.hops, left] };
  assert.equal(getHops([longer], { minutes: 1 }).route.length, 2);
  assert.throws(() => getLatencyHistory([longer], { hop: 3 }), /no hop 3; its route has 2/);
  assert.equal(getLatencyHistory([longer], { hop: "2", minutes: 0 }).hop, 2);
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { runInNewContext } from "node:vm";
import { test } from "node:test";
import { describeTrace } from "./agent.ts";

// Exercise the page's actual mutating tool, with probing and persistence replaced by fakes.
const page = readFileSync(new URL("../routes/+page.svelte", import.meta.url), "utf8");
const source = stripTypeScriptTypes(page.slice(page.indexOf("  async function agentStart("), page.indexOf("  /** Pauses a tab for an agent")));
function harness(existing: boolean) {
  const trace = {
    config: { sessionId: "one", host: "example.com", protocol: "icmp", port: 0, intervalMs: 1000, alertLatencyMs: 0, alertLossPct: 0 },
    addr: "192.0.2.1", paused: existing, alerting: false, error: null, hops: [], pathEnd: 1, diskStart: 0,
    target: { ttl: 1, addr: "192.0.2.1", name: null, times: [1000, 1001], rtts: [10, 20] },
  };
  let saved = 0;
  let started: Record<string, unknown> | undefined;
  const call = runInNewContext(`${source}\nagentStart`, {
    traces: existing ? [trace] : [], session: [], saveSession: () => saved++,
    usesPort: (protocol: string) => protocol === "tcp" || protocol === "udp",
    SYSTEM_ICMP: false, PROTOCOLS: { icmp: {}, tcp: {}, udp: {} }, DEFAULT_PORT: { tcp: 443, udp: 33434 },
    AGENT_MIN_INTERVAL_MS: 1000, AGENT_INTERVALS: [1000, 2500, 5000, 10000],
    same: (a: typeof trace.config, b: typeof trace.config) => a.host === b.host && a.protocol === b.protocol && a.port === b.port,
    togglePause: async () => { trace.paused = false; }, describeTrace,
    newConfig: (target: Record<string, unknown>, intervalMs: number) => ({ ...trace.config, ...target, intervalMs }),
    startTrace: async (config: Record<string, unknown>) => { started = config; return { ...trace, config }; }, bump: () => {},
  }) as (args: Record<string, unknown>) => Promise<Record<string, unknown>>;
  return { call, trace, saved: () => saved, started: () => started };
}
test("starting through MCP awaits the statistics and enforces the one-second minimum", async () => {
  const h = harness(false);
  const answer = await h.call({ host: " EXAMPLE.com ", protocol: "icmp", interval_ms: 100 });
  assert.equal(answer.started, "new tab");
  assert.equal(answer.address, "192.0.2.1");
  assert.equal((answer.round_trip as { avg_ms: number }).avg_ms, 15);
  assert.equal(h.started()?.host, "example.com");
  assert.equal(h.started()?.intervalMs, 1000);
  assert.match(String(answer.note), /not every 100 ms/);
  assert.equal(h.saved(), 1);
});
test("resuming through MCP returns the same target description", async () => {
  const h = harness(true);
  const answer = await h.call({ host: "example.com", protocol: "icmp" });
  assert.equal(answer.started, "resumed");
  assert.equal(answer.id, "one");
  assert.equal(answer.state, "running");
  assert.equal(h.trace.paused, false);
});

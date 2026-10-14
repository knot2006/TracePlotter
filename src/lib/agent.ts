// The answers of the MCP server's read-only tools (see src-tauri/src/mcp.rs), made from the
// open tabs with the same calculations the window uses. Times go out as UTC ISO strings,
// latencies in ms rounded to 0.01.
import { groupEvents } from "./events.ts";
import { type HopStats, lowerBound, mos, stats } from "./stats.ts";
import type { Hop, Protocol, Trace } from "./trace.ts";

/** Arguments of a tool call, as the agent sent them. */
export type Arguments = Record<string, unknown>;

const DEFAULT_MINUTES = 10;
/** The window start_trace and stop_trace report on, in seconds. */
export const DEFAULT_SPAN = DEFAULT_MINUTES * 60;
const DEFAULT_HISTORY_MINUTES = 60;
const DEFAULT_SLICES = 30;
const DEFAULT_EVENTS = 50;

const round = (value: number | null) => (value === null ? null : Math.round(value * 100) / 100);
const iso = (seconds: number) => new Date(seconds * 1000).toISOString();

/** A whole number argument within limits, or its default when missing or unreadable. Numbers sent as text count. */
function count(value: unknown, fallback: number, min: number, max: number) {
  const number = typeof value === "string" && value.trim() ? Number(value) : value;
  return typeof number === "number" && Number.isFinite(number) ? Math.min(max, Math.max(min, Math.round(number))) : fallback;
}

/** Minutes as asked, in seconds; 0 is the whole history. */
function seconds(value: unknown, fallback: number) {
  return (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : fallback) * 60;
}

/**
 * A hop's probes in the newest `span` seconds of the trace, all of them for 0. The span ends
 * at the trace's latest round, as in the window, so a hop that left the route has none.
 */
function newest(trace: Trace, hop: Hop, span: number): Hop {
  const latest = trace.target.times.at(-1);
  if (!span || latest === undefined) return hop;
  const from = lowerBound(hop.times, latest - span);
  return { ...hop, times: hop.times.slice(from), rtts: hop.rtts.slice(from) };
}

function numbers(s: HopStats) {
  return {
    sent: s.sent,
    received: s.received,
    loss_pct: round(s.lossPct),
    last_ms: round(s.last),
    avg_ms: round(s.avg),
    min_ms: round(s.min),
    max_ms: round(s.max),
    jitter_ms: round(s.jitter),
    mos: round(mos(s)),
  };
}

/**
 * The tab an agent means: by session id, host or address. Left out, the only open tab.
 * Throws a message that tells the agent what it can ask for instead.
 */
export function findTrace(traces: Trace[], target: unknown): Trace {
  const known = () => traces.map((t) => `${t.config.sessionId} (${t.config.host})`).join(", ") || "none";
  if (target === undefined || target === null || target === "") {
    if (traces.length === 1) return traces[0];
    throw new Error(`Say which target: ${known()}`);
  }
  const wanted = String(target).trim().toLowerCase();
  const byId = traces.find((t) => t.config.sessionId === wanted);
  if (byId) return byId;
  const matches = traces.filter((t) => t.config.host === wanted || t.addr === wanted);
  if (matches.length === 1) return matches[0];
  if (matches.length > 1) throw new Error(`More than one tab traces ${wanted}; use an id: ${known()}`);
  throw new Error(`No open target ${wanted}. Open targets: ${known()}`);
}

/** What list_targets says about one tab, with the round trip over the newest `span` seconds. */
export function describeTrace(trace: Trace, span: number) {
  const { config } = trace;
  const target = newest(trace, trace.target, span);
  const usesPort = config.protocol === "udp" || config.protocol === "tcp";
  return {
    id: config.sessionId,
    host: config.host,
    address: trace.addr || null,
    state: trace.paused ? "paused" : trace.alerting ? "alerting" : "running",
    error: trace.error,
    probe: {
      protocol: config.protocol,
      ...(usesPort ? { port: config.port } : {}),
      interval_ms: config.intervalMs,
    },
    hops: trace.pathEnd || null,
    alert_thresholds: { latency_ms: config.alertLatencyMs || null, loss_pct: config.alertLossPct || null },
    from: target.times.length ? iso(target.times[0]) : null,
    to: target.times.length ? iso(target.times[target.times.length - 1]) : null,
    round_trip: numbers(stats(target.rtts)),
  };
}

export function listTargets(traces: Trace[], args: Arguments) {
  const span = seconds(args.minutes, DEFAULT_MINUTES);
  return { targets: traces.map((trace) => describeTrace(trace, span)) };
}

/** Every hop on the current route, then the round trip to the target, as the hop table shows them. */
export function getHops(traces: Trace[], args: Arguments) {
  const trace = findTrace(traces, args.target);
  const span = seconds(args.minutes, DEFAULT_MINUTES);
  // Hops past the end of a route that got shorter get no more probes; the table leaves them out too.
  const route = trace.hops.slice(0, trace.pathEnd || trace.hops.length);
  return {
    ...describeTrace(trace, span),
    route: route.map((hop) => ({
      hop: hop.ttl,
      address: hop.addr,
      name: hop.name,
      ...numbers(stats(newest(trace, hop, span).rtts)),
    })),
  };
}

export function getRouteChanges(traces: Trace[], args: Arguments) {
  const trace = findTrace(traces, args.target);
  const limit = count(args.limit, DEFAULT_EVENTS, 1, 500);
  const groups = groupEvents(trace.events);
  return {
    id: trace.config.sessionId,
    host: trace.config.host,
    total: groups.length,
    events: groups
      .slice(-limit)
      .reverse()
      .map((group) =>
        group.kind === "route"
          ? {
              time: iso(group.t),
              kind: "route",
              hop_count: group.length,
              hops: group.hops.map(({ ttl, from, to }) => ({ hop: ttl, from, to })),
            }
          : { time: iso(group.t), kind: group.kind, text: group.text },
      ),
  };
}

/** The newest stretch of one series cut into equal slices, each summed up. */
export function getLatencyHistory(traces: Trace[], args: Arguments) {
  const trace = findTrace(traces, args.target);
  let series = trace.target;
  if (args.hop !== undefined) {
    // Only hops of the current route, as get_hops lists them.
    const route = trace.pathEnd || trace.hops.length;
    const ttl = count(args.hop, 0, 0, 255);
    if (ttl < 1 || ttl > route) throw new Error(`${trace.config.host} has no hop ${args.hop}; its route has ${route}`);
    series = trace.hops[ttl - 1];
  }
  const span = seconds(args.minutes, DEFAULT_HISTORY_MINUTES);
  const { times, rtts } = newest(trace, series, span);
  const base = { id: trace.config.sessionId, host: trace.config.host, hop: args.hop === undefined ? "round trip" : series.ttl };
  if (!times.length) return { ...base, slices: [] };
  const start = times[0];
  const end = times[times.length - 1];
  const parts = count(args.slices, DEFAULT_SLICES, 1, 300);
  const width = Math.max((end - start) / parts, 1e-9);
  const slices: { from: string; to: string; [key: string]: unknown }[] = [];
  let first = 0;
  for (let i = 0; i < parts && first < times.length; i++) {
    const until = i === parts - 1 ? Infinity : start + width * (i + 1);
    const last = Math.max(first, lowerBound(times, until));
    if (last === first) continue;
    const s = stats(rtts.slice(first, last));
    slices.push({
      from: iso(times[first]),
      to: iso(times[last - 1]),
      sent: s.sent,
      loss_pct: round(s.lossPct),
      avg_ms: round(s.avg),
      min_ms: round(s.min),
      max_ms: round(s.max),
      jitter_ms: round(s.jitter),
    });
    first = last;
  }
  return { ...base, slices };
}

/** The read-only tools by name. start_trace and stop_trace change tabs, so the page runs those. */
export const READ_TOOLS: Record<string, (traces: Trace[], args: Arguments) => unknown> = {
  list_targets: listTargets,
  get_hops: getHops,
  get_route_changes: getRouteChanges,
  get_latency_history: getLatencyHistory,
};

/** Fastest interval an agent may use: many routers throttle replies to more than one probe a second. */
export const AGENT_MIN_INTERVAL_MS = 1000;
/** The intervals the toolbar offers that agents may use. */
const AGENT_INTERVALS = [1000, 2500, 5000, 10000];
const PROTOCOLS: Protocol[] = ["winicmp", "icmp", "udp", "tcp"];
const DEFAULT_PORTS: Record<Protocol, number> = { winicmp: 0, icmp: 0, udp: 33434, tcp: 443 };

/**
 * What start_trace's arguments ask for, checked: the host, a protocol this machine has,
 * a valid port, and an interval of at least a second. `note` tells the agent when it got a
 * slower interval than it asked for, so that it does not take that for a fault.
 */
export function agentProbe(args: Arguments, windows: boolean) {
  const host = String(args.host ?? "").trim().toLowerCase();
  if (!host) throw new Error("start_trace needs a host");
  const named = String(args.protocol ?? (windows ? "winicmp" : "icmp")).trim().toLowerCase();
  const protocol = PROTOCOLS.find((p) => p === named && (windows || p !== "winicmp"));
  if (!protocol) throw new Error(`protocol ${named} is not available here; use ${PROTOCOLS.filter((p) => windows || p !== "winicmp").join(", ")}`);
  let port = 0;
  if (protocol === "udp" || protocol === "tcp") {
    port = args.port === undefined ? DEFAULT_PORTS[protocol] : Number(args.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(`port ${args.port} is not between 1 and 65535`);
  }
  const asked = count(args.interval_ms, AGENT_MIN_INTERVAL_MS, 0, 3_600_000);
  const intervalMs = AGENT_INTERVALS.find((ms) => ms >= asked) ?? AGENT_INTERVALS[AGENT_INTERVALS.length - 1];
  const note =
    asked < AGENT_MIN_INTERVAL_MS
      ? `Probing once a second, not every ${asked} ms: many routers throttle faster probes.`
      : undefined;
  return { host, protocol, port, intervalMs, note };
}

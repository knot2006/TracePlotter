// The answers of the MCP server's read-only tools (see src-tauri/src/mcp.rs), made from the
// open tabs with the same calculations the window uses. Times go out as UTC ISO strings,
// latencies in ms rounded to 0.01.
import { groupEvents } from "./events.ts";
import { DEFAULTS } from "./settings.ts";
import { type HopStats, lowerBound, mos, period, stats, stretchWidth, tallyStats } from "./stats.ts";
import type { Hop, RangeHop, Trace } from "./trace.ts";

/** Arguments of a tool call, as the agent sent them. */
export type Arguments = Record<string, unknown>;

/**
 * Reads a period of a trace's history files, as `historyRange` in trace.ts does: from,
 * to and width in seconds, and the hop numbers (0 for the round trip) to get stretches of.
 * Passed in, so that the tools can be tried without the backend.
 */
export type History = (trace: Trace, from: number, to: number, width: number, graphed: number[]) => Promise<RangeHop[]>;

/** The tallies of a period the window no longer holds, by hop number, or null when it holds it. */
async function older(trace: Trace, span: number, history?: History) {
  const p = period(trace, span);
  if (!p.older || !history) return null;
  // One stretch over the whole period, in whole minutes, which the backend keeps tallied.
  const width = Math.max(60, Math.ceil((p.to - p.from) / 60) * 60);
  const hops = await history(trace, p.from, p.to, width, []);
  return { ...p, totals: new Map(hops.map((hop) => [hop.ttl, tallyStats(hop.total)])) };
}

const DEFAULT_MINUTES = 10;
const DEFAULT_HISTORY_MINUTES = 60;
const DEFAULT_SLICES = 30;
const DEFAULT_EVENTS = 50;

const round = (value: number | null) => (value === null ? null : Math.round(value * 100) / 100);
const iso = (seconds: number) => new Date(seconds * 1000).toISOString();

/** A whole number argument within limits, or its default when missing or unreadable. */
function count(value: unknown, fallback: number, min: number, max: number) {
  return typeof value === "number" && Number.isFinite(value) ? Math.min(max, Math.max(min, Math.round(value))) : fallback;
}

/** Minutes as asked, in seconds; 0 is the whole history. */
function seconds(value: unknown, fallback: number) {
  return (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : fallback) * 60;
}

/** The newest `span` seconds of a hop's probes, all of them for 0. */
function newest(hop: Hop, span: number): Hop {
  if (!span || !hop.times.length) return hop;
  const from = lowerBound(hop.times, hop.times[hop.times.length - 1] - span);
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
export async function describeTrace(trace: Trace, span: number, history?: History) {
  const { config } = trace;
  const target = newest(trace.target, span);
  const disk = await older(trace, span, history);
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
    alert_thresholds: {
      latency_ms: config.alertLatencyMs || null,
      loss_pct: config.alertLossPct || null,
      window_s: config.alertSeconds ?? DEFAULTS.alertSeconds,
    },
    from: disk ? iso(disk.from) : target.times.length ? iso(target.times[0]) : null,
    to: target.times.length ? iso(target.times[target.times.length - 1]) : null,
    round_trip: numbers(disk?.totals.get(0) ?? stats(target.rtts)),
  };
}

export async function listTargets(traces: Trace[], args: Arguments, history?: History) {
  const span = seconds(args.minutes, DEFAULT_MINUTES);
  return { targets: await Promise.all(traces.map((trace) => describeTrace(trace, span, history))) };
}

/** Every hop on the current route, then the round trip to the target, as the hop table shows them. */
export async function getHops(traces: Trace[], args: Arguments, history?: History) {
  const trace = findTrace(traces, args.target);
  const span = seconds(args.minutes, DEFAULT_MINUTES);
  const disk = await older(trace, span, history);
  // Hops past the end of a route that got shorter get no more probes; the table leaves them out too.
  const route = trace.hops.slice(0, trace.pathEnd || trace.hops.length);
  return {
    ...(await describeTrace(trace, span, history)),
    route: route.map((hop) => ({
      hop: hop.ttl,
      address: hop.addr,
      name: hop.name,
      ...numbers(disk ? (disk.totals.get(hop.ttl) ?? stats([])) : stats(newest(hop, span).rtts)),
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

/**
 * The newest stretch of one series cut into equal slices, each summed up. A period older
 * than the window holds is read from the history files in slices of whole stretches.
 */
export async function getLatencyHistory(traces: Trace[], args: Arguments, history?: History) {
  const trace = findTrace(traces, args.target);
  let series = trace.target;
  if (args.hop !== undefined) {
    const hop = trace.hops[count(args.hop, 0, 0, 255) - 1];
    if (!hop) throw new Error(`${trace.config.host} has no hop ${args.hop}; it has ${trace.hops.length}`);
    series = hop;
  }
  const span = seconds(args.minutes, DEFAULT_HISTORY_MINUTES);
  const base = { id: trace.config.sessionId, host: trace.config.host, hop: args.hop === undefined ? "round trip" : series.ttl };
  const parts = count(args.slices, DEFAULT_SLICES, 1, 300);
  const p = period(trace, span);
  if (p.older && history) {
    const ttl = args.hop === undefined ? 0 : series.ttl;
    const width = stretchWidth(p.to - p.from, parts, trace.config.intervalMs / 1000);
    const hops = await history(trace, p.from, p.to, width, [ttl]);
    const stretches = hops.find((hop) => hop.ttl === ttl)?.stretches ?? [];
    return {
      ...base,
      slices: stretches.map((stretch) => {
        const s = tallyStats(stretch);
        return {
          from: iso(stretch.t / 1000),
          to: iso(stretch.t / 1000 + width),
          sent: s.sent,
          loss_pct: round(s.lossPct),
          avg_ms: round(s.avg),
          min_ms: round(s.min),
          max_ms: round(s.max),
          jitter_ms: round(s.jitter),
        };
      }),
    };
  }
  const { times, rtts } = newest(series, span);
  if (!times.length) return { ...base, slices: [] };
  const start = times[0];
  const end = times[times.length - 1];
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
export const READ_TOOLS: Record<string, (traces: Trace[], args: Arguments, history?: History) => unknown> = {
  list_targets: listTargets,
  get_hops: getHops,
  get_route_changes: getRouteChanges,
  get_latency_history: getLatencyHistory,
};

/** Fastest interval an agent may ask for: many routers throttle replies to more than one probe a second. */
export const AGENT_MIN_INTERVAL_MS = 1000;

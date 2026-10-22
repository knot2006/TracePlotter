import { Channel, invoke } from "@tauri-apps/api/core";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { describe, type TraceEvent } from "./events";
import { DEFAULTS, WINDOWS, WINDOW_HOURS, historyRounds, settings } from "./settings";
import { alertProbes, breaches, stats, type Tally } from "./stats";

/**
 * `icmp`, `udp` and `tcp` probe through raw sockets; `winicmp` is ICMP through the system
 * without one: the Windows ICMP API, or Linux ping sockets.
 */
export type Protocol = "winicmp" | "icmp" | "udp" | "tcp";

/** Packet types by name, in the order they are offered. */
export const PROTOCOLS: Record<Protocol, string> = {
  winicmp: WINDOWS ? "ICMP (Windows API)" : "ICMP (ping socket)",
  icmp: "ICMP (raw socket)",
  udp: "UDP",
  tcp: "TCP",
};

export const usesPort = (protocol: Protocol) => protocol === "udp" || protocol === "tcp";

/** Whether the packet type goes through a raw socket, which needs Administrator rights on Windows and CAP_NET_RAW on Linux. */
export const usesRawSocket = (protocol: Protocol) => protocol !== "winicmp";

/** A target's packet type in a few letters: "ICMP", "ICMP·WIN", "ICMP·PING", "TCP:443". */
export const protocolLabel = ({ protocol, port }: Pick<TraceConfig, "protocol" | "port">) =>
  usesPort(protocol)
    ? `${protocol.toUpperCase()}:${port}`
    : protocol === "icmp"
      ? "ICMP"
      : WINDOWS
        ? "ICMP·WIN"
        : "ICMP·PING";

// For tabs saved before these were settings of a tab.
export const DEFAULT_TIMEOUT_MS = DEFAULTS.timeoutMs;
export const DEFAULT_PACKET_SIZE = DEFAULTS.packetSize;
export const DEFAULT_ALERT_SECONDS = DEFAULTS.alertSeconds;

/** What the session file remembers about a target. Alert thresholds of 0 are off. */
export type TraceConfig = {
  /** Unique per tab; names the tab's history file. Two tabs to one host stay separate. */
  sessionId: string;
  host: string;
  protocol: Protocol;
  port: number;
  intervalMs: number;
  /** How long a probe may go unanswered before it counts as lost; never under the interval. */
  timeoutMs?: number;
  /** Size of a probe's IP packet in bytes. TCP probes ignore it. */
  packetSize?: number;
  alertLatencyMs: number;
  alertLossPct: number;
  /** How many seconds of the latest probes an alert looks at. */
  alertSeconds?: number;
  /** The user paused the trace; it then stays paused across launches. */
  paused?: boolean;
};

export type Hop = {
  ttl: number;
  addr: string | null;
  name: string | null;
  /** Unix seconds, one entry per probe sent to this hop. */
  times: number[];
  /** Round-trip ms aligned with `times`, `null` = lost. */
  rtts: (number | null)[];
};


export type Trace = {
  /** Stable for the life of the tab. */
  id: number;
  /** Backend id of the current run; a new one after every resume. */
  runId: number;
  /** Whether the session's saved history has been replayed into this trace. */
  loaded: boolean;
  paused: boolean;
  config: TraceConfig;
  addr: string;
  /** Indexed by ttl - 1. */
  hops: Hop[];
  error: string | null;
  alerting: boolean;
  events: TraceEvent[];
  /** Last event viewed in this tab; kept across tab switches, until the app closes. */
  lastReadEvent?: TraceEvent;
  /** Hop count of the last round that reached its final hop, 0 before the first. */
  pathLength: number;
  /**
   * The target itself, whatever hop number it answers at: one entry per round, lost when
   * it did not answer. Stays continuous when the route changes length. `ttl` is the hop
   * number of its last reply, 0 before the first.
   */
  target: Hop;
  /** The latest rounds that got any reply, newest last: hop count, and whether the final hop answered. */
  lengths: { hops: number; reached: boolean }[];
  /**
   * Number of hops on the current route, from `lengths`. `hops` can be longer, holding
   * the far end of an earlier, longer route.
   */
  pathEnd: number;
  /** Unix seconds of the last route-change notification, to rate-limit them. */
  routeNotified: number;
  /**
   * Unix seconds of the oldest round in the history files, 0 when unknown. The files keep
   * more than the window holds; older periods are read from them (`history_range`).
   */
  diskStart: number;
};

export type Sample = { t: number; hops: { ttl: number; addr: string | null; rtt: number | null }[] };
type Message = ({ kind: "sample" } & Sample) | { kind: "error"; message: string };

const TRIM_BATCH = 1000;
const MAX_EVENTS = 500;

// Reverse-DNS lookups by address. One that found no name is tried again after
// NAME_RETRY_MS: it may have failed only because DNS was not reachable yet.
const NAME_RETRY_MS = 60_000;
const names = new Map<string, { name: Promise<string | null>; found: boolean; at: number }>();

/** Whether the address has no lookup yet, or a finished one without a name that is due again. */
function nameIsDue(addr: string) {
  const entry = names.get(addr);
  return !entry || (!entry.found && Date.now() - entry.at > NAME_RETRY_MS);
}

function reverseDns(addr: string) {
  if (nameIsDue(addr)) {
    // `found` stays true while the lookup runs, so it is not started twice.
    const entry = {
      name: invoke<string | null>("reverse_dns", { addr }).catch(() => null),
      found: true,
      at: Date.now(),
    };
    entry.name.then((name) => (entry.found = name !== null));
    names.set(addr, entry);
  }
  return names.get(addr)!.name;
}

/** Raises a desktop notification, unless they are switched off in the settings. */
function notify(title: string, body: string) {
  if (settings.notifications) sendNotification({ title, body });
}

/**
 * Adds an event to the trace. One that happened `live` is also saved to the trace's
 * events file, which keeps events after the window has let go of their rounds; one
 * found again while history is replayed was saved when it happened.
 */
function addEvent(trace: Trace, event: TraceEvent, live: boolean) {
  trace.events.push(event);
  if (trace.events.length > MAX_EVENTS) trace.events.shift();
  if (live) invoke("append_event", { session: trace.config.sessionId, event: JSON.stringify(event) }).catch(() => {});
}

/**
 * Puts the saved events from before the window's oldest round in front of those its
 * replayed rounds brought back, so the list reaches as far back as the history files.
 */
async function loadEvents(trace: Trace) {
  const lines = await invoke<string[]>("read_events", { session: trace.config.sessionId }).catch(() => []);
  const held = trace.target.times[0] ?? Infinity;
  const older = lines.flatMap((line) => {
    try {
      const event = JSON.parse(line) as TraceEvent;
      return event.t < held ? [event] : [];
    } catch {
      return [];
    }
  });
  trace.events = [...older, ...trace.events].slice(-MAX_EVENTS);
}

/** Adds a probe to the hop, which keeps its newest `max` and at most a batch more. */
function record(hop: Hop, t: number, rtt: number | null, max: number) {
  hop.times.push(t);
  hop.rtts.push(rtt);
  // The oldest are dropped in batches: taking one off the front of a long array moves
  // every other element, which adds up when it happens for every hop every round.
  if (hop.times.length >= max + TRIM_BATCH) {
    hop.times.splice(0, hop.times.length - max);
    hop.rtts.splice(0, hop.rtts.length - max);
  }
}

// Load-balanced hops can change address every round, so notifications are spaced out.
const ROUTE_NOTICE_SECONDS = 60;
// Rounds the route's end looks back over while its final hop does not answer.
const PATH_END_ROUNDS = 10;

/** Adds one round to the trace. `live` rounds, unlike replayed history, can notify. */
function apply(trace: Trace, sample: Sample, live: boolean, onChange: () => void) {
  const t = sample.t / 1000;
  const max = historyRounds(WINDOW_HOURS, trace.config.intervalMs);
  const changes: TraceEvent[] = [];
  // A round that reached its final hop tells how long the path is now.
  const last = sample.hops.at(-1);
  if (last && last.rtt !== null) {
    if (trace.pathLength && last.ttl !== trace.pathLength) {
      changes.push({ t, kind: "length", from: trace.pathLength, to: last.ttl });
    }
    trace.pathLength = last.ttl;
  }
  // The route's end is the longer of the last two rounds once both reached their final
  // hop, so a shorter route shows after two rounds while a route flapping between two
  // lengths stays put. With an unreachable final hop it is the longest of the last
  // PATH_END_ROUNDS instead. Rounds without any reply (network down) do not count.
  if (sample.hops.some((probe) => probe.rtt !== null)) {
    trace.lengths.push({
      hops: Math.max(...sample.hops.map((probe) => probe.ttl)),
      reached: !!last && last.rtt !== null,
    });
    if (trace.lengths.length > PATH_END_ROUNDS) trace.lengths.shift();
    const latest = trace.lengths.slice(-2);
    const settled = latest.length === 2 && latest.every((round) => round.reached);
    trace.pathEnd = Math.max(...(settled ? latest : trace.lengths).map((round) => round.hops));
  }
  // The target's reply is the final hop of a round that reached it: the tracer ends a
  // round's hop list at the target, and one hop past the last reply when the target is
  // silent. Going by position rather than by address keeps saved rounds valid when the
  // name resolves to another address on a later start.
  const reply = last && last.rtt !== null ? last : undefined;
  if (reply) trace.target.ttl = reply.ttl;
  record(trace.target, t, reply ? reply.rtt : null, max);
  for (const probe of sample.hops) {
    while (trace.hops.length < probe.ttl) {
      trace.hops.push({ ttl: trace.hops.length + 1, addr: null, name: null, times: [], rtts: [] });
    }
    const hop = trace.hops[probe.ttl - 1];
    const moved = !!probe.addr && probe.addr !== hop.addr;
    if (probe.addr && (moved || (live && hop.name === null && nameIsDue(probe.addr)))) {
      if (moved && hop.addr) changes.push({ t, kind: "hop", ttl: hop.ttl, from: hop.addr, to: probe.addr });
      const addr = probe.addr;
      hop.addr = addr;
      if (moved) hop.name = null;
      reverseDns(addr).then((name) => {
        if (hop.addr === addr) {
          hop.name = name;
          onChange();
        }
      });
    }
    record(hop, t, probe.rtt, max);
  }
  for (const change of changes) addEvent(trace, change, live);
  if (live && changes.length && t - trace.routeNotified >= ROUTE_NOTICE_SECONDS) {
    trace.routeNotified = t;
    const more = changes.length > 1 ? ` (+${changes.length - 1} more)` : "";
    notify(`TracePlotter: route to ${trace.config.host} changed`, describe(changes[0]) + more);
  }
}

/** Learns where the trace's history files begin; they lose old days as new ones come. */
async function readDiskStart(trace: Trace) {
  const start = await invoke<number | null>("history_start", { session: trace.config.sessionId }).catch(() => null);
  trace.diskStart = start ? start / 1000 : 0;
}

/** A hop's probes over a stretch of time, as the backend tallies the history files. */
export type RangeHop = { ttl: number; total: Tally; stretches: (Tally & { t: number })[] };

/**
 * What the trace's history files hold from `from` to `to` (Unix seconds): per hop number,
 * 0 for the round trip, its tally, and for those in `graphed` stretches of `width`
 * seconds. Also learns again where the files begin.
 */
export async function historyRange(trace: Trace, from: number, to: number, width: number, graphed: number[]) {
  const range = await invoke<{ start: number | null; hops: RangeHop[] }>("history_range", {
    session: trace.config.sessionId,
    from: Math.floor(from * 1000),
    to: Math.floor(to * 1000) + 1,
    width: Math.round(width * 1000),
    graphed,
  });
  trace.diskStart = range.start ? range.start / 1000 : 0;
  return range.hops;
}

/** Raises or clears the alert from the target's latest probes. */
function checkAlert(trace: Trace) {
  const target = trace.target;
  if (!target.times.length) return;
  const { host, intervalMs, alertLatencyMs, alertLossPct, alertSeconds } = trace.config;
  const probes = alertProbes(alertSeconds ?? DEFAULT_ALERT_SECONDS, intervalMs);
  const alerting = breaches(target.rtts, probes, alertLatencyMs, alertLossPct);
  if (alerting === trace.alerting) return;
  trace.alerting = alerting;
  const t = target.times[target.times.length - 1];
  if (!alerting) {
    addEvent(trace, { t, kind: "recover", text: "Back within alert thresholds" }, true);
    return;
  }
  const s = stats(target.rtts.slice(-probes));
  const text = `Last ${s.sent} probes: ${s.avg === null ? "no replies" : `${s.avg.toFixed(1)} ms average`}, ${s.lossPct.toFixed(0)}% loss`;
  addEvent(trace, { t, kind: "alert", text }, true);
  notify(`TracePlotter: ${host}`, text);
}

let nextId = 0;

const newTarget = (config: TraceConfig): Hop => ({ ttl: 0, addr: null, name: config.host, times: [], rtts: [] });

/**
 * Starts probing into `trace`, replaying its saved history the first time.
 * `onChange` fires whenever the trace gains data.
 */
async function run(trace: Trace, onChange: () => void) {
  const withHistory = !trace.loaded;
  trace.error = null;
  // Samples that arrive before the history is replayed wait here, to keep time order.
  let early: Sample[] | null = [];
  const { sessionId, host, protocol, port, intervalMs, timeoutMs, packetSize } = trace.config;
  const channel = new Channel<Message>();
  channel.onmessage = (message) => {
    // A run that was restarted since belongs to an erased session.
    if (trace.config.sessionId !== sessionId) return;
    if (message.kind === "error") {
      // An empty message means the backend recovered from the error it reported.
      trace.error = message.message || null;
    } else if (early) {
      early.push(message);
      return;
    } else {
      apply(trace, message, true, onChange);
      checkAlert(trace);
    }
    onChange();
  };
  const started = await invoke<{ id: number; addr: string; history: Sample[] }>("start_trace", {
    session: sessionId,
    host,
    protocol,
    port,
    intervalMs,
    timeoutMs: timeoutMs ?? DEFAULT_TIMEOUT_MS,
    packetSize: packetSize ?? DEFAULT_PACKET_SIZE,
    withHistory,
    channel,
  });
  trace.runId = started.id;
  trace.addr = trace.target.addr = started.addr;
  trace.loaded = true;
  for (const sample of [...started.history, ...early]) apply(trace, sample, false, onChange);
  early = null;
  if (withHistory) await loadEvents(trace);
  await readDiskStart(trace);
}

/**
 * Starts a trace, first replaying the history saved for its session, if any.
 * With `keepOnFailure` a trace that cannot start (no network, name not resolving) is
 * returned paused with its error, to be resumed later, instead of throwing.
 */
export async function startTrace(config: TraceConfig, onChange: () => void, keepOnFailure = false): Promise<Trace> {
  const trace: Trace = {
    id: nextId++,
    runId: -1,
    loaded: false,
    paused: false,
    config,
    addr: "",
    hops: [],
    error: null,
    alerting: false,
    events: [],
    pathLength: 0,
    target: newTarget(config),
    lengths: [],
    pathEnd: 0,
    routeNotified: 0,
    diskStart: 0,
  };
  // A tab the user had paused comes back paused: its history is shown, probing is not started.
  if (config.paused) {
    trace.paused = true;
    const history = await invoke<Sample[]>("read_history", { session: config.sessionId, intervalMs: config.intervalMs }).catch(() => []);
    for (const sample of history) apply(trace, sample, false, onChange);
    trace.loaded = true;
    await loadEvents(trace);
    await readDiskStart(trace);
    return trace;
  }
  try {
    await run(trace, onChange);
  } catch (e) {
    if (!keepOnFailure) throw e;
    // Paused for now only; `config.paused` stays off so the next launch tries again.
    trace.paused = true;
    trace.error = `Could not start: ${e}`;
  }
  return trace;
}

/** Stops the trace's current run, if it ever started one. */
async function stopTrace(trace: Trace) {
  if (trace.runId >= 0) await invoke("stop_trace", { id: trace.runId });
}

/** Stops a trace for good and erases its saved history. */
export async function closeTrace(trace: Trace) {
  await stopTrace(trace);
  await invoke("delete_history", { session: trace.config.sessionId });
}

/**
 * Throws away everything the trace has collected, on screen and on disk, and starts it
 * again from nothing. The trace moves to a new session id, so the old run's file can be
 * erased while that run is still winding down.
 */
export async function restartTrace(trace: Trace, onChange: () => void) {
  await stopTrace(trace);
  const erased = trace.config.sessionId;
  trace.config.sessionId = crypto.randomUUID();
  trace.config.paused = false;
  Object.assign(trace, {
    hops: [],
    events: [],
    alerting: false,
    pathLength: 0,
    target: newTarget(trace.config),
    lengths: [],
    pathEnd: 0,
    routeNotified: 0,
    diskStart: 0,
    paused: false,
  });
  try {
    await run(trace, onChange);
  } catch (e) {
    trace.paused = true;
    throw e;
  } finally {
    await invoke("delete_history", { session: erased });
  }
}

/** Pauses probing, or resumes it into the same trace. The backend stops at its next round. */
export async function setPaused(trace: Trace, paused: boolean, onChange: () => void) {
  if (paused) {
    trace.paused = true;
    await stopTrace(trace);
  } else {
    await run(trace, onChange);
    trace.paused = false;
  }
  // Remembered with the tab; the caller saves the session.
  trace.config.paused = paused;
}

/** Asks where to save the bytes, `name` being the suggestion. Returns the path, or null if cancelled. */
export function exportFile(name: string, data: Uint8Array) {
  // The name travels in an HTTP header, which takes only Latin-1; a host name in another
  // script would make the call throw. The backend allows just these characters anyway.
  const safe = name.replace(/[^A-Za-z0-9._-]/g, "_");
  return invoke<string | null>("export_file", data, { headers: { "x-file-name": safe } });
}

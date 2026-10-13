import { Channel, invoke } from "@tauri-apps/api/core";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { ALERT_WINDOW, breaches, stats } from "./stats";

export type Protocol = "icmp" | "udp" | "tcp";

/** What the session file remembers about a target. Alert thresholds of 0 are off. */
export type TraceConfig = {
  /** Unique per tab; names the tab's history file. Two tabs to one host stay separate. */
  sessionId: string;
  host: string;
  protocol: Protocol;
  port: number;
  intervalMs: number;
  alertLatencyMs: number;
  alertLossPct: number;
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

export type TraceEvent = {
  /** Unix seconds. */
  t: number;
  kind: "route" | "alert" | "recover";
  text: string;
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
};

type Sample = { t: number; hops: { ttl: number; addr: string | null; rtt: number | null }[] };
type Message = ({ kind: "sample" } & Sample) | { kind: "error"; message: string };

// Matches the backend's MAX_HISTORY: 24 h at a 1 s interval.
const MAX_SAMPLES = 86_400;
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

function addEvent(trace: Trace, event: TraceEvent) {
  trace.events.push(event);
  if (trace.events.length > MAX_EVENTS) trace.events.shift();
}

function record(hop: Hop, t: number, rtt: number | null) {
  hop.times.push(t);
  hop.rtts.push(rtt);
  // The oldest are dropped in batches: taking one off the front of a long array moves
  // every other element, which adds up when it happens for every hop every round.
  if (hop.times.length >= MAX_SAMPLES + TRIM_BATCH) {
    hop.times.splice(0, TRIM_BATCH);
    hop.rtts.splice(0, TRIM_BATCH);
  }
}

// Load-balanced hops can change address every round, so notifications are spaced out.
const ROUTE_NOTICE_SECONDS = 60;
// Rounds the route's end looks back over while its final hop does not answer.
const PATH_END_ROUNDS = 10;

/** Adds one round to the trace. `live` rounds, unlike replayed history, can notify. */
function apply(trace: Trace, sample: Sample, live: boolean, onChange: () => void) {
  const t = sample.t / 1000;
  const changes: string[] = [];
  // A round that reached its final hop tells how long the path is now.
  const last = sample.hops.at(-1);
  if (last && last.rtt !== null) {
    if (trace.pathLength && last.ttl !== trace.pathLength) {
      changes.push(`Path length changed: ${trace.pathLength} → ${last.ttl} hops`);
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
  record(trace.target, t, reply ? reply.rtt : null);
  for (const probe of sample.hops) {
    while (trace.hops.length < probe.ttl) {
      trace.hops.push({ ttl: trace.hops.length + 1, addr: null, name: null, times: [], rtts: [] });
    }
    const hop = trace.hops[probe.ttl - 1];
    const moved = !!probe.addr && probe.addr !== hop.addr;
    if (probe.addr && (moved || (live && hop.name === null && nameIsDue(probe.addr)))) {
      if (moved && hop.addr) changes.push(`Hop ${hop.ttl} changed: ${hop.addr} → ${probe.addr}`);
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
    record(hop, t, probe.rtt);
  }
  for (const text of changes) addEvent(trace, { t, kind: "route", text });
  if (live && changes.length && t - trace.routeNotified >= ROUTE_NOTICE_SECONDS) {
    trace.routeNotified = t;
    const more = changes.length > 1 ? ` (+${changes.length - 1} more)` : "";
    sendNotification({ title: `TracePlotter: route to ${trace.config.host} changed`, body: changes[0] + more });
  }
}

/** Raises or clears the alert from the target's latest probes. */
function checkAlert(trace: Trace) {
  const target = trace.target;
  if (!target.times.length) return;
  const { host, alertLatencyMs, alertLossPct } = trace.config;
  const alerting = breaches(target.rtts, alertLatencyMs, alertLossPct);
  if (alerting === trace.alerting) return;
  trace.alerting = alerting;
  const t = target.times[target.times.length - 1];
  if (!alerting) {
    addEvent(trace, { t, kind: "recover", text: "Back within alert thresholds" });
    return;
  }
  const s = stats(target.rtts.slice(-ALERT_WINDOW));
  const text = `Last ${s.sent} probes: ${s.avg === null ? "no replies" : `${s.avg.toFixed(1)} ms average`}, ${s.lossPct.toFixed(0)}% loss`;
  addEvent(trace, { t, kind: "alert", text });
  sendNotification({ title: `TracePlotter: ${host}`, body: text });
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
  const { sessionId, host, protocol, port, intervalMs } = trace.config;
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
    withHistory,
    channel,
  });
  trace.runId = started.id;
  trace.addr = trace.target.addr = started.addr;
  trace.loaded = true;
  for (const sample of [...started.history, ...early]) apply(trace, sample, false, onChange);
  early = null;
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
  };
  try {
    await run(trace, onChange);
  } catch (e) {
    if (!keepOnFailure) throw e;
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
  Object.assign(trace, {
    hops: [],
    events: [],
    alerting: false,
    pathLength: 0,
    target: newTarget(trace.config),
    lengths: [],
    pathEnd: 0,
    routeNotified: 0,
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
}

/** Asks where to save the bytes, `name` being the suggestion. Returns the path, or null if cancelled. */
export function exportFile(name: string, data: Uint8Array) {
  // The name travels in an HTTP header, which takes only Latin-1; a host name in another
  // script would make the call throw. The backend allows just these characters anyway.
  const safe = name.replace(/[^A-Za-z0-9._-]/g, "_");
  return invoke<string | null>("export_file", data, { headers: { "x-file-name": safe } });
}

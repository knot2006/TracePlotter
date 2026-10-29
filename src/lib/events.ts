/** Something worth telling the user about a trace. `t` is in Unix seconds. */
export type TraceEvent =
  | { t: number; kind: "hop"; ttl: number; from: string; to: string }
  | { t: number; kind: "length"; from: number; to: number }
  | { t: number; kind: "alert" | "recover"; text: string };

/** What the Events list shows as one entry: a whole re-routing, or one alert or recovery. */
export type EventGroup =
  | {
      t: number;
      kind: "route";
      /** Hop count before and after, when it changed. */
      length: { from: number; to: number } | null;
      /** Hops that answer from another address than before, in route order. */
      hops: { ttl: number; from: string; to: string }[];
    }
  | { t: number; kind: "alert" | "recover"; text: string };

// A re-routing shows up over a round or two, one hop at a time. Changes this close
// together are one re-routing.
const SAME_CHANGE_SECONDS = 5;

export type Incident = { from: number; to: number | null; text: string };

/** Route events can be replayed from samples, but alert decisions only exist in the log. */
export function restoreEvents(saved: TraceEvent[], replayed: TraceEvent[], held: number): TraceEvent[] {
  const kept = saved.filter((event) => event.t < held || event.kind === "alert" || event.kind === "recover");
  const unique = new Map([...kept, ...replayed].map((event) => [JSON.stringify(event), event]));
  return [...unique.values()].sort((a, b) => a.t - b.t);
}

/** Pairs alerts with recoveries. An unmatched recovery has no known starting time. */
export function incidents(events: TraceEvent[]): Incident[] {
  const result: Incident[] = [];
  let open: Incident | null = null;
  for (const event of [...events].sort((a, b) => a.t - b.t)) {
    if (event.kind === "alert" && !open) {
      open = { from: event.t, to: null, text: event.text };
      result.push(open);
    } else if (event.kind === "recover" && open) {
      open.to = event.t;
      open = null;
    }
  }
  return result;
}

export function eventLabel(event: EventGroup): string {
  if (event.kind !== "route") return `${event.kind === "alert" ? "Alert" : "Recovered"}: ${event.text}`;
  const moves = event.hops.map((hop) => `hop ${hop.ttl}: ${hop.from} → ${hop.to}`);
  if (event.length) moves.unshift(`${event.length.from} → ${event.length.to} hops`);
  return `Route changed: ${moves.join(", ")}`;
}

export type TargetIncident = Incident & { target: string; observedUntil: number };

/** Groups overlapping recorded alert windows without implying that they share a cause. */
export function correlateIncidents(entries: TargetIncident[]) {
  const groups: { from: number; to: number; entries: TargetIncident[] }[] = [];
  for (const entry of [...entries].sort((a, b) => a.from - b.from)) {
    const end = Math.max(entry.from, entry.to ?? entry.observedUntil);
    const last = groups.at(-1);
    if (last && entry.from <= last.to) {
      last.to = Math.max(last.to, end);
      last.entries.push(entry);
    } else groups.push({ from: entry.from, to: end, entries: [entry] });
  }
  return groups;
}

/** One event in words, for a notification. */
export function describe(event: TraceEvent): string {
  if (event.kind === "hop") return `Hop ${event.ttl} changed: ${event.from} → ${event.to}`;
  if (event.kind === "length") return `Path length changed: ${event.from} → ${event.to} hops`;
  return event.text;
}

/**
 * Folds the events, oldest first, into list entries. The hop and length changes of one
 * re-routing become a single entry that says where each hop started and where it ended
 * up; a hop that went back to its old address within the entry is left out, and so is an
 * entry in which nothing changed in the end.
 */
export function groupEvents(events: TraceEvent[]): EventGroup[] {
  const groups: EventGroup[] = [];
  let open: { t: number; last: number; length: { from: number; to: number } | null; hops: Map<number, { from: string; to: string }> } | null = null;

  const close = () => {
    if (!open) return;
    const hops = [...open.hops]
      .filter(([, move]) => move.from !== move.to)
      .sort(([a], [b]) => a - b)
      .map(([ttl, move]) => ({ ttl, ...move }));
    const length = open.length && open.length.from !== open.length.to ? open.length : null;
    if (hops.length || length) groups.push({ t: open.t, kind: "route", length, hops });
    open = null;
  };

  for (const event of events) {
    if (event.kind !== "hop" && event.kind !== "length") {
      close();
      groups.push(event);
      continue;
    }
    if (open && event.t - open.last > SAME_CHANGE_SECONDS) close();
    const group = (open ??= { t: event.t, last: event.t, length: null, hops: new Map() });
    group.last = event.t;
    if (event.kind === "length") {
      group.length = { from: group.length?.from ?? event.from, to: event.to };
    } else {
      group.hops.set(event.ttl, { from: group.hops.get(event.ttl)?.from ?? event.from, to: event.to });
    }
  }
  close();
  return groups;
}

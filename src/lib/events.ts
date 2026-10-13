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

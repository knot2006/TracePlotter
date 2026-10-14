export type HopStats = {
  sent: number;
  /** Probes that got a reply. */
  received: number;
  lossPct: number;
  last: number | null;
  avg: number | null;
  min: number | null;
  max: number | null;
  jitter: number | null;
};

/**
 * The smallest round number at or above `value`, for the end of an axis: 1, 1.5, 2, 2.5, 3,
 * 4, 5, 6, 7, 8, 9 or 10 times a power of ten. Anything not above 0 gives 1.
 */
export function niceCeil(value: number): number {
  if (!(value > 0)) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  const step = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 7, 8, 9, 10].find((s) => s * power >= value * (1 - 1e-9)) ?? 10;
  return step * power;
}

/** Index of the first entry of ascending `times` that is `>= cutoff`. */
export function lowerBound(times: number[], cutoff: number): number {
  let low = 0;
  let high = times.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (times[mid] < cutoff) low = mid + 1;
    else high = mid;
  }
  return low;
}

/**
 * Jitter over time, aligned with `rtts`: a running average of how much each reply's
 * latency differs from the previous reply's. It is the estimate of RFC 3550, which moves
 * one sixteenth of the way to each new difference, so a single spike barely shows while
 * a lasting change does. `null` for lost probes and for the first reply.
 */
export function jitterSeries(rtts: (number | null)[]): (number | null)[] {
  let previous: number | null = null;
  let jitter: number | null = null;
  return rtts.map((rtt) => {
    if (rtt === null) return null;
    if (previous !== null) {
      const change = Math.abs(rtt - previous);
      jitter = jitter === null ? change : jitter + (change - jitter) / 16;
    }
    previous = rtt;
    return jitter;
  });
}

/**
 * Packet loss in percent for the graph: time is cut into buckets of `bucketSeconds`,
 * and every probe gets the loss of its bucket. Aligned with `times`.
 */
export function lossSeries(times: number[], rtts: (number | null)[], bucketSeconds: number): number[] {
  const sent = new Map<number, number>();
  const lost = new Map<number, number>();
  const bucket = (time: number) => Math.floor(time / bucketSeconds);
  times.forEach((time, i) => {
    const b = bucket(time);
    sent.set(b, (sent.get(b) ?? 0) + 1);
    if (rtts[i] === null) lost.set(b, (lost.get(b) ?? 0) + 1);
  });
  return times.map((time) => {
    const b = bucket(time);
    return ((lost.get(b) ?? 0) / sent.get(b)!) * 100;
  });
}

const SPAN_UNITS = [
  { name: "day", letter: "d", seconds: 86_400 },
  { name: "hour", letter: "h", seconds: 3600 },
  { name: "minute", letter: "m", seconds: 60 },
  { name: "second", letter: "s", seconds: 1 },
];

/**
 * A typed time span in seconds: a number and a unit that starts with s, m, h or d
 * ("30s", "5 min", "1.5h"); a bare number counts as minutes. "all" is 0, meaning
 * everything. At least 10 seconds. `null` when the text is not understood.
 */
export function parseSpan(text: string): number | null {
  const typed = text.trim().toLowerCase();
  if (typed && "all".startsWith(typed)) return 0;
  const match = /^(\d+(?:[.,]\d+)?)\s*([a-z]*)$/.exec(typed);
  if (!match) return null;
  const unit = SPAN_UNITS.find((u) => u.letter === (match[2][0] ?? "m"));
  if (!unit) return null;
  return Math.max(10, Math.round(Number(match[1].replace(",", ".")) * unit.seconds));
}

/** A time span in seconds in words, in the largest unit that divides it: "10 minutes". */
export function spanLabel(seconds: number): string {
  if (!seconds) return "All";
  const unit = SPAN_UNITS.find((u) => seconds % u.seconds === 0)!;
  const count = seconds / unit.seconds;
  return `${count} ${unit.name}${count === 1 ? "" : "s"}`;
}

/** A time span as it can be typed: "10m", or "all" for everything. */
export function spanShort(seconds: number): string {
  if (!seconds) return "all";
  const unit = SPAN_UNITS.find((u) => seconds % u.seconds === 0)!;
  return `${seconds / unit.seconds}${unit.letter}`;
}

/** Number of latest probes an alert looks at. */
export const ALERT_WINDOW = 10;

/**
 * Estimated call quality from 1 (unusable) to about 4.4 (best), using the
 * simplified ITU-T G.107 E-model on average latency, jitter and loss.
 */
export function mos(s: HopStats): number | null {
  if (s.avg === null) return null;
  const latency = s.avg + 2 * (s.jitter ?? 0) + 10;
  const base = latency < 160 ? 93.2 - latency / 40 : 93.2 - (latency - 120) / 10;
  const r = Math.min(100, Math.max(0, base - 2.5 * s.lossPct));
  return 1 + 0.035 * r + 0.000007 * r * (r - 60) * (100 - r);
}

/**
 * True when the last `ALERT_WINDOW` probes average above `latencyMs` or lose at
 * least `lossPct` percent. A threshold of 0 is off.
 */
export function breaches(rtts: (number | null)[], latencyMs: number, lossPct: number): boolean {
  const s = stats(rtts.slice(-ALERT_WINDOW));
  return (
    (latencyMs > 0 && s.avg !== null && s.avg > latencyMs) ||
    (lossPct > 0 && s.sent > 0 && s.lossPct >= lossPct)
  );
}

/**
 * A table as plain text with aligned columns, under the given title: the header labels,
 * then one line per row. Each column is as wide as its longest cell; `left` columns are
 * left-aligned, the others right-aligned like numbers.
 */
export function textTable(title: string, headers: { label: string; left?: boolean }[], rows: string[][]): string {
  const widths = headers.map((header, i) => Math.max(header.label.length, ...rows.map((row) => row[i].length)));
  const line = (cells: string[]) =>
    cells
      .map((cell, i) => (headers[i].left ? cell.padEnd(widths[i]) : cell.padStart(widths[i])))
      .join("  ")
      .trimEnd();
  return [title, "", line(headers.map((header) => header.label)), ...rows.map(line)].join("\n") + "\n";
}

/**
 * Summary of one hop's round-trip times in ms, `null` = lost probe.
 * Jitter is the mean absolute difference between consecutive replies.
 */
// ponytail: rescans every sample on each call (O(n) per hop per round).
// Keep running totals per hop if tabs with a full day of samples get slow.
export function stats(rtts: (number | null)[]): HopStats {
  let received = 0;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  let last: number | null = null;
  let jitterSum = 0;
  let jitterCount = 0;
  for (const rtt of rtts) {
    if (rtt === null) continue;
    if (last !== null) {
      jitterSum += Math.abs(rtt - last);
      jitterCount++;
    }
    received++;
    sum += rtt;
    if (rtt < min) min = rtt;
    if (rtt > max) max = rtt;
    last = rtt;
  }
  const sent = rtts.length;
  return {
    sent,
    received,
    lossPct: sent ? ((sent - received) / sent) * 100 : 0,
    last: sent ? rtts[sent - 1] : null,
    avg: received ? sum / received : null,
    min: received ? min : null,
    max: received ? max : null,
    jitter: jitterCount ? jitterSum / jitterCount : null,
  };
}

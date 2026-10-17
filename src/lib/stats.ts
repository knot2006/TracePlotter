import type { Trace } from "./trace.ts";

/**
 * The newest `span` seconds of a trace, or with 0 all its history, in Unix seconds, and
 * whether that reaches before the oldest round the window holds into the history files.
 */
export function period(trace: Trace, span: number) {
  const { times } = trace.target;
  const to = times.at(-1) ?? 0;
  const held = times[0] ?? 0;
  const from = span ? to - span : trace.diskStart && times.length ? Math.min(held, trace.diskStart) : held;
  const older = !!trace.diskStart && times.length > 0 && from < held - (2 * trace.config.intervalMs) / 1000;
  return { from, to, older };
}

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

/**
 * The latency axis for replies from `min` to `max` ms. From 0 it ends at a round number
 * a fifth above the highest, so the line stays off the top edge and the axis does not
 * jump with every new maximum. Starting at 0 keeps heights honest (a wobble of 1 ms on
 * 10 ms looks small), unless that would leave more than a fifth of the graph empty under
 * the lowest reply: then the axis runs between round numbers a margin below and above
 * the replies. The margin is never under a tenth of the latency or 5 ms, so a steady
 * line does not have its smallest wobbles blown up to full height.
 */
export function latencyAxis(min: number | null, max: number | null): [number, number] {
  const fromZero = niceCeil((max ?? 0) * 1.2);
  if (min === null || max === null) return [0, fromZero];
  const margin = Math.max((max - min) * 0.2, max * 0.1, 5);
  const [below, above] = [min - margin, max + margin];
  // A round step of 1, 2 or 5 times a power of ten, about a fifth of the axis.
  const fifth = (above - below) / 5;
  const power = 10 ** Math.floor(Math.log10(fifth));
  const step = [5, 2, 1].map((s) => s * power).find((s) => s <= fifth) ?? power;
  const bottom = Math.floor(below / step) * step;
  return bottom > fromZero / 5 ? [bottom, Math.ceil(above / step) * step] : [0, fromZero];
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

/**
 * Points on each side that a long period's line blends together (see `thin`). Two is
 * enough to keep the line from beating against waves a few pixels long, and still
 * draws any wave longer than about six pixels, even on a narrow window.
 */
export const BLEND_RADIUS = 2;

/** What a hop graph draws: times, then latency, jitter and loss aligned with them. */
export type GraphData = { xs: number[]; ys: (number | null)[]; js: (number | null)[]; ls: (number | null)[] };

/** Graph data with the lowest and highest latency each point stands for. */
export type ThinData = GraphData & { lo: (number | null)[]; hi: (number | null)[] };

/**
 * Graph data cut down to a point per stretch of `width` seconds for drawing a long period,
 * which would otherwise put many probes on every pixel and draw them as a solid wall. Each
 * run of points becomes one, at the run's first time, with the run's average latency and
 * jitter, and the lowest and highest latency in `lo` and `hi` for a band around the line.
 * The stretches start at multiples of `width`, as the backend's do, not at the first
 * point: so a run holds the same probes from one round to the next while the period
 * moves on, and the line and band stay still instead of shifting with every round. A
 * run in which every probe was lost stays lost, so the gap shows. Points at `breaks`
 * (where probing stopped) are kept as they are and end a run. With no `width` the data
 * comes back unchanged, its band no wider than the line.
 *
 * With a `radius`, each point's latency is then weighed together with its neighbours' out
 * to that many points on each side, the nearer ones counting more (triangle weights) and
 * each by its number of replies. Plain averages of stretches beat against anything that
 * repeats at about their length, such as a latency that rises and falls every quarter of
 * an hour, and draw a wave that is not there. The weights never reach across a break,
 * and a point without replies stays a gap.
 */
export function thin(data: GraphData, breaks: Set<number>, width: number, radius = 0): ThinData {
  const { xs, ys, js, ls } = data;
  if (!(width > 0)) return { ...data, lo: ys, hi: ys };
  const stretch = (x: number) => Math.floor(x / width);
  const out: ThinData = { xs: [], ys: [], js: [], ls: [], lo: [], hi: [] };
  // Replies behind each point, which weigh its average when neighbours are blended.
  const counts: number[] = [];
  let start = 0;
  while (start < xs.length) {
    if (breaks.has(xs[start])) {
      out.xs.push(xs[start]);
      out.ys.push(null);
      out.js.push(null);
      out.ls.push(null);
      out.lo.push(null);
      out.hi.push(null);
      counts.push(0);
      start++;
      continue;
    }
    let sum = 0;
    let replies = 0;
    let low = Infinity;
    let high = -Infinity;
    let jSum = 0;
    let jitters = 0;
    let end = start;
    for (; end < xs.length && stretch(xs[end]) === stretch(xs[start]) && !breaks.has(xs[end]); end++) {
      const y = ys[end];
      if (y !== null) {
        sum += y;
        replies++;
        low = Math.min(low, y);
        high = Math.max(high, y);
      }
      const j = js[end];
      if (j !== null) {
        jSum += j;
        jitters++;
      }
    }
    out.xs.push(xs[start]);
    out.ys.push(replies ? sum / replies : null);
    out.js.push(jitters ? jSum / jitters : null);
    // Loss is counted over stretches far longer than a run, so its first point speaks for it.
    out.ls.push(ls[start]);
    out.lo.push(replies ? low : null);
    out.hi.push(replies ? high : null);
    counts.push(replies);
    start = end;
  }
  if (radius) blend(out, counts, breaks, radius);
  return out;
}

/** Blends each point's latency with its neighbours', as `thin` describes. */
function blend(out: ThinData, counts: number[], breaks: Set<number>, radius: number) {
  const averages = out.ys;
  out.ys = averages.map((y, i) => {
    if (y === null) return null;
    let sum = 0;
    let weight = 0;
    for (const step of [-1, 1]) {
      for (let k = step < 0 ? 0 : 1; k <= radius; k++) {
        const j = i + step * k;
        if (j < 0 || j >= averages.length || breaks.has(out.xs[j])) break;
        const w = (radius + 1 - k) * counts[j];
        if (w) {
          sum += w * averages[j]!;
          weight += w;
        }
      }
    }
    return sum / weight;
  });
}

/** A hop's probes over a stretch of time, as the backend tallies its history files. */
export type Tally = {
  sent: number;
  received: number;
  sum: number;
  min: number | null;
  max: number | null;
  jitter_sum: number;
  jitter_count: number;
  last: number | null;
};

/** The numbers of a tally, as `stats` gives them for samples. */
export function tallyStats(t: Tally): HopStats {
  return {
    sent: t.sent,
    received: t.received,
    lossPct: t.sent ? ((t.sent - t.received) / t.sent) * 100 : 0,
    last: t.sent ? t.last : null,
    avg: t.received ? t.sum / t.received : null,
    min: t.min,
    max: t.max,
    jitter: t.jitter_count ? t.jitter_sum / t.jitter_count : null,
  };
}

/**
 * How long a stretch of a graph of `span` seconds should be, `pixels` wide, for a trace
 * probing every `interval` seconds: about a pixel, in whole minutes once that is a minute
 * or more, as the backend keeps day files tallied by the minute; never under the interval.
 */
export function stretchWidth(span: number, pixels: number, interval: number) {
  const perPixel = span / Math.max(1, pixels);
  return perPixel >= 60 ? Math.ceil(perPixel / 60) * 60 : Math.max(Math.ceil(perPixel), interval);
}

/**
 * Lines that each have points at some of the times, made into the one list of times a
 * graph takes and a value per line at each. Where a line has no point of its own it keeps
 * its last value, which draws the same on a stepped line; before its first point and
 * after its last it has none (`null`). So its only gaps are where it breaks itself.
 */
export function alignLines(lines: Map<number, number | null>[]) {
  const xs = [...new Set(lines.flatMap((line) => [...line.keys()]))].sort((a, b) => a - b);
  const ys = lines.map((line) => {
    const own = [...line.keys()];
    const [first, last] = [Math.min(...own), Math.max(...own)];
    let value: number | null = null;
    return xs.map((x) => {
      if (line.has(x)) value = line.get(x)!;
      return x < first || x > last ? null : value;
    });
  });
  return { xs, ys };
}

/**
 * Graph data from a hop's tallied stretches of `width` seconds (`t` in Unix ms), drawn
 * the way `thin` draws a long period: each stretch's average blended with `radius`
 * neighbours, inside a band from its lowest to its highest reply. Where probing stopped,
 * a break point ends the line: a gap longer than a graph of samples breaks at (5 s or
 * three intervals), and longer than a stretch and a half, as stretches without a probe
 * are normal when they are not much longer than the `interval`. Loss is counted over
 * stretches of `lossSeconds`, as on a graph of samples.
 */
export function rangeGraph(
  stretches: (Tally & { t: number })[],
  width: number,
  lossSeconds: number,
  radius: number,
  interval: number,
) {
  const maxGap = Math.max(1.5 * width, 3 * interval, 5);
  const group = (t: number) => Math.floor(t / 1000 / lossSeconds);
  const sent = new Map<number, number>();
  const lost = new Map<number, number>();
  for (const s of stretches) {
    sent.set(group(s.t), (sent.get(group(s.t)) ?? 0) + s.sent);
    lost.set(group(s.t), (lost.get(group(s.t)) ?? 0) + s.sent - s.received);
  }
  const data: ThinData = { xs: [], ys: [], js: [], ls: [], lo: [], hi: [] };
  const counts: number[] = [];
  const breaks = new Set<number>();
  const push = (x: number, s: Tally | null, loss: number | null) => {
    data.xs.push(x);
    data.ys.push(s?.received ? s.sum / s.received : null);
    data.js.push(s?.jitter_count ? s.jitter_sum / s.jitter_count : null);
    data.ls.push(loss);
    data.lo.push(s ? s.min : null);
    data.hi.push(s ? s.max : null);
    counts.push(s?.received ?? 0);
  };
  stretches.forEach((s, i) => {
    const x = s.t / 1000;
    const before = i > 0 ? stretches[i - 1].t / 1000 : null;
    if (before !== null && x - before > maxGap) {
      breaks.add(before + width);
      push(before + width, null, null);
    }
    push(x, s, (lost.get(group(s.t))! / sent.get(group(s.t))!) * 100);
  });
  blend(data, counts, breaks, radius);
  return { data, breaks };
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

/** The time spans Ctrl and the wheel step through, in seconds. */
const ZOOM_SPANS = [
  10, 15, 20, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10_800, 21_600, 43_200, 86_400, 172_800, 259_200,
  604_800, 1_209_600, 2_592_000,
];

/**
 * The time span one turn of the wheel zooms to from `span` seconds (0 for everything), over
 * a history `available` seconds long: the next round span in or out, which the time span
 * box can name. Zooming out to the whole history or past it shows everything.
 */
export function zoomSpan(span: number, out: boolean, available: number): number {
  const current = span || available;
  if (out) {
    const next = ZOOM_SPANS.find((s) => s > current);
    return next === undefined || next >= available ? 0 : next;
  }
  return [...ZOOM_SPANS].reverse().find((s) => s < current) ?? ZOOM_SPANS[0];
}

/** Number of latest probes an alert looks at: `seconds` of them at the interval, at least one. */
export const alertProbes = (seconds: number, intervalMs: number) => Math.max(1, Math.round((seconds * 1000) / intervalMs));

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
 * True when the last `probes` probes average above `latencyMs` or lose at least
 * `lossPct` percent. A threshold of 0 is off. Fewer probes than that are not judged, so
 * a lost first probe of a new trace does not count as total loss.
 */
export function breaches(rtts: (number | null)[], probes: number, latencyMs: number, lossPct: number): boolean {
  if (rtts.length < probes) return false;
  const s = stats(rtts.slice(-probes));
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
// ponytail: rescans every sample on each call (O(n) per hop per refresh). At the most
// history a tab holds (345,600 rounds) that is about 1 ms per hop, a small share of a
// refresh; keep running totals per hop only if a profile says otherwise.
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

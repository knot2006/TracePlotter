import type { Tally } from "./stats.ts";

export type Detail = {
  ttl: number;
  addr: string | null;
  total: Tally;
  median: number | null;
  p95: number | null;
  p99: number | null;
  longest_lost: number;
  longest_loss_seconds: number;
};

export type ExtraStats = Pick<Detail, "median" | "p95" | "p99" | "longest_lost" | "longest_loss_seconds">;

/** Extra columns from the same probes as the main table, without sorting every reply. */
export function sampleExtras(times: number[], rtts: (number | null)[], intervalMs: number, rounds = times): ExtraStats {
  const histogram = new Map<number, number>();
  let received = 0;
  let lost = 0;
  let lostStart = 0;
  let longest = 0;
  let seconds = 0;
  let round = 0;
  let previousRound = -1;
  times.forEach((time, i) => {
    while (round < rounds.length && rounds[round] < time) round++;
    if (i && (time - times[i - 1] > Math.max(5, 3 * intervalMs / 1000) || round !== previousRound + 1)) lost = 0;
    previousRound = round;
    const rtt = rtts[i];
    if (rtt !== null) {
      const value = Math.round(Math.max(0, rtt) * 10);
      histogram.set(value, (histogram.get(value) ?? 0) + 1);
      received++;
      lost = 0;
    } else {
      if (!lost) lostStart = time;
      lost++;
      longest = Math.max(longest, lost);
      seconds = Math.max(seconds, time - lostStart + intervalMs / 1000);
    }
  });
  const sorted = [...histogram].sort(([a], [b]) => a - b);
  const atRank = (rank: number) => {
    let count = 0;
    for (const [value, n] of sorted) {
      count += n;
      if (count >= rank) return value / 10;
    }
    return null;
  };
  const middle = atRank(Math.ceil(received / 2));
  const median = middle !== null && received % 2 === 0 ? (middle + atRank(received / 2 + 1)!) / 2 : middle;
  return { median, p95: atRank(Math.ceil(received * .95)), p99: atRank(Math.ceil(received * .99)),
    longest_lost: longest, longest_loss_seconds: seconds };
}

/** Observations about replies, never a claim that a particular router dropped transit traffic. */
export function explainLoss(hop: { sent: number; received: number }, target: { sent: number; received: number }, roundTrip = false): string {
  if (hop.sent < 10 || target.sent < 10) return "Too few probes to interpret loss; collect at least 10 probes.";
  if (roundTrip) {
    return target.received === 0
      ? "No destination replies in this period. The destination may be unreachable or may filter this probe protocol; this does not establish a service outage."
      : target.received < target.sent
        ? "Some destination probes received no reply. Loss, filtering, reply limits or the return path may contribute; traceroute alone cannot identify the cause."
        : "The destination answered every recorded probe in this period.";
  }
  if (hop.sent === hop.received) return "This hop answered every recorded probe in this period.";
  if (target.received === target.sent) {
    return "This hop missed replies while the destination answered every probe in this period. Reply limiting or filtering may explain this; it alone is not evidence of forwarding loss.";
  }
  return "Both this hop and the destination missed replies in this period. This does not establish where loss occurred: probes and return paths can differ, and routers may limit replies.";
}

export const replyAvailability = (sent: number, received: number) => sent ? (received / sent * 100).toFixed(2) + "%" : "–";

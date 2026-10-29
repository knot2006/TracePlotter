import { eventLabel, groupEvents, incidents, type TraceEvent } from "./events.ts";
import { replyAvailability, type Detail } from "./insights.ts";
import { tallyStats } from "./stats.ts";

const escape = (text: string) => text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
const ms = (n: number | null) => n === null ? "–" : n.toFixed(1);
const utc = (t: number) => new Date(t * 1000).toISOString();

/** The same replacement is used for graph captions, event text, names and addresses. */
export function redactor(values: string[]) {
  const unique = [...new Set(values.filter(Boolean))].sort((a, b) => b.length - a.length);
  const labels = new Map(unique.map((v, i) => [v, `[address ${i + 1}]`]));
  const pattern = unique.map((v) => v.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  const re = pattern ? new RegExp(`(?<![A-Za-z0-9_.:%+-])(?:${pattern})(?![A-Za-z0-9_:%+-]|\\.[A-Za-z0-9_-])`, "g") : null;
  return (text: string) => re ? text.replace(re, (value) => labels.get(value)!) : text;
}

export type SupportReport = {
  target: string;
  from: number;
  to: number;
  probe: string;
  details: Detail[];
  events: TraceEvent[];
  graph: string | null;
  redact: (text: string) => string;
  redacted: boolean;
  demo: boolean;
};

/** A self-contained, printable report with no scripts or external resources. */
export function supportReport(report: SupportReport): string {
  const { target, from, to, probe, details, events, graph, redact, redacted, demo } = report;
  const text = (s: string) => escape(redact(s));
  const rows = details.map((d) => {
    const s = tallyStats(d.total);
    const cells = [d.ttl ? `Hop ${d.ttl}` : "Round trip", d.addr ?? "–", String(s.sent), String(s.received), `${s.lossPct.toFixed(2)}%`, replyAvailability(s.sent, s.received), ms(s.last), ms(s.avg), ms(s.min), ms(s.max), ms(s.jitter), ms(d.median), ms(d.p95), ms(d.p99), String(d.longest_lost), d.longest_loss_seconds.toFixed(1)];
    return `<tr>${cells.map((cell, i) => `<td>${i === 1 ? text(cell) : escape(cell)}</td>`).join("")}</tr>`;
  }).join("");
  const selected = events.filter((event) => event.t >= from && event.t <= to);
  const timeline = groupEvents(selected).map((e) => `<li><time>${utc(e.t)}</time> ${text(eventLabel(e))}</li>`).join("");
  const outages = incidents(events).filter((i) => i.from <= to && (i.to ?? to) >= from).map((i) =>
    `<li>${utc(i.from)} → ${i.to === null ? "no recovery recorded" : utc(i.to)} · ${Math.max(0, Math.min(i.to ?? to, to) - Math.max(i.from, from)).toFixed(0)} s within this period · ${text(i.text)}</li>`).join("");
  // Only our own PNG data URL may appear in the image attribute.
  const image = graph && /^data:image\/png;base64,[A-Za-z0-9+/=]+$/.test(graph) ? `<img alt="Selected-period latency graphs" src="${graph}">` : "<p>No graph available.</p>";
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'"><title>TracePlotter support report</title><style>body{font:14px system-ui,sans-serif;line-height:1.5;margin:32px;color:#18202c}h1{font-size:24px}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}th,td{padding:6px;border:1px solid #ccd2da;text-align:right}th:first-child,td:first-child{text-align:left}img{max-width:100%;background:#0f1319}li{margin:8px 0}time{font-family:monospace}section{overflow:auto}@media print{body{margin:0;font-size:10px}img{break-inside:avoid}th{background:#eee}}</style></head><body>
<h1>TracePlotter support report${demo ? " (DEMO — made-up data)" : ""}</h1>
<p>Target: ${text(target)}<br>Period (UTC): ${utc(from)} → ${utc(to)}<br>Probe settings: ${escape(probe)}<br>Address redaction: ${redacted ? "on (consistent placeholders)" : "off"}</p>
<h2>Selected-period statistics</h2><p>Latencies are milliseconds, recorded here at 0.1 ms resolution. p95 and p99 use nearest rank; the median averages the two central replies for an even count. Availability is the percentage of recorded probes answered, not service uptime. Pauses and unrecorded time are excluded. Loss durations estimate consecutive missed-probe spans from the configured interval.</p>
<section><table><thead><tr>${["Hop", "Latest address in period", "Sent", "Received", "Loss", "Reply availability", "Last", "Average", "Min", "Max", "Jitter", "Median", "p95", "p99", "Longest loss (probes)", "Longest loss (s)"].map((h) => `<th>${h}</th>`).join("")}</tr></thead><tbody>${rows}</tbody></table></section>
<h2>Graphs</h2>${image}<p>Graph labels use the recording computer's local time; the timeline below uses UTC. Red marks indicate alerts, green recoveries and blue route changes.</p>
<h2>Alert incidents</h2><ul>${outages || "<li>No alert incidents recorded for this period.</li>"}</ul>
<h2>Events in period</h2><ul>${timeline || "<li>No recorded events.</li>"}</ul></body></html>`;
}

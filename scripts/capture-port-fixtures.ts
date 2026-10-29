// Freeze outputs of the original pure functions before replacing their callers with Rust.
import { writeFileSync } from "node:fs";
import * as stats from "../src/lib/stats.ts";
import { correlateIncidents, eventLabel, groupEvents, incidents, restoreEvents } from "../src/lib/events.ts";
import { explainLoss, replyAvailability, sampleExtras } from "../src/lib/insights.ts";
import { redactor, supportReport } from "../src/lib/report.ts";
import { cleanSettings, parseInterval } from "../src/lib/settings.ts";
import { whoisFacts, whoisRecord } from "../src/lib/whois.ts";

const cases: { name: string; args: unknown[]; expected: unknown }[] = [];
const save = (name: string, args: unknown[], expected: unknown) => cases.push({ name, args, expected });
const inputs = [[], [null], [null, null], [10, null, 20, 14, null], [10, null, 14, 11, null], [20, 20, 20], [300, 300, 300], [20, null, null, null, 20]];
for (let seed = 1; seed <= 20; seed++) inputs.push(Array.from({ length: seed * 7 }, (_, i) => (i % 7 === 0 ? null : ((i * 17 + seed * 23) % 200) / 3)));
for (const rtts of inputs) {
  save("stats", [rtts], stats.stats(rtts));
  save("mos", [rtts], stats.mos(stats.stats(rtts)));
  save("jitter", [rtts], stats.jitterSeries(rtts));
  const times = rtts.map((_, i) => i);
  save("loss", [times, rtts, 10], stats.lossSeries(times, rtts, 10));
  for (const width of [0, 2, 4]) {
    const data = { xs: times, ys: rtts, js: stats.jitterSeries(rtts), ls: times.map(() => 0) };
    for (const radius of [0, 2]) {
      const breaks = times.filter((i) => i % 13 === 12);
      save("thin", [data, breaks, width, radius], stats.thin(data, new Set(breaks), width, radius));
    }
  }
}
for (const [min, max] of [[245, 475], [305, 312], [1, 30], [10, 30], [null, null], [95, 120], [1, 2]]) save("axis", [min, max], stats.latencyAxis(min, max));
for (const text of ["30s", " 5 min ", "1.5h", "2 Days", "15", "3s", "all", "soon", "5x", "", "a"]) save("span", [text], stats.parseSpan(text));
for (const text of ["2", "60", "500", "90 s", "1,5 s", ".25", "750ms", "1 minute", "0.01", "5m", "2 hours", "fast", ""]) save("interval", [text], parseInterval(text));
for (const raw of [null, "junk", {}, { mcpPort: "soon", timeoutMs: "", intervalMs: 750, protocol: "sctp" }, { timeoutMs: 5, packetSize: 9000, alertLossPct: 150, mcpPort: 80, historyDays: 0.2 }, { warnMs: 300, badMs: 200 }, { notifications: false, lossByTarget: true }]) for (const system of [false, true]) save("settings", [raw, system], cleanSettings(raw, system));
for (const events of [
  [{ t: 100, kind: "length", from: 8, to: 9 }, { t: 100, kind: "hop", ttl: 7, from: "a", to: "b" }, { t: 101, kind: "hop", ttl: 5, from: "c", to: "d" }, { t: 101, kind: "hop", ttl: 7, from: "b", to: "e" }],
  [{ t: 100, kind: "hop", ttl: 3, from: "a", to: "b" }, { t: 102, kind: "hop", ttl: 3, from: "b", to: "a" }, { t: 200, kind: "hop", ttl: 3, from: "a", to: "b" }, { t: 201, kind: "alert", text: "slow" }, { t: 202, kind: "length", from: 9, to: 8 }],
]) save("events", [events], groupEvents(events as Parameters<typeof groupEvents>[0]));
for (const text of ["org-name: Example\ninetnum: 192.0.2.0 - 192.0.2.255\ncountry: PL\n", "% terms\n# terms\n\n\nnetname: EXAMPLE\n", "inetnum: wide\norg-name: provider\ninetnum: narrow\norg-name: customer\n"]) {
  save("whoisFacts", [text], whoisFacts(text));
  save("whoisRecord", [text], whoisRecord(text));
}
// Stretches exercise the disk-history renderer independently of file I/O.
const tally = (t:number,sent:number,received:number,sum:number,min:number|null,max:number|null) => ({t,sent,received,sum,min,max,jitter_sum:2,jitter_count:received>1?received-1:0,last:received?sum/received:null});
for(const stretches of [[],[tally(1000000,4,3,90,20,40),tally(1060000,4,4,120,25,35),tally(1400000,4,0,0,null,null)]]) for(const width of [1,60]) {
 const result=stats.rangeGraph(stretches,width,120,2,1);
 save('rangeGraph',[stretches,width,120,2,1],result.data);
}
for(const interval of [500,1000,2500,5000,10000]) for(const seconds of [1,10,30]) {
  const probes=stats.alertProbes(seconds,interval);save("alertProbes",[seconds,interval],probes);
  for(const rtts of [[50,null,50],[],Array(70).fill(null),Array(70).fill(50)])save("breaches",[rtts,probes,30,20],stats.breaches(rtts,probes,30,20));
}
for(const value of [0,0.1,1,1.01,1.5,1.6,2.5,9,12,14,99,100,100.001])save("niceCeil",[value],stats.niceCeil(value));
// 0.1.13: incidents, event labels, extra hop statistics, loss explanations and redaction.
const timeline = [{ t: 90, kind: "hop", ttl: 2, from: "a", to: "b" }, { t: 100, kind: "alert", text: "slow" }, { t: 105, kind: "alert", text: "again" },
  { t: 130, kind: "recover", text: "ok" }, { t: 140, kind: "recover", text: "late" }, { t: 150, kind: "alert", text: "open" }, { t: 95, kind: "length", from: 8, to: 9 }];
save("restoreEvents", [timeline, [{ t: 100, kind: "alert", text: "slow" }, { t: 96, kind: "hop", ttl: 3, from: "x", to: "y" }], 98], restoreEvents(timeline as never, [{ t: 100, kind: "alert", text: "slow" }, { t: 96, kind: "hop", ttl: 3, from: "x", to: "y" }] as never, 98));
save("incidents", [timeline], incidents(timeline as never));
for (const group of groupEvents([...timeline, { t: 160, kind: "hop", ttl: 4, from: "c", to: "d" }] as never)) save("eventLabel", [group], eventLabel(group));
const entries = [{ from: 100, to: 130, text: "a", target: "one", observedUntil: 200 }, { from: 120, to: null, text: "b", target: "two", observedUntil: 180 },
  { from: 300, to: 310, text: "c", target: "one", observedUntil: 400 }, { from: 90, to: 95, text: "d", target: "three", observedUntil: 95 }];
save("correlateIncidents", [entries], correlateIncidents(entries));
for (const [hop, target, roundTrip] of [[[5, 5], [5, 5], false], [[20, 20], [20, 20], false], [[20, 15], [20, 20], false], [[20, 15], [20, 18], false], [[20, 20], [20, 0], true], [[20, 20], [20, 19], true], [[20, 20], [20, 20], true]] as const) {
  const h = { sent: hop[0], received: hop[1] }, t = { sent: target[0], received: target[1] };
  save("explainLoss", [h, t, roundTrip], explainLoss(h, t, roundTrip));
}
for (const [sent, received] of [[0, 0], [3, 2], [7, 7]]) save("replyAvailability", [sent, received], replyAvailability(sent, received));
for (let seed = 1; seed <= 6; seed++) {
  const times = Array.from({ length: seed * 9 }, (_, i) => 1000 + i + (i > seed * 4 ? 20 : 0));
  const rtts = times.map((_, i) => (i * seed) % 5 === 0 ? null : ((i * 37 + seed * 11) % 400) / 7);
  const rounds = seed % 2 ? times : times.filter((_, i) => i % 4 !== 1);
  save("sampleExtras", [times, rtts, 1000, rounds], sampleExtras(times, rtts, 1000, rounds));
}
for (const text of ["Hop 3: 192.0.2.1 (core1.example.net) to 192.0.2.10", "example.net and sub.example.net.", "192.0.2.1:443 x192.0.2.1 192.0.2.10", ""]) {
  const values = ["192.0.2.1", "example.net", "core1.example.net", "192.0.2.10", ""];
  save("redact", [values, text], redactor(values)(text));
}
const reportDetails = [
  { ttl: 1, addr: "192.0.2.1", total: { sent: 4, received: 3, sum: 30.5, min: 9.5, max: 11, jitter_sum: 1.5, jitter_count: 2, last: 11 }, median: 10, p95: 11, p99: 11, longest_lost: 1, longest_loss_seconds: 1 },
  { ttl: 0, addr: null, total: { sent: 4, received: 4, sum: 80, min: 19, max: 21, jitter_sum: 2, jitter_count: 3, last: 20 }, median: 20, p95: 21, p99: 21, longest_lost: 0, longest_loss_seconds: 0 },
];
const reportEvents = [{ t: 1000, kind: "alert", text: "Loss 25% to <example.net>" }, { t: 1030.5, kind: "recover", text: "fine" }, { t: 1040, kind: "hop", ttl: 1, from: "192.0.2.1", to: "192.0.2.9" }, { t: 5000, kind: "alert", text: "later" }];
for (const redacted of [false, true]) {
  const values = ["example.net", "192.0.2.1", "192.0.2.9"];
  save("supportReport", [reportDetails, reportEvents, values, redacted], supportReport({ target: "example.net (192.0.2.1)", from: 990, to: 1100.25, probe: "ICMP, interval 1000 ms",
    details: reportDetails as never, events: reportEvents as never, graph: "data:image/png;base64,iVBORw0KGgo=", redact: redacted ? redactor(values) : (t) => t, redacted, demo: redacted }));
}
writeFileSync(new URL("../src-tauri/fixtures/legacy-logic.json", import.meta.url), JSON.stringify(cases, null, 2) + "\n");

console.log(`Captured ${cases.length} original-function cases.`);

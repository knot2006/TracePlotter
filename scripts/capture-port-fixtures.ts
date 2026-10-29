// Freeze outputs of the original pure functions before replacing their callers with Rust.
import { writeFileSync } from "node:fs";
import * as stats from "../src/lib/stats.ts";
import { groupEvents } from "../src/lib/events.ts";
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
writeFileSync(new URL("../src-tauri/fixtures/legacy-logic.json", import.meta.url), JSON.stringify(cases, null, 2) + "\n");

console.log(`Captured ${cases.length} original-function cases.`);

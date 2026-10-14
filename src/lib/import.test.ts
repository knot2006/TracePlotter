import assert from "node:assert/strict";
import { test } from "node:test";
import { exportName, guessInterval, hostFromFileName, parseSamplesCsv } from "./import.ts";
import { setLocale } from "./time.ts";

const EXPORT = [
  "time,hop,ip,rtt_ms",
  "2026-10-14T01:51:22.000Z,1,192.168.1.1,0.4",
  "2026-10-14T01:51:24.500Z,1,192.168.1.1,0.5",
  "2026-10-14T01:51:22.000Z,2,203.0.113.9,",
  "2026-10-14T01:51:24.500Z,2,203.0.113.9,7.25",
  "",
].join("\n");

test("a samples export is read back as rounds, oldest first, hops in order", () => {
  const t = Date.parse("2026-10-14T01:51:22.000Z");
  assert.deepEqual(parseSamplesCsv(EXPORT), [
    { t, hops: [{ ttl: 1, addr: "192.168.1.1", rtt: 0.4 }, { ttl: 2, addr: "203.0.113.9", rtt: null }] },
    { t: t + 2500, hops: [{ ttl: 1, addr: "192.168.1.1", rtt: 0.5 }, { ttl: 2, addr: "203.0.113.9", rtt: 7.25 }] },
  ]);
});

test("unreadable lines are skipped and odd addresses dropped", () => {
  const text = "﻿time,hop,ip,rtt_ms\r\nsoon,1,10.0.0.1,1\r\n2026-10-14T01:51:22Z,0,10.0.0.1,1\r\n2026-10-14T01:51:22Z,3,<b>,-4\r\n";
  assert.deepEqual(parseSamplesCsv(text), [{ t: Date.parse("2026-10-14T01:51:22Z"), hops: [{ ttl: 3, addr: null, rtt: null }] }]);
});

test("other files are refused", () => {
  assert.throws(() => parseSamplesCsv("Hop,IP,Loss\n1,10.0.0.1,0\n"), /not a TracePlotter samples export/);
  assert.throws(() => parseSamplesCsv("time,hop,ip,rtt_ms\n"), /no samples/);
});

test("the interval and the host are worked out from the export", () => {
  assert.equal(guessInterval(parseSamplesCsv(EXPORT)), 2500);
  assert.equal(guessInterval([{ t: 0, hops: [] }]), 1000);
  assert.equal(hostFromFileName("traceplotter-example.com-20261014-015122.csv"), "example.com");
  assert.equal(hostFromFileName("my capture.CSV"), "my capture");
});

test("exports are named after the host, their kind and the date and time as the host writes them", () => {
  const at = new Date(2026, 9, 14, 16, 9, 18);
  setLocale("pl-PL");
  const name = exportName("example.com", "samples", "csv", at);
  assert.equal(name, "TracePlotter_example.com_samples_14.10.2026_16-09-18.csv");
  assert.equal(hostFromFileName(name), "example.com");
  setLocale("en-US");
  assert.equal(exportName("my_host", "graph", "png", at), "TracePlotter_my_host_graph_10-14-2026_4-09-18-PM.png");
  assert.equal(hostFromFileName(exportName("my_host", "samples", "csv", at)), "my_host");
  setLocale("cs-CZ");
  assert.equal(exportName("h", "report", "txt", at), "TracePlotter_h_report_14-10-2026_16-09-18.txt");
});

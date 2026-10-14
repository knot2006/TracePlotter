import assert from "node:assert/strict";
import { test } from "node:test";
import { DEFAULTS, cleanSettings, historyRounds, intervalLabel, parseInterval } from "./settings.ts";

test("missing and unusable settings get their defaults", () => {
  assert.deepEqual(cleanSettings(null, false), DEFAULTS);
  assert.deepEqual(cleanSettings("junk", false), DEFAULTS);
  assert.deepEqual(cleanSettings({ mcpPort: "soon", timeoutMs: "", intervalMs: 750, protocol: "sctp" }, false), DEFAULTS);
  assert.equal(cleanSettings({}, true).protocol, "winicmp");
});

test("settings are kept within their limits", () => {
  const clean = cleanSettings({ timeoutMs: 5, packetSize: 9000, alertLossPct: 150, mcpPort: 80, historyHours: 0.2 }, false);
  assert.equal(clean.timeoutMs, 100);
  assert.equal(clean.packetSize, 1024);
  assert.equal(clean.alertLossPct, 100);
  assert.equal(clean.mcpPort, 1024);
  assert.equal(clean.historyHours, 1);
  assert.equal(cleanSettings({ mcpPort: "4000", intervalMs: 2500, protocol: "tcp" }, false).mcpPort, 4000);
  assert.equal(cleanSettings({ intervalMs: 2500 }, false).intervalMs, 2500);
  assert.equal(cleanSettings({ protocol: "tcp" }, false).protocol, "tcp");
  assert.equal(cleanSettings({ notifications: false }, false).notifications, false);
  assert.equal(cleanSettings({ notifications: "no" }, false).notifications, true);
});

test("the Windows ICMP API is a default on Windows only", () => {
  assert.equal(cleanSettings({ protocol: "winicmp" }, true).protocol, "winicmp");
  assert.equal(cleanSettings({ protocol: "winicmp" }, false).protocol, "icmp");
});

test("red latency starts above yellow", () => {
  const clean = cleanSettings({ warnMs: 300, badMs: 200 }, false);
  assert.equal(clean.warnMs, 300);
  assert.equal(clean.badMs, 301);
});

test("a typed interval is read in seconds unless it says otherwise, from 0.1 to 60 seconds", () => {
  assert.equal(parseInterval("2"), 2000);
  assert.equal(parseInterval("1,5 s"), 1500);
  assert.equal(parseInterval(".25"), 250);
  assert.equal(parseInterval("750ms"), 750);
  assert.equal(parseInterval("1 minute"), 60_000);
  assert.equal(parseInterval("0.01"), 100);
  assert.equal(parseInterval("5m"), 60_000);
  assert.equal(parseInterval(intervalLabel(2500)), 2500);
  assert.equal(parseInterval(intervalLabel(1000)), 1000);
  assert.equal(parseInterval("2 hours"), null);
  assert.equal(parseInterval("fast"), null);
  assert.equal(parseInterval(""), null);
});

test("history is counted in rounds of the tab's interval, up to a cap", () => {
  assert.equal(historyRounds(24, 1000), 86_400);
  assert.equal(historyRounds(24, 10_000), 8640);
  assert.equal(historyRounds(168, 500), 345_600);
});

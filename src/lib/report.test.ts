import assert from "node:assert/strict";
import { test } from "node:test";
import { redactor, supportReport } from "./report.ts";

test("redaction handles overlapping IPv4/IPv6 names and regex characters consistently", () => {
  const redact = redactor(["1.1.1.1", "1.1.1.10", "2001:db8::1", "host.example", "a+b"]);
  const result = redact("1.1.1.10 1.1.1.1 2001:db8::1 host.example a+b");
  for (const secret of ["1.1.1.1", "2001:db8::1", "host.example", "a+b"]) assert.ok(!result.includes(secret));
  assert.equal(redact("host.example"), redact("host.example"));
  assert.ok(!redact("host.example.").includes("host.example"));
  assert.equal(redactor(["a"])("latency to a"), "latency to [address 1]");
  assert.equal(redactor(["host"])("anotherhost.example"), "anotherhost.example");
});

test("reports escape target and events, redact free text and refuse foreign image sources", () => {
  const result = supportReport({ target: '<script>host.example</script>', from: 0, to: 20, probe: "ICMP", details: [],
    events: [{ t: 10, kind: "alert", text: '<img src=x> host.example' }], graph: 'https://host.example/image',
    redact: redactor(["host.example"]), redacted: true, demo: true });
  assert.ok(!result.includes("host.example"));
  assert.ok(!result.includes("<script>"));
  assert.ok(result.includes("&lt;img src=x&gt;"));
  assert.ok(result.includes("DEMO"));
  assert.ok(result.includes("10 s within this period"));
});

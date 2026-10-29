import assert from "node:assert/strict";
import { test } from "node:test";
import { explainLoss, replyAvailability, sampleExtras } from "./insights.ts";

test("extra columns measure percentiles and bursts in the selected probes", () => {
  const extra = sampleExtras([0, 1, 2, 3, 4, 20], [10.04, null, null, 20, null, null], 1000);
  assert.deepEqual(extra, { median: 15, p95: 20, p99: 20, longest_lost: 2, longest_loss_seconds: 2 });
  assert.equal(sampleExtras([], [], 1000).median, null);
  assert.equal(sampleExtras([1, 3], [null, null], 1000, [1, 2, 3]).longest_lost, 1);
});

test("loss observations distinguish hop replies from destination replies", () => {
  const healthy = { sent: 100, received: 100 };
  const lossy = { sent: 100, received: 60 };
  assert.match(explainLoss(lossy, healthy), /not evidence of forwarding loss/);
  assert.match(explainLoss(lossy, lossy), /does not establish where loss occurred/);
  assert.match(explainLoss({ sent: 5, received: 0 }, healthy), /Too few/);
  assert.match(explainLoss(lossy, { sent: 100, received: 0 }, true), /does not establish a service outage/);
  assert.equal(replyAvailability(0, 0), "–");
  assert.equal(replyAvailability(100, 99), "99.00%");
});

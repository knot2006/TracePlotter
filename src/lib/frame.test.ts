import assert from "node:assert/strict";
import { test } from "node:test";
import { onFrame } from "./frame.ts";

// Frames come only when the test draws one, as they stop when a window is minimized.
const pending = new Map<number, () => void>();
let ids = 0;
globalThis.requestAnimationFrame = (callback) => {
  pending.set(++ids, () => callback(0));
  return ids;
};
globalThis.cancelAnimationFrame = (id) => void pending.delete(id);
function draw() {
  const callbacks = [...pending.values()];
  pending.clear();
  for (const callback of callbacks) callback();
}

test("calls between two frames run once, on the frame", () => {
  let runs = 0;
  const run = onFrame(() => runs++);
  run();
  run();
  run();
  assert.equal(runs, 0);
  draw();
  assert.equal(runs, 1);
  draw();
  assert.equal(runs, 1);
  run();
  draw();
  assert.equal(runs, 2);
});

test("a cancelled run does not happen, and the next call asks for a frame again", () => {
  let runs = 0;
  const run = onFrame(() => runs++);
  run();
  run.cancel();
  draw();
  assert.equal(runs, 0);
  run();
  draw();
  assert.equal(runs, 1);
});

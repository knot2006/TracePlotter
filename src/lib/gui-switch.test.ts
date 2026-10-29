import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { runInNewContext } from "node:vm";
import { test } from "node:test";

const component = readFileSync(new URL("./Settings.svelte", import.meta.url), "utf8");
const source = stripTypeScriptTypes(component.slice(component.indexOf("  async function switchInterface("), component.indexOf("  async function setBackground(")));

test("switching exports only TracePlotter storage and reports restart failures", async () => {
  const values: Record<string, string> = { "traceplotter.session": '[{"sessionId":"kept-id","host":"example.com"}]', "traceplotter.settings": '{"warnMs":150}', unrelated: "leave this behind" };
  const calls: { command: string; args: { values: Record<string, string> } }[] = [];
  let fail = false;
  const errors: string[] = [];
  const state = { interfaceBusy: false, interfaceError: "", localStorage: { ...values, getItem: (key: string) => values[key] ?? null },
    showToast: (message: string) => errors.push(message),
    invoke: async (command: string, args: { values: Record<string, string> }) => { calls.push({ command, args }); if (fail) throw new Error("restart failed"); },
  };
  const run = runInNewContext(`${source}\nswitchInterface`, state) as () => Promise<void>;
  await run();
  assert.equal(calls[0].command, "switch_gui");
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args.values)), { "traceplotter.session": values["traceplotter.session"], "traceplotter.settings": values["traceplotter.settings"] });
  assert.equal(state.interfaceBusy, true);
  fail = true;
  await run();
  assert.match(state.interfaceError, /restart failed/);
  assert.equal(state.interfaceBusy, false);
  assert.match(errors[0], /restart failed/);
});

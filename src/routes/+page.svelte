<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { Menu } from "@tauri-apps/api/menu";
  import { onMount } from "svelte";
  import Summary from "$lib/Summary.svelte";
  import TraceTab, { COLUMNS, DEFAULT_HIDDEN } from "$lib/TraceTab.svelte";
  import { BAD_MS, WARN_MS, parseSpan, spanLabel } from "$lib/stats";
  import {
    closeTrace,
    restartTrace,
    setPaused,
    startTrace,
    type Protocol,
    type Trace,
    type TraceConfig,
  } from "$lib/trace";

  const DEFAULT_PORT = { icmp: 0, udp: 33434, tcp: 443 };
  const SESSION_KEY = "traceplotter.session";
  const COLUMNS_KEY = "traceplotter.hiddenColumns";
  const SPAN_KEY = "traceplotter.timeSpan";
  /** Time spans offered in the list, in seconds; 0 is everything. Any other can be typed. */
  const SPANS = [60, 300, 600, 1800, 3600, 21600, 0];

  // The open tabs, restored on the next launch. Holds the same objects as `trace.config`.
  let session: TraceConfig[] = [];
  const saveSession = () => localStorage.setItem(SESSION_KEY, JSON.stringify(session));
  const same = (a: Pick<TraceConfig, "host" | "protocol" | "port">, b: typeof a) =>
    a.host === b.host && a.protocol === b.protocol && a.port === b.port;

  let traces = $state.raw<Trace[]>([]);
  // The open tab: a trace's id, the overview of all targets, or none (a new target is being typed).
  let activeId = $state<number | "summary" | null>(null);
  // Traces are plain objects mutated in place; bumping `rev` is what redraws.
  let rev = $state(0);
  const bump = () => rev++;

  // Toolbar fields. They show the open tab's target; editing them describes a new one.
  let host = $state("");
  let protocol = $state<Protocol>("icmp");
  let port = $state(0);
  let intervalMs = $state(1000);
  // How much of the newest history the table and graphs cover; remembered between launches.
  // In seconds, 0 for everything.
  const storedSpan = Number(localStorage.getItem(SPAN_KEY) ?? 600);
  const initialSpan = Number.isFinite(storedSpan) && storedSpan >= 0 ? storedSpan : 600;
  let focusSeconds = $state(initialSpan);
  // What the time span box shows: the span in words, or what is being typed.
  let spanText = $state(spanLabel(initialSpan));

  /** Takes over what was typed or picked ("30s", "5m", "2h", "all") and writes it out in words. */
  function applySpan() {
    const typed = parseSpan(spanText);
    if (typed !== null) {
      focusSeconds = typed;
      localStorage.setItem(SPAN_KEY, String(typed));
    }
    spanText = spanLabel(focusSeconds);
  }
  let busy = $state(false);
  let error = $state("");
  let notice = $state("");
  // Keys of the hop table columns the user has switched off in View > Columns.
  let hidden = $state<string[]>(DEFAULT_HIDDEN);
  let hostInput: HTMLInputElement;

  const active = $derived(traces.find((t) => t.id === activeId));
  // The open tab's component, for the menu's export actions.
  let tab = $state<TraceTab>();

  const target = $derived({ host: host.trim().toLowerCase(), protocol, port: protocol === "icmp" ? 0 : port });
  // The open tab, while the toolbar still describes it. Then the round button pauses or
  // resumes it; otherwise the button starts the target typed into the toolbar.
  const current = $derived(active && same(active.config, target) ? active : undefined);
  const running = $derived(rev >= 0 && !!current && !current.paused);

  $effect(() => {
    if (!active) return;
    ({ host, protocol, port, intervalMs } = active.config);
  });

  /** Menu action that needs an open tab. */
  const onTab = (action: (tab: TraceTab) => void) => () => {
    if (tab) action(tab);
    else error = "Open a target first.";
  };

  function toggleColumn(key: string) {
    hidden = hidden.includes(key) ? hidden.filter((k) => k !== key) : [...hidden, key];
    localStorage.setItem(COLUMNS_KEY, JSON.stringify(hidden));
    // The ticks in View > Columns are fixed when the menu is built, and the table header's
    // own menu can change a column too, so the menu is built again.
    buildMenu().catch((e) => (error = `Menu failed: ${e}`));
  }

  async function togglePause(trace: Trace) {
    try {
      await setPaused(trace, !trace.paused, bump);
    } catch (e) {
      trace.error = `Could not resume: ${e}`;
    }
    rev++;
  }

  async function allowIcmp() {
    error = notice = "";
    const ok = confirm(
      "Add Windows Firewall rules that let ICMP replies reach TracePlotter?\n\n" +
        "They allow inbound echo reply, destination unreachable and time exceeded, for IPv4 and IPv6. " +
        "Without them ICMP traces lose hops, including the final one.",
    );
    if (!ok) return;
    try {
      await invoke("allow_icmp_replies");
      notice = "Firewall rules added.";
    } catch (e) {
      error = `Firewall rules not added: ${e}`;
    }
  }

  async function buildMenu() {
    const menu = await Menu.new({
      items: [
        {
          text: "File",
          items: [
            { id: "new", text: "New target", accelerator: "CmdOrCtrl+T", action: newTarget },
            { item: "Separator" },
            { id: "txt", text: "Export MTR report (.txt)…", action: onTab((t) => t.exportTxt()) },
            // Opens the print dialog; "Save as PDF" there writes the file.
            { id: "pdf", text: "Export page as PDF…", accelerator: "CmdOrCtrl+P", action: onTab(() => window.print()) },
            { id: "csv", text: "Export samples (.csv)…", action: onTab((t) => t.exportCsv()) },
            { id: "png", text: "Export graph (.png)…", action: onTab((t) => t.exportPng()) },
            { item: "Separator" },
            { id: "close", text: "Close tab", accelerator: "CmdOrCtrl+W", action: () => active && close(active) },
            { item: "CloseWindow", text: "Exit" },
          ],
        },
        {
          text: "View",
          items: [
            {
              text: "Columns",
              items: COLUMNS.map((column) => ({
                id: `column-${column.key}`,
                text: column.label,
                checked: !hidden.includes(column.key),
                action: () => toggleColumn(column.key),
              })),
            },
          ],
        },
        {
          text: "Trace",
          items: [
            { id: "pause", text: "Pause / resume", action: () => active && togglePause(active) },
            { id: "alerts", text: "Alerts…", action: onTab((t) => t.openAlerts()) },
            { id: "restart", text: "Restart and erase history…", action: () => active && restart(active) },
          ],
        },
        {
          text: "Tools",
          items: [{ id: "firewall", text: "Allow ICMP replies in Windows Firewall…", action: allowIcmp }],
        },
      ],
    });
    await menu.setAsAppMenu();
  }

  /** Opens a new tab for the session. Tabs are never merged, even for the same target. */
  async function open(config: TraceConfig) {
    const trace = await startTrace(config, bump);
    traces = [...traces, trace];
    activeId = trace.id;
    return trace;
  }

  /** Erases the tab's history and starts its trace again from nothing. */
  async function restart(trace: Trace) {
    if (!confirm(`Restart the trace to ${trace.config.host}?\n\nIts history is erased.`)) return;
    try {
      await restartTrace(trace, bump);
    } catch (e) {
      trace.error = `Could not restart: ${e}`;
    }
    saveSession();
    rev++;
  }

  /** The round button: pause or resume the open tab, or start what the toolbar describes. */
  async function go(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = notice = "";
    try {
      if (current) {
        await togglePause(current);
      } else {
        const { config } = await open({
          sessionId: crypto.randomUUID(),
          ...target,
          intervalMs,
          alertLatencyMs: 0,
          alertLossPct: 0,
        });
        session = [...session, config];
        saveSession();
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Applies a new interval to the open tab; probing restarts with it at the next round. */
  async function changeInterval() {
    if (!current) return;
    current.config.intervalMs = intervalMs;
    saveSession();
    if (current.paused) return;
    try {
      await setPaused(current, true, bump);
      await setPaused(current, false, bump);
    } catch (e) {
      current.error = `Could not restart: ${e}`;
    }
    rev++;
  }

  function newTarget() {
    activeId = null;
    host = "";
    hostInput.focus();
  }

  /** Closes the tab and erases its history. */
  function close(trace: Trace) {
    closeTrace(trace).catch((e) => (error = `History of ${trace.config.host} not erased: ${e}`));
    const index = traces.indexOf(trace);
    traces = traces.filter((t) => t !== trace);
    if (activeId === trace.id) activeId = (traces[index] ?? traces[index - 1])?.id ?? null;
    session = session.filter((c) => c !== trace.config);
    saveSession();
  }

  onMount(async () => {
    // The webview's own right-click menu offers "Save as" and "Print"; exports live in File.
    document.addEventListener("contextmenu", (event) => {
      if (!(event.target instanceof HTMLInputElement)) event.preventDefault();
    });
    try {
      const stored = JSON.parse(localStorage.getItem(COLUMNS_KEY) ?? "null");
      if (Array.isArray(stored)) hidden = stored;
    } catch {
      // Keep the defaults.
    }
    buildMenu().catch((e) => (error = `Menu failed: ${e}`));
    try {
      const stored = JSON.parse(localStorage.getItem(SESSION_KEY) ?? "[]");
      session = Array.isArray(stored) ? stored : [];
    } catch {
      session = [];
    }
    // Tabs saved before sessions had ids get one now. It must be saved at once: the id
    // names the history file, so a new id on every launch would start every launch empty.
    for (const config of session) config.sessionId ??= crypto.randomUUID();
    saveSession();
    // All at once, so one slow name lookup or long history does not hold up the others.
    // A tab that cannot start now (no network yet) still opens, paused, with its error.
    const restored = await Promise.all(
      session.map((config) =>
        startTrace(config, bump, true).catch((e) => {
          error = `Could not restore ${config.host}: ${e}`;
          return null;
        }),
      ),
    );
    // In saved order, ahead of any tab the user opened meanwhile.
    traces = [...restored.filter((trace) => trace !== null), ...traces];
    activeId ??= traces[0]?.id ?? null;
  });
</script>

<header>
  <div class="tabs" role="tablist">
    {#if traces.length}
      <div class="tab" class:active={activeId === "summary"}>
        <!-- The host box is emptied: with no tab open it describes a new target, and leaving
             the last tab's host there would make the round button open a copy of it. -->
        <button
          class="all"
          role="tab"
          aria-selected={activeId === "summary"}
          onclick={() => {
            activeId = "summary";
            host = "";
          }}
        >
          ☰ All targets
        </button>
      </div>
    {/if}
    {#each traces as trace (trace.id)}
      {@const { host, protocol, port } = trace.config}
      {@const state = rev < 0 ? "" : trace.alerting ? "alerting" : trace.paused ? "paused" : "running"}
      <div class="tab" class:active={trace.id === activeId}>
        <button role="tab" aria-selected={trace.id === activeId} onclick={() => (activeId = trace.id)}>
          <span class="dot {state}" role="img" aria-label={state}></span>
          {host}
          <small>{protocol.toUpperCase()}{protocol === "icmp" ? "" : `:${port}`}</small>
        </button>
        <button class="close" aria-label="Close {host}" onclick={() => close(trace)}>×</button>
      </div>
    {/each}
    <button class="add" aria-label="New target" title="New target (Ctrl+T)" onclick={newTarget}>+</button>
  </div>

  <form class="toolbar" onsubmit={go}>
    <input
      class="host"
      bind:this={hostInput}
      bind:value={host}
      placeholder="Host or IP, e.g. 1.1.1.1"
      aria-label="Host or IP"
      required
    />
    <select bind:value={protocol} aria-label="Protocol" onchange={() => (port = DEFAULT_PORT[protocol])}>
      <option value="icmp">ICMP</option>
      <option value="udp">UDP</option>
      <option value="tcp">TCP</option>
    </select>
    {#if protocol !== "icmp"}
      <input class="port" type="number" min="1" max="65535" bind:value={port} aria-label="Port" required />
    {/if}
    <button
      class="go"
      class:running
      disabled={busy}
      aria-label={running ? "Pause trace" : current ? "Resume trace" : "Start trace"}
      title={running ? "Pause trace" : current ? "Resume trace" : "Start trace"}
    >
      {running ? "⏸" : "▶"}
    </button>
    {#if current}
      <button
        class="restart"
        type="button"
        aria-label="Restart trace and erase its history"
        title="Restart trace and erase its history"
        onclick={() => restart(current)}>↻</button
      >
    {/if}
    <label>
      Interval
      <select bind:value={intervalMs} onchange={changeInterval}>
        <option value={500}>0.5 seconds</option>
        <option value={1000}>1 second</option>
        <option value={2500}>2.5 seconds</option>
        <option value={5000}>5 seconds</option>
        <option value={10000}>10 seconds</option>
      </select>
    </label>
    <label>
      Time span
      <!-- Emptied on focus: the list only offers entries that match the text in the box.
           Enter must not reach the form, where it would start or pause the trace. -->
      <input
        class="span"
        list="spans"
        placeholder="30s, 5m, 2h, all"
        title="How much of the newest history the table and graphs cover. Pick one or type, for example 30s, 5m, 2h or all."
        bind:value={spanText}
        onfocus={() => (spanText = "")}
        onchange={applySpan}
        onblur={applySpan}
        onkeydown={(event) => {
          if (event.key !== "Enter") return;
          event.preventDefault();
          event.currentTarget.blur();
        }}
      />
      <datalist id="spans">
        {#each SPANS as seconds}<option value={spanLabel(seconds)}></option>{/each}
      </datalist>
    </label>
    <div class="legend" role="img" aria-label="Latency colours: green under {WARN_MS} ms, yellow under {BAD_MS} ms, red above">
      <span class="good"></span>
      <span class="warn">{WARN_MS}ms</span>
      <span class="bad">{BAD_MS}ms</span>
    </div>
    {#if error}<span class="error" role="alert">{error}</span>{/if}
    {#if notice}<span role="status">{notice}</span>{/if}
  </form>
</header>

<main>
  {#if active}
    {#key active.id}
      <TraceTab
        bind:this={tab}
        trace={active}
        {rev}
        {hidden}
        {focusSeconds}
        onsave={saveSession}
        oncolumn={toggleColumn}
      />
    {/key}
  {:else if activeId === "summary" && traces.length}
    <Summary {traces} {rev} {focusSeconds} onopen={(trace) => (activeId = trace.id)} />
  {:else}
    <p class="empty">Type a host above and press the green button. Each target opens in its own tab.</p>
  {/if}
</main>

<style>
  :global(:root) {
    --bg: #0f1319;
    --panel: #161b24;
    --line: #232a36;
    --text: #e6eaf0;
    --muted: #9aa4b2;
    --accent: #2f8fe6;
    --good: #22c55e;
    --warn: #eab308;
    --bad: #ef4444;
    color-scheme: dark;
  }
  :global(body) {
    margin: 0;
    height: 100vh;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    color: var(--text);
    font: 14px/1.4 "Segoe UI Variable", "Segoe UI", system-ui, sans-serif;
  }
  :global(body > div) {
    display: flex !important;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  :global(:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  header {
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }

  .tabs {
    display: flex;
    align-items: stretch;
    overflow-x: auto;
    border-bottom: 2px solid var(--accent);
  }
  .tab {
    display: flex;
    align-items: center;
    color: var(--muted);
  }
  .tab.active {
    background: var(--accent);
    color: #fff;
  }
  .tab button,
  .add {
    all: unset;
    cursor: pointer;
    white-space: nowrap;
  }
  .tab button {
    padding: 6px 4px 6px 12px;
  }
  .tab .all {
    padding-right: 12px;
  }
  .tab button:focus-visible,
  .add:focus-visible {
    outline: 2px solid var(--text);
    outline-offset: -2px;
  }
  .tab small {
    margin-left: 4px;
    opacity: 0.75;
  }
  .dot {
    display: inline-block;
    width: 9px;
    height: 9px;
    margin-right: 4px;
    border-radius: 50%;
    border: 1px solid rgba(0, 0, 0, 0.35);
    background: var(--muted);
  }
  .dot.running {
    background: var(--good);
  }
  .dot.paused {
    background: var(--warn);
  }
  .dot.alerting {
    background: var(--bad);
  }
  .tab .close {
    padding: 4px 10px 6px 4px;
    font-size: 16px;
    opacity: 0.75;
  }
  .tab .close:hover {
    opacity: 1;
  }
  .add {
    padding: 4px 12px;
    font-size: 18px;
    color: var(--muted);
  }
  .add:hover {
    color: var(--text);
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 12px;
    align-items: center;
    padding: 8px 12px;
  }
  .toolbar label {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--muted);
  }
  input,
  :global(select) {
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 5px 8px;
  }
  .host {
    flex: 1 1 260px;
    min-width: 160px;
  }
  .port {
    width: 76px;
  }
  .span {
    width: 120px;
  }
  .go {
    width: 34px;
    height: 34px;
    padding: 0;
    border: 2px solid #0b3d1e;
    border-radius: 50%;
    background: var(--good);
    color: #062611;
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
  }
  .go.running {
    border-color: #4a3703;
    background: var(--warn);
    color: #2b2001;
  }
  .restart {
    width: 30px;
    height: 30px;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 50%;
    background: var(--bg);
    color: var(--text);
    font-size: 17px;
    line-height: 1;
    cursor: pointer;
  }
  .restart:hover {
    border-color: var(--bad);
    color: var(--bad);
  }
  .go:disabled {
    opacity: 0.6;
  }
  .legend {
    display: flex;
    width: 150px;
    height: 16px;
    border-radius: 3px;
    overflow: hidden;
    font-size: 10px;
    line-height: 16px;
    color: #111;
  }
  .legend span {
    flex: 1;
    padding-left: 2px;
  }
  .legend .good {
    background: var(--good);
  }
  .legend .warn {
    background: var(--warn);
  }
  .legend .bad {
    background: var(--bad);
  }
  .error {
    color: #f87171;
  }

  main {
    flex: 1;
    min-height: 0;
  }
  @media print {
    :global(body) {
      height: auto;
      print-color-adjust: exact;
    }
    header {
      display: none;
    }
  }
  .empty {
    margin: 0;
    padding: 64px 16px;
    text-align: center;
    color: var(--muted);
  }
</style>

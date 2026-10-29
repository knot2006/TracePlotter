<script lang="ts">
  import { getVersion } from "@tauri-apps/api/app";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { Menu } from "@tauri-apps/api/menu";
  import { onMount } from "svelte";
  import { AGENT_MIN_INTERVAL_MS, describeTrace, findTrace, READ_TOOLS, type Arguments } from "../lib/agent";
  import { onFrame } from "../lib/frame";
  import { BAND_FILLS } from "../lib/HopGraph.svelte";
  import { guessInterval, hostFromFileName, parseSamplesCsv } from "../lib/import";
  import ContextMenu from "../lib/ContextMenu.svelte";
  import Toast, { showToast } from "../lib/Toast.svelte";
  import Picker from "../lib/Picker.svelte";
  import Settings from "../lib/Settings.svelte";
  import {
    INTERVALS,
    intervalLabel,
    MACOS,
    parseInterval,
    RAW_SOCKET_RIGHTS,
    SYSTEM_ICMP,
    WINDOWS,
    settings,
    type Settings as AppSettings,
  } from "../lib/settings";
  import Summary from "../lib/Summary.svelte";
  import TraceTab, { COLUMNS, DEFAULT_HIDDEN } from "../lib/TraceTab.svelte";
  import { parseSpan, spanLabel, spanShort } from "../lib/stats";
  import { setLocale } from "../lib/time";
  import {
    closeTrace,
    historyRange,
    PROTOCOLS,
    protocolLabel,
    restartTrace,
    setPaused,
    startTrace,
    usesPort,
    usesRawSocket,
    type Protocol,
    type Trace,
    type TraceConfig,
  } from "../lib/trace";

  const DEFAULT_PORT = { winicmp: 0, icmp: 0, udp: 33434, tcp: 443 };
  const SESSION_KEY = "traceplotter.session";
  const COLUMNS_KEY = "traceplotter.hiddenColumns";
  const SPAN_KEY = "traceplotter.timeSpan";
  /** Time spans offered in the list, in seconds; 0 is everything. Any other can be typed. */
  const SPANS = [60, 300, 600, 1800, 3600, 21600, 86400, 604800, 0];

  // The open tabs, restored on the next launch. Holds the same objects as `trace.config`.
  let session: TraceConfig[] = [];
  // Also the open tab: a restart gives its trace a new session id, which is how it is remembered.
  const saveSession = () => {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session));
    saveActive();
  };
  const same = (a: Pick<TraceConfig, "host" | "protocol" | "port">, b: typeof a) =>
    a.host === b.host && a.protocol === b.protocol && a.port === b.port;

  let traces = $state.raw<Trace[]>([]);
  // The open tab: a trace's id, the overview of all targets, the settings, or none (a new
  // target is being typed).
  let activeId = $state<number | "summary" | "settings" | null>(null);
  // Traces are plain objects mutated in place; bumping `rev` is what redraws. A minimized
  // window keeps probing but skips the redraws, and catches up once it is shown again.
  let rev = $state(0);
  const bump = onFrame(() => rev++);

  // Toolbar fields. They show the open tab's target; editing them describes a new one,
  // which starts from the defaults in the settings.
  let host = $state("");
  let protocol = $state<Protocol>(settings.protocol);
  let port = $state(DEFAULT_PORT[settings.protocol]);
  let intervalMs = $state(settings.intervalMs);
  // How much of the newest history the table and graphs cover; remembered between launches.
  // In seconds, 0 for everything.
  const storedSpan = Number(localStorage.getItem(SPAN_KEY) ?? 600);
  // Whole seconds only: spanLabel and spanShort need a unit that divides the span.
  const initialSpan = Number.isInteger(storedSpan) && storedSpan >= 0 ? storedSpan : 600;
  let focusSeconds = $state(initialSpan);

  function changeSpan(seconds: number) {
    focusSeconds = seconds;
    localStorage.setItem(SPAN_KEY, String(seconds));
  }
  let busy = $state(false);

  // Updates. A newer release found on GitHub is offered in a bar at the top; nothing is
  // downloaded until the user asks for it.
  const AUTO_CHECK_KEY = "traceplotter.checkUpdatesAtStart";
  let version = $state("");
  let autoCheck = $state(localStorage.getItem(AUTO_CHECK_KEY) !== "no");
  let update = $state<{ version: string; notes: string } | null>(null);
  let updating = $state(false);

  /** Looks for a newer release. Only a check the user asked for reports "none" or an error. */
  async function checkUpdates(asked: boolean) {
    try {
      update = await invoke<{ version: string; notes: string } | null>("check_update");
      if (asked && !update) showToast(`TracePlotter ${version} is the latest version.`);
    } catch (e) {
      if (asked) showToast(String(e), true);
    }
  }

  /** Downloads and verifies the update, replaces the exe and restarts the app. */
  async function installUpdate() {
    updating = true;
    try {
      await invoke("install_update");
    } catch (e) {
      showToast(`Update failed: ${e}`, true);
      updating = false;
    }
  }

  // Administrator rights. The app starts without them; raw sockets and firewall rules need
  // them on Windows, so it offers to restart with them where those are asked for. Elsewhere
  // raw sockets need CAP_NET_RAW, which the app can give its exe and then restart. Taken as
  // given until the backend has answered.
  let elevated = $state(true);
  // Whether that right can be given at all; an AppImage cannot have it.
  let canElevate = $state(true);

  /**
   * Starts the app again as Administrator, after the Windows prompt, and closes this one.
   * Elsewhere gives the exe CAP_NET_RAW, after the password prompt, and restarts into it.
   */
  function restartElevated() {
    invoke("restart_elevated").catch((e) => showToast(String(e), true));
  }

  // Agent access: the MCP server that lets AI agents read the traces and start new ones.
  // Off until the user turns it on; it listens on this machine's loopback address only.
  const MCP_KEY = "traceplotter.mcpServer";
  let mcpOn = $state(localStorage.getItem(MCP_KEY) === "on");
  let mcpUrl = $state<string | null>(null);

  /** Starts or stops the MCP server. `chosen` is the user's choice, which is remembered and confirmed. */
  async function setAgentAccess(on: boolean, chosen: boolean) {
    try {
      mcpUrl = await invoke<string | null>("mcp_serve", { port: on ? settings.mcpPort : null });
      mcpOn = on;
      if (chosen) localStorage.setItem(MCP_KEY, on ? "on" : "off");
      if (chosen) showToast(mcpUrl ? `Agents can connect to ${mcpUrl}` : "Agent access is off.");
    } catch (e) {
      // The server that ran before, if any, was stopped to make way for this one.
      mcpUrl = null;
      showToast(`Agent access not started: ${e}`, true);
    }
    buildMenu().catch((e) => showToast(`Menu failed: ${e}`, true));
  }

  function copyAgentAddress() {
    if (!mcpUrl) return;
    navigator.clipboard
      .writeText(mcpUrl)
      .then(() => showToast("Agent address copied."))
      .catch((e) => showToast(`Not copied: ${e}`, true));
  }

  function setAutoCheck(on: boolean) {
    autoCheck = on;
    localStorage.setItem(AUTO_CHECK_KEY, on ? "yes" : "no");
  }

  /** Opens the settings. The host box is emptied, as it is for All targets. */
  function openSettings() {
    activeId = "settings";
    host = "";
  }

  /** Sets the toolbar to what a new target starts with. */
  function toDefaults() {
    protocol = settings.protocol;
    port = DEFAULT_PORT[protocol];
    intervalMs = settings.intervalMs;
  }

  /** Puts a setting changed in the Settings tab to work where a new value is not simply read. */
  function settingChanged(key: keyof AppSettings) {
    // With no target open the toolbar describes a new one.
    if (!active) toDefaults();
    if (key === "mcpPort" && mcpOn) setAgentAccess(true, false);
    if (key === "historyDays") {
      invoke("set_history_days", { days: settings.historyDays }).catch((e) => showToast(String(e), true));
    }
    // The toolbar's legend shows the latency limits.
    rev++;
  }

  /** A new tab's settings: the target and interval given, the rest from the defaults. */
  const newConfig = (target: Pick<TraceConfig, "host" | "protocol" | "port">, intervalMs: number): TraceConfig => ({
    sessionId: crypto.randomUUID(),
    ...target,
    intervalMs,
    timeoutMs: settings.timeoutMs,
    packetSize: settings.packetSize,
    alertLatencyMs: settings.alertLatencyMs,
    alertLossPct: settings.alertLossPct,
    alertSeconds: settings.alertSeconds,
  });

  /** The intervals the toolbar offers that agents may use. */
  const AGENT_INTERVALS = INTERVALS.filter((ms) => ms >= AGENT_MIN_INTERVAL_MS);

  /**
   * Starts tracing a host for an agent, in a new tab next to the others, or resumes the
   * paused tab that already traces it. The open tab stays as it is.
   */
  async function agentStart(args: Arguments) {
    const host = String(args.host ?? "").trim().toLowerCase();
    if (!host) throw new Error("start_trace needs a host");
    const protocol = (args.protocol ?? (SYSTEM_ICMP ? "winicmp" : "icmp")) as Protocol;
    if (!(protocol in PROTOCOLS) || (protocol === "winicmp" && !SYSTEM_ICMP)) {
      throw new Error(`protocol ${protocol} is not available here`);
    }
    const port = usesPort(protocol) ? Number(args.port ?? DEFAULT_PORT[protocol]) : 0;
    // Never faster than a round a second: many routers throttle replies to more, which reads as loss.
    const asked = Math.max(AGENT_MIN_INTERVAL_MS, Number(args.interval_ms) || AGENT_MIN_INTERVAL_MS);
    const interval = AGENT_INTERVALS.find((ms) => ms >= asked) ?? AGENT_INTERVALS[AGENT_INTERVALS.length - 1];
    // Told to the agent, so that it does not take the slower interval for a fault.
    const slowed =
      Number(args.interval_ms) < AGENT_MIN_INTERVAL_MS
        ? { note: `Probing once a second, not every ${args.interval_ms} ms: many routers throttle faster probes.` }
        : {};
    const existing = traces.find((t) => same(t.config, { host, protocol, port }));
    if (existing) {
      if (existing.paused) await togglePause(existing);
      return { started: existing.paused ? "could not resume" : "resumed", ...(await describeTrace(existing, 600)) };
    }
    const trace = await startTrace(newConfig({ host, protocol, port }, interval), bump);
    traces = [...traces, trace];
    session = [...session, trace.config];
    saveSession();
    return { started: "new tab", ...slowed, ...(await describeTrace(trace, 600)) };
  }

  /** Pauses a tab for an agent; its history stays. */
  async function agentStop(args: Arguments) {
    const trace = findTrace(traces, args.target);
    if (!trace.paused) await togglePause(trace);
    return describeTrace(trace, 600);
  }

  /** Runs a tool call the MCP server passed on, and sends its answer back. */
  async function answerAgent({ id, tool, arguments: args }: { id: number; tool: string; arguments: Arguments }) {
    try {
      const run = tool === "start_trace" ? agentStart : tool === "stop_trace" ? agentStop : null;
      const result = run ? await run(args) : await READ_TOOLS[tool]?.(traces, args, historyRange);
      if (result === undefined) throw new Error(`unknown tool ${tool}`);
      await invoke("mcp_respond", { id, result });
    } catch (e) {
      await invoke("mcp_respond", { id, error: e instanceof Error ? e.message : String(e) });
    }
  }

  // Keys of the hop table columns the user has switched off in View > Columns.
  let hidden = $state<string[]>(DEFAULT_HIDDEN);
  let hostInput: HTMLInputElement;

  const active = $derived(traces.find((t) => t.id === activeId));

  // The tab that was open, to open it again on the next launch: its session id, or
  // "summary" for All targets. Not written until the saved tabs are back, so that the
  // empty start does not overwrite it.
  const ACTIVE_KEY = "traceplotter.activeTab";
  let tabsRestored = $state(false);
  function saveActive() {
    if (!tabsRestored) return;
    // The settings are not a tab to come back to: the next launch opens the first target.
    localStorage.setItem(ACTIVE_KEY, activeId === "summary" ? "summary" : (active?.config.sessionId ?? ""));
  }
  $effect(saveActive);
  // The open tab's component, for the menu's export actions.
  let tab = $state<TraceTab>();
  // Whether the open tab's latency bars have room to show, for the legend that explains them.
  let barShown = $state(false);
  let summary = $state<Summary>();

  const target = $derived({ host: host.trim().toLowerCase(), protocol, port: usesPort(protocol) ? port : 0 });
  // The open tab, while the toolbar still describes it. Then the round button pauses or
  // resumes it; otherwise the button starts the target typed into the toolbar.
  const current = $derived(active && same(active.config, target) ? active : undefined);
  const running = $derived(rev >= 0 && !!current && !current.paused);
  // A raw-socket packet type is picked in the toolbar or used by a tab, and cannot work.
  const wantsAdmin = $derived(
    !elevated && (usesRawSocket(protocol) || traces.some((trace) => usesRawSocket(trace.config.protocol))),
  );
  // The latency limits in the legend. `settings` is a plain object, so they are read again
  // when `rev` changes.
  const limits = $derived(rev >= 0 ? { warn: settings.warnMs, bad: settings.badMs } : { warn: 0, bad: 0 });

  $effect(() => {
    if (!active) return;
    ({ host, protocol, port, intervalMs } = active.config);
  });

  /** Menu action that needs an open tab. */
  const onTab = (action: (tab: TraceTab) => void) => () => {
    if (tab) action(tab);
    else showToast("Open a target first.", true);
  };
  /** An export of the open tab, or with All targets open, of its compare graph. */
  const onTabOrSummary = (action: (tab: TraceTab) => void, onSummary: (summary: Summary) => void) => () => {
    if (activeId === "summary" && summary) onSummary(summary);
    else onTab(action)();
  };

  function toggleColumn(key: string) {
    hidden = hidden.includes(key) ? hidden.filter((k) => k !== key) : [...hidden, key];
    localStorage.setItem(COLUMNS_KEY, JSON.stringify(hidden));
    // The ticks in View > Columns are fixed when the menu is built, and the table header's
    // own menu can change a column too, so the menu is built again.
    buildMenu().catch((e) => showToast(`Menu failed: ${e}`, true));
  }

  function resetHidden() {
    hidden = DEFAULT_HIDDEN;
    localStorage.removeItem(COLUMNS_KEY);
    buildMenu().catch((e) => showToast(`Menu failed: ${e}`, true));
  }

  async function togglePause(trace: Trace) {
    try {
      await setPaused(trace, !trace.paused, bump);
    } catch (e) {
      trace.error = `Could not resume: ${e}`;
    }
    // Paused or running is part of the saved tab, so it comes back the same way.
    saveSession();
    rev++;
  }

  async function allowIcmp() {
    if (!elevated) {
      const restart = confirm(
        "Adding Windows Firewall rules needs Administrator rights.\n\n" +
          "Restart TracePlotter as Administrator? Open tabs and their history are kept.",
      );
      if (restart) restartElevated();
      return;
    }
    const ok = confirm(
      "Add Windows Firewall rules that let ICMP replies reach TracePlotter?\n\n" +
        "They allow inbound echo reply, destination unreachable and time exceeded, for IPv4 and IPv6. " +
        "Without them ICMP traces lose hops, including the final one.",
    );
    if (!ok) return;
    try {
      await invoke("allow_icmp_replies");
      showToast("Firewall rules added.");
    } catch (e) {
      showToast(`Firewall rules not added: ${e}`, true);
    }
  }

  async function buildMenu() {
    const menu = await Menu.new({
      items: [
        {
          text: "File",
          items: [
            { id: "new", text: "New target", accelerator: "CmdOrCtrl+T", action: newTarget },
            { id: "close", text: "Close tab", accelerator: "CmdOrCtrl+W", action: () => active && close(active) },
            { item: "Separator" },
            { id: "import", text: "Import samples (.csv)…", action: importSamples },
            {
              text: "Export",
              items: [
                { id: "txt", text: "MTR report (.txt)…", action: onTab((t) => t.exportTxt()) },
                {
                  id: "csv",
                  text: "Samples (.csv)…",
                  action: onTabOrSummary(
                    (t) => t.exportCsv(),
                    (s) => s.exportCsv(),
                  ),
                },
                {
                  id: "png",
                  text: "Graph (.png)…",
                  action: onTabOrSummary(
                    (t) => t.exportPng(),
                    (s) => s.exportPng(),
                  ),
                },
                // Opens the print dialog; "Save as PDF" there writes the file.
                { id: "pdf", text: "Page as PDF…", accelerator: "CmdOrCtrl+P", action: onTab(() => window.print()) },
              ],
            },
            { item: "Separator" },
            { id: "settings", text: "Settings…", accelerator: "CmdOrCtrl+,", action: openSettings },
            { item: "Separator" },
            { item: "CloseWindow", text: "Exit" },
          ],
        },
        // Only macOS needs these: there, copy and paste in the page go through the menu.
        ...(MACOS
          ? [
              {
                text: "Edit",
                items: [
                  { item: "Undo" as const },
                  { item: "Redo" as const },
                  { item: "Separator" as const },
                  { item: "Cut" as const },
                  { item: "Copy" as const },
                  { item: "Paste" as const },
                  { item: "SelectAll" as const },
                ],
              },
            ]
          : []),
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
            {
              id: "pause",
              text: "Pause / resume",
              accelerator: "CmdOrCtrl+Shift+P",
              action: () => active && togglePause(active),
            },
            { id: "restart", text: "Restart and erase history…", action: () => active && restart(active) },
            { item: "Separator" },
            { id: "probe", text: "Probe settings…", action: onTab((t) => t.openProbe()) },
            { id: "alerts", text: "Alerts…", action: onTab((t) => t.openAlerts()) },
          ],
        },
        {
          text: "Tools",
          items: [
            ...(WINDOWS
              ? [
                  { id: "firewall", text: "Allow ICMP replies in Windows Firewall…", action: allowIcmp },
                  { id: "elevate", text: "Restart as Administrator…", enabled: !elevated, action: restartElevated },
                  { item: "Separator" as const },
                ]
              : canElevate
                ? [
                    { id: "elevate", text: "Allow raw sockets…", enabled: !elevated, action: restartElevated },
                    { item: "Separator" as const },
                  ]
                : []),
            {
              text: "AI agents (MCP server)",
              items: [
                {
                  id: "agents",
                  text: "Allow agents to connect",
                  checked: mcpOn,
                  action: () => setAgentAccess(!mcpOn, true),
                },
                {
                  id: "agents-address",
                  text: mcpUrl ? `Copy address (${mcpUrl})` : "Copy address",
                  enabled: !!mcpUrl,
                  action: copyAgentAddress,
                },
              ],
            },
          ],
        },
        {
          text: "Help",
          items: [
            { id: "check-update", text: "Check for updates…", action: () => checkUpdates(true) },
            { item: "Separator" },
            {
              id: "github",
              text: "TracePlotter on GitHub",
              action: () => invoke("open_project_page").catch((e) => showToast(String(e), true)),
            },
            { id: "version", text: `TracePlotter ${version}`, enabled: false },
          ],
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

  /**
   * Opens a samples export (.csv) as a paused tab that holds its rounds, to look at a
   * capture made earlier or elsewhere. The export does not say how it was probed: the tab
   * gets the host from the file's name and the packet type of new targets, for when it is
   * resumed.
   */
  async function importSamples() {
    try {
      const picked = await invoke<[string, string] | null>("import_file");
      if (!picked) return;
      const [name, text] = picked;
      const samples = parseSamplesCsv(text);
      const target = {
        host: hostFromFileName(name).toLowerCase(),
        protocol: settings.protocol,
        port: DEFAULT_PORT[settings.protocol],
      };
      const config = { ...newConfig(target, guessInterval(samples)), paused: true };
      const history = samples.map((sample) => JSON.stringify(sample)).join("\n") + "\n";
      await invoke("import_history", new TextEncoder().encode(history), { headers: { "x-session": config.sessionId } });
      const trace = await open(config);
      session = [...session, config];
      saveSession();
      // A history longer than the settings keep is cut to its newest part.
      const kept = trace.target.times.length;
      showToast(`Imported ${kept === samples.length ? kept : `the newest ${kept} of ${samples.length}`} rounds from ${name}.`);
    } catch (e) {
      showToast(`Not imported: ${e instanceof Error ? e.message : e}`, true);
    }
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
    try {
      if (current) {
        await togglePause(current);
      } else {
        const { config } = await open(newConfig(target, intervalMs));
        session = [...session, config];
        saveSession();
      }
    } catch (e) {
      showToast(String(e), true);
    } finally {
      busy = false;
    }
  }

  // Traces that are restarting with new probe settings, and whether their settings
  // changed again since that restart began.
  const reprobing = new Map<Trace, boolean>();

  /** Saves a tab's changed probe settings; probing restarts with them at the next round. */
  async function reprobe(trace: Trace) {
    saveSession();
    // A restart reads the settings when it begins, so a change during it needs another.
    if (reprobing.has(trace)) return void reprobing.set(trace, true);
    if (trace.paused) return;
    try {
      do {
        reprobing.set(trace, false);
        await setPaused(trace, true, bump);
        await setPaused(trace, false, bump);
      } while (reprobing.get(trace));
    } catch (e) {
      trace.error = `Could not restart: ${e}`;
    } finally {
      reprobing.delete(trace);
    }
    saveSession();
    rev++;
  }

  /** Applies a new interval to the open tab. */
  function changeInterval() {
    if (!current) return;
    current.config.intervalMs = intervalMs;
    reprobe(current);
  }

  function newTarget() {
    activeId = null;
    host = "";
    toDefaults();
    hostInput.focus();
  }

  /** Closes the tab and erases its history, once the user has said so: there is no undo. */
  function close(trace: Trace) {
    if (!confirm(`Close the tab of ${trace.config.host}?\n\nIts history is erased.`)) return;
    closeTrace(trace).catch((e) => showToast(`History of ${trace.config.host} not erased: ${e}`, true));
    const index = traces.indexOf(trace);
    traces = traces.filter((t) => t !== trace);
    if (activeId === trace.id) activeId = (traces[index] ?? traces[index - 1])?.id ?? null;
    session = session.filter((c) => c !== trace.config);
    saveSession();
  }

  /** Arrow keys, Home and End move between the tabs, as in a tab strip; Enter or Space opens one. */
  function tabKeys(event: KeyboardEvent) {
    const tabs = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>('[role="tab"]')];
    const at = tabs.indexOf(event.target as HTMLElement);
    const keys: Record<string, number> = { ArrowLeft: at - 1, ArrowRight: at + 1, Home: 0, End: tabs.length - 1 };
    if (at < 0 || !(event.key in keys)) return;
    event.preventDefault();
    tabs[(keys[event.key] + tabs.length) % tabs.length].focus();
  }

  onMount(async () => {
    version = await getVersion().catch(() => "");
    elevated = await invoke<boolean>("is_elevated").catch(() => true);
    canElevate = await invoke<boolean>("can_elevate").catch(() => true);
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
    buildMenu().catch((e) => showToast(`Menu failed: ${e}`, true));
    // Before any tab shows a date: the host's own date and time format.
    setLocale(await invoke<string | null>("host_locale").catch(() => null));
    // Before any history is read: how much of it the backend keeps.
    await invoke("set_history_days", { days: settings.historyDays }).catch(() => {});
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
          showToast(`Could not restore ${config.host}: ${e}`, true);
          return null;
        }),
      ),
    );
    // In saved order, ahead of any tab the user opened meanwhile.
    traces = [...restored.filter((trace) => trace !== null), ...traces];
    // The tab that was open last time, unless the user has already picked one.
    const last = localStorage.getItem(ACTIVE_KEY);
    activeId ??=
      last === "summary" && traces.length
        ? "summary"
        : (traces.find((trace) => trace.config.sessionId === last) ?? traces[0])?.id ?? null;
    tabsRestored = true;
    if (autoCheck) checkUpdates(false);
    // Agents are let in once the tabs they ask about are back.
    await listen<Parameters<typeof answerAgent>[0]>("mcp-request", (event) => answerAgent(event.payload));
    // `--mcp` turns it on for this run only.
    if (mcpOn || (await invoke<boolean>("mcp_flag").catch(() => false))) setAgentAccess(true, false);
  });
</script>

<header>
  {#if update}
    <div class="update" role="status">
      <span>TracePlotter {update.version} is available (you have {version}).</span>
      <!-- Releases carry an exe for Windows only; elsewhere the update is downloaded by hand. -->
      {#if WINDOWS}
        <button class="install" disabled={updating} onclick={installUpdate}>
          {updating ? "Downloading…" : "Update and restart"}
        </button>
      {:else}
        <button class="install" onclick={() => invoke("open_project_page").catch((e) => showToast(String(e), true))}>
          Open the project page
        </button>
      {/if}
      <button disabled={updating} onclick={() => (update = null)}>Later</button>
      {#if update.notes}
        <details>
          <summary>What is new</summary>
          <pre>{update.notes}</pre>
        </details>
      {/if}
    </div>
  {/if}
  {#if wantsAdmin}
    <div class="update" role="status">
      {#if WINDOWS}
        <span>
          ICMP (raw socket), UDP and TCP probes need Administrator rights, which TracePlotter is running without.
        </span>
        <button class="install" onclick={restartElevated}>Restart as Administrator</button>
      {:else if canElevate}
        <span>
          ICMP (raw socket), UDP and TCP probes need raw sockets, which TracePlotter is not allowed to open yet.
        </span>
        <button class="install" onclick={restartElevated}>Allow raw sockets and restart</button>
      {:else}
        <span>
          ICMP (raw socket), UDP and TCP probes need raw sockets, which an AppImage cannot open. Use ICMP (ping
          socket), or install TracePlotter from a package.
        </span>
      {/if}
    </div>
  {/if}
  <div class="tabs" role="tablist" tabindex="-1" onkeydown={tabKeys}>
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
          <small>{protocolLabel({ protocol, port })}</small>
        </button>
        <button class="close" aria-label="Close {host}" onclick={() => close(trace)}>×</button>
      </div>
    {/each}
    <button class="add" aria-label="New target" title="New target (Ctrl+T)" onclick={newTarget}>+</button>
    <div class="tab settings" class:active={activeId === "settings"}>
      <button class="all" role="tab" aria-selected={activeId === "settings"} title="Settings (Ctrl+,)" onclick={openSettings}>
        ⚙&#xFE0E; Settings
      </button>
    </div>
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
    <Picker
      value={protocol}
      options={(Object.keys(PROTOCOLS) as Protocol[]).filter((value) => SYSTEM_ICMP || value !== "winicmp")}
      label={(value) => PROTOCOLS[value] + (!elevated && value !== "winicmp" ? ` (needs ${RAW_SOCKET_RIGHTS})` : "")}
      onpick={(value) => {
        protocol = value;
        port = DEFAULT_PORT[value];
      }}
      ariaLabel="Protocol"
    />
    {#if usesPort(protocol)}
      <input class="port" type="number" min="1" max="65535" bind:value={port} aria-label="Port" required />
    {/if}
    <button
      class="go"
      class:running
      disabled={busy}
      aria-label={running ? "Pause trace" : current ? "Resume trace" : "Start trace"}
      data-tip={running ? "Pause trace" : current ? "Resume trace" : "Start trace"}
    >
      {running ? "⏸︎" : "▶︎"}
    </button>
    {#if current}
      <button
        class="restart"
        type="button"
        aria-label="Restart trace and erase its history"
        data-tip="Restart trace and erase its history"
        onclick={() => restart(current)}>↻</button
      >
    {/if}
    <label>
      Interval
      <Picker
        value={intervalMs}
        options={INTERVALS}
        label={intervalLabel}
        onpick={(ms) => {
          intervalMs = ms;
          changeInterval();
        }}
        parse={parseInterval}
        short={(ms) => `${ms / 1000}s`}
        title="Time between rounds. Pick one or type, from 0.1s to 60s, for example 1.5s or 750ms."
        width="104px"
      />
    </label>
    <label>
      Time span
      <Picker
        value={focusSeconds}
        options={SPANS}
        label={spanLabel}
        onpick={changeSpan}
        parse={parseSpan}
        short={spanShort}
        title="How much of the newest history the table and graphs cover. Pick one or type, for example 30s, 5m, 2h or all."
        width="104px"
      />
    </label>
    <!-- The colours of the hop table's latency bars; not shown where there are none. -->
    {#if active && barShown}
      <div class="legend" role="img" aria-label="Latency colours: green under {limits.warn} ms, yellow under {limits.bad} ms, red above">
        <span style:background={BAND_FILLS[0]}></span>
        <span class="warn" style:background={BAND_FILLS[1]}>{limits.warn}ms</span>
        <span class="bad" style:background={BAND_FILLS[2]}>{limits.bad}ms</span>
      </div>
    {/if}
  </form>
</header>

<main>
  {#if active}
    {#key active.id}
      <TraceTab
        bind:this={tab}
        bind:barShown
        trace={active}
        {rev}
        {hidden}
        {focusSeconds}
        onsave={saveSession}
        onprobe={() => reprobe(active)}
        oncolumn={toggleColumn}
        oncolumnsreset={resetHidden}
        onspan={changeSpan}
      />
    {/key}
  {:else if activeId === "settings"}
    <Settings
      {version}
      {hidden}
      {autoCheck}
      {mcpOn}
      {mcpUrl}
      {elevated}
      {canElevate}
      onelevate={restartElevated}
      oncolumn={toggleColumn}
      onautocheck={setAutoCheck}
      onagents={(on) => setAgentAccess(on, true)}
      oncopy={copyAgentAddress}
      oncheck={() => checkUpdates(true)}
      onfirewall={allowIcmp}
      onchange={settingChanged}
    />
  {:else if activeId === "summary" && traces.length}
    <Summary
      bind:this={summary}
      {traces}
      {rev}
      {focusSeconds}
      onopen={(trace) => (activeId = trace.id)}
      ontoggle={togglePause}
      onspan={changeSpan}
    />
  {:else}
    <p class="empty">Type a host above and press Enter or Start (▶&#xFE0E;). Each target opens in its own tab.</p>
  {/if}
</main>

<ContextMenu />
<Toast />

<style>
  :global(:root) {
    --bg: #0f1319;
    --panel: #161b24;
    --line: #232a36;
    /* The edge of a field or button: a little clearer than --line, which is made for faint
       grid lines, without drawing a strong frame around every control. */
    --edge: #343d4b;
    --text: #e6eaf0;
    --muted: #9aa4b2;
    --accent: #2f8fe6;
    --good: #22c55e;
    --warn: #eab308;
    --bad: #ef4444;
    /* "only": WebKitGTK draws its form controls (the arrows of number boxes) dark for a
       plain "dark" only when the desktop asks for dark too. */
    color-scheme: only dark;
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
  /* Keeps a number box's digits off its arrows, and the arrows against its right edge
     instead of inside its padding. */
  :global(input[type="number"]::-webkit-inner-spin-button) {
    margin-left: 4px;
    margin-right: -4px;
  }
  :global(:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  /* The app's own tooltip, in the style of the graph and bar readouts: put the text in a
     `data-tip` attribute. It shows below the element on hover and on keyboard focus. */
  :global([data-tip]) {
    position: relative;
  }
  :global([data-tip]:hover::after),
  :global([data-tip]:focus-visible::after) {
    content: attr(data-tip);
    position: absolute;
    top: calc(100% + 8px);
    left: 50%;
    translate: -50%;
    z-index: 20;
    padding: 4px 8px;
    border: 1px solid var(--line);
    border-radius: 4px;
    background: rgba(15, 19, 25, 0.95);
    color: var(--text);
    font: 12px/1.4 "Segoe UI Variable", "Segoe UI", system-ui, sans-serif;
    white-space: nowrap;
    pointer-events: none;
  }
  /* Windows high contrast replaces backgrounds with its own colours, which would erase the
     colours that carry meaning here: status dots, latency badges, bars and legend. */
  :global(.dot),
  :global(.badge),
  :global(.track),
  :global(.legend),
  :global(.swatch),
  :global(.go) {
    forced-color-adjust: none;
  }
  header {
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }

  /* The offer of a newer version or of a restart as Administrator, across the top. */
  .update {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 12px;
    align-items: center;
    padding: 6px 12px;
    background: #12314f;
    color: #dbeafe;
  }
  .update button {
    font: inherit;
    color: inherit;
    background: transparent;
    border: 1px solid #5b89b8;
    border-radius: 6px;
    padding: 2px 10px;
    cursor: pointer;
  }
  /* Darker than the accent: white on the accent itself is too faint to read. */
  .update .install {
    background: #1f6fc2;
    border-color: #1f6fc2;
    color: #fff;
    font-weight: 600;
  }
  .update button:disabled {
    opacity: 0.6;
  }
  .update details {
    flex-basis: 100%;
  }
  .update summary {
    cursor: pointer;
  }
  .update pre {
    margin: 6px 0 4px;
    max-height: 30vh;
    overflow: auto;
    font: 12px/1.4 "Segoe UI", system-ui, sans-serif;
    white-space: pre-wrap;
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
  /* Dark with a stripe in the accent colour: white text and the status dot stay easy to
     read, which they were not on a tab filled with the accent colour. */
  .tab.active {
    background: color-mix(in srgb, var(--accent) 24%, var(--bg));
    box-shadow: inset 0 3px 0 var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .tab.active small {
    font-weight: 400;
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
  /* At the far end of the strip, apart from the targets. */
  .tab.settings {
    margin-left: auto;
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
  /* Each state has its own shape as well as its colour, which red-green colour blindness
     cannot tell apart: a ring while paused, a diamond while alerting. */
  .dot.paused {
    width: 7px;
    height: 7px;
    background: none;
    border: 2px solid var(--warn);
  }
  .dot.alerting {
    border-radius: 1px;
    rotate: 45deg;
    background: var(--bad);
  }
  .tab .close {
    padding: 4px 10px 6px 6px;
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
    border: 1px solid var(--edge);
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
    border: 1px solid var(--edge);
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
  /* At the right end of the toolbar, also when it wraps onto a line of its own. */
  .legend {
    display: flex;
    margin-left: auto;
    width: 165px;
    height: 20px;
    border-radius: 4px;
    overflow: hidden;
    font-size: 12px;
    line-height: 20px;
    font-variant-numeric: tabular-nums;
  }
  .legend span {
    flex: 1;
    padding-left: 5px;
  }
  /* On the tints of the graphs' latency bands, the colours of slow values in the table.
     The fainter tints of the hops' bars look dimmer here, on the toolbar's lighter panel. */
  /* The readout next to the cursor on a graph (HopGraph, CompareGraph), placed in window
     coordinates: to the lower right of the cursor, or flipped where the window ends. */
  :global(.tip) {
    position: fixed;
    z-index: 10;
    --dx: 12px;
    --dy: 12px;
    translate: var(--dx) var(--dy);
    padding: 4px 8px;
    border: 1px solid var(--line);
    border-radius: 4px;
    background: rgba(15, 19, 25, 0.92);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    pointer-events: none;
  }
  :global(.tip.flip) {
    --dx: calc(-100% - 12px);
  }
  :global(.tip.up) {
    --dy: calc(-100% - 12px);
  }
  :global(.tip.pinned) {
    --dy: 0px;
  }
  :global(.tip > div:first-child) {
    color: var(--muted);
  }
  .legend .warn {
    color: #fbbf24;
  }
  .legend .bad {
    color: #fca5a5;
  }
  .error {
    color: #f87171;
  }

  main {
    flex: 1;
    min-height: 0;
  }
  /* Printing (Edit > Export page as PDF) uses a light theme: dark text on white paper.
     The page's own dark background is not printed, so the on-screen light text would
     vanish. Row colours and the graphs' dark panels are kept by print-color-adjust. */
  @media print {
    :global(:root) {
      --bg: #fff;
      --panel: #eef2f7;
      --line: #cbd5e1;
      --text: #0f172a;
      --muted: #475569;
      color-scheme: light;
    }
    :global(html),
    :global(body),
    :global(body > div),
    main {
      display: block !important;
      height: auto;
      overflow: visible;
    }
    /* The page margin is 0 (see app.html), which drops the browser's header and footer
       lines; this padding is the margin instead. */
    :global(body) {
      padding: 10mm;
      background: #fff;
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

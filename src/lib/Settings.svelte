<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { showToast } from "./Toast.svelte";
  import {
    INTERVALS,
    LIMITS,
    RAW_SOCKET_RIGHTS,
    SYSTEM_ICMP,
    WINDOWS,
    changeSetting,
    intervalLabel,
    settings,
    type Settings,
  } from "./settings";
  import { COLUMNS } from "./TraceTab.svelte";
  import { PROTOCOLS } from "./trace";

  type Props = {
    version: string;
    /** Keys of the hop table columns that are switched off. */
    hidden: string[];
    /** Whether the app looks for a newer release when it starts. */
    autoCheck: boolean;
    /** Whether the MCP server runs, and the address agents connect to while it does. */
    mcpOn: boolean;
    mcpUrl: string | null;
    /** Whether the app may open raw sockets, and the call to restart with that right. */
    elevated: boolean;
    /** Whether the app can be given that right; an AppImage cannot. */
    canElevate: boolean;
    onelevate: () => void;
    /** Called to show or hide the column with this key. */
    oncolumn: (key: string) => void;
    onautocheck: (on: boolean) => void;
    /** Starts or stops the MCP server; done when `mcpOn` tells whether it worked. */
    onagents: (on: boolean) => Promise<void>;
    oncopy: () => void;
    /** Looks for a newer release now. */
    oncheck: () => void;
    onfirewall: () => void;
    /** Called after the setting with this key in `settings` changed. */
    onchange: (key: keyof Settings) => void;
  };
  let {
    version,
    hidden,
    autoCheck,
    mcpOn,
    mcpUrl,
    elevated,
    canElevate,
    onelevate,
    oncolumn,
    onautocheck,
    onagents,
    oncopy,
    oncheck,
    onfirewall,
    onchange,
  }: Props = $props();

  // `settings` is a plain object; this copy of it is what redraws the fields.
  let shown = $state({ ...settings });
  type BackgroundStatus = { available: boolean; closeToTray: boolean; minimizeToTray: boolean; startup: boolean; startupAvailable: boolean };
  let background = $state<BackgroundStatus>();
  let backgroundBusy = $state(false);
  let backgroundError = $state("");
  let nativeAvailable = $state(false);
  let interfaceReady = $state(false);
  let interfaceBusy = $state(false);
  let interfaceError = $state("");
  onMount(() => {
    invoke<BackgroundStatus>("background_status").then((status) => background = status)
      .catch((e) => backgroundError = String(e));
    invoke<{ nativeAvailable: boolean }>("gui_status").then((status) => nativeAvailable = status.nativeAvailable)
      .catch((e) => interfaceError = String(e)).finally(() => interfaceReady = true);
  });

  async function switchInterface() {
    interfaceBusy = true;
    interfaceError = "";
    try {
      const values = Object.fromEntries(Object.keys(localStorage)
        .filter((key) => key.startsWith("traceplotter."))
        .map((key) => [key, localStorage.getItem(key)]));
      await invoke("switch_gui", { values });
    } catch (e) {
      interfaceError = `The interface was not switched: ${e}`;
      interfaceBusy = false;
      showToast(interfaceError, true);
    }
  }

  async function setBackground(key: "closeToTray" | "minimizeToTray" | "startup", field: HTMLInputElement) {
    if (!background) return;
    backgroundBusy = true;
    try {
      if (key === "startup") await invoke("set_startup", { enabled: field.checked });
      else await invoke("set_background", { options: { closeToTray: background.closeToTray, minimizeToTray: background.minimizeToTray, [key]: field.checked } });
      background = await invoke<BackgroundStatus>("background_status");
    } catch (e) { showToast(`Background setting was not changed: ${e}`, true); }
    finally { field.checked = background[key]; backgroundBusy = false; }
  }

  /** Takes over a field's value and writes back what it became within its limits. */
  function set(key: keyof Settings, field: HTMLInputElement | HTMLSelectElement) {
    changeSetting(key, field.value);
    shown = { ...settings };
    field.value = String(settings[key]);
    onchange(key);
  }

  /** Attributes of the number field of a setting. */
  const number = (key: keyof typeof LIMITS) => ({
    type: "number",
    min: LIMITS[key][0],
    max: LIMITS[key][1],
    value: shown[key],
    onchange: (event: Event) => set(key, event.currentTarget as HTMLInputElement),
  });
</script>

<!-- One card per subject. A row is a setting: its name and a note on the left, its field
     on the right. -->
<div class="settings">
<div class="cards">
  <section>
    <h2>Interface</h2>
    <p>Try the native GUI preview. Switching restarts the app and keeps your tabs, settings and history.</p>
    {#if interfaceReady && !nativeAvailable && !interfaceError}<p>This build includes only the classic interface.</p>{/if}
    {#if interfaceError}<p role="alert">{interfaceError}</p>{/if}
    <div class="row"><span>Classic</span>
      <button disabled={!nativeAvailable || interfaceBusy} onclick={switchInterface}>{interfaceBusy ? "Restarting…" : "Switch to native GUI and restart"}</button>
    </div>
    <p>You can switch back in the native interface's Settings.</p>
  </section>
  <section>
    <h2>Background monitoring</h2>
    <p>Monitoring and alerts continue while the window is hidden. Reopen it from the tray icon. File > Exit or the tray's Exit and stop monitoring ends the app.</p>
    {#if backgroundError}<p role="alert">{backgroundError}</p>{/if}
    {#if background && !background.available}<p>The system tray is unavailable on this desktop. Closing the window exits the app.</p>{/if}
    <label class="row"><span>Close to tray <small>The window's × button keeps monitoring running</small></span>
      <input type="checkbox" disabled={!background?.available || backgroundBusy} checked={background?.closeToTray ?? false} onchange={(e) => setBackground("closeToTray", e.currentTarget)} /></label>
    <label class="row"><span>Minimize to tray</span>
      <input type="checkbox" disabled={!background?.available || backgroundBusy} checked={background?.minimizeToTray ?? false} onchange={(e) => setBackground("minimizeToTray", e.currentTarget)} /></label>
    {#if WINDOWS}
      <label class="row"><span>Start when I sign in to Windows <small>Starts in the tray and restores running targets. Turn this on again if you move the portable exe. Unavailable in demo mode.</small></span>
        <input type="checkbox" disabled={!background?.startupAvailable || backgroundBusy} checked={background?.startup ?? false} onchange={(e) => setBackground("startup", e.currentTarget)} /></label>
    {/if}
  </section>
  <section>
    <h2>New targets</h2>
    <p>What a new tab starts with. Open tabs keep their own: change those in the toolbar and under Trace.</p>
    <label class="row">
      <span>Packet type</span>
      <select value={shown.protocol} onchange={(event) => set("protocol", event.currentTarget)}>
        {#each Object.entries(PROTOCOLS) as [value, name]}
          {#if SYSTEM_ICMP || value !== "winicmp"}
            <option {value}>{name}{!elevated && value !== "winicmp" ? ` (needs ${RAW_SOCKET_RIGHTS})` : ""}</option>
          {/if}
        {/each}
      </select>
    </label>
    <label class="row">
      <span>Interval</span>
      <select value={shown.intervalMs} onchange={(event) => set("intervalMs", event.currentTarget)}>
        {#each INTERVALS as ms}<option value={ms}>{intervalLabel(ms)}</option>{/each}
      </select>
    </label>
    <label class="row">
      <span>Wait for replies <small>At least the interval</small></span>
      <span class="field"><input {...number("timeoutMs")} step="100" /> ms</span>
    </label>
    <label class="row">
      <span>Packet size <small>The whole IP packet; TCP ignores it</small></span>
      <span class="field"><input {...number("packetSize")} /> bytes</span>
    </label>

    <h3>Alerts</h3>
    <p>Raised when the round trip's latest probes go over a limit. 0 switches a limit off.</p>
    <label class="row">
      <span>Average latency over</span>
      <span class="field"><input {...number("alertLatencyMs")} /> ms</span>
    </label>
    <label class="row">
      <span>Packet loss of at least</span>
      <span class="field"><input {...number("alertLossPct")} /> %</span>
    </label>
    <label class="row">
      <span>Over the last <small>An alert waits until it has this much</small></span>
      <span class="field"><input {...number("alertSeconds")} /> s</span>
    </label>
  </section>

  <section>
    <h2>Display</h2>
    <h3>Latency colours</h3>
    <p>Where the table, the bars and the graphs turn from green to yellow and from yellow to red.</p>
    <label class="row">
      <span><i class="swatch warn"></i>Yellow from</span>
      <span class="field"><input {...number("warnMs")} /> ms</span>
    </label>
    <label class="row">
      <span><i class="swatch bad"></i>Red from</span>
      <span class="field"><input {...number("badMs")} /> ms</span>
    </label>

    <h3>Compare graph</h3>
    <label class="row">
      <span>Loss bars in each target's colour <small>Otherwise red, as on a hop's graph</small></span>
      <input
        type="checkbox"
        checked={shown.lossByTarget}
        onchange={(event) => {
          changeSetting("lossByTarget", event.currentTarget.checked);
          shown = { ...settings };
          onchange("lossByTarget");
        }}
      />
    </label>

    <h3>Hop table columns</h3>
    <div class="columns">
      {#each COLUMNS as column (column.key)}
        <label>
          <input type="checkbox" checked={!hidden.includes(column.key)} onchange={() => oncolumn(column.key)} />
          {column.label}
        </label>
      {/each}
    </div>
  </section>

  <section>
    <h2>Notifications</h2>
    <label class="row">
      <span>
        Desktop notifications
        <small>For alerts and route changes. Both are listed under Events of their tab either way</small>
      </span>
      <input
        type="checkbox"
        checked={shown.notifications}
        onchange={(event) => {
          changeSetting("notifications", event.currentTarget.checked);
          shown = { ...settings };
          onchange("notifications");
        }}
      />
    </label>
  </section>

  <section>
    <h2>History</h2>
    <label class="row">
      <span>Keep per target <small>Every round, on disk; older days are deleted</small></span>
      <span class="field"><input {...number("historyDays")} /> days</span>
    </label>
    <p>
      A day of history takes about 35 MB per target at an interval of one second. The newest day is held in
      the window; older periods are read from disk when the time span reaches them.
    </p>
  </section>

  <section>
    <h2>AI agents</h2>
    <p>
      Lets AI agents on this computer read the traces, start new ones and pause them, through an MCP server on the
      loopback address. Web pages and other machines cannot reach it.
    </p>
    <label class="row">
      <span>Allow AI agents to connect</span>
      <!-- Starting can fail (the port is taken), so the tick follows what happened. -->
      <input
        type="checkbox"
        checked={mcpOn}
        onchange={async (event) => {
          const box = event.currentTarget;
          await onagents(box.checked);
          box.checked = mcpOn;
        }}
      />
    </label>
    <label class="row">
      <span>Port</span>
      <span class="field"><input {...number("mcpPort")} /></span>
    </label>
    <div class="row">
      <span>Address</span>
      {#if mcpUrl}
        <span><code>{mcpUrl}</code> <button onclick={oncopy}>Copy</button></span>
      {:else}
        <span class="muted">Not running</span>
      {/if}
    </div>
  </section>

  {#if WINDOWS}
    <section>
      <h2>Administrator rights</h2>
      <p>
        ICMP (raw socket), UDP and TCP probes use raw sockets, which Windows allows only with Administrator rights.
        Adding firewall rules needs them too. ICMP (Windows API) does not.
      </p>
      <div class="row">
        <span>TracePlotter is running {elevated ? "with" : "without"} them</span>
        {#if !elevated}<button onclick={onelevate}>Restart as Administrator</button>{/if}
      </div>
      <div class="row">
        <span>
          Windows Firewall
          <small>ICMP (raw socket) traces lose hops, the final one included, until ICMP replies are let in</small>
        </span>
        <button onclick={onfirewall}>Allow ICMP replies…</button>
      </div>
    </section>
  {:else}
    <section>
      <h2>Raw sockets</h2>
      <p>
        ICMP (raw socket), UDP and TCP probes use raw sockets, which Linux allows with the capability CAP_NET_RAW.
        ICMP (ping socket) does not. The packages give TracePlotter the capability when they install it; otherwise
        allow raw sockets here once, which asks for an administrator's password and restarts the app. An AppImage
        cannot be allowed them; install TracePlotter from a package for those probes.
      </p>
      <div class="row">
        <span>TracePlotter is {elevated ? "allowed" : "not allowed"} to open them</span>
        {#if !elevated && canElevate}<button onclick={onelevate}>Allow raw sockets…</button>{/if}
      </div>
    </section>
  {/if}

  <section>
    <h2>Updates</h2>
    <label class="row">
      <span>Check for updates at startup</span>
      <input type="checkbox" checked={autoCheck} onchange={(event) => onautocheck(event.currentTarget.checked)} />
    </label>
    <div class="row">
      <span>TracePlotter {version}</span>
      <button onclick={oncheck}>Check for updates…</button>
    </div>
  </section>
</div>
</div>

<style>
  .settings {
    height: 100%;
    box-sizing: border-box;
    overflow: auto;
    padding: 16px;
  }
  /* Cards side by side as the window allows, each as tall as its own content. Not on the
     scrolling element: with a set height, columns would run off to the side. */
  .cards {
    columns: 380px;
    column-gap: 16px;
  }
  section {
    break-inside: avoid;
    margin-bottom: 16px;
    padding: 12px 16px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--panel);
  }
  h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  h3 {
    margin: 16px 0 6px;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--muted);
  }
  h2 + h3 {
    margin-top: 0;
  }
  p {
    margin: 0 0 8px;
    color: var(--muted);
  }
  .row {
    display: flex;
    gap: 16px;
    align-items: center;
    justify-content: space-between;
    min-height: 30px;
    padding: 4px 0;
    border-top: 1px solid var(--line);
  }
  .row > :last-child {
    flex: none;
  }
  /* A number field and its unit, as wide as the longest unit needs, so that the fields
     line up whatever unit follows them. */
  .field {
    width: 120px;
  }
  small {
    display: block;
    font-size: 12px;
    color: var(--muted);
  }
  .muted {
    color: var(--muted);
  }
  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    margin-right: 6px;
    border-radius: 2px;
  }
  .swatch.warn {
    background: var(--warn);
  }
  .swatch.bad {
    background: var(--bad);
  }
  .columns {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    gap: 4px 12px;
  }
  input[type="number"] {
    width: 72px;
    padding: 2px 6px;
    font: inherit;
    text-align: right;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--edge);
    border-radius: 4px;
  }
  select {
    padding: 2px 6px;
  }
  code {
    user-select: text;
  }
  button {
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--edge);
    border-radius: 6px;
    padding: 2px 10px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>

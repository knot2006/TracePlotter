<script lang="ts" module>
  import type { HopStats } from "./stats";
  import type { Hop } from "./trace";

  /** A table row: a hop, or the round trip to the target itself. */
  type Row = { hop: Hop; s: HopStats; mos: number | null; roundTrip: boolean };
  type Column = {
    key: string;
    label: string;
    title?: string;
    /** Left-aligned text instead of a right-aligned number. */
    text?: boolean;
    value: (row: Row) => string;
    cls?: (row: Row) => string;
  };

  const ms = (v: number | null) => (v === null ? "–" : v.toFixed(1));

  /** Hop table columns after the hop number, in order. `bar` is drawn, not text. */
  export const COLUMNS: Column[] = [
    { key: "ip", label: "IP", text: true, value: (r) => (r.roundTrip ? "Round trip" : (r.hop.addr ?? "*")) },
    // A hop without a DNS name shows its address again, so the column is never blank.
    {
      key: "name",
      label: "Name",
      text: true,
      value: (r) => (r.roundTrip ? "" : (r.hop.name ?? r.hop.addr ?? "")),
      cls: () => "name",
    },
    {
      key: "loss",
      label: "Loss",
      value: (r) => `${r.s.lossPct.toFixed(1)}%`,
      cls: (r) => (r.s.lossPct >= 10 ? "bad" : r.s.lossPct > 0 ? "warn" : ""),
    },
    { key: "sent", label: "Sent", value: (r) => String(r.s.sent) },
    { key: "recv", label: "Recv", title: "Probes that got a reply", value: (r) => String(r.s.received) },
    { key: "last", label: "Last", value: (r) => ms(r.s.last) },
    { key: "avg", label: "Avg", value: (r) => ms(r.s.avg) },
    { key: "min", label: "Min", value: (r) => ms(r.s.min) },
    { key: "max", label: "Max", value: (r) => ms(r.s.max) },
    { key: "jitter", label: "Jitter", value: (r) => ms(r.s.jitter) },
    {
      key: "mos",
      label: "MOS",
      title: "Estimated call quality, 1 (unusable) to 4.4 (best)",
      value: (r) => (r.mos === null ? "–" : r.mos.toFixed(2)),
      cls: (r) => (r.mos === null || r.mos >= 4 ? "" : r.mos < 3.1 ? "bad" : "warn"),
    },
    { key: "bar", label: "Latency bar", text: true, value: () => "" },
  ];
  export const DEFAULT_HIDDEN = ["mos"];
</script>

<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { Menu } from "@tauri-apps/api/menu";
  import { groupEvents } from "./events";
  import HopGraph from "./HopGraph.svelte";
  import { ALERT_WINDOW, BAD_MS, WARN_MS, lowerBound, mos, stats, textTable } from "./stats";
  import { dateTime, timeOfDay } from "./time";
  import { whoisFacts, whoisRecord } from "./whois";
  import { DEFAULT_PACKET_SIZE, DEFAULT_TIMEOUT_MS, exportFile, protocolLabel, type Trace } from "./trace";

  type Props = {
    trace: Trace;
    /** Changes whenever `trace` (a plain object, mutated in place) gains data. */
    rev: number;
    /** Keys of `COLUMNS` not to show. */
    hidden: string[];
    /** Length of the newest stretch the table and graphs cover, 0 for everything. */
    focusSeconds: number;
    /** Called after the alert thresholds in `trace.config` change. */
    onsave: () => void;
    /** Called after the probe settings in `trace.config` change, to probe with them. */
    onprobe: () => void;
    /** Called to show or hide the column with this key. */
    oncolumn: (key: string) => void;
  };
  let { trace, rev, hidden, focusSeconds, onsave, onprobe, oncolumn }: Props = $props();

  // The order of the columns after Hop, as keys. Changed by dragging a header or from its
  // right-click menu; shared by all tabs and remembered between launches. Columns the
  // saved order does not know (added by a newer version) follow at the end.
  const COLUMN_ORDER_KEY = "traceplotter.columnOrder";
  let order = $state<string[]>(loadOrder());

  function loadOrder(): string[] {
    try {
      const stored = JSON.parse(localStorage.getItem(COLUMN_ORDER_KEY) ?? "[]");
      return Array.isArray(stored) ? stored.filter((key) => typeof key === "string") : [];
    } catch {
      return [];
    }
  }

  const ordered = $derived([
    ...order.flatMap((key) => COLUMNS.find((column) => column.key === key) ?? []),
    ...COLUMNS.filter((column) => !order.includes(column.key)),
  ]);
  // The columns on screen, in order. The text export follows these too.
  const columns = $derived(ordered.filter((column) => !hidden.includes(column.key)));

  /** Puts a column in front of another one, or at the end with `before` null. */
  function moveColumn(key: string, before: string | null) {
    const keys = ordered.map((column) => column.key).filter((k) => k !== key);
    keys.splice(before === null ? keys.length : keys.indexOf(before), 0, key);
    order = keys;
    localStorage.setItem(COLUMN_ORDER_KEY, JSON.stringify(keys));
  }

  /** Moves a column one place left (-1) or right (1) among the columns on screen. */
  function shiftColumn(key: string, by: -1 | 1) {
    const at = columns.findIndex((column) => column.key === key);
    const target = columns[at + by];
    if (!target) return;
    moveColumn(key, by < 0 ? target.key : (columns[at + 2]?.key ?? null));
  }

  // Dragging a header onto another one moves it there: in front of it when dragged to the
  // left, behind it when dragged to the right.
  let dragged = $state<string | null>(null);
  let dropTarget = $state<string | null>(null);
  // Set while a header's edge is being dragged to resize, which must not start a move.
  let resizing = false;

  function dropColumn(onto: string) {
    if (dragged && dragged !== onto) {
      const from = columns.findIndex((column) => column.key === dragged);
      const to = columns.findIndex((column) => column.key === onto);
      moveColumn(dragged, from < to ? (columns[to + 1]?.key ?? null) : onto);
    }
    dragged = dropTarget = null;
  }

  const BACKGROUND = "#0f1319";
  /** Key of the round trip in `graphed`; hops use their number. */
  const ROUND_TRIP = 0;

  // What has a graph: hop numbers, and ROUND_TRIP for the target itself.
  let graphed = $state<number[]>([ROUND_TRIP]);
  let exportStatus = $state("");
  // End of the focus period in Unix seconds while scrolled back in time; null follows the newest data.
  let pinnedEnd = $state<number | null>(null);
  let graphs = $state<(HopGraph | undefined)[]>([]);

  // Cutting the hops down and working out their numbers and graphs costs time in step
  // with the samples in view. So a long time span is refreshed every few rounds, not
  // every round: `viewRev` changes once per `every` changes of `rev`, at most every tenth.
  const viewRev = $derived.by(() => {
    rev;
    const samples = trace.target.times.length;
    const inView = focusSeconds ? Math.min(samples, (focusSeconds * 1000) / trace.config.intervalMs) : samples;
    const every = Math.max(1, Math.min(10, Math.ceil(inView / 3600)));
    return Math.floor(rev / every);
  });

  // The period shown and the hops cut down to it; the table, graphs and text report use these.
  const view = $derived.by(() => {
    viewRev;
    // Only the hops of the current route. After the route got shorter, the hops beyond its
    // end get no more probes; showing them would freeze the table.
    const route = trace.hops.slice(0, trace.pathEnd || trace.hops.length);
    // The target series has an entry for every round, so it spans the whole history.
    const { times } = trace.target;
    const latest = times.at(-1) ?? 0;
    const earliest = times[0] ?? 0;
    if (!focusSeconds) return { hops: route, target: trace.target, from: earliest, to: latest, earliest, latest };
    const to = Math.min(latest, Math.max(pinnedEnd ?? latest, earliest + focusSeconds));
    const from = to - focusSeconds;
    const cut = (hop: Hop) => {
      const start = lowerBound(hop.times, from);
      const end = to >= latest ? hop.times.length : lowerBound(hop.times, to + 0.001);
      return start || end < hop.times.length
        ? { ...hop, times: hop.times.slice(start, end), rtts: hop.rtts.slice(start, end) }
        : hop;
    };
    return { hops: route.map(cut), target: cut(trace.target), from, to, earliest, latest };
  });
  const focused = $derived(view.hops);
  // One row per hop, then the round trip to the target itself.
  const rows = $derived(
    (view.target.times.length ? [...focused, view.target] : focused).map((hop) => {
      const s = stats(hop.rtts);
      return { hop, s, mos: mos(s), roundTrip: hop === view.target };
    }),
  );
  // The latency bars share one scale. Unzoomed it ends at the highest maximum, so every
  // bar fits. Turning the wheel over the bars zooms in on the low end; bars that then
  // reach past the end are cut there and marked.
  let barZoom = $state(1);
  const scale = $derived(Math.max(1, ...rows.map((r) => r.s.max ?? 0)) / barZoom);

  // The row whose latency bar is under the pointer, and where. The tooltip is drawn by the
  // page, not by a `title`: its numbers change every round, and the browser hides and
  // reopens a title tooltip whenever its text changes.
  let barTip = $state<{ x: number; y: number; key: number } | null>(null);
  const rowKey = (row: { roundTrip: boolean; hop: Hop }) => (row.roundTrip ? ROUND_TRIP : row.hop.ttl);
  const tipRow = $derived(barTip && rows.find((row) => rowKey(row) === barTip!.key));

  function zoomBars(event: WheelEvent) {
    event.preventDefault();
    barZoom = Math.max(1, Math.min(64, barZoom * (event.deltaY < 0 ? 1.25 : 0.8)));
  }
  // Green, yellow and red latency zones behind the bars, on the same scale as the bars.
  const bands = $derived.by(() => {
    const warn = Math.min(100, (WARN_MS / scale) * 100);
    const bad = Math.min(100, (BAD_MS / scale) * 100);
    return `linear-gradient(to right, var(--good-bg) 0 ${warn}%, var(--warn-bg) ${warn}% ${bad}%, var(--bad-bg) ${bad}% 100%)`;
  });

  // The hops with a graph, in route order with the round trip last. The round trip is the
  // default: its series does not break when the route changes length, which a hop
  // number's series does. Hops that left the route drop out.
  const graphHops = $derived.by(() => {
    const hops = graphed
      .filter((key) => key !== ROUND_TRIP)
      .sort((a, b) => a - b)
      .flatMap((ttl) => (focused[ttl - 1] ? [focused[ttl - 1]] : []));
    return graphed.includes(ROUND_TRIP) || !hops.length ? [...hops, view.target] : hops;
  });

  function toggleGraph(key: number) {
    if (!graphed.includes(key)) graphed = [...graphed, key];
    else if (graphed.length > 1) graphed = graphed.filter((k) => k !== key);
  }

  function headingOf(hop: Hop) {
    const { name, addr, ttl } = hop;
    const who = `${name ?? addr ?? "no reply"}${name && addr ? ` (${addr})` : ""}`;
    if (hop !== view.target) return `${who} hop ${ttl}`;
    return `${who} round trip${ttl ? `, hop ${ttl}` : ""}`;
  }

  /**
   * Moves the focus period by a fraction of its length; negative is back in time. Reaching
   * the newest data follows it live again. With focus "All" there is nothing to move.
   */
  function pan(fraction: number) {
    if (!focusSeconds) return;
    const end = Math.max(view.earliest + focusSeconds, view.to + focusSeconds * fraction);
    pinnedEnd = end >= view.latest - 1 ? null : end;
  }

  const grade = (avg: number | null) =>
    avg === null ? "none" : avg >= BAD_MS ? "bad" : avg >= WARN_MS ? "warn" : "good";
  // The events as list entries, newest first; a whole re-routing is one entry.
  const events = $derived.by(() => {
    rev;
    const groups = groupEvents(trace.events);
    return { count: groups.length, latest: groups.slice(-100).reverse() };
  });
  const alerting = $derived.by(() => {
    rev;
    return trace.alerting;
  });
  // The route answers but the target itself never has: the round trip then reads as
  // total loss, which deserves an explanation rather than only a red row.
  const silentTarget = $derived.by(() => {
    rev;
    return trace.target.ttl === 0 && trace.target.times.length >= 5 && trace.pathEnd > 0;
  });

  // Position along the bar scale; values beyond a zoomed scale stop at its end.
  const pct = (v: number) => `${Math.min(100, (v / scale) * 100)}%`;
  const clock = dateTime;

  // Bumped when a threshold in `trace.config` (a plain object) changes.
  let thresholdsRev = $state(0);
  const alertsOn = $derived(thresholdsRev >= 0 && (trace.config.alertLatencyMs > 0 || trace.config.alertLossPct > 0));

  function setThreshold(key: "alertLatencyMs" | "alertLossPct", value: string) {
    trace.config[key] = Math.max(0, Number(value) || 0);
    thresholdsRev++;
    onsave();
  }

  /** Takes over a probe setting, kept within the limits of its input. */
  function setProbe(key: "timeoutMs" | "packetSize", input: HTMLInputElement, fallback: number) {
    const value = Math.round(Number(input.value)) || fallback;
    trace.config[key] = Math.min(Number(input.max), Math.max(Number(input.min), value));
    input.value = String(trace.config[key]);
    onprobe();
  }

  let alertsDialog: HTMLDialogElement;
  let probeDialog: HTMLDialogElement;
  let eventsDialog: HTMLDialogElement;
  let whoisDialog: HTMLDialogElement;
  let whois = $state({ addr: "", text: "" });
  const facts = $derived(whoisFacts(whois.text));

  /** Opens the alert thresholds; also called from the app menu. */
  export function openAlerts() {
    alertsDialog.showModal();
  }

  /** Opens the probe settings; also called from the app menu. */
  export function openProbe() {
    probeDialog.showModal();
  }

  async function lookUp(addr: string) {
    whois = { addr, text: "Looking up…" };
    whoisDialog.showModal();
    try {
      whois = { addr, text: await invoke<string>("whois", { addr }) };
    } catch (e) {
      whois = { addr, text: `Lookup failed: ${e}` };
    }
  }

  /**
   * Right-click menu of a table header: move that column left or right, then a tick per
   * column, as in View > Columns. `key` is null for the Hop column, which stays first.
   */
  async function columnMenu(event: MouseEvent, key: string | null) {
    event.preventDefault();
    const at = columns.findIndex((column) => column.key === key);
    const menu = await Menu.new({
      items: [
        ...(key === null
          ? []
          : [
              { id: "move-left", text: "Move left", enabled: at > 0, action: () => shiftColumn(key, -1) },
              { id: "move-right", text: "Move right", enabled: at < columns.length - 1, action: () => shiftColumn(key, 1) },
              { item: "Separator" as const },
            ]),
        ...COLUMNS.map((column) => ({
          id: `header-${column.key}`,
          text: column.label,
          checked: !hidden.includes(column.key),
          action: () => oncolumn(column.key),
        })),
      ],
    });
    await menu.popup();
  }

  /** Right-click menu of a hop row. */
  async function hopMenu(event: MouseEvent, hop: Hop) {
    event.preventDefault();
    const { addr, name } = hop;
    if (!addr) return;
    const menu = await Menu.new({
      items: [
        { id: "copy-ip", text: `Copy IP ${addr}`, action: () => navigator.clipboard.writeText(addr) },
        { id: "copy-name", text: "Copy name", enabled: !!name, action: () => navigator.clipboard.writeText(name ?? "") },
        { id: "whois", text: "Whois lookup…", action: () => lookUp(addr) },
      ],
    });
    await menu.popup();
  }

  // Height of the hop table in px once the divider was dragged; null keeps it as tall as its
  // rows. Shared by all tabs and remembered between launches; the graphs take what is left.
  const TABLE_HEIGHT_KEY = "traceplotter.tableHeight";
  let tableHeight = $state<number | null>(Number(localStorage.getItem(TABLE_HEIGHT_KEY)) || null);
  let traceEl: HTMLDivElement;
  let hopsEl: HTMLDivElement;
  /** Height kept free for the graphs when the table is sized. */
  const GRAPH_MIN = 260;

  function setTableHeight(height: number | null) {
    tableHeight = height;
    if (height === null) localStorage.removeItem(TABLE_HEIGHT_KEY);
    else localStorage.setItem(TABLE_HEIGHT_KEY, String(height));
  }

  function resizeTable(height: number) {
    setTableHeight(Math.round(Math.max(60, Math.min(height, traceEl.clientHeight - GRAPH_MIN))));
  }

  function dragDivider(event: PointerEvent) {
    const divider = event.currentTarget as HTMLElement;
    const top = hopsEl.getBoundingClientRect().top;
    divider.setPointerCapture(event.pointerId);
    divider.onpointermove = (move) => resizeTable(move.clientY - top);
    divider.onpointerup = () => (divider.onpointermove = null);
  }

  function nudgeDivider(event: KeyboardEvent) {
    const step = event.key === "ArrowDown" ? 24 : event.key === "ArrowUp" ? -24 : 0;
    if (!step) return;
    event.preventDefault();
    resizeTable(hopsEl.clientHeight + step);
  }

  // Column widths in px set by dragging a header's right edge, by column key. Columns
  // without one size themselves. Shared by all tabs and remembered between launches.
  const COLUMN_WIDTHS_KEY = "traceplotter.columnWidths";
  let widths = $state<Record<string, number>>(loadWidths());

  function loadWidths() {
    try {
      const stored = JSON.parse(localStorage.getItem(COLUMN_WIDTHS_KEY) ?? "{}");
      return stored && typeof stored === "object" ? stored : {};
    } catch {
      return {};
    }
  }

  /** Sets a column's width, or with null lets it size itself again. */
  function setWidth(key: string, width: number | null) {
    const { [key]: _old, ...rest } = widths;
    widths = width === null ? rest : { ...rest, [key]: Math.round(Math.max(36, width)) };
    localStorage.setItem(COLUMN_WIDTHS_KEY, JSON.stringify(widths));
  }

  // Starting widths in px. The table has a fixed layout: every column has a width, so
  // resizing one moves only the columns to its right. The latency bar starts without a
  // width and takes whatever is left.
  const DEFAULT_WIDTHS: Record<string, number> = {
    hop: 52,
    ip: 140,
    name: 240,
    loss: 68,
    sent: 64,
    recv: 64,
    last: 64,
    avg: 64,
    min: 64,
    max: 64,
    jitter: 64,
    mos: 60,
  };
  const colWidth = (key: string) => {
    const width = widths[key] ?? DEFAULT_WIDTHS[key];
    return width ? `${width}px` : undefined;
  };
  // An empty last column takes the space left over when the latency bar is hidden or has
  // a width of its own; otherwise the browser would stretch every column to fill the table.
  const filler = $derived(!columns.some((column) => column.key === "bar") || widths.bar !== undefined);

  function dragColumn(event: PointerEvent, key: string) {
    const grip = event.currentTarget as HTMLElement;
    const startX = event.clientX;
    const startWidth = grip.parentElement!.getBoundingClientRect().width;
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    resizing = true;
    grip.onpointermove = (move) => setWidth(key, startWidth + move.clientX - startX);
    grip.onpointerup = grip.onpointercancel = () => {
      grip.onpointermove = null;
      resizing = false;
    };
  }

  function nudgeColumn(event: KeyboardEvent, key: string) {
    const step = event.key === "ArrowRight" ? 10 : event.key === "ArrowLeft" ? -10 : 0;
    if (!step) return;
    event.preventDefault();
    const grip = event.currentTarget as HTMLElement;
    setWidth(key, grip.parentElement!.getBoundingClientRect().width + step);
  }

  let statusTimer: ReturnType<typeof setTimeout> | undefined;
  /** Shows an export result for a few seconds. */
  function showStatus(text: string) {
    exportStatus = text;
    clearTimeout(statusTimer);
    if (text) statusTimer = setTimeout(() => (exportStatus = ""), 6000);
  }

  function fileName(extension: string) {
    const stamp = new Date().toISOString().slice(0, 19).replace(/[-:]/g, "").replace("T", "-");
    return `traceplotter-${trace.config.host}-${stamp}.${extension}`;
  }

  // Asks where to save; a cancelled dialog comes back as null.
  async function save(name: string, data: Uint8Array) {
    try {
      const path = await exportFile(name, data);
      showStatus(path ? `Saved ${path}` : "");
    } catch (e) {
      showStatus(`Export failed: ${e}`);
    }
  }

  const label = $derived.by(() => {
    const { host, protocol, port } = trace.config;
    // A tab restored paused has not resolved its address yet.
    const addr = trace.addr ? ` (${trace.addr})` : "";
    return `${host}${addr} ${protocolLabel({ protocol, port })}`;
  });

  const today = new Date().toDateString();
  /** Time of day, with the date when it is not today. */
  function stamp(t: number) {
    return new Date(t * 1000).toDateString() === today ? timeOfDay(t) : dateTime(t);
  }

  // The export functions are called from the app menu through `bind:this`.

  /**
   * The hop table as plain text, like `mtr --report`: the rows, the columns and their
   * order as on screen for the period shown. The latency bar is a drawing and is left out.
   */
  export function exportTxt() {
    const period = view.to > view.from ? `${clock(view.from)} to ${clock(view.to)}` : "no samples";
    const shown = columns.filter((column) => column.key !== "bar");
    const report = textTable(
      `TracePlotter report: ${label}\n${period}`,
      [{ label: "Hop" }, ...shown.map((column) => ({ label: column.label, left: column.text }))],
      // The dash the table shows for "no value" is written as a plain hyphen.
      rows.map((row) => [
        row.roundTrip ? "RT" : String(row.hop.ttl),
        ...shown.map((column) => column.value(row).replaceAll("–", "-")),
      ]),
    );
    save(fileName("txt"), new TextEncoder().encode(report));
  }

  // One row per probe, whole history. The IP is the hop's current address, not the one at probe time.
  export function exportCsv() {
    const lines = ["time,hop,ip,rtt_ms"];
    for (const hop of trace.hops) {
      for (let i = 0; i < hop.times.length; i++) {
        const time = new Date(hop.times[i] * 1000).toISOString();
        lines.push(`${time},${hop.ttl},${hop.addr ?? ""},${hop.rtts[i] ?? ""}`);
      }
    }
    save(fileName("csv"), new TextEncoder().encode(lines.join("\n") + "\n"));
  }

  // The graphs, one under the other, or with `only` just the graph at that position.
  // Their canvases are transparent, so they are redrawn on the app background, each
  // under a caption.
  export function exportPng(only?: number) {
    const canvases = graphs.flatMap((graph, i) => {
      const canvas = graph?.canvas();
      const wanted = only === undefined || only === i;
      return wanted && canvas && graphHops[i] ? [{ canvas, caption: headingOf(graphHops[i]) }] : [];
    });
    if (!canvases.length) return;
    const captionHeight = 28 * devicePixelRatio;
    const image = document.createElement("canvas");
    image.width = Math.max(...canvases.map((c) => c.canvas.width));
    image.height = canvases.reduce((sum, c) => sum + c.canvas.height + captionHeight, 0);
    const ctx = image.getContext("2d")!;
    ctx.fillStyle = BACKGROUND;
    ctx.fillRect(0, 0, image.width, image.height);
    ctx.font = `${13 * devicePixelRatio}px "Segoe UI", sans-serif`;
    ctx.textBaseline = "middle";
    let y = 0;
    for (const { canvas, caption } of canvases) {
      ctx.fillStyle = "#e6eaf0";
      ctx.fillText(`${caption} · ${stamp(view.from)} – ${stamp(view.to)}`, 12 * devicePixelRatio, y + captionHeight / 2);
      ctx.drawImage(canvas, 0, y + captionHeight);
      y += captionHeight + canvas.height;
    }
    image.toBlob(async (blob) => {
      if (blob) save(fileName("png"), new Uint8Array(await blob.arrayBuffer()));
    });
  }
</script>

<div class="trace" bind:this={traceEl}>
  <h1 class="print-only">
    TracePlotter · {label}{#if view.to > view.from} · {clock(view.from)} – {clock(view.to)}{/if}
  </h1>
  {#if trace.error}
    <p class="error" role="alert">{trace.error}</p>
  {/if}
  {#if silentTarget}
    <p class="hint" role="status">
      The target has not answered yet, so the round trip shows 100 % loss.
      {#if trace.config.protocol === "icmp"}
        If it should answer, allow ICMP replies under Tools > Allow ICMP replies in Windows Firewall…, or trace it
        with ICMP (Windows API), TCP or UDP.
      {:else if trace.config.protocol === "winicmp"}
        It may not answer pings; try TCP or UDP.
      {:else}
        It may not answer on this port; try another port or protocol.
      {/if}
    </p>
  {/if}

  <div class="hops" bind:this={hopsEl} style:height={tableHeight === null ? undefined : `${tableHeight}px`}>
    {#snippet grip(key: string, label: string)}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <span
        class="grip"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize the {label} column. Double-click for its usual width."
        title="Drag to resize. Double-click for the usual width."
        tabindex="0"
        onpointerdown={(event) => dragColumn(event, key)}
        onkeydown={(event) => nudgeColumn(event, key)}
        ondblclick={() => setWidth(key, null)}
      ></span>
    {/snippet}
    <table>
      <colgroup>
        <col style:width={colWidth("hop")} />
        {#each columns as column (column.key)}
          <col style:width={colWidth(column.key)} />
        {/each}
        {#if filler}<col />{/if}
      </colgroup>
      <thead>
        <tr>
          <th class="hop" oncontextmenu={(event) => columnMenu(event, null)}>Hop{@render grip("hop", "Hop")}</th>
          {#each columns as column (column.key)}
            <th
              class:text={column.text}
              class:drop={dropTarget === column.key}
              title={column.title ?? "Drag to move this column. Right-click for more."}
              draggable="true"
              ondragstart={(event) => {
                if (resizing) return event.preventDefault();
                dragged = column.key;
                event.dataTransfer?.setData("text/plain", column.key);
              }}
              ondragover={(event) => {
                if (!dragged || dragged === column.key) return;
                event.preventDefault();
                dropTarget = column.key;
              }}
              ondragleave={() => (dropTarget = null)}
              ondrop={(event) => {
                event.preventDefault();
                dropColumn(column.key);
              }}
              ondragend={() => (dragged = dropTarget = null)}
              oncontextmenu={(event) => columnMenu(event, column.key)}
            >
              {column.key === "bar"
                ? `Latency (0 – ${scale.toFixed(scale < 10 ? 1 : 0)} ms)${barZoom > 1 ? " · zoomed" : ""}`
                : column.label}
              {@render grip(column.key, column.label)}
            </th>
          {/each}
          {#if filler}<th></th>{/if}
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.roundTrip ? "round trip" : row.hop.ttl)}
          {@const { hop, s, roundTrip } = row}
          {@const shown = graphHops.includes(hop)}
          <tr class:selected={shown} class:round-trip={roundTrip} oncontextmenu={(event) => hopMenu(event, hop)}>
            <td class="hop">
              <button
                class="badge {grade(s.avg)}"
                aria-pressed={shown}
                aria-label={roundTrip ? "Graph of the round trip to the target" : `Graph of hop ${hop.ttl}`}
                title="Show or hide this row's graph. Right-click for more."
                onclick={() => toggleGraph(roundTrip ? ROUND_TRIP : hop.ttl)}>{roundTrip ? "RT" : hop.ttl}</button
              >
            </td>
            {#each columns as column (column.key)}
              {#if column.key === "bar"}
                <td class="bar">
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <div
                    class="track"
                    style:background={bands}
                    onwheel={zoomBars}
                    ondblclick={() => (barZoom = 1)}
                    onmousemove={(event) => (barTip = { x: event.clientX, y: event.clientY, key: rowKey(row) })}
                    onmouseleave={() => (barTip = null)}
                  >
                    <!-- Packet loss as a red bar from the left: the full width is 100 %. -->
                    {#if s.lossPct > 0}<span class="loss" style:width="{s.lossPct}%"></span>{/if}
                    {#if s.avg !== null && s.min !== null && s.max !== null}
                      <span class="range" style:left={pct(s.min)} style:right="calc(100% - {pct(s.max)})"></span>
                      <span class="avg" style:left={pct(s.avg)}></span>
                      {#if s.last !== null}
                        <span class="last" style:left={pct(s.last)}>×</span>
                      {/if}
                      {#if s.max > scale}<span class="over">›</span>{/if}
                    {/if}
                  </div>
                </td>
              {:else}
                <td class={column.cls?.(row)} class:text={column.text}>{column.value(row)}</td>
              {/if}
            {/each}
            {#if filler}<td></td>{/if}
          </tr>
        {:else}
          <tr><td colspan={columns.length + 1 + Number(filler)} class="text">Waiting for the first round…</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="divider"
    role="separator"
    aria-orientation="horizontal"
    aria-label="Resize the hop table. Double-click to fit its rows."
    title="Drag to resize the table. Double-click to fit its rows."
    tabindex="0"
    onpointerdown={dragDivider}
    onkeydown={nudgeDivider}
    ondblclick={() => setTableHeight(null)}
  ></div>

  <div class="bar-row graph-head">
    <span>
      {#if view.to > view.from}{stamp(view.from)} – {stamp(view.to)}{/if}
      {#if pinnedEnd !== null}(scrolled back){/if}
    </span>
    <span class="tools">
      <span role="status">{exportStatus}</span>
      {#if pinnedEnd !== null}<button onclick={() => (pinnedEnd = null)}>Back to live</button>{/if}
      <button class:alerting onclick={openAlerts}>
        {alerting ? "Alerting" : alertsOn ? "Alerts on" : "Alerts off"}
      </button>
      <button onclick={() => eventsDialog.showModal()}>Events ({events.count})</button>
    </span>
  </div>
  <div class="graph">
    {#each graphHops as hop, i (hop === view.target ? ROUND_TRIP : hop.ttl)}
      {@const key = hop === view.target ? ROUND_TRIP : hop.ttl}
      <HopGraph
        onclose={graphHops.length > 1 ? () => (graphed = graphed.filter((k) => k !== key)) : undefined}
        onsave={() => exportPng(i)}
        bind:this={graphs[i]}
        {hop}
        heading={headingOf(hop)}
        from={view.from}
        to={view.to}
        intervalMs={trace.config.intervalMs}
        syncKey="trace-{trace.id}"
        withJitter={graphHops.length === 1}
        onpan={pan}
        onreset={() => (pinnedEnd = null)}
      />
    {/each}
  </div>
  {#if focusSeconds && view.latest - view.earliest > focusSeconds}
    <!-- step="any": with whole steps counted from a fractional minimum, the thumb could not
         reach the maximum, so it sat short of the end while live. -->
    <input
      class="scrub"
      class:live={pinnedEnd === null}
      type="range"
      aria-label="Scroll the time span back in time"
      title={pinnedEnd === null ? "Live: following the newest data" : "Scrolled back. Drag fully right to go live."}
      min={view.earliest + focusSeconds}
      max={view.latest}
      step="any"
      value={view.to}
      oninput={(e) => {
        const end = Number(e.currentTarget.value);
        pinnedEnd = end >= view.latest - 1 ? null : end;
      }}
    />
  {/if}

  {#if barTip && tipRow}
    {@const s = tipRow.s}
    <div class="bar-tip" role="tooltip" style:left="{barTip.x}px" style:top="{barTip.y}px">
      {#if s.avg === null}
        <div>No replies</div>
      {:else}
        <div>Min {ms(s.min)} · Avg {ms(s.avg)} · Max {ms(s.max)} · Last {s.last === null ? "lost" : ms(s.last)} ms</div>
      {/if}
      {#if s.lossPct > 0}<div class="bad">Loss {s.lossPct.toFixed(1)}% (red bar)</div>{/if}
      <div class="note">Wheel zooms · double-click fits</div>
    </div>
  {/if}

  <dialog bind:this={alertsDialog} aria-labelledby="alerts-title">
    <h2 id="alerts-title">Alerts for {trace.config.host}</h2>
    <p>Alert when the round trip's last {ALERT_WINDOW} probes go over a limit. 0 switches a limit off.</p>
    <label>
      Average latency over
      <input
        type="number"
        min="0"
        value={trace.config.alertLatencyMs}
        onchange={(e) => setThreshold("alertLatencyMs", e.currentTarget.value)}
      />
      ms
    </label>
    <label>
      Packet loss of at least
      <input
        type="number"
        min="0"
        max="100"
        value={trace.config.alertLossPct}
        onchange={(e) => setThreshold("alertLossPct", e.currentTarget.value)}
      />
      %
    </label>
    <form method="dialog"><button>Close</button></form>
  </dialog>

  <dialog bind:this={probeDialog} aria-labelledby="probe-title">
    <h2 id="probe-title">Probe settings for {trace.config.host}</h2>
    <p>
      Packet type: {protocolLabel(trace.config)}. To use another, pick it in the toolbar and start the target again.
    </p>
    <label>
      Wait for replies
      <input
        type="number"
        min="100"
        max="10000"
        step="100"
        value={trace.config.timeoutMs ?? DEFAULT_TIMEOUT_MS}
        onchange={(e) => setProbe("timeoutMs", e.currentTarget, DEFAULT_TIMEOUT_MS)}
      />
      ms, at least the interval
    </label>
    <label>
      Packet size
      <input
        type="number"
        min="48"
        max="1024"
        value={trace.config.packetSize ?? DEFAULT_PACKET_SIZE}
        onchange={(e) => setProbe("packetSize", e.currentTarget, DEFAULT_PACKET_SIZE)}
      />
      bytes, the whole IP packet (TCP ignores it)
    </label>
    <form method="dialog"><button>Close</button></form>
  </dialog>

  <dialog bind:this={eventsDialog} aria-labelledby="events-title">
    <h2 id="events-title">Events for {trace.config.host}</h2>
    <ul class="events">
      {#each events.latest as event}
        <li>
          <div class="event-head">
            <span class="tag {event.kind}">
              {event.kind === "route" ? "Route changed" : event.kind === "alert" ? "Alert" : "Recovered"}
            </span>
            <span class="what">
              {#if event.kind === "route"}
                {#if event.length}{event.length.from} → {event.length.to} hops{/if}
                {#if event.length && event.hops.length}·{/if}
                {#if event.hops.length}{event.hops.length} {event.hops.length === 1 ? "hop" : "hops"} moved{/if}
              {:else}
                {event.text}
              {/if}
            </span>
            <time>{clock(event.t)}</time>
          </div>
          {#if event.kind === "route" && event.hops.length}
            <div class="moves">
              {#each event.hops as hop}
                <span class="ttl">Hop {hop.ttl}</span>
                <span>{hop.from}</span>
                <span class="arrow">→</span>
                <span>{hop.to}</span>
              {/each}
            </div>
          {/if}
        </li>
      {:else}
        <li class="none">No route changes or alerts yet.</li>
      {/each}
    </ul>
    <form method="dialog"><button>Close</button></form>
  </dialog>

  <dialog bind:this={whoisDialog} aria-labelledby="whois-title">
    <h2 id="whois-title">Whois {whois.addr}</h2>
    {#if facts.length}
      <dl class="facts">
        {#each facts as fact}
          <dt>{fact.label}</dt>
          <dd>{fact.value}</dd>
        {/each}
      </dl>
      <details>
        <summary>Full record</summary>
        <pre>{whoisRecord(whois.text)}</pre>
      </details>
    {:else}
      <pre>{whois.text}</pre>
    {/if}
    <form method="dialog"><button>Close</button></form>
  </dialog>
</div>

<style>
  .trace {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .error {
    margin: 0;
    padding: 8px 16px;
    background: #4c1d1d;
    color: #fecaca;
  }
  .hint {
    margin: 0;
    padding: 6px 16px;
    background: #3b3209;
    color: #fde68a;
    font-size: 13px;
  }
  /* As tall as its rows, until the divider is dragged; the graphs take the rest. */
  .hops {
    flex: none;
    max-height: calc(100% - 260px);
    overflow: auto;
  }
  .divider {
    flex: none;
    height: 7px;
    border-block: 1px solid var(--line);
    background: var(--panel);
    cursor: row-resize;
    touch-action: none;
  }
  .divider:hover,
  .divider:focus-visible {
    background: var(--accent);
  }
  /* Fixed layout: column widths come from the <col> elements, never from the content, so
     a resized column keeps its width and text that does not fit is cut with an ellipsis. */
  table {
    table-layout: fixed;
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }
  th,
  td {
    padding: 5px 10px;
    text-align: right;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hop {
    padding-inline: 6px;
    text-align: center;
  }
  th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--panel);
    color: var(--muted);
    font-weight: 500;
    font-size: 12px;
  }
  th[draggable="true"] {
    cursor: grab;
  }
  /* Where a dragged column will land. */
  th.drop {
    box-shadow: inset 3px 0 var(--accent);
    background: #1c3f63;
  }
  /* Drag handle on a header's right edge. */
  .grip {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 7px;
    border-right: 1px solid var(--line);
    cursor: col-resize;
    touch-action: none;
  }
  .grip:hover,
  .grip:focus-visible {
    border-right: 3px solid var(--accent);
  }
  .text {
    text-align: left;
  }
  .name {
    color: var(--muted);
  }
  tbody tr {
    position: relative;
    border-top: 1px solid var(--line);
  }
  tbody tr:hover {
    background: var(--panel);
  }
  /* Rows that have a graph: a clear blue tint and a blue bar on the left edge. */
  tr.selected,
  tr.selected:hover {
    background: #1c3f63;
  }
  tr.selected td:first-child {
    box-shadow: inset 4px 0 var(--accent);
  }
  tr.round-trip {
    border-top: 2px solid var(--muted);
    font-weight: 600;
  }
  tr.round-trip .badge {
    padding: 0 5px;
  }
  /* The hop button stretches over its row, so the whole row is the click target. */
  td button {
    all: unset;
    cursor: pointer;
  }
  td button::after {
    content: "";
    position: absolute;
    inset: 0;
  }
  td button:focus-visible::after {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  /* Hop number in a circle coloured by the hop's average latency. */
  td .badge {
    display: inline-block;
    min-width: 20px;
    height: 20px;
    border-radius: 10px;
    text-align: center;
    line-height: 20px;
    font-size: 12px;
    font-weight: 600;
    color: #06140b;
    background: var(--good);
  }
  td .badge.warn {
    background: var(--warn);
  }
  td .badge.bad {
    background: var(--bad);
    color: #fff;
  }
  td .badge.none {
    background: #4b5563;
    color: #fff;
  }
  .warn {
    color: #fbbf24;
  }
  .bad {
    color: #f87171;
  }
  td.bar {
    padding: 0 8px;
  }
  /* The coloured zones fill the row; marks are placed along the track's own width. */
  /* It sits above the row-wide click target of the hop button, so that its tooltip, wheel
     and double-click reach it. */
  .track {
    position: relative;
    z-index: 1;
    height: 30px;
    --good-bg: rgba(34, 197, 94, 0.22);
    --warn-bg: rgba(234, 179, 8, 0.22);
    --bad-bg: rgba(239, 68, 68, 0.26);
  }
  .range,
  .avg,
  .last,
  .over {
    position: absolute;
    top: 50%;
    translate: -50% -50%;
  }
  /* Follows the pointer; placed to its lower left, as the bars are at the window's right edge. */
  .bar-tip {
    position: fixed;
    z-index: 10;
    translate: calc(-100% - 12px) 14px;
    padding: 4px 8px;
    border: 1px solid var(--line);
    border-radius: 4px;
    background: rgba(15, 19, 25, 0.95);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    pointer-events: none;
  }
  .bar-tip .note {
    color: var(--muted);
    font-size: 11px;
  }
  .loss {
    position: absolute;
    left: 0;
    top: 6px;
    bottom: 6px;
    min-width: 2px;
    background: rgba(220, 38, 38, 0.75);
  }
  /* The bar continues beyond the end of a zoomed scale. */
  .over {
    right: 0;
    translate: 0 -50%;
    color: #f8fafc;
    font-weight: 700;
    line-height: 1;
  }
  .range {
    translate: 0 -50%;
    height: 2px;
    min-width: 2px;
    background: #cbd5e1;
  }
  .avg {
    width: 9px;
    height: 9px;
    box-sizing: border-box;
    border: 2px solid var(--bad);
    border-radius: 50%;
    background: var(--bg);
  }
  .last {
    color: #38bdf8;
    font-weight: 700;
    line-height: 1;
  }
  .bar-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    justify-content: space-between;
    align-items: center;
    padding: 4px 16px;
    background: var(--panel);
    color: var(--muted);
    font-size: 13px;
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .tools button,
  dialog button {
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 2px 10px;
    cursor: pointer;
  }
  .tools button.alerting {
    border-color: var(--bad);
    background: #4c1d1d;
    color: #fecaca;
  }
  .graph {
    flex: 1 1 0;
    min-height: 200px;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }
  /* Yellow while scrolled back, green while following the newest data. */
  .scrub {
    margin: 4px 16px 8px;
    accent-color: var(--warn);
  }
  .scrub.live {
    accent-color: var(--good);
  }
  dialog {
    max-width: min(720px, 90vw);
    max-height: 80vh;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--panel);
    color: var(--text);
  }
  dialog::backdrop {
    background: rgba(0, 0, 0, 0.5);
  }
  dialog h2 {
    margin: 0 0 8px;
    font-size: 16px;
  }
  dialog p {
    margin: 0 0 12px;
    color: var(--muted);
  }
  dialog label {
    display: block;
    margin-bottom: 8px;
  }
  dialog input {
    width: 72px;
    padding: 2px 6px;
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 4px;
  }
  /* The dialog is the only thing that scrolls; its Close button stays in view at the bottom. */
  dialog form {
    position: sticky;
    bottom: 0;
    margin-top: 12px;
    padding: 8px 0;
    text-align: right;
    background: var(--panel);
  }
  .facts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 16px;
    margin: 0 0 12px;
    min-width: 360px;
  }
  .facts dt {
    color: var(--muted);
  }
  .facts dd {
    margin: 0;
    user-select: text;
    overflow-wrap: anywhere;
  }
  dialog summary {
    cursor: pointer;
    color: var(--muted);
    margin-bottom: 8px;
  }
  dialog pre {
    user-select: text;
    margin: 0;
    font-size: 12px;
    white-space: pre-wrap;
  }
  /* One entry per re-routing, alert or recovery: a coloured tag, a summary and the time
     on one line, then for a re-routing the hops that moved, as aligned columns. */
  .events {
    min-width: min(560px, 80vw);
    margin: 0;
    padding: 0;
    max-height: 55vh;
    overflow: auto;
    list-style: none;
    font-size: 13px;
  }
  .events li {
    padding: 8px 4px;
    border-top: 1px solid var(--line);
  }
  .events li:first-child {
    border-top: 0;
  }
  .events .none {
    color: var(--muted);
  }
  .event-head {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  .event-head .what {
    flex: 1;
  }
  .events time {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .tag {
    padding: 1px 8px;
    border-radius: 10px;
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    background: #1e3a5f;
    color: #bfdbfe;
  }
  .tag.alert {
    background: #4c1d1d;
    color: #fecaca;
  }
  .tag.recover {
    background: #14532d;
    color: #bbf7d0;
  }
  .moves {
    display: grid;
    grid-template-columns: max-content max-content max-content max-content;
    gap: 2px 12px;
    margin: 6px 0 0 4px;
    font-variant-numeric: tabular-nums;
  }
  .moves .ttl,
  .moves .arrow {
    color: var(--muted);
  }
  .print-only {
    display: none;
  }
  /* Edit > Export page as PDF prints the page: heading, full hop table, then the graphs. */
  @media print {
    .print-only {
      display: block;
      margin: 0 0 8px;
      font-size: 15px;
      font-weight: 600;
    }
    .trace {
      display: block;
      height: auto;
    }
    .hops {
      height: auto !important;
      max-height: none;
      overflow: visible;
    }
    th {
      position: static;
    }
    td,
    th {
      padding: 3px 6px;
      font-size: 11px;
    }
    /* Colours picked for a dark screen, redone for white paper. */
    tr.selected {
      background: none;
    }
    tr.selected td:first-child {
      box-shadow: none;
    }
    .warn {
      color: #b45309;
    }
    .bad {
      color: #b91c1c;
    }
    .range {
      background: #475569;
    }
    .divider,
    .grip,
    .tools,
    .scrub,
    .bar-tip {
      display: none;
    }
    .graph {
      display: block;
      overflow: visible;
    }
  }
</style>

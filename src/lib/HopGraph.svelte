<script lang="ts" module>
  /** Tints of the latency bands behind a graph, green to red; the toolbar's legend uses them too. */
  export const BAND_FILLS = ["rgba(34, 197, 94, 0.32)", "rgba(234, 179, 8, 0.34)", "rgba(239, 68, 68, 0.36)"];
</script>

<script lang="ts">
  import { Menu } from "@tauri-apps/api/menu";
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";
  import { settings } from "./settings";
  import { jitterSeries, lossSeries, niceCeil } from "./stats";
  import { timeOfDay } from "./time";
  import type { Hop } from "./trace";

  type Props = {
    /** The hop's samples for the period shown. */
    hop: Hop;
    heading: string;
    /** The period shown, in Unix seconds. */
    from: number;
    to: number;
    intervalMs: number;
    /** Graphs with the same key move their cursors together. */
    syncKey: string;
    /** Adds the thin jitter strip above the latency graph. */
    withJitter: boolean;
    /** The user dragged the graph or turned the wheel: move the period by this fraction of its length. */
    onpan: (fraction: number) => void;
    /** The user double-clicked: back to the live period. */
    onreset: () => void;
    /** Closes this graph. Left out for the only graph, which cannot be closed. */
    onclose?: () => void;
    /** Saves just this graph as a PNG. */
    onsave: () => void;
    /** Copies just this graph to the clipboard as a PNG. */
    oncopy: () => void;
  };
  let { hop, heading, from, to, intervalMs, syncKey, withJitter, onpan, onreset, onclose, onsave, oncopy }: Props = $props();

  /** Right-click menu of the graph. */
  async function graphMenu(event: MouseEvent) {
    event.preventDefault();
    const menu = await Menu.new({
      items: [
        { id: "live", text: "Back to live", action: onreset },
        { item: "Separator" },
        { id: "save", text: "Save this graph as PNG…", action: onsave },
        { id: "copy", text: "Copy this graph as PNG", action: oncopy },
        { item: "Separator" },
        { id: "close", text: "Close this graph", enabled: !!onclose, action: () => onclose?.() },
      ],
    });
    await menu.popup();
  }

  const LOSS = "#f87171";

  let chartEl: HTMLDivElement;
  let jitterEl = $state<HTMLDivElement>();
  let plot: uPlot | undefined;
  let jitterPlot: uPlot | undefined;
  // What the cursor points at, for the readout next to it.
  // Placed in window coordinates, so it is not cut off at the edge of its own graph.
  let tip = $state<{ left: number; top: number; flip: boolean; up: boolean; pinned: boolean; lines: string[] } | null>(null);
  // The data the plots currently hold, for that readout.
  let shown = { js: [] as (number | null)[], breaks: new Set<number>(), bucket: 0 };

  // The loss axis shows only while the period has a lost probe.
  const lossy = $derived(hop.rtts.includes(null));

  /** The canvas of the latency graph, for the PNG export. */
  export function canvas() {
    return plot?.ctx.canvas;
  }

  /**
   * Times, latency, jitter and packet loss for the plots. Where probing stopped for a
   * while (pause, app closed) a null point is added right after the last probe, so the
   * lines break there instead of running flat to the next probe. uPlot breaks a line
   * only at null; it draws straight through undefined.
   */
  function graphData() {
    const step = intervalMs / 1000;
    const maxStep = Math.max(5, 3 * step);
    // Loss is counted over buckets of at least 10 probes, about 120 across the graph.
    const bucket = Math.ceil(Math.max(10 * step, (to - from) / 120) / step) * step;
    const jitters = jitterSeries(hop.rtts);
    const losses = lossSeries(hop.times, hop.rtts, bucket);
    const xs: number[] = [];
    const ys: (number | null)[] = [];
    const js: (number | null)[] = [];
    const ls: (number | null)[] = [];
    const breaks = new Set<number>();
    hop.times.forEach((time, i) => {
      const resumed = i > 0 && time - hop.times[i - 1] > maxStep;
      if (resumed) {
        const at = hop.times[i - 1] + step;
        breaks.add(at);
        xs.push(at);
        ys.push(null);
        js.push(null);
        ls.push(null);
      }
      xs.push(time);
      ys.push(hop.rtts[i]);
      // The first probe after a break has no meaningful jitter.
      js.push(resumed ? null : jitters[i]);
      ls.push(losses[i]);
    });
    return { xs, ys, js, ls, breaks, bucket };
  }

  // Green, yellow and red latency zones behind the graph line. They are drawn only over
  // the stretches of time that have probes; where the trace was not running (before it
  // began, while paused, while the app was closed) the graph stays plain.
  function drawBands(u: uPlot) {
    const { left, top, width, height } = u.bbox;
    const y = (ms: number) => Math.min(top + height, Math.max(top, u.valToPos(ms, "y", true)));
    const x = (t: number) => Math.min(left + width, Math.max(left, u.valToPos(t, "x", true)));
    const edges = [top + height, y(settings.warnMs), y(settings.badMs), top];
    const step = intervalMs / 1000;
    const xs = u.data[0];
    let from: number | null = null;
    for (let i = 0; i <= xs.length; i++) {
      // A stretch ends at a break point, or after the last probe.
      const ends = i === xs.length || shown.breaks.has(xs[i]);
      if (!ends) {
        from ??= xs[i];
        continue;
      }
      if (from !== null) {
        // The last probe's value holds for one interval, so the stretch covers that too.
        const [x0, x1] = [x(from), x(xs[i - 1] + step)];
        BAND_FILLS.forEach((fill, band) => {
          u.ctx.fillStyle = fill;
          u.ctx.fillRect(x0, edges[band + 1], x1 - x0, edges[band] - edges[band + 1]);
        });
      }
      from = null;
    }
  }

  function readout(u: uPlot) {
    const { idx, left, top } = u.cursor;
    if (dragging || idx == null || left == null || top == null || left < 0) {
      tip = null;
      return;
    }
    const [xs, ls, ys] = u.data;
    // uPlot points at the nearest sample however far away it is. Over a stretch without
    // samples (before the trace began, or while it was stopped) there is nothing to read.
    const at = u.valToPos(xs[idx], "x");
    const stepPx = u.valToPos(xs[idx] + intervalMs / 1000, "x") - at;
    if (Math.abs(at - left) > Math.max(8, stepPx)) {
      tip = null;
      return;
    }
    const time = timeOfDay(xs[idx]);
    const rtt = ys[idx];
    const jitter = shown.js[idx];
    const loss = ls[idx];
    const lines = shown.breaks.has(xs[idx])
      ? [time, "not probing"]
      : [
          time,
          rtt == null ? "lost" : `${rtt.toFixed(1)} ms`,
          ...(jitter == null ? [] : [`jitter ${jitter.toFixed(1)} ms`]),
          // The loss is that of the stretch of time the probe falls in, not of the whole graph.
          ...(loss ? [`loss ${loss.toFixed(0)} % in these ${Math.round(shown.bucket)} s`] : []),
        ];
    // Every stacked graph gets a readout for the same moment, because their cursors move
    // together. They must not land on each other: the graph under the pointer shows its
    // readout next to the pointer and keeps it inside its own area (above the pointer in
    // the lower part); the others pin theirs to the top of their own area.
    const area = u.over.getBoundingClientRect();
    tip = {
      left: area.left + left,
      top: hovered ? area.top + top : area.top + 4,
      // To the left of the cursor in the right half of the graph.
      flip: left > area.width / 2,
      up: hovered && top > area.height - 90,
      pinned: !hovered,
      lines,
    };
  }

  // Whether the pointer is over this graph, as opposed to a cursor synced from another one.
  let hovered = false;

  let dragging = $state(false);

  // Grab the graph and pull it sideways: the period follows the pointer, so dragging to
  // the right brings earlier data into view.
  function startDrag(event: PointerEvent) {
    // Left button only; the right button opens the graph's menu.
    if (event.button !== 0 || !plot) return;
    const width = plot.over.clientWidth;
    let lastX = event.clientX;
    chartEl.setPointerCapture(event.pointerId);
    dragging = true;
    tip = null;
    chartEl.onpointermove = (move) => {
      onpan((lastX - move.clientX) / width);
      lastX = move.clientX;
    };
    chartEl.onpointerup = chartEl.onpointercancel = () => {
      chartEl.onpointermove = null;
      dragging = false;
    };
  }

  $effect(() => {
    // uPlot cannot show or hide an axis later, so the plots are rebuilt when loss starts or ends.
    const withLoss = lossy;
    const axis = { stroke: "#9aa4b2", grid: { stroke: "rgba(255, 255, 255, 0.08)" }, ticks: { stroke: "#232a36" } };
    const range = (_u: uPlot, _min: number, max: number): [number, number] => [0, Math.max(1, max * 1.1)];
    // The latency axis starts at 0 and ends at a round number a fifth above the
    // highest value in view. Starting at 0 keeps heights honest: a wobble of 1 ms on 10 ms
    // looks small, not like the full-height swings an axis fitted to 10..12 would show.
    // The headroom keeps the line off the top edge, and round ends keep the axis from
    // jumping with every new maximum.
    const fit = (_u: uPlot, _min: number | null, max: number | null): [number, number] => [
      0,
      niceCeil((max ?? 0) * 1.2),
    ];
    const line = { width: 1.5, points: { show: false }, paths: uPlot.paths.stepped!({ align: 1 }) };
    const shared = {
      cursor: {
        // Dragging pans (see startDrag), so uPlot's drag-to-select is off.
        drag: { x: false, y: false },
        sync: { key: syncKey },
        // Double-click is handled below, not by uPlot's own reset of the time axis.
        bind: { dblclick: () => null },
      },
      legend: { show: false },
      // The same room on the right for both plots. Left to itself, the latency plot keeps
      // some for its last time label and the jitter strip, which has no time axis, keeps
      // none, so the strip ran on past the right edge of the graph below it.
      padding: [null, 24, null, null] as [null, number, null, null],
    };
    const chart = new uPlot(
      {
        ...shared,
        width: chartEl.clientWidth,
        height: chartEl.clientHeight,
        scales: {
          y: { range: fit },
          // Always the full 0 to 100 %, so the height of a loss bar is the share of probes
          // lost. Fitted to the highest bar, one lost probe in ten would fill the graph.
          loss: { range: [0, 100] },
        },
        axes: [
          { ...axis, values: (_u, ticks) => ticks.map(timeOfDay) },
          { ...axis, label: "Latency (ms)", labelSize: 20, size: 44 },
          { show: withLoss, scale: "loss", side: 1, stroke: LOSS, label: "Packet loss %", labelSize: 20, size: 36, grid: { show: false }, ticks: { stroke: "#232a36" } },
        ],
        // Loss is drawn first so the latency line stays on top of it.
        series: [
          {},
          { ...line, show: withLoss, scale: "loss", width: 1, stroke: LOSS, fill: "rgba(239, 68, 68, 0.55)" },
          { ...line, stroke: "#f8fafc" },
        ],
        hooks: { drawClear: [drawBands], setCursor: [readout] },
      },
      [[], [], []],
      chartEl,
    );
    const strip = jitterEl;
    const jitter =
      strip &&
      new uPlot(
        {
          ...shared,
          width: strip.clientWidth,
          height: strip.clientHeight,
          scales: { y: { range } },
          // Same total widths as the latency and loss axes with their labels, so the plots line up.
          axes: [
            { show: false },
            { ...axis, size: 64, space: 24 },
            { show: withLoss, side: 1, size: 56, values: () => [], grid: { show: false }, ticks: { show: false } },
          ],
          series: [{}, { ...line, width: 1, stroke: LOSS }],
        },
        [[], []],
        strip,
      );
    plot = chart;
    jitterPlot = jitter;
    const resize = new ResizeObserver(() => {
      chart.setSize({ width: chartEl.clientWidth, height: chartEl.clientHeight });
      if (jitter && strip) jitter.setSize({ width: strip.clientWidth, height: strip.clientHeight });
    });
    resize.observe(chartEl);
    return () => {
      resize.disconnect();
      chart.destroy();
      jitter?.destroy();
      plot = jitterPlot = undefined;
    };
  });

  $effect(() => {
    // Read so that this runs again, on the new plots, after a rebuild.
    lossy;
    jitterEl;
    if (!plot) return;
    const { xs, ys, js, ls, breaks, bucket } = graphData();
    shown = { js, breaks, bucket };
    const x = { min: from, max: Math.max(to, from + 1) };
    plot.setData([xs, ls, ys], false);
    plot.setScale("x", x);
    jitterPlot?.setData([xs, js], false);
    jitterPlot?.setScale("x", x);
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="hop-graph" oncontextmenu={graphMenu}>
  {#if withJitter}
    <div class="jitter" bind:this={jitterEl}><span class="caption">Jitter (ms, running average)</span></div>
  {/if}
  <div
    class="chart"
    role="img"
    aria-label="Latency graph for {heading}. Drag or turn the wheel to move in time, double-click to return to live."
    class:dragging
    bind:this={chartEl}
    onpointerdown={startDrag}
    onwheel={(event) => {
      event.preventDefault();
      onpan(Math.sign(event.deltaY) * 0.1);
    }}
    ondblclick={onreset}
    onmouseenter={() => (hovered = true)}
    onmouseleave={() => {
      hovered = false;
      tip = null;
    }}
  >
    <span class="title">
      {heading}
      {#if onclose}
        <!-- Its own pointerdown must not start a drag, which would swallow the click. -->
        <button
          aria-label="Close the graph of {heading}"
          title="Close this graph"
          onpointerdown={(event) => event.stopPropagation()}
          onclick={onclose}>×</button
        >
      {/if}
    </span>
    {#if tip}
      <div class="tip" class:flip={tip.flip} class:up={tip.up} class:pinned={tip.pinned} style:left="{tip.left}px" style:top="{tip.top}px">
        {#each tip.lines as line}<div>{line}</div>{/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .hop-graph {
    flex: 1 1 0;
    min-height: 130px;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--line);
  }
  .jitter {
    position: relative;
    height: 64px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.05);
  }
  .caption {
    position: absolute;
    top: 2px;
    left: 50%;
    translate: -50%;
    z-index: 1;
    font-size: 11px;
    color: #f87171;
    pointer-events: none;
  }
  .chart {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    cursor: grab;
    touch-action: none;
  }
  .chart.dragging,
  .chart.dragging :global(.u-over) {
    cursor: grabbing;
  }
  /* uPlot sets a crosshair cursor on its overlay; the graph is grabbed instead. */
  .chart :global(.u-over) {
    cursor: grab;
  }
  .title {
    position: absolute;
    top: 4px;
    left: 76px;
    z-index: 1;
    padding: 0 6px;
    border-radius: 3px;
    background: rgba(15, 19, 25, 0.7);
    font-size: 12px;
    font-weight: 600;
    pointer-events: none;
  }
  .title button {
    all: unset;
    margin-left: 6px;
    padding: 0 4px;
    font-size: 15px;
    line-height: 1;
    color: var(--muted);
    cursor: pointer;
    pointer-events: auto;
  }
  .title button:hover {
    color: #f87171;
  }
  .title button:focus-visible {
    outline: 2px solid var(--accent);
  }
  .tip {
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
  .tip.flip {
    --dx: calc(-100% - 12px);
  }
  .tip.up {
    --dy: calc(-100% - 12px);
  }
  .tip.pinned {
    --dy: 0px;
  }
  .tip div:first-child {
    color: var(--muted);
  }
  /* The page prints dark on white, but the graphs are drawn in light colours on their
     canvases, so each graph keeps the dark background it was drawn for. */
  @media print {
    .hop-graph {
      display: block;
      break-inside: avoid;
      margin-top: 8px;
      border: 0;
      background: #0f1319;
      color: #e6eaf0;
      print-color-adjust: exact;
    }
    .jitter,
    .chart {
      height: auto;
      overflow: visible;
    }
    .tip {
      display: none;
    }
    /* uPlot sizes these in px for the window; let them follow the paper width instead. */
    .hop-graph :global(.uplot),
    .hop-graph :global(.u-wrap),
    .hop-graph :global(canvas) {
      width: 100% !important;
      height: auto !important;
    }
    .hop-graph :global(.u-over),
    .hop-graph :global(.u-under) {
      display: none;
    }
  }
</style>

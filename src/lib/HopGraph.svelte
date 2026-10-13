<script lang="ts">
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";
  import { BAD_MS, WARN_MS, jitterSeries, lossSeries } from "./stats";
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
  };
  let { hop, heading, from, to, intervalMs, syncKey, withJitter, onpan, onreset, onclose }: Props = $props();

  const LOSS = "#f87171";

  let chartEl: HTMLDivElement;
  let jitterEl = $state<HTMLDivElement>();
  let plot: uPlot | undefined;
  let jitterPlot: uPlot | undefined;
  // What the cursor points at, for the readout next to it.
  let tip = $state<{ left: number; top: number; flip: boolean; lines: string[] } | null>(null);
  // The data the plots currently hold, for that readout.
  let shown = { js: [] as (number | null)[], breaks: new Set<number>() };

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
    return { xs, ys, js, ls, breaks };
  }

  // Green, yellow and red latency zones behind the graph line.
  function drawBands(u: uPlot) {
    const { left, top, width, height } = u.bbox;
    const y = (ms: number) => Math.min(top + height, Math.max(top, u.valToPos(ms, "y", true)));
    const edges = [top + height, y(WARN_MS), y(BAD_MS), top];
    const fills = ["rgba(34, 197, 94, 0.32)", "rgba(234, 179, 8, 0.34)", "rgba(239, 68, 68, 0.36)"];
    fills.forEach((fill, i) => {
      u.ctx.fillStyle = fill;
      u.ctx.fillRect(left, edges[i + 1], width, edges[i] - edges[i + 1]);
    });
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
    const time =new Date(xs[idx] * 1000).toLocaleTimeString([], { hour12: false });
    const rtt = ys[idx];
    const jitter = shown.js[idx];
    const loss = ls[idx];
    const lines = shown.breaks.has(xs[idx])
      ? [time, "not probing"]
      : [
          time,
          rtt == null ? "lost" : `${rtt.toFixed(1)} ms`,
          ...(jitter == null ? [] : [`jitter ${jitter.toFixed(1)} ms`]),
          ...(loss ? [`loss ${loss.toFixed(0)} %`] : []),
        ];
    tip = { left: u.over.offsetLeft + left, top: u.over.offsetTop + top, flip: left > u.over.clientWidth / 2, lines };
  }

  let dragging = $state(false);

  // Grab the graph and pull it sideways: the period follows the pointer, so dragging to
  // the right brings earlier data into view.
  function startDrag(event: PointerEvent) {
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
    };
    const chart = new uPlot(
      {
        ...shared,
        width: chartEl.clientWidth,
        height: chartEl.clientHeight,
        scales: {
          y: { range },
          // At least 0 to 10 %, growing in steps of 10.
          loss: { range: (_u, _min, max) => [0, Math.max(10, Math.ceil(max / 10) * 10)] },
        },
        axes: [
          { ...axis, values: (_u, ticks) => ticks.map((t) => new Date(t * 1000).toLocaleTimeString([], { hour12: false })) },
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
    const { xs, ys, js, ls, breaks } = graphData();
    shown = { js, breaks };
    const x = { min: from, max: Math.max(to, from + 1) };
    plot.setData([xs, ls, ys], false);
    plot.setScale("x", x);
    jitterPlot?.setData([xs, js], false);
    jitterPlot?.setScale("x", x);
  });
</script>

<div class="hop-graph">
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
    onmouseleave={() => (tip = null)}
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
      <div class="tip" class:flip={tip.flip} style:left="{tip.left}px" style:top="{tip.top}px">
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
    position: absolute;
    z-index: 2;
    translate: 12px 12px;
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
    translate: calc(-100% - 12px) 12px;
  }
  .tip div:first-child {
    color: var(--muted);
  }
  @media print {
    .hop-graph {
      display: block;
      break-inside: avoid;
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

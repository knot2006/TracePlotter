<script lang="ts">
  import { untrack } from "svelte";
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";
  import { popupMenu } from "./ContextMenu.svelte";
  import { captionedImage, copyImage, saveExport, saveImage } from "./exports";
  import { exportName } from "./import";
  import { changeSetting, settings } from "./settings";
  import { alignLines, BLEND_RADIUS, latencyAxis, rangeGraph, stretchWidth, tallyStats, zoomSpan, type Tally } from "./stats";
  import { dateTime, timeTicks } from "./time";
  import { historyRange, protocolLabel, type Trace } from "./trace";

  type Props = {
    /** The targets to compare, each drawn as its own line. */
    traces: Trace[];
    /** Length of the newest stretch shown, 0 for everything. */
    focusSeconds: number;
    /** The user turned the wheel with Ctrl held: show this many newest seconds, 0 for everything. */
    onspan: (seconds: number) => void;
  };
  let { traces, focusSeconds, onspan }: Props = $props();
  // Seconds from the oldest round of any target to the newest, as of the last load.
  let available = 0;

  // Told apart on the dark background, and none of them the green, yellow or red that
  // grade latency elsewhere. Beyond these the colours come round again with dashed lines.
  const COLORS = ["#60a5fa", "#f59e0b", "#c084fc", "#2dd4bf", "#f472b6", "#a3e635", "#e2e8f0"];
  const dashed = (i: number) => i >= COLORS.length;
  /** A target as the legend and readout name it: with its probe when another target has the same host. */
  const name = ({ config }: Trace) =>
    traces.filter((other) => other.config.host === config.host).length > 1
      ? `${config.host} ${protocolLabel(config)}`
      : config.host;
  // Loss bars unless they take the targets' colours, as on a hop's graph.
  const LOSS = "#f87171";
  // New rounds keep coming; the lines follow them this often.
  const REFRESH_MS = 2000;

  let chartEl: HTMLDivElement;
  let plot: uPlot | undefined;
  // Each target's stretches by start (Unix seconds), their length and the period they
  // cover, for the readout and the exports.
  let shown: { stretches: Map<number, Tally>[]; width: number; from: number; to: number } = {
    stretches: [],
    width: 0,
    from: 0,
    to: 0,
  };
  /**
   * The graph as an image, under a caption with the period and the hosts in their
   * colours, which on screen are in the legend below the canvas.
   */
  function graphImage() {
    const canvas = plot?.ctx.canvas;
    if (!canvas) return null;
    const caption = [
      { text: `Round trip · ${dateTime(shown.from)} – ${dateTime(shown.to)} · `, color: "#e6eaf0" },
      ...traces.map((trace, i) => ({
        text: `${dashed(i) ? "┅" : "■"} ${name(trace)}   `,
        color: COLORS[i % COLORS.length],
      })),
    ];
    return captionedImage([{ canvas, caption }]);
  }

  /** Saves the graph as a PNG; also called from the app menu. */
  export function exportPng() {
    const image = graphImage();
    if (image) saveImage(image, exportName("compare", "graph", "png"));
  }

  function copyPng() {
    const image = graphImage();
    if (image) copyImage(image, "Graph");
  }

  /**
   * Saves what the graph draws as CSV: per target, one row per stretch with its probes
   * and their numbers. Also called from the app menu.
   */
  export function exportCsv() {
    const lines = ["time,host,seconds,sent,lost,avg_ms,min_ms,max_ms,jitter_ms"];
    const ms = (v: number | null) => (v === null ? "" : v.toFixed(2));
    traces.forEach((trace, i) => {
      for (const [t, tally] of shown.stretches[i] ?? []) {
        const s = tallyStats(tally);
        const time = new Date(t * 1000).toISOString();
        lines.push(
          `${time},${trace.config.host},${shown.width},${s.sent},${s.sent - s.received},` +
            `${ms(s.avg)},${ms(s.min)},${ms(s.max)},${ms(s.jitter)}`,
        );
      }
    });
    saveExport(exportName("compare", "stretches", "csv"), new TextEncoder().encode(lines.join("\n") + "\n"));
  }

  function graphMenu(event: MouseEvent) {
    // The readout would show through under the menu.
    tip = null;
    popupMenu(event, {
      items: [
        { id: "save", text: "Save graph as PNG…", action: exportPng },
        { id: "copy", text: "Copy graph as PNG", action: copyPng },
        { item: "Separator" },
        { id: "csv", text: "Save stretches as CSV…", action: exportCsv },
        { item: "Separator" },
        { id: "loss-colours", text: "Loss bars in each target's colour", checked: lossByTarget, action: toggleLossColours },
      ],
    });
  }
  // What the cursor points at: the moment, and per target its numbers over that stretch.
  // Placed in window coordinates, like the readout of a hop graph.
  let tip = $state<{
    left: number;
    top: number;
    flip: boolean;
    up: boolean;
    time: string;
    rows: { color: string; host: string; text: string }[];
  } | null>(null);

  function readout(u: uPlot) {
    const { idx, left, top } = u.cursor;
    if (idx == null || left == null || top == null || left < 0) {
      tip = null;
      return;
    }
    const x = u.data[0][idx];
    const area = u.over.getBoundingClientRect();
    const { width } = shown;
    tip = {
      left: area.left + left,
      top: area.top + top,
      flip: left > area.width / 2,
      up: top > area.height / 2,
      time: `${dateTime(x)}, ${width >= 60 ? `${width / 60} min` : `${width} s`}`,
      rows: traces.map((trace, i) => {
        const tally = shown.stretches[i]?.get(x);
        const s = tally && tallyStats(tally);
        const text = !s?.sent
          ? "no probes"
          : s.avg === null
            ? `all ${s.sent} lost`
            : `${s.avg.toFixed(1)} ms (${s.min!.toFixed(1)}–${s.max!.toFixed(1)}), ` +
              `loss ${s.lossPct.toFixed(1)} %, jitter ${s.jitter === null ? "–" : s.jitter.toFixed(1)} ms`;
        return { color: COLORS[i % COLORS.length], host: name(trace), text };
      }),
    };
  }

  // Room under the plot for its legend: the hosts and their colours.
  const LEGEND_HEIGHT = 48;
  const plotHeight = () => Math.max(100, chartEl.clientHeight - LEGEND_HEIGHT);

  // Whether a target lost probes in the period; the loss axis shows only then.
  let lossy = $state(false);
  // The setting of the same name, here so that changing it from the menu redraws the plot.
  let lossByTarget = $state(settings.lossByTarget);

  function toggleLossColours() {
    changeSetting("lossByTarget", !lossByTarget);
    lossByTarget = settings.lossByTarget;
  }

  $effect(() => {
    // uPlot cannot show or hide an axis later, so the plot is rebuilt when loss starts or ends.
    const withLoss = lossy;
    const byTarget = lossByTarget;
    const axis = { stroke: "#9aa4b2", grid: { stroke: "rgba(255, 255, 255, 0.08)" }, ticks: { stroke: "#232a36" } };
    const line = { points: { show: false }, paths: uPlot.paths.stepped!({ align: 1 }) };
    const chart = new uPlot(
      {
        width: chartEl.clientWidth,
        height: plotHeight(),
        cursor: { drag: { x: false, y: false } },
        // The numbers at the cursor are in the readout, so the legend only names the lines.
        legend: { live: false },
        hooks: { setCursor: [readout] },
        scales: {
          y: { range: (_u, min, max) => latencyAxis(min, max) },
          // The full 0 to 100 %, as on a hop's graph, so a bar's height is the share lost.
          loss: { range: [0, 100] },
        },
        axes: [
          { ...axis, values: (_u, ticks) => timeTicks(ticks) },
          { ...axis, label: "Round trip (ms)", labelSize: 20, size: 44 },
          { ...axis, show: withLoss, scale: "loss", side: 1, label: "Packet loss %", labelSize: 20, size: 36, grid: { show: false } },
        ],
        // Each target's loss as bars from the bottom, red as on a hop's graph or in the
        // target's own colour (see the settings and the right-click menu), drawn first so that the latency lines stay
        // on top of them.
        series: [
          {},
          ...traces.map((trace, i) => ({
            ...line,
            label: `${name(trace)} loss`,
            scale: "loss",
            stroke: byTarget ? COLORS[i % COLORS.length] : LOSS,
            fill: byTarget ? `${COLORS[i % COLORS.length]}4d` : "rgba(239, 68, 68, 0.45)",
            // Filled without an outline, which would run along the bottom where nothing is lost.
            width: 0,
          })),
          ...traces.map((trace, i) => ({
            label: name(trace),
            stroke: COLORS[i % COLORS.length],
            dash: dashed(i) ? [6, 4] : undefined,
            width: 1.5,
            ...line,
          })),
        ],
      },
      [[], ...traces.flatMap(() => [[], []])],
      chartEl,
    );
    plot = chart;
    // The legend names each target once, by its line: the loss bars are all red or share
    // the line's colour. Its last rows are the loss series, then the lines.
    const rows = chart.root.querySelectorAll<HTMLElement>(".u-legend .u-series");
    for (let i = 0; i < traces.length; i++) rows[rows.length - 2 * traces.length + i]?.style.setProperty("display", "none");
    // A rebuilt plot starts empty; it is filled at once, not at the next refresh.
    untrack(() => load());
    const resize = new ResizeObserver(() => chart.setSize({ width: chartEl.clientWidth, height: plotHeight() }));
    resize.observe(chartEl);
    return () => {
      resize.disconnect();
      chart.destroy();
      plot = undefined;
    };
  });

  // Loads started so far, and whether one is under way. A timer tick waits for it; a
  // load overtaken by a newer one (other targets, another period) drops its result, which
  // would no longer fit the chart.
  let loads = 0;
  let loading = false;

  /**
   * Reads each target's round trip over the period from its history files. The backend
   * starts every stretch at a multiple of the width, so the targets' stretches fall on
   * one grid and line up.
   */
  async function load(fromTimer = false) {
    if (fromTimer && loading) return;
    const ticket = ++loads;
    const targets = traces;
    const latest = Math.max(...targets.map((trace) => trace.target.times.at(-1) ?? 0));
    if (!latest || !plot) return;
    const earliest = Math.min(...targets.map((trace) => trace.diskStart || trace.target.times[0] || latest));
    const to = latest;
    available = latest - earliest;
    const from = focusSeconds ? to - focusSeconds : earliest;
    const fastest = Math.min(...targets.map((trace) => trace.config.intervalMs)) / 1000;
    const width = stretchWidth(to - from, (chartEl.clientWidth || 2000) * devicePixelRatio, fastest);
    loading = true;
    try {
      const results = await Promise.all(
        targets.map((trace) => historyRange(trace, from, to, width, [0]).catch(() => [])),
      );
      if (ticket !== loads || !plot) return;
      const trips = results.map((hops) => hops.find((hop) => hop.ttl === 0)?.stretches ?? []);
      shown = { stretches: trips.map((trip) => new Map(trip.map((s) => [s.t / 1000, s]))), width, from, to };
      // Averages blend with their neighbours once a stretch holds several probes (see `thin`).
      // Loss is counted over stretches of at least 10 probes, about 120 across the graph, as
      // on a hop's graph: over single stretches one lost probe would draw a tall bar.
      const graphs = trips.map((stretches, i) => {
        const several = stretches.length && stretches.reduce((n, s) => n + s.sent, 0) > 1.5 * stretches.length;
        const interval = targets[i].config.intervalMs / 1000;
        const lossSeconds = Math.max(10 * interval, (to - from) / 120, width);
        return rangeGraph(stretches, width, lossSeconds, several ? BLEND_RADIUS : 0, interval).data;
      });
      const { xs, ys } = alignLines([
        ...graphs.map((data) => new Map(data.xs.map((x, i) => [x, data.ls[i]]))),
        ...graphs.map((data) => new Map(data.xs.map((x, i) => [x, data.ys[i]]))),
      ]);
      lossy = graphs.some((data) => data.ls.some((loss) => !!loss));
      plot.setData([xs, ...ys], false);
      plot.setScale("x", { min: from, max: Math.max(to, from + 1) });
    } finally {
      if (ticket === loads) loading = false;
    }
  }

  $effect(() => {
    // Again whenever the targets or the period change, and as new rounds come.
    traces;
    focusSeconds;
    load();
    const timer = setInterval(() => load(true), REFRESH_MS);
    return () => clearInterval(timer);
  });
</script>

<div
  class="compare"
  bind:this={chartEl}
  role="figure"
  aria-label="Round-trip latency of {traces.map(name).join(', ')}"
  onmouseleave={() => (tip = null)}
  onwheel={(event) => {
    // Ctrl and the wheel zoom the time span; the newest data stays at the right edge.
    if (!event.ctrlKey) return;
    event.preventDefault();
    const next = zoomSpan(focusSeconds, event.deltaY > 0, available);
    if (next !== focusSeconds) onspan(next);
  }}
  oncontextmenu={graphMenu}
>
  {#if tip}
    <div class="tip" class:flip={tip.flip} class:up={tip.up} style:left="{tip.left}px" style:top="{tip.top}px">
      <div>{tip.time}</div>
      {#each tip.rows as row}
        <div><span class="swatch" style:background={row.color}></span>{row.host}: {row.text}</div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .compare {
    position: relative;
    flex: 1;
    min-height: 220px;
    overflow: hidden;
  }
  .swatch {
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: 6px;
    border-radius: 2px;
  }
  .compare :global(.u-legend) {
    color: var(--text);
    font-size: 12px;
  }
</style>

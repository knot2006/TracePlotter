<script lang="ts">
  import uPlot from "uplot";
  import "uplot/dist/uPlot.min.css";
  import { popupMenu } from "./ContextMenu.svelte";
  import { captionedImage, copyImage, saveExport, saveImage } from "./exports";
  import { exportName } from "./import";
  import { alignLines, BLEND_RADIUS, latencyAxis, rangeGraph, stretchWidth, tallyStats, type Tally } from "./stats";
  import { dateTime, timeTicks } from "./time";
  import { historyRange, type Trace } from "./trace";

  type Props = {
    /** The targets to compare, each drawn as its own line. */
    traces: Trace[];
    /** Length of the newest stretch shown, 0 for everything. */
    focusSeconds: number;
  };
  let { traces, focusSeconds }: Props = $props();

  // Told apart on the dark background, and none of them the green, yellow or red that
  // grade latency elsewhere.
  const COLORS = ["#60a5fa", "#f59e0b", "#c084fc", "#2dd4bf", "#f472b6", "#a3e635", "#fb923c", "#e2e8f0"];
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
      ...traces.map((trace, i) => ({ text: `■ ${trace.config.host}   `, color: COLORS[i % COLORS.length] })),
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
        return { color: COLORS[i % COLORS.length], host: trace.config.host, text };
      }),
    };
  }

  // Room under the plot for its legend: the hosts and their colours.
  const LEGEND_HEIGHT = 48;
  const plotHeight = () => Math.max(100, chartEl.clientHeight - LEGEND_HEIGHT);

  $effect(() => {
    const axis = { stroke: "#9aa4b2", grid: { stroke: "rgba(255, 255, 255, 0.08)" }, ticks: { stroke: "#232a36" } };
    const chart = new uPlot(
      {
        width: chartEl.clientWidth,
        height: plotHeight(),
        cursor: { drag: { x: false, y: false } },
        // The numbers at the cursor are in the readout, so the legend only names the lines.
        legend: { live: false },
        hooks: { setCursor: [readout] },
        scales: { y: { range: (_u, min, max) => latencyAxis(min, max) } },
        axes: [
          { ...axis, values: (_u, ticks) => timeTicks(ticks) },
          { ...axis, label: "Round trip (ms)", labelSize: 20, size: 44 },
        ],
        series: [
          {},
          ...traces.map((trace, i) => ({
            label: trace.config.host,
            stroke: COLORS[i % COLORS.length],
            width: 1.5,
            points: { show: false },
            paths: uPlot.paths.stepped!({ align: 1 }),
          })),
        ],
      },
      [[], ...traces.map(() => [])],
      chartEl,
    );
    plot = chart;
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
      const lines = trips.map((stretches, i) => {
        const several = stretches.length && stretches.reduce((n, s) => n + s.sent, 0) > 1.5 * stretches.length;
        const interval = targets[i].config.intervalMs / 1000;
        const { data } = rangeGraph(stretches, width, width, several ? BLEND_RADIUS : 0, interval);
        return new Map(data.xs.map((x, i) => [x, data.ys[i]]));
      });
      const { xs, ys } = alignLines(lines);
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
  role="img"
  aria-label="Round-trip latency of {traces.map((t) => t.config.host).join(', ')}"
  onmouseleave={() => (tip = null)}
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

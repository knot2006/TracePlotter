<script lang="ts">
  import { settings } from "./settings";
  import { lowerBound, stats } from "./stats";
  import { protocolLabel, type Trace } from "./trace";

  type Props = {
    traces: Trace[];
    /** Changes whenever a trace (plain objects, mutated in place) gains data. */
    rev: number;
    /** Length of the newest stretch the numbers cover, 0 for everything. */
    focusSeconds: number;
    /** The user picked a target to look at. */
    onopen: (trace: Trace) => void;
    /** The user asked to pause a running trace or resume a paused one. */
    ontoggle: (trace: Trace) => void;
  };
  let { traces, rev, focusSeconds, onopen, ontoggle }: Props = $props();

  const SPARK_WIDTH = 160;
  const SPARK_HEIGHT = 24;

  /**
   * SVG path of the latency trend: the worst reply of each of up to SPARK_WIDTH equal
   * slices, scaled to the box. Slices where every probe was lost leave a gap.
   */
  function sparkline(rtts: (number | null)[]) {
    const slices = Math.min(SPARK_WIDTH, rtts.length);
    const worst: (number | null)[] = [];
    for (let i = 0; i < slices; i++) {
      const slice = rtts.slice(Math.floor((i * rtts.length) / slices), Math.floor(((i + 1) * rtts.length) / slices));
      const replies = slice.filter((rtt) => rtt !== null);
      worst.push(replies.length ? Math.max(...replies) : null);
    }
    const top = Math.max(1, ...worst.map((value) => value ?? 0));
    let path = "";
    let drawing = false;
    worst.forEach((value, i) => {
      if (value === null) {
        drawing = false;
        return;
      }
      const x = slices > 1 ? (i / (slices - 1)) * SPARK_WIDTH : 0;
      const y = SPARK_HEIGHT - 1 - (value / top) * (SPARK_HEIGHT - 2);
      path += `${drawing ? "L" : "M"}${x.toFixed(1)} ${y.toFixed(1)}`;
      drawing = true;
    });
    return path;
  }

  const ms = (v: number | null) => (v === null ? "–" : v.toFixed(1));
  const grade = (avg: number | null) =>
    avg === null ? "none" : avg >= settings.badMs ? "bad" : avg >= settings.warnMs ? "warn" : "good";

  const rows = $derived.by(() => {
    rev;
    return traces.map((trace) => {
      const { times, rtts } = trace.target;
      const from = focusSeconds ? lowerBound(times, (times.at(-1) ?? 0) - focusSeconds) : 0;
      const recent = rtts.slice(from);
      const state = trace.alerting ? "alerting" : trace.paused ? "paused" : "running";
      return { trace, s: stats(recent), spark: sparkline(recent), state };
    });
  });
</script>

<div class="summary">
  <table>
    <thead>
      <tr>
        <th></th>
        <th class="text">Target</th>
        <th class="text">IP</th>
        <th class="text">Probe</th>
        <th>Hops</th>
        <th>Loss</th>
        <th>Sent</th>
        <th>Last</th>
        <th>Avg</th>
        <th>Min</th>
        <th>Max</th>
        <th>Jitter</th>
        <th class="text">Round-trip trend</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as { trace, s, spark, state } (trace.id)}
        {@const { host, protocol, port } = trace.config}
        <tr>
          <td class="toggle">
            <button
              class="go"
              class:running={!trace.paused}
              aria-label="{trace.paused ? 'Resume' : 'Pause'} the trace to {host}"
              title={trace.paused ? "Resume trace" : "Pause trace"}
              onclick={() => ontoggle(trace)}>{trace.paused ? "▶" : "⏸"}</button
            >
          </td>
          <td class="text">
            <span class="dot {state}" role="img" aria-label={state}></span>
            <button onclick={() => onopen(trace)}>{host}</button>
          </td>
          <td class="text">{trace.addr}</td>
          <td class="text">{protocolLabel({ protocol, port })}</td>
          <td>{trace.pathEnd || "–"}</td>
          <td class:warn={s.lossPct > 0} class:bad={s.lossPct >= 10}>{s.lossPct.toFixed(1)}%</td>
          <td>{s.sent}</td>
          <td>{ms(s.last)}</td>
          <td class={grade(s.avg)}>{ms(s.avg)}</td>
          <td>{ms(s.min)}</td>
          <td>{ms(s.max)}</td>
          <td>{ms(s.jitter)}</td>
          <td class="text">
            <svg width={SPARK_WIDTH} height={SPARK_HEIGHT} viewBox="0 0 {SPARK_WIDTH} {SPARK_HEIGHT}" aria-hidden="true">
              <path d={spark} />
            </svg>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .summary {
    height: 100%;
    overflow: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }
  th,
  td {
    padding: 6px 10px;
    text-align: right;
    white-space: nowrap;
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
  .text {
    text-align: left;
  }
  tbody tr {
    position: relative;
    border-top: 1px solid var(--line);
  }
  tbody tr:hover {
    background: var(--panel);
  }
  /* The target's name stretches over its row, so the whole row opens the tab. */
  td.text button {
    all: unset;
    cursor: pointer;
    font-weight: 600;
  }
  td.text button::after {
    content: "";
    position: absolute;
    inset: 0;
  }
  td.text button:focus-visible::after {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .toggle {
    width: 34px;
    padding-right: 0;
  }
  /* Green to resume, yellow to pause, like the toolbar's button. It sits above the
     row-wide click target, so pressing it does not open the tab. */
  .go {
    position: relative;
    z-index: 1;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 2px solid #0b3d1e;
    border-radius: 50%;
    background: var(--good);
    color: #062611;
    font-size: 11px;
    line-height: 1;
    cursor: pointer;
  }
  .go.running {
    border-color: #4a3703;
    background: var(--warn);
    color: #2b2001;
  }
  .dot {
    display: inline-block;
    width: 9px;
    height: 9px;
    margin-right: 6px;
    border-radius: 50%;
    background: var(--good);
  }
  .dot.paused {
    background: var(--warn);
  }
  .dot.alerting {
    background: var(--bad);
  }
  .warn {
    color: #fbbf24;
  }
  .bad {
    color: #f87171;
  }
  .none {
    color: var(--muted);
  }
  svg {
    display: block;
  }
  path {
    fill: none;
    stroke: var(--text);
    stroke-width: 1.2;
  }
</style>

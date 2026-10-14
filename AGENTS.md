# Notes for AI agents

This file is for coding agents working on TracePlotter. People should start with
[README.md](README.md) and [CONTRIBUTING.md](CONTRIBUTING.md); everything there applies here too.

## The project

TracePlotter is a Tauri 2 desktop app: a continuous visual traceroute and ping monitor. The
interface is Svelte 5 with TypeScript and uPlot graphs, in `src/`. The backend is Rust, in
`src-tauri/`, and probes with the trippy-core engine. It is developed on Windows first, and CI runs
on `windows-latest`, but the code is kept portable: anything Windows-only sits behind
`#[cfg(windows)]` with a fallback for other systems.

The map of the code is under "Where things are" in CONTRIBUTING.md. In short, `src/routes/+page.svelte`
holds the window, tabs, menus and saved session; `src/lib/TraceTab.svelte` one tab; `src/lib/trace.ts`
a trace's data and its link to the backend; `src/lib/stats.ts` and `events.ts` the calculations;
`src/lib/agent.ts` the answers of the MCP server; `src-tauri/src/lib.rs` probing, history, updates
and the other commands; `src-tauri/src/mcp.rs` the MCP server; `src-tauri/src/winicmp.rs` the
Windows ICMP API.

## Checks

Run these before proposing a change, in this order. The backend embeds the built frontend, so
`npm run build` has to come before the Rust tests.

```bash
npm run check
npm test
npm run build
cargo clippy --lib --tests --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

On Linux the Rust build needs the WebKitGTK development packages (`libwebkit2gtk-4.1-dev`,
`libgtk-3-dev`, `librsvg2-dev`).

Do not run the full `npm run tauri build`, nor the ignored `live_trace` test, in a sandbox or cloud
machine: they need Windows, Administrator rights and a real network. Leave those to a person.

## Conventions

- Write code that reads like the code around it. Comments say why, in full sentences; they are as
  dense as the existing ones, not denser.
- Trace data is kept in plain objects changed in place, and the interface redraws when a counter
  (`rev`) changes. Do not move samples into Svelte state.
- Pure logic goes in `src/lib/*.ts` with a `*.test.ts` beside it, run by `node --test`. Imports in
  those files carry the `.ts` extension so that Node can run them.
- trippy-core is a moving target: take its new releases and fix our code when its API changes,
  rather than holding it back.
- A release bump changes the version in `package.json`, `src-tauri/Cargo.toml` and
  `src-tauri/tauri.conf.json` together; they must always match.
- Documentation and pull request descriptions are plain prose, in the voice of the existing docs.

## Demo mode

`--demo` replaces probing with a made-up nine-hop route built from documentation addresses, with a
quarter of an hour of history, and keeps its data in a separate `TracePlotter-demo-data` folder.
Use it for screenshots and for interface work:

```bash
npm run tauri dev -- -- --demo
```

## The MCP server

The app has a built-in MCP server so that agents can read its traces. People turn it on under
**Tools > Allow AI agents to connect (MCP server)**; `--mcp` turns it on for one run. It listens on
`http://127.0.0.1:3917/mcp` (Streamable HTTP, plain JSON answers, no event stream) and refuses
requests that carry an `Origin` header or another `Host`, so web pages cannot reach it.

For development, run the demo with the server on and point an agent at it; this repository's
`.mcp.json` already does that for Claude Code:

```bash
npm run tauri dev -- -- --demo --mcp
```

The tools are `list_targets`, `get_hops`, `get_route_changes` and `get_latency_history`, which only
read, and `start_trace` and `stop_trace`, which open or resume a tab and pause one. The Rust side
(`mcp.rs`) speaks the protocol and describes the tools; it passes each call to the webview as an
`mcp-request` event, where `agent.ts` and `+page.svelte` answer from the open tabs with the same
statistics the window shows, and the answer comes back through the `mcp_respond` command. A new
tool needs its description in `tools()` in `mcp.rs` and its answer in `READ_TOOLS` in `agent.ts`
(or in `answerAgent` in `+page.svelte` when it changes tabs).

Many routers throttle their replies when they are probed more than once a second, which then looks
like packet loss. Agents must not probe faster than that: `start_trace` never goes below a
1000 ms interval, and that limit should stay.

# Notes for AI agents

This file is for coding agents working on TracePlotter. People should start with
[README.md](README.md) and [CONTRIBUTING.md](CONTRIBUTING.md); everything there applies here too.

## The project

TracePlotter is a Rust desktop app with an egui interface and egui_plot graphs, in
`src/native/`. The backend probes with trippy-core. It is developed on Windows
first and also runs on Linux; CI runs on both. Keep platform-specific code behind the
appropriate `#[cfg]` with a fallback, and build without warnings on both.

The code map is in CONTRIBUTING.md. `native/mod.rs` owns the window; `native/layout.rs`
and `graphs.rs` draw it; `native/app.rs` runs the controller independently of painting;
`native/model.rs` holds traces, settings and saved sessions. `logic.rs` and `insights.rs`
hold calculations, `lib.rs` history and platform commands, `probe.rs` probing, and
`mcp.rs` the MCP server. `winicmp.rs`, `pingsock.rs` and `echo.rs` handle system ICMP.

## Checks

Run the native checks before proposing a change:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib
cargo native-test
```

On Linux the native build needs `libwayland-dev` and `libxkbcommon-dev`. No npm build,
WebKitGTK or WebView2 is needed.

Do not run the full release packaging or ignored live network tests in a sandbox or cloud
machine. Those need platform checks and a real network; raw sockets may also need extra rights.

When a person asks for an exe to try on Windows, use the `fast` Cargo profile:

```bash
cargo build --profile fast
```

The exe is `target/fast/traceplotter.exe`. Published builds use `release`;
do not hand out a `fast` build as a release.

## Conventions

- Write code that reads like the code around it. Comments say why, in full sentences; they are as
  dense as the existing ones, not denser.
- Trace data belongs to the controller. Rendering reads snapshots; probing and MCP must
  continue when the window is hidden.
- Pure logic and its unit tests live together in Rust modules. Widget tests live in
  `native/tests.rs` behind `ui-tests`.
- Old browser sessions are read from disposable SQLite/LevelDB copies on first launch.
  Keep original data untouched, preserve session IDs, and validate before saving.
- trippy-core is a moving target: take its new releases and fix our code when its API changes,
  rather than holding it back.
- The version is in `Cargo.toml` only.
- Documentation and pull request descriptions are plain prose, in the voice of the existing docs.

## Rust guidance and skills

Use the project-local [BeautifulRust skills](.agents/skills/README.md):

- Read [rust-best-practices](.agents/skills/rust-best-practices/SKILL.md) when writing,
  debugging or refactoring Rust, including Cargo changes, tests and rustdoc.
- For a Rust review, also read [rust-review](.agents/skills/rust-review/SKILL.md).
  Keep findings within the requested scope, cite locations and rules, and suggest
  the smallest useful fixes.
- When porting another language to Rust, also read
  [rust-port](.agents/skills/rust-port/SKILL.md). Preserve observable behavior and
  test it; extend the existing PORTING.md when porting notes belong there.

These skills supplement this file and CONTRIBUTING.md; the project instructions take
precedence. TracePlotter is an application even though it has a lib.rs. Keep concrete
types and the existing module layout, and keep pure logic tests beside their modules.
Cloning display snapshots is intentional: the controller owns the live data. Preserve
the documented trippy-core unwind-based stop mechanism and release panic strategy.
Use the native checks above rather than replacing them with a skill's generic commands.
Adopt additional lints incrementally, with a clean build on Windows and Linux, rather
than enabling the full upstream library lint set across the application.

## Demo mode

`--demo` replaces probing with a made-up nine-hop route built from documentation addresses, with a
quarter of an hour of history, and keeps its data in a separate `TracePlotter-demo-data` folder.
Each host name gets its own latency and loss. `--demo-days N` starts new tabs with N days of
history instead, for work on long periods.
Use it for screenshots and for interface work:

```bash
cargo native -- --demo
```

### Opening the Windows UI for the user

When asked to run or show the app, launch the built exe outside the execution sandbox
using the tool's `require_escalated` permission mode. In this environment a sandboxed
process can keep recording samples without showing its window on the user's desktop.
This is a desktop visibility requirement, not a request to run TracePlotter as Administrator.
For an interactive demo, use a visible window explicitly:

```powershell
Start-Process -FilePath (Resolve-Path 'target/fast/traceplotter.exe').Path -ArgumentList '--demo' -WindowStyle Normal -PassThru
```

Keep builds and headless tests in the sandbox. Preserve any requested demo switches and
use a separate `--instance <UUID>` when the demo needs isolated settings and history.
Do not report that the UI is open merely because a process started or history is updating.
Check that the launched process has a nonzero `MainWindowHandle`, a window title and
`Responding = True`; inspect the window with the computer-use tools when available.
If visual inspection is unavailable, say so. When retrying a hidden launch, stop only
the process started for that attempt and relaunch it outside the sandbox.

## The MCP server

The app has a built-in MCP server so that agents can read its traces. People turn it on under
**Tools > AI agents (MCP server) > Allow agents to connect**; `--mcp` turns it on for one run. It listens on
`http://127.0.0.1:3917/mcp` (Streamable HTTP, plain JSON answers, no event stream) and refuses
requests that carry an `Origin` header or another `Host`, so web pages cannot reach it.

For development, run the demo with the server on and point an agent at it; this repository's
`.mcp.json` already does that for Claude Code:

```bash
cargo native -- --demo --mcp
```

The tools are `list_targets`, `get_hops`, `get_route_changes` and `get_latency_history`, which only
read, and `start_trace` and `stop_trace`, which open or resume a tab and pause one.
The Rust side (`mcp.rs`) speaks the protocol and describes the tools. Each call becomes a
controller command; `native/agent.rs` answers from the same trace model the window shows.
A new tool needs its description in `tools()` in `mcp.rs` and its controller answer in
`native/agent.rs` or `native/app.rs` when it changes tabs.

Many routers throttle their replies when they are probed more than once a second, which then looks
like packet loss. Agents must not probe faster than that: `start_trace` never goes below a
1000 ms interval, and that limit should stay.

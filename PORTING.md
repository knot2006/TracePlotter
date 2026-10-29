# Native GUI port

The egui interface is an opt-in preview. Normal builds include both interfaces and start
with the classic Svelte/Tauri interface unless a different choice has been saved in Settings.
This is the implementation record for
[the migration plan](docs/egui-port-plan.md). Platform acceptance and cutover are still open.

## Run and verify

From the repository root, with stable Rust 1.95 or newer:

```bash
cargo native -- --demo
cargo native-test
cargo clippy --lib --tests --no-default-features --features egui-ui,ui-tests --manifest-path src-tauri/Cargo.toml -- -D warnings
```

`cargo native` is an alias for `cargo run --manifest-path src-tauri/Cargo.toml
--no-default-features --features egui-ui`. It needs no npm build, Tauri runtime or WebKit.
Add `--mcp` for agent tools and `--demo-days 2` for a longer synthetic history. Keep
`--demo` when using either interface for QA; data then goes into the separate demo folder.
Agent requests still use intervals of at least 1000 ms. Manual intervals remain separate.

In a normal build, **Settings > Interface > Switch to native GUI and restart** transfers
the current browser session directly and remembers the choice in `interface.json` in the data
folder. The native GUI offers the return switch under **Settings > Display > Interface**.
Returning restores the native session to browser storage before the page loads, keeping a
`traceplotter.classicBeforeNative` browser-storage backup. Session IDs and history files stay
unchanged. Restart arguments preserve demo and MCP flags and wait for the old process to exit.
`--classic` and `--egui` override the saved choice for one run; changing the setting persists it.
Single-interface builds disable the unavailable choice and keep their existing startup path.

To import a released Tauri installation's browser state, build the transition feature:

```bash
npm run build
cargo run --features legacy-migration --manifest-path src-tauri/Cargo.toml -- --egui --demo
```

Omit `--demo` to use real saved sessions and real probing. Close the old interface first.
The helper opens the original production origin and browser profile in a separate process,
reads only saved settings and sessions, writes validated `native-state.json`, and exits.
It starts no probes or MCP server. Browser storage and the JSONL history format are kept.
A development-server origin is different from the installed app's origin; an unmatched
profile is reported rather than converted to an empty session.

Native-only builds refuse migration when browser data exists. A corrupt native session
also produces an error instead of resetting it. Each successful session replacement keeps
`native-state.json.bak`. Keep the migration feature in any eventual transition release
until direct upgrades from supported Tauri versions no longer need it.

The default legacy check sequence remains:

```bash
npm run check
npm test
npm run build
cargo clippy --lib --tests --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

CI also checks the native feature set on Windows and Linux, the transition feature, and
the optional live-edit hook. GPU image tests are separate from the normal test suite:

```bash
cargo native-test native::tests::render_native_scenarios -- --ignored
```

This selects only the synthetic UI rendering test, not the ignored live-network tests.
It writes PNGs and a PDF to the system temporary folder, or `TRACEPLOTTER_QA_IMAGES`.
It needs a working GPU or software Vulkan adapter.

## Implementation

| Area | Native implementation | Status |
| --- | --- | --- |
| Probe workers, recorder, network recovery | `src-tauri/src/probe.rs`, existing OS modules | Shared with the legacy adapter; Tauri channels replaced by an event sink |
| History, paths, signatures and platform operations | Existing shared functions in `lib.rs` | Used by both interfaces; no duplicate probe or JSONL implementation |
| Sample application, route tracking and alerts | `native/model.rs` | Independent of paint; replay suppresses notifications and event writes |
| Sessions and layouts | `native/model.rs`, `native/migration.rs` | Versioned JSON, validation, atomic replacement and backup; stable session IDs |
| Controller and prepared views | `native/app.rs` | Typed commands, bounded probe queue, background startup/replay, DNS and range queries |
| Statistics and event/WHOIS parsing | `logic.rs` | Golden comparisons against the original TypeScript |
| MCP answers and transport | `native/agent.rs`, shared `mcp.rs` | Same six tools and loopback restrictions; tools work without UI paints |
| Toolbar, tabs, columns, settings and dialogs | `native/mod.rs`, `native/layout.rs` | Native egui controls, confirmations, notifications, WHOIS and update/elevation actions |
| Plots, comparison and navigation | `native/graphs.rs` | Owned time bounds, linked cursors/axes, split gaps, min/max areas, jitter and mapped loss |
| Import, time and exports | `native/import.rs`, `time.rs`, `export.rs` | CSV import, streaming raw samples, visible-column text, actual tally CSV, PNG/clipboard and paginated PDF |
| 0.1.13 incidents and statistics | `logic.rs`, shared `insights.rs`, `native/report.rs` | Median, p95, p99 and loss-burst columns, loss explanations, incidents, correlated incidents and the HTML support report; golden comparisons against the TypeScript |
| Background monitoring | `native/tray.rs` | Tray icon, close and minimize to tray, Hide to tray, Exit and stop monitoring, start at Windows sign-in |

Statistics and graph preparation run in the controller, not in the paint callback. Views
contain summaries and bounded series, not a clone of each tab's raw day on every frame.
History replay and reads run on workers; asynchronous results carry session and run identity.
Only one range read per target is pending at once. Superseded results cannot be displayed
for another generation or range. Older disk views reuse prepared results over short live
updates; their aggregate refresh can lag live samples by roughly twenty seconds. The
normal in-memory view updates on each sample.

A cancelled startup sees a shared cancellation flag before starting a worker. Stale events
are ignored. Erasing a closed session waits for a known recorder worker to finish; a tab
closed during startup is cleaned after that startup returns. Resuming waits off the
controller for the old worker. A slow ICMP round can therefore delay resume until it ends.
The intentional trippy unwind cancellation remains unchanged.

UI commands have a separate forwarding queue so probe backpressure cannot block a widget
callback. Raw probe events are retained and backpressured. Snapshot repaint requests coalesce
through egui. Alerts, recording and MCP do not depend on those requests being painted.

Window placement uses the existing physical-pixel `window.json`. Native Windows builds
embed the application icon and manifest. Version resources derive from Cargo.toml. No
second application version was added to npm or Tauri configuration.

## Behavior inventory and tests

The toolbar opens independent target sessions, including repeated hosts. Target controls
pause/resume, edit probe and alert settings, restart, and close with history-erasure
confirmation. All targets provides comparison selection, summary statistics and trends.
Settings covers probe defaults, alert windows, colours, retention, notifications, MCP and
update checks. Columns can be hidden, reordered and resized; table height is remembered.
Several selected hops can be graphed, and latency bars can be zoomed independently.

File actions include import, visible-column text/copy, table CSV, raw sample CSV, comparison
stretch CSV, graph PNG/copy and PDF. PNG/PDF exports render outside the window and include
scrolled-out selected graphs and report rows. PDF tables wrap names and paginate. Comparison
CSV uses `time,host,seconds,sent,lost,avg_ms,min_ms,max_ms,jitter_ms`; raw samples retain
`time,hop,ip,rtt_ms` and stream from the original day files.

Native text uses installed Segoe UI or DejaVu Sans, with an installed CJK fallback when
available, and egui defaults elsewhere.

Keyboard actions include Ctrl+T (also Ctrl+N), Ctrl+W, Ctrl+P, Ctrl+Shift+P, Ctrl+comma,
Ctrl+O, Ctrl+L and Ctrl+Tab/Shift+Ctrl+Tab. The View menu provides zoom and return to live; the history view also has a Live button. Graph
panning uses drag; Ctrl+wheel zooms while ordinary scrolling reaches stacked graphs in
small windows. Table Ctrl+click selects several hops. Escape closes the probe dialog or
clears a pinned graph time. Standard egui editing handles copy/paste and text input.

| Original source/tests | Verification in the port |
| --- | --- |
| `stats.test.ts` | Stats, MOS, RFC3550 jitter, loss, thinning, axis limits, parsing, tallied ranges, alert windows and breaches: `logic.rs` golden suite |
| `events.test.ts` | Grouping, reversal cancellation and ordering: same golden suite |
| `settings.test.ts` | Defaults, coercion, system ICMP, clamps and offered intervals: same golden suite |
| `whois.test.ts` | Record cleanup and selected facts: same golden suite |
| `agent.test.ts` | 176 captured read-tool results/errors: `native/agent.rs`, numerical JSON representation normalized |
| Page `agentStart` | New `agent-start.test.ts` runs the actual page function with fake probing; verifies descriptions and agent interval floor |
| `import.test.ts` | CSV ordering, loss, malformed rows/header, inferred interval and filenames: `native/import.rs` |
| `time.test.ts` | Regional export names and clock precision: `native/time.rs`; remaining OS locale acceptance below |
| `trace.ts` | Route shortening, replay/live alerts and duplicate sessions: `native/model.rs` |
| Browser frame scheduling | Controller updates without paints, stale-run rejection, bounded snapshots retaining all raw samples: `native/app.rs` |
| Session safety | Corrupt/duplicate IDs, stable migrated IDs, failed replacement and backup: model/migration tests |
| Worker cancellation | Cancelled startup opens no history or resolver; stop exposes completion before erasure: `probe.rs` |
| Widgets and rendering | AccessKit names, pause/live commands, settings and minimum-size layout: `native/tests.rs` |

`scripts/capture-port-fixtures.ts` captures 425 original pure-helper cases.
`scripts/capture-agent-fixtures.ts` captures 176 original MCP read cases. Recreate these
intentionally when changing the shared contract, then review the fixture diff. The logic
comparison allows relative floating-point error of 1e-9; rounded MCP numbers match exactly.
This is behavior coverage, not a claim that every original test was translated verbatim.

## Preserved quirks and explicit differences

- Statistics jitter is the mean difference between consecutive replies; the graph's
  jitter uses RFC3550 smoothing within the visible raw window. Missing replies do not
  reset either estimator. Sparse raw views keep individual probes; only views denser than
  the physical plot width are bucketed and blended.
- Root history timestamps remain milliseconds. Plot times and persisted event times
  remain seconds. MCP times remain UTC ISO strings; latency values are rounded to hundredths of a millisecond.
- Disk minute tallies can extend past a requested boundary, as in the existing backend.
  Imported dates before the Unix epoch still cannot become valid backend history lines.
- egui plots accept independently sampled series, so comparison lines do not need the
  uPlot `alignLines` adapter. They are stepped and break at missing data instead.
- Loss is explicitly mapped into latency coordinates and labelled from 0 to 100 percent;
  an additional egui_plot axis alone would not provide a separate coordinate system.
- FilledArea renders the min/max envelope. A general polygon assumes convex geometry and
  produced diagonal artifacts on real dense history; the rendering check caught this.
- PDF saving replaces the browser/system print dialog. Native reports use raster pages
  so the same Unicode labels and graphs render without a second font/shaping stack.
- Native file dialogs retain local date/time filenames. Polish, Czech and US patterns
  are covered in tests. Other locales use Chrono's locale data and still need OS checks.
- `--mcp` enables access for the current run only; saving other preferences does not
  convert that switch into persistent opt-in.
- The legacy `start_trace` tool accidentally spread a Promise after descriptions became
  async. It returned status without the target details. Both legacy spreads now await the
  description, with a failing-then-passing regression test. Native returns the same details.
- Window recovery currently favors the current monitor; restoring every multi-monitor
  position and DPI combination remains a cutover gate.
- Oversized spans and invalid ports are rejected rather than letting conversion overflow
  panic a worker. Native port errors use a stable application message rather than a Tauri
  deserialization message.
- Restart arguments now retain `--egui` and `--demo-days N` as well as `--demo`/`--mcp`.
- Events are not listed in a strip under the period. The Events button opens a menu with
  the event list and Show events, off by default, which marks route changes, alerts and
  recoveries on the latency graph. The event list stays open while events are inspected.
- The Linux tray uses tray-icon's StatusNotifierItem backend, which runs on its own thread
  without GTK, libappindicator or libxdo. Winit cannot hide a window on Wayland, so there
  Hide to tray, Close to tray and Minimize to tray minimize the window instead. Showing a
  window hidden on Windows or X11 from the tray, and start at sign-in, still need a check
  on those systems.
- Both interfaces keep the tray choices in `background.json` and register start at sign-in
  under the same name, so a transition release keeps them.

## BeautifulRust and Blaze

BeautifulRust's `rust-port`, `rust-best-practices` and `rust-review` skills were installed
from revision `58757ee890da9b3c8d5b785adc7a8d53cce27307` and applied as application guidance.
The new skills become available to a freshly started agent session. Golden tests were
captured before the initial Rust calculation implementation. Review priorities are data
loss, cancellation, stale results, API compatibility and paint-independent state.

Cargo enables incremental checks for unsafe operations, debug macros and TODOs. The
upstream library-only documentation/output rules and blanket cast/bool bans were not
imposed on existing desktop and FFI code. Intentional trippy unwinding is retained.
Normal Clippy warnings remain errors in both feature sets.

Blaze **0.20261027.0 (`49ca152`)** was installed with its own **nightly-2026-10-27**.
Its compiler remains optional. The native build was attempted with isolated output through
`CARGO_TARGET_DIR`; this release rejects `--target-dir`. It first rejected the package's
staticlib output. A temporary rlib-only trial then failed on writable statics reached
through eframe, rfd, standard channels and worker threads. The original crate types were
restored. There is no successful Blaze build or speed comparison to report.

The reviewed `blaze-live` source at revision
`0c0ca43feb462186ece3e46ad40cd1f8ac4b6270` is vendored with its licences and provenance. The
renamed crate was not on crates.io, and fetching the whole compiler as a Git dependency
also made Cargo print errors for upstream's intentionally invalid Cargo fixtures.

`dev-live` is default-off and excluded from normal/release dependency trees. The app starts
Live only in debug demo sessions, polls before drawing, and reports Applied/Declined/Restart.
A restart request queues a session save. This integration compiles through Cargo; actual
Blaze patches in an eframe window remain blocked by the compiler failure above.

For a later compatible Blaze release, run from `src-tauri/`:

```bash
CARGO_TARGET_DIR=/tmp/traceplotter-blaze-target blaze run --no-default-features --features egui-ui,dev-live -- --demo
```

On PowerShell, set `$env:CARGO_TARGET_DIR` to a separate build folder first. Restrict the
first live trial to function bodies in `native/graphs.rs`; restart with Cargo after changes
to state, workers, probing, FFI, history, migration or signature verification. Blaze's borrow
checker is incomplete. Use Cargo for verification and official builds, regardless of whether
a future Blaze build succeeds. Do not enable a global Cargo wrapper.

## Verified locally on 28 October 2026

- Legacy checks pass in the required order, including the new MCP regression.
- Native Clippy and unit/widget tests pass with `egui-ui,ui-tests`.
- Native-only and combined transition builds compile. The optional live hook compiles.
- Synthetic UI images render at 1100×720 and 640×420. PNG and PDF exports were rendered
  and inspected, including min/max areas, mapped loss and wrapped report fields.
- A real hidden Tauri helper migrated synthetic localStorage through its production origin
  and original profile. IDs, pause state, settings, selected tab, span and columns survived.
- A separate ten-target demo with two days of history exercised all six MCP tools,
  Host/Origin rejection, 1000 ms agent floor, and live updates after minimization. CLI MCP
  did not leak into saved settings. The controller became ready in 0.873 s in that run.
  After adding an eleventh target at one-second intervals, a process RSS sample was
  782,176 KiB. This includes history/GPU caches and is not a baseline comparison or a peak.

These measurements used Linux, X11, Rust 1.99.0 and the local wgpu renderer. They do not
establish a performance improvement over the legacy interface.

## Remaining acceptance gates

Do not switch the default feature or publish the native port until these are checked:

- Windows MSVC build and actual Windows UI/probe/elevation/firewall/update runs. A Linux
  Windows cross-check stopped at the missing `x86_64-w64-mingw32-gcc` dependency; the new
  Windows CI job is the automated build path.
- Narrator/AccessKit, keyboard focus through every dialog, Windows IME, regional formats,
  high DPI and multiple monitors, unplugged monitors, Remote Desktop and integrated GPUs.
- Linux Wayland and NVIDIA rendering, X11/Wayland clipboard ownership after the export
  worker exits, packaged desktop integration and capability/AppImage restart flows.
- Real saved profiles on Windows/WebView2 and Linux/WebKit, interrupted migration, long
  shutdown/startup races, and a supported rollback/upgrade matrix.
- Side-by-side cold/warm build, startup, frame time, minimized CPU, peak memory and history
  timings against the legacy interface on named Windows and Linux machines. Tune the older
  range refresh cache from these results.
- Human review of text/CSV/PDF export compatibility, long Unicode names, date boundaries,
  very long history, all-timeout traces and many selected graphs.
- Native Windows ZIP/exe signing, Linux packages and a verified signed update/rollback
  before changing the release workflow. No release, packaging or privileged/live network
  tests have been run in this environment.

Blaze compatibility is an independent optional experiment and is not a release gate.

## Layout pass on 29 October 2026

The native layout now follows the [legacy screenshots](docs/gui-reference/README.md):
tabs above the target toolbar; protocol, pause/restart, interval and time span together;
legacy column widths and row spacing; a period/actions strip; jitter above the latency
plot; and the history slider below the plots. Settings use two or three independent
card columns, falling back to one column in a narrow window. Probe editing is centred
with a modal backdrop and scrolls at the minimum size. Colours and further visual
polish remain for a later pass.

Native widget tests check this ordering at 640×420, 1100×720 and 1440×900, keep the
history slider and modal Close button visible, exercise toolbar start/pause, and verify
the settings columns. The native suite passes 58 tests; the separate synthetic GPU
render/export check also passes. [Updated captures](docs/gui-reference/egui/README.md)
include a real native demo window and renders of the settings, comparison and dialog.
The default frontend checks and legacy backend checks still pass. Windows and Wayland
acceptance remain open.

Native latency graphs use held steps for individual probes and the min/max envelope,
matching the original uPlot paths. A white latency line and a clearer stepped range
band keep short spikes visible; dense views still use the original averages and
preserve each bucket's real maximum. Regression tests cover a single-probe spike,
missing replies and maxima after thinning. Tab close controls use an unfilled X.

The stepped latency and jitter lines explicitly disable egui_plot's automatic band
aggregation. Equal-x vertical edges otherwise become opaque ribbons, even at ordinary
probe density. A widget regression checks the emitted drawing shapes for both hop and
comparison views; it fails with automatic bands enabled and passes with thin paths.

Hover readouts float beside the pointer, with time, latency and jitter on separate
lines and loss when present. Longer periods include the date; dense points include
their min/max range. Tooltips flip at window edges, hide during dragging or context
menus, and retain a pinned value. They consume no space below the graphs. Widget checks
cover four window sizes and the synthetic GPU captures include a hover view.

Right-click menus cover latency/jitter graphs, comparison graphs, full hop rows,
column headers and target tabs. They reuse the existing export, clipboard, WHOIS,
column preferences and trace commands. A graph export from its context menu includes
only that graph; closing a graph keeps the other selections. Comparison exports retain
their overlaid view. Tests exercise menu hit areas, copy, column move/reset, pause,
return to live, closing one graph and the comparison loss-colour preference.

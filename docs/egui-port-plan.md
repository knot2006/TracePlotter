# Porting TracePlotter to egui

This plan replaces the Svelte and uPlot interface with an egui desktop interface while
keeping TracePlotter's tracing, history, statistics, exports and agent tools. Windows
remains the first platform; Linux remains a release requirement. The current layout,
dark theme and keyboard shortcuts are the starting point.

The plan includes [Blaze](https://github.com/indexable-inc/blaze) for faster development
builds and optional live edits, and
[BeautifulRust](https://github.com/morganlinton/BeautifulRust) for porting, review and
lint guidance. Neither replaces verification with the project's Rust toolchain.
The native preview and tool integration are now implemented. [PORTING.md](../PORTING.md)
records the code, verification, known differences and acceptance gates. The legacy GUI
stays the default until those gates pass.

## Execution status

The runnable preview covers the shared controller, calculation port, native trace and
comparison views, session migration, exports and MCP/OS adapters. Linux synthetic checks
and the real migration helper pass. [PORTING.md](../PORTING.md) contains the test mapping,
601 captured parity cases, commands and measured results.

The checkboxes below retain the full acceptance scope. Windows runtime checks, Wayland,
accessibility, performance comparisons, packaging and signed-update runs still gate cutover.
The legacy GUI and published release workflow remain the default. Blaze was installed and
tried; compiler limitations currently block its optional build/live-edit experiment.

## Decisions

Use `eframe`, `egui`, `egui_extras` and `egui_plot`. The versions inspected on
28 October 2026 are egui/eframe 0.37 and egui_plot 0.38. Match the egui version used
by every companion crate and commit the resolved Cargo.lock. Set an explicit minimum
Rust version after checking the selected crates' manifests. Keep trippy-core's existing
policy of taking new releases and adapting to API changes.

Keep the existing Cargo package and the version in `src-tauri/Cargo.toml` during the
port. Split its large `lib.rs` into modules as each part is needed. A workspace split
or directory rename is unnecessary for the first native release.

Introduce additive `egui-ui`, `legacy-ui` and `legacy-migration` features. The existing
interface stays the default until the native interface passes the parity gates. The
entry point chooses one interface for a process. Make Tauri, its plugins and build
integration optional, so a native development build can compile without them.

Use ordinary egui tabs and resizable panels initially. The existing hop table fits
`egui_extras::TableBuilder`; docking and an additional table framework are unnecessary.
Use `rfd` for file dialogs and `arboard` for graph images on the clipboard where the
eframe integration does not cover the operation. Evaluate a PDF writer in the export
milestone. Reuse existing Rust networking, serialization and update dependencies.

## Behavior to preserve

- Several targets, including separate sessions for repeated instances of the same host;
  the All targets, target and Settings tabs; remembered tab selection.
- ICMP through the Windows API and Linux ping sockets, raw ICMP, UDP and TCP, IPv4/IPv6,
  existing probe limits, retries, network-change recovery and platform privilege flows.
- Pause/resume, probe settings, alert thresholds, route-change events, DNS and WHOIS.
- Hop selection, column visibility/order/widths, the resizable latency bar and table,
  loss, sent/received, last/average/minimum/maximum latency, jitter and MOS.
- Several hop graphs, the jitter strip, latency bands, loss bars, linked cursors,
  pinned readouts, pan/zoom, the history slider and return to live; target comparison.
- History retention of 1 to 365 days, newest in-memory history of 24 hours with the
  existing 345,600-round cap, older-range aggregation, and saved route/alert events.
- Text, CSV, graph PNG, clipboard images and PDF exports; CSV import as a paused tab.
- Portable Windows data, Linux data paths, window placement, session restoration,
  signed Windows updates and Linux links to releases.
- The six existing MCP tools, their response fields, validation and error behavior;
  opt-in loopback access, Origin/Host checks and configurable port.
- `--demo`, `--demo-days N` and `--mcp`, including separate demo data and the visible
  demo label. An agent's `start_trace` retains its 1000 ms minimum interval. Preserve
  the current manual interval choices separately.

Closing a target or restarting its trace still requires confirmation before erasing
history. Replaying history must not send notifications or append the same events again.

## Architecture

The runtime owns traces independently of the window. Probe workers send typed events
to a controller; UI actions and MCP calls send commands to that same controller. The
controller applies samples, manages sessions and evaluates alerts even when the window
is minimized. Rendering never drives probing or acts as the MCP request dispatcher.

Use the existing thread-based approach and standard channels before adding an async
runtime. Keep one authoritative trace model. Publish small summaries and cached graph
views to egui; do not clone a day's samples every frame. A short-held lock around a
published snapshot is sufficient; it must not cover file/network I/O or drawing.

History reads, DNS, WHOIS, imports, exports and update downloads run outside the render
thread. Tag asynchronous results with session/run identity and request generation so a
closed tab, changed address or newer graph request cannot receive an obsolete result.
Retain every probe for recording and statistics even when screen updates are coalesced.
Bound pending work and cancel superseded range queries; do not silently discard samples
to keep the UI responsive.

The intended module mapping is:

| Current source | Rust destination inside `src-tauri/src/` | Responsibility |
| --- | --- | --- |
| Probing and recorder in `lib.rs`, `echo.rs`, `winicmp.rs`, `pingsock.rs` | `probe.rs` and existing OS modules | Existing probe engine and typed results |
| History and range reads in `lib.rs` | `history.rs` | Existing JSONL files, retention and aggregation |
| `trace.ts` and tab lifecycle in `+page.svelte` | `trace.rs`, `app.rs` | Trace model, commands, notifications and sessions |
| `stats.ts`, `events.ts` | `stats.rs`, `events.rs` | Statistics, graph data, alert and route logic |
| `settings.ts`, browser storage in the Svelte components | `settings.rs`, `session.rs` | Validation and versioned native state |
| `time.ts`, `whois.ts`, `import.ts` | `time.rs`, `whois.rs`, `import.rs` | Formatting, record parsing and CSV import |
| `agent.ts`, `answerAgent`, `mcp.rs` | `agent.rs`, `mcp.rs` | Tool answers and transport over the common model |
| `+page.svelte`, `TraceTab.svelte`, `Summary.svelte`, `Settings.svelte` | `ui/` modules | Window, menus, tabs, table, settings and dialogs |
| `HopGraph.svelte`, `CompareGraph.svelte` | `ui/graphs.rs` | Plot painting, navigation and readouts |
| `exports.ts`, component export methods | `export.rs` | Text/CSV, report layout and image/PDF rendering |
| Platform and update commands in `lib.rs` | `platform.rs`, `updates.rs` | Dialogs, capabilities, restart and signed updates |

Keep these concrete application modules. Add a trait or another crate only when an
actual caller requires it. The Tauri adapter and egui entry point both call the same
extracted probe/history functions while the interfaces coexist.

## Milestone 0: capture the existing contract

- [x] Run the repository's checks in the required order and record the results below.
- [ ] Inventory commands, menus, shortcuts, persisted keys, exports and MCP examples.
- [ ] Capture demo screenshots at the default 1100 by 720 size and the 640 by 420
  minimum, with multiple tabs, comparison graphs, settings and dialogs.
- [ ] Record start time, working memory, visible frame times, minimized CPU and history
  query times on named Windows and Linux machines using the existing app.
- [ ] Establish identical demo scenarios: one target, ten targets, long-history ranges,
  loss, all-timeout periods, pauses, route changes and an imported capture.
- [ ] Characterize untested sample application and tab lifecycle behavior before
  moving that logic; carry those cases into the Rust test mapping.
- [ ] Start `PORTING.md` with API and test mappings, preserved quirks, deliberate
  differences and verification results. Record external tool/skill revisions.

Exit: the current app supplies a reproducible behavior and performance baseline.

## Milestone 1: extract the Rust runtime

- [ ] Move probing, history, data paths and update verification into the modules above
  without changing the on-disk format or probe algorithms.
- [ ] Replace Tauri channels and `AppHandle` in the shared logic with typed messages;
  leave conversion to IPC in the legacy adapter.
- [ ] Define the runtime command/event boundary and exercise the extracted probe and
  recorder with demo input. Move the TypeScript model logic in the next milestone,
  after its characterization tests exist.
- [ ] Make shutdown, pause/resume and restart explicit. Preserve the existing trippy
  unwind mechanism and its reason; a general Rust style rule must not remove it.
- [ ] Keep recording independent of paint. Test worker failure, cancellation and
  shutdown with made-up data and temporary directories.

Exit: the legacy interface still works, and the common runtime can run without a webview.

## Milestone 2: port the logic and tests

Apply BeautifulRust's tests-first porting workflow. Translate each existing pure-logic
test before its implementation, keeping a mapping from the original test name. Add
characterization cases for untested behavior in `trace.ts` and the page controller.

- [ ] Port `stats.test.ts`, `events.test.ts`, `settings.test.ts`, `import.test.ts`,
  `time.test.ts`, `whois.test.ts` and `agent.test.ts` as their modules move.
- [ ] Port sample application, route-end tracking, bounded sample retention, DNS
  caching, alerts and event recording from `trace.ts` against their characterization
  tests. Alert evaluation must continue without paints.
- [ ] Replace the browser-specific `frame.test.ts` with tests of coalesced redraw
  requests; the browser scheduling implementation itself has no native role.
- [ ] Export deterministic fixtures from the TypeScript implementation and compare
  Rust results on the same inputs. Match ordering, nulls, limits and error cases;
  document justified floating-point tolerances and display-format differences.
- [ ] Cover zero replies, mixed loss, pauses, changing route lengths, IPv6, malformed
  CSV, date boundaries, settings coercion/clamps and alert windows at each interval.
- [ ] Preserve the distinction between history timestamps in milliseconds and graph
  timestamps in seconds. Use named conversions at the boundary.
- [ ] Keep existing jitter, MOS, loss buckets, thinning weights and gap semantics.
  Preserve bounded event history and notification throttling.

Exit: every mapped test passes and the shared model produces the same statistics and
MCP data as the existing app. Behavior changes are explicit in `PORTING.md`.

## Milestone 3: build one complete egui trace view

- [ ] Add the feature-gated eframe entry point, theme, fonts and window icon.
- [ ] Build the toolbar, target tabs, one hop table and two linked plots using demo data.
- [ ] Move live probe events through the runtime into that view without blocking input.
- [ ] Preserve number alignment, column sizes, density, labels, focus indicators and
  keyboard operation. Show unavailable values distinctly from zero.
- [ ] Implement return to live, pause, zoom and range navigation with buttons/keyboard
  equivalents as well as pointer gestures. Values remain available without hover.
- [ ] Capture screenshot and interaction tests with `egui_kittest`. Check Windows
  Narrator/AccessKit behavior and Linux keyboard input on the chosen eframe build.
- [ ] Check renderer startup on integrated graphics, Windows Remote Desktop, and Linux
  X11/Wayland including NVIDIA. Choose eframe's renderer/fallback policy from these
  results and give a useful error when no supported graphics adapter is available.

Use egui_plot's linked X axes/cursors, but explicitly own the displayed time bounds and
axis gutters. Upstream has open reports about linked plots with different data extents
and label widths. Test those cases rather than assuming linking handles layout.

For milliseconds and loss percentages on one plot, map the loss series into plot
coordinates and format a separate right axis/readout in percent. egui_plot's extra
axis labels are not independent coordinate systems. The mapping must remain correct
when latency limits change and must always label loss from 0 to 100 percent.

Exit: the core table/graph interaction works on Windows and Linux, with no daily-history
copy or full statistics recalculation in each frame. This is the first checkpoint for
measuring the chosen stack before expanding the UI.

## Milestone 4: migrate saved sessions safely

Browser `localStorage` is the main upgrade risk. Rust cannot obtain all of these values
from the existing history files. Inventory and migrate these keys:

`traceplotter.session`, `traceplotter.settings`, `traceplotter.activeTab`,
`traceplotter.hiddenColumns`, `traceplotter.columnOrder`, `traceplotter.columnWidths`,
`traceplotter.tableHeight`, `traceplotter.timeSpan`, `traceplotter.compare`,
`traceplotter.checkUpdatesAtStart` and `traceplotter.mcpServer`.

- [ ] Define a versioned native state document and write it with platform-correct atomic
  replacement. Preserve stable session IDs and the existing JSONL history layout.
- [ ] Add a legacy export bridge that reads these keys from the existing webview and
  writes a validated migration document through a narrow backend command.
- [ ] Support direct upgrades from versions that never contained the export bridge.
  For the first native releases, retain a migration-only legacy mode: the native
  startup invokes the same executable in a separate migration process, using the old
  webview origin and data directory to export storage, then loads the native state.
  The helper only exports state; it starts neither probes nor the MCP server.
  This avoids parsing browser databases or assuming everyone installed an intermediate
  release. Confirm the mechanism on WebView2 and WebKitGTK before cutover.
- [ ] Keep migration idempotent. Mark it complete only after validation and durable
  native-state writing; retain the old storage and a backup. A failure reports recovery
  steps and must not overwrite a real session with empty defaults.
- [ ] Keep demo and real migration completely separate. Compare interfaces using copies
  of demo data so two runtimes never write the same history during testing.
- [ ] Test absent/corrupt storage, duplicate hosts, paused traces, imported sessions,
  interrupted migration, repeat migration and direct upgrade from the current release.

Exit: an existing user's targets, history, settings and layout open correctly in egui.
An untouched copy of the previous data can still be opened by the previous release.
Normal changes made after migration need not be synchronized back to the old browser
state; document that rollback boundary.

The first native release may still carry Tauri solely for migration compatibility.
Retiring that helper is a separate gate: keep a supported direct-upgrade route before
removing its runtime dependencies. On Linux, retain the helper's library dependencies
in packages for as long as that migration path needs them.

## Milestone 5: complete interface and feature parity

- [ ] Finish the All targets view, comparison selection, sparklines and target graphs.
- [ ] Finish column reordering/resizing/hiding, the latency bar, multiple selected hops,
  graph removal, table resizing and remembered layout.
- [ ] Implement jitter strips, latency bands, loss bars, min/max envelopes, pause gaps,
  day-aware time ticks, pinned readouts and captioned graph images.
- [ ] Keep the existing time-bucket aggregation and smoothing rules. Dense-line banding
  in egui_plot is a rendering optimization, not a substitute for history semantics.
- [ ] Cache graph preparation by session/run, data revision, time range and physical
  plot width. Invalidate on relevant settings changes and discard obsolete results.
- [ ] Finish settings, probe/alert dialogs, context menus, WHOIS display, notifications,
  toasts, errors, update controls and destructive-action confirmations.
- [ ] Preserve all shortcuts, menu actions and sensible focus after opening/closing
  tabs or dialogs. Include per-monitor scaling and off-screen-window recovery.
- [ ] Port text/CSV export and CSV import with the existing filenames and data format.
- [ ] Implement PNG save/copy at physical pixel resolution. Build a reusable report
  layout for table plus graphs and use it for PDF. PDF replaces browser printing;
  direct system printing is a separate platform integration if parity testing needs it.
- [ ] Check local date/time and filename formatting against Windows regional settings
  and Linux locales; do not silently replace the current host format with US English.

Exit: every item in the behavior inventory has an implementation and a verified result.

## Milestone 6: move MCP and OS integration off Tauri

- [ ] Have `mcp.rs` call the common command/query path instead of emitting to a webview.
  Port the four read-tool answers and the start/stop actions against the same model.
- [ ] Preserve tool schemas, IDs, ordering, timestamp units, history slices, clamping,
  errors and the 1000 ms agent interval limit. Exercise all six tools while minimized.
- [ ] Preserve request timeouts, enable/disable behavior, port-conflict errors, loopback
  binding and Origin/Host rejection. Test these using local synthetic data.
- [ ] Replace Tauri notification, dialog, window, restart and browser-opening calls with
  native equivalents. Verify Windows image copy and Linux Wayland/X11 clipboard paths.
- [ ] Preserve Windows elevation/firewall commands and Linux capability/AppImage
  restrictions. Keep OS-specific code behind its existing cfg boundaries.
- [ ] Preserve the signed version check, download size limits, replace/rollback logic,
  predecessor wait and window/session saving before update/elevation restarts.

Exit: egui handles normal operation without Tauri calls. The optional legacy migration
process is the only remaining webview consumer in a transition release.

## Blaze development workflow

Introduce Blaze after the first native demo view runs through Cargo. Pin a tested Blaze
release or revision in the development notes and record its own compiler version.
Upstream uses its own nightly even when the project pins another toolchain. Stable
Cargo/rustc checks remain the source of truth for borrowing, correctness and releases.

- [ ] Start with `blaze build` and `blaze run` on the native feature set. Verify the
  build actually reports Blaze use; unsupported code can fail rather than fall back.
  Document an explicit switch back to Cargo.
- [ ] Keep Blaze's target output separate from Cargo's verification and release output.
  Do not change global Cargo configuration to enable it.
- [ ] Compare cold build, unchanged build, a function edit, a widget edit and a struct
  edit against Cargo on the same checkout. Record daemon memory as well as elapsed time.
- [ ] Evaluate `blaze-live` behind a default-off `dev-live` feature. Enable runtime
  patching only for debug demo sessions; official builds exclude this dependency.
- [ ] Start `Live` once and poll at a frame boundary before drawing. Report `Applied`,
  `Declined` and `Restart` outcomes. Save demo session state before a required restart.
  Request redraws while this development feature polls, without adding a normal-app
  continuous paint loop.
- [ ] Restrict the first live-edit trial to UI function bodies in demo mode. Probe,
  OS/FFI, history, migration and signature-verification edits use a clean Cargo build
  and restart. State/type changes must follow the restart outcome.
- [ ] Verify actual eframe windows, text input, plots, dialogs and worker interaction.
  Blaze's egui harness is headless; it does not prove our window integration works.
- [ ] Treat Windows live edits as experimental until tested locally: upstream documents
  manual Windows validation and incomplete installer/sandbox coverage. A failure of
  this optional development path must not block a verified Cargo build of the port.

The intended commands, once the features exist, are:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --no-default-features --features egui-ui -- --demo --mcp
blaze run --manifest-path src-tauri/Cargo.toml --no-default-features --features egui-ui,dev-live -- --demo --mcp
```

Validate Blaze's flag support and output isolation when introducing the commands.
Use an external file watcher if wanted; upstream does not provide a watch mode.

## BeautifulRust development workflow

Use the three skills together: `rust-port` for behavior/test mappings,
`rust-best-practices` while implementing, and `rust-review` at milestone boundaries.
Install the reviewed revision through the permitted skill-installation workflow when
implementation starts. Keep project conventions and explicit user instructions first.

Port the tests before the implementation and maintain one `PORTING.md`, rather than
separate reports for every small module. Preserve ordering and error behavior. Note
quirks and deliberate differences instead of quietly changing them during translation.
Use `Option`, typed commands and `Result` where appropriate; keep rendering separate
from I/O and keep platform unsafe code confined to the existing OS modules.

Review the supplied `lints/lints.toml` and `lints/clippy.toml` against the application.
Adopt applicable checks incrementally as modules are ported, keeping the existing
warnings-as-errors requirement. Configure lints in Cargo; document narrow, justified
exceptions for framework APIs, FFI conversions and intentional trippy unwinding.
Library-only documentation/output rules need not be imposed on the desktop application.
Do not turn lint adoption into an unrelated rewrite of working probe code.

Each milestone review should identify correctness, data-loss and concurrency defects
first, include source locations and fixes, and record which checks ran. Review the
actual diff and affected callers; a separate review agent is not required by this plan.

## Milestone 7: packaging, CI and cutover

- [ ] Keep the current checks, in their existing order, while the legacy app remains:

  ```bash
  npm run check
  npm test
  npm run build
  cargo clippy --lib --tests --manifest-path src-tauri/Cargo.toml -- -D warnings
  cargo test --lib --manifest-path src-tauri/Cargo.toml
  ```

- [ ] Add `cargo fmt --check`, a native build, native tests and Clippy on Windows and
  Linux for `--no-default-features --features egui-ui`. Check the migration feature
  combination separately. Build UI snapshots in a renderer-capable test environment.
  Keep the nightly/Blaze experiment outside required release checks.
- [ ] Compare Cargo-built native and legacy behavior on the shared fixtures. Validate
  the native demo at normal/minimum sizes, different scaling, multiple monitors,
  minimized/restored state, and 7-day and 30-day history views.
- [ ] On named machines, require responsive pointer interaction with a 16.7 ms p95
  visible frame target at 1080p. Use equivalent optimized profiles, datasets, window
  size and probe cadence for both interfaces. Compare startup, memory and idle/minimized
  CPU with the baseline; investigate regressions before cutover. Memory must settle once
  retention caps are reached and temporary history queries finish.
- [ ] Keep probe/alert/MCP results correct during a long minimized run and while range
  queries or exports execute. Test bounded queues and a slow disk using demo data.
- [ ] Update release workflows, icons, desktop metadata, package dependencies, Arch
  scripts, third-party notices and documentation. Keep the version solely in Cargo.toml.
- [ ] Preserve `TracePlotter.exe`, its signature asset and portable data directory.
  Keep the current signing tool initially; replace it only after compatibility tests
  prove the existing updater accepts the new signatures and signed version metadata.
- [ ] Preserve `.deb`, `.rpm`, AppImage and Arch outputs. Select and validate native
  packaging tools before removing the existing Tauri bundling steps.
- [ ] Build local Windows trial executables with the `fast` profile and official
  releases with `release`. No Blaze patches or `dev-live` code enter either artifact.
- [ ] Have a person verify real ICMP/UDP/TCP, IPv4/IPv6, privileges, notifications and
  signed update/restart behavior on Windows and Linux. Do not run the ignored live
  network test or full Tauri packaging in a sandbox/cloud machine.
- [ ] Make egui the default after parity, migration and platform gates pass. Retire
  Svelte/uPlot, Node-based UI tooling and the legacy adapter after fixture mappings
  are complete. Keep the isolated migration helper for the supported upgrade window.
- [ ] Update README, CONTRIBUTING and AGENTS with the native commands and checks.

The native build becomes the shipping interface only after data migration, all six MCP
tools, all export formats, platform packaging and the signed update path are verified.
The optional Blaze experiment can remain disabled independently of that decision.

## Checks run while preparing this plan

The existing application was checked on Linux on 28 October 2026, in the required order:

| Check | Result |
| --- | --- |
| `npm run check` | Passed; no Svelte errors or warnings |
| `npm test` | Passed |
| `npm run build` | Passed |
| `cargo clippy --lib --tests --manifest-path src-tauri/Cargo.toml -- -D warnings` | Passed |
| `cargo test --lib --manifest-path src-tauri/Cargo.toml` | Passed; 21 passed, 5 ignored |

These results cover the existing app. No egui implementation, Blaze installation or
native performance/migration experiment was performed. Windows checks and live network,
release-signing and packaging tests remain implementation acceptance work.

## Sources and revision notes

The repository links below were inspected on 28 October 2026. They describe upstream
capabilities, not a successful TracePlotter integration or measured speedup.

- [egui_plot 0.38 release](https://github.com/emilk/egui_plot/releases/tag/0.38.0),
  [custom axes](https://github.com/emilk/egui_plot/blob/main/examples/custom_axes/src/app.rs),
  [linked bounds report](https://github.com/emilk/egui_plot/issues/75) and
  [linked layout report](https://github.com/emilk/egui_plot/issues/227).
- [egui_kittest](https://github.com/emilk/egui/tree/main/crates/egui_kittest).
- Blaze revision `0c0ca43feb462186ece3e46ad40cd1f8ac4b6270`:
  [usage](https://github.com/indexable-inc/blaze/blob/0c0ca43feb462186ece3e46ad40cd1f8ac4b6270/docs/usage.md),
  [platforms](https://github.com/indexable-inc/blaze/blob/0c0ca43feb462186ece3e46ad40cd1f8ac4b6270/docs/platforms.md),
  [live API](https://github.com/indexable-inc/blaze/blob/0c0ca43feb462186ece3e46ad40cd1f8ac4b6270/crates/blaze-live/src/lib.rs)
  and [headless egui harness](https://github.com/indexable-inc/blaze/blob/0c0ca43feb462186ece3e46ad40cd1f8ac4b6270/harness/live-egui/src/main.rs).
  Its documentation differs on which live-edit platforms have reached a release;
  verify the chosen binary instead of inferring support from `main`.
- BeautifulRust revision `58757ee890da9b3c8d5b785adc7a8d53cce27307`:
  [porting skill](https://github.com/morganlinton/BeautifulRust/blob/58757ee890da9b3c8d5b785adc7a8d53cce27307/skills/rust-port/SKILL.md),
  [best practices](https://github.com/morganlinton/BeautifulRust/blob/58757ee890da9b3c8d5b785adc7a8d53cce27307/skills/rust-best-practices/SKILL.md),
  [review skill](https://github.com/morganlinton/BeautifulRust/blob/58757ee890da9b3c8d5b785adc7a8d53cce27307/skills/rust-review/SKILL.md)
  and [lint configuration](https://github.com/morganlinton/BeautifulRust/tree/58757ee890da9b3c8d5b785adc7a8d53cce27307/lints).

# Contributing to TracePlotter

TracePlotter is a Rust desktop app with an [egui](https://www.egui.rs) interface,
[egui_plot](https://github.com/emilk/egui_plot) graphs and the
[trippy-core](https://github.com/fujiapple852/trippy) probe engine. It is developed on
Windows first; the code is kept portable.

## What you need

- Rust 1.100 or newer
- On Windows, Visual Studio Build Tools with "Desktop development with C++"
- On Debian or Ubuntu, `libwayland-dev` and `libxkbcommon-dev`

Windows x64 builds use Rust's bundled LLD linker, set in `.cargo/config.toml`, to shorten
linking without installing another tool. The Visual Studio Build Tools are still needed
for the Windows SDK and libraries. Rust 1.90 and newer already use LLD on x64 Linux.

## Run from source

From the repository root:

```bash
cargo native
cargo native -- --demo
cargo native -- --demo --mcp --demo-days 7
```

`cargo native` runs `cargo run`. Demo mode uses
made-up data in a separate folder and starts no probes. `--mcp` enables the MCP server
for that run. [AGENTS.md](AGENTS.md) has more for coding agents.

Only traces through raw sockets (ICMP (raw socket), UDP, TCP) need extra rights. Use
**Tools > Restart as Administrator…** on Windows, or **Allow raw sockets…** on Linux.

To log elapsed startup milestones to stderr, set `TRACEPLOTTER_STARTUP_TIMINGS=1`
before launching the app. The timings include window and GPU creation, settings,
system fonts, the first UI frame, deferred tray setup and each saved history replay. Use an optimized
`fast` build for launch measurements; `cargo run` also includes build time.

Settings load alongside GPU initialization. Normal launches create the tray and check
startup registration after the first frame; sign-in launches create the tray before
hiding, so a failed tray still leaves a visible window.

The window uses bundled fonts until system fonts finish loading. Saved tabs restore
their history with two workers, with the active tab queued first. History is shown
before hostname resolution, so a DNS failure does not hide an existing capture.

Historical graph and MCP range reads share up to four workers, leaving a core free
when possible. Their bounded queue prevents overlapping requests from creating a
separate set of threads for every tab. Day results are merged in chronological order.

Hidden and minimized windows keep probing, recording history and serving MCP, but do
not rebuild display snapshots or request repaints for probe updates. Restoring the
window refreshes its snapshot even when every trace is paused.

## Test

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib
cargo native-test
```

CI runs the native checks on Windows and Linux. No frontend build, npm, WebKitGTK or
WebView2 is needed. `ui-tests` adds the headless widget tests. Synthetic GPU/export checks
remain ignored unless requested explicitly.

A live probe check against a real host needs an elevated terminal and a network:

```bash
cargo test --lib -- --ignored live_trace --nocapture
```

Set `TRACEPLOTTER_TARGET` to an IP address to change its target (default `1.1.1.1`).

## Build

```bash
cargo build --release --locked
```

The Windows exe is written to `target/release/traceplotter.exe`. For a local
build to try on Windows, use the `fast` profile:

```bash
cargo build --profile fast --locked
```

It writes `target/fast/traceplotter.exe`, compiles on all cores, skips cross-crate
link-time optimisation and reuses compiled code between builds. Published builds use
`release`. Data next to a Windows build is deleted by `cargo clean`; copy the exe and its
data folder elsewhere to keep them.

## Where things are

- `src/native/mod.rs`: the window, menus, dialogs and commands.
- `src/native/layout.rs`, `graphs.rs`, `context.rs` and `theme.rs`: the interface.
- `src/native/app.rs`: the controller; probing, alerts and MCP keep running while hidden.
- `src/native/model.rs`: sessions, settings, trace data and persistence.
- `src/native/migration.rs`: automatic import of browser-stored sessions from older versions.
- `src/native/agent.rs`: the MCP tools' answers from the trace model.
- `src/native/export.rs`, `import.rs`, `report.rs` and `time.rs`: exports, CSV import,
  support reports and regional formatting.
- `src/native/tray.rs`: background monitoring and Windows startup registration.
- `src/logic.rs` and `insights.rs`: statistics and events, with unit tests.
- `src/lib.rs`: history, DNS, WHOIS, updates and platform commands.
- `src/probe.rs`, `winicmp.rs`, `pingsock.rs` and `echo.rs`: probing.
- `src/mcp.rs`: the MCP protocol and HTTP listener.

[PORTING.md](PORTING.md) describes saved-session compatibility and remaining platform checks.

## Rust guidelines

We use [BeautifulRust](https://github.com/morganlinton/BeautifulRust) as supplementary
Rust guidance. The [project-local copy](.agents/skills/README.md) includes writing,
review and porting skills, reference digests, the upstream license and the source commit.
The conventions in this guide and AGENTS.md take precedence.

Keep application code direct. Prefer concrete types and small functions over speculative
traits and builders. Use enums for meaningful modes, newtypes where units or IDs could
be confused, and Option and Result for absence and expected failures. Borrow with &str
and slices when ownership is unnecessary; move owned values when consuming them.
Avoid clones that only work around a borrow, while keeping deliberate snapshots and
shared ownership where the controller needs them.

Handle bad input and network failures without panicking, and preserve useful context in
errors. Document invariants and failure modes that signatures cannot express. Keep unsafe
code confined to platform or FFI boundaries with safe wrappers; every unsafe block needs
a SAFETY comment explaining why its requirements hold. Keep locks short and avoid doing
I/O or invoking callbacks while holding them.

Test observable behavior and edge cases, keeping pure logic tests beside their modules.
Measure before optimizing. Let rustfmt handle formatting and fix clippy findings rather
than suppressing them. If a lint does not fit an item, use a narrowly scoped
`#[expect(clippy::name, reason = "...")]` and explain why. Add new project lints
incrementally; the existing native checks must stay clean on Windows and Linux.

## Dependencies

Dependabot opens weekly pull requests for Rust crates and workflow actions. `trippy-core`
is not pinned or capped: when its API changes, fix the code rather than hold it back.
An update of it needs a build and the live probe check before it is merged.

## Publishing a release

Set the version in `Cargo.toml`, update `Cargo.lock`, commit, then push a tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

The Release workflow builds the native app, signs the Windows exe and publishes packages.
`scripts/package-linux.sh` builds the Debian and RPM packages with dpkg-deb and rpmbuild,
and `scripts/package-appimage.sh` the AppImage with linuxdeploy, without a GTK plugin. The Tauri CLI is used only to sign Windows
updates in the existing format.
The private signing key remains in `TAURI_SIGNING_PRIVATE_KEY`; its optional password is
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The public key is `UPDATE_PUBLIC_KEY` in `lib.rs`.
Signing keeps the existing Tauri/minisign format so older installed versions can verify updates.
Never commit the private key. Keep a backup: changing the key prevents existing installations
from accepting updates.

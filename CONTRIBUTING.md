# Contributing to TracePlotter

TracePlotter is a Rust desktop app with an [egui](https://www.egui.rs) interface,
[egui_plot](https://github.com/emilk/egui_plot) graphs and the
[trippy-core](https://github.com/fujiapple852/trippy) probe engine. It is developed on
Windows first; the code is kept portable.

## What you need

- Rust 1.95 or newer
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

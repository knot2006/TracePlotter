# Native GUI

TracePlotter uses egui for every launch. The Svelte interface, Tauri runtime, interface
switcher and interface-selection flags have been removed. The Rust crate now lives at the
repository root, with code in `src/` and builds in `target/`. Installed copies keep using the same data folders and saved formats.

## Saved sessions

Existing `native-state.json` files load directly. On the first launch without that file,
the app reads the previous version's browser storage, validates the tabs, settings and
layout, and saves them as `native-state.json`. No manual conversion or webview is needed.

The importer reads WebKitGTK's SQLite local storage on Linux and WebView2's LevelDB local
storage on Windows. It opens temporary copies, including SQLite's pending WAL writes,
and leaves the original profile untouched. Production origins take precedence over the
development server. Malformed data stops startup with an error instead of saving an empty
session. Close the previous version before starting the new one.

Session IDs stay the same, so trace history and events continue using their existing
JSONL files. `background.json` and `window.json` also keep their formats. Once a native
session exists, browser data is not imported again. A corrupt native session is reported;
it is not replaced by an older browser session.

## Development

```bash
cargo native -- --demo --mcp
cargo native-test
```

The golden fixtures under `fixtures/` preserve the original statistics and agent
contracts. They remain regression inputs even though the frontend that generated them
has been removed. GUI screenshots under `docs/gui-reference/` remain design references.

`dev-live` is an optional, default-off Blaze hook for debug demo sessions. Its vendored
source and provenance are under `vendor/blaze-live/`. It compiles through Cargo;
actual Blaze compilation remains an experiment. Use Cargo for verification and releases.

## Platform checks still needed

- Windows probing, elevation, firewall changes, signed updates and rollback.
- Narrator, IME, regional formats, high DPI, multiple monitors and Remote Desktop.
- Real WebView2 profiles, including interrupted upgrades and shutdown races.
- Linux Wayland/NVIDIA rendering, clipboard ownership and packaged desktop integration.
- Performance measurements on named Windows and Linux machines.

Use demo mode for interface checks. Do not run privileged or real-network checks in a sandbox.

# Changelog

## 0.2.0

TracePlotter is now a fully native application. The interface is drawn with
[egui](https://www.egui.rs) straight on the GPU, in the same Rust program that probes. Tauri,
the webview and the Svelte and TypeScript interface are gone, and so is the npm build.

### What changes for you

- **No webview.** Windows no longer needs the WebView2 runtime, and the Linux packages no longer
  need WebKitGTK 4.1. The app needs only the system's graphics drivers, and X11 or Wayland on
  Linux. The AppImage no longer carries its own WebKitGTK.
- **Your session comes along.** On its first launch the new version reads the tabs, settings and
  layout that the previous version kept in its browser storage, checks them and saves them as
  `native-state.json`. Trace history, events, `background.json` and `window.json` keep their
  files and formats, and session IDs stay the same, so graphs continue where they left off. The
  old browser profile is read from a copy and left as it was. Close the previous version before
  starting the new one.
- **Two themes.** Glass, the new default, has charcoal panels, soft borders and green accents.
  Its Transparency slider lets the desktop show through, blurred on Windows 11 and on KDE
  Plasma under Wayland. Ink is the previous near-black look, now with stronger state colours.
  Both are under **Settings > Appearance** and apply at once.
- **The same features.** Traces, the hop table, latency and loss graphs, route changes, alerts,
  incident timelines, support reports, background monitoring with the tray, image and PDF
  export, updates and the MCP server for AI agents all work as in 0.1.13.

### Fixes

- The history slider keeps its handle at the live end while a trace has less history than
  the time span.
- Screenshots of the window no longer show a grey veil over it.
- The menu bar's hover highlight leaves room around the menu names.

### For developers

The Rust crate moved from `src-tauri/` to the repository root: code in `src/`, builds in
`target/`. `cargo native -- --demo` runs the app in demo mode and `cargo native-test` runs the
interface tests; Node.js is no longer needed. `PORTING.md` describes the move and the session
import in more detail.

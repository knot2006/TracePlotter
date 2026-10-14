# Contributing to TracePlotter

TracePlotter is a [Tauri 2](https://tauri.app) desktop app. The interface is
[Svelte 5](https://svelte.dev) with [uPlot](https://github.com/leeoniya/uPlot) graphs, in `src/`.
The backend is Rust, in `src-tauri/`, and probes with the
[trippy-core](https://github.com/fujiapple852/trippy) engine. It is developed on Windows first; the
code is kept portable.

## What you need

- Node.js 26 or newer
- Rust (stable, MSVC toolchain)
- Visual Studio Build Tools with "Desktop development with C++"

## Run from source

A normal terminal will do. Only traces through raw sockets (ICMP (raw socket), UDP, TCP) need
Administrator rights; for those, start the terminal elevated or use **Tools > Restart as
Administrator…** in the app.

```bash
npm install
npm run tauri dev
```

Add `-- -- -- --demo` to work on the interface with made-up data and no probing. npm on Windows
takes the first `--` for itself; elsewhere two are enough, and the third does no harm:

```bash
npm run tauri dev -- -- -- --demo
```

Add `--mcp` as well to turn on the MCP server for that run, so that an agent can work against the
demo data. [AGENTS.md](AGENTS.md) has more for coding agents.

## Test

```bash
npm run check
npm test
cargo clippy --lib --tests --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

The CI workflow runs these on every push and pull request.

A live probe check against a real host needs an elevated terminal and a network:

```bash
cargo test --lib --manifest-path src-tauri/Cargo.toml -- --ignored live_trace --nocapture
```

Set `TRACEPLOTTER_TARGET` to an IP address to change its target (default `1.1.1.1`).

## Build

```bash
npm run tauri build -- --no-bundle
```

The exe is written to `src-tauri/target/release/traceplotter.exe`. That is a build folder: the
app's `TracePlotter-data` folder next to it is deleted by `cargo clean`.

The release build optimises on one core and takes a few minutes. For a local test build, the
`fast` profile compiles on all cores and writes `src-tauri/target/fast/traceplotter.exe`:

```bash
npm run tauri build -- --no-bundle -- --profile fast
```

## Where things are

- `src/routes/+page.svelte`: window layout, tabs, toolbar, menus, the saved session.
- `src/lib/TraceTab.svelte`: one tab, with the hop table, dialogs and exports.
- `src/lib/HopGraph.svelte`: one latency graph with its jitter strip.
- `src/lib/Settings.svelte` and `settings.ts`: the Settings tab and the app-wide settings it changes.
- `src/lib/trace.ts`: a trace's data and its link to the backend.
- `src/lib/stats.ts`, `events.ts`, `time.ts`: calculations and formatting, with tests beside them.
- `src-tauri/src/lib.rs`: probing, history files, updates and the other backend commands.
- `src-tauri/src/mcp.rs` and `src/lib/agent.ts`: the MCP server that lets AI agents read the traces.

Dates and numbers of a trace are kept in plain objects that are changed in place; the interface
redraws when a counter (`rev`) changes. That keeps a day of samples out of Svelte's reactivity.

## Dependencies

Dependabot opens weekly pull requests for npm packages, Rust crates and the workflows' actions.
`trippy-core` is not pinned or capped, so new releases are taken as they come, breaking ones
included. Its API is unstable, so when an update breaks the build we fix our code rather than hold
the version back. An update of it needs a build and the live probe check before it is merged.

## Publishing a release

1. Set the new version in `src-tauri/Cargo.toml`, the only place that holds it (Tauri takes it
   from there), build once so that `Cargo.lock` follows, and commit.
2. Tag that commit and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.

The Release workflow then builds the exe, signs it for that version and publishes it with its
signature. Installed copies find it through GitHub's latest release and install it only if the
signature is valid and names the version the release claims.

### The signing key

The workflow needs the repository secret `TAURI_SIGNING_PRIVATE_KEY`: the contents of the private
key made with `npm run tauri signer generate`. Its public half is `UPDATE_PUBLIC_KEY` in
`src-tauri/src/lib.rs`. A key with a password also needs the secret
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

Whoever has the private key can publish updates that every installed copy accepts. Keep it out of
the repository and keep a backup: with a new key, existing copies no longer accept updates.

To check that a file signed with the key is accepted by the app's own verification:

```bash
npm run tauri signer sign -- -f path/to/private.key -p "" --app-version 1.2.3 sample.bin
TRACEPLOTTER_SIGNED_FILE=sample.bin cargo test --lib --manifest-path src-tauri/Cargo.toml -- --ignored a_file_signed
```

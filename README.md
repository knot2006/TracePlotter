# TracePlotter

Open-source visual traceroute and ping monitor.

- Continuous traceroute over **ICMP, UDP or TCP** (IPv4 and IPv6)
- One tab per target, several targets at once
- Per-hop loss, last/avg/min/max latency and jitter
- Latency, jitter and packet-loss graphs for any hop, with a slider to scroll back in time
- MOS (estimated call quality) per hop
- Route changes are logged and notified; the trace recovers by itself after a network change
- Alerts on latency or loss of the final hop, with a desktop notification
- Open targets are restored on the next launch, with up to 24 h of history per target
- Export the hop table as text, samples as CSV, the graph as PNG and the page as PDF

Every tab is its own session, even when two tabs trace the same host. A tab's history is one
JSON-lines file in `%APPDATA%\io.github.traceplotter\history`; closing the tab or restarting its
trace erases that file.

Built with [Tauri 2](https://tauri.app), [Svelte 5](https://svelte.dev), [uPlot](https://github.com/leeoniya/uPlot)
and the [trippy-core](https://github.com/fujiapple852/trippy) probe engine. Windows first; the code is portable.

## Requirements

- Node.js 22+
- Rust (stable, MSVC toolchain) and Visual Studio Build Tools with "Desktop development with C++"
- **Administrator rights.** Probing uses raw sockets, so the exe asks for elevation (UAC) on start.

### Windows firewall

Windows Defender Firewall drops incoming ICMP replies to raw sockets by default. Without a rule, hops
show as lost and ICMP traces never reach the final hop. Allow them once with
**Tools > Allow ICMP replies in Windows Firewall…**, or from an elevated PowerShell:

```powershell
New-NetFirewallRule -Name TRACEPLOTTER_ICMPV4 -DisplayName "TracePlotter ICMPv4 replies" -Direction Inbound -Protocol ICMPv4 -IcmpType 0,3,11 -Action Allow
New-NetFirewallRule -Name TRACEPLOTTER_ICMPV6 -DisplayName "TracePlotter ICMPv6 replies" -Direction Inbound -Protocol ICMPv6 -IcmpType 129,1,3 -Action Allow
```

The rules allow echo reply, destination unreachable and time exceeded only; they do not make this
machine answer pings.

Remove them again with `Remove-NetFirewallRule TRACEPLOTTER_ICMPV4, TRACEPLOTTER_ICMPV6`.

## Develop

Run from an **elevated** terminal, otherwise the app fails to start with `os error 740`.

```bash
npm install
npm run tauri dev
```

## Test

```bash
npm test
npm run check
cargo test --lib --manifest-path src-tauri/Cargo.toml
```

Live probe check against a real host (elevated terminal, needs network):

```bash
cargo test --lib --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
```

Set `TRACEPLOTTER_TARGET` to an IP address to change the target (default `1.1.1.1`).

## Build

```bash
npm run tauri build
```

## License

Apache-2.0

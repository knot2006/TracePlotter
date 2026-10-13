# TracePlotter
<img width="1404" height="965" alt="image" src="https://github.com/user-attachments/assets/5c11aa92-9f6f-4e8b-b414-088da4ea6c57" />

Open-source visual traceroute and ping monitor.

- Continuous traceroute over **ICMP, UDP or TCP** (IPv4 and IPv6)
- ICMP through the Windows ICMP API or through raw sockets; reply timeout and packet size per target
- One tab per target, several targets at once
- Per-hop loss, last/avg/min/max latency and jitter
- Latency, jitter and packet-loss graphs for any hop, with a slider to scroll back in time
- MOS (estimated call quality) per hop
- Route changes are logged and notified; the trace recovers by itself after a network change
- Alerts on latency or loss of the round trip, with a desktop notification
- Open targets are restored on the next launch, with up to 24 h of history per target
- Export the hop table as text, samples as CSV, the graph as PNG and the page as PDF

Every tab is its own session, even when two tabs trace the same host. Closing a tab or restarting
its trace erases that tab's history.

## Requirements

- Windows 10 or 11 with the WebView2 runtime (part of Windows 11).
- **Administrator rights.** Raw-socket, UDP and TCP probing needs them, so the exe asks for elevation
  (UAC) on start.

### Windows firewall

Traces with the packet type **ICMP (Windows API)** need no firewall rule. This section is for
**ICMP (raw socket)**.

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

## Portable

There is no installer. `TracePlotter.exe` runs from any folder and keeps everything it stores in a
`TracePlotter-data` folder next to itself: trace history (`history`, one JSON-lines file per tab),
open tabs and settings (`webview`) and the window's size and position (`window.json`). Copy the exe
together with that folder to move the app; delete the folder to reset it. Nothing is written to the
user profile.

Not portable: the WebView2 runtime and the Windows Firewall rules above, which belong to the
machine.

## Updates

The app looks for a newer release on GitHub when it starts and under **Help > Check for
updates…**. A found update is offered in a bar at the top; **Update and restart** downloads it,
checks its signature, replaces the exe and restarts. Tabs and history stay. Nothing is downloaded
without that click, and **Help > Check for updates at startup** switches the check off.

An update is installed only if it is signed with the release key whose public half is built into
the app, so a download that was tampered with is refused.

## Demo mode

Start the exe with `--demo` to try the interface without probing anything. Every tab then shows
the same made-up nine-hop route, built from addresses reserved for documentation, with a quarter
of an hour of history already filled in. Demo data goes to a separate `TracePlotter-demo-data`
folder, so real tabs and history are left alone.

```powershell
.\TracePlotter.exe --demo
```

## Contributing

Building from source, running the tests and publishing a release are described in
[CONTRIBUTING.md](CONTRIBUTING.md).

Built with [Tauri 2](https://tauri.app), [Svelte 5](https://svelte.dev), [uPlot](https://github.com/leeoniya/uPlot)
and the [trippy-core](https://github.com/fujiapple852/trippy) probe engine.

## License

Apache-2.0

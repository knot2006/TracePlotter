use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpStream, ToSocketAddrs, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use trippy_core::{Builder, Port, PortDirection, ProbeStatus, Protocol, Round};

// ponytail: a history file holds up to twice this many rounds (24 h at a 1 s interval)
// and is read back in one piece when its tab starts. Rotate files if that gets slow.
const MAX_HISTORY: usize = 86_400;

/// One probe result for one hop in one round. `rtt` is `None` when the probe was lost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct HopSample {
    ttl: u8,
    addr: Option<IpAddr>,
    rtt: Option<f64>,
}

/// One round of a trace; also one line of a history file. `t` is Unix milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    t: u64,
    hops: Vec<HopSample>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum TraceEvent {
    Sample(Sample),
    Error { message: String },
}

#[derive(Serialize)]
struct Started {
    id: u32,
    addr: IpAddr,
    history: Vec<Sample>,
}

#[derive(Default)]
struct Traces {
    next_id: AtomicU32,
    stops: Mutex<HashMap<u32, Arc<AtomicBool>>>,
}

/// Rounds in a row without a single reply before the tracer is rebuilt.
const DEAD_ROUNDS: usize = 3;
/// Wait before rebuilding a tracer that failed.
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Panic payloads used to leave `Tracer::run_with`: for good, or to rebuild the tracer.
struct Stopped;
struct Restart;

/// Probes up to the furthest hop that replied this round, as trippy's own state does.
fn round_to_hops(round: &Round<'_>) -> Vec<HopSample> {
    round
        .probes
        .iter()
        .filter_map(|probe| match probe {
            ProbeStatus::Awaited(p) => Some(HopSample {
                ttl: p.ttl.0,
                addr: None,
                rtt: None,
            }),
            ProbeStatus::Complete(p) => Some(HopSample {
                ttl: p.ttl.0,
                addr: Some(p.host),
                // Rounded to 0.01 ms to keep history lines short.
                rtt: p
                    .received
                    .duration_since(p.sent)
                    .ok()
                    .map(|d| (d.as_secs_f64() * 100_000.0).round() / 100.0),
            }),
            _ => None,
        })
        .filter(|hop| hop.ttl <= round.largest_ttl.0)
        .collect()
}

/// The local address the OS would send from to reach `target` right now, `None` when it
/// has no route. Connecting a UDP socket only asks the routing table; nothing is sent.
fn local_source(target: IpAddr) -> Option<IpAddr> {
    let any: IpAddr = if target.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    };
    let socket = UdpSocket::bind((any, 0)).ok()?;
    socket.connect((target, 53)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

/// Makes untrusted text usable as a single file name component.
fn file_safe(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() => c,
            '.' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}

/// Trims a history file to its newest `max` rounds and returns those lines.
/// A missing file is an empty history.
fn trim_history(path: &Path, max: usize) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let recent = &lines[lines.len().saturating_sub(max)..];
    if recent.len() < lines.len() {
        let _ = fs::write(path, recent.join("\n") + "\n");
    }
    recent.iter().map(|line| line.to_string()).collect()
}

/// Reads the newest `max` rounds of a history file and trims the file to them.
/// Unreadable lines are skipped.
fn load_history(path: &Path, max: usize) -> Vec<Sample> {
    trim_history(path, max)
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Whether the app was started with `--demo`. Every tab then shows a made-up route with
/// documentation-only addresses instead of probing the network, for screenshots and for
/// working on the interface. Demo data lives in its own folder, apart from real history.
fn demo() -> bool {
    static DEMO: OnceLock<bool> = OnceLock::new();
    *DEMO.get_or_init(|| std::env::args().any(|arg| arg == "--demo"))
}

/// The demo route: address, name, usual latency in ms and share of probes lost per hop.
/// The addresses between the home router and the target are from the ranges reserved
/// for documentation (RFC 5737), so they belong to nobody.
const DEMO_ROUTE: [(&str, &str, f64, f64); 9] = [
    ("192.168.1.1", "router.home.arpa", 0.4, 0.0),
    ("10.20.0.1", "gw.access.example.net", 1.3, 0.0),
    ("198.51.100.17", "core1.waw.example.net", 4.1, 0.0),
    ("198.51.100.42", "core2.waw.example.net", 4.6, 0.0),
    ("203.0.113.9", "ix1.waw.example.org", 7.0, 0.0),
    ("203.0.113.77", "", 7.4, 0.35),
    ("192.0.2.33", "edge3.fra.example.com", 7.6, 0.0),
    ("192.0.2.120", "", 7.9, 0.01),
    ("8.8.8.8", "dns.google", 7.7, 0.004),
];

/// Pseudo-random numbers in 0..1 for the demo (xorshift); no need for a crate.
struct DemoRng(u64);

impl DemoRng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// One made-up round at Unix time `t` in ms: steady near hops, a slow swell and the odd
/// spike shared by the hops beyond the provider's edge, and one hop that often stays silent.
fn demo_sample(rng: &mut DemoRng, t: u64) -> Sample {
    let swell = 1.0 + 0.5 * (t as f64 / 120_000.0).sin().max(0.0);
    let spike = if rng.next() < 0.02 {
        15.0 + 45.0 * rng.next()
    } else {
        0.0
    };
    let hops = DEMO_ROUTE
        .iter()
        .enumerate()
        .map(|(i, (addr, _, usual, loss))| {
            let far = i >= 2;
            let rtt = usual * if far { swell } else { 1.0 }
                + usual * 0.2 * rng.next()
                + if far { spike } else { 0.0 };
            let lost = rng.next() < *loss;
            HopSample {
                ttl: i as u8 + 1,
                addr: (!lost).then(|| addr.parse().unwrap()),
                rtt: (!lost).then(|| (rtt * 100.0).round() / 100.0),
            }
        })
        .collect();
    Sample { t, hops }
}

/// `start_trace` in demo mode: a tab with a quarter of an hour of made-up history that
/// keeps getting made-up rounds. Nothing is sent on the network.
fn start_demo(
    session: &str,
    interval_ms: u64,
    with_history: bool,
    channel: Channel<TraceEvent>,
    traces: &Traces,
) -> Result<Started, String> {
    let interval_ms = interval_ms.clamp(200, 60_000);
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    };
    let mut rng = DemoRng(now() | 1);
    let path = history_path(session)?;
    if with_history && !path.exists() {
        let start = now();
        let past: Vec<String> = (1..=900u64)
            .rev()
            .filter_map(|ago| {
                serde_json::to_string(&demo_sample(&mut rng, start - ago * interval_ms)).ok()
            })
            .collect();
        fs::write(&path, past.join("\n") + "\n").map_err(|e| e.to_string())?;
    }
    let history = if with_history {
        load_history(&path, MAX_HISTORY)
    } else {
        Vec::new()
    };
    let file = OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let id = traces.next_id.fetch_add(1, Ordering::Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    traces.stops.lock().unwrap().insert(id, stop.clone());
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(interval_ms));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let sample = demo_sample(&mut rng, now());
            if let Ok(line) = serde_json::to_string(&sample) {
                let _ = writeln!(&file, "{line}");
            }
            if channel.send(TraceEvent::Sample(sample)).is_err() {
                return;
            }
        }
    });
    let (target, ..) = DEMO_ROUTE[DEMO_ROUTE.len() - 1];
    Ok(Started {
        id,
        addr: target.parse().unwrap(),
        history,
    })
}

#[tauri::command(async)]
fn start_trace(
    // Id of the tab; names its history file, so tabs never share history.
    session: String,
    host: String,
    protocol: String,
    port: u16,
    interval_ms: u64,
    // False when resuming a paused trace, whose history the caller already holds.
    with_history: bool,
    channel: Channel<TraceEvent>,
    traces: State<'_, Traces>,
) -> Result<Started, String> {
    if demo() {
        return start_demo(&session, interval_ms, with_history, channel, &traces);
    }
    let host = host.trim().to_lowercase();
    let addr = (host.as_str(), 0)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {host}"))?
        .ip();
    let (probe, ports) = match protocol.as_str() {
        "icmp" => (Protocol::Icmp, PortDirection::None),
        "udp" => (Protocol::Udp, PortDirection::FixedDest(Port(port))),
        "tcp" => (Protocol::Tcp, PortDirection::FixedDest(Port(port))),
        other => return Err(format!("unknown protocol {other}")),
    };
    let interval = Duration::from_millis(interval_ms.clamp(200, 60_000));
    let id = traces.next_id.fetch_add(1, Ordering::Relaxed);
    let build = move || {
        Builder::new(addr)
            .protocol(probe)
            .port_direction(ports)
            // Distinct identifiers keep concurrent ICMP traces from reading each other's
            // replies. The identifier is 16 bits, so it repeats every 65,536 runs.
            .trace_identifier(id as u16)
            .min_round_duration(interval)
            .max_round_duration(interval)
            .build()
    };
    let mut tracer = Some(build().map_err(|e| e.to_string())?);

    let path = history_path(&session)?;
    let history = if with_history {
        load_history(&path, MAX_HISTORY)
    } else {
        Vec::new()
    };
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;

    let stop = Arc::new(AtomicBool::new(false));
    traces.stops.lock().unwrap().insert(id, stop.clone());

    std::thread::spawn(move || {
        // A tracer keeps the source address and sockets it started with. After the machine
        // changes network (VPN, Wi-Fi, new default route) those are dead: every probe is
        // lost or sending fails. So the tracer is rebuilt as soon as the source address
        // for the target changes, and as a fallback after DEAD_ROUNDS silent rounds or
        // any error. Rebuilding rediscovers the source address and the path.
        let silent_rounds = AtomicUsize::new(0);
        let failing = AtomicBool::new(false);
        // Rounds appended since the file was last trimmed. Trimming every MAX_HISTORY
        // rounds keeps the file under twice that, however long the trace runs.
        let appended = AtomicUsize::new(0);
        while !stop.load(Ordering::Relaxed) {
            let source = local_source(addr);
            let current = match tracer.take().map_or_else(&build, Ok) {
                Ok(tracer) => tracer,
                Err(e) => {
                    failing.store(true, Ordering::Relaxed);
                    let _ = channel.send(TraceEvent::Error {
                        message: format!("Probing failed, retrying: {e}"),
                    });
                    std::thread::sleep(RETRY_DELAY);
                    continue;
                }
            };
            // ponytail: trippy-core 0.13 has no way to cancel an endless trace, so the round
            // callback unwinds out of it; stop and restart take effect at the next round.
            // Replace with a real cancel if upstream adds one.
            let result = catch_unwind(AssertUnwindSafe(|| {
                current.run_with(|round| {
                    // Stopped (paused, closed, restarted): this round is not recorded.
                    if stop.load(Ordering::Relaxed) {
                        resume_unwind(Box::new(Stopped));
                    }
                    // The machine now sends from another address: this round went out on
                    // the old route, so it is dropped and the tracer rebuilt right away.
                    if local_source(addr) != source {
                        resume_unwind(Box::new(Restart));
                    }
                    let t = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    let sample = Sample {
                        t,
                        hops: round_to_hops(round),
                    };
                    let silent = sample.hops.iter().all(|hop| hop.rtt.is_none());
                    // An empty message clears the error shown for an earlier failure.
                    if failing.swap(false, Ordering::Relaxed) {
                        let _ = channel.send(TraceEvent::Error {
                            message: String::new(),
                        });
                    }
                    let saved = serde_json::to_string(&sample)
                        .map_err(std::io::Error::other)
                        .and_then(|line| writeln!(&file, "{line}"));
                    if let Err(e) = saved {
                        failing.store(true, Ordering::Relaxed);
                        let _ = channel.send(TraceEvent::Error {
                            message: format!("History is not being saved: {e}"),
                        });
                    } else if appended.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_HISTORY {
                        appended.store(0, Ordering::Relaxed);
                        trim_history(&path, MAX_HISTORY);
                    }
                    // A failed send means the webview is gone, so stop as well.
                    if channel.send(TraceEvent::Sample(sample)).is_err() {
                        resume_unwind(Box::new(Stopped));
                    }
                    if !silent {
                        silent_rounds.store(0, Ordering::Relaxed);
                    } else if silent_rounds.fetch_add(1, Ordering::Relaxed) + 1 >= DEAD_ROUNDS {
                        silent_rounds.store(0, Ordering::Relaxed);
                        resume_unwind(Box::new(Restart));
                    }
                })
            }));
            let message = match result {
                Ok(Ok(())) => return,
                Err(payload) if payload.is::<Stopped>() => return,
                Err(payload) if payload.is::<Restart>() => continue,
                Ok(Err(e)) => format!("Probing failed, retrying: {e}"),
                Err(_) => {
                    let _ = channel.send(TraceEvent::Error {
                        message: "Trace stopped: probe thread panicked".into(),
                    });
                    return;
                }
            };
            failing.store(true, Ordering::Relaxed);
            let _ = channel.send(TraceEvent::Error { message });
            std::thread::sleep(RETRY_DELAY);
        }
    });

    Ok(Started { id, addr, history })
}

/// The folder for everything the app stores: trace history, the webview's storage (open
/// tabs and settings) and the window's size. It sits next to the exe, not in the user
/// profile, so the app is portable: copy the exe with this folder and nothing is left behind.
fn data_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        exe.parent().unwrap_or(Path::new(".")).join(if demo() {
            "TracePlotter-demo-data"
        } else {
            "TracePlotter-data"
        })
    })
}

/// The history file of one tab, in a folder that is created on demand.
fn history_path(session: &str) -> Result<PathBuf, String> {
    let name = file_safe(session);
    if name.is_empty() {
        return Err("missing session id".into());
    }
    let dir = data_dir().join("history");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir.join(format!("{name}.jsonl")))
}

/// Size, position and maximized state of the window, kept between launches.
/// Position and size are in physical pixels, of the window while not maximized.
#[derive(Clone, Copy, Serialize, Deserialize)]
struct WindowState {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

fn load_window_state() -> Option<WindowState> {
    let text = fs::read_to_string(data_dir().join("window.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remembers how the window is placed. Called when it is about to close.
fn save_window_state(window: &tauri::Window) {
    // A minimized window reports a position far off screen; keep what was saved before.
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let maximized = window.is_maximized().unwrap_or(false);
    let placed = match (window.outer_position(), window.inner_size()) {
        (Ok(at), Ok(size)) => WindowState {
            x: at.x,
            y: at.y,
            width: size.width,
            height: size.height,
            maximized,
        },
        _ => return,
    };
    // While maximized the window reports the screen's size; keep the size it goes back to.
    let state = match load_window_state() {
        Some(before) if maximized => WindowState { maximized, ..before },
        _ => placed,
    };
    if let Ok(text) = serde_json::to_string(&state) {
        let _ = fs::write(data_dir().join("window.json"), text);
    }
}

/// Puts the window back where it was. The position is used only if it is still on a
/// connected screen, so the window cannot open out of sight after a monitor is unplugged.
fn restore_window_state(window: &tauri::WebviewWindow) {
    let Some(state) = load_window_state() else {
        return;
    };
    let _ = window.set_size(tauri::PhysicalSize::new(state.width, state.height));
    const MARGIN: i32 = 50;
    let on_screen = window.available_monitors().is_ok_and(|monitors| {
        monitors.iter().any(|monitor| {
            let (at, size) = (monitor.position(), monitor.size());
            state.x + MARGIN >= at.x
                && state.x + MARGIN < at.x + size.width as i32
                && state.y + MARGIN >= at.y
                && state.y + MARGIN < at.y + size.height as i32
        })
    });
    if on_screen {
        let _ = window.set_position(tauri::PhysicalPosition::new(state.x, state.y));
    }
    if state.maximized {
        let _ = window.maximize();
    }
}

/// Erases a tab's saved history. A trace still writing to it loses those rounds too.
#[tauri::command]
fn delete_history(session: String) -> Result<(), String> {
    let path = history_path(&session)?;
    match fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot delete {}: {e}", path.display()))
        }
        _ => Ok(()),
    }
}

#[tauri::command]
fn stop_trace(id: u32, traces: State<'_, Traces>) {
    if let Some(stop) = traces.stops.lock().unwrap().remove(&id) {
        stop.store(true, Ordering::Relaxed);
    }
}

/// The locale of the host's regional format (how it writes dates and times), such as
/// "pl-PL". This is the Windows "Regional format" setting, which can differ from the
/// display language the webview goes by. `None` where it cannot be read.
#[tauri::command]
fn host_locale() -> Option<String> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetUserDefaultLocaleName(name: *mut u16, size: i32) -> i32;
        }
        // LOCALE_NAME_MAX_LENGTH is 85, including the terminating zero.
        let mut name = [0u16; 85];
        // SAFETY: the buffer is valid for `name.len()` UTF-16 units, the size passed.
        let length = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
        (length > 1).then(|| String::from_utf16_lossy(&name[..length as usize - 1]))
    }
    #[cfg(not(windows))]
    None
}

/// Hostname for a hop address, `None` when it has no PTR record.
#[tauri::command(async)]
fn reverse_dns(addr: IpAddr) -> Option<String> {
    if demo() {
        let text = addr.to_string();
        return DEMO_ROUTE
            .iter()
            .find(|(hop, name, ..)| *hop == text && !name.is_empty())
            .map(|(_, name, ..)| name.to_string());
    }
    dns_lookup::lookup_addr(&addr)
        .ok()
        .filter(|name| *name != addr.to_string())
}

const WHOIS_TIMEOUT: Duration = Duration::from_secs(8);

/// Sends one query to a whois server (TCP port 43) and returns its whole answer.
fn whois_query(server: &str, query: &str) -> std::io::Result<String> {
    let addr = (server, 43)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("no address for {server}")))?;
    let mut stream = TcpStream::connect_timeout(&addr, WHOIS_TIMEOUT)?;
    stream.set_read_timeout(Some(WHOIS_TIMEOUT))?;
    stream.set_write_timeout(Some(WHOIS_TIMEOUT))?;
    stream.write_all(format!("{query}\r\n").as_bytes())?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer)?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// Who an address belongs to: asks IANA, then the registry IANA refers to.
#[tauri::command(async)]
fn whois(addr: IpAddr) -> Result<String, String> {
    let query = addr.to_string();
    let iana = whois_query("whois.iana.org", &query).map_err(|e| e.to_string())?;
    let registry = iana
        .lines()
        .find_map(|line| line.strip_prefix("refer:"))
        .map(str::trim);
    match registry {
        Some(server) => whois_query(server, &query).map_err(|e| format!("{server}: {e}")),
        None => Ok(iana),
    }
}

/// Asks the user where to save the raw request body, suggesting the Downloads folder and
/// the name given in the `x-file-name` header. Returns the chosen path, `None` if cancelled.
#[tauri::command(async)]
fn export_file(app: AppHandle, request: Request<'_>) -> Result<Option<String>, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("export needs a binary body".into());
    };
    let name = request
        .headers()
        .get("x-file-name")
        .and_then(|value| value.to_str().ok())
        .map(file_safe)
        .filter(|name| !name.is_empty())
        .ok_or("export needs a file name")?;
    let mut dialog = app.dialog().file().set_file_name(&name);
    if let Ok(downloads) = app.path().download_dir() {
        dialog = dialog.set_directory(downloads);
    }
    if let Some(extension) = Path::new(&name).extension().and_then(|e| e.to_str()) {
        dialog = dialog.add_filter(extension.to_uppercase(), &[extension]);
    }
    let Some(path) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

/// Adds the inbound Windows Firewall rules that let ICMP replies reach raw sockets:
/// echo reply, destination unreachable and time exceeded, for IPv4 and IPv6.
/// Without them the firewall drops the replies and hops show as lost.
/// Also removes the rules added under the app's former name, PingScape.
#[tauri::command(async)]
fn allow_icmp_replies() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
            Remove-NetFirewallRule -Name TRACEPLOTTER_ICMPV4, TRACEPLOTTER_ICMPV6, \
                PINGSCAPE_ICMPV4, PINGSCAPE_ICMPV6 -ErrorAction SilentlyContinue; \
            New-NetFirewallRule -Name TRACEPLOTTER_ICMPV4 -DisplayName 'TracePlotter ICMPv4 replies' \
                -Direction Inbound -Protocol ICMPv4 -IcmpType 0, 3, 11 -Action Allow | Out-Null; \
            New-NetFirewallRule -Name TRACEPLOTTER_ICMPV6 -DisplayName 'TracePlotter ICMPv6 replies' \
                -Direction Inbound -Protocol ICMPv6 -IcmpType 129, 1, 3 -Action Allow | Out-Null";
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Full path: this process is elevated, so PATH must not pick the binary.
        let root = std::env::var("SystemRoot").map_err(|e| e.to_string())?;
        let output = std::process::Command::new(format!(
            r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe"
        ))
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
    #[cfg(not(windows))]
    Err("Firewall rules are only needed on Windows".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Traces::default())
        // The window is created here, not in tauri.conf.json, to point the webview's
        // storage at the data folder. It stays hidden until it is placed where it was.
        .setup(|app| {
            fs::create_dir_all(data_dir())?;
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("TracePlotter")
                .inner_size(1100.0, 720.0)
                .min_inner_size(640.0, 420.0)
                .data_directory(data_dir().join("webview"))
                .visible(false)
                .build()?;
            restore_window_state(&window);
            window.show()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                save_window_state(window);
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_trace,
            stop_trace,
            delete_history,
            host_locale,
            reverse_dns,
            whois,
            export_file,
            allow_icmp_replies
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use trippy_core::{
        CompletionReason, Flags, Probe, RoundId, Sequence, TimeToLive, TraceId,
    };

    fn awaited(ttl: u8) -> ProbeStatus {
        ProbeStatus::Awaited(Probe {
            sequence: Sequence(0),
            identifier: TraceId(0),
            src_port: Port(0),
            dest_port: Port(0),
            ttl: TimeToLive(ttl),
            round: RoundId(0),
            sent: SystemTime::now(),
            flags: Flags::empty(),
        })
    }

    #[test]
    fn round_keeps_lost_probes_up_to_largest_ttl() {
        let probes = [awaited(1), ProbeStatus::NotSent, awaited(2), awaited(3)];
        let round = Round::new(&probes, TimeToLive(2), CompletionReason::RoundTimeLimitExceeded);
        let lost = |ttl| HopSample {
            ttl,
            addr: None,
            rtt: None,
        };
        assert_eq!(round_to_hops(&round), vec![lost(1), lost(2)]);
    }

    #[test]
    fn history_keeps_newest_rounds_and_skips_bad_lines() {
        let path = std::env::temp_dir().join(format!("traceplotter-test-{}.jsonl", std::process::id()));
        let line = |t| format!(r#"{{"t":{t},"hops":[{{"ttl":1,"addr":"10.0.0.1","rtt":1.5}}]}}"#);
        fs::write(&path, [line(1), line(2), "garbage".into(), line(4)].join("\n") + "\n").unwrap();

        let loaded = load_history(&path, 3);
        let kept = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(loaded.iter().map(|s| s.t).collect::<Vec<_>>(), [2, 4]);
        assert_eq!(loaded[0].hops[0].rtt, Some(1.5));
        assert_eq!(kept.lines().count(), 3);
        assert_eq!(file_safe("..\\a/b:c"), ".._a_b_c");
    }

    #[test]
    fn demo_rounds_cover_the_route_with_documentation_addresses() {
        let mut rng = DemoRng(1);
        let rounds: Vec<Sample> = (0..200).map(|i| demo_sample(&mut rng, i * 1000)).collect();
        assert!(rounds.iter().all(|round| round.hops.len() == DEMO_ROUTE.len()));
        assert!(rounds.iter().all(|round| round.hops.iter().all(|hop| hop.rtt.is_none_or(|rtt| rtt > 0.0))));
        // The silent hop loses a fair share, the target next to nothing.
        let lost = |ttl: u8| rounds.iter().filter(|r| r.hops[ttl as usize - 1].rtt.is_none()).count();
        assert!((40..110).contains(&lost(6)), "hop 6 lost {}", lost(6));
        assert!(lost(9) < 10);
    }

    /// Live check, needs Administrator and network:
    /// `cargo test --lib -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_trace_gets_replies() {
        for (protocol, ports) in [
            (Protocol::Icmp, PortDirection::None),
            (Protocol::Udp, PortDirection::FixedDest(Port(33434))),
            (Protocol::Tcp, PortDirection::FixedDest(Port(443))),
        ] {
            let replies = std::sync::atomic::AtomicUsize::new(0);
            let target = std::env::var("TRACEPLOTTER_TARGET").unwrap_or("1.1.1.1".into());
            Builder::new(target.parse().unwrap())
                .protocol(protocol)
                .port_direction(ports)
                .max_rounds(Some(3))
                .build()
                .unwrap()
                .run_with(|round| {
                    let hops = round_to_hops(round);
                    println!("{protocol:?} {hops:?}");
                    replies.fetch_add(hops.iter().filter(|h| h.rtt.is_some()).count(), Ordering::Relaxed);
                })
                .unwrap();
            assert!(replies.into_inner() > 0, "{protocol:?}: no replies");
        }
    }
}

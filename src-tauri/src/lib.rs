use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpStream, ToSocketAddrs, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use trippy_core::{Builder, Port, PortDirection, ProbeStatus, Protocol, Round, Tracer};

#[cfg(any(windows, target_os = "linux"))]
mod echo;
mod mcp;
#[cfg(target_os = "linux")]
mod pingsock;
#[cfg(windows)]
mod winicmp;

/// ICMP echo through the system, which needs no raw socket: the Windows ICMP API, or Linux
/// ping sockets.
#[cfg(target_os = "linux")]
use pingsock::Pinger;
#[cfg(windows)]
use winicmp::Pinger;

/// Hours of history a tab keeps. The Settings tab changes it through `set_history_hours`.
static HISTORY_HOURS: AtomicU64 = AtomicU64::new(24);
// ponytail: a history file holds up to twice this many rounds (4 days at a 1 s interval)
// and is read back in one piece when its tab starts. Rotate files if that gets slow.
const MAX_HISTORY: usize = 345_600;

/// How many rounds a tab keeps: the hours of history at its interval, up to MAX_HISTORY.
fn history_rounds(interval_ms: u64) -> usize {
    let rounds = HISTORY_HOURS.load(Ordering::Relaxed) * 3_600_000 / interval_ms.max(1);
    (rounds as usize).clamp(1, MAX_HISTORY)
}

/// Sets how many hours of history tabs keep. Running traces follow at their next trim,
/// saved histories when they are next read.
#[tauri::command]
fn set_history_hours(hours: u64) {
    HISTORY_HOURS.store(hours.clamp(1, 168), Ordering::Relaxed);
}

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

/// Rounds in a row without a single reply before the tracer is rebuilt, for rounds that
/// last one interval. Longer rounds (a reply timeout above the interval) count for more.
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
        load_history(&path, history_rounds(interval_ms))
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

/// Where the rounds of a running trace go: its history file and the frontend.
struct Recorder {
    channel: Channel<TraceEvent>,
    file: fs::File,
    path: PathBuf,
    /// The tab shows an error; the next recorded round clears it.
    failing: AtomicBool,
    /// The trace's interval, which sets how many rounds the file keeps.
    interval_ms: u64,
    /// Rounds appended since the file was last trimmed. Trimming every `history_rounds`
    /// rounds keeps the file under twice that, however long the trace runs.
    appended: AtomicUsize,
}

impl Recorder {
    /// Shows an error in the tab until a round is recorded.
    fn error(&self, message: String) {
        self.failing.store(true, Ordering::Relaxed);
        let _ = self.channel.send(TraceEvent::Error { message });
    }

    /// Saves one round and sends it to the frontend. `false` when the webview is gone,
    /// so the trace should stop.
    fn record(&self, hops: Vec<HopSample>) -> bool {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let sample = Sample { t, hops };
        // An empty message clears the error shown for an earlier failure.
        if self.failing.swap(false, Ordering::Relaxed) {
            let _ = self.channel.send(TraceEvent::Error {
                message: String::new(),
            });
        }
        let saved = serde_json::to_string(&sample)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(&self.file, "{line}"));
        if let Err(e) = saved {
            self.error(format!("History is not being saved: {e}"));
        } else if self.appended.fetch_add(1, Ordering::Relaxed) + 1 >= history_rounds(self.interval_ms) {
            self.appended.store(0, Ordering::Relaxed);
            trim_history(&self.path, history_rounds(self.interval_ms));
        }
        self.channel.send(TraceEvent::Sample(sample)).is_ok()
    }
}

/// Runs a raw-socket trace until `stop` is set. `tracer` is the one built to check the
/// settings; later ones come from `build`.
///
/// A tracer keeps the source address and sockets it started with. After the machine
/// changes network (VPN, Wi-Fi, new default route) those are dead: every probe is lost or
/// sending fails. So the tracer is rebuilt as soon as the source address for the target
/// changes, and as a fallback after DEAD_ROUNDS silent rounds or any error. Rebuilding
/// rediscovers the source address and the path.
fn trace_raw(
    build: impl Fn() -> Result<Tracer, trippy_core::Error>,
    mut tracer: Option<Tracer>,
    // Silent rounds in a row after which the tracer is rebuilt.
    dead_rounds: usize,
    addr: IpAddr,
    stop: &AtomicBool,
    recorder: &Recorder,
) {
    let silent_rounds = AtomicUsize::new(0);
    while !stop.load(Ordering::Relaxed) {
        let source = local_source(addr);
        let current = match tracer.take().map_or_else(&build, Ok) {
            Ok(tracer) => tracer,
            Err(e) => {
                recorder.error(format!("Probing failed, retrying: {e}"));
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
                let hops = round_to_hops(round);
                let silent = hops.iter().all(|hop| hop.rtt.is_none());
                if !recorder.record(hops) {
                    resume_unwind(Box::new(Stopped));
                }
                if !silent {
                    silent_rounds.store(0, Ordering::Relaxed);
                } else if silent_rounds.fetch_add(1, Ordering::Relaxed) + 1 >= dead_rounds {
                    silent_rounds.store(0, Ordering::Relaxed);
                    resume_unwind(Box::new(Restart));
                }
            })
        }));
        match result {
            Ok(Ok(())) => return,
            Err(payload) if payload.is::<Stopped>() => return,
            Err(payload) if payload.is::<Restart>() => continue,
            Ok(Err(e)) => recorder.error(format!("Probing failed, retrying: {e}")),
            Err(_) => {
                recorder.error("Trace stopped: probe thread panicked".into());
                return;
            }
        }
        std::thread::sleep(RETRY_DELAY);
    }
}

/// Runs a trace by ICMP echo through the system (`Pinger`) until `stop` is set. It holds no
/// source address, so a change of network needs no rebuilding.
#[cfg(any(windows, target_os = "linux"))]
fn trace_echo(
    addr: IpAddr,
    packet_size: u16,
    timeout: Duration,
    interval: Duration,
    stop: &AtomicBool,
    recorder: &Recorder,
) {
    let mut pinger = match Pinger::new(addr, packet_size, timeout) {
        Ok(pinger) => pinger,
        Err(e) => return recorder.error(format!("Trace stopped: {e}")),
    };
    loop {
        let hops = pinger.round(interval);
        // Stopped (paused, closed, restarted): this round is not recorded.
        if stop.load(Ordering::Relaxed) || !recorder.record(hops) {
            return;
        }
    }
}

// One argument per probe setting: the webview passes them by name.
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
fn start_trace(
    // Id of the tab; names its history file, so tabs never share history.
    session: String,
    host: String,
    // "icmp", "udp" and "tcp" probe through raw sockets, "winicmp" through the system's ICMP
    // echo: the Windows ICMP API, or Linux ping sockets. The name stays for saved tabs.
    protocol: String,
    port: u16,
    interval_ms: u64,
    // How long a probe may go unanswered before it counts as lost.
    timeout_ms: u64,
    // Size of a probe's whole IP packet in bytes. TCP probes have no payload and ignore it.
    packet_size: u16,
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
    // `None` is the system's ICMP echo.
    let raw = match protocol.as_str() {
        "icmp" => Some((Protocol::Icmp, PortDirection::None)),
        "udp" => Some((Protocol::Udp, PortDirection::FixedDest(Port(port)))),
        "tcp" => Some((Protocol::Tcp, PortDirection::FixedDest(Port(port)))),
        "winicmp" if cfg!(any(windows, target_os = "linux")) => None,
        other => return Err(format!("unknown protocol {other}")),
    };
    if raw.is_some() && !is_elevated() {
        #[cfg(windows)]
        return Err(format!(
            "{} probes use raw sockets, which need Administrator rights. Restart TracePlotter as Administrator (File menu) or use ICMP (Windows API).",
            protocol.to_uppercase()
        ));
        #[cfg(not(windows))]
        return Err(format!(
            "{} probes use raw sockets, which TracePlotter may not open yet. Allow them under Tools > Allow raw sockets… or use ICMP (ping socket).",
            protocol.to_uppercase()
        ));
    }
    let interval = Duration::from_millis(interval_ms.clamp(200, 60_000));
    // A round lasts at least the interval, and a reply counts until its round ends.
    let timeout = Duration::from_millis(timeout_ms.clamp(100, 10_000)).max(interval);
    // From the smallest IPv6 probe to the most trippy-core sends.
    let packet_size = packet_size.clamp(48, 1024);
    let id = traces.next_id.fetch_add(1, Ordering::Relaxed);
    let build = raw.map(|(probe, ports)| {
        move || {
            Builder::new(addr)
                .protocol(probe)
                .port_direction(ports)
                // Distinct identifiers keep concurrent ICMP traces from reading each other's
                // replies. The identifier is 16 bits, so it repeats every 65,536 runs.
                .trace_identifier(id as u16)
                .packet_size(packet_size)
                // A round ends at the interval once the target answered; without an
                // answer it waits out the timeout.
                .min_round_duration(interval)
                .max_round_duration(timeout)
                .build()
        }
    });
    // Built here so that settings the tracer refuses are reported to the caller.
    let tracer = match &build {
        Some(build) => Some(build().map_err(|e| e.to_string())?),
        None => None,
    };

    let path = history_path(&session)?;
    let history = if with_history {
        load_history(&path, history_rounds(interval.as_millis() as u64))
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
    // A silent round lasts the timeout. The rebuild should come after about DEAD_ROUNDS
    // intervals whatever the timeout, so a long timeout does not keep a dead tracer for long.
    let dead_rounds = (DEAD_ROUNDS as u128 * interval.as_millis())
        .div_ceil(timeout.as_millis())
        .max(1) as usize;
    let recorder = Recorder {
        channel,
        file,
        path,
        failing: AtomicBool::new(false),
        interval_ms: interval.as_millis() as u64,
        appended: AtomicUsize::new(0),
    };

    std::thread::spawn(move || match build {
        Some(build) => trace_raw(build, tracer, dead_rounds, addr, &stop, &recorder),
        #[cfg(any(windows, target_os = "linux"))]
        None => trace_echo(addr, packet_size, timeout, interval, &stop, &recorder),
        #[cfg(not(any(windows, target_os = "linux")))]
        None => {}
    });

    Ok(Started { id, addr, history })
}

/// The folder for everything the app stores: trace history, the webview's storage (open
/// tabs and settings) and the window's size. On Windows it sits next to the exe, not in the
/// user profile, so the app is portable: copy the exe with this folder and nothing is left
/// behind. Elsewhere the exe is installed where the user cannot write, so it is in the
/// user's data folder (`~/.local/share`).
fn data_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        #[cfg(windows)]
        let base = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| ".".into());
        #[cfg(not(windows))]
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")))
            .unwrap_or_else(|| ".".into());
        base.join(if demo() {
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

/// A tab's saved history without starting its trace, for a tab that is restored paused.
#[tauri::command(async)]
fn read_history(session: String, interval_ms: u64) -> Result<Vec<Sample>, String> {
    Ok(load_history(&history_path(&session)?, history_rounds(interval_ms)))
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
/// "pl-PL". This is the Windows "Regional format" setting, or elsewhere the locale of
/// LC_TIME, which can differ from the display language the webview goes by. `None` where it
/// cannot be read.
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
    locale_name(["LC_ALL", "LC_TIME", "LANG"].into_iter().filter_map(|key| std::env::var(key).ok()))
}

/// The first set POSIX locale, such as "pl_PL.UTF-8", as a locale name such as "pl-PL".
#[cfg(any(not(windows), test))]
fn locale_name(mut values: impl Iterator<Item = String>) -> Option<String> {
    let value = values.find(|value| !value.is_empty())?;
    let name = value.split(['.', '@']).next()?.replace('_', "-");
    (!name.is_empty() && name != "C" && name != "POSIX").then_some(name)
}

/// The DNS name under which an address has its PTR record.
#[cfg(any(windows, test))]
fn ptr_name(addr: IpAddr) -> String {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa")
        }
        IpAddr::V6(v6) => {
            let nibbles = v6.octets().into_iter().rev().map(|byte| format!("{:x}.{:x}.", byte & 15, byte >> 4));
            nibbles.collect::<String>() + "ip6.arpa"
        }
    }
}

/// The name in an address's PTR record, asked from DNS only.
///
/// The system's own reverse lookup (`getnameinfo`) also tries NetBIOS: for an address
/// without a PTR record it sends name queries to that address itself. A router answers
/// those with "port unreachable", and many routers send only one ICMP error a second, so
/// the answer to that second's probe was dropped and the hop showed packet loss.
#[cfg(windows)]
fn lookup_ptr(addr: IpAddr) -> Option<String> {
    use windows_sys::Win32::NetworkManagement::Dns::{
        DNS_QUERY_NO_MULTICAST, DNS_QUERY_NO_NETBT, DNS_RECORDW, DNS_TYPE_PTR, DnsFree,
        DnsFreeRecordList, DnsQuery_W,
    };
    let name: Vec<u16> = ptr_name(addr).encode_utf16().chain([0]).collect();
    let mut records: *mut DNS_RECORDW = std::ptr::null_mut();
    // SAFETY: `name` is zero-terminated. On success `records` is a list the API owns until
    // `DnsFree`; a PTR record's data is a pointer to a zero-terminated name.
    unsafe {
        let status = DnsQuery_W(
            name.as_ptr(),
            DNS_TYPE_PTR,
            DNS_QUERY_NO_NETBT | DNS_QUERY_NO_MULTICAST,
            std::ptr::null_mut(),
            (&raw mut records).cast(),
            std::ptr::null_mut(),
        );
        if status != 0 || records.is_null() {
            return None;
        }
        let mut found = None;
        let mut record = records;
        while !record.is_null() && found.is_none() {
            if (*record).wType == DNS_TYPE_PTR {
                let host = (*record).Data.PTR.pNameHost;
                let length = (0..).take_while(|&i| *host.add(i) != 0).count();
                found = Some(String::from_utf16_lossy(std::slice::from_raw_parts(host, length)));
            }
            record = (*record).pNext;
        }
        DnsFree(records.cast(), DnsFreeRecordList);
        found
    }
}

#[cfg(not(windows))]
fn lookup_ptr(addr: IpAddr) -> Option<String> {
    dns_lookup::lookup_addr(&addr)
        .ok()
        .filter(|name| *name != addr.to_string())
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
    lookup_ptr(addr)
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

/// Where releases are published: the app looks here for a newer version.
const UPDATE_REPO: &str = "knot2006/TracePlotter";
/// File names of a release's exe and of its signature.
const UPDATE_EXE: &str = "TracePlotter.exe";
const UPDATE_SIGNATURE: &str = "TracePlotter.exe.sig";
/// The public half of the release signing key, as `tauri signer generate` prints it. An
/// update is installed only if its signature was made with the private half, which stays
/// with the publisher. Being in control of the download server is not enough to forge one.
const UPDATE_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDJEREM1Mzc5NUUwQ0I4NTUKUldSVnVBeGVlVlBjTFh6QzNrRjJpM3hubngvcTRMOE4wV3RIaDdZNmFWUlRqTmJXRnhUY09VakUK";
/// Largest exe an update may be; the download stops there.
const UPDATE_MAX_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Serialize)]
struct UpdateInfo {
    version: String,
    notes: String,
}

struct Release {
    version: String,
    notes: String,
    exe_url: String,
    signature_url: String,
}

/// "1.2.3" or "v1.2.3" as numbers, to compare versions. `None` for anything else.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Whether `candidate` is a later version than `current`. Unreadable versions are not.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// Checks that `data` carries a valid signature by the key in `public_key` and returns
/// the app version the signature was made for. Both the key and the signature are in the
/// form the Tauri signer writes: base64 of a minisign file. The version is in the
/// signature's trusted comment, which the signature covers, so it cannot be changed
/// without the private key.
fn verify_update(data: &[u8], signature: &str, public_key: &str) -> Result<String, String> {
    use base64::Engine;
    let text = |encoded: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or("the signature is not readable")
    };
    let key = minisign_verify::PublicKey::decode(&text(public_key)?).map_err(|e| e.to_string())?;
    let signature =
        minisign_verify::Signature::decode(&text(signature)?).map_err(|e| e.to_string())?;
    key.verify(data, &signature, true)
        .map_err(|_| "the update's signature does not match; it was not installed".to_string())?;
    signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .map(|version| version.trim().to_string())
        .ok_or_else(|| "the update's signature names no version; it was not installed".into())
}

/// Requests a URL and returns the answer's body. Connecting and the wait for the answer
/// to start are kept short; the body gets ten minutes, so that a slow connection still
/// gets the exe.
fn http_get(url: &str) -> Result<ureq::Body, ureq::Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .build()
        .into();
    let response = agent
        .get(url)
        .header("User-Agent", concat!("TracePlotter/", env!("CARGO_PKG_VERSION")))
        .call()?;
    Ok(response.into_body())
}

/// Largest samples export that can be imported; four days at a one-second interval.
const IMPORT_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Asks the user for a samples export (.csv) and returns its file name and text, `None`
/// if cancelled. The webview reads it into rounds (see import.ts).
#[tauri::command(async)]
fn import_file(app: AppHandle) -> Result<Option<(String, String)>, String> {
    let mut dialog = app.dialog().file().add_filter("CSV", &["csv"]);
    if let Ok(downloads) = app.path().download_dir() {
        dialog = dialog.set_directory(downloads);
    }
    let Some(path) = dialog.blocking_pick_file() else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let cannot = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
    if fs::metadata(&path).map_err(cannot)?.len() > IMPORT_MAX_BYTES {
        return Err(format!("{} is too large to import", path.display()));
    }
    let text = fs::read_to_string(&path).map_err(cannot)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(Some((name, text)))
}

/// Saves the raw request body as the history of the new tab named in the `x-session`
/// header: one round per line, as `Recorder` writes them. Lines that are not rounds are
/// skipped when the history is read. A session that has a history keeps it.
#[tauri::command(async)]
fn import_history(request: Request<'_>) -> Result<(), String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("import needs a binary body".into());
    };
    let session = request
        .headers()
        .get("x-session")
        .and_then(|value| value.to_str().ok())
        .ok_or("import needs a session id")?;
    let path = history_path(session)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// The latest published release, `None` when there is none (or the repo is not public).
/// A release that is newer but lacks the exe or its signature is an error, not "none":
/// otherwise a broken release would read as "you have the latest version".
fn latest_release() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest");
    let mut response = match http_get(&url) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(format!("could not look for updates: {e}")),
    };
    let release: serde_json::Value = response.read_json().map_err(|e| e.to_string())?;
    let Some(tag) = release["tag_name"].as_str() else {
        return Ok(None);
    };
    let version = tag.trim_start_matches('v').to_string();
    let asset = |name: &str| {
        release["assets"].as_array()?.iter().find_map(|asset| {
            (asset["name"] == name).then(|| asset["browser_download_url"].as_str())?
        })
    };
    let (Some(exe_url), Some(signature_url)) = (asset(UPDATE_EXE), asset(UPDATE_SIGNATURE)) else {
        return if is_newer(&version, env!("CARGO_PKG_VERSION")) {
            Err(format!(
                "release {version} has no {UPDATE_EXE} with its {UPDATE_SIGNATURE}, so it cannot be installed from here"
            ))
        } else {
            Ok(None)
        };
    };
    Ok(Some(Release {
        version,
        notes: release["body"].as_str().unwrap_or_default().to_string(),
        exe_url: exe_url.to_string(),
        signature_url: signature_url.to_string(),
    }))
}

/// A newer release than the running version, if one is published.
#[tauri::command(async)]
fn check_update() -> Result<Option<UpdateInfo>, String> {
    Ok(latest_release()?
        .filter(|release| is_newer(&release.version, env!("CARGO_PKG_VERSION")))
        .map(|release| UpdateInfo {
            version: release.version,
            notes: release.notes,
        }))
}

/// Downloads the latest release, checks it, puts it in place of the running exe and
/// restarts into it. Tabs and history are untouched: they live in the data folder.
///
/// The download is accepted only if its signature is valid and was made for exactly the
/// version the release claims, which must be newer than this one. Without the version
/// check, an old signed exe could be offered again under a newer release name.
///
/// The running exe cannot be overwritten but can be renamed. The new exe is first written
/// next to it as `.exe.new`; then the running one is renamed to `.exe.old` (removed at the
/// next start) and the new one renamed into place. Only those two renames happen without
/// an exe at its usual path, and the first is undone if the second fails.
#[tauri::command(async)]
fn install_update(app: AppHandle) -> Result<(), String> {
    // Releases carry one exe, for Windows. Elsewhere the package manager or the user updates.
    if !cfg!(windows) {
        return Err("updates are installed from the release page outside Windows".into());
    }
    let release = latest_release()?.ok_or("no update is published")?;
    if !is_newer(&release.version, env!("CARGO_PKG_VERSION")) {
        return Err("this is already the latest version".into());
    }
    let download = |e: ureq::Error| format!("could not download the update: {e}");
    let signature = http_get(&release.signature_url)
        .map_err(download)?
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let mut exe = Vec::new();
    http_get(&release.exe_url)
        .map_err(download)?
        .into_reader()
        .take(UPDATE_MAX_BYTES)
        .read_to_end(&mut exe)
        .map_err(|e| format!("could not download the update: {e}"))?;
    let signed_version = verify_update(&exe, &signature, UPDATE_PUBLIC_KEY)?;
    if signed_version != release.version {
        return Err(format!(
            "the download is version {signed_version}, not the {} the release claims; it was not installed",
            release.version
        ));
    }

    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = current.with_extension("exe.new");
    let old = current.with_extension("exe.old");
    fs::write(&new, &exe).map_err(|e| format!("could not write the new exe: {e}"))?;
    let _ = fs::remove_file(&old);
    let swapped = fs::rename(&current, &old).and_then(|()| {
        fs::rename(&new, &current).inspect_err(|_| {
            let _ = fs::rename(&old, &current);
        })
    });
    if let Err(e) = swapped {
        let _ = fs::remove_file(&new);
        return Err(format!("could not replace the exe: {e}"));
    }
    // A restart does not pass through the window's close request, where this is saved.
    if let Some(window) = app.get_webview_window("main") {
        save_window_state(&window.as_ref().window());
    }
    app.restart()
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
        // Full path: this process is elevated when this works, so PATH must not pick the binary.
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

/// Whether the process may open raw sockets: on Windows with Administrator rights, which
/// firewall rules need as well, elsewhere with CAP_NET_RAW. In demo mode nothing is probed,
/// so it says yes.
#[tauri::command]
fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        demo() || unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        demo() || raw_sockets_allowed()
    }
}

/// Whether raw sockets may be opened outside Windows: as root, or with CAP_NET_RAW (bit 13
/// of the effective capabilities), which `setcap cap_net_raw+ep` gives the exe, as the
/// packages do when they install it and `restart_elevated` does on request. Where /proc
/// cannot be read, as on macOS, the answer is no.
#[cfg(not(windows))]
fn raw_sockets_allowed() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let caps = status.lines().find_map(|line| line.strip_prefix("CapEff:"))?;
            u64::from_str_radix(caps.trim(), 16).ok()
        })
        .is_some_and(|caps| caps & (1 << 13) != 0)
}

/// Switch that names the process a restarted one takes over from.
#[cfg(windows)]
const AFTER: &str = "--after";

/// Starts the exe again with Administrator rights, which Windows asks the user to allow,
/// and closes this one. Elsewhere it gives the exe CAP_NET_RAW through `pkexec setcap`,
/// which asks for an administrator's password, and restarts into it: the capability stays
/// with the file, so later starts have it too. Tabs and history are kept, as across any
/// restart.
#[tauri::command]
fn restart_elevated(app: AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |text: &std::ffi::OsStr| text.encode_wide().chain([0]).collect::<Vec<u16>>();
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // The switches this run was started with, and this process for the new one to wait for.
        let mut args: Vec<String> = std::env::args()
            .filter(|arg| arg == "--demo" || arg == "--mcp")
            .collect();
        args.extend([AFTER.to_string(), std::process::id().to_string()]);
        let started = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                wide("runas".as_ref()).as_ptr(),
                wide(exe.as_os_str()).as_ptr(),
                wide(args.join(" ").as_ref()).as_ptr(),
                std::ptr::null(),
                1, // SW_SHOWNORMAL
            )
        };
        // Anything up to 32 is an error; the usual one is the user saying no.
        if started as usize <= 32 {
            return Err("TracePlotter was not restarted: Administrator rights were not given".into());
        }
        // An exit does not pass through the window's close request, where this is saved.
        if let Some(window) = app.get_webview_window("main") {
            save_window_state(&window.as_ref().window());
        }
        app.exit(0);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        // An AppImage runs from a mount that ignores file capabilities.
        if std::env::var_os("APPIMAGE").is_some() {
            return Err("An AppImage cannot be allowed raw sockets. Use ICMP (ping socket), or install TracePlotter from a package.".into());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // pkexec wants a full path, and distributions keep setcap in different folders.
        let setcap = ["/usr/sbin/setcap", "/usr/bin/setcap", "/sbin/setcap"]
            .into_iter()
            .find(|path| Path::new(path).exists())
            .ok_or("setcap is not installed (on Debian and Ubuntu it is in libcap2-bin)")?;
        let status = std::process::Command::new("pkexec")
            .args([setcap, "cap_net_raw+ep"])
            .arg(&exe)
            .status()
            .map_err(|e| format!("cannot run pkexec: {e}"))?;
        if !status.success() {
            return Err(format!(
                "TracePlotter was not allowed raw sockets. Allow them with: sudo setcap cap_net_raw+ep {}",
                exe.display()
            ));
        }
        // A restart does not pass through the window's close request, where this is saved.
        if let Some(window) = app.get_webview_window("main") {
            save_window_state(&window.as_ref().window());
        }
        app.restart()
    }
}

/// Opens the project's GitHub page in the default browser.
#[tauri::command]
fn open_project_page() -> Result<(), String> {
    let url = format!("https://github.com/{UPDATE_REPO}");
    #[cfg(windows)]
    {
        let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let opened = unsafe {
            windows_sys::Win32::UI::Shell::ShellExecuteW(
                std::ptr::null_mut(),
                wide("open").as_ptr(),
                wide(&url).as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // SW_SHOWNORMAL
            )
        };
        if opened as usize <= 32 {
            return Err(format!("The browser did not open {url}"));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        std::process::Command::new(opener)
            .arg(&url)
            .spawn()
            .map(drop)
            .map_err(|e| format!("The browser did not open {url}: {e}"))
    }
}

/// After `restart_elevated`: waits for the instance this one replaces to close. Two
/// instances with different rights cannot share the webview's data folder.
// ponytail: waits for the app's process only. If its webview processes linger and the
// window fails to open, wait for the data folder's lock file as well.
#[cfg(windows)]
fn wait_for_predecessor() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    let pid = std::env::args()
        .skip_while(|arg| arg != AFTER)
        .nth(1)
        .and_then(|pid| pid.parse::<u32>().ok());
    let Some(pid) = pid else { return };
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, 10_000);
            CloseHandle(process);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    wait_for_predecessor();
    // WebKitGTK's DMA-BUF renderer breaks the Wayland connection with NVIDIA's driver
    // ("Error 71 (Protocol error) dispatching to Wayland display") and the app quits at
    // once. Without it the page is drawn through shared memory, which is fast enough here.
    #[cfg(target_os = "linux")]
    if Path::new("/sys/module/nvidia").exists()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        // SAFETY: no other thread is running yet to read the environment.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Traces::default())
        .manage(mcp::Mcp::default())
        // The window is created here, not in tauri.conf.json, to point the webview's
        // storage at the data folder. It stays hidden until it is placed where it was.
        .setup(|app| {
            // The exe an update replaced; it could not be deleted while it was running.
            if let Ok(exe) = std::env::current_exe() {
                let _ = fs::remove_file(exe.with_extension("exe.old"));
            }
            fs::create_dir_all(data_dir())?;
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("TracePlotter")
                .inner_size(1100.0, 720.0)
                .min_inner_size(640.0, 420.0)
                .data_directory(data_dir().join("webview"))
                // Tauri's own file-drop handling swallows drag and drop inside the page
                // on Windows, which the hop table uses to reorder its columns.
                .disable_drag_drop_handler()
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
            read_history,
            set_history_hours,
            delete_history,
            host_locale,
            reverse_dns,
            whois,
            export_file,
            import_file,
            import_history,
            check_update,
            install_update,
            allow_icmp_replies,
            is_elevated,
            restart_elevated,
            open_project_page,
            mcp::mcp_flag,
            mcp::mcp_serve,
            mcp::mcp_respond
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
    fn posix_locales_become_locale_names() {
        let name = |values: &[&str]| locale_name(values.iter().map(|v| v.to_string()));
        assert_eq!(name(&["", "pl_PL.UTF-8"]).as_deref(), Some("pl-PL"));
        assert_eq!(name(&["de_DE@euro"]).as_deref(), Some("de-DE"));
        assert_eq!(name(&["C.UTF-8"]), None);
        assert_eq!(name(&[]), None);
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
    fn ptr_names_reverse_the_address() {
        assert_eq!(ptr_name("192.168.1.20".parse().unwrap()), "20.1.168.192.in-addr.arpa");
        assert_eq!(
            ptr_name("2001:db8::567:89ab".parse().unwrap()),
            "b.a.9.8.7.6.5.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
        );
    }

    /// Live check, needs network: `cargo test --lib ptr -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_ptr_lookup_finds_a_name() {
        assert_eq!(lookup_ptr("8.8.8.8".parse().unwrap()).as_deref(), Some("dns.google"));
        assert_eq!(lookup_ptr("192.0.2.1".parse().unwrap()), None);
    }

    /// The version is kept in Cargo.toml only; tauri.conf.json has none, so the version the
    /// window shows (the frontend's `getVersion`) must be Cargo's, as the updater's is.
    #[test]
    fn the_app_reports_the_version_from_cargo_toml() {
        let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
        assert_eq!(context.package_info().version.to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn history_is_counted_in_rounds_of_the_interval_up_to_a_cap() {
        // At the 24 hours it starts with; no test changes them.
        assert_eq!(history_rounds(1000), 86_400);
        assert_eq!(history_rounds(10_000), 8640);
        assert_eq!(history_rounds(200), MAX_HISTORY);
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

    #[test]
    fn versions_compare_by_number_and_updates_need_a_valid_signature() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("latest", "0.1.0"));
        assert!(!is_newer("1.2", "0.1.0"));
        // The built-in key is readable, and a made-up signature is turned down.
        assert!(verify_update(b"exe", "bm90IGEgc2lnbmF0dXJl", UPDATE_PUBLIC_KEY).is_err());
        assert!(verify_update(b"exe", "", UPDATE_PUBLIC_KEY).is_err());
    }

    /// Live check, needs network: `cargo test --lib live_http -- --ignored`
    #[test]
    #[ignore]
    fn live_http_requests_work() {
        let url = "https://api.github.com/repos/rust-lang/rust/releases/latest";
        let release: serde_json::Value = http_get(url).unwrap().read_json().unwrap();
        assert!(release["tag_name"].is_string());
        // A missing page is an error with its status, which is how "no release" is told.
        let missing = http_get("https://api.github.com/repos/rust-lang/no-such-repo-here");
        assert!(matches!(missing, Err(ureq::Error::StatusCode(404))));
        assert!(latest_release().is_ok());
    }

    /// Checks a file signed with the release key against the built-in public key:
    /// set `TRACEPLOTTER_SIGNED_FILE` to a file that has its `.sig` next to it.
    #[test]
    #[ignore]
    fn a_file_signed_with_the_release_key_verifies() {
        let path = std::env::var("TRACEPLOTTER_SIGNED_FILE").unwrap();
        let data = fs::read(&path).unwrap();
        let signature = fs::read_to_string(format!("{path}.sig")).unwrap();
        verify_update(&data, &signature, UPDATE_PUBLIC_KEY).unwrap();
        let mut tampered = data.clone();
        tampered.push(0);
        assert!(verify_update(&tampered, &signature, UPDATE_PUBLIC_KEY).is_err());
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

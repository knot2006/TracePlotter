//! Probe workers and demo generation for the desktop app.

use super::*;

/// The demo route: address, name, usual latency in ms and share of probes lost per hop.
/// It crosses three oceans so that its hops show every latency colour, and the swell in
/// `demo_sample` moves the far ones across the limits. The addresses between the home
/// router and the target are from the ranges reserved for documentation (RFC 5737), so
/// they belong to nobody.
pub(super) const DEMO_ROUTE: [(&str, &str, f64, f64); 9] = [
    ("192.168.1.1", "router.home.arpa", 0.4, 0.0),
    ("10.20.0.1", "gw.access.example.net", 1.3, 0.0),
    ("198.51.100.17", "core1.waw.example.net", 4.1, 0.0),
    ("203.0.113.9", "ix1.fra.example.org", 18.0, 0.0),
    ("203.0.113.77", "edge1.nyc.example.org", 92.0, 0.0),
    ("192.0.2.33", "", 128.0, 0.35),
    ("192.0.2.120", "core1.lax.example.com", 165.0, 0.0),
    ("192.0.2.150", "", 172.0, 0.01),
    ("192.0.2.200", "www.syd.example.com", 245.0, 0.004),
];

/// Pseudo-random numbers in 0..1 for the demo (xorshift); no need for a crate.
pub(super) struct DemoRng(pub(super) u64);

impl DemoRng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// How one demo target's route differs from the others': the long-haul latency, the size,
/// length and timing of its swell, its rise in the busy hours of the day, and extra loss
/// at the target. Picked from the host name, so that tabs can be told apart and a host
/// always looks the same.
pub(super) struct DemoLook {
    scale: f64,
    swell: f64,
    period_ms: f64,
    phase: f64,
    busy: f64,
    busy_phase: f64,
    loss: f64,
}

pub(super) fn demo_look(host: &str) -> DemoLook {
    // FNV-1a, so that names that differ in one letter still look different.
    let seed = host.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let mut rng = DemoRng(seed | 1);
    rng.next();
    DemoLook {
        scale: 0.6 + 0.8 * rng.next(),
        swell: 0.05 + 0.25 * rng.next(),
        period_ms: 60_000.0 + 180_000.0 * rng.next(),
        phase: std::f64::consts::TAU * rng.next(),
        busy: 0.1 + 0.4 * rng.next(),
        busy_phase: std::f64::consts::TAU * rng.next(),
        loss: 0.02 * rng.next(),
    }
}

/// One made-up round at Unix time `t` in ms: steady near hops, a slow swell and the odd
/// spike shared by the hops beyond the provider's edge, and one hop that often stays silent.
pub(super) fn demo_sample(rng: &mut DemoRng, look: &DemoLook, t: u64) -> Sample {
    // Keep each random route for 30 seconds so that a change stays visible in Events.
    let mut route_rng =
        DemoRng((t / 30_000).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ look.phase.to_bits());
    let edge = ["203.0.113.77", "203.0.113.78", "203.0.113.79"][(route_rng.next() * 3.0) as usize];
    let day = std::f64::consts::TAU * (t % DAY_MS) as f64 / DAY_MS as f64;
    let swell = 1.0
        + look.swell * (t as f64 / look.period_ms + look.phase).sin().max(0.0)
        + look.busy * (day + look.busy_phase).sin().max(0.0);
    let spike = if rng.next() < 0.02 {
        15.0 + 45.0 * rng.next()
    } else {
        0.0
    };
    let hops = DEMO_ROUTE
        .iter()
        .enumerate()
        .map(|(i, (addr, _, usual, loss))| {
            let addr = if i == 4 { edge } else { addr };
            let far = i >= 2;
            // Beyond the first ocean the route is the target's own.
            let usual = if i >= 4 { usual * look.scale } else { *usual };
            let rtt = usual * if far { swell } else { 1.0 }
                + usual * 0.2 * rng.next()
                + if far { spike } else { 0.0 };
            let target = i == DEMO_ROUTE.len() - 1;
            let lost = rng.next() < loss + if target { look.loss } else { 0.0 };
            HopSample {
                ttl: i as u8 + 1,
                addr: (!lost).then(|| addr.parse().unwrap()),
                rtt: (!lost).then(|| (rtt * 100.0).round() / 100.0),
            }
        })
        .collect();
    Sample { t, hops }
}

/// How much made-up history a new demo tab starts with, in ms: `--demo-days N` days, to
/// try long periods, or else a quarter of an hour.
fn demo_history_ms() -> u64 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|arg| arg == "--demo-days")
        .and_then(|i| args.get(i + 1)?.parse::<u64>().ok())
        .map_or(900_000, |days| days.clamp(1, 365) * DAY_MS)
}

/// `start_trace` in demo mode: a tab with made-up history (see `demo_history_ms`) that
/// keeps getting made-up rounds. Nothing is sent on the network.
fn start_demo(
    host: &str,
    session: &str,
    interval_ms: u64,
    with_history: bool,
    channel: EventSink,
    traces: &Traces,
    stop: Arc<AtomicBool>,
) -> Result<Started, String> {
    let interval_ms = interval_ms.clamp(100, 60_000);
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    };
    let mut rng = DemoRng(now() | 1);
    let look = demo_look(host);
    let dir = history_dir(session)?;
    if with_history && day_files(&dir).is_empty() {
        let start = now();
        let mut writer = HistoryWriter::new(dir.clone());
        for ago in (1..=demo_history_ms() / interval_ms).rev() {
            if stop.load(Ordering::Relaxed) {
                return Err("Trace cancelled".into());
            }
            let sample = demo_sample(&mut rng, &look, start - ago * interval_ms);
            if let Ok(line) = serde_json::to_string(&sample) {
                writer.append(sample.t, &line).map_err(|e| e.to_string())?;
            }
        }
    }
    let history = if with_history {
        load_history(&dir, history_rounds(interval_ms))
    } else {
        Vec::new()
    };
    let mut writer = HistoryWriter::new(dir);
    let id = traces.next_id.fetch_add(1, Ordering::Relaxed);
    if stop.load(Ordering::Relaxed) {
        return Err("Trace cancelled".into());
    }
    traces.stops.lock().unwrap().insert(id, stop.clone());
    let worker = std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(interval_ms));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let sample = demo_sample(&mut rng, &look, now());
            if let Ok(line) = serde_json::to_string(&sample) {
                let _ = writer.append(sample.t, &line);
            }
            if channel.send(TraceEvent::Sample(sample)).is_err() {
                return;
            }
        }
    });
    traces.workers.lock().unwrap().insert(id, worker);
    let (target, ..) = DEMO_ROUTE[DEMO_ROUTE.len() - 1];
    Ok(Started {
        id,
        addr: target.parse().unwrap(),
        history,
    })
}

/// Where the rounds of a running trace go: its history files and the frontend.
struct Recorder {
    channel: EventSink,
    history: Mutex<HistoryWriter>,
    /// The tab shows an error; the next recorded round clears it.
    failing: AtomicBool,
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
            .and_then(|line| self.history.lock().unwrap().append(t, &line));
        if let Err(e) = saved {
            self.error(format!("History is not being saved: {e}"));
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

/// The settings needed by the probe engine, independent of the window.
pub(super) struct ProbeConfig {
    pub cancel: Option<Arc<AtomicBool>>,
    pub session: String,
    pub host: String,
    pub protocol: String,
    pub port: u16,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub packet_size: u16,
    pub with_history: bool,
}

/// Delivers probe events without requiring a webview or a render frame.
#[derive(Clone)]
pub(super) struct EventSink(Arc<dyn Fn(TraceEvent) -> bool + Send + Sync>);

impl EventSink {
    pub fn new(send: impl Fn(TraceEvent) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    fn send(&self, event: TraceEvent) -> Result<(), ()> {
        (self.0)(event).then_some(()).ok_or(())
    }
}

pub(super) fn start(
    config: ProbeConfig,
    channel: EventSink,
    traces: &Traces,
) -> Result<Started, String> {
    let ProbeConfig {
        cancel,
        session,
        host,
        protocol,
        port,
        interval_ms,
        timeout_ms,
        packet_size,
        with_history,
    } = config;

    let stop = cancel.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    if stop.load(Ordering::Relaxed) {
        return Err("Trace cancelled".into());
    }
    if demo() {
        return start_demo(
            &host,
            &session,
            interval_ms,
            with_history,
            channel,
            traces,
            stop,
        );
    }
    let host = host.trim().to_lowercase();
    let addr = (host.as_str(), 0)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {host}"))?
        .ip();
    // Ping sockets need the user's group within net.ipv4.ping_group_range. Where that is
    // shut, ICMP goes through a raw socket instead if TracePlotter may open one.
    #[cfg(target_os = "linux")]
    let protocol = if protocol == "winicmp" && !pingsock::available(addr) && raw_sockets_allowed() {
        "icmp".to_string()
    } else {
        protocol
    };
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
    let interval = Duration::from_millis(interval_ms.clamp(100, 60_000));
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

    let dir = history_dir(&session)?;
    let history = if with_history {
        load_history(&dir, history_rounds(interval.as_millis() as u64))
    } else {
        Vec::new()
    };

    if stop.load(Ordering::Relaxed) {
        return Err("Trace cancelled".into());
    }
    traces.stops.lock().unwrap().insert(id, stop.clone());
    // A silent round lasts the timeout. The rebuild should come after about DEAD_ROUNDS
    // intervals whatever the timeout, so a long timeout does not keep a dead tracer for long.
    let dead_rounds = (DEAD_ROUNDS as u128 * interval.as_millis())
        .div_ceil(timeout.as_millis())
        .max(1) as usize;
    let recorder = Recorder {
        channel,
        history: Mutex::new(HistoryWriter::new(dir)),
        failing: AtomicBool::new(false),
    };

    let worker = std::thread::spawn(move || match build {
        Some(build) => trace_raw(build, tracer, dead_rounds, addr, &stop, &recorder),
        #[cfg(any(windows, target_os = "linux"))]
        None => trace_echo(addr, packet_size, timeout, interval, &stop, &recorder),
        #[cfg(not(any(windows, target_os = "linux")))]
        None => {}
    });

    traces.workers.lock().unwrap().insert(id, worker);
    Ok(Started { id, addr, history })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_start_never_resolves_or_opens_history() {
        let traces = Traces::default();
        let stop = Arc::new(AtomicBool::new(true));
        let result = start(
            ProbeConfig {
                cancel: Some(stop),
                session: "cancelled".into(),
                host: "not-a-host.invalid".into(),
                protocol: "icmp".into(),
                port: 0,
                interval_ms: 1000,
                timeout_ms: 1000,
                packet_size: 84,
                with_history: true,
            },
            EventSink::new(|_| panic!("a cancelled start sent an event")),
            &traces,
        );
        assert!(matches!(result,Err(e) if e=="Trace cancelled"));
        assert!(traces.stops.lock().unwrap().is_empty());
        assert!(traces.workers.lock().unwrap().is_empty());
    }
    #[test]
    fn stopping_returns_the_worker_so_erasure_can_wait() {
        let traces = Traces::default();
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
        });
        traces.stops.lock().unwrap().insert(1, stop);
        traces.workers.lock().unwrap().insert(1, worker);
        stop_probe(1, &traces)
            .expect("worker is registered")
            .join()
            .unwrap();
        assert!(stop_probe(1, &traces).is_none());
    }
}

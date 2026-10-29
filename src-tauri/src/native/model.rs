//! Trace state and native session documents; painting has no role in applying samples.

use super::*;
use crate::logic::{Settings, Stats, stats};
use serde_json::{Value, json};
use std::collections::VecDeque;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Config {
    pub session_id: String,
    pub host: String,
    pub protocol: String,
    pub port: u16,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub packet_size: u16,
    pub alert_latency_ms: u64,
    pub alert_loss_pct: u64,
    pub alert_seconds: u64,
    pub paused: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            host: String::new(),
            protocol: "icmp".into(),
            port: 0,
            interval_ms: 1000,
            timeout_ms: 1000,
            packet_size: 84,
            alert_latency_ms: 0,
            alert_loss_pct: 0,
            alert_seconds: 30,
            paused: false,
        }
    }
}
impl Config {
    pub fn new(host: String, s: &Settings) -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            host: host.trim().to_lowercase(),
            protocol: s.protocol.clone(),
            port: match s.protocol.as_str() {
                "tcp" => 443,
                "udp" => 33434,
                _ => 0,
            },
            interval_ms: s.interval_ms,
            timeout_ms: s.timeout_ms,
            packet_size: s.packet_size,
            alert_latency_ms: s.alert_latency_ms,
            alert_loss_pct: s.alert_loss_pct,
            alert_seconds: s.alert_seconds,
            paused: false,
        }
    }
    pub fn validate(&mut self) -> Result<(), String> {
        self.host = self.host.trim().to_lowercase();
        if self.host.is_empty() {
            return Err("a target needs a host".into());
        }
        if self.session_id.is_empty()
            || file_safe(&self.session_id) != self.session_id
            || [".", ".."].contains(&self.session_id.as_str())
        {
            return Err("invalid session id".into());
        }
        if !["icmp", "winicmp", "udp", "tcp"].contains(&self.protocol.as_str()) {
            return Err(format!("unknown protocol {}", self.protocol));
        }
        self.interval_ms = self.interval_ms.clamp(100, 60000);
        self.timeout_ms = self.timeout_ms.clamp(100, 10000);
        self.packet_size = self.packet_size.clamp(48, 1024);
        self.alert_seconds = self.alert_seconds.clamp(1, 3600);
        self.alert_latency_ms = self.alert_latency_ms.min(60000);
        self.alert_loss_pct = self.alert_loss_pct.min(100);
        Ok(())
    }
    pub fn probe(&self, with_history: bool) -> ProbeConfig {
        ProbeConfig {
            cancel: None,
            session: self.session_id.clone(),
            host: self.host.clone(),
            protocol: self.protocol.clone(),
            port: self.port,
            interval_ms: self.interval_ms,
            timeout_ms: self.timeout_ms,
            packet_size: self.packet_size,
            with_history,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Session {
    pub version: u32,
    pub settings: Settings,
    pub targets: Vec<Config>,
    pub active: String,
    pub hidden_columns: Vec<String>,
    pub column_order: Vec<String>,
    pub column_widths: BTreeMap<String, f32>,
    pub table_height: Option<f32>,
    pub span: f64,
    pub compare: Vec<String>,
    pub check_updates: bool,
    pub mcp: bool,
}
impl Default for Session {
    fn default() -> Self {
        let mut settings = Settings::default();
        if cfg!(any(windows, target_os = "linux")) {
            settings.protocol = "winicmp".into();
        }
        Self {
            version: 1,
            settings,
            targets: Vec::new(),
            active: "summary".into(),
            hidden_columns: vec!["mos".into()],
            column_order: vec![],
            column_widths: BTreeMap::new(),
            table_height: None,
            span: 600.0,
            compare: vec![],
            check_updates: true,
            mcp: false,
        }
    }
}
impl Session {
    pub fn load() -> Result<Self, String> {
        let path = data_dir().join("native-state.json");
        match fs::read_to_string(&path) {
            Ok(text) => Self::decode(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }
    pub fn decode(text: &str) -> Result<Self, String> {
        let mut state: Self =
            serde_json::from_str(text).map_err(|e| format!("invalid native session: {e}"))?;
        if state.version != 1 {
            return Err(format!(
                "unsupported native session version {}",
                state.version
            ));
        }
        let mut ids = HashSet::new();
        for target in &mut state.targets {
            target.validate()?;
            if !ids.insert(target.session_id.clone()) {
                return Err("duplicate session id".into());
            }
        }
        state.settings = crate::logic::clean_settings(
            &serde_json::to_value(&state.settings).map_err(|e| e.to_string())?,
            cfg!(any(windows, target_os = "linux")),
        );
        if !state.span.is_finite() || state.span < 0.0 {
            return Err("invalid time span".into());
        }
        Ok(state)
    }
    pub fn save(&self) -> Result<(), String> {
        self.save_in(data_dir())
    }
    fn save_in(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join("native-state.json");
        let mut stored = self.clone();
        if stored.active == "settings" {
            stored.active = stored
                .targets
                .first()
                .map_or_else(|| "summary".into(), |t| t.session_id.clone());
        }
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?;
        // Validate before replacing a good session, including values changed by the UI.
        Self::decode(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)?;
        if path.exists() {
            fs::copy(&path, path.with_extension("json.bak"))
                .map_err(|e| format!("cannot back up session: {e}"))?;
        }
        atomic_write(&path, &bytes)
    }
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("json.new");
    let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(not(windows))]
    fs::rename(&temp, path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: both paths are valid, zero-terminated UTF-16 buffers for this call.
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Hop {
    pub ttl: u8,
    pub addr: Option<IpAddr>,
    pub name: Option<String>,
    pub times: Vec<f64>,
    pub rtts: Vec<Option<f64>>,
}
impl Hop {
    fn record(&mut self, t: f64, rtt: Option<f64>, max: usize) {
        self.times.push(t);
        self.rtts.push(rtt);
        if self.times.len() >= max + 1000 {
            let count = self.times.len() - max;
            self.times.drain(..count);
            self.rtts.drain(..count);
        }
    }
    pub fn bounds(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        self.times.partition_point(|t| *t < from)..self.times.partition_point(|t| *t <= to)
    }
    pub fn stats(&self, from: f64, to: f64) -> Stats {
        stats(&self.rtts[self.bounds(from, to)])
    }
}

#[derive(Clone)]
pub(super) struct Trace {
    pub config: Config,
    pub paused: bool,
    pub addr: Option<IpAddr>,
    pub run: Option<u32>,
    pub generation: u64,
    pub cancel: Option<Arc<AtomicBool>>,
    pub loaded: bool,
    pub error: Option<String>,
    pub hops: Vec<Hop>,
    pub target: Hop,
    pub path_length: u8,
    pub path_end: u8,
    lengths: VecDeque<(u8, bool)>,
    pub events: Vec<Value>,
    route_notified: f64,
    pub alerting: bool,
    pub disk_start: Option<u64>,
}
impl Trace {
    pub fn new(config: Config) -> Self {
        Self {
            paused: config.paused,
            config,
            addr: None,
            run: None,
            generation: 0,
            cancel: None,
            loaded: false,
            error: None,
            hops: vec![],
            target: Hop::default(),
            path_length: 0,
            path_end: 0,
            lengths: VecDeque::new(),
            events: vec![],
            route_notified: 0.0,
            alerting: false,
            disk_start: None,
        }
    }
    pub fn restore(&mut self, replay: Self) {
        self.hops = replay.hops;
        self.target = replay.target;
        self.path_length = replay.path_length;
        self.path_end = replay.path_end;
        self.lengths = replay.lengths;
        self.events = replay.events;
        self.disk_start = replay.disk_start;
        self.loaded = true;
    }
    pub fn period(&self, span: f64) -> (f64, f64) {
        let to = self.target.times.last().copied().unwrap_or(0.0);
        let held = self.target.times.first().copied().unwrap_or(0.0);
        let from = if span > 0.0 {
            to - span
        } else {
            self.disk_start
                .map_or(held, |t| held.min(t as f64 / 1000.0))
        };
        (from, to)
    }
    pub fn apply(&mut self, sample: Sample, live: bool) -> Vec<Value> {
        self.disk_start.get_or_insert(sample.t);
        let t = sample.t as f64 / 1000.0;
        let mut changes = vec![];
        let last = sample.hops.last();
        if let Some(last) = last.filter(|h| h.rtt.is_some()) {
            if self.path_length != 0 && last.ttl != self.path_length {
                changes.push(json!({"t":t,"kind":"length","from":self.path_length,"to":last.ttl}));
            }
            self.path_length = last.ttl;
            self.target.ttl = last.ttl;
        }
        if sample.hops.iter().any(|h| h.rtt.is_some()) {
            self.lengths.push_back((
                sample.hops.iter().map(|h| h.ttl).max().unwrap_or(0),
                last.is_some_and(|h| h.rtt.is_some()),
            ));
            if self.lengths.len() > 10 {
                self.lengths.pop_front();
            }
            let settled =
                self.lengths.len() >= 2 && self.lengths.iter().rev().take(2).all(|(_, r)| *r);
            self.path_end = self
                .lengths
                .iter()
                .rev()
                .take(if settled { 2 } else { 10 })
                .map(|(n, _)| *n)
                .max()
                .unwrap_or(0);
        }
        let max = history_rounds(self.config.interval_ms);
        self.target.record(t, last.and_then(|h| h.rtt), max);
        for probe in sample.hops {
            if probe.ttl == 0 {
                continue;
            }
            while self.hops.len() < usize::from(probe.ttl) {
                self.hops.push(Hop {
                    ttl: (self.hops.len() + 1) as u8,
                    ..Hop::default()
                });
            }
            let hop = &mut self.hops[usize::from(probe.ttl) - 1];
            if let Some(addr) = probe.addr.filter(|a| Some(*a) != hop.addr) {
                if let Some(old) = hop.addr {
                    changes.push(json!({"t":t,"kind":"hop","ttl":hop.ttl,"from":old,"to":addr}));
                }
                hop.addr = Some(addr);
                hop.name = None;
            }
            hop.record(t, probe.rtt, max);
        }
        if live {
            self.error = None;
            self.check_alert(t, &mut changes);
        }
        for e in &changes {
            self.events.push(e.clone());
        }
        if self.events.len() > 500 {
            self.events.drain(..self.events.len() - 500);
        }
        changes
    }
    fn check_alert(&mut self, t: f64, changes: &mut Vec<Value>) {
        let probes = crate::logic::alert_probes(self.config.alert_seconds, self.config.interval_ms);
        let s = stats(&self.target.rtts[self.target.rtts.len().saturating_sub(probes)..]);
        let alert = crate::logic::breaches(
            &self.target.rtts,
            probes,
            self.config.alert_latency_ms as f64,
            self.config.alert_loss_pct as f64,
        );
        if alert == self.alerting {
            return;
        }
        self.alerting = alert;
        let text = if alert {
            format!(
                "Last {} probes: {}, {:.0}% loss",
                s.sent,
                s.avg
                    .map_or_else(|| "no replies".into(), |v| format!("{v:.1} ms average")),
                s.loss_pct
            )
        } else {
            "Back within alert thresholds".into()
        };
        changes.push(json!({"t":t,"kind":if alert {"alert"} else {"recover"},"text":text}));
    }
    pub fn notification(&mut self, changes: &[Value]) -> Option<String> {
        if let Some(e) = changes.iter().find(|e| e["kind"] == "alert") {
            return e["text"].as_str().map(str::to_owned);
        }
        let routes: Vec<_> = changes
            .iter()
            .filter(|e| e["kind"] == "hop" || e["kind"] == "length")
            .collect();
        let e = *routes.first()?;
        let t = e["t"].as_f64()?;
        if t - self.route_notified < 60.0 {
            return None;
        }
        self.route_notified = t;
        let mut text = if e["kind"] == "hop" {
            format!(
                "Hop {} changed: {} → {}",
                e["ttl"],
                e["from"].as_str().unwrap_or_default(),
                e["to"].as_str().unwrap_or_default()
            )
        } else {
            format!("Path length changed: {} → {} hops", e["from"], e["to"])
        };
        if routes.len() > 1 {
            text.push_str(&format!(" (+{} more)", routes.len() - 1));
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(t: u64, rtts: &[Option<f64>]) -> Sample {
        Sample {
            t,
            hops: rtts
                .iter()
                .enumerate()
                .map(|(i, rtt)| HopSample {
                    ttl: (i + 1) as u8,
                    addr: rtt.map(|_| "192.0.2.1".parse().unwrap()),
                    rtt: *rtt,
                })
                .collect(),
        }
    }
    #[test]
    fn route_shortening_needs_two_completed_rounds() {
        let mut trace = Trace::new(Config::default());
        trace.apply(sample(1000, &[Some(1.0), Some(2.0), Some(3.0)]), false);
        trace.apply(sample(2000, &[Some(1.0), Some(2.0)]), false);
        assert_eq!(trace.path_end, 3);
        trace.apply(sample(3000, &[Some(1.0), Some(2.0)]), false);
        assert_eq!(trace.path_end, 2);
        assert_eq!(trace.target.rtts, vec![Some(3.0), Some(2.0), Some(2.0)]);
    }
    #[test]
    fn replay_does_not_raise_alerts_but_live_samples_do() {
        let mut trace = Trace::new(Config {
            alert_loss_pct: 50,
            alert_seconds: 1,
            ..Config::default()
        });
        assert!(trace.apply(sample(1000, &[None]), false).is_empty());
        assert!(!trace.alerting);
        let changes = trace.apply(sample(2000, &[None]), true);
        assert_eq!(changes[0]["kind"], "alert");
        assert!(trace.alerting);
        assert_eq!(
            trace
                .apply(sample(3000, &[Some(1.0)]), true)
                .last()
                .unwrap()["kind"],
            "recover"
        );
    }
    #[test]
    fn duplicate_hosts_keep_distinct_session_ids() {
        let a = Config::new("example.com".into(), &Settings::default());
        let b = Config::new("example.com".into(), &Settings::default());
        assert_ne!(a.session_id, b.session_id);
    }
    #[test]
    fn invalid_session_is_not_reset_silently() {
        assert!(Session::decode("broken").is_err());
        assert!(Session::decode(r#"{"version":99}"#).is_err());
        assert!(
            Session::decode(r#"{"targets":[{"sessionId":"..","host":"example.com"}]}"#).is_err()
        );
    }
    #[test]
    fn session_writes_keep_a_backup_and_reject_bad_replacements() {
        let dir =
            std::env::temp_dir().join(format!("traceplotter-session-{}", uuid::Uuid::new_v4()));
        let mut state = Session::default();
        state.save_in(&dir).unwrap();
        let path = dir.join("native-state.json");
        let first = fs::read(&path).unwrap();
        state.span = 3600.0;
        state.save_in(&dir).unwrap();
        assert_eq!(fs::read(path.with_extension("json.bak")).unwrap(), first);
        let good = fs::read(&path).unwrap();
        state.span = f64::NAN;
        assert!(state.save_in(&dir).is_err());
        assert_eq!(fs::read(&path).unwrap(), good);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn a_new_trace_remembers_its_first_recorded_sample_for_older_history() {
        let mut trace = Trace::new(Config::default());
        trace.apply(sample(1000, &[Some(10.0)]), true);
        trace.apply(sample(2000, &[Some(12.0)]), true);
        assert_eq!(trace.disk_start, Some(1000));
        assert_eq!(trace.period(0.0), (1.0, 2.0));
    }
}

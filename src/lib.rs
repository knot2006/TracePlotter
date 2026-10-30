use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpStream, ToSocketAddrs, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trippy_core::{Builder, Port, PortDirection, ProbeStatus, Protocol, Round, Tracer};

#[cfg(any(windows, target_os = "linux"))]
mod echo;
mod logic;
mod mcp;
mod native;
mod probe;
use probe::*;
mod insights;
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

/// Days of history a tab keeps on disk, every round of them. The Settings tab changes it
/// through `set_history_days`.
static HISTORY_DAYS: AtomicU64 = AtomicU64::new(7);
/// Hours of the newest rounds a tab loads into the window. Older periods are read from
/// the history files when they are looked at (`history_range`).
const WINDOW_HOURS: u64 = 24;
/// Most rounds a tab loads into the window, whatever its interval.
const MAX_HISTORY: usize = 345_600;

/// How many rounds a tab loads: WINDOW_HOURS of them at its interval, up to MAX_HISTORY.
fn history_rounds(interval_ms: u64) -> usize {
    let rounds = WINDOW_HOURS * 3_600_000 / interval_ms.max(1);
    (rounds as usize).clamp(1, MAX_HISTORY)
}

/// Sets how many days of history tabs keep. Older days go when a trace starts a new day
/// and when a history is next read.
fn set_history_days(days: u64) {
    HISTORY_DAYS.store(days.clamp(1, 365), Ordering::Relaxed);
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

#[derive(Default, Clone)]
struct Traces {
    next_id: Arc<AtomicU32>,
    stops: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    workers: Arc<Mutex<HashMap<u32, std::thread::JoinHandle<()>>>>,
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

const DAY_MS: u64 = 86_400_000;

/// The history files of a tab, oldest first, with their days. A tab's history is a folder
/// with one file of rounds per UTC day, named by the day's number since 1970, so that old
/// rounds go by deleting whole files and a long history never has to be rewritten.
fn day_files(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut days: Vec<(u64, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let day = path.file_stem()?.to_str()?.parse().ok()?;
            (path.extension()? == "jsonl").then_some((day, path))
        })
        .collect();
    days.sort();
    days
}

/// Unix time in ms.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The oldest day a tab keeps, of its day files (oldest first): the kept days counted
/// back from its newest day, so that an imported capture or a tab paused for a long time
/// keeps its last days. From today instead when the newest day lies after it: one round
/// saved while the clock was set ahead must not make every real day look old.
fn first_kept_day(days: &[(u64, PathBuf)]) -> Option<u64> {
    let newest = days.last()?.0.min(now_ms() / DAY_MS);
    Some(newest.saturating_sub(HISTORY_DAYS.load(Ordering::Relaxed)))
}

/// Deletes the days of a tab's history before the oldest one it keeps.
fn trim_days(dir: &Path) {
    let days = day_files(dir);
    let Some(first) = first_kept_day(&days) else {
        return;
    };
    for (day, path) in days {
        if day < first {
            let _ = fs::remove_file(path);
        }
    }
}

/// The time of a history line, read off its start where `Recorder` puts it, without
/// parsing the rest. Lines written another way are parsed; lines without one are `None`.
fn line_time(line: &str) -> Option<u64> {
    line.strip_prefix(r#"{"t":"#)
        .and_then(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .or_else(|| {
            serde_json::from_str::<Sample>(line)
                .ok()
                .map(|sample| sample.t)
        })
}

/// Appends rounds to a tab's history, each to the file of its day. Starting a new day
/// drops the days that have fallen out of the kept days.
struct HistoryWriter {
    dir: PathBuf,
    day: Option<(u64, fs::File)>,
}

impl HistoryWriter {
    fn new(dir: PathBuf) -> Self {
        Self { dir, day: None }
    }

    fn append(&mut self, t: u64, line: &str) -> std::io::Result<()> {
        let day = t / DAY_MS;
        if self.day.as_ref().is_none_or(|(open, _)| *open != day) {
            let path = self.dir.join(format!("{day}.jsonl"));
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            let started = self.day.is_some();
            self.day = Some((day, file));
            if started {
                trim_days(&self.dir);
            }
        }
        let (_, file) = self.day.as_ref().unwrap();
        writeln!(&*file, "{line}")
    }
}

/// Appends history lines, oldest first, each to the file of its day. Lines without a
/// time are dropped.
fn write_lines<'a>(dir: &Path, lines: impl IntoIterator<Item = &'a str>) -> std::io::Result<()> {
    let mut writer = HistoryWriter::new(dir.to_path_buf());
    for line in lines {
        if let Some(t) = line_time(line) {
            writer.append(t, line)?;
        }
    }
    Ok(())
}

/// Reads the newest `max` rounds of a tab's history, after dropping the days that have
/// fallen out of it. Only the newest days that hold them are read. Unreadable lines are skipped.
fn load_history(dir: &Path, max: usize) -> Vec<Sample> {
    trim_days(dir);
    let mut texts = Vec::new();
    let mut count = 0;
    for (_, path) in day_files(dir).iter().rev() {
        if count >= max {
            break;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        count += text.lines().count();
        texts.push(text);
    }
    let lines: Vec<&str> = texts.iter().rev().flat_map(|text| text.lines()).collect();
    lines[lines.len().saturating_sub(max)..]
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The probes of one hop over a stretch of time. Stretches add up, so a long period is
/// counted from the tallies of its minutes instead of from every round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Tally {
    sent: u32,
    received: u32,
    /// Sum of the replies' times, for their average.
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
    /// Sum and number of the differences between consecutive replies, for jitter.
    jitter_sum: f64,
    jitter_count: u32,
    /// The latest probe: its time, or `None` when it was lost.
    last: Option<f64>,
}

impl Tally {
    /// Counts a probe; `previous` is the hop's reply before it, for jitter.
    fn add(&mut self, rtt: Option<f64>, previous: Option<f64>) {
        self.sent += 1;
        self.last = rtt;
        let Some(rtt) = rtt else {
            return;
        };
        self.received += 1;
        self.sum += rtt;
        self.min = Some(self.min.map_or(rtt, |min| min.min(rtt)));
        self.max = Some(self.max.map_or(rtt, |max| max.max(rtt)));
        if let Some(previous) = previous {
            self.jitter_sum += (rtt - previous).abs();
            self.jitter_count += 1;
        }
    }

    /// Adds the tally of a later stretch.
    fn merge(&mut self, later: &Tally) {
        let pick = |a: Option<f64>, b: Option<f64>, f: fn(f64, f64) -> f64| match (a, b) {
            (Some(a), Some(b)) => Some(f(a, b)),
            (a, b) => a.or(b),
        };
        self.sent += later.sent;
        self.received += later.received;
        self.sum += later.sum;
        self.min = pick(self.min, later.min, f64::min);
        self.max = pick(self.max, later.max, f64::max);
        self.jitter_sum += later.jitter_sum;
        self.jitter_count += later.jitter_count;
        if later.sent > 0 {
            self.last = later.last;
        }
    }
}

/// Tallies per hop number, 0 being the round trip, and per stretch by its start in Unix ms.
type Tallies = HashMap<u8, BTreeMap<u64, Tally>>;

/// Tallies the rounds of a history file from `from` to before `to` in stretches of `width`
/// ms. The round trip is the final hop of each round, lost when it did not answer, as
/// the window counts it.
fn tally_rounds(text: &str, width: u64, from: u64, to: u64) -> Tallies {
    let mut tallies = Tallies::new();
    tally_into(&mut tallies, &mut HashMap::new(), text, width, from, to);
    tallies
}

/// `tally_rounds` into existing tallies. `previous` holds each hop's latest reply, for
/// jitter, and carries it over to the next piece of the same file.
fn tally_into(
    tallies: &mut Tallies,
    previous: &mut HashMap<u8, f64>,
    text: &str,
    width: u64,
    from: u64,
    to: u64,
) {
    // A round's time is read before the rest of its line, so rounds outside the period
    // cost next to nothing: a short period is often read from a whole day's file.
    let inside = |line: &&str| line_time(line).is_some_and(|t| (from..to).contains(&t));
    for sample in text
        .lines()
        .filter(inside)
        .filter_map(|line| serde_json::from_str::<Sample>(line).ok())
    {
        let start = sample.t / width * width;
        let reply = sample.hops.last().and_then(|hop| hop.rtt);
        for (ttl, rtt) in sample
            .hops
            .iter()
            .map(|hop| (hop.ttl, hop.rtt))
            .chain([(0, reply)])
        {
            let before = previous.get(&ttl).copied();
            tallies
                .entry(ttl)
                .or_default()
                .entry(start)
                .or_default()
                .add(rtt, before);
            if let Some(rtt) = rtt {
                previous.insert(ttl, rtt);
            }
        }
    }
}

const MINUTE_MS: u64 = 60_000;

/// The minute tallies of one day file, and how far into the file they reach. A finished
/// day is read once, and today's file only from where the last look stopped, so that
/// a long period can be looked at again and again for little.
#[derive(Default)]
struct DayTally {
    read: u64,
    tallies: Tallies,
    previous: HashMap<u8, f64>,
}

impl DayTally {
    /// Tallies the whole lines added to the file since the last call.
    fn catch_up(&mut self, path: &Path) {
        let Ok(mut file) = fs::File::open(path) else {
            return;
        };
        let len = file.metadata().map_or(0, |meta| meta.len());
        // A shorter file was written anew.
        if len < self.read {
            *self = Self::default();
        }
        if len == self.read {
            return;
        }
        let mut bytes = Vec::new();
        let read = file
            .seek(SeekFrom::Start(self.read))
            .and_then(|_| (&mut file).take(len - self.read).read_to_end(&mut bytes));
        if read.is_err() {
            return;
        }
        // The last line may still be being written; it waits for the next call.
        let whole = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |i| i + 1);
        let text = String::from_utf8_lossy(&bytes[..whole]);
        tally_into(
            &mut self.tallies,
            &mut self.previous,
            &text,
            MINUTE_MS,
            0,
            u64::MAX,
        );
        self.read += whole as u64;
    }
}

/// The minute tallies of the day files looked at so far, by file.
// ponytail: about 1 MB per day of a 1 s trace, kept until its file is gone. Drop the
// least recently used days if long histories of many tabs take too much memory.
static DAY_TALLIES: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<DayTally>>>>> = OnceLock::new();

/// Adds tallies of `step`-ms stretches into stretches of `width` ms, a multiple of
/// `step`, keeping those that overlap the period from `from` to before `to`.
fn rebucket(into: &mut Tallies, tallies: &Tallies, step: u64, width: u64, from: u64, to: u64) {
    for (ttl, stretches) in tallies {
        let hop = into.entry(*ttl).or_default();
        for (start, tally) in stretches.range(from.saturating_sub(step - 1)..to) {
            hop.entry(start / width * width).or_default().merge(tally);
        }
    }
}

/// One stretch of a hop's graph: its start in Unix ms and its tally.
#[derive(Clone, Serialize, Deserialize)]
struct Stretch {
    t: u64,
    #[serde(flatten)]
    tally: Tally,
}

/// A hop over a period of its history: its tally over the whole period, and for a hop
/// with a graph, its stretches.
#[derive(Clone, Serialize, Deserialize)]
struct RangeHop {
    ttl: u8,
    total: Tally,
    stretches: Vec<Stretch>,
}

/// A period of a tab's history, and where the history begins (Unix ms, `None` when empty).
#[derive(Clone, Serialize, Deserialize)]
struct Range {
    start: Option<u64>,
    hops: Vec<RangeHop>,
}

/// What a tab's history holds from `from` to before `to` (Unix ms), per hop number (0 for
/// the round trip): a tally over the whole period, and for the hops in `graphed` one per
/// `width` ms. The day files the period touches are read by as many threads as there are
/// cores. With a width of whole minutes they are tallied by the minute and kept (see
/// `DayTally`); such stretches can reach up to a minute past the period's ends.
fn history_range(
    session: String,
    from: u64,
    to: u64,
    width: u64,
    graphed: Vec<u8>,
) -> Result<Range, String> {
    let dir = history_dir(&session)?;
    let width = width.max(1);
    let by_minute = width.is_multiple_of(MINUTE_MS);
    let all = day_files(&dir);
    let start = all.first().and_then(|(_, path)| first_time(path));
    let days: Vec<PathBuf> = all
        .into_iter()
        .filter(|(day, _)| (from / DAY_MS..=to / DAY_MS).contains(day))
        .map(|(_, path)| path)
        .collect();
    let cache = DAY_TALLIES.get_or_init(Default::default);
    cache.lock().unwrap().retain(|path, _| path.exists());

    let tally_day = |path: &Path| -> Tallies {
        if !by_minute {
            return tally_rounds(
                &fs::read_to_string(path).unwrap_or_default(),
                width,
                from,
                to,
            );
        }
        let day = cache
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_default()
            .clone();
        let mut day = day.lock().unwrap();
        day.catch_up(path);
        let mut tallies = Tallies::new();
        rebucket(&mut tallies, &day.tallies, MINUTE_MS, width, from, to);
        tallies
    };
    // A pool rather than a thread per day: each thread holds a whole day file in memory.
    let next = AtomicUsize::new(0);
    let parts: Mutex<Vec<Option<Tallies>>> = Mutex::new(vec![None; days.len()]);
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(days.len());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = days.get(i) else {
                        return;
                    };
                    let tallies = tally_day(path);
                    parts.lock().unwrap()[i] = Some(tallies);
                }
            });
        }
    });

    // Days in order, so that each hop's last probe is the latest one.
    let mut hops: BTreeMap<u8, RangeHop> = BTreeMap::new();
    for part in parts.into_inner().unwrap().into_iter().flatten() {
        let mut ttls: Vec<&u8> = part.keys().collect();
        ttls.sort();
        for ttl in ttls {
            let hop = hops.entry(*ttl).or_insert(RangeHop {
                ttl: *ttl,
                total: Tally::default(),
                stretches: Vec::new(),
            });
            for (t, tally) in &part[ttl] {
                hop.total.merge(tally);
                if !graphed.contains(ttl) {
                    continue;
                }
                match hop.stretches.last_mut() {
                    Some(stretch) if stretch.t == *t => stretch.tally.merge(tally),
                    _ => hop.stretches.push(Stretch {
                        t: *t,
                        tally: *tally,
                    }),
                }
            }
        }
    }
    Ok(Range {
        start,
        hops: hops.into_values().collect(),
    })
}

/// The time of the first round in a history file.
fn first_time(path: &Path) -> Option<u64> {
    let file = fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line_time(&line))
}

/// When a tab's history begins, in Unix ms: the first round of its oldest day file.
/// `None` for a tab without history.
fn history_start(session: String) -> Result<Option<u64>, String> {
    let dir = history_dir(&session)?;
    Ok(day_files(&dir)
        .first()
        .and_then(|(_, path)| first_time(path)))
}

/// The events file of a tab: its route changes, alerts and recoveries, one JSON object
/// per line as the window describes them, `t` in Unix seconds. It keeps the events of
/// the rounds the window no longer holds.
fn events_path(session: &str) -> Result<PathBuf, String> {
    Ok(history_dir(session)?.join("events.jsonl"))
}

/// Adds an event, one line of JSON, to a tab's events file.
fn append_event(session: String, event: String) -> Result<(), String> {
    if event.contains('\n') {
        return Err("an event is one line".into());
    }
    let path = events_path(&session)?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{event}"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// A tab's saved events, oldest first. Those from before the oldest day its history keeps
/// are dropped from the file on the way.
fn read_events(session: String) -> Result<Vec<String>, String> {
    let path = events_path(&session)?;
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let days = day_files(path.parent().unwrap_or(Path::new(".")));
    let first_ms = first_kept_day(&days).map_or(0, |day| day * DAY_MS);
    let kept: Vec<String> = text
        .lines()
        .filter(|line| line_time(line).is_some_and(|t| t * 1000 >= first_ms))
        .map(str::to_owned)
        .collect();
    if kept.len() < text.lines().count() {
        let _ = fs::write(
            &path,
            kept.iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        );
    }
    Ok(kept)
}

/// A Unix time in ms as an ISO 8601 UTC time, the way the window writes them:
/// 2026-10-16T12:00:00.000Z.
fn iso_time(ms: u64) -> String {
    // Days to a civil date, after Howard Hinnant's `civil_from_days`.
    let z = (ms / DAY_MS) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rest = ms % DAY_MS;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

/// Whether the app was started with `--demo`. Every tab then shows a made-up route with
/// documentation-only addresses instead of probing the network, for screenshots and for
/// working on the interface. Demo data lives in its own folder, apart from real history.
fn demo() -> bool {
    static DEMO: OnceLock<bool> = OnceLock::new();
    *DEMO.get_or_init(|| std::env::args().any(|arg| arg == "--demo"))
}

/// The folder for everything the app stores: trace history, open tabs, settings and the
/// window's size. On Windows it sits next to the exe, not in the
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
        // A relative XDG_DATA_HOME is to be ignored, as the XDG spec says.
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")))
            .unwrap_or_else(|| ".".into());
        base.join(if demo() {
            "TracePlotter-demo-data"
        } else {
            "TracePlotter-data"
        })
    })
}

/// The history folder of one tab, and the single file earlier versions kept its
/// history in. Neither is created.
fn history_paths(session: &str) -> Result<(PathBuf, PathBuf), String> {
    let name = file_safe(session);
    if name.is_empty() {
        return Err("missing session id".into());
    }
    let root = data_dir().join("history");
    Ok((root.join(&name), root.join(format!("{name}.jsonl"))))
}

/// The history folder of one tab, created on demand. A history still in one file, as
/// earlier versions kept it, is split into days first.
fn history_dir(session: &str) -> Result<PathBuf, String> {
    let (dir, single) = history_paths(session)?;
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    if let Ok(text) = fs::read_to_string(&single) {
        write_lines(&dir, text.lines())
            .map_err(|e| format!("cannot split {}: {e}", single.display()))?;
        let _ = fs::remove_file(&single);
    }
    Ok(dir)
}

/// Size, position and maximized state of the window, kept between launches.
/// Position and size are in physical pixels, of the window while not maximized.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
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

/// A tab's saved history without starting its trace, for a tab that is restored paused.
fn read_history(session: String, interval_ms: u64) -> Result<Vec<Sample>, String> {
    Ok(load_history(
        &history_dir(&session)?,
        history_rounds(interval_ms),
    ))
}

/// Erases a tab's saved history. A trace still writing to it loses those rounds too.
fn delete_history(session: String) -> Result<(), String> {
    let (dir, single) = history_paths(&session)?;
    let gone = |result: std::io::Result<()>, path: &Path| match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot delete {}: {e}", path.display()))
        }
        _ => Ok(()),
    };
    gone(fs::remove_dir_all(&dir), &dir)?;
    gone(fs::remove_file(&single), &single)
}

fn stop_probe(id: u32, traces: &Traces) -> Option<std::thread::JoinHandle<()>> {
    if let Some(stop) = traces.stops.lock().unwrap().remove(&id) {
        stop.store(true, Ordering::Relaxed);
    }
    traces.workers.lock().unwrap().remove(&id)
}

/// The locale of the host's regional format (how it writes dates and times), such as
/// "pl-PL". This is the Windows "Regional format" setting, or elsewhere the locale of
/// LC_TIME. `None` where it cannot be read.
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
    locale_name(
        ["LC_ALL", "LC_TIME", "LANG"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok()),
    )
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
            let nibbles = v6
                .octets()
                .into_iter()
                .rev()
                .map(|byte| format!("{:x}.{:x}.", byte & 15, byte >> 4));
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
                found = Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                    host, length,
                )));
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
        .header(
            "User-Agent",
            concat!("TracePlotter/", env!("CARGO_PKG_VERSION")),
        )
        .call()?;
    Ok(response.into_body())
}

/// Largest samples export that can be imported; four days at a one-second interval.
const IMPORT_MAX_BYTES: u64 = 512 * 1024 * 1024;

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
fn prepare_update() -> Result<(), String> {
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
    Ok(())
}

/// Adds the inbound Windows Firewall rules that let ICMP replies reach raw sockets:
/// echo reply, destination unreachable and time exceeded, for IPv4 and IPv6.
/// Without them the firewall drops the replies and hops show as lost.
/// Also removes the rules added under the app's former name, PingScape.
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
            let caps = status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:"))?;
            u64::from_str_radix(caps.trim(), 16).ok()
        })
        .is_some_and(|caps| caps & (1 << 13) != 0)
}

/// Whether `restart_elevated` can give the app raw sockets. An AppImage runs from a mount
/// that ignores file capabilities, so it cannot.
fn can_elevate() -> bool {
    std::env::var_os("APPIMAGE").is_none()
}

/// Switch that names the process a restarted one takes over from.
const AFTER: &str = "--after";

/// The switches this run was started with, and this process for the new one to wait for.
fn restart_args() -> Vec<String> {
    let mut args = Vec::new();
    let mut original = std::env::args().skip(1);
    while let Some(arg) = original.next() {
        match arg.as_str() {
            "--demo" | "--mcp" => args.push(arg),
            "--demo-days" => {
                args.push(arg);
                if let Some(days) = original.next() {
                    args.push(days);
                }
            }
            _ => {}
        }
    }
    args.extend([AFTER.to_string(), std::process::id().to_string()]);
    args
}

/// Starts the exe again with Administrator rights, which Windows asks the user to allow,
/// and closes this one. Elsewhere it gives the exe CAP_NET_RAW through `pkexec setcap`,
/// which asks for an administrator's password, and restarts into it: the capability stays
/// with the file, so later starts have it too. Tabs and history are kept, as across any
/// restart. Called from a worker so the window keeps drawing while the prompt waits for an answer.
fn spawn_elevated() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |text: &std::ffi::OsStr| text.encode_wide().chain([0]).collect::<Vec<u16>>();
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let args = restart_args();
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
            return Err(
                "TracePlotter was not restarted: Administrator rights were not given".into(),
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if !can_elevate() {
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
        // The new instance waits for this one to close, as on Windows.
        std::process::Command::new(&exe)
            .args(restart_args())
            .spawn()
            .map_err(|e| {
                format!("TracePlotter was allowed raw sockets but did not restart: {e}")
            })?;
        Ok(())
    }
}

/// Opens the project's GitHub page in the default browser.
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
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(&url)
            .spawn()
            .map(drop)
            .map_err(|e| format!("The browser did not open {url}: {e}"))
    }
}

/// After `restart_elevated`: waits for the instance this one replaces to close. Two
/// instances must not write the same session and history files at once.
fn wait_for_predecessor() {
    let pid = std::env::args()
        .skip_while(|arg| arg != AFTER)
        .nth(1)
        .and_then(|pid| pid.parse::<u32>().ok());
    let Some(pid) = pid else { return };
    #[cfg(not(windows))]
    {
        // The process is gone when its /proc entry is.
        let entry = Path::new("/proc").join(pid.to_string());
        let start = std::time::Instant::now();
        while entry.exists() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, 10_000);
            CloseHandle(process);
        }
    }
}

/// Starts the egui desktop app.
pub fn run() {
    native::run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use trippy_core::{CompletionReason, Flags, Probe, RoundId, Sequence, TimeToLive, TraceId};

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
        let round = Round::new(
            &probes,
            TimeToLive(2),
            CompletionReason::RoundTimeLimitExceeded,
        );
        let lost = |ttl| HopSample {
            ttl,
            addr: None,
            rtt: None,
        };
        assert_eq!(round_to_hops(&round), vec![lost(1), lost(2)]);
    }

    #[test]
    fn ptr_names_reverse_the_address() {
        assert_eq!(
            ptr_name("192.168.1.20".parse().unwrap()),
            "20.1.168.192.in-addr.arpa"
        );
        assert_eq!(
            ptr_name("2001:db8::567:89ab".parse().unwrap()),
            "b.a.9.8.7.6.5.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
        );
    }

    /// Live check, needs network: `cargo test --lib ptr -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_ptr_lookup_finds_a_name() {
        assert_eq!(
            lookup_ptr("8.8.8.8".parse().unwrap()).as_deref(),
            Some("dns.google")
        );
        assert_eq!(lookup_ptr("192.0.2.1".parse().unwrap()), None);
    }

    #[test]
    fn history_is_counted_in_rounds_of_the_interval_up_to_a_cap() {
        // 24 hours of rounds, up to the cap.
        assert_eq!(history_rounds(1000), 86_400);
        assert_eq!(history_rounds(10_000), 8640);
        assert_eq!(history_rounds(200), MAX_HISTORY);
    }

    #[test]
    fn history_is_kept_in_day_files_and_loads_its_newest_rounds() {
        let dir = std::env::temp_dir().join(format!("traceplotter-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let line =
            |t: u64| format!(r#"{{"t":{t},"hops":[{{"ttl":1,"addr":"10.0.0.1","rtt":1.5}}]}}"#);
        // Ten days, two rounds each, with a bad line among them. 7 days are kept.
        let day = |d: u64| 20_000 * DAY_MS + d * DAY_MS;
        let mut lines: Vec<String> = (0..10)
            .flat_map(|d| [line(day(d)), line(day(d) + 1000)])
            .collect();
        lines.insert(5, "garbage".into());
        write_lines(&dir, lines.iter().map(String::as_str)).unwrap();
        // A writer starting a new day drops the days that fell out of the kept hours.
        assert_eq!(
            day_files(&dir)
                .iter()
                .map(|(d, _)| d - 20_000)
                .collect::<Vec<_>>(),
            [2, 3, 4, 5, 6, 7, 8, 9]
        );

        let loaded = load_history(&dir, 3);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            loaded.iter().map(|s| s.t).collect::<Vec<_>>(),
            [day(8) + 1000, day(9), day(9) + 1000]
        );
        assert_eq!(loaded[0].hops[0].rtt, Some(1.5));
        assert_eq!(file_safe("..\\a/b:c"), ".._a_b_c");
    }

    #[test]
    fn tallies_count_hops_and_the_round_trip_by_stretch_and_add_up() {
        let round = |t: u64, rtts: [Option<f64>; 2]| {
            serde_json::to_string(&Sample {
                t,
                hops: rtts
                    .iter()
                    .enumerate()
                    .map(|(i, rtt)| HopSample {
                        ttl: i as u8 + 1,
                        addr: None,
                        rtt: *rtt,
                    })
                    .collect(),
            })
            .unwrap()
        };
        let text = [
            round(0, [Some(1.0), Some(10.0)]),
            round(1000, [Some(3.0), None]),
            round(2000, [Some(2.0), Some(14.0)]),
            round(3000, [None, Some(12.0)]),
        ]
        .join("\n");
        let tallies = tally_rounds(&text, 2000, 0, u64::MAX);
        let hop1 = &tallies[&1];
        assert_eq!(hop1[&0].sent, 2);
        assert_eq!(hop1[&0].sum, 4.0);
        assert_eq!(hop1[&0].jitter_sum, 2.0);
        assert_eq!(hop1[&2000].last, None);
        // The round trip is the final hop; its lost probe counts and jitter skips it.
        let trip = &tallies[&0];
        assert_eq!((trip[&0].sent, trip[&0].received), (2, 1));
        assert_eq!(trip[&2000].jitter_sum, 6.0);

        let mut whole = Tally::default();
        for tally in trip.values() {
            whole.merge(tally);
        }
        assert_eq!(
            (whole.sent, whole.received, whole.min, whole.max),
            (4, 3, Some(10.0), Some(14.0))
        );
        assert_eq!(whole.last, Some(12.0));
        assert_eq!(
            tally_rounds(&text, 2000, 1000, 3000)[&0]
                .values()
                .map(|t| t.sent)
                .sum::<u32>(),
            2
        );
    }

    #[test]
    fn a_day_is_tallied_by_the_minute_as_its_file_grows() {
        let path =
            std::env::temp_dir().join(format!("traceplotter-day-{}.jsonl", std::process::id()));
        let line = |t: u64, rtt: f64| {
            format!(r#"{{"t":{t},"hops":[{{"ttl":1,"addr":null,"rtt":{rtt}}}]}}"#)
        };
        // The second line is not finished yet: no newline.
        fs::write(
            &path,
            format!("{}\n{}", line(1_000, 10.0), &line(61_000, 30.0)[..10]),
        )
        .unwrap();
        let mut day = DayTally::default();
        day.catch_up(&path);
        assert_eq!(day.tallies[&1].len(), 1);
        fs::write(
            &path,
            format!("{}\n{}\n", line(1_000, 10.0), line(61_000, 30.0)),
        )
        .unwrap();
        day.catch_up(&path);
        let hop = &day.tallies[&1];
        assert_eq!((hop[&0].sent, hop[&60_000].sent), (1, 1));
        // Jitter carries over from the earlier piece of the file.
        assert_eq!(hop[&60_000].jitter_sum, 20.0);
        let mut five = Tallies::new();
        rebucket(&mut five, &day.tallies, MINUTE_MS, 300_000, 0, u64::MAX);
        assert_eq!(five[&1][&0].sent, 2);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_day_file_dated_after_today_does_not_expire_the_real_ones() {
        let today = now_ms() / DAY_MS;
        let days = |list: &[u64]| {
            list.iter()
                .map(|d| (*d, PathBuf::new()))
                .collect::<Vec<_>>()
        };
        assert_eq!(first_kept_day(&days(&[today - 3, today])), Some(today - 7));
        assert_eq!(
            first_kept_day(&days(&[today - 3, today + 1000])),
            Some(today - 7)
        );
        // An old capture keeps its own last days.
        assert_eq!(first_kept_day(&days(&[20_000, 20_009])), Some(20_002));
    }

    #[test]
    fn times_are_written_as_iso_utc() {
        assert_eq!(iso_time(1_000_000), "1970-01-01T00:16:40.000Z");
        assert_eq!(iso_time(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_time(1_792_181_234_567), "2026-10-16T20:07:14.567Z");
    }

    #[test]
    fn a_line_knows_its_time_however_it_was_written() {
        assert_eq!(
            line_time(r#"{"t":1700000000123,"hops":[]}"#),
            Some(1_700_000_000_123)
        );
        assert_eq!(line_time(r#"{"hops":[],"t":5}"#), Some(5));
        assert_eq!(line_time("garbage"), None);
    }

    #[test]
    fn demo_rounds_cover_the_route_with_documentation_addresses() {
        let mut rng = DemoRng(1);
        let look = demo_look("demo.example");
        let rounds: Vec<Sample> = (0..200)
            .map(|i| demo_sample(&mut rng, &look, i * 1000))
            .collect();
        assert!(
            rounds
                .iter()
                .all(|round| round.hops.len() == DEMO_ROUTE.len())
        );
        assert!(rounds.iter().all(|round| {
            round
                .hops
                .iter()
                .all(|hop| hop.rtt.is_none_or(|rtt| rtt > 0.0))
        }));
        // The silent hop loses a fair share, the target next to nothing.
        let lost = |ttl: u8| {
            rounds
                .iter()
                .filter(|r| r.hops[ttl as usize - 1].rtt.is_none())
                .count()
        };
        assert!((40..110).contains(&lost(6)), "hop 6 lost {}", lost(6));
        assert!(lost(9) < 10);
    }

    #[test]
    fn demo_route_changes_stay_put_between_events() {
        let mut rng = DemoRng(1);
        let look = demo_look("demo.example");
        let rounds: Vec<Sample> = (0..600)
            .map(|i| demo_sample(&mut rng, &look, i * 1000))
            .collect();
        assert!(
            rounds
                .windows(2)
                .any(|pair| pair[0].hops[4].addr != pair[1].hops[4].addr)
        );
        for pair in rounds.windows(2) {
            if pair[0].t / 30_000 == pair[1].t / 30_000 {
                assert_eq!(pair[0].hops[4].addr, pair[1].hops[4].addr);
            }
        }
        let target: IpAddr = DEMO_ROUTE[8].0.parse().unwrap();
        assert!(
            rounds
                .iter()
                .all(|round| round.hops[8].addr.is_none_or(|addr| addr == target))
        );
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
                    replies.fetch_add(
                        hops.iter().filter(|h| h.rtt.is_some()).count(),
                        Ordering::Relaxed,
                    );
                })
                .unwrap();
            assert!(replies.into_inner() > 0, "{protocol:?}: no replies");
        }
    }
}

//! A controller that keeps probing, alerts and MCP independent of render frames.

use super::*;
use crate::insights::{Detail, read_details};
use crate::logic::{self, Extras, GraphData, Stats};
use model::{Config, Hop, Session, Trace};
use serde_json::{Value, json};
use std::sync::RwLock;
use std::sync::mpsc::{self, Receiver, SyncSender};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct View {
    pub session: String,
    pub hops: Vec<u8>,
    pub end: Option<f64>,
    pub width: usize,
}
#[derive(Clone)]
pub(super) struct Row {
    pub ttl: u8,
    pub addr: String,
    pub name: String,
    pub stats: Stats,
    /// Median, percentiles and loss bursts, when a detailed column is shown; `None` while
    /// older history is still being read.
    pub extras: Option<Extras>,
}
#[derive(Clone)]
pub(super) struct TraceView {
    pub config: Config,
    pub paused: bool,
    pub addr: String,
    pub loading: bool,
    pub error: Option<String>,
    pub alerting: bool,
    pub rows: Vec<Row>,
    pub total: Stats,
    pub total_extras: Option<Extras>,
    /// The period the detailed columns were read for, when it reaches past the samples in
    /// memory; `None` while that read is pending.
    pub details_period: Option<Option<(f64, f64)>>,
    pub spark: Vec<Option<f64>>,
    pub events: Vec<Value>,
    pub from: f64,
    pub to: f64,
}
#[derive(Clone)]
pub(super) struct PlotView {
    pub session: String,
    pub ttl: u8,
    pub label: String,
    pub data: GraphData,
    pub width: f64,
    pub stretches: Vec<Stretch>,
}
#[derive(Clone, Default)]
pub(super) struct Snapshot {
    pub session: Session,
    pub traces: Vec<TraceView>,
    pub plots: Vec<PlotView>,
    pub error: Option<String>,
    pub mcp_url: Option<String>,
    pub plot_range: [f64; 2],
    pub history_loading: bool,
}

pub(super) struct Loaded {
    started: Started,
    replay: Option<Box<Trace>>,
}
impl Loaded {
    fn read(mut started: Started, config: &Config, history: bool) -> Self {
        let replay = history.then(|| {
            let mut trace = Trace::new(config.clone());
            for sample in std::mem::take(&mut started.history) {
                trace.apply(sample, false);
            }
            trace.disk_start = history_start(config.session_id.clone())
                .ok()
                .flatten()
                .or(trace.disk_start);
            let first = trace.target.times.first().copied().unwrap_or(f64::INFINITY);
            let saved = read_events(config.session_id.clone())
                .unwrap_or_default()
                .iter()
                .filter_map(|s| serde_json::from_str::<Value>(s).ok())
                .collect::<Vec<_>>();
            let mut events = logic::restore_events(&saved, &trace.events, first);
            if events.len() > 500 {
                events.drain(..events.len() - 500);
            }
            trace.events = events;
            trace.loaded = true;
            Box::new(trace)
        });
        Self { started, replay }
    }
}

/// Exact statistics for a period older than memory: its scope, the period and its hops.
type Details = (String, (f64, f64), Vec<Detail>);
#[derive(Clone, PartialEq)]
pub(super) struct RangeKey {
    id: String,
    generation: u64,
    from: u64,
    to: u64,
    width: u64,
    graphed: Vec<u8>,
    live: bool,
}
impl RangeKey {
    fn compatible(&self, other: &Self) -> bool {
        self.id == other.id
            && self.generation == other.generation
            && self.live == other.live
            && self.from.abs_diff(other.from) <= if self.live { 10000 } else { 0 }
            && self.to.abs_diff(other.to) <= if self.live { 10000 } else { 0 }
            && self.width == other.width
            && self.graphed == other.graphed
    }
}

pub(super) enum Command {
    Open(Config),
    Pause(String),
    Close(String),
    Restart(String),
    Edit(Config),
    Preferences(Box<Session>),
    Select(String),
    Compare(Vec<String>),
    View(View),
    Agent(String, Value, SyncSender<Result<Value, String>>),
    Started(String, u64, Result<Loaded, String>),
    Sample(String, u64, TraceEvent),
    Name(IpAddr, Option<String>),
    Range(RangeKey, Result<Range, String>),
    Details(String, String, (f64, f64), Result<Vec<Detail>, String>),
    Failure(String),
    Save,
    ClearError,
    Shutdown,
}

pub(super) struct Runtime {
    pub send: mpsc::Sender<Command>,
    pub snapshot: Arc<RwLock<Arc<Snapshot>>>,
    worker: Option<std::thread::JoinHandle<()>>,
    forwarder: Option<std::thread::JoinHandle<()>>,
}
impl Runtime {
    pub fn start(session: Session, ctx: eframe::egui::Context) -> Self {
        let mcp_choice = session.mcp;
        let (send, receive) = mpsc::sync_channel(1024);
        let snapshot = Arc::new(RwLock::new(Arc::new(Snapshot {
            session: session.clone(),
            ..Snapshot::default()
        })));
        let (outgoing, pending) = mpsc::channel();
        let forward = send.clone();
        let forwarder = std::thread::spawn(move || {
            for command in pending {
                let shutdown = matches!(command, Command::Shutdown);
                if forward.send(command).is_err() || shutdown {
                    break;
                }
            }
        });
        let mut runtime = Self {
            send: outgoing,
            snapshot: Arc::clone(&snapshot),
            worker: None,
            forwarder: Some(forwarder),
        };
        runtime.worker = Some(std::thread::spawn(move || {
            Controller {
                session,
                mcp_choice,
                traces: vec![],
                probes: Traces::default(),
                send,
                receive,
                snapshot,
                ctx,
                view: None,
                mcp: mcp::Mcp::default(),
                mcp_url: None,
                error: None,
                names: HashMap::new(),
                pending: HashMap::new(),
                new_targets: HashSet::new(),
                failed_ranges: HashMap::new(),
                stopped: HashMap::new(),
                ranges: HashMap::new(),
                range_pending: HashSet::new(),
                details: HashMap::new(),
                details_pending: HashSet::new(),
            }
            .run()
        }));
        runtime
    }
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn fixture(snapshot: Snapshot) -> (Self, Receiver<Command>) {
        let (send, receive) = mpsc::channel();
        (
            Self {
                send,
                snapshot: Arc::new(RwLock::new(Arc::new(snapshot))),
                worker: None,
                forwarder: None,
            },
            receive,
        )
    }
    pub fn read(&self) -> Arc<Snapshot> {
        self.snapshot
            .read()
            .expect("snapshot lock poisoned")
            .clone()
    }
    pub fn command(&self, command: Command) {
        let _ = self.send.send(command);
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.send.send(Command::Shutdown);
        if let Some(forwarder) = self.forwarder.take() {
            let _ = forwarder.join();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Controller {
    session: Session,
    mcp_choice: bool,
    traces: Vec<Trace>,
    probes: Traces,
    send: SyncSender<Command>,
    receive: Receiver<Command>,
    snapshot: Arc<RwLock<Arc<Snapshot>>>,
    ctx: eframe::egui::Context,
    view: Option<View>,
    mcp: mcp::Mcp,
    mcp_url: Option<String>,
    error: Option<String>,
    names: HashMap<IpAddr, (Option<String>, u64)>,
    pending: HashMap<String, (Value, SyncSender<Result<Value, String>>)>,
    new_targets: HashSet<String>,
    failed_ranges: HashMap<String, RangeKey>,
    stopped: HashMap<String, std::thread::JoinHandle<()>>,
    ranges: HashMap<String, (RangeKey, Range)>,
    range_pending: HashSet<String>,
    details: HashMap<String, Details>,
    details_pending: HashSet<String>,
}
impl Controller {
    fn run(mut self) {
        set_history_days(self.session.settings.history_days);
        for config in self.session.targets.clone() {
            self.open(config);
        }
        self.save();
        if self.session.mcp || mcp::mcp_flag() {
            self.session.mcp = true;
            self.set_mcp();
        }
        self.publish();
        while let Ok(command) = self.receive.recv() {
            if matches!(command, Command::Shutdown) {
                break;
            }
            match command {
                Command::Open(config) => {
                    self.session.active = config.session_id.clone();
                    self.view = None;
                    if !config.paused {
                        self.new_targets.insert(config.session_id.clone());
                    }
                    self.open(config);
                    self.save();
                }
                Command::Pause(id) => {
                    if let Some(i) = self.index(&id) {
                        self.pause(i);
                        self.save();
                    }
                }
                Command::Close(id) => {
                    if let Some(i) = self.index(&id) {
                        let loading = !self.traces[i].loaded
                            || (!self.traces[i].paused && self.traces[i].run.is_none());
                        self.stop(i);
                        self.traces.remove(i);
                        if !loading {
                            self.erase_history(id.clone());
                        }
                        self.session.compare.retain(|s| s != &id);
                        if self.session.active == id {
                            self.view = None;
                            self.session.active = self
                                .traces
                                .get(i)
                                .or_else(|| i.checked_sub(1).and_then(|i| self.traces.get(i)))
                                .map_or_else(|| "summary".into(), |t| t.config.session_id.clone());
                        }
                        self.save();
                    }
                }
                Command::Restart(id) => {
                    if let Some(i) = self.index(&id) {
                        let loading = !self.traces[i].loaded
                            || (!self.traces[i].paused && self.traces[i].run.is_none());
                        self.stop(i);
                        let mut config = self.traces.remove(i).config;
                        if !loading {
                            self.erase_history(id.clone());
                        }
                        let compared = self.session.compare.contains(&id);
                        self.session.compare.retain(|s| s != &id);
                        config.session_id = uuid::Uuid::new_v4().to_string();
                        config.paused = false;
                        self.session.active = config.session_id.clone();
                        self.view = None;
                        if compared {
                            self.session.compare.push(config.session_id.clone());
                        }
                        self.open(config);
                        if let Some(trace) = self.traces.pop() {
                            self.traces.insert(i, trace);
                        }
                        self.save();
                    }
                }
                Command::Edit(mut config) => {
                    if let Err(e) = config.validate() {
                        self.error = Some(e);
                    } else if let Some(i) = self.index(&config.session_id) {
                        self.stop(i);
                        self.traces[i].config = config;
                        if !self.traces[i].paused {
                            self.launch(i);
                        }
                        self.save();
                    }
                }
                Command::Preferences(s) => {
                    let previous = (self.session.mcp, self.session.settings.mcp_port);
                    self.session.theme = s.theme;
                    self.session.transparency = s.transparency;
                    self.session.settings = s.settings;

                    self.session.hidden_columns = s.hidden_columns;
                    self.session.column_order = s.column_order;
                    self.session.column_widths = s.column_widths;
                    self.session.table_height = s.table_height;
                    self.session.span = s.span;

                    self.session.check_updates = s.check_updates;
                    self.session.show_events = s.show_events;
                    if self.session.mcp != s.mcp {
                        self.mcp_choice = s.mcp;
                    }
                    self.session.mcp = s.mcp;
                    set_history_days(self.session.settings.history_days);
                    if previous != (self.session.mcp, self.session.settings.mcp_port) {
                        self.set_mcp();
                    }
                    self.save();
                }
                Command::Select(id) => {
                    if ["summary", "settings"].contains(&id.as_str()) || self.index(&id).is_some() {
                        if self.session.active != id {
                            self.view = None;
                        }
                        self.session.active = id;
                        self.save();
                    }
                }
                Command::Compare(ids) => {
                    let mut seen = HashSet::new();
                    self.session.compare = ids
                        .into_iter()
                        .filter(|id| self.index(id).is_some() && seen.insert(id.clone()))
                        .collect();
                    self.save();
                }
                Command::View(view) => self.view = Some(view),
                Command::Started(id, generation, result) => self.started(&id, generation, result),
                Command::Sample(id, generation, event) => self.sample(&id, generation, event),
                Command::Name(addr, name) => {
                    self.names.insert(addr, (name.clone(), now_ms()));
                    for trace in &mut self.traces {
                        for hop in &mut trace.hops {
                            if hop.addr == Some(addr) {
                                hop.name = name.clone();
                            }
                        }
                    }
                }
                Command::Agent(tool, args, reply) => match self.agent(&tool, args, &reply) {
                    Ok(Some(value)) => {
                        let _ = reply.send(Ok(value));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                },
                Command::Range(key, result) => {
                    self.range_pending.remove(&key.id);
                    match result {
                        Ok(range) => {
                            if let Some(i) = self
                                .index(&key.id)
                                .filter(|i| self.traces[*i].generation == key.generation)
                            {
                                self.traces[i].disk_start = range.start;
                                self.ranges.insert(key.id.clone(), (key, range));
                            }
                        }
                        Err(e) => {
                            self.failed_ranges.insert(key.id.clone(), key);
                            self.error = Some(e);
                        }
                    }
                }
                Command::Details(id, scope, period, result) => {
                    self.details_pending.remove(&id);
                    match result {
                        Ok(hops) => {
                            self.details.insert(id, (scope, period, hops));
                        }
                        Err(e) => {
                            self.error = Some(format!("Detailed statistics could not be read: {e}"))
                        }
                    }
                }
                Command::Failure(e) => self.error = Some(e),
                Command::Save => self.save(),
                Command::ClearError => self.error = None,
                Command::Shutdown => unreachable!(),
            }
            self.publish();
        }
        for i in 0..self.traces.len() {
            self.stop(i);
        }
        self.save();
        for (_, (_, reply)) in self.pending.drain() {
            let _ = reply.send(Err("TracePlotter closed".into()));
        }
    }
    fn index(&self, id: &str) -> Option<usize> {
        self.traces.iter().position(|t| t.config.session_id == id)
    }
    fn save(&mut self) {
        self.session.targets = self.traces.iter().map(|t| t.config.clone()).collect();
        let mut stored = self.session.clone();
        stored.mcp = self.mcp_choice;
        if let Err(e) = stored.save() {
            self.error = Some(e);
        }
    }
    fn open(&mut self, mut config: Config) {
        if let Err(e) = config.validate() {
            self.error = Some(e);
            return;
        }
        if self.index(&config.session_id).is_some() {
            self.error = Some("session already open".into());
            return;
        }
        self.traces.push(Trace::new(config));
        let i = self.traces.len() - 1;
        self.launch(i);
    }
    fn erase_history(&mut self, id: String) {
        if let Some((_, reply)) = self.pending.remove(&id) {
            let _ = reply.send(Err("Target closed".into()));
        }
        let worker = self.stopped.remove(&id);
        let send = self.send.clone();
        std::thread::spawn(move || {
            if let Some(worker) = worker {
                let _ = worker.join();
            }
            if let Err(e) = delete_history(id) {
                let _ = send.send(Command::Failure(e));
            }
        });
    }
    fn stop(&mut self, i: usize) {
        let trace = &mut self.traces[i];
        trace.generation += 1;
        if let Some(cancel) = trace.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(id) = trace.run.take()
            && let Some(worker) = stop_probe(id, &self.probes)
        {
            self.stopped.insert(trace.config.session_id.clone(), worker);
        }
    }
    fn pause(&mut self, i: usize) {
        if self.traces[i].paused {
            self.traces[i].paused = false;
            self.traces[i].config.paused = false;
            self.launch(i);
        } else {
            self.stop(i);
            self.traces[i].paused = true;
            self.traces[i].config.paused = true;
        }
    }
    fn launch(&mut self, i: usize) {
        let trace = &mut self.traces[i];
        trace.generation += 1;
        trace.error = None;
        let generation = trace.generation;
        let config = trace.config.clone();
        let history = !trace.loaded;
        let cancel = Arc::new(AtomicBool::new(false));
        trace.cancel = Some(Arc::clone(&cancel));
        let previous = self.stopped.remove(&config.session_id);
        let send = self.send.clone();
        let probes = self.probes.clone();
        std::thread::spawn(move || {
            let id = config.session_id.clone();
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            if config.paused {
                let result = read_history(id.clone(), config.interval_ms).map(|history| Started {
                    id: u32::MAX,
                    addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    history,
                });
                let result = result.map(|started| Loaded::read(started, &config, history));
                let _ = send.send(Command::Started(id, generation, result));
                return;
            }
            let (events, receive) = mpsc::sync_channel(256);
            let mut probe = config.probe(history);
            probe.cancel = Some(cancel);
            let result = probe::start(
                probe,
                EventSink::new(move |e| events.send(e).is_ok()),
                &probes,
            );
            let result = result.map(|started| Loaded::read(started, &config, history));
            let success = result.is_ok();
            if send
                .send(Command::Started(id.clone(), generation, result))
                .is_err()
                || !success
            {
                return;
            }
            // Deliver startup/history before forwarding live samples, even with short intervals.
            for event in receive {
                if send
                    .send(Command::Sample(id.clone(), generation, event))
                    .is_err()
                {
                    return;
                }
            }
        });
    }
    fn started(&mut self, id: &str, generation: u64, result: Result<Loaded, String>) {
        let Some(i) = self
            .index(id)
            .filter(|i| self.traces[*i].generation == generation)
        else {
            if let Ok(loaded) = result
                && let Some(worker) = stop_probe(loaded.started.id, &self.probes)
            {
                self.stopped.insert(id.to_owned(), worker);
            }
            if self.index(id).is_none() {
                self.erase_history(id.to_owned());
            }
            return;
        };
        match result {
            Ok(loaded) => {
                self.new_targets.remove(id);
                let started = loaded.started;
                let trace = &mut self.traces[i];
                trace.paused = trace.config.paused;
                trace.addr = (!started.addr.is_unspecified()).then_some(started.addr);
                trace.target.addr = trace.addr;
                if !trace.config.paused {
                    trace.run = Some(started.id);
                }
                if let Some(replay) = loaded.replay {
                    trace.restore(*replay);
                }
                self.resolve(i);
                if let Some((mut answer, reply)) = self.pending.remove(id) {
                    let description = agent::describe(&self.traces[i], 600.0, None);
                    answer
                        .as_object_mut()
                        .expect("start metadata is an object")
                        .extend(
                            description
                                .as_object()
                                .expect("description is an object")
                                .clone(),
                        );
                    let _ = reply.send(Ok(answer));
                }
            }
            Err(e) => {
                self.traces[i].loaded = true;
                self.traces[i].error = Some(e.clone());
                self.traces[i].paused = true;
                if let Some((_, reply)) = self.pending.remove(id) {
                    let _ = reply.send(Err(e.clone()));
                }
                if self.new_targets.remove(id) {
                    self.traces.remove(i);
                    if self.session.active == id {
                        self.session.active = "summary".into();
                    }
                    self.error = Some(e);
                    self.save();
                }
            }
        }
    }
    fn sample(&mut self, id: &str, generation: u64, event: TraceEvent) {
        let Some(i) = self
            .index(id)
            .filter(|i| self.traces[*i].generation == generation)
        else {
            return;
        };
        match event {
            TraceEvent::Error { message } => {
                self.traces[i].error = (!message.is_empty()).then_some(message)
            }
            TraceEvent::Sample(sample) => {
                let changes = self.traces[i].apply(sample, true);
                for event in &changes {
                    if let Err(e) = append_event(id.to_owned(), event.to_string()) {
                        self.error = Some(e);
                    }
                }
                if self.session.settings.notifications
                    && let Some(body) = self.traces[i].notification(&changes)
                {
                    let host = self.traces[i].config.host.clone();
                    std::thread::spawn(move || {
                        let _ = notify_rust::Notification::new()
                            .summary(&format!("TracePlotter: {host}"))
                            .body(&body)
                            .show();
                    });
                }
                self.resolve(i);
            }
        }
    }
    fn resolve(&mut self, i: usize) {
        for addr in self.traces[i]
            .hops
            .iter()
            .filter(|h| h.name.is_none())
            .filter_map(|h| h.addr)
            .collect::<Vec<_>>()
        {
            if self
                .names
                .get(&addr)
                .is_some_and(|(name, t)| name.is_some() || now_ms().saturating_sub(*t) < 60000)
            {
                continue;
            }
            self.names.insert(addr, (None, now_ms()));
            let send = self.send.clone();
            std::thread::spawn(move || {
                let name = reverse_dns(addr);
                let _ = send.send(Command::Name(addr, name));
            });
        }
    }
    fn set_mcp(&mut self) {
        let send = self.send.clone();
        let result = self.mcp.start(
            self.session.mcp.then_some(self.session.settings.mcp_port),
            Arc::new(move |tool, args| {
                let (reply, receive) = mpsc::sync_channel(1);
                send.send(Command::Agent(tool.into(), args, reply))
                    .map_err(|_| "TracePlotter closed".to_string())?;
                receive
                    .recv_timeout(Duration::from_secs(30))
                    .map_err(|_| "TracePlotter did not answer in time".to_string())?
            }),
        );
        match result {
            Ok(url) => self.mcp_url = url,
            Err(e) => {
                self.mcp_url = None;
                self.error = Some(e);
            }
        }
    }
    fn publish(&mut self) {
        let mut snapshot = Snapshot {
            session: self.session.clone(),
            error: self.error.clone(),
            mcp_url: self.mcp_url.clone(),
            ..Snapshot::default()
        };
        snapshot.session.targets = self.traces.iter().map(|t| t.config.clone()).collect();
        let view = self.view.clone();
        let selected: Vec<String> = view.as_ref().map_or_else(Vec::new, |v| {
            if v.session == "summary" {
                self.session.compare.clone()
            } else {
                vec![v.session.clone()]
            }
        });
        let latest = self
            .traces
            .iter()
            .filter(|t| selected.contains(&t.config.session_id))
            .filter_map(|t| t.target.times.last().copied())
            .reduce(f64::max)
            .unwrap_or(0.0);
        let to = view
            .as_ref()
            .and_then(|v| v.end)
            .unwrap_or(latest)
            .min(latest);
        let earliest = self
            .traces
            .iter()
            .filter(|t| selected.contains(&t.config.session_id))
            .map(|t| t.period(0.0).0)
            .reduce(f64::min)
            .unwrap_or(to);
        let from = if self.session.span > 0.0 {
            if view
                .as_ref()
                .is_some_and(|v| v.session == "summary" && v.end.is_none())
            {
                self.traces
                    .iter()
                    .filter(|t| selected.contains(&t.config.session_id))
                    .map(|t| t.period(self.session.span).0)
                    .reduce(f64::min)
                    .unwrap_or(to - self.session.span)
            } else {
                to - self.session.span
            }
        } else {
            earliest
        };
        snapshot.plot_range = [from, to];
        for trace in &self.traces {
            let active = selected.contains(&trace.config.session_id);
            let summary = view.as_ref().is_some_and(|v| v.session == "summary");
            let (start, end) =
                if active && !(summary && view.as_ref().is_some_and(|v| v.end.is_none())) {
                    (from, to)
                } else {
                    trace.period(self.session.span)
                };
            let interval = trace.config.interval_ms as f64 / 1000.0;
            let width = stretch_width(
                end - start,
                view.as_ref().map_or(800, |v| v.width),
                interval,
            );
            let hops = if active {
                view.as_ref().map_or_else(Vec::new, |v| {
                    if v.session == "summary" {
                        vec![0]
                    } else {
                        v.hops.clone()
                    }
                })
            } else {
                vec![]
            };
            let old =
                trace.disk_start.is_some_and(|disk| {
                    trace.target.times.first().is_some_and(|held| {
                        start.max(disk as f64 / 1000.0) < *held - 3.0 * interval
                    })
                });
            let live = view.as_ref().is_none_or(|v| v.end.is_none());
            let key = RangeKey {
                id: trace.config.session_id.clone(),
                generation: trace.generation,
                from: if live {
                    (start.max(0.0) / 10.0).floor() as u64 * 10000
                } else {
                    (start.max(0.0) * 1000.0).round() as u64
                },
                to: if live {
                    (end.max(0.0) / 10.0).ceil() as u64 * 10000 + 1
                } else {
                    (end.max(0.0) * 1000.0).round() as u64 + 1
                },
                live,
                width: (width * 1000.0).max(1.0) as u64,
                graphed: if summary { vec![0] } else { hops.clone() },
            };
            let disk = old
                .then(|| self.ranges.get(&trace.config.session_id))
                .flatten()
                .filter(|(cached, _)| cached.compatible(&key))
                .map(|(_, r)| r);
            if old && disk.is_none() && !self.range_pending.contains(&key.id) {
                self.range_pending.insert(key.id.clone());
                let send = self.send.clone();
                let request = key.clone();
                std::thread::spawn(move || {
                    let result = history_range(
                        request.id.clone(),
                        request.from,
                        request.to,
                        request.width,
                        request.graphed.clone(),
                    );
                    let _ = send.send(Command::Range(request, result));
                });
            }
            snapshot.history_loading |= old && disk.is_none();
            // Detailed columns cost a pass over every reply, so they are only worked out for
            // a shown tab with one of those columns visible.
            let extras_shown = active
                && !summary
                && model::EXTRA_COLUMNS
                    .iter()
                    .any(|c| !self.session.hidden_columns.iter().any(|h| h == c));
            let scope = format!(
                "{} {} {} {:?}",
                key.id,
                key.generation,
                self.session.span,
                view.as_ref().and_then(|v| v.end)
            );
            if extras_shown && old {
                // Live periods refresh at most twice a minute; another period asks at once.
                let stale = self
                    .details
                    .get(&key.id)
                    .is_none_or(|(s, (_, to), _)| *s != scope || (live && end - to > 30.0));
                if stale && self.details_pending.insert(key.id.clone()) {
                    let send = self.send.clone();
                    let (id, scope, interval_ms) =
                        (key.id.clone(), scope.clone(), trace.config.interval_ms);
                    std::thread::spawn(move || {
                        let result = read_details(
                            &id,
                            (start.max(0.0) * 1000.0) as u64,
                            (end.max(0.0) * 1000.0) as u64 + 1,
                            interval_ms,
                        );
                        let _ = send.send(Command::Details(id, scope, (start, end), result));
                    });
                }
            }
            let details = (extras_shown && old).then(|| {
                self.details
                    .get(&key.id)
                    .filter(|(s, ..)| *s == scope)
                    .map(|(_, period, hops)| (*period, hops))
            });
            let extras = |ttl: u8, hop: &Hop| -> Option<Extras> {
                if !extras_shown {
                    return None;
                }
                match details {
                    Some(found) => found.and_then(|(_, hops)| {
                        hops.iter().find(|d| d.ttl == ttl).map(|d| Extras {
                            median: d.median,
                            p95: d.p95,
                            p99: d.p99,
                            longest_lost: d.longest_lost,
                            longest_loss_seconds: d.longest_loss_seconds,
                        })
                    }),
                    None => {
                        let (b, r) = (hop.bounds(start, end), trace.target.bounds(start, end));
                        Some(logic::sample_extras(
                            &hop.times[b.clone()],
                            &hop.rtts[b],
                            trace.config.interval_ms as f64,
                            &trace.target.times[r],
                        ))
                    }
                }
            };
            let total = |ttl: u8, hop: &Hop| {
                disk.map_or_else(
                    || hop.stats(start, end),
                    |r| {
                        r.hops
                            .iter()
                            .find(|h| h.ttl == ttl)
                            .map_or_else(Stats::default, |h| h.total.into())
                    },
                )
            };
            let rows = trace
                .hops
                .iter()
                .take(if trace.path_end == 0 {
                    trace.hops.len()
                } else {
                    usize::from(trace.path_end)
                })
                .map(|hop| Row {
                    ttl: hop.ttl,
                    addr: hop.addr.map_or_else(|| "*".into(), |a| a.to_string()),
                    name: hop.name.clone().unwrap_or_default(),
                    stats: total(hop.ttl, hop),
                    extras: extras(hop.ttl, hop),
                })
                .collect();
            snapshot.traces.push(TraceView {
                config: trace.config.clone(),
                paused: trace.paused,
                addr: trace.addr.map_or_else(String::new, |a| a.to_string()),
                loading: !trace.loaded
                    || (!trace.paused && trace.run.is_none() && trace.error.is_none()),
                error: trace.error.clone(),
                alerting: trace.alerting,
                rows,
                total: total(0, &trace.target),
                total_extras: extras(0, &trace.target),
                details_period: details.map(|found| found.map(|(period, _)| period)),
                spark: if let Some(disk) = disk {
                    let values = disk
                        .hops
                        .iter()
                        .find(|h| h.ttl == 0)
                        .map(|h| h.stretches.iter().map(|s| s.tally.max).collect::<Vec<_>>())
                        .unwrap_or_default();
                    sparkline(&values)
                } else {
                    sparkline(&trace.target.rtts[trace.target.bounds(start, end)])
                },
                events: trace.events.clone(),
                from: trace.period(0.0).0,
                to: trace.period(0.0).1,
            });
            if active {
                for ttl in hops {
                    if let Some(hop) = if ttl == 0 {
                        Some(&trace.target)
                    } else {
                        trace.hops.get(usize::from(ttl) - 1)
                    } {
                        let loss_seconds =
                            ((10.0 * interval).max((end - start) / 120.0) / interval).ceil()
                                * interval;
                        let data = if let Some(disk) = disk {
                            disk.hops.iter().find(|h| h.ttl == ttl).map_or_else(
                                GraphData::default,
                                |h| {
                                    logic::range_graph(
                                        &h.stretches,
                                        width,
                                        loss_seconds,
                                        2,
                                        interval,
                                    )
                                },
                            )
                        } else {
                            graph(
                                hop,
                                start,
                                end,
                                width,
                                interval,
                                view.as_ref().map_or(800, |v| v.width),
                            )
                        };
                        let stretches = disk
                            .and_then(|d| d.hops.iter().find(|h| h.ttl == ttl))
                            .map_or_else(
                                || stretches(hop, start, end, width),
                                |h| h.stretches.clone(),
                            );
                        let data = if summary {
                            logic::range_graph(&stretches, width, loss_seconds, 0, interval)
                        } else {
                            data
                        };
                        let label = if view.as_ref().is_some_and(|v| v.session == "summary") {
                            trace.config.host.clone()
                        } else {
                            // Worded as the legacy graph heading: the name, its address,
                            // and whether this is the round trip.
                            // The target row carries no name or address of its own.
                            let (name, addr) = if ttl == 0 {
                                (
                                    hop.name.clone().or_else(|| Some(trace.config.host.clone())),
                                    hop.addr.or(trace.addr),
                                )
                            } else {
                                (hop.name.clone(), hop.addr)
                            };
                            let who = match (&name, addr) {
                                (Some(name), Some(addr)) => format!("{name} ({addr})"),
                                (Some(name), None) => name.clone(),
                                (None, Some(addr)) => addr.to_string(),
                                (None, None) => "no reply".into(),
                            };
                            match (ttl, hop.ttl) {
                                (0, 0) => format!("{who} round trip"),
                                (0, last) => format!("{who} round trip, hop {last}"),
                                _ => format!("{who} hop {ttl}"),
                            }
                        };
                        snapshot.plots.push(PlotView {
                            session: trace.config.session_id.clone(),
                            ttl,
                            label,
                            data,
                            width,
                            stretches,
                        });
                    }
                }
            }
        }
        *self.snapshot.write().expect("snapshot lock poisoned") = Arc::new(snapshot);
        self.ctx.request_repaint();
    }
    fn agent(
        &mut self,
        tool: &str,
        args: Value,
        reply: &SyncSender<Result<Value, String>>,
    ) -> Result<Option<Value>, String> {
        if matches!(
            tool,
            "list_targets" | "get_hops" | "get_route_changes" | "get_latency_history"
        ) {
            // Tool requests may read files; keep them off both the UI and probe controller.
            let traces = self.traces.clone();
            let tool = tool.to_owned();
            let reply = reply.clone();
            std::thread::spawn(move || {
                let _ = reply.send(agent::read(&traces, &tool, &args));
            });
            return Ok(None);
        }
        match tool {
            "stop_trace" => {
                let i = agent::find(&self.traces, &args["target"])?;
                if !self.traces[i].paused {
                    self.pause(i);
                    self.save();
                }
                Ok(Some(agent::describe(&self.traces[i], 600.0, None)))
            }
            "start_trace" => {
                let host = args["host"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                if host.is_empty() {
                    return Err("start_trace needs a host".into());
                }
                let mut config = Config::new(host, &self.session.settings);
                config.protocol = args["protocol"]
                    .as_str()
                    .unwrap_or(if cfg!(any(windows, target_os = "linux")) {
                        "winicmp"
                    } else {
                        "icmp"
                    })
                    .into();
                if !["icmp", "udp", "tcp"].contains(&config.protocol.as_str())
                    && !(config.protocol == "winicmp" && cfg!(any(windows, target_os = "linux")))
                {
                    return Err(format!(
                        "protocol {} is not available here",
                        config.protocol
                    ));
                }
                let number = |v: &Value| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                };
                config.port = if ["tcp", "udp"].contains(&config.protocol.as_str()) {
                    let port = number(&args["port"]).unwrap_or(if config.protocol == "tcp" {
                        443.0
                    } else {
                        33434.0
                    });
                    if !port.is_finite() || port.fract() != 0.0 || !(0.0..=65535.0).contains(&port)
                    {
                        return Err("port must be a whole number from 0 to 65535".into());
                    }
                    port as u16
                } else {
                    0
                };
                let asked = number(&args["interval_ms"])
                    .filter(|n| *n != 0.0)
                    .unwrap_or(1000.0)
                    .max(1000.0);
                config.interval_ms = [1000, 2500, 5000, 10000]
                    .into_iter()
                    .find(|ms| *ms as f64 >= asked)
                    .unwrap_or(10000);
                if let Some(i) = self.traces.iter().position(|t| {
                    t.config.host == config.host
                        && t.config.protocol == config.protocol
                        && t.config.port == config.port
                }) {
                    if self.traces[i].paused {
                        self.pending.insert(
                            self.traces[i].config.session_id.clone(),
                            (json!({"started":"resumed"}), reply.clone()),
                        );
                        self.pause(i);
                        self.save();
                        return Ok(None);
                    }
                    self.save();
                    let mut answer = agent::describe(&self.traces[i], 600.0, None);
                    answer["started"] = json!(if self.traces[i].paused {
                        "could not resume"
                    } else {
                        "resumed"
                    });
                    return Ok(Some(answer));
                }
                config.validate()?;
                let mut metadata = json!({"started":"new tab"});
                if number(&args["interval_ms"]).is_some_and(|n| n < 1000.0) {
                    metadata["note"] = json!(format!(
                        "Probing once a second, not every {} ms: many routers throttle faster probes.",
                        args["interval_ms"]
                            .as_str()
                            .map_or_else(|| args["interval_ms"].to_string(), str::to_owned)
                    ));
                }
                self.new_targets.insert(config.session_id.clone());
                self.pending
                    .insert(config.session_id.clone(), (metadata, reply.clone()));
                self.open(config);
                self.save();
                Ok(None)
            }
            _ => Err(format!("unknown tool {tool}")),
        }
    }
}
pub(super) fn stretch_width(span: f64, width: usize, interval: f64) -> f64 {
    let pixel = span / width.max(1) as f64;
    if pixel >= 60.0 {
        (pixel / 60.0).ceil() * 60.0
    } else {
        pixel.ceil().max(interval)
    }
}
fn graph(hop: &Hop, from: f64, to: f64, width: f64, interval: f64, pixels: usize) -> GraphData {
    let range = hop.bounds(from, to);
    let times = &hop.times[range.clone()];
    let rtts = &hop.rtts[range];
    let jitters = logic::jitter(rtts);
    let losses = logic::loss(
        times,
        rtts,
        ((10.0 * interval).max((to - from) / 120.0) / interval).ceil() * interval,
    );
    let mut data = GraphData::default();
    let mut breaks = HashSet::new();
    for i in 0..times.len() {
        let gap = i > 0 && times[i] - times[i - 1] > 5.0f64.max(3.0 * interval);
        if gap {
            let t = times[i - 1] + interval;
            breaks.insert(t.to_bits());
            data.xs.push(t);
            data.ys.push(None);
            data.js.push(None);
            data.ls.push(None);
        }
        data.xs.push(times[i]);
        data.ys.push(rtts[i]);
        data.js.push(if gap { None } else { jitters[i] });
        data.ls.push(Some(losses[i]));
    }
    let width = if data.xs.len() > pixels { width } else { 0.0 };
    logic::thin(&data, &breaks, width, 2)
}

fn sparkline(rtts: &[Option<f64>]) -> Vec<Option<f64>> {
    let n = rtts.len().min(160);
    (0..n)
        .map(|i| {
            rtts[i * rtts.len() / n..(i + 1) * rtts.len() / n]
                .iter()
                .flatten()
                .copied()
                .reduce(f64::max)
        })
        .collect()
}
fn stretches(hop: &Hop, from: f64, to: f64, width: f64) -> Vec<Stretch> {
    let mut buckets = BTreeMap::<u64, Tally>::new();
    let mut previous = None;
    for i in hop.bounds(from, to) {
        let t = ((hop.times[i] / width).floor() * width * 1000.0) as u64;
        buckets.entry(t).or_default().add(hop.rtts[i], previous);
        if hop.rtts[i].is_some() {
            previous = hop.rtts[i];
        }
    }
    buckets
        .into_iter()
        .map(|(t, tally)| Stretch { t, tally })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller() -> Controller {
        let (send, receive) = mpsc::sync_channel(1024);
        Controller {
            session: Session::default(),
            mcp_choice: false,
            traces: vec![],
            probes: Traces::default(),
            send,
            receive,
            snapshot: Arc::new(RwLock::new(Arc::new(Snapshot::default()))),
            ctx: egui::Context::default(),
            view: None,
            mcp: mcp::Mcp::default(),
            mcp_url: None,
            error: None,
            names: HashMap::new(),
            pending: HashMap::new(),
            new_targets: HashSet::new(),
            failed_ranges: HashMap::new(),
            stopped: HashMap::new(),
            ranges: HashMap::new(),
            range_pending: HashSet::new(),
            details: HashMap::new(),
            details_pending: HashSet::new(),
        }
    }
    fn sample(t: u64) -> TraceEvent {
        TraceEvent::Sample(Sample {
            t,
            hops: vec![HopSample {
                ttl: 1,
                addr: None,
                rtt: Some(10.0),
            }],
        })
    }
    #[test]
    fn samples_and_snapshots_need_no_paint_and_ignore_old_runs() {
        let mut c = controller();
        let mut trace = Trace::new(Config {
            session_id: "one".into(),
            host: "example.com".into(),
            ..Config::default()
        });
        trace.generation = 2;
        trace.loaded = true;
        c.traces.push(trace);
        c.sample("one", 1, sample(1000));
        assert!(c.traces[0].target.times.is_empty());
        c.sample("one", 2, sample(2000));
        c.publish();
        assert_eq!(c.snapshot.read().unwrap().traces[0].total.sent, 1);
    }
    #[test]
    fn graph_snapshots_are_bounded_while_the_model_keeps_every_probe() {
        let mut c = controller();
        c.session.span = 0.0;
        let mut trace = Trace::new(Config {
            session_id: "one".into(),
            host: "example.com".into(),
            ..Config::default()
        });
        for t in 1..=10000 {
            if let TraceEvent::Sample(sample) = sample(t * 1000) {
                trace.apply(sample, false);
            }
        }
        trace.loaded = true;
        c.traces.push(trace);
        c.view = Some(View {
            session: "one".into(),
            hops: vec![0],
            end: None,
            width: 500,
        });
        c.publish();
        assert_eq!(c.traces[0].target.times.len(), 10000);
        assert!(c.snapshot.read().unwrap().plots[0].data.xs.len() <= 502);
    }
    #[test]
    fn sparse_graphs_keep_each_probe_and_start_jitter_at_the_visible_window() {
        let hop = Hop {
            ttl: 1,
            times: vec![1.0, 2.0, 3.0, 4.0],
            rtts: vec![Some(10.0), Some(20.0), Some(5.0), Some(15.0)],
            ..Hop::default()
        };
        let data = graph(&hop, 3.0, 4.0, 1.0, 1.0, 100);
        assert_eq!(data.xs, vec![3.0, 4.0]);
        assert_eq!(data.ys, vec![Some(5.0), Some(15.0)]);
        assert_eq!(data.js, vec![None, Some(10.0)]);
    }

    #[test]
    fn isolated_spikes_survive_raw_views_and_dense_range_bands() {
        let hop = Hop {
            ttl: 1,
            times: (0..1000).map(f64::from).collect(),
            rtts: (0..1000)
                .map(|i| Some(if i == 500 { 200.0 } else { 10.0 }))
                .collect(),
            ..Hop::default()
        };
        let raw = graph(&hop, 0.0, 999.0, 1.0, 1.0, 2000);
        assert_eq!(
            raw.ys[500],
            Some(200.0),
            "A single probe's peak must not be averaged in a raw view"
        );
        let dense = graph(&hop, 0.0, 999.0, 10.0, 1.0, 100);
        assert!(dense.xs.len() <= 100);
        assert_eq!(
            dense.hi.iter().flatten().copied().reduce(f64::max),
            Some(200.0),
            "The range band must reach the real peak even when the average is smoothed"
        );
        assert_eq!(
            dense.lo.iter().flatten().copied().reduce(f64::min),
            Some(10.0)
        );
    }
}

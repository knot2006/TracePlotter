//! Native desktop interface. Probing and agent requests continue without paints.
use super::*;
use eframe::egui::{self, Color32, RichText};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::mpsc;
mod agent;
mod app;
mod context;
mod export;
mod graphs;
mod import;
mod layout;
pub(super) mod migration;
mod model;
mod report;
mod time;
mod tray;
use app::{Command, Runtime, Snapshot, TraceView, View};
use model::{Config, Session};

pub(super) fn run() {
    wait_for_predecessor();
    let title = if demo() {
        "TracePlotter (demo)"
    } else {
        "TracePlotter"
    };
    let icon = image::load_from_memory(include_bytes!("../../icons/128x128.png"))
        .ok()
        .map(|i| {
            let i = i.to_rgba8();
            egui::IconData {
                width: i.width(),
                height: i.height(),
                rgba: i.into_raw(),
            }
        });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_app_id("io.github.knot2006.traceplotter")
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 420.0])
        .with_decorations(false);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let result = eframe::run_native(
        title,
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Window::new(cc).map_err(std::io::Error::other)?))),
    );
    if let Err(e) = result {
        let message = format!("TracePlotter could not open: {e}");
        eprintln!("{message}");
        rfd::MessageDialog::new()
            .set_title("TracePlotter")
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
const COLUMNS: [(&str, &str); 16] = [
    ("ip", "IP"),
    ("name", "Name"),
    ("loss", "Loss"),
    ("sent", "Sent"),
    ("recv", "Recv"),
    ("last", "Last"),
    ("avg", "Avg"),
    ("min", "Min"),
    ("max", "Max"),
    ("jitter", "Jitter"),
    ("median", "Median"),
    ("p95", "p95"),
    ("p99", "p99"),
    ("lossBurst", "Longest loss"),
    ("mos", "MOS"),
    ("bar", "Latency bar"),
];
enum Job {
    Message(Result<Option<String>, String>),
    Imported(Result<Option<Config>, String>),
    Whois(IpAddr, Result<String, String>),
    Updates(Result<Option<String>, String>),
    Restart(Result<(), String>),
}
struct Window {
    runtime: Runtime,
    host: String,
    toolbar_protocol: String,
    toolbar_port: u16,
    toolbar_interval: u64,
    selected: Vec<u8>,
    last_view: Option<View>,
    error: Option<String>,
    message: Option<String>,
    end: Option<f64>,
    pinned: Option<f64>,
    editing: Option<(Config, bool)>,
    columns_open: bool,
    reset_columns: bool,
    events_open: bool,
    /// The tab Settings was opened from, which a second click on Settings returns to.
    before_settings: Option<String>,
    /// The section shown on the settings tab, and how far its page is scrolled.
    settings_page: usize,
    settings_scroll: f32,
    /// The event list's kind filter, `None` for every kind, and whether it keeps to the
    /// displayed period.
    events_filter: Option<&'static str>,
    events_in_period: bool,
    whois: Option<(IpAddr, String)>,
    jobs: mpsc::Receiver<Job>,
    send: mpsc::Sender<Job>,
    busy: bool,
    plot_width: Option<f32>,
    #[cfg(debug_assertions)]
    captured: bool,
    #[cfg(debug_assertions)]
    qa_exit: Option<std::time::Instant>,
    placed: bool,
    window_state: Option<WindowState>,
    /// Only the real window keeps its placement; test windows leave `window.json` alone.
    remember_window: bool,
    focus_host: bool,
    table_height: Option<f32>,
    bar_zoom: f64,
    needs_exit: bool,
    gpu: Option<eframe::egui_wgpu::RenderState>,
    span_text: String,
    interval_text: String,
    /// Text buffers of the defaults' and the probe dialog's interval fields.
    default_interval_text: String,
    probe_interval_text: String,
    /// A pending close or restart that erases history: session ID, host, and whether to restart.
    erasing: Option<(String, String, bool)>,
    /// The route change, alert or recovery being inspected; cleared by going back to live.
    selected_event: Option<Value>,
    /// The support report dialog, open with whether to redact names and addresses.
    report_dialog: Option<bool>,
    /// The tray icon, when the desktop has a tray.
    tray: Option<tray::Tray>,
    background: tray::Options,
    /// Whether the app starts at sign-in, as last read from Windows.
    startup: bool,
    #[cfg(feature = "dev-live")]
    live: Option<blaze_live::Live>,
}
impl Window {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        migration::prepare()?;
        let session = Session::load()?;
        apply_theme(&cc.egui_ctx);
        let runtime = Runtime::start(session.clone(), cc.egui_ctx.clone());
        let mut result = Self::with_runtime(runtime);
        result.gpu = cc.wgpu_render_state.clone();
        result.window_state = load_window_state();
        result.remember_window = true;
        result.tray = tray::start(&cc.egui_ctx);
        result.background = tray::Options::load();
        result.startup = tray::startup();
        // Started at sign-in: monitor from the tray without showing the window.
        if result.tray.is_some() && std::env::args().any(|a| a == "--background") {
            tray::hide(&cc.egui_ctx);
        }
        #[cfg(feature = "dev-live")]
        if cfg!(debug_assertions) && demo() {
            result.live = match blaze_live::Live::start() {
                Ok(live) => Some(live),
                Err(e) => {
                    eprintln!("Blaze live unavailable: {e}");
                    None
                }
            };
        }
        if session.check_updates && !demo() {
            result.update_check(&cc.egui_ctx);
        }
        Ok(result)
    }
    fn with_runtime(runtime: Runtime) -> Self {
        let (send, jobs) = mpsc::channel();
        let config = {
            let snapshot = runtime.read();
            snapshot
                .session
                .targets
                .iter()
                .find(|target| target.session_id == snapshot.session.active)
                .cloned()
                .unwrap_or_else(|| Config::new(String::new(), &snapshot.session.settings))
        };
        let host = config.host.clone();
        Self {
            runtime,
            host,
            toolbar_protocol: config.protocol,
            toolbar_port: config.port,
            toolbar_interval: config.interval_ms,
            selected: vec![0],
            last_view: None,
            error: None,
            message: None,
            end: None,
            pinned: None,
            editing: None,
            columns_open: false,
            reset_columns: false,
            events_open: false,
            before_settings: None,
            settings_page: 0,
            settings_scroll: 0.0,
            events_filter: None,
            events_in_period: false,
            whois: None,
            jobs,
            send,
            busy: false,
            plot_width: None,
            #[cfg(debug_assertions)]
            captured: false,
            #[cfg(debug_assertions)]
            qa_exit: if demo() {
                std::env::var("TRACEPLOTTER_QA_EXIT_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|ms| std::time::Instant::now() + Duration::from_millis(ms))
            } else {
                None
            },
            placed: false,
            window_state: None,
            remember_window: false,
            focus_host: false,
            table_height: None,
            bar_zoom: 1.0,
            needs_exit: false,
            gpu: None,
            span_text: String::new(),
            interval_text: String::new(),
            default_interval_text: String::new(),
            probe_interval_text: String::new(),
            erasing: None,
            selected_event: None,
            report_dialog: None,
            tray: None,
            background: tray::Options::default(),
            startup: false,
            #[cfg(feature = "dev-live")]
            live: None,
        }
    }
    fn preferences(&self, session: Session) {
        self.runtime
            .command(Command::Preferences(Box::new(session)));
    }
    fn job(&mut self, ctx: &egui::Context, work: impl FnOnce() -> Job + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        let send = self.send.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = send.send(work());
            ctx.request_repaint();
        });
    }
    fn update_check(&mut self, ctx: &egui::Context) {
        self.job(ctx, || {
            Job::Updates(check_update().map(|update| update.map(|u| u.version)))
        });
    }
    fn import(&mut self, ctx: &egui::Context, session: &Session) {
        let settings = session.settings.clone();
        self.job(ctx, move || Job::Imported(import::pick(&settings)));
    }
    fn select(&mut self, session: &mut Session, id: String) {
        if id == "settings" && session.active != "settings" {
            self.before_settings = Some(session.active.clone());
        }
        self.runtime.command(Command::Select(id.clone()));
        session.active = id;
        self.selected = vec![0];
        self.bar_zoom = 1.0;
        if let Some(trace) = self
            .runtime
            .read()
            .traces
            .iter()
            .find(|t| t.config.session_id == session.active)
        {
            self.sync_toolbar(&trace.config);
        }
        if matches!(session.active.as_str(), "summary" | "settings") {
            self.sync_toolbar(&Config::new(String::new(), &session.settings));
        }
        self.end = None;
        self.pinned = None;
        self.last_view = None;
    }
    /// Shows up to five minutes around `event` and marks it as the one being inspected.
    fn jump_to_event(&mut self, trace: &TraceView, span: &mut f64, event: Value) {
        let t = event["t"].as_f64().unwrap_or(trace.to);
        let length = (trace.to - trace.from).clamp(10.0, 300.0);
        self.end = Some((t + length / 2.0).min(trace.to));
        self.pinned = None;
        *span = length;
        self.selected_event = Some(event);
    }
    /// Selecting the inspected event again clears the selection without moving the period.
    fn toggle_event(&mut self, trace: &TraceView, span: &mut f64, event: Value) {
        if self
            .selected_event
            .as_ref()
            .is_some_and(|s| graphs::same_event(s, &event))
        {
            self.selected_event = None;
        } else {
            self.jump_to_event(trace, span, event);
        }
    }
    /// Asks in the window, not a system dialog, so the question wears the app's theme.
    fn erase(&mut self, trace: &TraceView, restart: bool) {
        self.erasing = Some((
            trace.config.session_id.clone(),
            trace.config.host.clone(),
            restart,
        ));
    }
    /// How the window is placed now, or `None` while it is minimized. A maximized window
    /// keeps the size and position it will return to.
    fn placement(&self, ctx: &egui::Context) -> Option<WindowState> {
        ctx.input(|i| {
            let v = i.viewport();
            if v.minimized == Some(true) {
                return None;
            }
            let p = i.pixels_per_point();
            // Wayland reports neither rect: the size then comes from the content, and the
            // position, which only the compositor knows there, stays as it was.
            let inner = v.inner_rect.unwrap_or(i.content_rect());
            let (x, y) = v.outer_rect.map_or_else(
                || self.window_state.map_or((0, 0), |s| (s.x, s.y)),
                |outer| ((outer.min.x * p) as i32, (outer.min.y * p) as i32),
            );
            let maximized = v.maximized.unwrap_or(false);
            let placed = WindowState {
                x,
                y,
                width: (inner.width() * p) as u32,
                height: (inner.height() * p) as u32,
                maximized,
            };
            Some(if maximized {
                self.window_state
                    .map_or(placed, |s| WindowState { maximized, ..s })
            } else {
                placed
            })
        })
    }
    fn save_window(&mut self, ctx: &egui::Context) {
        if let Some(state) = self.placement(ctx) {
            self.window_state = Some(state);
            if let Ok(bytes) = serde_json::to_vec(&state)
                && let Err(e) = model::atomic_write(&data_dir().join("window.json"), &bytes)
            {
                self.error = Some(e);
            }
        }
    }
    fn restore_window(&mut self, ctx: &egui::Context) {
        // Wayland drops a resize sent before the window's first frame has been shown.
        if self.placed || ctx.cumulative_frame_nr() < 2 {
            return;
        }
        self.placed = true;
        if let Some(s) = self.window_state {
            let (p, monitor) = ctx.input(|i| (i.pixels_per_point(), i.viewport().monitor_size));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                s.width as f32 / p,
                s.height as f32 / p,
            )));
            if monitor.is_some_and(|m| {
                s.x >= 0
                    && s.y >= 0
                    && (s.x as f32 / p) < m.x - 50.0
                    && (s.y as f32 / p) < m.y - 50.0
            }) {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                    s.x as f32 / p,
                    s.y as f32 / p,
                )));
            }
            if s.maximized {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
        }
    }
    fn poll(&mut self, session: &mut Session) -> bool {
        let mut changed = false;
        for job in self.jobs.try_iter() {
            self.busy = false;
            match job {
                Job::Restart(result) => match result {
                    Ok(()) => self.needs_exit = true,
                    Err(e) => self.error = Some(e),
                },
                Job::Message(result) => match result {
                    Ok(message) => self.message = message,
                    Err(e) => self.error = Some(e),
                },
                Job::Imported(result) => match result {
                    Ok(Some(config)) => {
                        session.active = config.session_id.clone();
                        self.runtime.command(Command::Open(config));
                        changed = true;
                    }
                    Ok(None) => {}
                    Err(e) => self.error = Some(e),
                },
                Job::Whois(addr, result) => {
                    self.whois = Some((
                        addr,
                        result.unwrap_or_else(|e| format!("WHOIS failed: {e}")),
                    ))
                }
                Job::Updates(result) => match result {
                    Ok(Some(version)) => {
                        self.message = Some(format!(
                            "TracePlotter {version} is available. Open Help for updates."
                        ))
                    }
                    Ok(None) => {
                        self.message = Some("You have the latest published version.".into())
                    }
                    Err(e) => self.error = Some(e),
                },
            }
        }
        changed
    }
    fn export(&mut self, ui: &egui::Ui, snapshot: &Snapshot, kind: export::Kind) {
        if self.busy {
            return;
        }
        if matches!(
            kind,
            export::Kind::Png | export::Kind::CopyImage | export::Kind::Pdf
        ) {
            let Some(gpu) = self.gpu.clone() else {
                self.error = Some("No renderer is available for image exports.".into());
                return;
            };
            let snapshot = snapshot.clone();
            let pixels_per_point = ui.ctx().pixels_per_point();
            self.job(ui.ctx(), move || {
                Job::Message(export::render(&gpu, &snapshot, kind, pixels_per_point))
            });
        } else {
            let snapshot = snapshot.clone();
            self.job(ui.ctx(), move || {
                Job::Message(export::save(&snapshot, kind))
            });
        }
    }

    fn menus(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, session: &mut Session) -> bool {
        let mut changed = false;
        let active = snapshot
            .traces
            .iter()
            .find(|t| t.config.session_id == session.active);
        let control_height = ui.spacing().interact_size.y;
        ui.spacing_mut().interact_size.y = 24.0;
        // Inset so the first menu lines up with the tab strip rather than the window edge.
        // The window buttons set the row's 40 px height, as in MonoCode's title bar, and
        // reach the right edge.
        egui::Frame::new().inner_margin(egui::Margin { left: 8, ..Default::default() }).show(ui, |ui| egui::MenuBar::new().ui(ui, |ui| {
            // Items centre on the row's height as it is when they are placed.
            ui.set_min_height(40.0);
            // egui draws menu-bar buttons without a frame, so they light up only once a menu
            // is open. A frame that is clear at rest gives them the hover fill at once.
            let visuals = ui.visuals_mut();
            visuals.button_frame = true;
            visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.menu_button("File", |ui| {
                menu_rows(ui);
                if ui.add(egui::Button::new("New target").shortcut_text("Ctrl+T")).clicked() {
                    self.host.clear();
                    self.select(session, "summary".into());
                    self.focus_host = true;
                    changed = true;
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.busy,
                        egui::Button::new("Import samples…").shortcut_text("Ctrl+O"),
                    )
                    .clicked()
                {
                    self.import(ui.ctx(), session);
                    ui.close();
                }
                // Everything that writes a file or the clipboard, in one place.
                ui.add_enabled_ui(
                    !self.busy
                        && (active.is_some()
                            || (session.active == "summary" && !snapshot.traces.is_empty())),
                    |ui| {
                        ui.menu_button("Export", |ui| {
                            menu_rows(ui);
                            for (label, kind) in [
                                ("Report text…", export::Kind::Text),
                                ("Hop table CSV…", export::Kind::Csv),
                                ("All samples CSV…", export::Kind::Samples),
                                ("Plotted stretches CSV…", export::Kind::Stretches),
                                ("Graphs PNG…", export::Kind::Png),
                                ("Report PDF…", export::Kind::Pdf),
                            ] {
                                if ui.button(label).clicked() {
                                    self.export(ui, snapshot, kind);
                                    ui.close();
                                }
                            }
                            if active.is_some() && ui.button("Support report (.html)…").clicked() {
                                self.report_dialog = Some(false);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Copy graphs image").clicked() {
                                self.export(ui, snapshot, export::Kind::CopyImage);
                                ui.close();
                            }
                            if ui.button("Copy hop table report").clicked() {
                                ui.ctx().copy_text(export::report(snapshot, false));
                                ui.close();
                            }
                        });
                    },
                );
                ui.separator();
                if let Some(trace) = active
                    && ui
                        .add(egui::Button::new("Close tab and erase history…").shortcut_text("Ctrl+W"))
                        .clicked()
                {
                    self.erase(trace, false);
                    ui.close();
                }
                if ui.add(egui::Button::new("Settings…").shortcut_text("Ctrl+,")).clicked() {
                    self.select(session, "settings".into());
                    changed = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit and stop monitoring").clicked() {
                    if let Some(tray) = &self.tray {
                        tray.quitting.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    self.save_window(ui.ctx());
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Trace", |ui| {
                menu_rows(ui);
                if let Some(t) = active {
                    menu_heading(ui, &t.config.host);
                    if ui
                        .add(
                            egui::Button::new(if t.paused { "Resume" } else { "Pause" })
                                .shortcut_text("Space"),
                        )
                        .clicked()
                    {
                        self.runtime
                            .command(Command::Pause(t.config.session_id.clone()));
                        ui.close();
                    }
                    if ui.button("Edit probe and alerts…").clicked() {
                        self.editing = Some((t.config.clone(), false));
                        ui.close();
                    }
                    if ui.button("Event list…").clicked() {
                        self.events_open = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Restart and erase history…").clicked() {
                        self.erase(t, true);
                        ui.close();
                    }
                    if ui
                        .add(egui::Button::new("Close and erase history…").shortcut_text("Ctrl+W"))
                        .clicked()
                    {
                        self.erase(t, false);
                        ui.close();
                    }
                } else {
                    ui.weak("Open a target's tab for its commands.");
                }
                ui.separator();
                if ui
                    .add_enabled(
                        snapshot.traces.iter().any(|t| !t.paused),
                        egui::Button::new("Pause all"),
                    )
                    .clicked()
                {
                    for t in &snapshot.traces {
                        if !t.paused {
                            self.runtime
                                .command(Command::Pause(t.config.session_id.clone()));
                        }
                    }
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                menu_rows(ui);
                if ui.button("All targets").clicked() {
                    self.select(session, "summary".into());
                    changed = true;
                    ui.close();
                }
                if ui.button("Columns…").clicked() {
                    self.columns_open = true;
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.end.is_some(),
                        egui::Button::new("Return to live").shortcut_text("Ctrl+L"),
                    )
                    .clicked()
                {
                    self.end = None;
                    self.pinned = None;
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(self.tray.is_some(), egui::Button::new("Hide to tray"))
                    .on_disabled_hover_text("The system tray is unavailable on this desktop")
                    .clicked()
                {
                    self.save_window(ui.ctx());
                    tray::hide(ui.ctx());
                    ui.close();
                }
            });
            ui.menu_button("Tools", |ui| {
                menu_rows(ui);
                menu_heading(ui, "AI agents");
                changed |= ui
                    .checkbox(&mut session.mcp, "Allow AI agents to connect (MCP)")
                    .changed();
                if let Some(url) = &snapshot.mcp_url {
                    ui.label(RichText::new(url).monospace().small().color(palette().soft));
                    if ui.button("Copy MCP address").clicked() {
                        ui.ctx().copy_text(url.clone());
                        ui.close();
                    }
                }
                ui.separator();
                menu_heading(ui, "Raw sockets");
                ui.weak(if is_elevated() {
                    "TracePlotter may open raw sockets."
                } else {
                    "Raw sockets need additional rights."
                });
                if ui
                    .add_enabled(
                        !self.busy && !is_elevated() && can_elevate(),
                        egui::Button::new(if cfg!(windows) {
                            "Restart as Administrator…"
                        } else {
                            "Allow raw sockets and restart…"
                        }),
                    )
                    .clicked()
                {
                    self.runtime.command(Command::Save);
                    self.save_window(ui.ctx());
                    self.job(ui.ctx(), || Job::Restart(spawn_elevated()));
                    ui.close();
                }
                // The firewall rules are for the Windows ICMP API; Linux has nothing to allow.
                if cfg!(windows)
                    && ui
                        .add_enabled(!self.busy, egui::Button::new("Allow Windows ICMP replies"))
                        .clicked()
                {
                    self.job(ui.ctx(), || {
                        Job::Message(
                            allow_icmp_replies()
                                .map(|()| Some("ICMP firewall rules installed.".into())),
                        )
                    });
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                menu_rows(ui);
                ui.label(RichText::new(concat!("TracePlotter ", env!("CARGO_PKG_VERSION"))).strong());
                ui.weak("Native egui preview");
                ui.separator();
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Check for updates"))
                    .clicked()
                {
                    self.update_check(ui.ctx());
                    ui.close();
                }
                if cfg!(windows)
                    && ui
                        .add_enabled(
                            !self.busy,
                            egui::Button::new("Install signed update and restart…"),
                        )
                        .clicked()
                {
                    self.runtime.command(Command::Save);
                    self.save_window(ui.ctx());
                    self.job(ui.ctx(), || {
                        Job::Restart(prepare_update().and_then(|()| {
                            std::process::Command::new(
                                std::env::current_exe().map_err(|e| e.to_string())?,
                            )
                            .args(restart_args())
                            .spawn()
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                        }))
                    });
                    ui.close();
                }
                changed |= ui
                    .checkbox(&mut session.check_updates, "Check for updates at startup")
                    .changed();
                ui.separator();
                if ui.button("Project page and releases").clicked() {
                    if let Err(e) = open_project_page() {
                        self.error = Some(e);
                    }
                    ui.close();
                }
            });
            if self.busy {
                ui.spinner();
                ui.weak("Working…");
            }
            if demo() {
                ui.colored_label(palette().warn, "DEMO");
            }
            window_controls(ui);
        }));
        ui.spacing_mut().interact_size.y = control_height;
        changed
    }
    fn table(
        &mut self,
        ui: &mut egui::Ui,
        trace: &TraceView,
        session: &mut Session,
        snapshot: &Snapshot,
    ) -> bool {
        let mut changed = false;
        let columns = ordered_columns(session)
            .into_iter()
            .filter(|id| !session.hidden_columns.contains(id))
            .collect::<Vec<_>>();
        let height = self
            .table_height
            .or(session.table_height)
            .unwrap_or((trace.rows.len() + 1) as f32 * 32.0)
            // The header and one row stay, so the splitter below cannot be dragged away; only
            // a window too small for any table hides it.
            .clamp(
                59f32.min((ui.available_height() - 287.0).max(0.0)),
                (ui.available_height() - 287.0).max(0.0),
            );
        if height < 1.0 {
            return false;
        }
        let bar_max = trace
            .rows
            .iter()
            .filter_map(|h| h.stats.max)
            .fold(1.0, f64::max);
        egui::ScrollArea::horizontal()
            .id_salt("table-horizontal")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                let released = ui.input(|i| i.pointer.any_released());
                let multiple = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                let mut table = egui_extras::TableBuilder::new(ui)
                    .id_salt(("hops", columns.join(",")))
                    .striped(false)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .sense(egui::Sense::click())
                    .max_scroll_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .column(egui_extras::Column::initial(52.0).at_least(32.0));
                for id in &columns {
                    let column =
                        if id == "bar" {
                            egui_extras::Column::remainder().at_least(152.0)
                        } else {
                            egui_extras::Column::initial(
                                session.column_widths.get(id).copied().unwrap_or(
                                    match id.as_str() {
                                        "ip" => 140.0,
                                        "name" => 240.0,
                                        "loss" => 68.0,
                                        "median" | "p95" | "p99" => 72.0,
                                        "lossBurst" => 172.0,
                                        _ => 64.0,
                                    },
                                ),
                            )
                            .at_least(40.0)
                        };
                    table = table.column(column);
                }
                if self.reset_columns {
                    table.reset();
                    self.reset_columns = false;
                }
                table
                    .header(27.0, |mut row| {
                        row.col(|ui| {
                            ui.add_sized(
                                [52.0, 27.0],
                                egui::Label::new(RichText::new("Hop").small()),
                            );
                            let response = ui.interact(
                                ui.max_rect(),
                                ui.scope_id().with("column-menu"),
                                egui::Sense::click(),
                            );
                            response
                                .context_menu(|ui| changed |= self.column_menu(ui, session, None));
                        });
                        for id in &columns {
                            row.col(|ui| {
                                let direction = if ["ip", "name", "bar"].contains(&id.as_str()) {
                                    egui::Layout::left_to_right(egui::Align::Center)
                                } else {
                                    egui::Layout::right_to_left(egui::Align::Center)
                                };
                                ui.with_layout(direction, |ui| {
                                    ui.add_space(10.0);
                                    let scale = bar_max / self.bar_zoom;
                                    let label = if id == "bar" {
                                        format!(
                                            "Latency (0 – {scale:.digits$} ms){zoomed}",
                                            digits = usize::from(scale < 10.0),
                                            zoomed = if self.bar_zoom > 1.0 { " · zoomed" } else { "" }
                                        )
                                    } else {
                                        COLUMNS
                                            .iter()
                                            .find(|c| c.0 == id)
                                            .map_or("", |c| c.1)
                                            .into()
                                    };
                                    let header = ui.label(RichText::new(label).small());
                                    if let Some(tip) = column_title(id) {
                                        header.on_hover_text(tip);
                                    }
                                });
                                let response = ui.interact(
                                    ui.max_rect(),
                                    ui.scope_id().with("column-menu"),
                                    egui::Sense::click(),
                                );
                                response.context_menu(|ui| {
                                    changed |= self.column_menu(ui, session, Some(id))
                                });
                            });
                        }
                    })
                    .body(|body| {
                        if released && !changed {
                            for (id, &width) in columns.iter().zip(body.widths().iter().skip(1)) {
                                if session
                                    .column_widths
                                    .get(id)
                                    .is_none_or(|old| (*old - width).abs() > 1.0)
                                {
                                    session.column_widths.insert(id.clone(), width);
                                    changed = true;
                                }
                            }
                        }
                        body.rows(32.0, trace.rows.len() + 1, |mut row| {
                            let hop = trace.rows.get(row.index());
                            let ttl = hop.map_or(0, |h| h.ttl);
                            let stats = hop.map_or(trace.total, |h| h.stats);
                            row.set_selected(self.selected.contains(&ttl));
                            row.col(|ui| {
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(52.0, 32.0),
                                    egui::Sense::hover(),
                                );
                                let text = if ttl == 0 {
                                    "RT".into()
                                } else {
                                    ttl.to_string()
                                };
                                ui.painter().circle_filled(
                                    rect.center(),
                                    10.0,
                                    latency_color(stats.avg, &session.settings),
                                );
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    &text,
                                    egui::FontId::proportional(12.0),
                                    ui.visuals().extreme_bg_color,
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::Label, true, &text)
                                });
                            });
                            for id in &columns {
                                row.col(|ui| {
                                    let text = cell(id, hop, stats, hop.map_or(trace.total_extras, |h| h.extras));
                                    if id == "bar" {
                                        latency_bar(
                                            ui,
                                            stats,
                                            bar_max,
                                            &session.settings,
                                            &mut self.bar_zoom,
                                        );
                                    } else if id == "ip" || id == "name" {
                                        ui.add_space(10.0);
                                        ui.add(egui::Label::new(text).truncate());
                                    } else {
                                        // Only loss is coloured; the latency columns stay plain, as in
                                        // the legacy table, and the hop badge carries their grade.
                                        let color = match id.as_str() {
                                            "loss" if stats.loss_pct >= 10.0 => palette().bad,
                                            "loss" if stats.loss_pct > 0.0 => palette().warn,
                                            _ => ui.visuals().text_color(),
                                        };
                                        let tip = match id.as_str() {
                                            "loss" => Some(
                                                logic::explain_loss(
                                                    (stats.sent, stats.received),
                                                    (trace.total.sent, trace.total.received),
                                                    ttl == 0,
                                                )
                                                .to_owned(),
                                            ),
                                            id if model::EXTRA_COLUMNS.contains(&id) => {
                                                trace.details_period.flatten().map(|(from, to)| {
                                                    format!(
                                                        "Calculated for {} – {}",
                                                        time::date_time(from),
                                                        time::date_time(to)
                                                    )
                                                })
                                            }
                                            _ => None,
                                        };
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add_space(10.0);
                                                let label = ui.add(
                                                    egui::Label::new(
                                                        RichText::new(text).color(color),
                                                    )
                                                    .truncate(),
                                                );
                                                if let Some(tip) = tip {
                                                    label.on_hover_text(tip);
                                                }
                                            },
                                        );
                                    }
                                });
                            }
                            row.response().container_context_menu(|ui| {
                                self.hop_menu(ui, trace, hop, snapshot)
                            });
                            if row.response().clicked() {
                                if multiple {
                                    if self.selected.contains(&ttl) {
                                        self.selected.retain(|h| *h != ttl);
                                    } else {
                                        self.selected.push(ttl);
                                    }
                                } else {
                                    self.selected = vec![ttl];
                                }
                            }
                        });
                    });
            });
        // The splitter between the table and the graphs: drag it to give the graphs more
        // room, double-click it to fit the table to its rows again.
        // A stable id: one taken from the position would change as the splitter moves and
        // end the drag after the first frame.
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
        let response = ui
            .interact(rect, ui.scope_id().with("table-splitter"), egui::Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Resize table and graphs")
        });
        let active = response.hovered() || response.dragged();
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            if active {
                egui::Stroke::new(2.0, palette().accent)
            } else {
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
            },
        );
        if !active {
            ui.painter().hline(
                rect.center().x - 16.0..=rect.center().x + 16.0,
                rect.center().y,
                egui::Stroke::new(3.0, palette().muted),
            );
        }
        if response.double_clicked() {
            self.table_height = None;
            session.table_height = None;
            return true;
        }
        if response.dragged() {
            self.table_height = Some((height + response.drag_delta().y).max(0.0));
        }
        if response.drag_stopped() {
            session.table_height = self.table_height.take();
            changed = true;
        }
        changed
    }
    fn dialogs(&mut self, ctx: &egui::Context, session: &mut Session) -> bool {
        let mut changed = false;
        if let Some(mut redacted) = self.report_dialog {
            let snapshot = self.runtime.read();
            let [from, to] = snapshot.plot_range;
            let mut save = false;
            let modal = egui::Modal::new(egui::Id::unique("support-report"))
                .frame(egui::Frame::popup(&ctx.style_of(egui::Theme::Dark)).inner_margin(16))
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.strong("Export support report");
                    ui.add_space(8.0);
                    ui.label("Exports the displayed period with its graphs, probe settings, hop statistics, reply observations, alert incidents and route changes as one HTML file. Open it in a browser to print or save as PDF.");
                    ui.add_space(6.0);
                    ui.label(format!(
                        "{} – {}. Use the time span and graph slider to select another period before exporting.",
                        time::date_time(from),
                        time::date_time(to)
                    ));
                    ui.add_space(6.0);
                    ui.checkbox(&mut redacted, "Replace target names and addresses with consistent placeholders");
                    if snapshot.history_loading {
                        ui.weak("Waiting for the selected history to load…");
                    }
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        save = ui
                            .add_enabled(
                                !self.busy && !snapshot.history_loading,
                                egui::Button::new(if self.busy { "Preparing report…" } else { "Save report…" }),
                            )
                            .clicked();
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                    });
                });
            self.report_dialog = Some(redacted);
            if save {
                match self.gpu.clone() {
                    Some(gpu) => {
                        let snapshot = (*snapshot).clone();
                        let pixels_per_point = ctx.pixels_per_point();
                        self.job(ctx, move || {
                            Job::Message(export::support_report(&gpu, &snapshot, redacted, pixels_per_point))
                        });
                    }
                    None => self.error = Some("No renderer is available for the report's graphs.".into()),
                }
                self.report_dialog = None;
            } else if modal.should_close() {
                self.report_dialog = None;
            }
        }
        if let Some((id, host, restart)) = self.erasing.clone() {
            let mut confirmed = false;
            let modal = egui::Modal::new(egui::Id::unique("erase-history"))
                .frame(egui::Frame::popup(&ctx.style_of(egui::Theme::Dark)).inner_margin(16))
                .show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.strong(&host);
                    ui.add_space(8.0);
                    ui.label(if restart {
                        "Restart this trace and erase its recorded history?"
                    } else {
                        "Close this target and erase its recorded history?"
                    });
                    ui.add_space(16.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let action = if restart { "Restart and erase" } else { "Close and erase" };
                        confirmed = ui
                            .add(
                                egui::Button::new(RichText::new(action).color(palette().surface))
                                    .fill(palette().bad),
                            )
                            .clicked();
                        if ui.button("Cancel").clicked() {
                            ui.close();
                        }
                    });
                });
            if confirmed {
                self.runtime.command(if restart {
                    Command::Restart(id)
                } else {
                    Command::Close(id)
                });
                self.last_view = None;
                self.erasing = None;
            } else if modal.should_close() {
                self.erasing = None;
            }
        }
        if self.columns_open {
            let mut open = true;
            egui::Window::new("Columns")
                .open(&mut open)
                .show(ctx, |ui| {
                    let mut order = ordered_columns(session);
                    let mut movement = None;
                    for (i, id) in order.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let mut visible = !session.hidden_columns.contains(id);
                            if ui
                                .checkbox(
                                    &mut visible,
                                    COLUMNS.iter().find(|c| c.0 == id).map_or("", |c| c.1),
                                )
                                .changed()
                            {
                                session.hidden_columns.retain(|c| c != id);
                                if !visible {
                                    session.hidden_columns.push(id.clone());
                                }
                                changed = true;
                            }
                            if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                                movement = Some((i, i - 1));
                            }
                            if ui
                                .add_enabled(i + 1 < order.len(), egui::Button::new("Down"))
                                .clicked()
                            {
                                movement = Some((i, i + 1));
                            }
                        });
                    }
                    if let Some((a, b)) = movement {
                        order.swap(a, b);
                        session.column_order = order;
                        changed = true;
                    }
                });
            self.columns_open = open;
        }
        if let Some((config, new)) = &mut self.editing {
            let mut open = true;
            let mut apply = false;
            let modal = egui::Modal::new(egui::Id::unique("probe-and-alerts"))
                .frame(egui::Frame::popup(&ctx.style_of(egui::Theme::Dark)).inner_margin(14))
                .show(ctx, |ui| {
                    ui.set_width((ctx.content_rect().width() - 64.0).clamp(280.0, 530.0));
                    ui.strong(if *new {
                        "New target"
                    } else {
                        "Probe and alerts"
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 130.0).max(180.0))
                        .show(ui, |ui| {
                            egui::Grid::new("probe-form")
                                .num_columns(2)
                                .spacing([24.0, 8.0])
                                .show(ui, |ui| {
                                    let label = ui.label("Host");
                                    ui.add_enabled(
                                        *new,
                                        egui::TextEdit::singleline(&mut config.host),
                                    )
                                    .labelled_by(label.id);
                                    ui.end_row();
                                    ui.label("Packet type");
                                    if protocol(ui, &mut config.protocol) {
                                        config.port = match config.protocol.as_str() {
                                            "tcp" => 443,
                                            "udp" => 33434,
                                            _ => 0,
                                        };
                                    }
                                    ui.end_row();
                                    if ["udp", "tcp"].contains(&config.protocol.as_str()) {
                                        let label = ui.label("Port");
                                        ui.add(
                                            egui::DragValue::new(&mut config.port).range(1..=65535),
                                        )
                                        .labelled_by(label.id);
                                        ui.end_row();
                                    }
                                    ui.label("Interval");
                                    layout::interval_input(
                                        ui,
                                        "probe-interval",
                                        &mut self.probe_interval_text,
                                        &mut config.interval_ms,
                                        160.0,
                                        &mut self.error,
                                    );
                                    ui.end_row();
                                    for (label, value, min, max, suffix) in [
                                        ("Timeout", &mut config.timeout_ms, 100, 10000, " ms"),
                                        (
                                            "Alert latency (0 = off)",
                                            &mut config.alert_latency_ms,
                                            0,
                                            60000,
                                            " ms",
                                        ),
                                        (
                                            "Alert loss (0 = off)",
                                            &mut config.alert_loss_pct,
                                            0,
                                            100,
                                            " %",
                                        ),
                                        ("Alert window", &mut config.alert_seconds, 1, 3600, " s"),
                                    ] {
                                        let label = ui.label(label);
                                        ui.add(
                                            egui::DragValue::new(value)
                                                .range(min..=max)
                                                .suffix(suffix),
                                        )
                                        .labelled_by(label.id);
                                        ui.end_row();
                                    }
                                    let packet_label = ui.label("Packet size");
                                    ui.add(
                                        egui::DragValue::new(&mut config.packet_size)
                                            .range(48..=1024)
                                            .suffix(" bytes"),
                                    )
                                    .labelled_by(packet_label.id);
                                    ui.end_row();
                                });
                            ui.label(
                                "Fast probing can look like loss when routers throttle replies.",
                            );
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            open = false;
                        }
                        apply = ui
                            .add_enabled(!config.host.trim().is_empty(), egui::Button::new("Apply"))
                            .clicked();
                    });
                });
            open &= !modal.should_close();
            if apply {
                let (mut config, new) = self.editing.take().expect("edit dialog exists");
                match config.validate() {
                    Ok(()) => {
                        if new {
                            session.active = config.session_id.clone();
                            self.runtime.command(Command::Open(config));
                            changed = true;
                        } else {
                            self.runtime.command(Command::Edit(config));
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            } else if !open {
                self.editing = None;
            }
        }
        if let Some((addr, text)) = &self.whois {
            let mut open = true;
            egui::Window::new(format!("WHOIS {addr}"))
                .open(&mut open)
                .default_size([640.0, 500.0])
                .show(ctx, |ui| {
                    for fact in logic::whois_facts(text) {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(fact["label"].as_str().unwrap_or_default());
                            ui.label(fact["value"].as_str().unwrap_or_default());
                        });
                    }
                    if ui.button("Copy record").clicked() {
                        ctx.copy_text(logic::whois_record(text));
                    }
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.monospace(logic::whois_record(text));
                    });
                });
            if !open {
                self.whois = None;
            }
        }
        changed
    }
}
impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "dev-live")]
        if let Some(live) = self.live.as_mut() {
            if let Some(outcome) = live.poll() {
                eprintln!("Blaze: {outcome}");
                match outcome {
                    blaze_live::Outcome::Restart(reason) => {
                        self.runtime.command(Command::Save);
                        self.message = Some(format!(
                            "Blaze needs a restart: {reason}. Demo session is being saved."
                        ));
                    }
                    blaze_live::Outcome::Applied { .. } => {
                        self.message = Some("Blaze edit applied. Verify it with Cargo.".into())
                    }
                    blaze_live::Outcome::Declined(reason) => {
                        self.error = Some(format!("Blaze edit declined: {reason}"))
                    }
                }
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        #[cfg(debug_assertions)]
        if let Some(exit) = self.qa_exit {
            if std::time::Instant::now() >= exit {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
        }
        if self.needs_exit {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.restore_window(ui.ctx());
        let snapshot = self.runtime.read();
        let mut session = snapshot.session.clone();
        let mut changed = self.poll(&mut session);
        if self
            .last_view
            .as_ref()
            .is_some_and(|view| view.session != session.active && session.active != "settings")
        {
            self.selected = vec![0];
            self.end = None;
            self.pinned = None;
            self.last_view = None;
            if let Some(trace) = snapshot
                .traces
                .iter()
                .find(|t| t.config.session_id == session.active)
            {
                self.sync_toolbar(&trace.config);
            }
        }
        let shortcut = |key, modifiers| ui.input_mut(|i| i.consume_key(modifiers, key));
        if shortcut(egui::Key::T, egui::Modifiers::CTRL)
            || shortcut(egui::Key::N, egui::Modifiers::CTRL)
        {
            self.host.clear();
            self.select(&mut session, "summary".into());
            self.focus_host = true;
            changed = true;
        }
        if shortcut(egui::Key::O, egui::Modifiers::CTRL) {
            self.import(ui.ctx(), &session);
        }
        if shortcut(egui::Key::Comma, egui::Modifiers::CTRL) {
            self.select(&mut session, "settings".into());
            changed = true;
        }
        if shortcut(egui::Key::P, egui::Modifiers::CTRL) {
            self.export(ui, &snapshot, export::Kind::Pdf);
        }
        if shortcut(egui::Key::L, egui::Modifiers::CTRL) {
            self.end = None;
            self.pinned = None;
        }
        if shortcut(egui::Key::Tab, egui::Modifiers::CTRL)
            || shortcut(
                egui::Key::Tab,
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            )
        {
            let tabs = std::iter::once("summary".to_owned())
                .chain(snapshot.traces.iter().map(|t| t.config.session_id.clone()))
                .chain(["settings".into()])
                .collect::<Vec<_>>();
            let index = tabs
                .iter()
                .position(|id| *id == session.active)
                .unwrap_or(0);
            let shift = ui.input(|i| i.modifiers.shift);
            let next = if shift {
                (index + tabs.len() - 1) % tabs.len()
            } else {
                (index + 1) % tabs.len()
            };
            self.select(&mut session, tabs[next].clone());
            changed = true;
        }
        if let Some(trace) = snapshot
            .traces
            .iter()
            .find(|t| t.config.session_id == session.active)
        {
            if shortcut(egui::Key::W, egui::Modifiers::CTRL) {
                self.erase(trace, false);
            }
            if shortcut(egui::Key::P, egui::Modifiers::CTRL | egui::Modifiers::SHIFT)
                || (!ui.ctx().egui_wants_keyboard_input()
                    && ui.input(|i| i.key_pressed(egui::Key::Space)))
            {
                self.runtime
                    .command(Command::Pause(trace.config.session_id.clone()));
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.editing = None;
            self.pinned = None;
        }
        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                changed |= self.menus(ui, &snapshot, &mut session);
                changed |= self.tabs(ui, &snapshot, &mut session);
                changed |= self.toolbar(ui, &snapshot, &mut session);
            });
        if self.error.is_some()
            || snapshot.error.is_some()
            || self.message.is_some()
            || snapshot.history_loading
        {
            egui::Panel::bottom("status").show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(error) = self.error.as_ref().or(snapshot.error.as_ref()) {
                        ui.colored_label(palette().bad, error);
                        if ui.button("Dismiss").clicked() {
                            self.error = None;
                            self.runtime.command(Command::ClearError);
                        }
                    } else if let Some(message) = &self.message {
                        ui.label(message);
                        if small_button(ui, "Dismiss").clicked() {
                            self.message = None;
                        }
                    }
                    if snapshot.history_loading {
                        ui.spinner();
                        ui.label("Reading history…");
                    }
                });
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(palette().inset)).show(ui, |ui| {
            if session.active == "settings" {
                changed |= self.settings(ui, &mut session);
            } else {
                let active = snapshot
                    .traces
                    .iter()
                    .find(|t| t.config.session_id == session.active);
                if let Some(trace) = active {
                    if let Some(error) = &trace.error {
                        ui.colored_label(palette().bad, error);
                    }
                    changed |= self.table(ui, trace, &mut session, &snapshot);
                } else {
                    changed |= self.summary(ui, &snapshot, &mut session);
                    if snapshot.traces.is_empty() {
                        ui.add_space(16.0);
                        ui.label("Type a host above and press Enter or Start. Each target opens in its own tab.");
                    }
                }
                let old_span = session.span;
                changed |= self.period_bar(ui, &snapshot, active, &mut session);
                let graph_budget = (ui.available_height() - 35.0).max(0.0);
                let graph = egui::ScrollArea::vertical()
                    .id_salt("graphs-scroll")
                    .max_height(graph_budget)
                    .min_scrolled_height(graph_budget)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_height(graph_budget);
                        if snapshot.session.active != session.active || snapshot.session.span != session.span {
                            ui.label("Updating view…");
                            return graphs::Output { width: (ui.available_width() - 120.0).max(1.0), action: None };
                        }
                        graphs::show(ui, &snapshot, &mut self.end, &mut self.pinned, &mut session.span, session.active == "summary", self.selected_event.as_ref())
                    }).inner;
                self.plot_width = Some(graph.width);
                if let Some(action) = graph.action {
                    match action {
                        graphs::Action::Live => { self.end = None; self.pinned = None; }
                        graphs::Action::Event(event) => {
                            if let Some(trace) = active {
                                self.toggle_event(trace, &mut session.span, event);
                            }
                        }
                        graphs::Action::Close(ttl) => {
                            if self.selected.len() > 1 { self.selected.retain(|hop| *hop != ttl); }
                        }
                        graphs::Action::LossColors(value) => {
                            session.settings.loss_by_target = value;
                            changed = true;
                        }
                        graphs::Action::Export(kind, scope) => {
                            let mut image = (*snapshot).clone();
                            if let Some((id, ttl)) = scope {
                                image.plots.retain(|plot| plot.session == id && plot.ttl == ttl);
                            }
                            self.export(ui, &image, kind);
                        }
                    }
                }
                self.history_slider(ui, &snapshot, &session);
                // Back to live, however it happened, ends the inspection of an event.
                if self.end.is_none() {
                    self.selected_event = None;
                }
                changed |= session.span != old_span;
                let view = View {
                    session: if active.is_some() {
                        session.active.clone()
                    } else {
                        "summary".into()
                    },
                    hops: self.selected.clone(),
                    end: self.end,
                    width: (self
                        .plot_width
                        .unwrap_or((ui.available_width() - 120.0).max(1.0))
                        * ui.ctx().pixels_per_point())
                    .max(1.0) as usize,
                };
                if self.last_view.as_ref() != Some(&view) {
                    self.runtime.command(Command::View(view.clone()));
                    self.last_view = Some(view);
                }
                if self.events_open
                    && let Some(trace) = active
                {
                    self.event_list(ui.ctx(), trace, snapshot.plot_range, &mut session.span);
                }
            }
        });
        changed |= self.dialogs(ui.ctx(), &mut session);
        // Also saved once a move or resize settles, so a crash or a killed process keeps
        // it. The first seconds are skipped: the restored placement is still being applied.
        if let Some(tray) = &self.tray {
            let (close, minimized) = ui.input(|i| {
                (i.viewport().close_requested(), i.viewport().minimized == Some(true))
            });
            if close
                && self.background.close_to_tray
                && !tray.quitting.load(std::sync::atomic::Ordering::Relaxed)
            {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.save_window(ui.ctx());
                tray::hide(ui.ctx());
            } else if minimized && self.background.minimize_to_tray && !tray::wayland() {
                tray::hide(ui.ctx());
            }
        }
        let settled = ui.input(|i| i.time > 2.0 && !i.pointer.any_down());
        if self.remember_window
            && (ui.input(|i| i.viewport().close_requested())
                || (settled && self.placement(ui.ctx()) != self.window_state))
        {
            self.save_window(ui.ctx());
        }
        #[cfg(debug_assertions)]
        if demo()
            && !snapshot.history_loading
            && !self.captured
            && snapshot
                .traces
                .iter()
                .any(|t| !t.loading && !t.rows.is_empty())
            && (session.active == "summary" || !snapshot.plots.is_empty())
            && let Some(path) = std::env::var_os("TRACEPLOTTER_SCREENSHOT")
        {
            self.captured = true;
            let minimize = std::env::var_os("TRACEPLOTTER_QA_MINIMIZE").is_some();
            let ctx = ui.ctx().clone();
            ui.ctx().request_screenshot(move |image| {
                let pixels = image
                    .pixels
                    .iter()
                    .flat_map(|p| p.to_array())
                    .collect::<Vec<_>>();
                if let Err(e) = image::save_buffer(
                    path,
                    &pixels,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                ) {
                    eprintln!("Screenshot: {e}");
                }
                if minimize {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            });
        }
        if changed {
            self.preferences(session);
        }
        window_edges(ui.ctx());
        focus_ring(ui.ctx());
    }
}
/// A ring around whichever control has keyboard focus, drawn once for all of them: egui's
/// own focus look is a faint hover fill, and controls painted here have none at all.
fn focus_ring(ctx: &egui::Context) {
    // Only after Tab: a click that focuses a control needs no ring.
    let id = egui::Id::unique("keyboard-focus");
    let tab = ctx.input(|i| i.key_pressed(egui::Key::Tab));
    let click = ctx.input(|i| i.pointer.any_pressed());
    let keyboard = tab || (!click && ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false));
    ctx.data_mut(|d| d.insert_temp(id, keyboard));
    if !keyboard {
        return;
    }
    if let Some(response) = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)) {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("focus-ring")))
            .rect_stroke(response.rect.expand(1.0), 4.0, egui::Stroke::new(2.0, palette().accent), egui::StrokeKind::Outside);
    }
}
/// Menu rows sized for reading down a list; the toolbar's 32 px controls would leave them
/// sparse and hard to scan.
fn menu_rows(ui: &mut egui::Ui) {
    let spacing = ui.spacing_mut();
    spacing.interact_size.y = 26.0;
    spacing.item_spacing.y = 2.0;
    spacing.button_padding = egui::vec2(8.0, 4.0);
    ui.set_min_width(230.0);
}
/// A group's name inside a menu: small and muted, so it reads as a label, not a command.
fn menu_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).small().color(palette().muted));
}
/// A compact button that still meets the 24 px minimum target size.
fn small_button(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).small()).min_size(egui::vec2(0.0, 24.0)))
}
/// The window has no system frame: the free part of the menu row moves it, and these
/// buttons stand in for the system ones.
fn window_controls(ui: &mut egui::Ui) {
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut row = egui::Rangef::NOTHING;
        for (command, label) in [
            (egui::ViewportCommand::Close, "Close window"),
            (
                egui::ViewportCommand::Maximized(!maximized),
                if maximized { "Restore" } else { "Maximize" },
            ),
            (egui::ViewportCommand::Minimized(true), "Minimize"),
        ] {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(46.0, 40.0), egui::Sense::click());
            row = rect.y_range();
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
            let close = label == "Close window";
            if response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    if close { palette().bad.gamma_multiply(0.8) } else { palette().text.gamma_multiply(0.1) },
                );
            }
            let c = rect.center();
            let stroke = egui::Stroke::new(1.2, if response.hovered() { palette().text } else { palette().soft });
            let square = |c: egui::Pos2| egui::Rect::from_center_size(c, egui::vec2(9.0, 9.0));
            match label {
                "Close window" => {
                    for d in [egui::vec2(5.0, 5.0), egui::vec2(5.0, -5.0)] {
                        ui.painter().line_segment([c - d, c + d], stroke);
                    }
                }
                "Restore" => {
                    let back = square(c + egui::vec2(2.0, -2.0));
                    ui.painter().line_segment([back.left_top(), back.right_top()], stroke);
                    ui.painter().line_segment([back.right_top(), back.right_bottom()], stroke);
                    ui.painter().rect_stroke(
                        square(c + egui::vec2(-1.0, 1.0)),
                        0.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                "Maximize" => {
                    ui.painter()
                        .rect_stroke(square(c), 0.0, stroke, egui::StrokeKind::Inside);
                }
                _ => {
                    ui.painter().hline(c.x - 5.0..=c.x + 5.0, c.y, stroke);
                }
            }
            if response.on_hover_text(label).clicked() {
                ui.ctx().send_viewport_cmd(command);
            }
        }
        // Only the free stretch of the menu row: the space left below it belongs to the tabs.
        let free = egui::Rect::from_x_y_ranges(ui.available_rect_before_wrap().x_range(), row);
        let id = ui.scope_id().with("title-drag");
        let drag = ui.interact(free, id, egui::Sense::click());
        // The move starts on the press itself: waiting for egui's drag threshold loses
        // presses, as the compositor only accepts a move while the press is current. The
        // compositor then keeps the release, so double-clicks are timed from the presses.
        if drag.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
            let now = ui.input(|i| i.time);
            let last = ui.data_mut(|d| {
                let last = d.get_temp::<f64>(id);
                d.insert_temp(id, now);
                last
            });
            if last.is_some_and(|t| now - t < 0.4) {
                ui.data_mut(|d| d.remove::<f64>(id));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            } else {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }
    });
}
/// Without a system frame the window draws its own border, and its edges resize it.
/// Runs after the panels so that its cursor wins over the widgets under the edge.
fn window_edges(ctx: &egui::Context) {
    if ctx.input(|i| {
        i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
    }) {
        return;
    }
    let rect = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("window-border")))
        .rect_stroke(rect, 0.0, egui::Stroke::new(1.0, palette().line), egui::StrokeKind::Inside);
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    const RESIZE_EDGE: f32 = 5.0;
    let west = pos.x - rect.left() < RESIZE_EDGE;
    let east = rect.right() - pos.x < RESIZE_EDGE;
    let north = pos.y - rect.top() < RESIZE_EDGE;
    let south = rect.bottom() - pos.y < RESIZE_EDGE;
    use egui::{CursorIcon as C, ResizeDirection as D};
    let (direction, cursor) = match (west, east, north, south) {
        (true, _, true, _) => (D::NorthWest, C::ResizeNorthWest),
        (_, true, true, _) => (D::NorthEast, C::ResizeNorthEast),
        (true, _, _, true) => (D::SouthWest, C::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, C::ResizeSouthEast),
        (true, ..) => (D::West, C::ResizeWest),
        (_, true, ..) => (D::East, C::ResizeEast),
        (_, _, true, _) => (D::North, C::ResizeNorth),
        (.., true) => (D::South, C::ResizeSouth),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}
fn protocol(ui: &mut egui::Ui, value: &mut String) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(ui.next_auto_id())
        .selected_text(protocol_label(value))
        .show_ui(ui, |ui| {
            for p in ["winicmp", "icmp", "udp", "tcp"] {
                if p != "winicmp" || cfg!(any(windows, target_os = "linux")) {
                    changed |= ui
                        .selectable_value(value, p.into(), protocol_label(p))
                        .changed();
                }
            }
        })
        .response
        .on_hover_text("Packet type");
    changed
}
fn protocol_label(p: &str) -> &str {
    match p {
        "winicmp" => {
            if cfg!(windows) {
                "ICMP (Windows API)"
            } else {
                "ICMP (ping socket)"
            }
        }
        "icmp" => "ICMP (raw socket)",
        "udp" => "UDP (raw socket)",
        "tcp" => "TCP (raw socket)",
        _ => p,
    }
}
fn ordered_columns(session: &Session) -> Vec<String> {
    let mut columns = vec![];
    for id in session
        .column_order
        .iter()
        .map(String::as_str)
        .chain(COLUMNS.iter().map(|c| c.0))
    {
        if COLUMNS.iter().any(|c| c.0 == id) && !columns.iter().any(|c| c == id) {
            columns.push(id.into());
        }
    }
    columns
}
fn ms(value: Option<f64>) -> String {
    value.map_or_else(|| "–".into(), |v| format!("{v:.1} ms"))
}
fn span_label(span: f64) -> String {
    if span == 0.0 {
        "All history".into()
    } else if span >= 86400.0 {
        format!("{} days", span / 86400.0)
    } else if span >= 3600.0 {
        format!("{} hours", span / 3600.0)
    } else if span >= 60.0 {
        format!("{} minutes", span / 60.0)
    } else {
        format!("{span} seconds")
    }
}
fn latency_color(value: Option<f64>, settings: &logic::Settings) -> Color32 {
    match value {
        None => palette().muted,
        Some(v) if v >= settings.bad_ms as f64 => palette().bad,
        Some(v) if v >= settings.warn_ms as f64 => palette().warn,
        _ => palette().good,
    }
}
/// What a column header explains on hover, as the legacy table's titles do.
fn column_title(id: &str) -> Option<&'static str> {
    Some(match id {
        "ip" => "Address of the router that answered",
        "name" => "Its reverse DNS name, or the address when it has none",
        "loss" => "Share of probes that got no reply",
        "sent" => "Probes sent in the period",
        "recv" => "Probes that got a reply",
        "last" => "Latest reply in ms",
        "avg" => "Average reply in ms",
        "min" => "Fastest reply in ms",
        "max" => "Slowest reply in ms",
        "jitter" => "Average change between consecutive replies in ms",
        "mos" => "Estimated call quality, from 1 (bad) to 4.5 (excellent)",
        "bar" => "Range, average and latest reply on one scale, with loss as a red bar",
        "median" => "Middle reply latency in ms",
        "p95" => "95% of replies were at or below this latency in ms",
        "p99" => "99% of replies were at or below this latency in ms",
        "lossBurst" => {
            "Longest consecutive run of missed probes; duration is estimated from probe timing"
        }
        _ => return None,
    })
}
fn cell(id: &str, hop: Option<&app::Row>, s: logic::Stats, extras: Option<logic::Extras>) -> String {
    let detail = |value: fn(&logic::Extras) -> Option<f64>| {
        extras.map_or_else(|| "…".into(), |e| value(&e).map_or_else(|| "–".into(), |v| format!("{v:.1}")))
    };
    match id {
        "median" => detail(|e| e.median),
        "p95" => detail(|e| e.p95),
        "p99" => detail(|e| e.p99),
        "lossBurst" => extras.map_or_else(
            || "…".into(),
            |e| {
                format!(
                    "{} {} · {:.1} s",
                    e.longest_lost,
                    if e.longest_lost == 1 { "probe" } else { "probes" },
                    e.longest_loss_seconds
                )
            },
        ),
        "ip" => hop.map_or_else(|| "Round trip".into(), |h| h.addr.clone()),
        "name" => hop.map_or_else(String::new, |h| {
            if h.name.is_empty() {
                h.addr.clone()
            } else {
                h.name.clone()
            }
        }),
        "loss" => format!("{:.1}%", s.loss_pct),
        "sent" => s.sent.to_string(),
        "recv" => s.received.to_string(),
        "mos" => logic::mos(s).map_or_else(|| "–".into(), |v| format!("{v:.2}")),
        _ => {
            let value = match id {
                "last" => s.last,
                "avg" => s.avg,
                "min" => s.min,
                "max" => s.max,
                "jitter" => s.jitter,
                _ => None,
            };
            value.map_or_else(|| "–".into(), |v| format!("{v:.1}"))
        }
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;

fn spark(ui: &mut egui::Ui, values: &[Option<f64>]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 24.0), egui::Sense::hover());
    let top = values.iter().flatten().copied().fold(1.0, f64::max);
    let mut previous = None;
    for (i, value) in values.iter().enumerate() {
        let next = value.map(|v| {
            egui::pos2(
                rect.left()
                    + i as f32 / values.len().saturating_sub(1).max(1) as f32 * rect.width(),
                rect.bottom() - 1.0 - (v / top) as f32 * (rect.height() - 2.0),
            )
        });
        if let (Some(a), Some(b)) = (previous, next) {
            ui.painter()
                .line_segment([a, b], egui::Stroke::new(1.5, palette().accent));
        }
        previous = next;
    }
}

fn latency_bar(
    ui: &mut egui::Ui,
    stats: logic::Stats,
    max: f64,
    settings: &logic::Settings,
    zoom: &mut f64,
) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ui.available_height()),
        egui::Sense::click(),
    );
    response.clone().on_hover_text(format!("Latency 0–{:.1} ms. Min {}, average {}, max {}. Wheel or + and - zoom; double-click or 0 fits all values.",max / *zoom,ms(stats.min),ms(stats.avg),ms(stats.max)));
    if response.clicked() {
        response.request_focus();
    }
    if response.double_clicked()
        || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Num0)))
    {
        *zoom = 1.0;
    }
    if response.hovered() {
        let scroll = ui.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| {
                    if let egui::Event::MouseWheel { delta, .. } = e {
                        Some(delta.y)
                    } else {
                        None
                    }
                })
                .sum::<f32>()
        });
        if scroll != 0.0 {
            *zoom = (*zoom * if scroll > 0.0 { 1.25 } else { 0.8 }).clamp(1.0, 64.0);
        }
    }
    if response.has_focus() {
        if ui.input(|i| i.key_pressed(egui::Key::Plus)) {
            *zoom = (*zoom * 1.25).min(64.0);
        } else if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
            *zoom = (*zoom * 0.8).max(1.0);
        }
    }
    let scale = max / *zoom;
    let x = |v: f64| rect.left() + (v / scale).clamp(0.0, 1.0) as f32 * rect.width();
    let painter = ui.painter_at(rect);
    // Good, slow and bad latency zones behind the marks, on the same scale as the marks.
    let edges = [0.0, settings.warn_ms as f64, settings.bad_ms as f64, f64::INFINITY];
    for (band, (color, strength)) in [(palette().good, 0.32), (palette().warn, 0.32), (palette().bad, 0.34)]
        .into_iter()
        .enumerate()
    {
        let zone = egui::Rect::from_x_y_ranges(x(edges[band])..=x(edges[band + 1]), rect.y_range());
        painter.rect_filled(zone, 0.0, color.gamma_multiply(strength));
    }
    // Packet loss is a bar from the left, where the full width is 100 %.
    if stats.loss_pct > 0.0 {
        let width = (stats.loss_pct / 100.0) as f32 * rect.width();
        painter.rect_filled(
            egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 6.0),
                egui::vec2(width.max(2.0), rect.height() - 12.0),
            ),
            0.0,
            palette().bad.gamma_multiply(0.75),
        );
    }
    if let (Some(min), Some(maximum), Some(avg)) = (stats.min, stats.max, stats.avg) {
        let y = rect.center().y;
        painter.line_segment(
            [egui::pos2(x(min), y), egui::pos2(x(maximum), y)],
            egui::Stroke::new(2.0, palette().soft),
        );
        let center = egui::pos2(x(avg), y);
        painter.circle(
            center,
            3.5,
            palette().inset,
            egui::Stroke::new(2.0, latency_color(Some(avg), settings)),
        );
        if let Some(last) = stats.last {
            painter.text(
                egui::pos2(x(last), y),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(13.0),
                palette().accent,
            );
        }
        if maximum > scale {
            painter.text(
                rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                "›",
                egui::FontId::proportional(14.0),
                palette().text,
            );
        }
    }
}

fn fonts() -> &'static egui::FontDefinitions {
    static FONTS: OnceLock<egui::FontDefinitions> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = egui::FontDefinitions::default();
        #[cfg(windows)]
        let paths = {
            let root = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            vec![
                ("system", root.join("Fonts/segoeui.ttf")),
                ("cjk", root.join("Fonts/msyh.ttc")),
            ]
        };
        // Distributions keep the same fonts in different folders: Debian, Arch, Fedora.
        #[cfg(not(windows))]
        let paths = [
            (
                "system",
                &[
                    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                    "/usr/share/fonts/TTF/DejaVuSans.ttf",
                    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
                    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
                ][..],
            ),
            (
                "cjk",
                &[
                    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
                    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
                ][..],
            ),
        ]
        .map(|(name, candidates)| {
            let found = candidates.iter().map(PathBuf::from).find(|p| p.exists());
            (name, found.unwrap_or_default())
        });
        for (name, path) in paths {
            if let Ok(bytes) = fs::read(path) {
                fonts
                    .font_data
                    .insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
                let proportional = fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default();
                if name == "system" {
                    proportional.insert(0, name.into());
                } else {
                    proportional.push(name.into());
                    fonts
                        .families
                        .entry(egui::FontFamily::Monospace)
                        .or_default()
                        .push(name.into());
                }
            }
        }
        fonts
    })
}
/// The interface's colours, after MonoCode: near-black surfaces, grey text and one blue
/// accent, with colour kept for state so that a hop that is fine stays quiet.
#[derive(Clone, Copy)]
pub(super) struct Palette {
    /// Black, for marks and glyphs drawn on a filled colour.
    pub surface: Color32,
    /// MonoCode's shell: the title bar, tabs, toolbar and settings navigation.
    pub shell: Color32,
    /// MonoCode's inset pane, a step darker: the table, graphs and settings pages.
    pub inset: Color32,
    pub raised: Color32,
    pub input: Color32,
    pub line: Color32,
    /// The edge of a control: 3:1 against the shell, as WCAG asks of control boundaries.
    pub edge: Color32,
    pub text: Color32,
    pub soft: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_light: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
}
const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
/// MonoCode's Ink theme.
// Ink's greys come from MonoCode's default neutral shell, hsl(240, 0%, 9%), and its inset
// pane at 45% of that lightness.
const INK: Palette = Palette {
    surface: rgb(0x000000),
    shell: rgb(0x171717),
    inset: rgb(0x0a0a0a),
    raised: rgb(0x1f1f1f),
    input: rgb(0x0f0f0f),
    line: rgb(0x2a2a2a),
    edge: rgb(0x646464),
    text: rgb(0xf5f5f5),
    soft: rgb(0xd6d6d6),
    muted: rgb(0x8f8f8f),
    accent: rgb(0x7aa7ff),
    accent_light: rgb(0x9cc0ff),
    good: rgb(0x8fd6a8),
    warn: rgb(0xf2c46d),
    bad: rgb(0xff6b6b),
};
/// The colours the interface paints with.
pub(super) fn palette() -> &'static Palette {
    &INK
}
fn apply_theme(ctx: &egui::Context) {
    ctx.set_fonts(fonts().clone());
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = palette().shell;
    visuals.window_fill = palette().raised;
    visuals.extreme_bg_color = palette().input;
    visuals.faint_bg_color = palette().raised;
    visuals.window_stroke = egui::Stroke::new(1.0, palette().line);
    visuals.hyperlink_color = palette().accent_light;
    visuals.warn_fg_color = palette().warn;
    visuals.error_fg_color = palette().bad;
    visuals.selection.bg_fill = palette().accent.gamma_multiply(0.3);
    visuals.selection.stroke = egui::Stroke::new(1.0, palette().text);
    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = palette().shell;
    w.noninteractive.weak_bg_fill = palette().shell;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, palette().line);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, palette().soft);
    // Controls are lifted by translucent ink, not by a tint of their own.
    for (state, fill, text) in [
        (&mut w.inactive, 0.08, palette().soft),
        (&mut w.hovered, 0.14, palette().text),
        (&mut w.active, 0.2, palette().text),
        (&mut w.open, 0.14, palette().text),
    ] {
        state.bg_fill = palette().text.gamma_multiply(fill);
        state.weak_bg_fill = state.bg_fill;
        state.fg_stroke = egui::Stroke::new(1.0, text);
    }
    w.inactive.bg_stroke = egui::Stroke::new(1.0, palette().edge);
    w.hovered.bg_stroke = egui::Stroke::new(1.0, palette().muted);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.interact_size.y = 32.0;
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
    });
}

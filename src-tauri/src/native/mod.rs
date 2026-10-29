//! Native desktop interface. Probing and agent requests continue without paints.
use super::*;
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;
use std::sync::mpsc;
mod agent;
mod app;
mod export;
mod graphs;
mod import;
pub(super) mod migration;
mod model;
mod time;
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
        .with_min_inner_size([640.0, 420.0]);
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
const COLUMNS: [(&str, &str); 12] = [
    ("ip", "IP address"),
    ("name", "Name"),
    ("loss", "Loss %"),
    ("sent", "Sent"),
    ("recv", "Received"),
    ("last", "Last"),
    ("avg", "Average"),
    ("min", "Min"),
    ("max", "Max"),
    ("jitter", "Jitter"),
    ("mos", "MOS"),
    ("bar", "Latency"),
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
    selected: Vec<u8>,
    last_view: Option<View>,
    error: Option<String>,
    message: Option<String>,
    end: Option<f64>,
    pinned: Option<f64>,
    editing: Option<(Config, bool)>,
    columns_open: bool,
    events_open: bool,
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
    focus_host: bool,
    table_height: Option<f32>,
    bar_zoom: f64,
    needs_exit: bool,
    gpu: Option<eframe::egui_wgpu::RenderState>,
    span_text: String,
    interval_text: String,
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
        let host = {
            let snapshot = runtime.read();
            snapshot
                .session
                .targets
                .iter()
                .find(|t| t.session_id == snapshot.session.active)
                .map_or_else(String::new, |t| t.host.clone())
        };
        Self {
            runtime,
            host,
            selected: vec![0],
            last_view: None,
            error: None,
            message: None,
            end: None,
            pinned: None,
            editing: None,
            columns_open: false,
            events_open: false,
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
            window_state: load_window_state(),
            focus_host: false,
            table_height: None,
            bar_zoom: 1.0,
            needs_exit: false,
            gpu: None,
            span_text: String::new(),
            interval_text: String::new(),
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
            self.host = trace.config.host.clone();
        }
        self.end = None;
        self.pinned = None;
        self.last_view = None;
    }
    fn erase(&mut self, trace: &TraceView, restart: bool) {
        let text = if restart {
            "Restart this trace and erase its recorded history?"
        } else {
            "Close this target and erase its recorded history?"
        };
        if rfd::MessageDialog::new()
            .set_title(&trace.config.host)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
        {
            self.runtime.command(if restart {
                Command::Restart(trace.config.session_id.clone())
            } else {
                Command::Close(trace.config.session_id.clone())
            });
            self.last_view = None;
        }
    }
    fn save_window(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            let v = i.viewport();
            if v.minimized == Some(true) {
                return;
            }
            let p = i.pixels_per_point();
            if let (Some(outer), Some(inner)) = (v.outer_rect, v.inner_rect) {
                let maximized = v.maximized.unwrap_or(false);
                let placed = WindowState {
                    x: (outer.min.x * p) as i32,
                    y: (outer.min.y * p) as i32,
                    width: (inner.width() * p) as u32,
                    height: (inner.height() * p) as u32,
                    maximized,
                };
                let state = Some(if maximized {
                    self.window_state
                        .map_or(placed, |s| WindowState { maximized, ..s })
                } else {
                    placed
                });
                if let Some(state) = state {
                    self.window_state = Some(state);
                    if let Ok(bytes) = serde_json::to_vec(&state)
                        && let Err(e) = model::atomic_write(&data_dir().join("window.json"), &bytes)
                    {
                        self.error = Some(e);
                    }
                }
            }
        });
    }
    fn restore_window(&mut self, ctx: &egui::Context) {
        if self.placed {
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
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New target       Ctrl+T").clicked() {
                    self.host.clear();
                    self.select(session, "summary".into());
                    self.focus_host = true;
                    changed = true;
                    ui.close();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Import samples…  Ctrl+O"))
                    .clicked()
                {
                    self.import(ui.ctx(), session);
                    ui.close();
                }
                ui.separator();
                ui.add_enabled_ui(
                    !self.busy
                        && (active.is_some()
                            || (session.active == "summary" && !snapshot.traces.is_empty())),
                    |ui| {
                        for (label, kind) in [
                            ("Save report text…", export::Kind::Text),
                            ("Save hop table CSV…", export::Kind::Csv),
                            ("Save all samples CSV…", export::Kind::Samples),
                            ("Save plotted stretches CSV…", export::Kind::Stretches),
                            ("Save graphs PNG…", export::Kind::Png),
                            ("Copy graphs image", export::Kind::CopyImage),
                            ("Save report PDF…", export::Kind::Pdf),
                        ] {
                            if ui.button(label).clicked() {
                                self.export(ui, snapshot, kind);
                                ui.close();
                            }
                        }
                    },
                );
                if let Some(trace) = active
                    && ui.button("Close tab and erase history…   Ctrl+W").clicked()
                {
                    self.erase(trace, false);
                    ui.close();
                }
                if ui.button("Settings…   Ctrl+,").clicked() {
                    self.select(session, "settings".into());
                    changed = true;
                    ui.close();
                }
                if ui.button("Copy visible hop table report").clicked() {
                    ui.ctx().copy_text(export::report(snapshot, false));
                    ui.close();
                }
                if ui.button("Quit").clicked() {
                    self.save_window(ui.ctx());
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Trace", |ui| {
                if let Some(t) = active {
                    if ui
                        .button(if t.paused {
                            "Resume    Space"
                        } else {
                            "Pause     Space"
                        })
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
                    if ui.button("Restart and erase history…").clicked() {
                        self.erase(t, true);
                        ui.close();
                    }
                    if ui.button("Close and erase history…  Ctrl+W").clicked() {
                        self.erase(t, false);
                        ui.close();
                    }
                    if ui.button("Route and alert events").clicked() {
                        self.events_open = true;
                        ui.close();
                    }
                } else {
                    ui.label("Select a target tab.");
                }
                ui.separator();
                if ui.button("Pause all").clicked() {
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
                if ui.button("All targets").clicked() {
                    self.select(session, "summary".into());
                    changed = true;
                    ui.close();
                }
                if ui.button("Columns…").clicked() {
                    self.columns_open = true;
                    ui.close();
                }
                if ui.button("Return to live   Ctrl+L").clicked() {
                    self.end = None;
                    self.pinned = None;
                    ui.close();
                }
                let available = snapshot
                    .traces
                    .iter()
                    .map(|t| t.to - t.from)
                    .reduce(f64::max)
                    .unwrap_or(600.0);
                if ui.button("Zoom in").clicked() {
                    session.span = graphs::zoom(session.span, false, available);
                    changed = true;
                    ui.close();
                }
                if ui.button("Zoom out").clicked() {
                    session.span = graphs::zoom(session.span, true, available);
                    changed = true;
                    ui.close();
                }
                if ui.button("Settings").clicked() {
                    self.select(session, "settings".into());
                    changed = true;
                    ui.close();
                }
            });
            ui.menu_button("Tools", |ui| {
                changed |= ui
                    .checkbox(&mut session.mcp, "Allow AI agents to connect (MCP)")
                    .changed();
                if let Some(url) = &snapshot.mcp_url {
                    ui.label(url);
                    if ui.button("Copy MCP address").clicked() {
                        ui.ctx().copy_text(url.clone());
                    }
                }
                if ui
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
                ui.label(if is_elevated() {
                    "Raw sockets available"
                } else {
                    "Raw sockets need additional rights"
                });
            });
            ui.menu_button("Help", |ui| {
                ui.label(concat!("TracePlotter ", env!("CARGO_PKG_VERSION")));
                ui.label("Native egui preview");
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
                if ui.button("Project and releases").clicked() {
                    if let Err(e) = open_project_page() {
                        self.error = Some(e);
                    }
                    ui.close();
                }
                changed |= ui
                    .checkbox(&mut session.check_updates, "Check updates at startup")
                    .changed();
            });
            if self.busy {
                ui.spinner();
                ui.weak("Working…");
            }
            if demo() {
                ui.colored_label(Color32::from_rgb(255, 195, 75), "DEMO");
            }
        });
        changed
    }
    fn settings(&mut self, ui: &mut egui::Ui, session: &mut Session) -> bool {
        let mut changed = false;
        ui.heading("Settings");
        ui.label(
            "Defaults apply to new targets. Each open target keeps its probe and alert settings.",
        );
        egui::ScrollArea::vertical().show(ui,|ui|{
            let s=&mut session.settings;
            egui::Grid::new("defaults").num_columns(2).spacing([30.0,10.0]).show(ui,|ui|{
                ui.label("Packet type");changed|=protocol(ui,&mut s.protocol);ui.end_row();
                let label=ui.label("Interval");
                egui::ComboBox::from_id_salt("default-interval").selected_text(format!("{} seconds",s.interval_ms as f64/1000.0)).show_ui(ui,|ui|{
                    for ms in [500,1000,2500,5000,10000]{changed|=ui.selectable_value(&mut s.interval_ms,ms,format!("{} seconds",ms as f64/1000.0)).changed();}
                }).response.labelled_by(label.id);ui.end_row();
                for(label,value,min,max,suffix)in[("Timeout",&mut s.timeout_ms,100,10000," ms"),("Alert latency (0 = off)",&mut s.alert_latency_ms,0,60000," ms"),("Alert loss (0 = off)",&mut s.alert_loss_pct,0,100," %"),("Alert window",&mut s.alert_seconds,1,3600," s"),("Warning latency",&mut s.warn_ms,1,10000," ms"),("Bad latency",&mut s.bad_ms,2,60000," ms"),("Keep history",&mut s.history_days,1,365," days")]{
                    let label=ui.label(label);changed|=ui.add(egui::DragValue::new(value).range(min..=max).suffix(suffix)).labelled_by(label.id).changed();ui.end_row();
                }
                let label=ui.label("Packet size");changed|=ui.add(egui::DragValue::new(&mut s.packet_size).range(48..=1024).suffix(" bytes")).labelled_by(label.id).changed();ui.end_row();
                let label=ui.label("MCP port");changed|=ui.add(egui::DragValue::new(&mut s.mcp_port).range(1024..=65535)).labelled_by(label.id).changed();ui.end_row();
            });
            s.bad_ms=s.bad_ms.max(s.warn_ms+1);
            changed|=ui.checkbox(&mut s.notifications,"Desktop notifications for alerts and route changes").changed();
            changed|=ui.checkbox(&mut s.loss_by_target,"Colour comparison loss bars by target").changed();
            changed|=ui.checkbox(&mut session.mcp,"Allow AI agents to connect to the loopback MCP server").changed();
            ui.label("Agent probes never run faster than once a second. Stop sharing by clearing the checkbox.");
            if ui.button("Restore default settings").clicked(){s.clone_from(&Session::default().settings);changed=true;}
        });
        changed
    }
    fn table(&mut self, ui: &mut egui::Ui, trace: &TraceView, session: &mut Session) -> bool {
        let mut changed = false;
        let columns = ordered_columns(session)
            .into_iter()
            .filter(|id| !session.hidden_columns.contains(id))
            .collect::<Vec<_>>();
        let height = self
            .table_height
            .or(session.table_height)
            .unwrap_or(242.0)
            .clamp(65.0, (ui.available_height() - 250.0).max(65.0));
        egui::ScrollArea::horizontal()
            .id_salt("table-horizontal")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let released = ui.input(|i| i.pointer.any_released());
                let multiple = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                let mut table = egui_extras::TableBuilder::new(ui)
                    .id_salt(("hops", columns.join(",")))
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .max_scroll_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .column(egui_extras::Column::initial(38.0).at_least(32.0));
                for id in &columns {
                    table =
                        table.column(
                            egui_extras::Column::initial(
                                session.column_widths.get(id).copied().unwrap_or(
                                    match id.as_str() {
                                        "ip" => 120.0,
                                        "name" => 160.0,
                                        "bar" => 130.0,
                                        _ => 67.0,
                                    },
                                ),
                            )
                            .at_least(40.0),
                        );
                }
                table
                    .header(23.0, |mut row| {
                        row.col(|ui| {
                            ui.strong("Hop");
                        });
                        for id in &columns {
                            row.col(|ui| {
                                ui.strong(COLUMNS.iter().find(|c| c.0 == id).map_or("", |c| c.1));
                            });
                        }
                    })
                    .body(|body| {
                        if released {
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
                        body.rows(22.0, trace.rows.len() + 1, |mut row| {
                            let hop = trace.rows.get(row.index());
                            let ttl = hop.map_or(0, |h| h.ttl);
                            let stats = hop.map_or(trace.total, |h| h.stats);
                            row.set_selected(self.selected.contains(&ttl));
                            row.col(|ui| {
                                ui.label(if ttl == 0 {
                                    "RT".into()
                                } else {
                                    ttl.to_string()
                                });
                            });
                            for id in &columns {
                                row.col(|ui| {
                                    let text = cell(id, hop, stats);
                                    if id == "bar" {
                                        let max = trace
                                            .rows
                                            .iter()
                                            .filter_map(|h| h.stats.max)
                                            .fold(1.0, f64::max);
                                        latency_bar(
                                            ui,
                                            stats,
                                            max,
                                            &session.settings,
                                            &mut self.bar_zoom,
                                        );
                                    } else if id == "ip" {
                                        let response = ui.add(egui::Label::new(text).truncate());
                                        response.context_menu(|ui| {
                                            if ui.button("Copy address").clicked() {
                                                ui.ctx().copy_text(hop.map_or_else(
                                                    || trace.addr.clone(),
                                                    |h| h.addr.clone(),
                                                ));
                                                ui.close();
                                            }
                                            if let Some(addr) =
                                                hop.and_then(|h| h.addr.parse::<IpAddr>().ok())
                                                && ui
                                                    .add_enabled(
                                                        !self.busy,
                                                        egui::Button::new("WHOIS…"),
                                                    )
                                                    .clicked()
                                            {
                                                self.job(ui.ctx(), move || {
                                                    Job::Whois(addr, whois(addr))
                                                });
                                                ui.close();
                                            }
                                        });
                                    } else if id == "name" {
                                        ui.add(egui::Label::new(text).truncate());
                                    } else {
                                        let color = if ["last", "avg", "min", "max"]
                                            .contains(&id.as_str())
                                        {
                                            latency_color(
                                                match id.as_str() {
                                                    "last" => stats.last,
                                                    "avg" => stats.avg,
                                                    "min" => stats.min,
                                                    _ => stats.max,
                                                },
                                                &session.settings,
                                            )
                                        } else if id == "loss" && stats.loss_pct > 0.0 {
                                            Color32::from_rgb(245, 124, 137)
                                        } else {
                                            ui.visuals().text_color()
                                        };
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(text)
                                                            .monospace()
                                                            .color(color),
                                                    )
                                                    .truncate(),
                                                );
                                            },
                                        );
                                    }
                                });
                            }
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
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::drag());
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
        if response.dragged() {
            self.table_height = Some((height + response.drag_delta().y).max(65.0));
        }
        if response.drag_stopped() {
            session.table_height = self.table_height.take();
            changed = true;
        }
        changed
    }
    fn dialogs(&mut self, ctx: &egui::Context, session: &mut Session) -> bool {
        let mut changed = false;
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
            egui::Window::new(if *new {
                "New target"
            } else {
                "Probe and alerts"
            })
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("probe-form")
                    .num_columns(2)
                    .spacing([24.0, 8.0])
                    .show(ui, |ui| {
                        let label = ui.label("Host");
                        ui.add_enabled(*new, egui::TextEdit::singleline(&mut config.host))
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
                            ui.add(egui::DragValue::new(&mut config.port).range(1..=65535))
                                .labelled_by(label.id);
                            ui.end_row();
                        }
                        for (label, value, min, max, suffix) in [
                            ("Interval", &mut config.interval_ms, 100, 60000, " ms"),
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
                            ui.add(egui::DragValue::new(value).range(min..=max).suffix(suffix))
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
                ui.horizontal(|ui| {
                    ui.label("Typed interval");
                    let input = ui
                        .text_edit_singleline(&mut self.interval_text)
                        .on_hover_text("Interval such as 750ms or 1.5s");
                    if input.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Some(ms) = logic::parse_interval(&self.interval_text)
                    {
                        config.interval_ms = ms;
                    }
                });
                ui.label("Fast probing can look like loss when routers throttle replies.");
                apply = ui
                    .add_enabled(!config.host.trim().is_empty(), egui::Button::new("Apply"))
                    .clicked();
            });
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
                self.host = trace.config.host.clone();
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
        egui::Panel::top("toolbar").show(ui, |ui| {
            changed |= self.menus(ui, &snapshot, &mut session);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("TracePlotter").strong().size(18.0));
                let host_label = ui.label("Host");
                let input = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.host)
                            .hint_text("Address or host name")
                            .desired_width((ui.available_width() - 210.0).max(100.0)),
                    )
                    .labelled_by(host_label.id);
                if self.focus_host {
                    input.request_focus();
                    self.focus_host = false;
                }
                if (ui.button("Start trace").clicked()
                    || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                    && !self.host.trim().is_empty()
                {
                    let config = Config::new(self.host.clone(), &session.settings);
                    self.select(&mut session, config.session_id.clone());
                    changed = true;
                    self.runtime.command(Command::Open(config));
                }
                if ui.button("Advanced…").clicked() {
                    self.editing = Some((Config::new(self.host.clone(), &session.settings), true));
                }
            });
            egui::ScrollArea::horizontal()
                .id_salt("tabs")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(session.active == "summary", "All targets")
                            .clicked()
                        {
                            self.select(&mut session, "summary".into());
                            changed = true;
                        }
                        for trace in &snapshot.traces {
                            let label = if trace.paused {
                                format!("{} (paused)", trace.config.host)
                            } else if trace.alerting {
                                format!("{} (alert)", trace.config.host)
                            } else {
                                trace.config.host.clone()
                            };
                            let response = ui
                                .selectable_label(session.active == trace.config.session_id, label);
                            if response.clicked() {
                                self.select(&mut session, trace.config.session_id.clone());
                                changed = true;
                            }
                            response.context_menu(|ui| {
                                if ui.button("Edit…").clicked() {
                                    self.editing = Some((trace.config.clone(), false));
                                    ui.close();
                                }
                                if ui.button("Close and erase history…").clicked() {
                                    self.erase(trace, false);
                                    ui.close();
                                }
                            });
                        }
                        if ui
                            .selectable_label(session.active == "settings", "Settings")
                            .clicked()
                        {
                            self.select(&mut session, "settings".into());
                            changed = true;
                        }
                    });
                });
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(error) = self.error.as_ref().or(snapshot.error.as_ref()) {
                    ui.colored_label(Color32::LIGHT_RED, error);
                    if ui.button("Dismiss").clicked() {
                        self.error = None;
                        self.runtime.command(Command::ClearError);
                    }
                } else if let Some(message) = &self.message {
                    ui.label(message);
                    if ui.small_button("Dismiss").clicked() {
                        self.message = None;
                    }
                } else {
                    ui.weak(if self.end.is_some() {
                        "History view — return to live to follow new probes"
                    } else {
                        "Live — Ctrl+click table rows to compare hops"
                    });
                }
                if snapshot.history_loading {
                    ui.spinner();
                    ui.label("Reading history…");
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if session.active == "settings" {
                changed |= self.settings(ui, &mut session);
            } else {
                let active = snapshot
                    .traces
                    .iter()
                    .find(|t| t.config.session_id == session.active);
                if let Some(trace) = active {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&trace.config.host);
                        ui.weak(&trace.addr);
                        ui.label(format!(
                            "{} · {} ms",
                            trace.config.protocol, trace.config.interval_ms
                        ));
                        if ui
                            .button(if trace.paused { "Resume" } else { "Pause" })
                            .clicked()
                        {
                            self.runtime
                                .command(Command::Pause(trace.config.session_id.clone()));
                        }
                        if ui.button("Edit…").clicked() {
                            self.editing = Some((trace.config.clone(), false));
                        }
                        if ui.button("Events").clicked() {
                            self.events_open = !self.events_open;
                        }
                        if trace.loading {
                            ui.spinner();
                        }
                    });
                    if let Some(error) = &trace.error {
                        ui.colored_label(Color32::LIGHT_RED, error);
                    }
                    changed |= self.table(ui, trace, &mut session);
                } else {
                    ui.heading("All targets");
                    egui::ScrollArea::both().max_height(170.0).show(ui, |ui| {
                        egui::Grid::new("summary")
                            .striped(true)
                            .spacing([18.0, 7.0])
                            .show(ui, |ui| {
                                for label in [
                                    "Compare", "Target", "State", "Average", "Loss", "Jitter",
                                    "MOS", "Trend",
                                ] {
                                    ui.strong(label);
                                }
                                ui.end_row();
                                for trace in &snapshot.traces {
                                    let id = &trace.config.session_id;
                                    let mut checked = session.compare.contains(id);
                                    if ui
                                        .checkbox(&mut checked, "")
                                        .on_hover_text(format!("Compare {}", trace.config.host))
                                        .changed()
                                    {
                                        session.compare.retain(|s| s != id);
                                        if checked {
                                            session.compare.push(id.clone());
                                        }
                                        self.runtime
                                            .command(Command::Compare(session.compare.clone()));
                                        changed = true;
                                    }
                                    if ui.link(&trace.config.host).clicked() {
                                        self.select(&mut session, id.clone());
                                        changed = true;
                                    }
                                    ui.label(if trace.paused {
                                        "Paused"
                                    } else if trace.alerting {
                                        "Alert"
                                    } else if trace.loading {
                                        "Starting…"
                                    } else {
                                        "Running"
                                    });
                                    ui.colored_label(
                                        latency_color(trace.total.avg, &session.settings),
                                        ms(trace.total.avg),
                                    );
                                    ui.monospace(format!("{:.1}%", trace.total.loss_pct));
                                    ui.monospace(ms(trace.total.jitter));
                                    ui.monospace(
                                        logic::mos(trace.total)
                                            .map_or_else(|| "–".into(), |v| format!("{v:.2}")),
                                    );
                                    spark(ui, &trace.spark);
                                    ui.end_row();
                                }
                            });
                    });
                    if snapshot.traces.is_empty() {
                        ui.label("Enter a host above to start, or import a saved samples CSV.");
                    }
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("Time span");
                    egui::ComboBox::from_id_salt("span")
                        .selected_text(span_label(session.span))
                        .show_ui(ui, |ui| {
                            for span in [
                                30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 21600.0, 86400.0,
                                604800.0, 2592000.0, 0.0,
                            ] {
                                changed |= ui
                                    .selectable_value(&mut session.span, span, span_label(span))
                                    .changed();
                            }
                        })
                        .response
                        .on_hover_text("Time span");
                    let typed = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.span_text)
                                .hint_text("e.g. 1.5h")
                                .desired_width(70.0),
                        )
                        .on_hover_text("Custom time span");
                    if typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if let Some(span) = logic::parse_span(&self.span_text) {
                            session.span = span;
                            changed = true;
                        } else {
                            self.error =
                                Some("Enter a time span such as 30s, 5m, 1.5h, 2d or all.".into());
                        }
                    }
                    let available = snapshot
                        .traces
                        .iter()
                        .map(|t| t.to - t.from)
                        .reduce(f64::max)
                        .unwrap_or(600.0);
                    if ui.button("Zoom in").clicked() {
                        session.span = graphs::zoom(session.span, false, available);
                        changed = true;
                    }
                    if ui.button("Zoom out").clicked() {
                        session.span = graphs::zoom(session.span, true, available);
                        changed = true;
                    }
                    let (first, last) = graphs::navigation(&snapshot);
                    if ui
                        .add_enabled(session.span > 0.0, egui::Button::new("Earlier"))
                        .clicked()
                    {
                        self.end = graphs::pan(
                            self.end.unwrap_or(last),
                            -session.span / 2.0,
                            session.span,
                            first,
                            last,
                        );
                    }
                    if ui
                        .add_enabled(
                            session.span > 0.0 && self.end.is_some(),
                            egui::Button::new("Later"),
                        )
                        .clicked()
                    {
                        self.end = graphs::pan(
                            self.end.unwrap_or(last),
                            session.span / 2.0,
                            session.span,
                            first,
                            last,
                        );
                    }
                    if ui.button("Live").clicked() {
                        self.end = None;
                        self.pinned = None;
                    }
                    if self.pinned.is_some() && ui.button("Unpin").clicked() {
                        self.pinned = None;
                    }
                });
                let relevant = snapshot
                    .traces
                    .iter()
                    .filter(|t| {
                        t.config.session_id == session.active
                            || (session.active == "summary"
                                && session.compare.contains(&t.config.session_id))
                    })
                    .collect::<Vec<_>>();
                let first = relevant
                    .iter()
                    .map(|t| t.from)
                    .reduce(f64::min)
                    .unwrap_or(0.0);
                let last = relevant
                    .iter()
                    .map(|t| t.to)
                    .reduce(f64::max)
                    .unwrap_or(0.0);
                if last > first {
                    ui.horizontal(|ui| {
                        ui.small(time::date_time(first));
                        let mut end = self.end.unwrap_or(last);
                        if ui
                            .add_enabled(
                                session.span > 0.0,
                                egui::Slider::new(
                                    &mut end,
                                    (first + session.span).min(last)..=last,
                                )
                                .show_value(false)
                                .text("History position"),
                            )
                            .changed()
                        {
                            self.end = (end < last).then_some(end);
                        }
                        ui.small(time::date_time(last));
                    });
                }
                if self.selected.len() > 1 && session.active != "summary" {
                    ui.horizontal_wrapped(|ui| {
                        for ttl in self.selected.clone() {
                            if ui
                                .small_button(if ttl == 0 {
                                    "Remove round trip".into()
                                } else {
                                    format!("Remove hop {ttl}")
                                })
                                .clicked()
                            {
                                self.selected.retain(|h| *h != ttl);
                            }
                        }
                    });
                }
                let old_span = session.span;
                let graph_budget = ui.available_height().max(210.0);
                self.plot_width = Some(
                    egui::ScrollArea::vertical()
                        .id_salt("graphs-scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_max_height(graph_budget);
                            if snapshot.session.active != session.active
                                || snapshot.session.span != session.span
                            {
                                ui.label("Updating view…");
                                return (ui.available_width() - 120.0).max(1.0);
                            }
                            graphs::show(
                                ui,
                                &snapshot,
                                &mut self.end,
                                &mut self.pinned,
                                &mut session.span,
                                session.active == "summary",
                            )
                        })
                        .inner,
                );
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
                    let mut open = true;
                    egui::Window::new("Route and alert events")
                        .open(&mut open)
                        .default_size([720.0, 400.0])
                        .show(ui.ctx(), |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for event in logic::group_events(&trace.events).iter().rev() {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.weak(time::date_time(
                                            event["t"].as_f64().unwrap_or(0.0),
                                        ));
                                        ui.label(event.to_string());
                                    });
                                }
                            });
                        });
                    self.events_open = open;
                }
            }
        });
        changed |= self.dialogs(ui.ctx(), &mut session);
        if ui.input(|i| i.viewport().close_requested()) {
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
        None => Color32::GRAY,
        Some(v) if v >= settings.bad_ms as f64 => Color32::from_rgb(245, 124, 137),
        Some(v) if v >= settings.warn_ms as f64 => Color32::from_rgb(237, 191, 91),
        _ => Color32::from_rgb(120, 210, 180),
    }
}
fn cell(id: &str, hop: Option<&app::Row>, s: logic::Stats) -> String {
    match id {
        "ip" => hop.map_or_else(|| "Round trip".into(), |h| h.addr.clone()),
        "name" => hop.map_or_else(String::new, |h| {
            if h.name.is_empty() {
                h.addr.clone()
            } else {
                h.name.clone()
            }
        }),
        "loss" => format!("{:.1}", s.loss_pct),
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
                .line_segment([a, b], egui::Stroke::new(1.5, Color32::LIGHT_BLUE));
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
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 17.0), egui::Sense::click());
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
    if let (Some(min), Some(maximum), Some(avg)) = (stats.min, stats.max, stats.avg) {
        let x = |v: f64| rect.left() + (v / max * *zoom).clamp(0.0, 1.0) as f32 * rect.width();
        ui.painter().line_segment(
            [
                egui::pos2(x(min), rect.center().y),
                egui::pos2(x(maximum), rect.center().y),
            ],
            egui::Stroke::new(3.0, Color32::from_rgb(83, 150, 194)),
        );
        ui.painter().circle_filled(
            egui::pos2(x(avg), rect.center().y),
            3.5,
            latency_color(Some(avg), settings),
        );
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
        #[cfg(not(windows))]
        let paths = vec![
            (
                "system",
                PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
            ),
            (
                "cjk",
                PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
            ),
        ];
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
fn apply_theme(ctx: &egui::Context) {
    ctx.set_fonts(fonts().clone());
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(15, 19, 25);
    visuals.window_fill = Color32::from_rgb(22, 28, 36);
    visuals.extreme_bg_color = Color32::from_rgb(11, 15, 21);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.interact_size.y = 24.0;
    });
}

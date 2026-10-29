//! Window geometry follows the captured legacy interface; the controller owns its data.
use super::*;

impl Window {
    pub(super) fn sync_toolbar(&mut self, config: &Config) {
        self.host = config.host.clone();
        self.toolbar_protocol = config.protocol.clone();
        self.toolbar_port = config.port;
        self.toolbar_interval = config.interval_ms;
    }

    pub(super) fn tabs(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        session: &mut Session,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            let strip_width = (ui.available_width() - 104.0).max(100.0);
            egui::ScrollArea::horizontal()
                .id_salt("tabs")
                .max_width(strip_width)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if ui
                            .add_sized(
                                [108.0, 35.0],
                                egui::Button::selectable(
                                    session.active == "summary",
                                    "All targets",
                                ),
                            )
                            .clicked()
                        {
                            self.select(session, "summary".into());
                            changed = true;
                        }
                        for trace in &snapshot.traces {
                            let selected = session.active == trace.config.session_id;
                            let frame = egui::Frame::new()
                                .fill(if selected {
                                    ui.visuals().selection.bg_fill
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .inner_margin(egui::Margin::symmetric(8, 0));
                            let response = frame
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(9.0, 9.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().circle_filled(
                                            rect.center(),
                                            4.5,
                                            if trace.alerting {
                                                Color32::LIGHT_RED
                                            } else if trace.paused {
                                                Color32::YELLOW
                                            } else {
                                                Color32::LIGHT_GREEN
                                            },
                                        );
                                        let name = format!(
                                            "{}  {}",
                                            trace.config.host,
                                            short_protocol(&trace.config)
                                        );
                                        let tab = ui.add_sized(
                                            [0.0, 35.0],
                                            egui::Button::new(RichText::new(name).strong())
                                                .frame(false),
                                        );
                                        if tab.clicked() {
                                            self.select(session, trace.config.session_id.clone());
                                            changed = true;
                                        }
                                        if icon_button(
                                            ui,
                                            "×",
                                            &format!("Close {}", trace.config.host),
                                            24.0,
                                        )
                                        .clicked()
                                        {
                                            self.erase(trace, false);
                                        }
                                    });
                                })
                                .response;
                            response.container_context_menu(|ui| self.target_menu(ui, trace));
                        }
                        if icon_button(ui, "+", "New target", 35.0).clicked() {
                            self.select(session, "summary".into());
                            self.focus_host = true;
                            changed = true;
                        }
                    });
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized(
                        [96.0, 35.0],
                        egui::Button::selectable(session.active == "settings", "Settings"),
                    )
                    .clicked()
                {
                    self.select(session, "settings".into());
                    changed = true;
                }
            });
        });
        changed
    }

    fn toolbar_trace<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a TraceView> {
        snapshot.traces.iter().find(|t| {
            t.config.host == self.host.trim()
                && t.config.protocol == self.toolbar_protocol
                && t.config.port == self.toolbar_port
        })
    }

    pub(super) fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        session: &mut Session,
    ) -> bool {
        let mut changed = false;
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                let width = ui.available_width();
                let compact = width < 900.0;
                let inline_legend = width >= 1300.0;
                ui.spacing_mut().item_spacing = egui::vec2(12.0, 8.0);
                let host_width = if compact {
                    (width - 284.0).max(160.0)
                } else if inline_legend {
                    (width - 920.0).max(160.0)
                } else {
                    (width - 730.0).max(160.0)
                } + if self.toolbar_trace(snapshot).is_none() {
                    if inline_legend { 219.0 } else { 42.0 }
                } else {
                    0.0
                };
                ui.horizontal(|ui| {
                    changed |= self.target_controls(ui, snapshot, session, host_width);
                    if !compact {
                        self.interval_picker(ui, snapshot);
                        changed |= self.span_picker(ui, session);
                        if inline_legend {
                            self.latency_legend(ui, snapshot, session);
                        }
                    }
                });
                if compact {
                    ui.horizontal(|ui| {
                        self.interval_picker(ui, snapshot);
                        changed |= self.span_picker(ui, session);
                    });
                } else if !inline_legend
                    && self.toolbar_trace(snapshot).is_some()
                    && !session.hidden_columns.iter().any(|c| c == "bar")
                {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.latency_legend(ui, snapshot, session);
                    });
                }
            });
        changed
    }

    fn latency_legend(&self, ui: &mut egui::Ui, snapshot: &Snapshot, session: &Session) {
        if self.toolbar_trace(snapshot).is_some()
            && !session.hidden_columns.iter().any(|c| c == "bar")
        {
            ui.add_sized(
                [165.0, 20.0],
                egui::Label::new(
                    RichText::new(format!(
                        "{}ms   {}ms",
                        session.settings.warn_ms, session.settings.bad_ms
                    ))
                    .small(),
                ),
            );
        }
    }

    fn target_controls(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        session: &mut Session,
        host_width: f32,
    ) -> bool {
        let mut changed = false;
        let input = ui.add_sized(
            [host_width, 32.0],
            egui::TextEdit::singleline(&mut self.host).hint_text("Host or IP, e.g. 1.1.1.1"),
        );
        input.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Host or IP"));
        if self.focus_host {
            input.request_focus();
            self.focus_host = false;
        }
        let old_protocol = self.toolbar_protocol.clone();
        ui.scope(|ui| {
            ui.spacing_mut().combo_width = 184.0;
            protocol(ui, &mut self.toolbar_protocol);
        });
        if self.toolbar_protocol != old_protocol {
            self.toolbar_port = match self.toolbar_protocol.as_str() {
                "udp" => 33434,
                "tcp" => 443,
                _ => 0,
            };
        }
        if matches!(self.toolbar_protocol.as_str(), "udp" | "tcp") {
            ui.add(egui::DragValue::new(&mut self.toolbar_port).range(1..=65535))
                .on_hover_text("Port");
        }
        let current = snapshot.traces.iter().find(|t| {
            t.config.host == self.host.trim()
                && t.config.protocol == self.toolbar_protocol
                && t.config.port == self.toolbar_port
        });
        let running = current.is_some_and(|t| !t.paused);
        let start = icon_button(
            ui,
            if running { "⏸" } else { "▶" },
            if running {
                "Pause"
            } else if current.is_some() {
                "Resume"
            } else {
                "Start trace"
            },
            34.0,
        );
        if start.clicked() || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            if let Some(trace) = current {
                self.runtime
                    .command(Command::Pause(trace.config.session_id.clone()));
            } else if !self.host.trim().is_empty() {
                let mut config = Config::new(self.host.clone(), &session.settings);
                config.protocol = self.toolbar_protocol.clone();
                config.port = self.toolbar_port;
                config.interval_ms = self.toolbar_interval;
                self.select(session, config.session_id.clone());
                self.runtime.command(Command::Open(config));
                changed = true;
            }
        }
        if let Some(trace) = current
            && icon_button(ui, "↻", "Restart trace and erase its history", 30.0).clicked()
        {
            self.erase(trace, true);
        }

        changed
    }

    fn interval_picker(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let current = self.toolbar_trace(snapshot);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let label = ui.label("Interval");
            let before = self.toolbar_interval;
            egui::ComboBox::from_id_salt("toolbar-interval")
                .width(104.0)
                .selected_text(interval_label(self.toolbar_interval))
                .show_ui(ui, |ui| {
                    for ms in [500, 1000, 2500, 5000, 10000] {
                        ui.selectable_value(&mut self.toolbar_interval, ms, interval_label(ms));
                    }
                    ui.separator();
                    let typed = ui.add(
                        egui::TextEdit::singleline(&mut self.interval_text).hint_text("e.g. 750ms"),
                    );
                    if typed.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Some(ms) = logic::parse_interval(&self.interval_text)
                    {
                        self.toolbar_interval = ms;
                        ui.close();
                    }
                })
                .response
                .labelled_by(label.id);
            if before != self.toolbar_interval
                && let Some(trace) = current
            {
                let mut config = trace.config.clone();
                config.interval_ms = self.toolbar_interval;
                self.runtime.command(Command::Edit(config));
            }
        });
    }

    fn span_picker(&mut self, ui: &mut egui::Ui, session: &mut Session) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let label = ui.label("Time span");
            egui::ComboBox::from_id_salt("span")
                .width(144.0)
                .selected_text(span_label(session.span))
                .show_ui(ui, |ui| {
                    for span in [
                        30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 21600.0, 86400.0, 604800.0,
                        2592000.0, 0.0,
                    ] {
                        changed |= ui
                            .selectable_value(&mut session.span, span, span_label(span))
                            .changed();
                    }
                    ui.separator();
                    let typed = ui.add(
                        egui::TextEdit::singleline(&mut self.span_text).hint_text("e.g. 1.5h"),
                    );
                    if typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if let Some(span) = logic::parse_span(&self.span_text) {
                            session.span = span;
                            changed = true;
                            ui.close();
                        } else {
                            self.error =
                                Some("Enter a time span such as 30s, 5m, 1.5h, 2d or all.".into());
                        }
                    }
                })
                .response
                .labelled_by(label.id);
        });
        changed
    }

    pub(super) fn summary(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        session: &mut Session,
    ) -> bool {
        let mut changed = false;
        egui::ScrollArea::both()
            .max_height((ui.available_height() - 260.0).max(64.0))
            .show(ui, |ui| {
                egui::Grid::new("summary")
                    .spacing([16.0, 10.0])
                    .min_row_height(32.0)
                    .show(ui, |ui| {
                        for label in [
                            "Compare",
                            "",
                            "Target",
                            "IP",
                            "Probe",
                            "Hops",
                            "Loss",
                            "Sent",
                            "Last",
                            "Avg",
                            "Min",
                            "Max",
                            "Jitter",
                            "Round-trip trend",
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
                            if icon_button(
                                ui,
                                if trace.paused { "▶" } else { "⏸" },
                                &format!(
                                    "{} {}",
                                    if trace.paused { "Resume" } else { "Pause" },
                                    trace.config.host
                                ),
                                26.0,
                            )
                            .clicked()
                            {
                                self.runtime.command(Command::Pause(id.clone()));
                            }
                            let target = ui.link(&trace.config.host);
                            if target.clicked() {
                                self.select(session, id.clone());
                                changed = true;
                            }
                            target.context_menu(|ui| self.target_menu(ui, trace));
                            ui.label(&trace.addr);
                            ui.label(short_protocol(&trace.config));
                            ui.label(trace.rows.len().to_string());
                            ui.label(format!("{:.1}%", trace.total.loss_pct));
                            ui.label(trace.total.sent.to_string());
                            for value in [
                                trace.total.last,
                                trace.total.avg,
                                trace.total.min,
                                trace.total.max,
                                trace.total.jitter,
                            ] {
                                ui.label(value.map_or_else(|| "–".into(), |v| format!("{v:.1}")));
                            }
                            spark(ui, &trace.spark);
                            ui.end_row();
                        }
                    });
            });
        changed
    }

    pub(super) fn period_bar(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        active: Option<&TraceView>,
    ) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(16, 4))
            .show(ui, |ui| {
                ui.spacing_mut().interact_size.y = 24.0;
                ui.horizontal(|ui| {
                    ui.small(format!(
                        "{} – {}",
                        time::date_time(snapshot.plot_range[0]),
                        time::date_time(snapshot.plot_range[1])
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(trace) = active {
                            if ui
                                .small_button(format!("Events ({})", trace.events.len()))
                                .clicked()
                            {
                                self.events_open = true;
                            }
                            if ui
                                .small_button(
                                    if trace.config.alert_latency_ms == 0
                                        && trace.config.alert_loss_pct == 0
                                    {
                                        "Alerts off"
                                    } else {
                                        "Alerts on"
                                    },
                                )
                                .clicked()
                            {
                                self.editing = Some((trace.config.clone(), false));
                            }
                        }
                        if self.end.is_some() && ui.small_button("Live").clicked() {
                            self.end = None;
                            self.pinned = None;
                        }
                        if self.pinned.is_some() && ui.small_button("Unpin").clicked() {
                            self.pinned = None;
                        }
                    });
                });
            });
    }

    pub(super) fn history_slider(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        session: &Session,
    ) {
        let (first, last) = graphs::navigation(snapshot);
        if last <= first {
            return;
        }
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(16, 5))
            .show(ui, |ui| {
                ui.spacing_mut().interact_size.y = 20.0;
                let mut end = self.end.unwrap_or(last);
                ui.spacing_mut().slider_width = ui.available_width();
                let response = ui
                    .add_enabled(
                        session.span > 0.0,
                        egui::Slider::new(&mut end, (first + session.span).min(last)..=last)
                            .show_value(false),
                    )
                    .on_hover_text("History position");
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Slider, true, "History position")
                });
                if response.changed() {
                    self.end = (end < last).then_some(end);
                }
            });
    }

    pub(super) fn settings(&mut self, ui: &mut egui::Ui, session: &mut Session) -> bool {
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .show(ui, |ui| {
                egui::Frame::new().inner_margin(16).show(ui, |ui| {
                    let count = if ui.available_width() >= 1250.0 {
                        3
                    } else if ui.available_width() >= 900.0 {
                        2
                    } else {
                        1
                    };
                    let sections: &[&[usize]] = match count {
                        3 => &[&[0], &[1, 2, 3], &[4, 5, 6]],
                        2 => &[&[0, 1, 2], &[3, 4, 5, 6]],
                        _ => &[&[0, 1, 2, 3, 4, 5, 6]],
                    };
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.columns(count, |columns| {
                        for (column, sections) in columns.iter_mut().zip(sections) {
                            column.spacing_mut().item_spacing.y = 16.0;
                            for &section in *sections {
                                changed |= self.settings_card(column, session, section);
                            }
                        }
                    });
                });
            });
        changed
    }

    fn settings_card(&mut self, ui: &mut egui::Ui, session: &mut Session, section: usize) -> bool {
        let mut changed = false;
        egui::Frame::group(ui.style()).inner_margin(16).corner_radius(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            let s = &mut session.settings;
            match section {
                0 => {
                    ui.strong("New targets");
                    ui.weak("What a new tab starts with. Open tabs keep their own: change those in the toolbar and under Trace.");
                    ui.separator();
                    setting(ui,"Packet type","",|ui|protocol(ui,&mut s.protocol),&mut changed);
                    setting(ui,"Interval","",|ui| {
                        let mut edit = false;
                        egui::ComboBox::from_id_salt("default-interval").width(140.0).selected_text(interval_label(s.interval_ms)).show_ui(ui, |ui| {
                            for ms in [500,1000,2500,5000,10000] { edit |= ui.selectable_value(&mut s.interval_ms,ms,interval_label(ms)).changed(); }
                        }).response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Default interval"));
                        edit
                    },&mut changed);
                    changed |= number_setting(ui,"Wait for replies","At least the interval",&mut s.timeout_ms,100..=10000," ms");
                    changed |= number_setting(ui,"Packet size","The whole IP packet; TCP ignores it",&mut s.packet_size,48..=1024," bytes");
                    ui.add_space(8.0);
                    ui.weak("ALERTS");
                    ui.weak("Raised when the round trip's latest probes go over a limit. 0 switches a limit off.");
                    changed |= number_setting(ui,"Average latency over","",&mut s.alert_latency_ms,0..=60000," ms");
                    changed |= number_setting(ui,"Packet loss of at least","",&mut s.alert_loss_pct,0..=100," %");
                    changed |= number_setting(ui,"Over the last","An alert waits until it has this much",&mut s.alert_seconds,1..=3600," s");
                }
                1 => {
                    ui.strong("Display");
                    ui.weak("LATENCY COLOURS");
                    ui.weak("Where the table, the bars and the graphs turn from green to yellow and from yellow to red.");
                    changed |= number_setting(ui,"Yellow from","",&mut s.warn_ms,1..=10000," ms");
                    changed |= number_setting(ui,"Red from","",&mut s.bad_ms,2..=60000," ms");
                    s.bad_ms=s.bad_ms.max(s.warn_ms+1);
                    ui.add_space(8.0);
                    ui.weak("COMPARE GRAPH");
                    changed |= ui.checkbox(&mut s.loss_by_target,"Loss bars in each target's colour").changed();
                    ui.weak("Otherwise red, as on a hop's graph");
                    ui.add_space(8.0);
                    ui.weak("HOP TABLE COLUMNS");
                    egui::Grid::new("settings-columns").num_columns(3).show(ui, |ui| {
                        for (index,(id,label)) in COLUMNS.iter().enumerate() {
                            let mut visible=!session.hidden_columns.iter().any(|c|c==id);
                            if ui.checkbox(&mut visible,*label).changed() {
                                session.hidden_columns.retain(|c|c!=id);
                                if !visible {session.hidden_columns.push((*id).into());}
                                changed=true;
                            }
                            if index%3==2 {ui.end_row();}
                        }
                    });
                }
                2 => {
                    ui.strong("Notifications");
                    changed |= ui.checkbox(&mut s.notifications,"Desktop notifications").changed();
                    ui.weak("For alerts and route changes. Both are listed under Events of their tab either way.");
                }
                3 => {
                    ui.strong("History");
                    changed |= number_setting(ui,"Keep per target","Every round, on disk; older days are deleted",&mut s.history_days,1..=365," days");
                    ui.weak("A day of history takes about 35 MB per target at an interval of one second. The newest day is held in the window; older periods are read from disk when the time span reaches them.");
                }
                4 => {
                    ui.strong("AI agents");
                    ui.weak("Lets AI agents on this computer read the traces, start new ones and pause them, through an MCP server on the loopback address. Web pages and other machines cannot reach it.");
                    changed |= ui.checkbox(&mut session.mcp,"Allow AI agents to connect").changed();
                    changed |= number_setting(ui,"Port","",&mut s.mcp_port,1024..=65535,"");
                    ui.horizontal_wrapped(|ui| {ui.label("Address");ui.weak(if session.mcp {"Enabled on loopback"} else {"Not running"});});
                }
                5 => {
                    ui.strong("Raw sockets");
                    ui.weak(if cfg!(windows) {"ICMP (raw socket), UDP and TCP need Administrator rights. ICMP (Windows API) does not."} else {"ICMP (raw socket), UDP and TCP use raw sockets, which Linux allows with CAP_NET_RAW. ICMP (ping socket) does not."});
                    if is_elevated() {ui.label("TracePlotter is allowed to open them");}
                    else if ui.add_enabled(!self.busy && can_elevate(),egui::Button::new("Allow raw sockets and restart…")).clicked() {
                        self.runtime.command(Command::Save);
                        self.save_window(ui.ctx());
                        self.job(ui.ctx(),||Job::Restart(spawn_elevated()));
                    }
                }
                _ => {
                    ui.strong("Updates");
                    changed |= ui.checkbox(&mut session.check_updates,"Check for updates at startup").changed();
                    ui.label(concat!("TracePlotter ",env!("CARGO_PKG_VERSION")));
                    if ui.add_enabled(!self.busy,egui::Button::new("Check for updates…")).clicked() {self.update_check(ui.ctx());}
                    if ui.small_button("Restore default settings").clicked() {s.clone_from(&Session::default().settings);changed=true;}
                }
            }
        });
        changed
    }
}

fn icon_button(ui: &mut egui::Ui, glyph: &str, label: &str, size: f32) -> egui::Response {
    let vector = matches!(glyph, "⏸" | "▶" | "↻");
    let response = ui.add_sized(
        [size, size],
        egui::Button::new(
            RichText::new(if vector { "" } else { glyph }).size(if glyph == "×" {
                16.0
            } else {
                14.0
            }),
        )
        .corner_radius(size as u8 / 2)
        .frame(glyph != "×"),
    );
    let center = response.rect.center();
    let color = ui.visuals().text_color();
    if glyph == "⏸" {
        for x in [-3.5, 3.5] {
            ui.painter().rect_filled(
                egui::Rect::from_center_size(center + egui::vec2(x, 0.0), egui::vec2(3.0, 11.0)),
                0.0,
                color,
            );
        }
    } else if glyph == "▶" {
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                center + egui::vec2(-4.0, -6.0),
                center + egui::vec2(6.0, 0.0),
                center + egui::vec2(-4.0, 6.0),
            ],
            color,
            egui::Stroke::NONE,
        ));
    } else if glyph == "↻" {
        let points = (0..20)
            .map(|i| {
                let angle = 0.4 + i as f32 / 19.0 * 5.1;
                center + egui::vec2(angle.cos(), angle.sin()) * 7.0
            })
            .collect();
        ui.painter()
            .add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
        ui.painter().line_segment(
            [
                center + egui::vec2(2.0, -8.0),
                center + egui::vec2(7.0, -5.0),
            ],
            egui::Stroke::new(1.5, color),
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response.on_hover_text(label)
}
fn short_protocol(config: &Config) -> String {
    match config.protocol.as_str() {
        "winicmp" => {
            if cfg!(windows) {
                "ICMP·WIN".into()
            } else {
                "ICMP·PING".into()
            }
        }
        "icmp" => "ICMP".into(),
        _ => format!("{}:{}", config.protocol.to_uppercase(), config.port),
    }
}
fn interval_label(ms: u64) -> String {
    format!(
        "{} {}",
        ms as f64 / 1000.0,
        if ms == 1000 { "second" } else { "seconds" }
    )
}
fn setting(
    ui: &mut egui::Ui,
    label: &str,
    note: &str,
    widget: impl FnOnce(&mut egui::Ui) -> bool,
    changed: &mut bool,
) {
    ui.horizontal(|ui| {
        let label_width = (ui.available_width() - 156.0).max(90.0);
        let label_response = ui
            .allocate_ui(egui::vec2(label_width, 0.0), |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let response = ui.label(label);
                    if !note.is_empty() {
                        ui.small(note);
                    }
                    response
                })
                .inner
            })
            .inner;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.scope(|ui| {
                ui.set_width(144.0);
                *changed |= widget(ui);
            })
            .response
            .labelled_by(label_response.id);
        });
    });
    ui.separator();
}
fn number_setting<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    note: &str,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    suffix: &str,
) -> bool {
    let mut changed = false;
    setting(
        ui,
        label,
        note,
        |ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            let response = ui.add(egui::DragValue::new(value).range(range).suffix(suffix));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SpinButton, true, label));
            response.changed()
        },
        &mut changed,
    );
    changed
}

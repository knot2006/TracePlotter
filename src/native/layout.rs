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
        // A hairline and some air set the tabs apart from the title bar's menus.
        let top = ui.cursor().min.y;
        ui.painter().hline(
            ui.max_rect().x_range(),
            top,
            egui::Stroke::new(1.0, palette(ui.ctx()).line),
        );
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let strip_width = (ui.available_width() - 120.0).max(100.0);
                    egui::ScrollArea::horizontal()
                        .id_salt("tabs")
                        .max_width(strip_width)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let all = fixed_tab(
                                    ui,
                                    "All targets",
                                    session.active == "summary",
                                    false,
                                );
                                if all.clicked() {
                                    self.select(session, "summary".into());
                                    changed = true;
                                }
                                for trace in &snapshot.traces {
                                    let selected = session.active == trace.config.session_id;
                                    // The plate is painted behind once the tab's size and hover are known.
                                    let plate = ui.painter().add(egui::Shape::Noop);
                                    let response = egui::Frame::new()
                                        .inner_margin(egui::Margin {
                                            left: 10,
                                            right: 4,
                                            top: 0,
                                            bottom: 0,
                                        })
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
                                                        palette(ui.ctx()).bad
                                                    } else if trace.paused {
                                                        palette(ui.ctx()).warn
                                                    } else {
                                                        palette(ui.ctx()).good
                                                    },
                                                );
                                                let mut name = egui::text::LayoutJob::default();
                                                let host = RichText::new(format!(
                                                    "{}  ",
                                                    trace.config.host
                                                ))
                                                .color(if selected {
                                                    palette(ui.ctx()).text
                                                } else {
                                                    palette(ui.ctx()).soft
                                                });
                                                for part in [
                                                    if selected { host.strong() } else { host },
                                                    RichText::new(short_protocol(&trace.config))
                                                        .size(12.0)
                                                        .color(palette(ui.ctx()).muted),
                                                ] {
                                                    part.append_to(
                                                        &mut name,
                                                        ui.style(),
                                                        egui::FontSelection::Default,
                                                        egui::Align::Center,
                                                    );
                                                }
                                                let tab = ui.add_sized(
                                                    [0.0, TAB_HEIGHT],
                                                    egui::Button::new(name).frame(false),
                                                );
                                                if tab.clicked() {
                                                    self.select(
                                                        session,
                                                        trace.config.session_id.clone(),
                                                    );
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
                                    ui.painter().set(
                                        plate,
                                        plate_shape(
                                            ui.ctx(),
                                            response.rect,
                                            selected,
                                            response.contains_pointer(),
                                        ),
                                    );
                                    response
                                        .container_context_menu(|ui| self.target_menu(ui, trace));
                                }
                                if icon_button(ui, "+", "New target", TAB_HEIGHT).clicked() {
                                    self.select(session, "summary".into());
                                    self.focus_host = true;
                                    changed = true;
                                }
                            });
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let settings =
                            fixed_tab(ui, "Settings", session.active == "settings", true);
                        if settings.clicked() {
                            // A second click closes Settings, back to the tab it was opened from.
                            let next = if session.active == "settings" {
                                self.before_settings
                                    .take()
                                    .unwrap_or_else(|| "summary".into())
                            } else {
                                "settings".into()
                            };
                            self.select(session, next);
                            changed = true;
                        }
                    });
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
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(165.0, 22.0), egui::Sense::hover());
            let third = rect.width() / 3.0;
            for (i, (color, text)) in [
                (palette(ui.ctx()).good, String::new()),
                (
                    palette(ui.ctx()).warn,
                    format!("{}ms", session.settings.warn_ms),
                ),
                (
                    palette(ui.ctx()).bad,
                    format!("{}ms", session.settings.bad_ms),
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let chip = egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(i as f32 * third, 0.0),
                    egui::vec2(third, rect.height()),
                );
                ui.painter()
                    .rect_filled(chip, 0.0, color.gamma_multiply(0.25));
                ui.painter().text(
                    chip.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(12.0),
                    color,
                );
            }
            response.on_hover_text(format!(
                "Latency bar zones: yellow from {} ms, red from {} ms",
                session.settings.warn_ms, session.settings.bad_ms
            ));
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
            egui::TextEdit::singleline(&mut self.host)
                .hint_text("Host or IP, e.g. 1.1.1.1")
                .align(egui::Align2::LEFT_CENTER),
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
            ui.label("Interval");
            if interval_input(
                ui,
                "toolbar-interval",
                &mut self.interval_text,
                &mut self.toolbar_interval,
                130.0,
                &mut self.error,
            ) && let Some(trace) = current
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
            ui.label("Time span");
            let presets = [
                30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 21600.0, 86400.0, 604800.0, 2592000.0,
                0.0,
            ];
            let picked = match preset_input(
                ui,
                "span",
                "Time span",
                &mut self.span_text,
                span_label(session.span),
                150.0,
                "e.g. 1.5h",
                &presets.map(span_label),
            ) {
                Some(Picked::Preset(i)) => Some(presets[i]),
                Some(Picked::Typed(text)) => {
                    let span = logic::parse_span(&text);
                    if span.is_none() {
                        self.error =
                            Some("Enter a time span such as 30s, 5m, 1.5h, 2d or all.".into());
                    }
                    span
                }
                None => None,
            };
            if let Some(span) = picked.filter(|s| *s != session.span) {
                session.span = span;
                changed = true;
            }
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
        changed |= self.recent_incidents(ui, snapshot, session);
        changed
    }

    /// Alert windows that overlap across targets, newest first, within the time span.
    fn recent_incidents(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        session: &mut Session,
    ) -> bool {
        let latest = snapshot.traces.iter().map(|t| t.to).fold(0.0, f64::max);
        let from = if session.span > 0.0 {
            latest - session.span
        } else {
            0.0
        };
        let entries = snapshot
            .traces
            .iter()
            .flat_map(|trace| {
                logic::incidents(&trace.events)
                    .into_iter()
                    .map(|incident| logic::TargetIncident {
                        incident,
                        target: trace.config.session_id.clone(),
                        observed_until: trace.to,
                    })
            })
            .filter(|e| e.incident.to.unwrap_or(e.observed_until) >= from)
            .collect();
        let groups = logic::correlate_incidents(entries);
        if groups.is_empty() {
            return false;
        }
        let mut changed = false;
        ui.add_space(8.0);
        egui::CollapsingHeader::new(format!("Recent alert incidents ({})", groups.len().min(10)))
            .id_salt("recent-incidents")
            .show(ui, |ui| {
                ui.weak("Overlapping alert windows across targets. Timing alone does not establish a shared cause.");
                for group in groups.iter().rev().take(10) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} · {:.0} s observed ·",
                            time::date_time(group.from),
                            group.to - group.from
                        ));
                        let mut shown = HashSet::new();
                        for entry in &group.entries {
                            if !shown.insert(&entry.target) {
                                continue;
                            }
                            if let Some(trace) =
                                snapshot.traces.iter().find(|t| t.config.session_id == entry.target)
                                && ui.link(&trace.config.host).clicked()
                            {
                                self.select(session, entry.target.clone());
                                changed = true;
                            }
                        }
                        if group.entries.iter().any(|e| e.incident.to.is_none()) {
                            ui.weak("· recovery not recorded for every target");
                        }
                    });
                }
            });
        changed
    }

    pub(super) fn period_bar(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        active: Option<&TraceView>,
        session: &mut Session,
    ) -> bool {
        let mut changed = false;
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
                    if let Some(period) = active.and_then(|t| t.details_period) {
                        ui.small(period.map_or_else(
                            || "· Reading detailed columns…".into(),
                            |(_, to)| format!("· Detailed columns updated {}", time::date_time(to)),
                        ));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(trace) = active {
                            let count = logic::group_events(&trace.events).len();
                            ui.menu_button(RichText::new(format!("Events ({count})")).small(), |ui| {
                                menu_rows(ui);
                                if ui.button("Event list…").clicked() {
                                    self.events_open = true;
                                    ui.close();
                                }
                                changed |= ui
                                    .checkbox(&mut session.show_events, "Show events")
                                    .on_hover_text("Mark route changes, alerts and recoveries on the latency graph")
                                    .changed();
                            });
                            if small_button(
                                ui,
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
                        if self.end.is_some() && small_button(ui, "Live").clicked() {
                            self.end = None;
                            self.pinned = None;
                        }
                        if self.pinned.is_some() && small_button(ui, "Unpin").clicked() {
                            self.pinned = None;
                        }
                        if self.selected_event.is_some()
                            && small_button(ui, "Clear event").clicked()
                        {
                            self.selected_event = None;
                        }
                    });
                });
            });
        changed
    }

    /// Every grouped event of the open tab, newest first, with alert incidents above them.
    /// Clicking an event shows its moment on the graph; the list stays open for the next.
    pub(super) fn event_list(
        &mut self,
        ctx: &egui::Context,
        trace: &TraceView,
        [from, to]: [f64; 2],
        span: &mut f64,
    ) {
        let all = logic::group_events(&trace.events);
        let kinds = [
            (None, "All"),
            (Some("route"), "Route changes"),
            (Some("alert"), "Alerts"),
            (Some("recover"), "Recoveries"),
        ];
        let mut open = self.events_open;
        let mut jump = None;
        egui::Window::new(format!("Events — {}", trace.config.host))
            .id(egui::Id::unique("event-list"))
            .open(&mut open)
            .collapsible(false)
            .default_size([780.0, 480.0])
            .min_size([420.0, 240.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (kind, label) in kinds {
                        let count = all
                            .iter()
                            .filter(|e| kind.is_none_or(|k| e["kind"] == k))
                            .count();
                        if ui
                            .selectable_label(
                                self.events_filter == kind,
                                format!("{label} ({count})"),
                            )
                            .clicked()
                        {
                            self.events_filter = kind;
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.events_in_period, "Only the displayed period");
                });
                let incidents: Vec<_> = logic::incidents(&trace.events)
                    .into_iter()
                    .filter(|i| i.from <= to && i.to.unwrap_or(to) >= from)
                    .collect();
                if !incidents.is_empty() {
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new(format!(
                        "Alert incidents in the displayed period ({})",
                        incidents.len()
                    ))
                    .id_salt("event-list-incidents")
                    .default_open(true)
                    .show(ui, |ui| {
                        for incident in incidents {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(time::date_time(incident.from))
                                        .monospace()
                                        .color(palette(ui.ctx()).soft),
                                );
                                ui.label(match incident.to {
                                    Some(end) => {
                                        format!("{:.0} s until recovery", end - incident.from)
                                    }
                                    None => "no recovery recorded".into(),
                                });
                                ui.weak(incident.text);
                            });
                        }
                    });
                }
                ui.separator();
                let shown: Vec<&Value> = all
                    .iter()
                    .rev()
                    .filter(|e| self.events_filter.is_none_or(|k| e["kind"] == k))
                    .filter(|e| {
                        !self.events_in_period
                            || (from..=to).contains(&e["t"].as_f64().unwrap_or(0.0))
                    })
                    .collect();
                if shown.is_empty() {
                    ui.add_space(12.0);
                    ui.weak(if all.is_empty() {
                        "No route changes or alerts recorded yet."
                    } else {
                        "No events match this filter."
                    });
                    return;
                }
                ui.weak("Click an event to show its moment on the graph.");
                ui.add_space(2.0);
                let ctx = ui.ctx().clone();
                egui_extras::TableBuilder::new(ui)
                    .id_salt("event-list-table")
                    .striped(true)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(egui_extras::Column::exact(150.0))
                    .column(egui_extras::Column::exact(120.0))
                    .column(egui_extras::Column::remainder().clip(true))
                    .header(22.0, |mut row| {
                        for title in ["Time", "Type", "Details"] {
                            row.col(|ui| {
                                ui.label(
                                    RichText::new(title).small().color(palette(ui.ctx()).muted),
                                );
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(26.0, shown.len(), |mut row| {
                            let event = shown[row.index()];
                            let chosen = self
                                .selected_event
                                .as_ref()
                                .is_some_and(|s| graphs::same_event(s, event));
                            row.set_selected(chosen);
                            let color = graphs::event_color(&ctx, event);
                            let label = logic::event_label(event);
                            let (kind, details) = match label.split_once(": ") {
                                Some((kind, details)) => (kind.to_owned(), details.to_owned()),
                                None => (label.clone(), String::new()),
                            };
                            // Selectable text would take the click from the row.
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(time::date_time(
                                            event["t"].as_f64().unwrap_or(0.0),
                                        ))
                                        .monospace()
                                        .color(palette(ui.ctx()).soft),
                                    )
                                    .selectable(false),
                                );
                            });
                            row.col(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(14.0, 14.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 4.0, color);
                                ui.add(
                                    egui::Label::new(RichText::new(kind).color(color))
                                        .selectable(false),
                                );
                            });
                            row.col(|ui| {
                                ui.add(egui::Label::new(details).truncate().selectable(false));
                            });
                            if row.response().on_hover_text(&label).clicked() {
                                jump = Some(event.clone());
                            }
                        });
                    });
            });
        self.events_open = open;
        if let Some(event) = jump {
            self.jump_to_event(trace, span, event);
        }
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
                glass_handle(ui);
                // Green while following live data, the accent while looking back in history.
                let strength = if theme::current(ui.ctx()) == Theme::Glass {
                    1.0
                } else {
                    0.75
                };
                ui.visuals_mut().selection.bg_fill = if self.end.is_none() {
                    palette(ui.ctx()).good.gamma_multiply(strength)
                } else {
                    palette(ui.ctx()).accent.gamma_multiply(strength)
                };
                let response = ui
                    .add_enabled(
                        session.span > 0.0,
                        egui::Slider::new(&mut end, (first + session.span).min(last)..=last)
                            .show_value(false)
                            .trailing_fill(true),
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

    /// Settings as MonoCode lays them out: sections listed on the left, one page each, with
    /// grouped rows that put the name and its explanation beside the control.
    pub(super) fn settings(&mut self, ui: &mut egui::Ui, session: &mut Session) -> bool {
        let mut changed = false;
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let nav = egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .inner_margin(egui::Margin::symmetric(8, 12))
                .show(ui, |ui| {
                    ui.set_width(200.0);
                    ui.set_min_height(height - 24.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (index, (title, _)) in SETTINGS_PAGES.iter().enumerate() {
                            if nav_row(ui, title, self.settings_page == index).clicked() {
                                self.settings_page = index;
                            }
                        }
                    });
                });
            // The wheel scrolls the page from anywhere on the settings tab, the navigation
            // included, and the area fills the tab so the space below a short page reacts too.
            let mut area = egui::ScrollArea::vertical()
                .id_salt(("settings-scroll", self.settings_page))
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
            if nav.response.contains_pointer() {
                let wheel = ui.input(|i| i.smooth_scroll_delta.y);
                if wheel != 0.0 {
                    area = area.vertical_scroll_offset((self.settings_scroll - wheel).max(0.0));
                }
            }
            let shown = area.show(ui, |ui| {
                // The page is centred in the space beside the navigation. The scroll area
                // takes the row's horizontal layout, so the page sets its own.
                let width = (ui.available_width() - 64.0).clamp(240.0, 720.0);
                let side = ((ui.available_width() - width) / 2.0).max(0.0);
                ui.vertical(|ui| {
                    ui.add_space(28.0);
                    ui.horizontal(|ui| {
                        ui.add_space(side);
                        ui.vertical(|ui| {
                            ui.set_width(width);
                            let (title, description) = SETTINGS_PAGES[self.settings_page];
                            page_header(ui, title, description);
                            changed |= self.settings_body(ui, session);
                            ui.add_space(32.0);
                        });
                    });
                });
            });
            self.settings_scroll = shown.state.clamped_offset().y;
        });
        changed
    }

    fn settings_body(&mut self, ui: &mut egui::Ui, session: &mut Session) -> bool {
        let mut changed = false;
        let s = &mut session.settings;
        match self.settings_page {
            0 => {
                settings_group(ui, "Probe", "", |ui| {
                    setting(
                        ui,
                        "Packet type",
                        "",
                        |ui| protocol(ui, &mut s.protocol),
                        &mut changed,
                    );
                    setting(
                        ui,
                        "Interval",
                        "",
                        |ui| {
                            interval_input(
                                ui,
                                "default-interval",
                                &mut self.default_interval_text,
                                &mut s.interval_ms,
                                150.0,
                                &mut self.error,
                            )
                        },
                        &mut changed,
                    );
                    changed |= number_setting(
                        ui,
                        "Wait for replies",
                        "At least the interval",
                        &mut s.timeout_ms,
                        100..=10000,
                        " ms",
                    );
                    changed |= number_setting(
                        ui,
                        "Packet size",
                        "The whole IP packet; TCP ignores it",
                        &mut s.packet_size,
                        48..=1024,
                        " bytes",
                    );
                });
                settings_group(
                    ui,
                    "Alerts",
                    "Raised when the round trip's latest probes go over a limit. 0 switches a limit off.",
                    |ui| {
                        changed |= number_setting(
                            ui,
                            "Average latency over",
                            "",
                            &mut s.alert_latency_ms,
                            0..=60000,
                            " ms",
                        );
                        changed |= number_setting(
                            ui,
                            "Packet loss of at least",
                            "",
                            &mut s.alert_loss_pct,
                            0..=100,
                            " %",
                        );
                        changed |= number_setting(
                            ui,
                            "Over the last",
                            "An alert waits until it has this much",
                            &mut s.alert_seconds,
                            1..=3600,
                            " s",
                        );
                    },
                );
            }
            1 => {
                settings_group(
                    ui,
                    "",
                    "Changes apply immediately and are remembered.",
                    |ui| {
                        setting(
                            ui,
                            "Theme",
                            "Glass uses charcoal panels and green accents.",
                            |ui| {
                                let previous = session.theme;
                                egui::ComboBox::from_id_salt("native-theme-choice")
                                    .selected_text(session.theme.label())
                                    .show_ui(ui, |ui| {
                                        for theme in [Theme::Ink, Theme::Glass] {
                                            ui.selectable_value(
                                                &mut session.theme,
                                                theme,
                                                theme.label(),
                                            );
                                        }
                                    })
                                    .response
                                    .widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::ComboBox,
                                            true,
                                            "Theme",
                                        )
                                    });
                                if session.theme != previous {
                                    apply_theme(ui.ctx(), session.theme);
                                    ui.ctx().request_repaint();
                                }
                                session.theme != previous
                            },
                            &mut changed,
                        );
                        if session.theme == Theme::Glass {
                            setting(
                                ui,
                                "Transparency",
                                "How much of the desktop shows through. Windows 11 and KDE Plasma blur it.",
                                |ui| {
                                    glass_handle(ui);
                                    ui.spacing_mut().slider_width = 160.0;
                                    // The slider follows the value already applied, not the
                                    // saved session, which catches up only after a save.
                                    let mut percent = (1.0 - theme::opacity(ui.ctx())) * 100.0;
                                    // The setting's row lays out from the right: the value, then the slider.
                                    ui.label(format!("{percent:.0} %"));
                                    let response = ui.add(
                                        egui::Slider::new(&mut percent, 0.0..=70.0)
                                            .show_value(false),
                                    );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Slider,
                                            true,
                                            "Transparency",
                                        )
                                    });
                                    if response.changed() {
                                        theme::set_transparency(ui.ctx(), percent / 100.0);
                                    }
                                    // A drag is saved once, when it ends, rather than every frame.
                                    let done = response.drag_stopped()
                                        || (response.changed() && !response.dragged());
                                    if done {
                                        session.transparency = 1.0 - theme::opacity(ui.ctx());
                                    }
                                    done
                                },
                                &mut changed,
                            );
                        }
                    },
                );
                settings_group(
                    ui,
                    "Latency colours",
                    "Where the table, the bars and the graphs turn from green to yellow and from yellow to red.",
                    |ui| {
                        changed |=
                            number_setting(ui, "Yellow from", "", &mut s.warn_ms, 1..=10000, " ms");
                        changed |=
                            number_setting(ui, "Red from", "", &mut s.bad_ms, 2..=60000, " ms");
                    },
                );
                s.bad_ms = s.bad_ms.max(s.warn_ms + 1);
            }
            2 => {
                settings_group(ui, "Compare graph", "", |ui| {
                    changed |= switch_setting(
                        ui,
                        "Loss bars in each target's colour",
                        "Otherwise red, as on a hop's graph",
                        &mut s.loss_by_target,
                    );
                });
                settings_group(
                    ui,
                    "Hop table columns",
                    "Also under View > Columns and a right-click on the table's header.",
                    |ui| {
                        for (id, label) in COLUMNS {
                            let mut visible = !session.hidden_columns.iter().any(|c| c == id);
                            if switch_setting(
                                ui,
                                label,
                                column_title(id).unwrap_or(""),
                                &mut visible,
                            ) {
                                session.hidden_columns.retain(|c| c != id);
                                if !visible {
                                    session.hidden_columns.push(id.into());
                                }
                                changed = true;
                            }
                        }
                    },
                );
            }
            3 => {
                settings_group(ui, "", "", |ui| {
                    changed |= switch_setting(
                        ui,
                        "Desktop notifications",
                        "For alerts and route changes. Both are listed under Events of their tab either way.",
                        &mut s.notifications,
                    );
                });
            }
            4 => {
                settings_group(ui, "", "", |ui| {
                    changed |= number_setting(
                        ui,
                        "Keep per target",
                        "Every round, on disk; older days are deleted",
                        &mut s.history_days,
                        1..=365,
                        " days",
                    );
                });
                settings_note(
                    ui,
                    "A day of history takes about 35 MB per target at an interval of one second. The newest day is held in the window; older periods are read from disk when the time span reaches them.",
                );
            }
            5 => {
                let before = self.background;
                ui.add_enabled_ui(self.tray.is_some(), |ui| {
                    settings_group(ui, "", "", |ui| {
                        switch_setting(
                            ui,
                            "Close to tray",
                            "The window's × button keeps monitoring running",
                            &mut self.background.close_to_tray,
                        );
                        switch_setting(
                            ui,
                            "Minimize to tray",
                            "Minimizing hides the window to the tray",
                            &mut self.background.minimize_to_tray,
                        );
                    });
                });
                if (before.close_to_tray, before.minimize_to_tray)
                    != (
                        self.background.close_to_tray,
                        self.background.minimize_to_tray,
                    )
                    && let Err(e) = self.background.save()
                {
                    self.error = Some(format!("Background setting was not changed: {e}"));
                    self.background = before;
                }
                if cfg!(windows) {
                    let mut startup = self.startup;
                    ui.add_enabled_ui(tray::startup_available(), |ui| {
                        settings_group(ui, "Start at sign-in", "", |ui| {
                            if switch_setting(ui, "Start when I sign in to Windows", "Starts in the tray and restores running targets. Turn this on again if you move the portable exe. Unavailable in demo mode.", &mut startup) {
                                match tray::set_startup(startup) {
                                    Ok(()) => self.startup = tray::startup(),
                                    Err(e) => self.error = Some(format!("Background setting was not changed: {e}")),
                                }
                            }
                        });
                    });
                }
                if self.tray.is_none() {
                    settings_note(
                        ui,
                        "The system tray is unavailable on this desktop. Closing the window exits the app.",
                    );
                } else if tray::wayland() {
                    settings_note(
                        ui,
                        "On Wayland the window is minimized rather than hidden, as Wayland does not let apps hide their windows.",
                    );
                }
            }
            6 => {
                let address = self.runtime.read().mcp_url.clone();
                settings_group(ui, "", "", |ui| {
                    changed |= switch_setting(
                        ui,
                        "Allow AI agents to connect",
                        "Tools > AI agents turns this on and off as well",
                        &mut session.mcp,
                    );
                    changed |= number_setting(
                        ui,
                        "Port",
                        "On the loopback address only",
                        &mut s.mcp_port,
                        1024..=65535,
                        "",
                    );
                    setting(
                        ui,
                        "Address",
                        "",
                        |ui| {
                            ui.label(
                                RichText::new(address.as_deref().unwrap_or("Not running"))
                                    .monospace()
                                    .color(palette(ui.ctx()).soft),
                            );
                            false
                        },
                        &mut changed,
                    );
                });
            }
            7 => {
                settings_group(ui, "", "", |ui| {
                    setting(
                        ui,
                        "Raw sockets",
                        if is_elevated() {
                            "TracePlotter may open them"
                        } else {
                            "Not allowed yet"
                        },
                        |ui| {
                            if is_elevated() {
                                ui.label(RichText::new("Allowed").color(palette(ui.ctx()).good));
                            } else if ui
                                .add_enabled(
                                    !self.busy && can_elevate(),
                                    egui::Button::new("Allow and restart…"),
                                )
                                .clicked()
                            {
                                self.runtime.command(Command::Save);
                                self.save_window(ui.ctx());
                                self.job(ui.ctx(), || Job::Restart(spawn_elevated()));
                            }
                            false
                        },
                        &mut changed,
                    );
                });
            }
            _ => {
                settings_group(ui, "", "", |ui| {
                    changed |= switch_setting(
                        ui,
                        "Check for updates at startup",
                        "",
                        &mut session.check_updates,
                    );
                    setting(
                        ui,
                        concat!("TracePlotter ", env!("CARGO_PKG_VERSION")),
                        "Native egui preview",
                        |ui| {
                            if ui
                                .add_enabled(!self.busy, egui::Button::new("Check for updates…"))
                                .clicked()
                            {
                                self.update_check(ui.ctx());
                            }
                            false
                        },
                        &mut changed,
                    );
                });
                settings_group(ui, "Reset", "", |ui| {
                    setting(
                        ui,
                        "Restore default settings",
                        "Probe, display, alert and history settings; targets stay",
                        |ui| {
                            let reset = ui
                                .add(egui::Button::new(
                                    RichText::new("Restore defaults").color(palette(ui.ctx()).bad),
                                ))
                                .clicked();
                            if reset {
                                s.clone_from(&Session::default().settings);
                            }
                            reset
                        },
                        &mut changed,
                    );
                });
            }
        }
        changed
    }
}

fn icon_button(ui: &mut egui::Ui, glyph: &str, label: &str, size: f32) -> egui::Response {
    let vector = matches!(glyph, "⏸" | "▶" | "↻" | "×");
    // Pause is filled with the accent while a trace runs, start or resume with green; the
    // restart arrow is amber, as it erases history.
    let fill = match glyph {
        "⏸" => Some(palette(ui.ctx()).accent),
        "▶" => Some(palette(ui.ctx()).good),
        _ => None,
    };
    let mut button = egui::Button::new(
        RichText::new(if vector { "" } else { glyph }).size(if glyph == "×" {
            16.0
        } else {
            14.0
        }),
    )
    // New target sits among the tabs, so it takes their rounded-square plate.
    .corner_radius(if glyph == "+" { 6 } else { size as u8 / 2 })
    .frame(glyph != "×");
    if let Some(fill) = fill {
        button = button.fill(fill);
    } else if glyph == "+" {
        button = button
            .fill(plate_fill(ui.ctx(), false, false))
            .stroke(egui::Stroke::NONE);
    }
    // The frameless close cross takes the tab's height, so it shares the tab's centre.
    let height = if glyph == "×" { TAB_HEIGHT } else { size };
    let response = ui.add_sized([size, height], button);
    let center = response.rect.center();
    let color = match glyph {
        _ if fill.is_some() => palette(ui.ctx()).surface,
        "↻" => palette(ui.ctx()).warn,
        _ => ui.visuals().text_color(),
    };
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
    } else if glyph == "×" {
        let color = if response.hovered() {
            palette(ui.ctx()).text
        } else {
            palette(ui.ctx()).muted
        };
        for d in [egui::vec2(3.5, 3.5), egui::vec2(3.5, -3.5)] {
            ui.painter()
                .line_segment([center - d, center + d], egui::Stroke::new(1.5, color));
        }
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
/// The probe interval as a typed field with the usual presets, as the toolbar, the
/// defaults for new targets and the probe dialog all offer it. Returns whether it changed.
pub(super) fn interval_input(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    value: &mut u64,
    width: f32,
    error: &mut Option<String>,
) -> bool {
    let presets = [500, 1000, 2500, 5000, 10000];
    let before = *value;
    match preset_input(
        ui,
        id,
        "Interval",
        text,
        interval_label(*value),
        width,
        "e.g. 750ms",
        &presets.map(interval_label),
    ) {
        Some(Picked::Preset(i)) => *value = presets[i],
        Some(Picked::Typed(typed)) => match logic::parse_interval(&typed) {
            Some(ms) => *value = ms,
            None => *error = Some("Enter an interval such as 750ms or 2s.".into()),
        },
        None => {}
    }
    *value != before
}
enum Picked {
    Preset(usize),
    Typed(String),
}
/// A field to type in with a ▾ list of presets, as the legacy `<input list>`. It shows
/// `shown` until it is edited; typed text is returned once Enter or a click elsewhere
/// commits it, so the caller parses it.
#[allow(clippy::too_many_arguments)]
fn preset_input(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    text: &mut String,
    shown: String,
    width: f32,
    hint: &str,
    presets: &[String],
) -> Option<Picked> {
    let mut picked = None;
    let visuals = ui.visuals().widgets.inactive;
    let frame = egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(visuals.bg_stroke)
        .corner_radius(visuals.corner_radius)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let height = ui.spacing().interact_size.y;
            ui.add_space(8.0);
            let field = ui.add_sized(
                [width - 34.0, height],
                egui::TextEdit::singleline(text)
                    .id_salt(id)
                    .hint_text(hint)
                    .frame(egui::Frame::NONE)
                    .align(egui::Align2::LEFT_CENTER),
            );
            field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name));
            // Escape gives up the edit; the field shows `shown` again on the next frame.
            if field.lost_focus() && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                if *text != shown {
                    picked = Some(Picked::Typed(text.trim().to_owned()));
                }
            } else if !field.has_focus() {
                *text = shown.clone();
            }
            let (rect, arrow) =
                ui.allocate_exact_size(egui::vec2(26.0, height), egui::Sense::click());
            let c = rect.center();
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    c + egui::vec2(-4.5, -2.5),
                    c + egui::vec2(4.5, -2.5),
                    c + egui::vec2(0.0, 3.0),
                ],
                if arrow.hovered() {
                    palette(ui.ctx()).text
                } else {
                    palette(ui.ctx()).soft
                },
                egui::Stroke::NONE,
            ));
            arrow.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Presets"));
            (field, arrow)
        });
    let (field, arrow) = frame.inner;
    // As an HTML input with a datalist: the presets open on a click into the field as well
    // as on the arrow. They stay open while the pointer is over them, so a pick still lands
    // after the field has lost focus, and close on a click elsewhere, Escape or a pick.
    let state = field.id.with("presets");
    let (mut open, list): (bool, Option<egui::Rect>) =
        ui.data(|d| d.get_temp(state)).unwrap_or_default();
    if arrow.clicked() {
        open = !open;
    } else if field.clicked() || field.gained_focus() {
        open = true;
    }
    // The pointer's own position: the list is on a layer above this one, which
    // `rect_contains_pointer` would count as covering it.
    let over_list = list.is_some_and(|r| {
        ui.input(|i| i.pointer.interact_pos())
            .is_some_and(|p| r.contains(p))
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape))
        || (ui.input(|i| i.pointer.any_pressed())
            && !frame.response.contains_pointer()
            && !over_list)
    {
        open = false;
    }
    let mut keep_open = open;
    let list = egui::Popup::from_response(&frame.response)
        .id(state.with("popup"))
        .kind(egui::PopupKind::Menu)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .open_bool(&mut keep_open)
        .width(width)
        .show(|ui| {
            for (i, preset) in presets.iter().enumerate() {
                if ui.selectable_label(*preset == shown, preset).clicked() {
                    picked = Some(Picked::Preset(i));
                }
            }
        })
        .map(|r| r.response.rect);
    if let Some(Picked::Preset(i)) = picked {
        // The field then loses focus; with the pick as its text, that is not a new entry.
        text.clone_from(&presets[i]);
        open = false;
        field.surrender_focus();
    }
    ui.data_mut(|d| d.insert_temp(state, (open && keep_open, list)));
    picked
}
/// The height of a tab's plate.
const TAB_HEIGHT: f32 = 30.0;
/// MonoCode's tab plates: a faint fill on every tab, a little more on hover. The open tab
/// takes the accent, so it stands out from the rest at a glance.
fn plate_fill(ctx: &egui::Context, selected: bool, hovered: bool) -> Color32 {
    if selected {
        palette(ctx).accent.gamma_multiply(0.24)
    } else if hovered {
        palette(ctx).text.gamma_multiply(0.07)
    } else {
        palette(ctx).text.gamma_multiply(0.04)
    }
}
/// Glass draws a slider as a green fill up to a white-rimmed handle, which turns into a
/// white disc while it is hovered or dragged.
fn glass_handle(ui: &mut egui::Ui) {
    if theme::current(ui.ctx()) != Theme::Glass {
        return;
    }
    let handle = palette(ui.ctx()).text;
    ui.visuals_mut().selection.bg_fill = palette(ui.ctx()).accent;
    let widgets = &mut ui.visuals_mut().widgets;
    // The rail and the resting handle share this fill, so the rim is what marks the handle.
    widgets.inactive.bg_fill = handle.gamma_multiply(0.16);
    widgets.inactive.fg_stroke = egui::Stroke::new(2.0, handle);
    for state in [&mut widgets.hovered, &mut widgets.active] {
        state.bg_fill = handle;
        state.bg_stroke = egui::Stroke::NONE;
    }
}
/// A tab's plate. Glass rims the open one with the accent, as its chosen options are.
fn plate_shape(
    ctx: &egui::Context,
    rect: egui::Rect,
    selected: bool,
    hovered: bool,
) -> egui::Shape {
    let stroke = if selected && theme::current(ctx) == Theme::Glass {
        egui::Stroke::new(1.0, palette(ctx).accent.gamma_multiply(0.7))
    } else {
        egui::Stroke::NONE
    };
    egui::epaint::RectShape::new(
        rect,
        6.0,
        plate_fill(ctx, selected, hovered),
        stroke,
        egui::StrokeKind::Inside,
    )
    .into()
}
/// All targets or Settings: a plate with its mark, as the targets' tabs have.
fn fixed_tab(ui: &mut egui::Ui, label: &str, selected: bool, gear: bool) -> egui::Response {
    let plate = ui.painter().add(egui::Shape::Noop);
    let text = RichText::new(format!("     {label}")).color(if selected {
        palette(ui.ctx()).text
    } else {
        palette(ui.ctx()).soft
    });
    let response = ui.add_sized(
        [if gear { 104.0 } else { 120.0 }, TAB_HEIGHT],
        egui::Button::new(if selected { text.strong() } else { text }).frame(false),
    );
    ui.painter().set(
        plate,
        plate_shape(ui.ctx(), response.rect, selected, response.hovered()),
    );
    tab_icon(ui, &response, label, gear);
    response
}
/// A list or gear mark at the left of a fixed tab, painted because the bundled fonts
/// have no such glyphs.
fn tab_icon(ui: &egui::Ui, response: &egui::Response, label: &str, gear: bool) {
    // The padding that makes room for the mark is not part of the accessible name.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Tab, true, label));
    let color = ui.style().interact(response).text_color();
    let center = egui::pos2(response.rect.left() + 16.0, response.rect.center().y);
    let stroke = egui::Stroke::new(1.5, color);
    if gear {
        for i in 0..8 {
            let angle = i as f32 * std::f32::consts::FRAC_PI_4;
            let dir = egui::vec2(angle.cos(), angle.sin());
            ui.painter().line_segment(
                [center + dir * 4.0, center + dir * 6.5],
                egui::Stroke::new(2.0, color),
            );
        }
        ui.painter().circle_stroke(center, 4.0, stroke);
    } else {
        for y in [-4.0, 0.0, 4.0] {
            ui.painter()
                .hline(center.x - 5.5..=center.x + 5.5, center.y + y, stroke);
        }
    }
}
pub(super) fn short_protocol(config: &Config) -> String {
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
/// The sections of Settings, in their navigation order, with each page's description.
const SETTINGS_PAGES: [(&str, &str); 9] = [
    (
        "New targets",
        "What a new tab starts with. Open tabs keep their own: change those in the toolbar and under Trace.",
    ),
    (
        "Appearance",
        "The theme, Glass's transparency and the latency colours.",
    ),
    (
        "Display",
        "The comparison graph and the hop table's columns.",
    ),
    ("Notifications", "Desktop notifications for this computer."),
    (
        "History",
        "How long every target's rounds are kept on disk.",
    ),
    (
        "Background monitoring",
        "Monitoring and alerts continue while the window is hidden. Reopen it from the tray icon. File > Exit and stop monitoring, or the tray's exit item, ends the app.",
    ),
    (
        "AI agents",
        "Lets AI agents on this computer read the traces, start new ones and pause them, through an MCP server on the loopback address. Web pages and other machines cannot reach it.",
    ),
    (
        "Raw sockets",
        if cfg!(windows) {
            "ICMP (raw socket), UDP and TCP need Administrator rights. ICMP (Windows API) does not."
        } else {
            "ICMP (raw socket), UDP and TCP use raw sockets, which Linux allows with CAP_NET_RAW. ICMP (ping socket) does not."
        },
    ),
    ("Updates", "Releases are published on the project page."),
];

/// A section in the settings navigation: clear at rest, lit on hover, filled when chosen.
fn nav_row(ui: &mut egui::Ui, title: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(184.0, 30.0), egui::Sense::click());
    let fill = if selected {
        if theme::current(ui.ctx()) == Theme::Glass {
            ui.visuals().selection.bg_fill
        } else {
            palette(ui.ctx()).text.gamma_multiply(0.10)
        }
    } else if response.hovered() {
        palette(ui.ctx()).text.gamma_multiply(0.05)
    } else {
        Color32::TRANSPARENT
    };
    let glass = theme::current(ui.ctx()) == Theme::Glass;
    let stroke = if selected && glass {
        egui::Stroke::new(1.0, palette(ui.ctx()).accent.gamma_multiply(0.7))
    } else {
        egui::Stroke::NONE
    };
    ui.painter()
        .rect(rect, 6.0, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(14.0),
        if selected && glass {
            palette(ui.ctx()).accent_light
        } else if selected || response.hovered() {
            palette(ui.ctx()).text
        } else {
            palette(ui.ctx()).soft
        },
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, title));
    response
}

fn page_header(ui: &mut egui::Ui, title: &str, description: &str) {
    ui.label(
        RichText::new(title)
            .size(20.0)
            .color(palette(ui.ctx()).text),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(description)
            .size(13.0)
            .color(palette(ui.ctx()).muted),
    );
    ui.add_space(8.0);
}

/// A titled box of rows: a faint fill and hairline border, as MonoCode's settings groups.
fn settings_group(ui: &mut egui::Ui, title: &str, note: &str, rows: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(16.0);
    if !title.is_empty() {
        ui.label(
            RichText::new(title)
                .size(13.0)
                .color(palette(ui.ctx()).text),
        );
    }
    if !note.is_empty() {
        ui.label(
            RichText::new(note)
                .size(12.0)
                .color(palette(ui.ctx()).muted),
        );
    }
    if !title.is_empty() || !note.is_empty() {
        ui.add_space(8.0);
    }
    egui::Frame::new()
        // Glass sinks its groups a shade below the panel, as cards set into a frosted pane.
        .fill(if theme::current(ui.ctx()) == Theme::Glass {
            Color32::from_black_alpha(56)
        } else {
            palette(ui.ctx()).text.gamma_multiply(0.03)
        })
        .stroke(egui::Stroke::new(
            1.0,
            palette(ui.ctx()).text.gamma_multiply(0.10),
        ))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui);
        });
}

fn settings_note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(
        RichText::new(text)
            .size(12.0)
            .color(palette(ui.ctx()).muted),
    );
}

/// An on/off switch: the accent when on, a grey track when off.
fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(36.0, 20.0), egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let track = if ui.is_enabled() {
        palette(ui.ctx())
            .text
            .gamma_multiply(0.2)
            .lerp_to_gamma(palette(ui.ctx()).accent, t)
    } else {
        palette(ui.ctx()).text.gamma_multiply(0.1)
    };
    ui.painter().rect_filled(rect, 10.0, track);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, t);
    // Glass greys the knob while off, so on and off differ by more than the track.
    let knob = if theme::current(ui.ctx()) == Theme::Glass {
        palette(ui.ctx()).muted.lerp_to_gamma(Color32::WHITE, t)
    } else {
        palette(ui.ctx()).text
    };
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, knob);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, label)
    });
    response
}

fn switch_setting(ui: &mut egui::Ui, label: &str, note: &str, on: &mut bool) -> bool {
    let mut changed = false;
    setting(
        ui,
        label,
        note,
        |ui| switch(ui, on, label).changed(),
        &mut changed,
    );
    changed
}

/// One row of a settings group: the name and its explanation on the left, the control on
/// the right, and a hairline above every row but the first.
fn setting(
    ui: &mut egui::Ui,
    label: &str,
    note: &str,
    widget: impl FnOnce(&mut egui::Ui) -> bool,
    changed: &mut bool,
) {
    if ui.cursor().min.y > ui.max_rect().min.y + 1.0 {
        let y = ui.cursor().min.y;
        ui.painter().hline(
            ui.max_rect().x_range(),
            y,
            egui::Stroke::new(1.0, palette(ui.ctx()).text.gamma_multiply(0.05)),
        );
    }
    ui.add_space(12.0);
    let label_width = (ui.available_width() - 200.0).max(120.0);
    // The row is as tall as its text, at least a control's height, and both sides are
    // centred in it, so a label lines up with its control.
    let note_height = if note.is_empty() {
        0.0
    } else {
        2.0 + ui
            .painter()
            .layout(
                note.into(),
                egui::FontId::proportional(12.0),
                palette(ui.ctx()).muted,
                label_width,
            )
            .size()
            .y
    };
    let label_height = ui
        .painter()
        .layout(
            label.into(),
            egui::FontId::proportional(13.0),
            palette(ui.ctx()).text,
            label_width,
        )
        .size()
        .y;
    let height = (label_height + note_height).max(28.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // Given its full height, the text block centres in the row like the control.
            let label_response = ui
                .allocate_ui(egui::vec2(label_width, label_height + note_height), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let response = ui.label(
                            RichText::new(label)
                                .size(13.0)
                                .color(palette(ui.ctx()).text),
                        );
                        if !note.is_empty() {
                            ui.label(
                                RichText::new(note)
                                    .size(12.0)
                                    .color(palette(ui.ctx()).muted),
                            );
                        }
                        response
                    })
                    .inner
                })
                .inner;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                *changed |= widget(ui);
            })
            .response
            .labelled_by(label_response.id);
        },
    );
    ui.add_space(12.0);
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

//! Context actions share the window's existing commands and exports.
use super::*;

impl Window {
    pub(super) fn column_menu(
        &mut self,
        ui: &mut egui::Ui,
        session: &mut Session,
        key: Option<&str>,
    ) -> bool {
        ui.spacing_mut().interact_size.y = 22.0;
        ui.spacing_mut().item_spacing.y = 2.0;
        let mut changed = false;
        let mut order = ordered_columns(session);
        if let Some(at) = key.and_then(|key| order.iter().position(|id| id == key)) {
            for (label, next) in [
                ("Move left", at.checked_sub(1)),
                ("Move right", (at + 1 < order.len()).then_some(at + 1)),
            ] {
                if ui
                    .add_enabled(next.is_some(), egui::Button::new(label))
                    .clicked()
                {
                    if let Some(next) = next {
                        order.swap(at, next);
                        session.column_order = order.clone();
                        changed = true;
                    }
                    ui.close();
                }
            }
            ui.separator();
        }
        for (id, label) in COLUMNS {
            let mut visible = !session.hidden_columns.iter().any(|column| column == id);
            if ui.checkbox(&mut visible, label).changed() {
                session.hidden_columns.retain(|column| column != id);
                if !visible {
                    session.hidden_columns.push(id.into());
                }
                changed = true;
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Reset to defaults").clicked() {
            self.reset_columns = true;
            session.column_order.clear();
            session.column_widths.clear();
            session.hidden_columns = vec!["mos".into()];
            changed = true;
            ui.close();
        }
        changed
    }

    pub(super) fn hop_menu(
        &mut self,
        ui: &mut egui::Ui,
        trace: &TraceView,
        hop: Option<&app::Row>,
        snapshot: &Snapshot,
    ) {
        menu_rows(ui);
        let addr = hop.map_or(trace.addr.as_str(), |hop| hop.addr.as_str());
        let name = hop.map_or(trace.config.host.as_str(), |hop| hop.name.as_str());
        if let Ok(ip) = addr.parse::<IpAddr>() {
            if ui.button(format!("Copy IP {addr}")).clicked() {
                ui.ctx().copy_text(addr.to_owned());
                ui.close();
            }
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("Copy name"))
                .clicked()
            {
                ui.ctx().copy_text(name.to_owned());
                ui.close();
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Whois lookup…"))
                .clicked()
            {
                self.job(ui.ctx(), move || Job::Whois(ip, whois(ip)));
                ui.close();
            }
            ui.separator();
        }
        if ui
            .add_enabled(!self.busy, egui::Button::new("Save MTR report…"))
            .clicked()
        {
            self.export(ui, snapshot, export::Kind::Text);
            ui.close();
        }
        if ui.button("Copy MTR report").clicked() {
            ui.ctx().copy_text(export::report(snapshot, false));
            ui.close();
        }
    }

    pub(super) fn target_menu(&mut self, ui: &mut egui::Ui, trace: &TraceView) {
        menu_rows(ui);
        if ui
            .button(if trace.paused { "Resume" } else { "Pause" })
            .clicked()
        {
            self.runtime
                .command(Command::Pause(trace.config.session_id.clone()));
            ui.close();
        }
        if ui.button("Edit probe and alerts…").clicked() {
            self.editing = Some((trace.config.clone(), false));
            ui.close();
        }
        if ui.button("Restart and erase history…").clicked() {
            self.erase(trace, true);
            ui.close();
        }
        if ui.button("Close and erase history…").clicked() {
            self.erase(trace, false);
            ui.close();
        }
    }
}

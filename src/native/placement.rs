//! Restore the Windows window while eframe still keeps it hidden.
use super::{WindowState, egui};
use egui_winit::winit::dpi::{PhysicalPosition, PhysicalSize};

pub(super) fn restore(cc: &eframe::CreationContext<'_>, state: Option<WindowState>) -> bool {
    let Some(window) = cc.winit_window() else {
        return false;
    };
    let Some(state) = state else {
        return true;
    };
    let monitors = window.available_monitors().collect::<Vec<_>>();
    let bounds = |monitor: &egui_winit::winit::monitor::MonitorHandle| {
        let pos = monitor.position();
        let size = monitor.size();
        egui::Rect::from_min_size(
            egui::pos2(pos.x as f32, pos.y as f32),
            egui::vec2(size.width as f32, size.height as f32),
        )
    };
    let saved_pos = egui::pos2(state.x as f32, state.y as f32);
    let saved_rect = egui::Rect::from_min_size(
        saved_pos,
        egui::vec2(state.width as f32, state.height as f32),
    );
    let monitor = monitors
        .iter()
        .find(|m| bounds(m).contains(saved_pos))
        .or_else(|| {
            monitors
                .iter()
                .filter(|m| bounds(m).intersect(saved_rect).area() > 0.0)
                .max_by(|a, b| {
                    bounds(a)
                        .intersect(saved_rect)
                        .area()
                        .total_cmp(&bounds(b).intersect(saved_rect).area())
                })
        })
        .cloned()
        .or_else(|| window.primary_monitor())
        .or_else(|| monitors.first().cloned());
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor() as f32 * cc.egui_ctx.zoom_factor();
        let size = state
            .size
            .filter(|s| s.iter().all(|v| v.is_finite() && *v > 0.0))
            .map_or_else(
                || egui::vec2(state.width as f32, state.height as f32),
                |s| egui::vec2(s[0], s[1]) * scale,
            );
        let (pos, size) = fit(saved_pos, size, bounds(&monitor), scale);
        // Physical coordinates also work on monitors left of the primary display,
        // and do not depend on the DPI of the monitor used during window creation.
        window.set_outer_position(PhysicalPosition::new(pos.x as i32, pos.y as i32));
        let _ = window.request_inner_size(PhysicalSize::new(size.x as u32, size.y as u32));
    }
    // This helper cloaks winit's brief show/hide when maximizing a hidden window.
    egui_winit::set_maximized(window, state.maximized);
    true
}

fn fit(
    pos: egui::Pos2,
    size: egui::Vec2,
    monitor: egui::Rect,
    scale: f32,
) -> (egui::Pos2, egui::Vec2) {
    let size = size
        .max(egui::vec2(640.0, 420.0) * scale)
        .min(monitor.size());
    let pos = egui::pos2(
        pos.x.clamp(monitor.left(), monitor.right() - size.x),
        pos.y.clamp(monitor.top(), monitor.bottom() - size.y),
    );
    (pos, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_keeps_negative_monitor_coordinates_and_fits_disconnected_displays() {
        let left = egui::Rect::from_min_size(egui::pos2(-1920.0, 0.0), egui::vec2(1920.0, 1080.0));
        let size = egui::vec2(1100.0, 720.0);
        assert_eq!(
            fit(egui::pos2(-1800.0, 100.0), size, left, 1.0).0,
            egui::pos2(-1800.0, 100.0)
        );
        let primary = left.translate(egui::vec2(1920.0, 0.0));
        assert_eq!(
            fit(egui::pos2(-1800.0, 100.0), size, primary, 1.0).0,
            egui::pos2(0.0, 100.0)
        );
        assert_eq!(
            fit(egui::pos2(1900.0, 1000.0), size * 2.0, primary, 2.0),
            (primary.min, primary.size())
        );
    }
}

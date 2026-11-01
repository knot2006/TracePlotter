//! A short entrance, driven by UI time rather than a delay in startup.
use super::{egui, palette};
use std::time::Duration;

#[derive(Default)]
pub(super) struct Opening {
    started: Option<f64>,
    finished: bool,
}

impl Opening {
    pub fn finish(&mut self) {
        self.finished = true;
    }

    pub fn begin(&mut self, ui: &mut egui::Ui) -> Option<f32> {
        // Keep the usual egui animation switch effective here too. An interaction
        // completes the entrance immediately so controls never feel held back.
        if ui.style().animation_time <= 0.0
            || ui.input(|i| {
                i.pointer.any_pressed()
                    || i.events
                        .iter()
                        .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            })
        {
            self.finish();
        }
        if self.finished {
            return None;
        }
        let now = ui.input(|i| i.time);
        let elapsed = (now - *self.started.get_or_insert(now)).max(0.0) as f32;
        if elapsed >= 0.55 {
            self.finish();
            return None;
        }
        let fade = (elapsed / 0.28).clamp(0.0, 1.0);
        ui.multiply_opacity(1.0 - (1.0 - fade).powi(3));
        ui.ctx().request_repaint_after(Duration::from_millis(16));
        Some(elapsed / 0.55)
    }
}

pub(super) fn sweep(ctx: &egui::Context, progress: f32) {
    let rect = ctx.content_rect();
    let width = rect.width() * 0.18;
    let head = rect.left() + (rect.width() + width) * progress;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("opening-sweep"),
    ));
    // A fading tail suggests a packet crossing the window, in the current accent.
    for segment in 0..24 {
        let tail = segment as f32 / 24.0;
        let x = head - width * (1.0 - tail);
        let end = (x + width / 24.0).min(rect.right());
        let start = x.max(rect.left());
        if start < end {
            painter.line_segment(
                [
                    egui::pos2(start, rect.top() + 1.0),
                    egui::pos2(end, rect.top() + 1.0),
                ],
                egui::Stroke::new(
                    2.0,
                    palette(ctx).accent.gamma_multiply(tail * (1.0 - progress)),
                ),
            );
        }
    }
}

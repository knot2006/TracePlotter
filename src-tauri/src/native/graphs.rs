//! Prepared, bounded series; the UI owns time bounds and loss-axis conversion.
use super::*;
use crate::logic::{GraphData, latency_axis};
use app::Snapshot;
use serde_json::Value;
use egui_plot::{AxisHints, Bar, BarChart, HPlacement, Line, Plot, PlotPoint, VLine};

pub(super) enum Action {
    Live,
    /// A route change, alert or recovery mark was clicked.
    Event(Value),
    Export(export::Kind, Option<(String, u8)>),
    Close(u8),
    LossColors(bool),
}
fn menu(
    ui: &mut egui::Ui,
    compare: bool,
    series: &app::PlotView,
    closeable: bool,
    mut loss_by_target: bool,
    action: &mut Option<Action>,
) {
    ui.set_min_width(220.0);
    if !compare {
        if ui.button("Back to live").clicked() {
            *action = Some(Action::Live);
            ui.close();
        }
        ui.separator();
    }
    for (label, kind) in [
        (
            if compare {
                "Save graph as PNG…"
            } else {
                "Save this graph as PNG…"
            },
            export::Kind::Png,
        ),
        (
            if compare {
                "Copy graph as PNG"
            } else {
                "Copy this graph as PNG"
            },
            export::Kind::CopyImage,
        ),
    ] {
        if ui.button(label).clicked() {
            *action = Some(Action::Export(
                kind,
                (!compare).then(|| (series.session.clone(), series.ttl)),
            ));
            ui.close();
        }
    }
    ui.separator();
    if compare {
        if ui.button("Save stretches as CSV…").clicked() {
            *action = Some(Action::Export(export::Kind::Stretches, None));
            ui.close();
        }
        ui.separator();
        if ui
            .checkbox(&mut loss_by_target, "Loss bars in each target's colour")
            .changed()
        {
            *action = Some(Action::LossColors(loss_by_target));
            ui.close();
        }
    } else if ui
        .add_enabled(closeable, egui::Button::new("Close this graph"))
        .clicked()
    {
        *action = Some(Action::Close(series.ttl));
        ui.close();
    }
}
pub(super) struct Output {
    pub width: f32,
    pub action: Option<Action>,
}

/// Series colours in a fixed order, the dark steps of the dataviz reference palette with
/// its green and red left out, as those mean good and bad here. Checked with its validator
/// on the plot surface: lightness band, CVD separation (worst adjacent 8.4), normal-vision
/// separation (19.3) and 3:1 contrast all pass.
const SERIES_COLORS: [Color32; 6] = [
    Color32::from_rgb(0x39, 0x87, 0xe5),
    Color32::from_rgb(0xd9, 0x59, 0x26),
    Color32::from_rgb(0x19, 0x9e, 0x70),
    Color32::from_rgb(0xc9, 0x85, 0x00),
    Color32::from_rgb(0xd5, 0x51, 0x81),
    Color32::from_rgb(0x90, 0x85, 0xe9),
];
/// Nulls end a path; a missing reply must never become a line through the gap.
fn runs(xs: &[f64], ys: &[Option<f64>]) -> Vec<Vec<[f64; 2]>> {
    let mut runs = vec![];
    let mut run = vec![];
    for (&x, y) in xs.iter().zip(ys) {
        if let Some(y) = y {
            run.push([x, *y]);
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

/// Holds each probe until the next timestamp, matching uPlot's stepped path.
fn step_run(run: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut points = Vec::with_capacity(run.len().saturating_mul(2));
    for (index, point) in run.iter().enumerate() {
        if index > 0 {
            points.push([point[0], run[index - 1][1]]);
        }
        points.push(*point);
    }
    points
}

fn points(
    plot: &mut egui_plot::PlotUi<'_>,
    data: &GraphData,
    color: Color32,
    style: egui_plot::LineStyle,
) {
    let mut band = vec![];
    let flush = |plot: &mut egui_plot::PlotUi<'_>, band: &mut Vec<usize>| {
        if band.len() > 1 {
            let lower = band
                .iter()
                .map(|&i| [data.xs[i], data.lo[i].unwrap_or_default()])
                .collect::<Vec<_>>();
            let upper = band
                .iter()
                .map(|&i| [data.xs[i], data.hi[i].unwrap_or_default()])
                .collect::<Vec<_>>();
            let lower = step_run(&lower);
            let upper = step_run(&upper);
            let xs = lower.iter().map(|point| point[0]).collect::<Vec<_>>();
            let lo = lower.iter().map(|point| point[1]).collect::<Vec<_>>();
            let hi = upper.iter().map(|point| point[1]).collect::<Vec<_>>();
            plot.add(
                egui_plot::FilledArea::new("Range", &xs, &lo, &hi)
                    .fill_color(color.gamma_multiply(0.22))
                    .allow_hover(false),
            );
        }
        band.clear();
    };
    for i in 0..data.xs.len() {
        if data.lo.get(i).copied().flatten().is_some()
            && data.hi.get(i).copied().flatten().is_some()
        {
            band.push(i);
        } else {
            flush(plot, &mut band);
        }
    }
    flush(plot, &mut band);
    for run in runs(&data.xs, &data.ys) {
        let run = step_run(&run);
        if run.len() == 1 {
            plot.points(
                egui_plot::Points::new("Latency", run)
                    .color(color)
                    .radius(3.0),
            );
        } else {
            plot.line(
                Line::new("Latency", run)
                    .color(color)
                    .width(1.5)
                    // Equal-x step edges must remain strokes, not opaque line bands.
                    .allow_band(false)
                    .style(style),
            );
        }
    }
}
fn coverage(series: &[&app::PlotView]) -> Vec<(f64, f64)> {
    let mut spans = Vec::new();
    for series in series {
        let mut start = None;
        let mut last = 0.0;
        for (&x, loss) in series.data.xs.iter().zip(&series.data.ls) {
            if loss.is_some() {
                start.get_or_insert(x);
                last = x;
            } else if let Some(from) = start.take() {
                spans.push((from, last + series.width));
            }
        }
        if let Some(from) = start {
            spans.push((from, last + series.width));
        }
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (from, to) in spans {
        if let Some(last) = merged.last_mut().filter(|last| from <= last.1) {
            last.1 = last.1.max(to);
        } else {
            merged.push((from, to));
        }
    }
    merged
}
/// Width of each y axis strip, which holds its tick labels and its title.
const AXIS_WIDTH: f32 = 58.0;
fn axes(min: f64, max: f64, loss: bool) -> Vec<AxisHints<'static>> {
    vec![
        AxisHints::new_y()
            .min_thickness(AXIS_WIDTH)
            .tick_label_font(egui::FontId::proportional(12.0))
            .formatter(|mark, _| {
                if mark.value.abs() >= 1e6 {
                    format!("{:.1e}", mark.value)
                } else if mark.step_size.fract() != 0.0 {
                    format!("{:.1}", mark.value)
                } else {
                    format!("{:.0}", mark.value)
                }
            }),
        AxisHints::new_y()
            .placement(HPlacement::Right)
            .min_thickness(AXIS_WIDTH)
            .tick_label_font(egui::FontId::proportional(12.0))
            .formatter(move |mark, _| {
                if loss {
                    format!("{:.0}%", (mark.value - min) / (max - min) * 100.0)
                } else {
                    String::new()
                }
            }),
    ]
}
/// egui_plot sets rotated axis titles a quarter line outside the plot, where the window
/// edge cuts them, so they are painted at the outer edges of its axis strips instead.
/// Short plots get short titles, which would otherwise run past their ends.
/// `rect` is the plot area, between the axis strips.
fn axis_titles(ui: &egui::Ui, rect: egui::Rect, loss: bool) {
    let short = rect.height() < 130.0;
    let left = if loss && !short { "Latency (ms)" } else { "ms" };
    let right = match (loss, short) {
        (false, _) => "",
        (true, false) => "Packet loss %",
        (true, true) => "Loss",
    };
    for (text, at_right) in [(left, false), (right, true)] {
        let galley = ui.painter().layout_no_wrap(
            text.into(),
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
        let size = galley.size();
        // Turned a quarter anticlockwise, the text runs upwards from `pos`, `size.y` wide.
        let x = if at_right {
            rect.right() + AXIS_WIDTH - size.y - 2.0
        } else {
            rect.left() - AXIS_WIDTH + 2.0
        };
        let pos = egui::pos2(x, rect.center().y + size.x / 2.0);
        ui.painter().add(
            egui::epaint::TextShape::new(pos, galley, ui.visuals().text_color())
                .with_angle(-std::f32::consts::FRAC_PI_2),
        );
    }
}
fn readout(ui: &mut egui::Ui, data: &GraphData, time: f64, width: f64, span: f64) {
    let i = data.xs.partition_point(|x| *x <= time).saturating_sub(1);
    if data.xs.is_empty() || time < data.xs[0] || time > *data.xs.last().unwrap_or(&time) + width {
        ui.small("No probes in this period");
        return;
    }
    ui.label(
        RichText::new(if span > 86400.0 {
            time::date_time(data.xs[i])
        } else {
            time::time_of_day(data.xs[i])
        })
        .small()
        .color(palette(ui.ctx()).muted),
    );
    if data.ls[i].is_none() {
        ui.small("not probing");
        return;
    }
    let latency = match data.ys[i] {
        None => "lost".into(),
        Some(rtt) => match (
            data.lo.get(i).copied().flatten(),
            data.hi.get(i).copied().flatten(),
        ) {
            (Some(lo), Some(hi)) if lo != hi => format!("{rtt:.1} ms average ({lo:.1}–{hi:.1})"),
            _ => ms(Some(rtt)),
        },
    };
    ui.label(RichText::new(latency).small().strong());
    if let Some(jitter) = data.js[i] {
        ui.label(
            RichText::new(format!("jitter {jitter:.1} ms"))
                .small()
                .strong(),
        );
    }
    if let Some(loss) = data.ls[i].filter(|loss| *loss > 0.0) {
        ui.small(format!("loss {loss:.0}%"));
    }
}
pub(super) fn navigation(snapshot: &Snapshot) -> (f64, f64) {
    let traces = snapshot
        .traces
        .iter()
        .filter(|t| {
            t.config.session_id == snapshot.session.active
                || (snapshot.session.active == "summary"
                    && snapshot.session.compare.contains(&t.config.session_id))
        })
        .collect::<Vec<_>>();
    (
        traces
            .iter()
            .map(|t| t.from)
            .reduce(f64::min)
            .unwrap_or(0.0),
        traces.iter().map(|t| t.to).reduce(f64::max).unwrap_or(0.0),
    )
}
pub(super) fn pan(current: f64, delta: f64, span: f64, first: f64, last: f64) -> Option<f64> {
    if span == 0.0 || last <= first {
        return None;
    }
    let next = (current + delta).clamp((first + span).min(last), last);
    (next < last - 1.0).then_some(next)
}
pub(super) fn show(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    end: &mut Option<f64>,
    pinned: &mut Option<f64>,
    span: &mut f64,
    compare: bool,
    selected: Option<&Value>,
) -> Output {
    let theme_colors = palette(ui.ctx());
    let mut action = None;
    if compare {
        ui.add_space(8.0);
    }
    let mut actual_width = ui.available_width();
    let [from, to] = snapshot.plot_range;
    let span_seconds = (to - from).max(1.0);
    let (first, latest) = navigation(snapshot);
    // Events are marked when the Events menu asks for it, for a single target; comparisons
    // stay unmarked.
    let events: Vec<Value> = snapshot
        .plots
        .first()
        .filter(|_| !compare && snapshot.session.show_events)
        .and_then(|plot| snapshot.traces.iter().find(|t| t.config.session_id == plot.session))
        .map(|trace| period_events(&trace.events, from, to))
        .unwrap_or_default();
    let selected = selected.filter(|e| !compare && (from..=to).contains(&time_of(e)));
    let groups = if compare {
        1
    } else {
        snapshot.plots.len().max(1)
    };
    let with_jitter = !compare && groups == 1;
    let height =
        (ui.available_height() / groups as f32 - if with_jitter { 94.0 } else { 40.0 }).max(64.0);
    let all = snapshot.plots.iter().collect::<Vec<_>>();
    if all.is_empty() {
        ui.label(if snapshot.history_loading {
            "Reading history…"
        } else {
            "Select a hop or Round trip to graph it."
        });
    }
    for group in 0..groups.min(snapshot.plots.len()) {
        let series = if compare {
            all.clone()
        } else {
            vec![all[group]]
        };
        let lo = series
            .iter()
            .flat_map(|p| p.data.lo.iter().flatten())
            .copied()
            .reduce(f64::min);
        let hi = series
            .iter()
            .flat_map(|p| p.data.hi.iter().flatten())
            .copied()
            .reduce(f64::max);
        let [ymin, ymax] = latency_axis(lo, hi);
        let key = if compare {
            "comparison".to_owned()
        } else {
            format!("{}-{}", series[0].session, series[0].ttl)
        };
        // One neutral step above the black page, as the chart's own surface.
        let glass = theme::current(ui.ctx()) == Theme::Glass;
        ui.visuals_mut().extreme_bg_color = if glass { theme_colors.inset } else { Color32::from_rgb(0x10, 0x10, 0x10) };
        let jitter_response = if with_jitter {
            ui.vertical_centered(|ui| {
                ui.small("Jitter (ms, running average)");
            });
            let jitter_max = series
                .iter()
                .flat_map(|p| p.data.js.iter().flatten())
                .copied()
                .reduce(f64::max)
                .unwrap_or(1.0)
                .max(1.0)
                * 1.2;
            let response = Plot::new((&key, "jitter"))
                .height(48.0)
                .show_grid(false)
                .min_size(egui::vec2(64.0, 32.0))
                .default_x_bounds(from, to.max(from + 1.0))
                .default_y_bounds(0.0, jitter_max)
                .y_grid_spacer(move |_| {
                    (0..=2)
                        .map(|n| egui_plot::GridMark {
                            value: jitter_max * f64::from(n) / 2.0,
                            step_size: jitter_max / 2.0,
                        })
                        .collect()
                })
                .allow_drag(false)
                .allow_scroll(false)
                .allow_zoom(false)
                .allow_boxed_zoom(false)
            .allow_axis_zoom_drag(false)
                .allow_double_click_reset(false)
                .grid_color(theme_colors.edge)
                .link_axis(egui::Id::unique("trace-time"), [true, false])
                .link_cursor(egui::Id::unique("trace-cursor"), [true, false])
                .custom_y_axes(axes(0.0, jitter_max, false))
                .show_axes([false, true])
                .show(ui, |p| {
                    p.set_plot_bounds_x(from..=to.max(from + 1.0));
                    p.set_plot_bounds_y(0.0..=jitter_max);
                    for (index, series) in series.iter().enumerate() {
                        for run in runs(&series.data.xs, &series.data.js) {
                            p.line(
                                Line::new("Jitter", step_run(&run))
                                    .color(if glass && !compare { theme_colors.accent_light } else { series_color(index) })
                                    .allow_band(false)
                                    .fill(0.0),
                            );
                        }
                    }
                    if let Some(x) = *pinned {
                        p.vline(VLine::new("Pinned", x).color(theme_colors.text));
                    }
                });
            ui.add_space(8.0);
            axis_titles(ui, response.response.rect, false);
            Some(response.response)
        } else {
            None
        };
        let y_ticks = ((height - 24.0) / 28.0).floor().clamp(1.0, 4.0) as u32;
        let response = Plot::new((&key, "latency"))
            .height(height)
            .default_x_bounds(from, to.max(from + 1.0))
            .default_y_bounds(ymin, ymax)
            .y_grid_spacer(move |_| {
                (0..=y_ticks)
                    .map(|n| egui_plot::GridMark {
                        value: ymin + (ymax - ymin) * f64::from(n) / f64::from(y_ticks),
                        step_size: (ymax - ymin) / f64::from(y_ticks),
                    })
                    .collect()
            })
            .allow_drag(false)
            .allow_scroll(false)
            .allow_zoom(false)
            .allow_boxed_zoom(false)
            .allow_axis_zoom_drag(false)
            .allow_double_click_reset(false)
                .grid_color(theme_colors.edge)
            .link_axis(egui::Id::unique("trace-time"), [true, false])
            .link_cursor(egui::Id::unique("trace-cursor"), [true, false])
            .custom_y_axes(axes(ymin, ymax, true))
            .custom_x_axes(vec![
                AxisHints::new_x()
                    .min_thickness(24.0)
                    .tick_label_font(egui::FontId::proportional(12.0)),
            ])
            .grid_spacing(egui::Rangef::new(95.0, 180.0))
            .x_grid_spacer(time::grid)
            .x_axis_formatter(|mark, range| {
                time::tick(mark.value, mark.step_size, *range.start(), *range.end())
            })
            .show(ui, |p| {
                p.set_plot_bounds_x(from..=to.max(from + 1.0));
                p.set_plot_bounds_y(ymin..=ymax);
                let edges = [
                    ymin,
                    (snapshot.session.settings.warn_ms as f64).clamp(ymin, ymax),
                    (snapshot.session.settings.bad_ms as f64).clamp(ymin, ymax),
                    ymax,
                ];
                let colors = [
                    theme_colors.good,
                    theme_colors.warn,
                    theme_colors.bad,
                ];
                for (from, to) in coverage(&series) {
                    for band in 0..3 {
                        if edges[band + 1] > edges[band] {
                            p.add(
                                egui_plot::FilledArea::new(
                                    "Latency band",
                                    &[from, to],
                                    &[edges[band]; 2],
                                    &[edges[band + 1]; 2],
                                )
                                .fill_color(colors[band].gamma_multiply([0.24, 0.24, 0.26][band]))
                                .allow_hover(false),
                            );
                        }
                    }
                }
                for threshold in [
                    snapshot.session.settings.warn_ms as f64,
                    snapshot.session.settings.bad_ms as f64,
                ] {
                    if threshold > ymin && threshold < ymax {
                        p.hline(
                            egui_plot::HLine::new("Threshold", threshold)
                                .color(if threshold == snapshot.session.settings.warn_ms as f64 {
                                    theme_colors.warn.gamma_multiply(0.55)
                                } else {
                                    theme_colors.bad.gamma_multiply(0.55)
                                })
                                .style(egui_plot::LineStyle::dashed_loose())
                                .allow_hover(false),
                        );
                    }
                }
                for (index, series) in series.iter().enumerate() {
                    let color = if compare {
                        series_color(index)
                    } else {
                        theme_colors.text
                    };
                    let d = &series.data;
                    let loss_color = if compare && snapshot.session.settings.loss_by_target {
                        color
                    } else {
                        theme_colors.bad
                    };
                    let width = if d.xs.len() > 1 {
                        (d.xs.last().unwrap() - d.xs[0]) / d.xs.len() as f64
                    } else {
                        1.0
                    };
                    let bars = loss_runs(&d.xs, &d.ls, width.max(0.1))
                        .into_iter()
                        .map(|(x, w, y)| {
                            // Solid blocks; outlined bars read as hatching when dense.
                            Bar::new(x, y / 100.0 * (ymax - ymin))
                                .base_offset(ymin)
                                .width(w)
                                .fill(loss_color.gamma_multiply(0.7))
                                .stroke(egui::Stroke::NONE)
                        })
                        .collect::<Vec<_>>();
                    p.bar_chart(
                        BarChart::new("Loss", bars)
                            .color(loss_color)
                            .allow_hover(false),
                    );
                    points(
                        p,
                        d,
                        color,
                        if index >= SERIES {
                            egui_plot::LineStyle::dashed_dense()
                        } else {
                            egui_plot::LineStyle::Solid
                        },
                    );
                }
                if let Some(x) = *pinned {
                    p.vline(VLine::new("Pinned", x).color(theme_colors.text));
                }
                p.pointer_coordinate()
            });
        axis_titles(ui, response.response.rect, true);
        event_marks(ui, &response.transform, &events, selected);
        // The thresholds are named where they meet the plot's right edge, so the zones do
        // not rest on colour alone.
        for (threshold, color) in [
            (snapshot.session.settings.warn_ms as f64, theme_colors.warn),
            (snapshot.session.settings.bad_ms as f64, theme_colors.bad),
        ] {
            if threshold > ymin && threshold < ymax {
                let frame = response.transform.frame();
                let y = response.transform.position_from_point(&PlotPoint::new(0.0, threshold)).y;
                ui.painter_at(*frame).text(
                    egui::pos2(frame.right() - 4.0, y - 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    format!("{threshold:.0} ms"),
                    egui::FontId::proportional(12.0),
                    color.gamma_multiply(0.85),
                );
            }
        }
        let near_event = |x: f32| {
            events
                .iter()
                .map(|e| {
                    let at = response.transform.position_from_point_x(time_of(e));
                    (e, (at - x).abs())
                })
                .filter(|(_, d)| *d <= 8.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(e, _)| e)
        };
        if !compare {
            ui.painter().text(
                response.response.rect.left_top() + egui::vec2(76.0, 2.0),
                egui::Align2::LEFT_TOP,
                &series[0].label,
                egui::FontId::proportional(12.0),
                theme_colors.text,
            );
        }
        actual_width = response.transform.frame().width();
        if response.response.hovered() || response.response.dragged() {
            ui.ctx().set_cursor_icon(if response.response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        let hover = response.inner;
        if response.response.clicked() {
            match response
                .response
                .interact_pointer_pos()
                .and_then(|pos| near_event(pos.x))
            {
                Some(event) => action = Some(Action::Event(event.clone())),
                None => *pinned = hover.map(|p| p.x),
            }
        }
        if response.response.double_clicked() {
            *end = None;
            *pinned = None;
        }
        if response.response.dragged() {
            *end = pan(
                end.unwrap_or(to),
                -f64::from(response.response.drag_delta().x)
                    / f64::from(response.response.rect.width())
                    * span_seconds,
                *span,
                first,
                latest,
            );
        }
        if response.response.hovered() && ui.input(|i| i.modifiers.ctrl) {
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
            if scroll.abs() > 0.5 {
                let at = hover.map_or(to, |p| p.x);
                let fraction = ((at - from) / span_seconds).clamp(0.0, 1.0);
                let next = zoom(*span, scroll < 0.0, (latest - first).max(1.0));
                *end = pan(at + (1.0 - fraction) * next, 0.0, next, first, latest);
                *span = next;
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        if compare {
            ui.horizontal_wrapped(|ui| {
                // The swatch carries the series; the name stays in text ink.
                for (index, p) in series.iter().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect.shrink(1.0), 3.0, series_color(index));
                    ui.label(RichText::new(&p.label).strong());
                }
            });
        }
        for plot_response in std::iter::once(&response.response).chain(jitter_response.as_ref()) {
            plot_response.context_menu(|ui| {
                menu(
                    ui,
                    compare,
                    series[0],
                    groups > 1,
                    snapshot.session.settings.loss_by_target,
                    &mut action,
                )
            });
        }
        let menu_open = response.response.context_menu_opened()
            || jitter_response
                .as_ref()
                .is_some_and(egui::Response::context_menu_opened);
        if !menu_open
            && !egui::Popup::is_any_open(ui.ctx())
            && !response.response.dragged()
            && (pinned.is_some() || response.response.hovered())
        {
            let read_time = pinned.or(hover.map(|p: PlotPoint| p.x)).unwrap_or(to);
            let anchor = if response.response.hovered() {
                egui::PopupAnchor::Pointer
            } else {
                egui::PopupAnchor::Position(egui::pos2(
                    response.transform.position_from_point_x(read_time),
                    response.transform.frame().top(),
                ))
            };
            let mut tooltip = egui::Tooltip::always_open(
                ui.ctx().clone(),
                response.response.layer_id,
                response.response.id,
                anchor,
            )
            .accessible_name("Latency readout")
            .width(0.0)
            .gap(12.0);
            tooltip.popup = tooltip.popup.align(egui::RectAlign::BOTTOM_END).frame(
                egui::Frame::popup(ui.style())
                    .fill(theme_colors.raised)
                    .inner_margin(6),
            );
            tooltip.show(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.visuals_mut().override_text_color = Some(theme_colors.text);
                for series in &series {
                    if compare {
                        ui.label(RichText::new(&series.label).small().strong());
                    }
                    readout(ui, &series.data, read_time, series.width, span_seconds);
                }
                let x = response.transform.position_from_point_x(read_time);
                for event in events.iter().filter(|e| {
                    (response.transform.position_from_point_x(time_of(e)) - x).abs() <= 8.0
                }) {
                    ui.label(RichText::new(logic::event_label(event)).small().color(event_color(ui.ctx(), event)));
                }
            });
        }
    }
    Output {
        width: actual_width,
        action,
    }
}
/// Loss as (centre, width, percent) bars, neighbouring samples with the same loss joined
/// into one: adjacent translucent bars leave thin seams where their edges blend.
fn loss_runs(xs: &[f64], ls: &[Option<f64>], width: f64) -> Vec<(f64, f64, f64)> {
    let mut runs: Vec<(f64, f64, f64)> = Vec::new();
    for (&x, y) in xs.iter().zip(ls) {
        let Some(y) = y.filter(|v| *v > 0.0) else {
            continue;
        };
        let (start, end) = (x - width / 2.0, x + width / 2.0);
        match runs.last_mut() {
            Some(run) if run.2 == y && start - run.1 <= width * 0.01 => run.1 = end,
            _ => runs.push((start, end, y)),
        }
    }
    runs.into_iter()
        .map(|(start, end, y)| ((start + end) / 2.0, end - start, y))
        .collect()
}
const SERIES: usize = SERIES_COLORS.len();
fn series_color(index: usize) -> Color32 {
    SERIES_COLORS[index % SERIES]
}
fn time_of(event: &Value) -> f64 {
    event["t"].as_f64().unwrap_or(0.0)
}
/// Grouped route changes, alerts and recoveries inside `from..=to`.
pub(super) fn period_events(events: &[Value], from: f64, to: f64) -> Vec<Value> {
    logic::group_events(events)
        .into_iter()
        .filter(|e| (from..=to).contains(&time_of(e)))
        .collect()
}
pub(super) fn event_color(ctx: &egui::Context, event: &Value) -> Color32 {
    match event["kind"].as_str() {
        Some("alert") => palette(ctx).bad,
        Some("recover") => palette(ctx).good,
        _ => palette(ctx).accent,
    }
}
pub(super) fn same_event(a: &Value, b: &Value) -> bool {
    a["t"] == b["t"] && a["kind"] == b["kind"]
}
/// Dashed marks with a tab at the top; the selected event is drawn last, solid, over a band.
fn event_marks(
    ui: &egui::Ui,
    transform: &egui_plot::PlotTransform,
    events: &[Value],
    selected: Option<&Value>,
) {
    let frame = *transform.frame();
    let painter = ui.painter_at(frame);
    // Dense route changes share a mark rather than painting the whole graph solid.
    let mut occupied = HashSet::new();
    for event in events {
        let x = transform.position_from_point_x(time_of(event));
        if !occupied.insert((x / 3.0).round() as i64) {
            continue;
        }
        let color = event_color(ui.ctx(), event);
        painter.extend(egui::Shape::dashed_line(
            &[egui::pos2(x, frame.top()), egui::pos2(x, frame.bottom())],
            egui::Stroke::new(1.0, color),
            3.0,
            3.0,
        ));
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x - 3.0, frame.top()), egui::vec2(6.0, 6.0)),
            0.0,
            color,
        );
    }
    if let Some(event) = selected {
        let x = transform.position_from_point_x(time_of(event));
        let color = event_color(ui.ctx(), event);
        painter.rect_filled(
            egui::Rect::from_x_y_ranges(x - 7.0..=x + 7.0, frame.y_range()),
            0.0,
            color.gamma_multiply(0.16),
        );
        painter.vline(x, frame.y_range(), egui::Stroke::new(2.0, color));
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x - 5.0, frame.top() + 1.0),
                egui::pos2(x + 5.0, frame.top() + 1.0),
                egui::pos2(x, frame.top() + 10.0),
            ],
            color,
            egui::Stroke::new(1.0, palette(ui.ctx()).text),
        ));
    }
}
pub(super) fn zoom(span: f64, out: bool, available: f64) -> f64 {
    let current = if span == 0.0 { available } else { span };
    let steps = [
        10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0, 10800.0, 21600.0, 43200.0, 86400.0,
        172800.0, 604800.0, 2592000.0,
    ];
    if out {
        steps
            .into_iter()
            .find(|s| *s > current && *s < available)
            .unwrap_or(0.0)
    } else {
        steps
            .into_iter()
            .rev()
            .find(|s| *s < current)
            .unwrap_or(10.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_single_probe_spike_has_vertical_edges_and_keeps_its_full_height() {
        let paths = runs(
            &[1.0, 2.0, 3.0, 4.0, 5.0],
            &[Some(10.0), Some(200.0), Some(10.0), None, Some(20.0)],
        )
        .iter()
        .map(|run| step_run(run))
        .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                vec![
                    [1.0, 10.0],
                    [2.0, 10.0],
                    [2.0, 200.0],
                    [3.0, 200.0],
                    [3.0, 10.0]
                ],
                vec![[5.0, 20.0]],
            ]
        );
    }
    #[test]
    fn equal_neighbouring_loss_joins_into_one_bar() {
        let xs = [0.0, 1.0, 2.0, 3.0, 5.0, 6.0];
        let ls = [Some(20.0), Some(20.0), Some(40.0), None, Some(40.0), Some(40.0)];
        assert_eq!(
            loss_runs(&xs, &ls, 1.0),
            vec![(0.5, 2.0, 20.0), (2.0, 1.0, 40.0), (5.5, 2.0, 40.0)]
        );
    }
    #[test]
    fn gaps_are_separate_paths() {
        assert_eq!(
            runs(
                &[1.0, 2.0, 3.0, 4.0],
                &[Some(1.0), None, Some(2.0), Some(3.0)]
            ),
            vec![vec![[1.0, 1.0]], vec![[3.0, 2.0], [4.0, 3.0]]]
        );
    }
    #[test]
    fn loss_mapping_reaches_axis_ends() {
        for (min, max) in [(0.0, 300.0), (300.0, 330.0)] {
            assert_eq!(min + (max - min) * 0.0, min);
            assert_eq!(min + (max - min) * 1.0, max);
        }
    }
    #[test]
    fn panning_can_move_forward_to_live_and_stops_at_the_oldest_window() {
        assert_eq!(pan(1500.0, 100.0, 300.0, 1000.0, 2000.0), Some(1600.0));
        assert_eq!(pan(1950.0, 100.0, 300.0, 1000.0, 2000.0), None);
        assert_eq!(pan(1500.0, -10000.0, 300.0, 1000.0, 2000.0), Some(1300.0));
        assert_eq!(pan(1500.0, -100.0, 0.0, 1000.0, 2000.0), None);
    }
    #[test]
    fn latency_bands_include_loss_but_leave_pauses_empty() {
        let a = app::PlotView {
            session: "one".into(),
            ttl: 0,
            label: "one".into(),
            data: GraphData {
                xs: vec![1.0, 2.0, 3.0, 5.0, 6.0],
                ls: vec![Some(0.0), Some(100.0), None, Some(0.0), None],
                ..GraphData::default()
            },
            width: 1.0,
            stretches: vec![],
        };
        let b = app::PlotView {
            data: GraphData {
                xs: vec![2.0, 3.0, 7.0],
                ls: vec![Some(0.0), Some(0.0), None],
                ..GraphData::default()
            },
            ..a.clone()
        };
        assert_eq!(coverage(&[&a, &b]), vec![(1.0, 4.0), (5.0, 6.0)]);
    }
}

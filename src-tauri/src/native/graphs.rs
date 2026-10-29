//! Prepared, bounded series; the UI owns time bounds and loss-axis conversion.
use super::*;
use crate::logic::{GraphData, latency_axis};
use app::Snapshot;
use egui_plot::{AxisHints, Bar, BarChart, HPlacement, Line, Plot, PlotPoint, VLine};

const COLORS: [Color32; 6] = [
    Color32::from_rgb(84, 186, 244),
    Color32::from_rgb(169, 139, 250),
    Color32::from_rgb(68, 211, 170),
    Color32::from_rgb(255, 190, 92),
    Color32::from_rgb(239, 126, 179),
    Color32::from_rgb(138, 205, 86),
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
fn points(
    plot: &mut egui_plot::PlotUi<'_>,
    data: &GraphData,
    color: Color32,
    style: egui_plot::LineStyle,
    stepped: bool,
) {
    let mut band = vec![];
    let flush = |plot: &mut egui_plot::PlotUi<'_>, band: &mut Vec<usize>| {
        if band.len() > 1 {
            let xs = band.iter().map(|&i| data.xs[i]).collect::<Vec<_>>();
            let lo = band
                .iter()
                .map(|&i| data.lo[i].unwrap_or_default())
                .collect::<Vec<_>>();
            let hi = band
                .iter()
                .map(|&i| data.hi[i].unwrap_or_default())
                .collect::<Vec<_>>();
            plot.add(
                egui_plot::FilledArea::new("Range", &xs, &lo, &hi)
                    .fill_color(color.gamma_multiply(0.12))
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
        let run = if stepped {
            let mut points = Vec::new();
            for (i, point) in run.iter().enumerate() {
                if i > 0 {
                    points.push([point[0], run[i - 1][1]]);
                }
                points.push(*point);
            }
            points
        } else {
            run
        };
        if run.len() == 1 {
            plot.points(
                egui_plot::Points::new("Latency", run)
                    .color(color)
                    .radius(2.0),
            );
        } else {
            plot.line(Line::new("Latency", run).color(color).style(style));
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
fn axes(min: f64, max: f64, loss: bool) -> Vec<AxisHints<'static>> {
    vec![
        AxisHints::new_y()
            .label("ms")
            .min_thickness(58.0)
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
            .label(if loss { "Loss" } else { " " })
            .min_thickness(46.0)
            .formatter(move |mark, _| {
                if loss {
                    format!("{:.0}%", (mark.value - min) / (max - min) * 100.0)
                } else {
                    String::new()
                }
            }),
    ]
}
fn value(data: &GraphData, time: f64, width: f64) -> String {
    let i = data.xs.partition_point(|x| *x <= time).saturating_sub(1);
    if data.xs.is_empty() || time < data.xs[0] || time > *data.xs.last().unwrap_or(&time) + width {
        return "No probes in this period".into();
    }
    format!(
        "{}    RTT {}    Jitter {}    Loss {}",
        time::date_time(data.xs[i]),
        ms(data.ys[i]),
        ms(data.js[i]),
        data.ls[i].map_or_else(|| "–".into(), |v| format!("{v:.1}%"))
    )
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
) -> f32 {
    let mut actual_width = ui.available_width();
    let [from, to] = snapshot.plot_range;
    let span_seconds = (to - from).max(1.0);
    let (first, latest) = navigation(snapshot);
    let groups = if compare {
        1
    } else {
        snapshot.plots.len().max(1)
    };
    let readouts = if compare {
        snapshot.plots.len().max(1)
    } else {
        1
    };
    let height =
        (ui.available_height() / groups as f32 - 112.0 - (readouts - 1) as f32 * 18.0).max(85.0);
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
        ui.horizontal_wrapped(|ui| {
            for (index, p) in series.iter().enumerate() {
                ui.colored_label(
                    COLORS[index % COLORS.len()],
                    RichText::new(&p.label).strong(),
                );
            }
            ui.weak("Loss 0–100%");
        });
        let key = if compare {
            "comparison".to_owned()
        } else {
            format!("{}-{}", series[0].session, series[0].ttl)
        };
        let response = Plot::new((&key, "latency"))
            .height(height)
            .default_x_bounds(from, to.max(from + 1.0))
            .default_y_bounds(ymin, ymax)
            .y_grid_spacer(move |_| {
                (0..=4)
                    .map(|n| egui_plot::GridMark {
                        value: ymin + (ymax - ymin) * f64::from(n) / 4.0,
                        step_size: (ymax - ymin) / 4.0,
                    })
                    .collect()
            })
            .allow_drag(false)
            .allow_scroll(false)
            .allow_zoom(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .link_axis(egui::Id::unique("trace-time"), [true, false])
            .link_cursor(egui::Id::unique("trace-cursor"), [true, false])
            .custom_y_axes(axes(ymin, ymax, true))
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
                    Color32::from_rgb(23, 92, 65),
                    Color32::from_rgb(125, 92, 20),
                    Color32::from_rgb(122, 39, 52),
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
                                .fill_color(colors[band].gamma_multiply(0.12))
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
                                    Color32::from_rgb(123, 103, 42)
                                } else {
                                    Color32::from_rgb(119, 56, 64)
                                })
                                .allow_hover(false),
                        );
                    }
                }
                for (index, series) in series.iter().enumerate() {
                    let color = COLORS[index % COLORS.len()];
                    let d = &series.data;
                    let loss_color = if compare && snapshot.session.settings.loss_by_target {
                        color
                    } else {
                        Color32::from_rgb(223, 78, 93)
                    };
                    let width = if d.xs.len() > 1 {
                        (d.xs.last().unwrap() - d.xs[0]) / d.xs.len() as f64
                    } else {
                        1.0
                    };
                    let bars =
                        d.xs.iter()
                            .zip(&d.ls)
                            .filter_map(|(&x, y)| {
                                y.filter(|v| *v > 0.0).map(|y| {
                                    Bar::new(x, y / 100.0 * (ymax - ymin))
                                        .base_offset(ymin)
                                        .width(width.max(0.1))
                                })
                            })
                            .collect::<Vec<_>>();
                    p.bar_chart(
                        BarChart::new("Loss", bars)
                            .color(loss_color.gamma_multiply(0.35))
                            .allow_hover(false),
                    );
                    points(
                        p,
                        d,
                        color,
                        if index >= COLORS.len() {
                            egui_plot::LineStyle::dashed_dense()
                        } else {
                            egui_plot::LineStyle::Solid
                        },
                        compare,
                    );
                }
                if let Some(x) = *pinned {
                    p.vline(VLine::new("Pinned", x).color(Color32::WHITE));
                }
                p.pointer_coordinate()
            });
        actual_width = response.transform.frame().width();
        let hover = response.inner;
        if response.response.clicked() {
            *pinned = hover.map(|p| p.x);
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
        let jitter_max = series
            .iter()
            .flat_map(|p| p.data.js.iter().flatten())
            .copied()
            .reduce(f64::max)
            .unwrap_or(1.0)
            .max(1.0)
            * 1.2;
        Plot::new((&key, "jitter"))
            .height(53.0)
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
            .allow_double_click_reset(false)
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
                            Line::new("Jitter", run)
                                .color(COLORS[index % COLORS.len()])
                                .fill(0.0),
                        );
                    }
                }
                if let Some(x) = *pinned {
                    p.vline(VLine::new("Pinned", x).color(Color32::WHITE));
                }
            });
        let read_time = pinned.or(hover.map(|p: PlotPoint| p.x)).unwrap_or(to);
        for series in &series {
            ui.small(format!(
                "{}: {}",
                series.label,
                value(&series.data, read_time, series.width)
            ));
        }
    }
    actual_width
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

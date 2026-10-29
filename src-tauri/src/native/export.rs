//! Native dialogs and exports run on workers; images come from the rendered graph view.
use super::*;
use app::Snapshot;
use serde_json::Value;
use std::collections::HashSet;
use std::io::BufRead;
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Text,
    Csv,
    Samples,
    Stretches,
    Png,
    CopyImage,
    Pdf,
}
fn csv(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn host(snapshot: &Snapshot) -> String {
    snapshot
        .traces
        .iter()
        .find(|t| t.config.session_id == snapshot.session.active)
        .map_or_else(|| "all-targets".into(), |t| t.config.host.clone())
}
fn traces(snapshot: &Snapshot) -> impl Iterator<Item = &TraceView> {
    snapshot.traces.iter().filter(|t| {
        snapshot.session.active == "summary" || t.config.session_id == snapshot.session.active
    })
}
fn save_dialog(snapshot: &Snapshot, kind: &str, extension: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(time::export_name(&host(snapshot), kind, extension))
        .add_filter(extension, &[extension])
        .save_file()
}
pub(super) fn report(snapshot: &Snapshot, comma: bool) -> String {
    let shown = ordered_columns(&snapshot.session)
        .into_iter()
        .filter(|id| id != "bar" && !snapshot.session.hidden_columns.contains(id))
        .collect::<Vec<_>>();
    let headers = std::iter::once("Hop".to_owned())
        .chain(shown.iter().map(|id| {
            COLUMNS
                .iter()
                .find(|c| c.0 == id)
                .map_or("", |c| c.1)
                .to_owned()
        }))
        .collect::<Vec<_>>();
    let mut out = String::new();
    for trace in traces(snapshot) {
        if !comma {
            out.push_str(&format!(
                "TracePlotter report: {} ({})\n{} to {}\n\n",
                trace.config.host,
                trace.addr,
                time::date_time(snapshot.plot_range[0]),
                time::date_time(snapshot.plot_range[1])
            ));
        }
        let rows = (0..=trace.rows.len())
            .map(|i| {
                let hop = trace.rows.get(i);
                let stats = hop.map_or(trace.total, |h| h.stats);
                std::iter::once(hop.map_or_else(|| "RT".into(), |h| h.ttl.to_string()))
                    .chain(
                        shown
                            .iter()
                            .map(|id| cell(id, hop, stats, hop.map_or(trace.total_extras, |h| h.extras)).replace('–', "-")),
                    )
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if comma {
            out.push_str(&headers.iter().map(|s| csv(s)).collect::<Vec<_>>().join(","));
            out.push('\n');
            for row in rows {
                out.push_str(&row.iter().map(|s| csv(s)).collect::<Vec<_>>().join(","));
                out.push('\n');
            }
        } else {
            let widths = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    rows.iter()
                        .map(|r| r[i].chars().count())
                        .max()
                        .unwrap_or(0)
                        .max(h.chars().count())
                })
                .collect::<Vec<_>>();
            for row in std::iter::once(headers.clone()).chain(rows) {
                let line = row
                    .iter()
                    .enumerate()
                    .map(|(i, cell)| {
                        if i > 0 && ["ip", "name"].contains(&shown[i - 1].as_str()) {
                            format!("{cell:width$}", width = widths[i])
                        } else {
                            format!("{cell:>width$}", width = widths[i])
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                out.push_str(line.trim_end());
                out.push('\n');
            }
        }
    }
    out
}
pub(super) fn save(snapshot: &Snapshot, kind: Kind) -> Result<Option<String>, String> {
    let kind = if kind == Kind::Samples && snapshot.session.active == "summary" {
        Kind::Stretches
    } else {
        kind
    };
    let extension = if kind == Kind::Text { "txt" } else { "csv" };
    let name = match kind {
        Kind::Samples => "samples",
        Kind::Stretches => "stretches",
        _ => "report",
    };
    let Some(path) = save_dialog(snapshot, name, extension) else {
        return Ok(None);
    };
    let mut out = std::io::BufWriter::new(fs::File::create(&path).map_err(|e| e.to_string())?);
    match kind {
        Kind::Text | Kind::Csv => out
            .write_all(report(snapshot, kind == Kind::Csv).as_bytes())
            .map_err(|e| e.to_string())?,
        Kind::Samples => {
            writeln!(out, "time,hop,ip,rtt_ms").map_err(|e| e.to_string())?;
            for trace in traces(snapshot) {
                let dir = history_dir(&trace.config.session_id)?;
                for (_, day) in day_files(&dir) {
                    let file = fs::File::open(day).map_err(|e| e.to_string())?;
                    for line in std::io::BufReader::new(file).lines() {
                        let line = line.map_err(|e| e.to_string())?;
                        if let Ok(sample) = serde_json::from_str::<Sample>(&line) {
                            for hop in sample.hops {
                                writeln!(
                                    out,
                                    "{},{},{},{}",
                                    iso_time(sample.t),
                                    hop.ttl,
                                    hop.addr.map_or_else(String::new, |a| a.to_string()),
                                    hop.rtt.map_or_else(String::new, |v| v.to_string())
                                )
                                .map_err(|e| e.to_string())?;
                            }
                        }
                    }
                }
            }
        }
        Kind::Stretches => {
            writeln!(
                out,
                "time,host,seconds,sent,lost,avg_ms,min_ms,max_ms,jitter_ms"
            )
            .map_err(|e| e.to_string())?;
            for plot in &snapshot.plots {
                let host = snapshot
                    .traces
                    .iter()
                    .find(|t| t.config.session_id == plot.session)
                    .map_or(plot.label.as_str(), |t| t.config.host.as_str());
                for stretch in &plot.stretches {
                    let s = logic::Stats::from(stretch.tally);
                    let ms = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:.2}"));
                    writeln!(
                        out,
                        "{},{},{},{},{},{},{},{},{}",
                        iso_time(stretch.t),
                        csv(host),
                        plot.width,
                        s.sent,
                        s.sent - s.received,
                        ms(s.avg),
                        ms(s.min),
                        ms(s.max),
                        ms(s.jitter)
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        _ => return Err("this export needs a rendered image".into()),
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(Some(format!("Saved {}", path.display())))
}
/// Render on the existing device with a separate renderer and font atlas.
/// This includes scrolled-out rows and graphs without changing the user's window.
fn paint(
    gpu: &eframe::egui_wgpu::RenderState,
    size: [u32; 2],
    pixels_per_point: f32,
    theme: Theme,
    mut draw: impl FnMut(&mut egui::Ui),
) -> Result<image::RgbaImage, String> {
    use eframe::{
        egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor, capture::CaptureState},
        wgpu,
    };
    let logical_size = size;
    let pixels_per_point = pixels_per_point.clamp(0.5, 4.0);
    let size = [
        (size[0] as f32 * pixels_per_point).ceil() as u32,
        (size[1] as f32 * pixels_per_point).ceil() as u32,
    ];
    if size
        .iter()
        .any(|n| *n == 0 || *n > gpu.device.limits().max_texture_dimension_2d)
    {
        return Err(
            "The export exceeds the graphics adapter's image size limit. Select fewer graphs."
                .into(),
        );
    }
    let ctx = egui::Context::default();
    apply_theme(&ctx, theme);
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(logical_size[0] as f32, logical_size[1] as f32),
        )),
        ..Default::default()
    };
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(pixels_per_point);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format, RendererOptions::PREDICTABLE);
    let mut last = None;
    for _ in 0..3 {
        let mut output = ctx.run_ui(raw.clone(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| draw(ui));
        });
        for (id, images) in output.textures_delta.set.drain() {
            for delta in images {
                renderer.update_texture(&gpu.device, &gpu.queue, id, &delta);
            }
        }
        for id in output.textures_delta.free.drain() {
            renderer.free_texture(&id);
        }
        last = Some(output);
    }
    let output = last.expect("three render passes were run");
    let shapes = ctx.tessellate(output.shapes, pixels_per_point);
    let screen = ScreenDescriptor {
        size_in_pixels: size,
        pixels_per_point,
    };
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let buffers = renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &shapes, &screen);
    let mut capture = CaptureState::new(
        &gpu.device,
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        format,
    );
    let view = capture.texture.create_view(&Default::default());
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("TracePlotter export"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            })
            .forget_lifetime();
        renderer.render(&mut pass, &shapes, &screen);
    }
    let buffer = capture.copy_textures(&gpu.device, None, &mut encoder);
    gpu.queue
        .submit(buffers.into_iter().chain([encoder.finish()]));
    let (send, receive) = mpsc::sync_channel(1);
    capture.read_screen_rgba(
        buffer,
        vec![egui::ScreenshotCallback::new(move |image| {
            let _ = send.send(image);
        })],
    );
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(15)),
        })
        .map_err(|e| format!("Cannot render export: {e}"))?;
    let image = receive
        .recv_timeout(Duration::from_secs(1))
        .map_err(|e| format!("Cannot read rendered export: {e}"))?;
    let bytes = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    image::RgbaImage::from_raw(size[0], size[1], bytes)
        .ok_or_else(|| "The renderer returned an incomplete image".into())
}
fn graph_image(
    gpu: &eframe::egui_wgpu::RenderState,
    snapshot: &Snapshot,
    pixels_per_point: f32,
) -> Result<image::RgbaImage, String> {
    if snapshot.plots.is_empty() {
        return Err("Select a target or comparison before exporting graphs.".into());
    }
    let compare = snapshot.session.active == "summary";
    let height = if compare {
        350 + snapshot.plots.len() as u32 * 22
    } else {
        100 + snapshot.plots.len() as u32 * 290
    };
    paint(gpu, [1100, height], pixels_per_point, snapshot.session.theme, |ui| {
        ui.heading(format!("TracePlotter — {}", host(snapshot)));
        ui.label(format!(
            "{} to {}",
            time::date_time(snapshot.plot_range[0]),
            time::date_time(snapshot.plot_range[1])
        ));
        let (mut end, mut pin, mut span) = (None, None, snapshot.session.span);
        graphs::show(ui, snapshot, &mut end, &mut pin, &mut span, compare, None);
    })
}
fn report_row_height(hop: Option<&app::Row>) -> usize {
    hop.map_or(20, |h| {
        h.name
            .chars()
            .count()
            .div_ceil(29)
            .max(h.addr.len().div_ceil(30))
            .max(1)
            * 16
            + 6
    })
}
fn report_pages(
    gpu: &eframe::egui_wgpu::RenderState,
    snapshot: &Snapshot,
    pixels_per_point: f32,
) -> Result<Vec<image::RgbaImage>, String> {
    let shown = ordered_columns(&snapshot.session)
        .into_iter()
        .filter(|id| id != "bar" && !snapshot.session.hidden_columns.contains(id))
        .collect::<Vec<_>>();
    let mut pages = vec![];
    for trace in traces(snapshot) {
        let rows = trace
            .rows
            .iter()
            .map(Some)
            .chain([None])
            .collect::<Vec<_>>();
        let mut chunks = Vec::new();
        let (mut start, mut height) = (0, 0);
        for (i, row) in rows.iter().enumerate() {
            let h = report_row_height(*row);
            if height + h > 590 && i > start {
                chunks.push(start..i);
                start = i;
                height = 0;
            }
            height += h;
        }
        chunks.push(start..rows.len());
        for range in chunks {
            let chunk = &rows[range];
            pages.push(paint(gpu, [1100, 740], pixels_per_point, snapshot.session.theme, |ui| {
                ui.heading(format!(
                    "TracePlotter — {} ({})",
                    trace.config.host, trace.addr
                ));
                ui.label(format!(
                    "{} to {}",
                    time::date_time(snapshot.plot_range[0]),
                    time::date_time(snapshot.plot_range[1])
                ));
                ui.label(format!(
                    "{} · {} ms interval · {} bytes",
                    trace.config.protocol, trace.config.interval_ms, trace.config.packet_size
                ));
                ui.separator();
                egui::Grid::new("report-table")
                    .striped(true)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("Hop");
                        for id in &shown {
                            ui.strong(COLUMNS.iter().find(|c| c.0 == id).map_or("", |c| c.1));
                        }
                        ui.end_row();
                        for &hop in chunk {
                            let stats = hop.map_or(trace.total, |h| h.stats);
                            ui.monospace(hop.map_or_else(|| "RT".into(), |h| h.ttl.to_string()));
                            for id in &shown {
                                let text = cell(id, hop, stats, hop.map_or(trace.total_extras, |h| h.extras));
                                if ["ip", "name"].contains(&id.as_str()) {
                                    ui.add_sized(
                                        [230.0, (report_row_height(hop) - 6) as f32],
                                        egui::Label::new(text).wrap(),
                                    );
                                } else {
                                    ui.add(egui::Label::new(text).extend());
                                }
                            }
                            ui.end_row();
                        }
                    });
            })?);
        }
    }
    for chunk in snapshot.plots.chunks(2) {
        let mut graphs = snapshot.clone();
        graphs.plots = chunk.to_vec();
        pages.push(graph_image(gpu, &graphs, pixels_per_point)?);
    }
    if pages.is_empty() {
        return Err("There are no targets to report.".into());
    }
    Ok(pages)
}
pub(super) fn render(
    gpu: &eframe::egui_wgpu::RenderState,
    snapshot: &Snapshot,
    kind: Kind,
    pixels_per_point: f32,
) -> Result<Option<String>, String> {
    let pdf = kind == Kind::Pdf;
    let path = if kind == Kind::CopyImage {
        None
    } else {
        let path = save_dialog(
            snapshot,
            if pdf { "report" } else { "graph" },
            if pdf { "pdf" } else { "png" },
        );
        if path.is_none() {
            return Ok(None);
        }
        path
    };
    if pdf {
        let pages = report_pages(gpu, snapshot, pixels_per_point)?;
        fs::write(
            path.as_ref().expect("save dialog returned a path"),
            pdf_images(&pages),
        )
        .map_err(|e| e.to_string())?;
    } else {
        let image = graph_image(gpu, snapshot, pixels_per_point)?;
        if kind == Kind::CopyImage {
            // Linux clipboard ownership must outlive the worker that created the image.
            static CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
            let mut clipboard = CLIPBOARD
                .get_or_init(|| Mutex::new(None))
                .lock()
                .map_err(|e| e.to_string())?;
            if clipboard.is_none() {
                *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
            }
            clipboard
                .as_mut()
                .expect("clipboard initialized")
                .set_image(arboard::ImageData {
                    width: image.width() as usize,
                    height: image.height() as usize,
                    bytes: std::borrow::Cow::Owned(image.into_raw()),
                })
                .map_err(|e| e.to_string())?;
            return Ok(Some("Graphs copied to clipboard.".into()));
        } else {
            image
                .save(path.as_ref().expect("save dialog returned a path"))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(path.map(|p| format!("Saved {}", p.display())))
}
/// The displayed period of the open tab as one HTML file: graphs, probe settings, exact hop
/// statistics from the history files, incidents and events. Redaction replaces every name
/// and address, graph captions included, with numbered placeholders.
pub(super) fn support_report(
    gpu: &eframe::egui_wgpu::RenderState,
    snapshot: &Snapshot,
    redacted: bool,
    pixels_per_point: f32,
) -> Result<Option<String>, String> {
    let trace = snapshot
        .traces
        .iter()
        .find(|t| t.config.session_id == snapshot.session.active)
        .ok_or("Open a target before exporting a support report.")?;
    let config = &trace.config;
    let [from, to] = snapshot.plot_range;
    let details = crate::insights::read_details(
        &config.session_id,
        (from.max(0.0) * 1000.0).floor() as u64,
        (to.max(0.0) * 1000.0).floor() as u64 + 1,
        config.interval_ms,
    )?;
    // Older events from the log, and live ones not yet written to it.
    let mut seen = HashSet::new();
    let mut events: Vec<Value> = read_events(config.session_id.clone())
        .unwrap_or_default()
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .chain(trace.events.iter().cloned())
        .filter(|e| seen.insert(e.to_string()))
        .collect();
    events.sort_by(|a, b| {
        a["t"].as_f64().unwrap_or(0.0).total_cmp(&b["t"].as_f64().unwrap_or(0.0))
    });
    let addresses: Vec<String> = [config.host.clone(), trace.addr.clone()]
        .into_iter()
        .chain(trace.rows.iter().flat_map(|r| [r.addr.clone(), r.name.clone()]))
        .chain(events.iter().filter(|e| e["kind"] == "hop").flat_map(|e| {
            ["from", "to"].map(|k| e[k].as_str().unwrap_or_default().to_owned())
        }))
        .chain(details.iter().filter_map(|d| d.addr.map(|a| a.to_string())))
        .collect();
    let redactor = logic::redactor(&addresses);
    let plain = |text: &str| text.to_owned();
    let redact: &dyn Fn(&str) -> String = if redacted { &redactor } else { &plain };
    let mut shown = snapshot.clone();
    for t in &mut shown.traces {
        t.config.host = redact(&t.config.host);
    }
    for plot in &mut shown.plots {
        plot.label = redact(&plot.label);
    }
    let graph = graph_image(gpu, &shown, pixels_per_point).ok().and_then(|image| {
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        Some(format!("data:image/png;base64,{}", report::base64(&png)))
    });
    let probe = format!(
        "{}, interval {} ms, timeout {} ms, packet {} bytes, alert latency {} ms, alert loss {}%, alert window {} s",
        layout::short_protocol(config),
        config.interval_ms,
        config.timeout_ms,
        config.packet_size,
        config.alert_latency_ms,
        config.alert_loss_pct,
        config.alert_seconds
    );
    let html = report::support_report(&report::SupportReport {
        target: &format!("{} ({})", config.host, trace.addr),
        from,
        to,
        probe: &probe,
        details: &details,
        events: &events,
        graph: graph.as_deref(),
        redact,
        redacted,
        demo: demo(),
    });
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(time::export_name(
            if redacted { "redacted" } else { &config.host },
            "incident",
            "html",
        ))
        .add_filter("html", &["html"])
        .save_file()
    else {
        return Ok(None);
    };
    fs::write(&path, html).map_err(|e| e.to_string())?;
    Ok(Some(format!("Saved {}", path.display())))
}
fn pdf_images(images: &[image::RgbaImage]) -> Vec<u8> {
    use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};
    let mut pdf = Pdf::new();
    let (catalog, pages) = (Ref::new(1), Ref::new(2));
    let ids = (0..images.len())
        .map(|i| Ref::new(3 + i as i32 * 3))
        .collect::<Vec<_>>();
    pdf.catalog(catalog).pages(pages);
    pdf.pages(pages)
        .kids(ids.iter().copied())
        .count(images.len() as i32);
    for (i, pixels) in images.iter().enumerate() {
        let (page, image, stream) = (
            ids[i],
            Ref::new(ids[i].get() + 1),
            Ref::new(ids[i].get() + 2),
        );
        let mut p = pdf.page(page);
        p.parent(pages)
            .media_box(Rect::new(0.0, 0.0, 842.0, 595.0))
            .contents(stream);
        p.resources().x_objects().pair(Name(b"View"), image);
        p.finish();
        let rgb = pixels
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect::<Vec<_>>();
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&rgb, 6);
        let mut object = pdf.image_xobject(image, &compressed);
        object.filter(Filter::FlateDecode);
        object
            .width(pixels.width() as i32)
            .height(pixels.height() as i32)
            .bits_per_component(8);
        object.color_space().device_rgb();
        object.finish();
        let scale = (802.0 / pixels.width() as f32).min(555.0 / pixels.height() as f32);
        let mut content = Content::new();
        content.transform([
            pixels.width() as f32 * scale,
            0.0,
            0.0,
            pixels.height() as f32 * scale,
            20.0,
            595.0 - 20.0 - pixels.height() as f32 * scale,
        ]);
        content.x_object(Name(b"View"));
        pdf.stream(stream, &content.finish());
    }
    pdf.finish()
}

#[cfg(all(test, feature = "ui-tests"))]
pub(super) fn render_fixture(
    gpu: &eframe::egui_wgpu::RenderState,
    snapshot: &Snapshot,
    path: &Path,
) -> Result<(), String> {
    let pixels_per_point = 2.0;
    graph_image(gpu, snapshot, pixels_per_point)?
        .save(path.join("graphs-export.png"))
        .map_err(|e| e.to_string())?;
    let pages = report_pages(gpu, snapshot, pixels_per_point)?;
    fs::write(path.join("report-export.pdf"), pdf_images(&pages)).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pdf_contains_image_with_matching_dimensions() {
        let bytes =
            pdf_images(&[
                image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap(),
            ]);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-"));
        assert!(text.contains("/Width 2"));
        assert!(text.contains("/Height 1"));
    }
    #[test]
    fn csv_quotes_names() {
        assert_eq!(csv("a,\"b"), "\"a,\"\"b\"");
    }
}

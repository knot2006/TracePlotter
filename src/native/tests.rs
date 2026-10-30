use super::*;
use egui_kittest::{Harness, kittest::Queryable};
fn fixture(active: &str) -> Snapshot {
    let config = Config {
        session_id: "demo-one".into(),
        host: "example.com".into(),
        ..Config::default()
    };
    let mut trace = model::Trace::new(config.clone());
    for t in 1000..1601 {
        trace.apply(
            Sample {
                t: t * 1000,
                hops: (1..=9)
                    .map(|ttl| HopSample {
                        ttl,
                        addr: Some(format!("192.0.2.{ttl}").parse().unwrap()),
                        rtt: Some(f64::from(ttl) * 20.0 + (t % 20) as f64),
                    })
                    .collect(),
            },
            false,
        );
    }
    let stats = trace.target.stats(1000.0, 1600.0);
    let xs = (1000..=1600).map(f64::from).collect::<Vec<_>>();
    let ys = (1000..=1600)
        .map(|t| match t {
            1080 => Some(190.0),
            1190 => Some(150.0),
            1300..=1303 => None,
            1420 => Some(210.0),
            _ => Some(20.0 + f64::from(t % 7)),
        })
        .collect::<Vec<_>>();
    let data = logic::GraphData {
        xs,
        ys: ys.clone(),
        js: logic::jitter(&ys),
        ls: ys
            .iter()
            .map(|value| Some(if value.is_some() { 0.0 } else { 100.0 }))
            .collect(),
        lo: ys.clone(),
        hi: ys,
    };
    Snapshot {
        session: Session {
            active: active.into(),
            compare: vec!["demo-one".into()],
            targets: vec![config.clone()],
            check_updates: false,
            ..Session::default()
        },
        traces: vec![TraceView {
            config,
            paused: false,
            addr: "192.0.2.1".into(),
            loading: false,
            error: None,
            alerting: false,
            rows: trace
                .hops
                .iter()
                .map(|hop| app::Row {
                    ttl: hop.ttl,
                    addr: hop.addr.unwrap().to_string(),
                    name: format!("router{}.example", hop.ttl),
                    stats: hop.stats(1000.0, 1600.0),
                    extras: None,
                })
                .collect(),
            total: stats,
            total_extras: None,
            details_period: None,
            spark: vec![Some(20.0), None, Some(30.0)],
            events: vec![],
            from: 1000.0,
            to: 1600.0,
        }],
        plots: vec![app::PlotView {
            session: "demo-one".into(),
            ttl: 0,
            label: "Round trip".into(),
            data,
            width: 1.0,
            stretches: vec![],
        }],
        plot_range: [1000.0, 1600.0],
        ..Snapshot::default()
    }
}
fn harness(active: &str, size: [f32; 2]) -> (Harness<'static, Window>, mpsc::Receiver<Command>) {
    themed_harness(active, size, Theme::Ink)
}
fn themed_harness(
    active: &str,
    size: [f32; 2],
    theme: Theme,
) -> (Harness<'static, Window>, mpsc::Receiver<Command>) {
    let mut snapshot = fixture(active);
    snapshot.session.theme = theme;
    let (runtime, commands) = Runtime::fixture(snapshot);
    let h = Harness::builder().with_size(size).build_eframe(|cc| {
        apply_theme(&cc.egui_ctx, theme);
        let mut window = Window::with_runtime(runtime);
        window.placed = true;
        window.window_state = None;
        window
    });
    (h, commands)
}

#[test]
fn theme_choice_applies_without_restart_and_sends_a_saved_preference() {
    let (mut h, commands) = harness("settings", [1100.0, 720.0]);
    h.state_mut().settings_page = 1;
    h.run();
    assert!(h.query_all_by_label("User interface").next().is_none());
    assert!(h.query_by_label("Apply and restart").is_none());
    let ink_accent = palette(&h.ctx).accent;
    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Theme")
        .click();
    h.run();
    h.get_by_label("Glass").click();
    h.run();
    assert_ne!(palette(&h.ctx).accent, ink_accent);
    assert!(!h.state().busy && !h.state().needs_exit);
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Preferences(s) if s.theme == Theme::Glass))
    );

    h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Theme")
        .click();
    h.run();
    h.get_by_label("Ink").click();
    h.run();
    assert_eq!(palette(&h.ctx).accent, ink_accent);
}

#[test]
fn glass_transparency_slider_applies_and_sends_a_saved_preference() {
    let (mut h, commands) = themed_harness("settings", [1100.0, 720.0], Theme::Glass);
    h.state_mut().settings_page = 1;
    h.run();
    let rail = h
        .get_by_role_and_label(egui::accesskit::Role::Slider, "Transparency")
        .rect();
    h.hover_at(rail.left_center());
    h.drag_at(rail.left_center());
    h.run();
    h.hover_at(rail.center());
    h.run();
    h.drop_at(rail.center());
    h.run();
    assert!(theme::opacity(&h.ctx) < 1.0);
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Preferences(s) if s.transparency > 0.0))
    );
}

#[test]
fn glass_keeps_controls_visible_at_normal_and_minimum_sizes() {
    for size in [[1100.0, 720.0], [640.0, 420.0]] {
        let (mut h, _) = themed_harness("demo-one", size, Theme::Glass);
        h.run();
        assert_eq!(theme::current(&h.ctx), Theme::Glass);
        assert!(h.get_by_label("Pause").rect().right() <= size[0]);
        assert!(h.get_by_label("History position").rect().bottom() <= size[1]);
        h.state_mut().editing = Some((fixture("demo-one").session.targets[0].clone(), false));
        h.run();
        assert!(h.get_by_label("Close").rect().bottom() <= size[1]);
    }
}
#[test]
fn trace_controls_emit_typed_commands() {
    let (mut h, commands) = harness("demo-one", [1100.0, 720.0]);
    h.run();
    h.get_by_label("Pause").click();
    h.run();
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c,Command::Pause(id) if id=="demo-one"))
    );
    h.state_mut().end = Some(1500.0);
    h.run();
    h.get_by_label("Live").click();
    h.run();
    assert!(h.state().end.is_none());
}
#[test]
fn a_move_or_resize_gives_egui_back_the_release_the_compositor_kept() {
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    release_pointer_after_compositor(&ctx, &mut raw);
    assert!(raw.events.is_empty());
    hand_pointer_to_compositor(&ctx, egui::ViewportCommand::StartDrag);
    release_pointer_after_compositor(&ctx, &mut raw);
    assert!(matches!(
        raw.events[..],
        [egui::Event::PointerButton {
            button: egui::PointerButton::Primary,
            pressed: false,
            ..
        }]
    ));
    release_pointer_after_compositor(&ctx, &mut raw);
    assert_eq!(raw.events.len(), 1);
}

#[test]
fn window_corners_resize_from_either_edge_without_covering_controls() {
    use egui::{CursorIcon as C, ResizeDirection as D};
    let (mut h, _commands) = harness("demo-one", [1100.0, 720.0]);
    h.run();
    for (pos, direction, cursor) in [
        ([4.0, 14.0], D::NorthWest, C::ResizeNorthWest),
        ([14.0, 4.0], D::NorthWest, C::ResizeNorthWest),
        ([1096.0, 14.0], D::NorthEast, C::ResizeNorthEast),
        ([1086.0, 4.0], D::NorthEast, C::ResizeNorthEast),
        ([4.0, 706.0], D::SouthWest, C::ResizeSouthWest),
        ([14.0, 716.0], D::SouthWest, C::ResizeSouthWest),
        ([1096.0, 706.0], D::SouthEast, C::ResizeSouthEast),
        ([1086.0, 716.0], D::SouthEast, C::ResizeSouthEast),
    ] {
        let pos = egui::pos2(pos[0], pos[1]);
        h.hover_at(pos);
        h.run();
        assert_eq!(
            h.output().platform_output.cursor_icon,
            cursor,
            "Corner cursor at {pos:?}"
        );
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
        assert!(h.output().viewport_output.values().any(|viewport| viewport.commands.iter().any(|command| matches!(command, egui::ViewportCommand::BeginResize(actual) if *actual == direction))), "Start corner resize at {pos:?}");
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
    }
    for pos in [
        h.get_by_label("Close window").rect().center(),
        // Inside the Close button, near the corner, where the old target reached.
        egui::pos2(1080.0, 8.0),
        egui::pos2(1084.0, 705.0),
    ] {
        h.hover_at(pos);
        h.run();
        assert!(
            !matches!(
                h.output().platform_output.cursor_icon,
                C::ResizeNorthWest | C::ResizeNorthEast | C::ResizeSouthWest | C::ResizeSouthEast
            ),
            "Nearby controls must remain usable at {pos:?}"
        );
    }
}

#[test]
fn settings_and_minimum_window_are_accessible() {
    let (mut h, _commands) = harness("settings", [640.0, 420.0]);
    h.run();
    assert!(h.query_all_by_label("Wait for replies").next().is_some());
    h.get_by_label("All targets").click();
    h.run();
}

#[test]
fn legacy_layout_keeps_tabs_above_controls_and_history_below_plots() {
    for size in [[1100.0, 720.0], [1440.0, 900.0], [640.0, 420.0]] {
        let (mut h, _commands) = harness("demo-one", size);
        h.run();
        let tabs = h.get_by_label("example.com  ICMP").rect();
        let host = h.get_by_label("Host or IP").rect();
        let jitter = h.get_by_label("Jitter (ms, running average)").rect();
        let history = h.get_by_label("History position").rect();
        assert!(
            tabs.bottom() <= host.top(),
            "Tabs must precede the toolbar at {size:?}"
        );
        assert!(
            host.bottom() < jitter.top(),
            "Toolbar must precede plots at {size:?}"
        );
        assert!(
            jitter.bottom() < history.top(),
            "History slider must follow plots at {size:?}"
        );
        assert!(
            history.bottom() <= size[1],
            "History slider must remain visible at {size:?}: {history:?}"
        );
        assert!(
            h.get_by_label("Settings").rect().right() <= size[0],
            "Settings must remain inside the tab strip"
        );
    }
}

#[test]
fn settings_list_their_sections_and_show_one_page_at_a_time() {
    for size in [[1100.0, 720.0], [1440.0, 900.0]] {
        let (mut h, _commands) = harness("settings", size);
        h.run();
        let new_targets = h
            .get_by_role_and_label(egui::accesskit::Role::Tab, "New targets")
            .rect();
        let ai = h
            .get_by_role_and_label(egui::accesskit::Role::Tab, "AI agents")
            .rect();
        assert!(
            new_targets.top() < ai.top() && (new_targets.left() - ai.left()).abs() < 1.0,
            "Sections are listed down one column at {size:?}"
        );
        assert!(h.query_all_by_label("Wait for replies").next().is_some());
        assert!(
            h.query_all_by_label("Allow AI agents to connect")
                .next()
                .is_none()
        );
        h.get_by_role_and_label(egui::accesskit::Role::Tab, "AI agents")
            .click();
        h.run();
        assert!(
            h.query_all_by_label("Allow AI agents to connect")
                .next()
                .is_some()
        );
        assert!(h.query_all_by_label("Wait for replies").next().is_none());
    }
}

#[test]
fn toolbar_starts_a_new_target_and_modal_close_keeps_its_layout_visible() {
    let (mut h, commands) = harness("demo-one", [640.0, 420.0]);
    h.run();
    h.get_by_label("Host or IP")
        .set_text_value_accesskit("new.example");
    h.run();
    h.get_by_label("Start trace").click();
    h.run();
    assert!(commands.try_iter().any(|command|matches!(command,Command::Open(config) if config.host=="new.example" && config.interval_ms==1000)));
    h.state_mut().editing = Some((fixture("demo-one").session.targets[0].clone(), false));
    h.run();
    let close = h.get_by_label("Close");
    assert!(
        close.rect().right() <= 640.0 && close.rect().bottom() <= 420.0,
        "Dialog Close must remain visible in a small window"
    );
    close.click();
    h.run();
    assert!(h.state().editing.is_none());
}

#[test]
fn hover_readout_is_a_floating_tooltip_without_moving_the_graph() {
    for size in [
        [1100.0, 720.0],
        [1440.0, 900.0],
        [640.0, 420.0],
        [1865.0, 900.0],
    ] {
        let (mut h, _commands) = harness("demo-one", size);
        h.run();
        let history = h.get_by_label("History position").rect();
        let title = h
            .output()
            .shapes
            .iter()
            .rev()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Round trip" => Some(text.pos),
                _ => None,
            })
            .expect("Latency graph title must be painted");
        for pointer in [
            title + egui::vec2(150.0, 12.0),
            egui::pos2(size[0] - 70.0, history.top() - 50.0),
        ] {
            h.hover_at(pointer);
            h.run();
            assert!(h.query_by_label("Latency readout").is_some());
            let tip = h.get_by_label_contains("jitter ").rect();
            assert!(
                tip.width() > 60.0 && tip.height() < 20.0,
                "Tooltip values must stay on single lines: {tip:?}"
            );
            assert!(
                egui::Rect::from_min_size(egui::Pos2::ZERO, size.into()).contains_rect(tip),
                "Tooltip must stay on screen at {size:?}: {tip:?}"
            );
            assert!(
                tip.center().distance(pointer) < 160.0,
                "Tooltip must follow the pointer"
            );
            assert!(h.query_by_label_contains("jitter ").is_some());
            assert!(h.query_by_label_contains("Round trip:").is_none());
            assert_eq!(
                history,
                h.get_by_label("History position").rect(),
                "Hover must not move history"
            );
        }
        h.hover_at(egui::pos2(1.0, 1.0));
        h.run();
        assert!(h.query_by_label("Latency readout").is_none());
        h.state_mut().pinned = Some(1190.0);
        h.run();
        assert!(h.query_by_label("150.0 ms").is_some());
    }
}

#[test]
fn graph_context_menu_restores_live_and_comparison_options() {
    let (mut h, _commands) = harness("demo-one", [1100.0, 720.0]);
    h.state_mut().end = Some(1500.0);
    h.state_mut().pinned = Some(1190.0);
    h.run();
    let history = h.get_by_label("History position").rect();
    secondary_click(&h, egui::pos2(500.0, history.top() - 65.0));
    h.run();
    assert!(h.query_by_label("Save this graph as PNG…").is_some());
    assert!(h.query_by_label("Copy this graph as PNG").is_some());
    assert!(h.query_by_label("Close this graph").is_some());
    assert!(
        h.query_by_label("Latency readout").is_none(),
        "Readout must hide behind a context menu"
    );
    h.get_by_label("Back to live").click();
    h.run();
    assert!(h.state().end.is_none() && h.state().pinned.is_none());

    let (mut h, commands) = harness("summary", [1100.0, 720.0]);
    h.run();
    let history = h.get_by_label("History position").rect();
    secondary_click(&h, egui::pos2(500.0, history.top() - 85.0));
    h.run();
    assert!(h.query_by_label("Save graph as PNG…").is_some());
    assert!(h.query_by_label("Copy graph as PNG").is_some());
    assert!(h.query_by_label("Save stretches as CSV…").is_some());
    h.get_by_label("Loss bars in each target's colour").click();
    h.run();
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Preferences(session) if session.settings.loss_by_target))
    );
}

fn secondary_click(h: &Harness<'_, Window>, pos: egui::Pos2) {
    h.hover_at(pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

#[test]
fn graph_context_close_removes_only_the_clicked_graph() {
    let (mut h, _commands) = harness("demo-one", [1440.0, 900.0]);
    let mut snapshot = fixture("demo-one");
    let mut hop = snapshot.plots[0].clone();
    hop.ttl = 1;
    hop.label = "Hop 1".into();
    snapshot.plots.push(hop);
    *h.state()
        .runtime
        .snapshot
        .write()
        .expect("fixture snapshot lock") = Arc::new(snapshot);
    h.state_mut().selected = vec![0, 1];
    h.run();
    let title = h
        .output()
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Round trip" => Some(text.pos),
            _ => None,
        })
        .expect("Latency graph title must be painted");
    secondary_click(&h, title + egui::vec2(150.0, 12.0));
    h.run();
    h.get_by_label("Close this graph").click();
    h.run();
    assert_eq!(h.state().selected, [1]);
}

#[test]
fn row_header_and_tab_context_menus_use_the_existing_actions() {
    let (mut h, commands) = harness("demo-one", [1440.0, 900.0]);
    h.run();
    h.get_by_label("router1.example").click_secondary();
    h.run();
    assert!(h.query_by_label("Copy IP 192.0.2.1").is_some());
    assert!(h.query_by_label("Whois lookup…").is_some());
    assert!(h.query_by_label("Save MTR report…").is_some());
    assert!(h.query_by_label("Copy MTR report").is_some());
    h.get_by_label("Copy name").click();
    h.step();
    assert!(h.output().platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "router1.example")));

    h.get_by_label("IP").click_secondary();
    h.run();
    assert!(h.query_by_label("Move left").is_some());
    assert!(h.query_by_label("Move right").is_some());
    h.get_by_label("MOS").click();
    h.run();
    assert!(commands.try_iter().any(|c| matches!(c, Command::Preferences(session) if !session.hidden_columns.iter().any(|id| id == "mos"))));

    h.get_by_label("IP").click_secondary();
    h.run();
    h.get_by_label("Move right").click();
    h.run();
    assert!(commands.try_iter().any(|c| matches!(c, Command::Preferences(session) if session.column_order.first().is_some_and(|id| id == "name"))));
    h.get_by_label("IP").click_secondary();
    h.run();
    h.get_by_label("Reset to defaults").click();
    h.run();
    assert!(commands.try_iter().any(|c| matches!(c, Command::Preferences(session) if session.column_order.is_empty() && session.column_widths.is_empty() && session.hidden_columns == ["mos"])));

    h.get_by_label("example.com  ICMP").click_secondary();
    h.run();
    assert!(h.query_by_label("Edit probe and alerts…").is_some());
    assert!(h.query_by_label("Restart and erase history…").is_some());
    assert!(h.query_by_label("Close and erase history…").is_some());
    h.query_all_by_label("Pause")
        .last()
        .expect("Pause menu item")
        .click();
    h.run();
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Pause(id) if id == "demo-one"))
    );
}

#[test]
fn stepped_probe_lines_do_not_turn_into_opaque_bands() {
    fn contains_band(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Band(_) => true,
            egui::Shape::Vec(shapes) => shapes.iter().any(contains_band),
            _ => false,
        }
    }
    for active in ["demo-one", "summary"] {
        let (mut h, _commands) = harness(active, [1440.0, 900.0]);
        h.run();
        assert!(
            !h.output()
                .shapes
                .iter()
                .any(|shape| contains_band(&shape.shape)),
            "{active}: render probe steps as thin paths, never opaque bands"
        );
    }
}

#[test]
#[ignore = "requires a working GPU or software Vulkan adapter"]
fn render_native_scenarios() {
    let output = std::env::var_os("TRACEPLOTTER_QA_IMAGES").map_or_else(
        || std::env::temp_dir().join("traceplotter-egui-images"),
        PathBuf::from,
    );
    fs::create_dir_all(&output).unwrap();
    let gpu = egui_kittest::wgpu::create_render_state(
        egui_kittest::wgpu::default_wgpu_setup(),
        eframe::egui_wgpu::RendererOptions::PREDICTABLE,
    );
    export::render_fixture(&gpu, &fixture("demo-one"), &output).unwrap();
    for theme in [Theme::Ink, Theme::Glass] {
        if theme == Theme::Glass {
            let mut snapshot = fixture("demo-one");
            snapshot.session.theme = theme;
            let exports = output.join("glass");
            fs::create_dir_all(&exports).unwrap();
            export::render_fixture(&gpu, &snapshot, &exports).unwrap();
        }
        for (active, name, size) in [
            ("demo-one", "trace", [1100.0, 720.0]),
            ("demo-one", "trace-hover", [1100.0, 720.0]),
            ("demo-one", "trace-menu", [1100.0, 720.0]),
            ("demo-one", "columns-menu", [1440.0, 900.0]),
            ("demo-one", "trace-wide", [1440.0, 900.0]),
            ("demo-one", "trace-small", [640.0, 420.0]),
            ("summary", "summary", [1100.0, 720.0]),
            ("settings", "settings", [1100.0, 720.0]),
            ("settings", "settings-appearance", [1100.0, 720.0]),
            ("settings", "settings-wide", [1440.0, 900.0]),
            ("demo-one", "probe-dialog", [1100.0, 720.0]),
        ] {
            let (mut h, _commands) = themed_harness(active, size, theme);
            if name == "settings-appearance" {
                h.state_mut().settings_page = 1;
            }
            if name == "probe-dialog" {
                h.state_mut().editing = Some((fixture(active).session.targets[0].clone(), false));
            }
            h.run();
            if name == "trace-hover" {
                let history = h.get_by_label("History position").rect();
                h.hover_at(egui::pos2(500.0, history.top() - 70.0));
                h.run();
                assert!(h.query_by_label("Latency readout").is_some());
            }
            if name == "trace-menu" {
                let history = h.get_by_label("History position").rect();
                secondary_click(&h, egui::pos2(500.0, history.top() - 65.0));
                h.run();
                assert!(h.query_by_label("Save this graph as PNG…").is_some());
            }
            if name == "columns-menu" {
                h.get_by_label("IP").click_secondary();
                h.run();
                assert!(h.query_by_label("Reset to defaults").is_some());
            }
            h.render()
                .unwrap()
                .save(output.join(format!(
                    "{}{name}.png",
                    if theme == Theme::Glass { "glass-" } else { "" }
                )))
                .unwrap();
        }
    }
}

#[test]
fn closing_a_tab_asks_in_the_window_before_erasing() {
    let (mut h, commands) = harness("demo-one", [1100.0, 720.0]);
    h.run();
    h.get_by_label("Close example.com").click();
    h.run();
    assert!(commands.try_iter().all(|c| !matches!(c, Command::Close(_))));
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().erasing.is_none());
    h.get_by_label("Close example.com").click();
    h.run();
    h.get_by_label("Close and erase").click();
    h.run();
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Close(id) if id == "demo-one"))
    );
}

#[test]
fn event_list_jumps_to_an_event_and_the_menu_sets_graph_marks() {
    let (mut h, commands) = harness("demo-one", [1100.0, 720.0]);
    let mut snapshot = fixture("demo-one");
    snapshot.traces[0].events = vec![
        serde_json::json!({"t": 1200.0, "kind": "alert", "text": "loss 40%"}),
        serde_json::json!({"t": 1300.0, "kind": "recover", "text": "loss 0%"}),
    ];
    *h.state()
        .runtime
        .snapshot
        .write()
        .expect("fixture snapshot lock") = Arc::new(snapshot);
    h.run();
    h.get_by_label("Events (2)").click();
    h.run();
    h.get_by_label("Show events").click();
    h.run();
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Preferences(s) if s.show_events))
    );
    h.get_by_label("Events (2)").click();
    h.run();
    h.get_by_label("Event list…").click();
    h.run();
    // The incident above the table quotes the same alert; the table row is the last.
    h.query_all_by_label("loss 40%")
        .last()
        .expect("event row")
        .click();
    h.run();
    let selected = h.state().selected_event.clone().expect("event selected");
    assert_eq!(selected["kind"], "alert");
    assert!(
        h.state().end.is_some(),
        "Selecting an event pins the period"
    );
    assert!(
        h.state().events_open,
        "The list stays open for the next event"
    );
    h.get_by_label("Clear event").click();
    h.run();
    assert!(h.state().selected_event.is_none());
    assert!(
        h.state().end.is_some(),
        "Clearing keeps the period where it is"
    );
}

#[test]
fn detailed_columns_show_the_controller_values() {
    let (mut h, _commands) = harness("demo-one", [1440.0, 900.0]);
    let mut snapshot = fixture("demo-one");
    snapshot.session.hidden_columns = vec!["mos".into()];
    snapshot.traces[0].total_extras = Some(logic::Extras {
        median: Some(12.3),
        p95: Some(45.6),
        p99: None,
        longest_lost: 3,
        longest_loss_seconds: 3.0,
    });
    *h.state()
        .runtime
        .snapshot
        .write()
        .expect("fixture snapshot lock") = Arc::new(snapshot);
    h.run();
    assert!(h.query_by_label("12.3").is_some());
    assert!(h.query_by_label("45.6").is_some());
    assert!(h.query_by_label("3 probes · 3.0 s").is_some());
}

#[test]
fn splitter_gives_the_graphs_more_room_and_cannot_be_lost() {
    let (mut h, commands) = harness("demo-one", [1100.0, 720.0]);
    h.run();
    let jitter =
        |h: &Harness<'static, Window>| h.get_by_label("Jitter (ms, running average)").rect().top();
    // Returns where the graphs start while the splitter is still held.
    let drag = |h: &mut Harness<'static, Window>, dy: f32| {
        let at = h.get_by_label("Resize table and graphs").rect().center();
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        for step in 1..=4 {
            h.event(egui::Event::PointerMoved(
                at + egui::vec2(0.0, dy * step as f32 / 4.0),
            ));
            h.run();
        }
        let held = jitter(h);
        h.event(egui::Event::PointerButton {
            pos: at + egui::vec2(0.0, dy),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        held
    };
    let before = jitter(&h);
    let during = drag(&mut h, -80.0);
    assert!(
        during < before - 40.0,
        "Graphs start higher: {before} -> {during}"
    );
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Preferences(s) if s.table_height.is_some()))
    );
    drag(&mut h, -2000.0);
    assert!(
        h.query_by_label("Resize table and graphs").is_some(),
        "The splitter stays reachable"
    );
}

#[test]
fn settings_tab_closes_back_to_the_tab_it_came_from() {
    let (mut h, commands) = harness("demo-one", [1100.0, 720.0]);
    h.run();
    h.get_by_label("Settings").click();
    h.run();
    assert_eq!(h.state().before_settings.as_deref(), Some("demo-one"));
    let tab = h.get_by_label("Settings").rect();
    assert!(
        tab.right() <= 1100.0 - 8.0,
        "Settings tab clear of the edge: {tab:?}"
    );
    // The fixture runtime does not apply the selection itself, as the controller does.
    let mut snapshot = fixture("settings");
    snapshot.session.active = "settings".into();
    *h.state()
        .runtime
        .snapshot
        .write()
        .expect("fixture snapshot lock") = Arc::new(snapshot);
    h.run();
    while commands.try_recv().is_ok() {}
    h.get_by_label("Settings").click();
    h.run();
    assert!(h.state().before_settings.is_none());
    assert!(
        commands
            .try_iter()
            .any(|c| matches!(c, Command::Select(id) if id == "demo-one"))
    );
}

#[test]
fn clicking_into_a_preset_field_opens_its_list() {
    let (mut h, _commands) = harness("demo-one", [1440.0, 900.0]);
    h.run();
    assert!(h.query_by_label("5 seconds").is_none());
    h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Interval")
        .click();
    h.run();
    h.run();
    h.get_by_label("5 seconds").click();
    h.run();
    assert_eq!(h.state().toolbar_interval, 5000);
    h.run();
    assert!(
        h.query_by_label("5 seconds").is_none(),
        "A pick closes the list"
    );
}

#[test]
fn latency_column_shrinks_with_the_window() {
    let (mut h, _commands) = harness("demo-one", [1440.0, 720.0]);
    h.run();
    h.set_size(egui::vec2(1100.0, 720.0));
    h.run();
    // The table fits the narrower window again, so there is nothing to scroll sideways.
    let before = h.get_by_label("Hop").rect().left();
    h.hover_at(egui::pos2(400.0, 300.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(-400.0, 0.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run();
    assert_eq!(h.get_by_label("Hop").rect().left(), before);
}

#[test]
fn clicking_a_cell_text_selects_its_row() {
    let (mut h, _commands) = harness("demo-one", [1100.0, 720.0]);
    h.run();
    h.get_by_label("router3.example").click();
    h.run();
    assert_eq!(h.state().selected, vec![3]);
}

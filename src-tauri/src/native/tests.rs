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
                hops: vec![HopSample {
                    ttl: 1,
                    addr: Some("192.0.2.1".parse().unwrap()),
                    rtt: Some(20.0 + (t % 20) as f64),
                }],
            },
            false,
        );
    }
    let stats = trace.target.stats(1000.0, 1600.0);
    let data = logic::GraphData {
        xs: vec![1000.0, 1200.0, 1400.0, 1600.0],
        ys: vec![Some(20.0), None, Some(50.0), Some(40.0)],
        js: vec![None, None, Some(4.0), Some(3.0)],
        ls: vec![Some(0.0), Some(100.0), Some(20.0), Some(0.0)],
        lo: vec![Some(15.0), None, Some(35.0), Some(38.0)],
        hi: vec![Some(25.0), None, Some(55.0), Some(45.0)],
    };
    Snapshot {
        session: Session {
            active: active.into(),
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
            rows: vec![app::Row {
                ttl: 1,
                addr: "192.0.2.1".into(),
                name: "router.example".into(),
                stats,
            }],
            total: stats,
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
    let (runtime, commands) = Runtime::fixture(fixture(active));
    let h = Harness::builder().with_size(size).build_eframe(|cc| {
        apply_theme(&cc.egui_ctx);
        let mut window = Window::with_runtime(runtime);
        window.placed = true;
        window.window_state = None;
        window
    });
    (h, commands)
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
    h.get_by_label("Live").click();
    h.run();
    assert!(h.state().end.is_none());
}
#[test]
fn settings_and_minimum_window_are_accessible() {
    let (mut h, _commands) = harness("settings", [640.0, 420.0]);
    h.run();
    assert!(h.query_all_by_label("Packet type").next().is_some());
    h.get_by_label("All targets").click();
    h.run();
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
    for (active, name, size) in [
        ("demo-one", "trace", [1100.0, 720.0]),
        ("demo-one", "trace-small", [640.0, 420.0]),
        ("settings", "settings", [1100.0, 720.0]),
    ] {
        let (mut h, _commands) = harness(active, size);
        h.run();
        h.render()
            .unwrap()
            .save(output.join(format!("{name}.png")))
            .unwrap();
    }
}

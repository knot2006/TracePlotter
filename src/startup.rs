//! Opt-in elapsed startup timings, without writing to the user's data directory.
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Option<Instant>> = OnceLock::new();

pub(super) fn mark(stage: &str) {
    if let Some(start) = START
        .get_or_init(|| std::env::var_os("TRACEPLOTTER_STARTUP_TIMINGS").map(|_| Instant::now()))
    {
        eprintln!(
            "Startup +{:.1} ms: {stage}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

//! Opt-in timing log for measurements (docs/PERFORMANCE.md): with `MARKDOWNMATE_TIMING` set,
//! milestones go to stderr as `markdownmate-timing: <ms since main started> <what>`. Unset (the
//! normal case) it only checks a flag.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Option<Instant>> = OnceLock::new();

fn start() -> Option<Instant> {
    *START.get_or_init(|| std::env::var_os("MARKDOWNMATE_TIMING").map(|_| Instant::now()))
}

/// Starts the clock; call first thing in `main`.
pub fn init() {
    start();
}

/// Whether the log is on.
pub fn enabled() -> bool {
    start().is_some()
}

/// Logs `what` (built only when the log is on).
pub fn mark(what: impl FnOnce() -> String) {
    if let Some(start) = start() {
        eprintln!(
            "markdownmate-timing: {:.0} {}",
            start.elapsed().as_secs_f64() * 1000.0,
            what()
        );
    }
}

/// Logs `what` with the time since `since`, e.g. `tags indexed in 120 ms`.
pub fn took(since: Instant, what: &str) {
    mark(|| format!("{what} in {:.0} ms", since.elapsed().as_secs_f64() * 1000.0));
}

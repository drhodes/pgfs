//! On-demand CPU profiling, gated behind the `profiling` feature flag.
//!
//! The signal-waiter thread (see main.rs) calls [`toggle`] on SIGUSR2. The
//! first call starts a `pprof` CPU sampler at 997 Hz; the second stops it
//! and renders two files under /tmp:
//!
//! - `/tmp/pgfs-profile-{ts}.svg`    — inferno flamegraph
//! - `/tmp/pgfs-profile-{ts}.stacks` — readable per-stack sample dump
//!
//! With the feature disabled the module compiles to a logging stub, so a
//! default build contains zero profiling instructions in the hot path.
//! The profiler guard lives here, on the signal-waiter thread — never in a
//! signal handler.

use std::path::PathBuf;

/// The active sampler, when profiling is running. Owned by the signal-waiter
/// thread (the only caller of [`toggle`]), which is a background thread per
/// the Profiling context: signal dispositions only ever set a flag.
#[cfg(feature = "profiling")]
static PROFILER: std::sync::Mutex<Option<pprof::ProfilerGuard<'static>>> =
    std::sync::Mutex::new(None);

/// Toggle CPU profiling. Returns `Some(path)` when a profile was stopped and
/// rendered (the SVG path), `None` when the profiler was started or not
/// compiled in.
#[cfg(feature = "profiling")]
pub fn toggle() -> Option<PathBuf> {
    let mut slot = PROFILER.lock().unwrap_or_else(|p| p.into_inner());
    if slot.is_none() {
        match pprof::ProfilerGuardBuilder::default()
            .frequency(997)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
        {
            Ok(guard) => {
                tracing::info!("CPU profiling started (997 Hz)");
                *slot = Some(guard);
                None
            }
            Err(e) => {
                tracing::error!("failed to start CPU profiler: {e}");
                None
            }
        }
    } else {
        let guard = slot.take().expect("just checked is_some");
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let svg_path = PathBuf::from(format!("/tmp/pgfs-profile-{ts}.svg"));
        let stacks_path = PathBuf::from(format!("/tmp/pgfs-profile-{ts}.stacks"));
        match guard.report().build() {
            Ok(report) => {
                // Flamegraph for humans/browsers.
                if let Ok(mut w) = std::fs::File::create(&svg_path) {
                    if let Err(e) = report.flamegraph(&mut w) {
                        tracing::warn!("flamegraph render failed: {e}");
                    }
                } else {
                    tracing::warn!("could not open {} for writing", svg_path.display());
                }
                // Per-stack sample dump for machines (and quick greps).
                if std::fs::write(&stacks_path, format!("{report:?}")).is_err() {
                    tracing::warn!("could not write {}", stacks_path.display());
                }
                tracing::info!(
                    "CPU profiling stopped; wrote {} and {}",
                    svg_path.display(),
                    stacks_path.display()
                );
                Some(svg_path)
            }
            Err(e) => {
                tracing::error!("failed to build profile report: {e}");
                None
            }
        }
    }
}

/// Stub build: no profiling code compiled in.
#[cfg(not(feature = "profiling"))]
pub fn toggle() -> Option<PathBuf> {
    tracing::info!("CPU profiling not compiled in — rebuild with --features profiling");
    None
}

#[cfg(all(test, feature = "profiling"))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn toggle_starts_and_stops() {
        // Clean slate (tests share the process).
        *PROFILER.lock().unwrap() = None;

        let started = toggle();
        assert!(started.is_none(), "first toggle starts the profiler");

        // Let the 997 Hz sampler accumulate a few samples of this thread.
        let t0 = Instant::now();
        let mut acc: u64 = 0;
        while t0.elapsed() < Duration::from_millis(50) {
            acc = acc.wrapping_add(1);
            std::hint::spin_loop();
        }
        std::hint::black_box(acc);

        let stopped = toggle().expect("second toggle stops and returns a path");
        assert!(
            stopped.to_string_lossy().starts_with("/tmp/pgfs-profile-"),
            "unexpected path: {stopped:?}"
        );
        assert!(stopped.exists(), "flamegraph file should exist on disk");

        let stacks = PathBuf::from(stopped.to_string_lossy().replace(".svg", ".stacks"));
        assert!(stacks.exists(), "stacks dump should exist on disk");
        assert!(
            PROFILER.lock().unwrap().is_none(),
            "profiler should be stopped after second toggle"
        );
    }
}

#[cfg(all(test, not(feature = "profiling")))]
mod tests {
    use super::*;

    #[test]
    fn toggle_is_noop_without_feature() {
        assert!(toggle().is_none());
    }
}

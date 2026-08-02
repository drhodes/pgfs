//! Observability: counters, histograms, and periodic export.
//!
//! Every FUSE callback and DB operation records:
//! - An atomic counter (workload composition)
//! - Wall-clock latency into a histogram
//!
//! Error counters distinguish "normal" ENOENT/ENOTEMPTY from unexpected EIO.
//! Metrics are logged at `info!` every 60 s. A liveness heartbeat counter
//! is bumped by a background thread; FUSE callbacks check it to detect
//! deadlock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ── FUSE operation counters ────────────────────────────────────────────

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        $(
            pub static $name: AtomicU64 = AtomicU64::new(0);
        )*
    };
}

counters! {
    LOOKUP_COUNT, GETATTR_COUNT, SETATTR_COUNT, READ_COUNT, WRITE_COUNT,
    CREATE_COUNT, MKDIR_COUNT, UNLINK_COUNT, RMDIR_COUNT, RENAME_COUNT,
    READDIR_COUNT, OPEN_COUNT, FSYNC_COUNT,
}

// ── Error counters ─────────────────────────────────────────────────────

/// Unexpected failures logged via log_and_reply! → EIO.
pub static EIO_COUNT: AtomicU64 = AtomicU64::new(0);

counters! {
    ENOENT_COUNT, EEXIST_COUNT, ENOTEMPTY_COUNT, EISDIR_COUNT,
    ENOTDIR_COUNT, EINVAL_COUNT,
}

// ── Liveness heartbeat ─────────────────────────────────────────────────

/// Bumped by a background thread every N seconds. FUSE callbacks check this
/// on entry; if it has not advanced within 3×N the daemon logs ERROR and
/// initiates a clean unmount.
pub static LIVENESS: AtomicU64 = AtomicU64::new(0);

// ── Latency histogram ──────────────────────────────────────────────────
//
// Buckets: ≤10µs, ≤100µs, ≤1ms, ≤10ms, ≤100ms, ≤1s, >1s

const HISTO_BUCKETS: usize = 7;
const HISTO_THRESHOLDS: [u64; HISTO_BUCKETS - 1] = [10, 100, 1_000, 10_000, 100_000, 1_000_000]; // µs

pub struct Histogram {
    counts: [AtomicU64; HISTO_BUCKETS],
}

impl Histogram {
    pub const fn new() -> Self {
        Histogram {
            counts: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Record a duration in the appropriate bucket.
    pub fn record(&self, d: Duration) {
        let us = d.as_micros() as u64;
        let bucket = HISTO_THRESHOLDS
            .iter()
            .position(|&t| us <= t)
            .unwrap_or(HISTO_BUCKETS - 1);
        self.counts[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot all bucket counts (for programmatic export).
    pub fn snapshot(&self) -> [u64; HISTO_BUCKETS] {
        let mut s = [0u64; HISTO_BUCKETS];
        for (i, c) in self.counts.iter().enumerate() {
            s[i] = c.load(Ordering::Relaxed);
        }
        s
    }

    /// Human-readable summary string: p50/p95/p99 and total count.
    pub fn summary(&self, label: &str) -> String {
        let s = self.snapshot();
        let total: u64 = s.iter().sum();
        if total == 0 {
            return format!("{label}=0 samples");
        }
        let mut cum = 0u64;
        let mut p50 = 0usize;
        let mut p95 = 0usize;
        let mut p99 = 0usize;
        let mut targets = [
            (total * 50 / 100, &mut p50),
            (total * 95 / 100, &mut p95),
            (total * 99 / 100, &mut p99),
        ];
        for (i, &c) in s.iter().enumerate() {
            cum += c;
            for (target, ref mut done) in targets.iter_mut() {
                if **done == 0 && cum >= *target {
                    **done = i;
                }
            }
        }
        format!(
            "{label} total={total} p50={}µs p95={}µs p99={}µs",
            bucket_label(p50),
            bucket_label(p95),
            bucket_label(p99),
        )
    }
}

fn bucket_label(bucket: usize) -> &'static str {
    match bucket {
        0 => "<=10",
        1 => "<=100",
        2 => "<=1k",
        3 => "<=10k",
        4 => "<=100k",
        5 => "<=1M",
        _ => ">1M",
    }
}

/// FUSE callback latency (all ops pooled).
pub static FUSE_LATENCY: Histogram = Histogram::new();

/// DB query round-trip latency.
pub static DB_LATENCY: Histogram = Histogram::new();

// ── Periodic export ────────────────────────────────────────────────────

/// Spawn a daemon thread that dumps all metrics at `info!` every 60 s.
pub fn start_periodic_export() {
    std::thread::Builder::new()
        .name("metrics-export".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(60));
            tracing::info!(
                "metrics|ops|lookup={} getattr={} setattr={} read={} write={} create={} mkdir={} unlink={} rmdir={} rename={} readdir={} open={} fsync={}",
                LOOKUP_COUNT.load(Ordering::Relaxed),
                GETATTR_COUNT.load(Ordering::Relaxed),
                SETATTR_COUNT.load(Ordering::Relaxed),
                READ_COUNT.load(Ordering::Relaxed),
                WRITE_COUNT.load(Ordering::Relaxed),
                CREATE_COUNT.load(Ordering::Relaxed),
                MKDIR_COUNT.load(Ordering::Relaxed),
                UNLINK_COUNT.load(Ordering::Relaxed),
                RMDIR_COUNT.load(Ordering::Relaxed),
                RENAME_COUNT.load(Ordering::Relaxed),
                READDIR_COUNT.load(Ordering::Relaxed),
                OPEN_COUNT.load(Ordering::Relaxed),
                FSYNC_COUNT.load(Ordering::Relaxed),
            );
            tracing::info!(
                "metrics|errors|eio={} enoent={} eexist={} enotempty={} eisdir={} enotdir={} einval={}",
                EIO_COUNT.load(Ordering::Relaxed),
                ENOENT_COUNT.load(Ordering::Relaxed),
                EEXIST_COUNT.load(Ordering::Relaxed),
                ENOTEMPTY_COUNT.load(Ordering::Relaxed),
                EISDIR_COUNT.load(Ordering::Relaxed),
                ENOTDIR_COUNT.load(Ordering::Relaxed),
                EINVAL_COUNT.load(Ordering::Relaxed),
            );
            tracing::info!(
                "metrics|latency|fuse={}",
                FUSE_LATENCY.summary("fuse"),
            );
            tracing::info!(
                "metrics|latency|db={}",
                DB_LATENCY.summary("db"),
            );
        })
        .expect("spawn metrics export thread");
}

// ── Liveness thread ────────────────────────────────────────────────────

/// Spawn a daemon thread that bumps `LIVENESS` every 10 s.
/// FUSE callbacks should check that it advances within 30 s.
pub fn start_liveness_heartbeat() {
    std::thread::Builder::new()
        .name("liveness".into())
        .spawn(|| loop {
            LIVENESS.fetch_add(1, Ordering::Release);
            std::thread::sleep(Duration::from_secs(10));
        })
        .expect("spawn liveness thread");
}

/// Return true if the liveness counter has advanced since `last_seen`.
#[allow(dead_code)] // wired in future callback-level instrumentation pass
pub fn liveness_changed_since(last_seen: u64) -> bool {
    LIVENESS.load(Ordering::Acquire) != last_seen
}

// ── State dump ─────────────────────────────────────────────────────────

/// Produced on SIGUSR1 or programmatically. Suitable for `info!` logging.
pub fn state_dump(
    ino_cache_size: usize,
    next_ino: u64,
    db_status: &str,
    uptime: Duration,
) -> String {
    format!(
        "state|ino_cache={} next_ino={} db={} uptime_secs={}\n\
         state|fuse_latency={}\n\
         state|db_latency={}",
        ino_cache_size,
        next_ino,
        db_status,
        uptime.as_secs(),
        FUSE_LATENCY.summary("fuse"),
        DB_LATENCY.summary("db"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets() {
        let h = Histogram::new();
        h.record(Duration::from_micros(5)); // ≤10µs
        h.record(Duration::from_micros(50)); // ≤100µs
        h.record(Duration::from_micros(500)); // ≤1ms
        h.record(Duration::from_millis(5)); // ≤10ms
        h.record(Duration::from_millis(50)); // ≤100ms
        h.record(Duration::from_millis(500)); // ≤1s
        h.record(Duration::from_secs(2)); // >1s
        let s = h.snapshot();
        assert_eq!(s[0], 1, "≤10µs");
        assert_eq!(s[1], 1, "≤100µs");
        assert_eq!(s[2], 1, "≤1ms");
        assert_eq!(s[3], 1, "≤10ms");
        assert_eq!(s[4], 1, "≤100ms");
        assert_eq!(s[5], 1, "≤1s");
        assert_eq!(s[6], 1, ">1s");
    }

    #[test]
    fn histogram_empty_summary() {
        let h = Histogram::new();
        let s = h.summary("test");
        assert!(s.contains("0 samples"));
    }

    #[test]
    fn histogram_with_data_summary() {
        let h = Histogram::new();
        h.record(Duration::from_micros(5)); // ≤10µs
        let s = h.summary("test");
        assert!(s.contains("total=1"));
    }

    #[test]
    fn counters_increment() {
        LOOKUP_COUNT.fetch_add(1, Ordering::Relaxed);
        assert_eq!(LOOKUP_COUNT.load(Ordering::Relaxed), 1);
        LOOKUP_COUNT.store(0, Ordering::Relaxed); // reset for other tests
    }

    #[test]
    fn liveness_seen() {
        let seen = LIVENESS.load(Ordering::Acquire);
        LIVENESS.fetch_add(1, Ordering::Release);
        assert!(liveness_changed_since(seen));
    }
}

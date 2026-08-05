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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

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

// ── Replica routing counters ──────────────────────────────────────────

/// Reads served from the replica standby (see spec/replica.py).
pub static REPLICA_READ_COUNT: AtomicU64 = AtomicU64::new(0);

/// Reads that fell back to the primary because the standby was unreachable
/// or had not caught up with the primary's WAL. Only incremented when a
/// replica is configured (a single-node mount has nothing to fall back
/// from). A rising count means replication is struggling.
pub static REPLICA_FALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Set once the first fallback has been reported at WARN (rate limit so
/// the log doesn't spam while a standby is down).
pub static REPLICA_FALLBACK_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// ── Liveness heartbeat ─────────────────────────────────────────────────

/// Bumped by a background thread every N seconds. FUSE callbacks check this
/// on entry (see [`check_liveness`]); if it has not advanced within 3×N the
/// daemon logs ERROR, flags [`DEADLOCK_DETECTED`], and a watchdog thread in
/// main.rs performs the clean unmount.
pub static LIVENESS: AtomicU64 = AtomicU64::new(0);

/// Heartbeat cadence (N), and the stall deadline (3×N), in milliseconds.
const HEARTBEAT_PERIOD_MS: u64 = 10_000;
const HEARTBEAT_DEADLINE_MS: u64 = 3 * HEARTBEAT_PERIOD_MS;

/// The LIVENESS value the last callback observed, and the monotonic
/// deadline by which the next beat must arrive (millis since process
/// boot). Used by [`check_liveness`].
///
/// Both are monotonic (see [`now_mono_millis`]) so a wall-clock jump (NTP
/// step, manual `date`) can never cause a spurious "deadlock": the whole
/// point of this check is to avoid false positives on an idle mount.
static LAST_BEAT_SEEN: AtomicU64 = AtomicU64::new(u64::MAX);
static BEAT_DEADLINE_AT: AtomicU64 = AtomicU64::new(u64::MAX);

/// Set by a FUSE callback that caught a stalled heartbeat; the watchdog
/// thread in main.rs polls this and performs the clean unmount.
pub static DEADLOCK_DETECTED: AtomicBool = AtomicBool::new(false);

/// Call at the top of every FUSE callback. Returns true while the liveness
/// heartbeat is advancing; false once the counter has not advanced within
/// 3×N seconds (N = 10 s), i.e. the daemon is wedged. The first call
/// always returns true — it only establishes the baseline.
pub fn check_liveness() -> bool {
    let now_ms = now_mono_millis();
    let counter = LIVENESS.load(Ordering::Acquire);
    let last_counter = LAST_BEAT_SEEN.load(Ordering::Relaxed);
    if last_counter == u64::MAX || counter != last_counter {
        // First observation, or the heartbeat advanced since the last
        // callback: healthy. Arm a fresh deadline 3×N out.
        LAST_BEAT_SEEN.store(counter, Ordering::Relaxed);
        BEAT_DEADLINE_AT.store(now_ms + HEARTBEAT_DEADLINE_MS, Ordering::Relaxed);
        true
    } else {
        // Counter unchanged since the last callback. Still healthy while
        // the deadline (3×N after the last observed beat) has not passed;
        // past it, the heartbeat is wedged.
        now_ms < BEAT_DEADLINE_AT.load(Ordering::Relaxed)
    }
}

/// Monotonic millis since process boot. Immune to wall-clock jumps, unlike
/// `SystemTime::now()` — critical for a check whose job is avoiding false
/// positives.
fn now_mono_millis() -> u64 {
    static BOOT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    BOOT.get_or_init(Instant::now).elapsed().as_millis() as u64
}

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

/// RAII guard: records the enclosing FUSE callback's wall-clock duration
/// into [`FUSE_LATENCY`] when it drops — i.e. on every exit path of the
/// callback (happy path, early return, `log_and_reply!`, ...). Created at
/// the top of every `Filesystem` method (spec/observe.py Metrics #2).
pub struct FuseLatencyGuard(Instant);

impl FuseLatencyGuard {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Drop for FuseLatencyGuard {
    fn drop(&mut self) {
        FUSE_LATENCY.record(self.0.elapsed());
    }
}

/// DB query round-trip latency.
pub static DB_LATENCY: Histogram = Histogram::new();

// ── Periodic export ────────────────────────────────────────────────────

/// Spawn a daemon thread that dumps all metrics at `info!` every 60 s.
pub fn start_periodic_export() {
    std::thread::Builder::new()
        .name("metrics-export".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(60));
            // Re-arm the replica-fallback WARN so the first fallback of
            // every period is reported (spec/replica.py FallsBackToPrimary:
            // "The first fallback per period is logged at WARN").
            REPLICA_FALLBACK_WARNED.store(false, Ordering::Relaxed);
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
                "metrics|replica|reads={} fallbacks={}",
                REPLICA_READ_COUNT.load(Ordering::Relaxed),
                REPLICA_FALLBACK_COUNT.load(Ordering::Relaxed),
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
pub fn start_liveness_heartbeat() {
    std::thread::Builder::new()
        .name("liveness".into())
        .spawn(|| loop {
            LIVENESS.fetch_add(1, Ordering::Release);
            std::thread::sleep(Duration::from_secs(10));
        })
        .expect("spawn liveness thread");
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
    fn liveness_check_detects_stall() {
        // NOTE: these tests mutate process-global statics (LIVENESS,
        // BEAT_DEADLINE_AT, FUSE_LATENCY). They are only safe because no
        // other test touches the same statics concurrently. If a future
        // test calls check_liveness() or records FUSE_LATENCY, it must also
        // snapshot/reset these statics.
        // First observation initializes the baseline and reports healthy.
        assert!(check_liveness());
        // A beat arrives: still healthy.
        LIVENESS.fetch_add(1, Ordering::Release);
        assert!(check_liveness());
        // No beat for >3×N: deadlocked. Forge a deadline that has already
        // passed (0 = start of the process's monotonic clock) — this works
        // regardless of how long the test process has been alive.
        BEAT_DEADLINE_AT.store(0, Ordering::Relaxed);
        assert!(!check_liveness());
        // Recovery: the next beat arms a fresh deadline.
        LIVENESS.fetch_add(1, Ordering::Release);
        assert!(check_liveness());
    }

    #[test]
    fn latency_guard_records_one_sample() {
        let before = FUSE_LATENCY.snapshot();
        {
            let _guard = FuseLatencyGuard::new();
            std::thread::sleep(Duration::from_micros(1));
        }
        let after = FUSE_LATENCY.snapshot();
        let delta: u64 = after.iter().zip(before.iter()).map(|(a, b)| a - b).sum();
        assert_eq!(delta, 1, "guard must record exactly one latency sample");
    }
}

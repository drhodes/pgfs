//! Counters, histograms and gauges (spec/observe.py Metrics), exported
//! as nine INFO lines every 60 s (spec/app.py ExportsPeriodicMetrics).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Per-callback operation counters, in export order.
pub const OPS: &[&str] = &[
    "lookup",
    "getattr",
    "setattr",
    "readlink",
    "mknod",
    "mkdir",
    "unlink",
    "rmdir",
    "symlink",
    "rename",
    "link",
    "open",
    "read",
    "write",
    "flush",
    "release",
    "fsync",
    "readdir",
    "readdirplus",
    "fsyncdir",
    "statfs",
    "setxattr",
    "getxattr",
    "listxattr",
    "removexattr",
    "create",
    "fallocate",
    "lseek",
    "copy_file_range",
    "forget",
];

static OP_COUNTS: [AtomicU64; 30] = [const { AtomicU64::new(0) }; 30];

pub fn count_op(op: &str) {
    if let Some(i) = OPS.iter().position(|o| *o == op) {
        OP_COUNTS[i].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn op_count(op: &str) -> u64 {
    OPS.iter()
        .position(|o| *o == op)
        .map(|i| OP_COUNTS[i].load(Ordering::Relaxed))
        .unwrap_or(0)
}

static ERRNO_COUNTS: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];

/// Count an errno where it is replied (spec/app.py ExportsPeriodicMetrics:
/// none stays at zero by omission).
pub fn count_errno(errno: i32) {
    if let Some(i) = crate::errno::CATALOGUE
        .iter()
        .position(|c| c.errno == errno)
    {
        ERRNO_COUNTS[i].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn errno_count(errno: i32) -> u64 {
    crate::errno::CATALOGUE
        .iter()
        .position(|c| c.errno == errno)
        .map(|i| ERRNO_COUNTS[i].load(Ordering::Relaxed))
        .unwrap_or(0)
}

pub static REPLICA_READ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static REPLICA_FALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);
pub static REPLICA_FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);
pub static REPLICA_LAG_BYTES: AtomicU64 = AtomicU64::new(0);
pub static REPLICA_LAG_MS: AtomicU64 = AtomicU64::new(0);
pub static REPLICA_TICKET_WAITS: AtomicU64 = AtomicU64::new(0);

/// Database connection state of the mount (spec/failure.py DatabaseStateMachine).
pub static DB_STATE: AtomicU8 = AtomicU8::new(0);
pub static RECONNECTS: AtomicU64 = AtomicU64::new(0);
pub static FENCED: AtomicU64 = AtomicU64::new(0);
pub static SLOW_DB: AtomicBool = AtomicBool::new(false);
pub static OLDEST_INFLIGHT_MS: AtomicU64 = AtomicU64::new(0);
pub static CHECKSUM_FAILURES: AtomicU64 = AtomicU64::new(0);
pub static USER_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);

/// The `metrics|space|…` line, refreshed by housekeeping (spec/space.py ReportsSpace).
pub static SPACE_LINE: Mutex<String> = Mutex::new(String::new());

const HISTO_BUCKETS: usize = 7;
const HISTO_THRESHOLDS: [u64; HISTO_BUCKETS - 1] = [10, 100, 1_000, 10_000, 100_000, 1_000_000]; // µs

/// A latency histogram with fixed buckets at ≤10µs … >1s.
pub struct Histogram {
    counts: [AtomicU64; HISTO_BUCKETS],
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    pub const fn new() -> Self {
        Histogram {
            counts: [const { AtomicU64::new(0) }; HISTO_BUCKETS],
        }
    }

    pub fn record(&self, d: Duration) {
        let us = d.as_micros() as u64;
        let bucket = HISTO_THRESHOLDS
            .iter()
            .position(|&t| us <= t)
            .unwrap_or(HISTO_BUCKETS - 1);
        self.counts[bucket].fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> [u64; HISTO_BUCKETS] {
        let mut s = [0u64; HISTO_BUCKETS];
        for (i, c) in self.counts.iter().enumerate() {
            s[i] = c.load(Ordering::Relaxed);
        }
        s
    }

    pub fn summary(&self, label: &str) -> String {
        summarise(label, &self.snapshot())
    }
}

/// The bucket index holding percentile `pct` of `s`.
fn percentile_bucket(s: &[u64; HISTO_BUCKETS], pct: u64) -> Option<usize> {
    let total: u64 = s.iter().sum();
    if total == 0 {
        return None;
    }
    let target = (total * pct).div_ceil(100).max(1);
    let mut cum = 0;
    for (i, &c) in s.iter().enumerate() {
        cum += c;
        if cum >= target {
            return Some(i);
        }
    }
    Some(HISTO_BUCKETS - 1)
}

fn summarise(label: &str, s: &[u64; HISTO_BUCKETS]) -> String {
    let total: u64 = s.iter().sum();
    if total == 0 {
        return format!("{label}=0 samples");
    }
    let b = |p| bucket_label(percentile_bucket(s, p).unwrap_or(0));
    format!(
        "{label} total={total} p50={}µs p95={}µs p99={}µs",
        b(50),
        b(95),
        b(99)
    )
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

pub static FUSE_LATENCY: Histogram = Histogram::new();
pub static DB_LATENCY: Histogram = Histogram::new();

/// Records a FUSE callback's wall-clock time when dropped.
pub struct FuseLatencyGuard(std::time::Instant);

impl Default for FuseLatencyGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl FuseLatencyGuard {
    pub fn new() -> Self {
        Self(std::time::Instant::now())
    }
}

impl Drop for FuseLatencyGuard {
    fn drop(&mut self) {
        FUSE_LATENCY.record(self.0.elapsed());
    }
}

// ── Slow database signal (spec/failure.py SlowDatabaseSignal) ───────────

#[derive(Default)]
struct StatementWindow {
    current: [u64; HISTO_BUCKETS],
    /// The previous 60 one-minute histograms.
    history: Vec<[u64; HISTO_BUCKETS]>,
    slow_minutes: u32,
}

static STATEMENTS: Mutex<Option<HashMap<&'static str, StatementWindow>>> = Mutex::new(None);

pub fn record_statement_latency(name: &'static str, d: Duration) {
    let us = d.as_micros() as u64;
    let bucket = HISTO_THRESHOLDS
        .iter()
        .position(|&t| us <= t)
        .unwrap_or(HISTO_BUCKETS - 1);
    if let Ok(mut g) = STATEMENTS.try_lock() {
        g.get_or_insert_with(HashMap::new)
            .entry(name)
            .or_default()
            .current[bucket] += 1;
    }
}

/// Rotate the per-statement windows (once a minute) and update the
/// slow_db gauge: set when a statement's last-minute p99 stays above 4×
/// its previous-hour p99 for 5 consecutive minutes.
pub fn rotate_statement_windows() {
    let mut warn = Vec::new();
    let mut any_slow = false;
    if let Ok(mut g) = STATEMENTS.lock() {
        for (name, w) in g.get_or_insert_with(HashMap::new).iter_mut() {
            let mut base = [0u64; HISTO_BUCKETS];
            for h in &w.history {
                for i in 0..HISTO_BUCKETS {
                    base[i] += h[i];
                }
            }
            let cur = percentile_bucket(&w.current, 99);
            let basep = percentile_bucket(&base, 99);
            let slow = matches!((cur, basep), (Some(c), Some(b)) if w.history.len() >= 5 && c > b && bucket_upper_us(c) > 4 * bucket_upper_us(b));
            if slow {
                w.slow_minutes += 1;
            } else {
                w.slow_minutes = 0;
            }
            if w.slow_minutes >= 5 {
                any_slow = true;
                if w.slow_minutes == 5 {
                    warn.push(format!(
                        "{name}: p99 {}µs over the last minute vs {}µs over the previous hour",
                        bucket_label(cur.unwrap_or(0)),
                        bucket_label(basep.unwrap_or(0))
                    ));
                }
            }
            w.history.push(w.current);
            if w.history.len() > 60 {
                w.history.remove(0);
            }
            w.current = [0; HISTO_BUCKETS];
        }
    }
    for line in warn {
        tracing::warn!("slow database: {line} (threshold 4x for 5 minutes)");
    }
    let was = SLOW_DB.swap(any_slow, Ordering::Relaxed);
    if was && !any_slow {
        tracing::info!("slow database: latency recovered");
    }
}

fn bucket_upper_us(b: usize) -> u64 {
    HISTO_THRESHOLDS.get(b).copied().unwrap_or(10_000_000)
}

pub fn db_state_name(s: u8) -> &'static str {
    match s {
        0 => "connected",
        1 => "broken",
        2 => "reconnecting",
        3 => "revalidating",
        4 => "fenced",
        _ => "unknown",
    }
}

/// The nine metrics lines.
pub fn export_lines() -> Vec<String> {
    let ops: Vec<String> = OPS
        .iter()
        .enumerate()
        .map(|(i, o)| format!("{o}={}", OP_COUNTS[i].load(Ordering::Relaxed)))
        .collect();
    let errs: Vec<String> = crate::errno::CATALOGUE
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}={}",
                c.name.to_lowercase(),
                ERRNO_COUNTS[i].load(Ordering::Relaxed)
            )
        })
        .collect();
    let space = SPACE_LINE.lock().map(|s| s.clone()).unwrap_or_default();
    vec![
        format!("metrics|ops|{}", ops.join(" ")),
        format!("metrics|errors|{}", errs.join(" ")),
        format!(
            "metrics|replica|reads={} fallbacks={} lag_bytes={} lag_ms={} ticket_waits={}",
            REPLICA_READ_COUNT.load(Ordering::Relaxed),
            REPLICA_FALLBACK_COUNT.load(Ordering::Relaxed),
            REPLICA_LAG_BYTES.load(Ordering::Relaxed),
            REPLICA_LAG_MS.load(Ordering::Relaxed),
            REPLICA_TICKET_WAITS.load(Ordering::Relaxed),
        ),
        format!("metrics|latency|fuse={}", FUSE_LATENCY.summary("fuse")),
        format!("metrics|latency|db={}", DB_LATENCY.summary("db")),
        format!(
            "metrics|db|state={} reconnects={} fenced={} slow_db={}",
            db_state_name(DB_STATE.load(Ordering::Relaxed)),
            RECONNECTS.load(Ordering::Relaxed),
            FENCED.load(Ordering::Relaxed),
            SLOW_DB.load(Ordering::Relaxed),
        ),
        format!(
            "metrics|inflight|oldest_ms={}",
            OLDEST_INFLIGHT_MS.load(Ordering::Relaxed)
        ),
        format!(
            "metrics|space|{}",
            if space.is_empty() {
                "pending".to_string()
            } else {
                space
            }
        ),
        format!(
            "metrics|integrity|checksum_failures={}",
            CHECKSUM_FAILURES.load(Ordering::Relaxed)
        ),
    ]
}

pub fn start_periodic_export() {
    std::thread::Builder::new()
        .name("metrics-export".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(60));
            REPLICA_FALLBACK_WARNED.store(false, Ordering::Relaxed);
            rotate_statement_windows();
            for line in export_lines() {
                tracing::info!("{line}");
            }
        })
        .expect("spawn metrics export thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_and_summary() {
        let h = Histogram::new();
        for _ in 0..98 {
            h.record(Duration::from_micros(5));
        }
        h.record(Duration::from_millis(5));
        h.record(Duration::from_secs(2));
        let s = h.summary("x");
        assert!(s.contains("total=100"), "{s}");
        assert!(s.contains("p50=<=10µs"), "{s}");
        assert!(s.contains("p99=<=10kµs"), "{s}");
    }

    #[test]
    fn export_has_nine_lines_and_every_errno() {
        count_errno(libc::EROFS);
        let lines = export_lines();
        assert_eq!(lines.len(), 9);
        assert!(lines[1].contains("erofs="));
        assert!(lines[1].contains("etimedout="));
        assert!(errno_count(libc::EROFS) >= 1);
    }

    #[test]
    fn ops_are_counted() {
        count_op("symlink");
        assert!(op_count("symlink") >= 1);
        assert_eq!(OPS.len(), OP_COUNTS.len());
    }
}

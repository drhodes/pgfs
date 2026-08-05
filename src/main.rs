mod db;
mod error;
mod fs;
mod metrics;
mod profiling;

use clap::Parser;
use db::Db;
use error::Result;
use fs::PgFs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing_subscriber::prelude::*;

/// pgfs — mount a directory tree of files backed by a Postgres table.
///
/// This is the smallest useful version: arbitrary subdirectories, whole
/// files stored as blobs, no chunked blocks, no search. It exists to prove
/// the read/write path end to end before any of that gets layered on.
#[derive(Parser)]
struct Args {
    /// Where to mount the filesystem, e.g. ./testdata/mnt
    mountpoint: String,

    /// libpq-style connection string. Defaults to a Unix socket inside
    /// this project's testdata directory -- never a system-wide Postgres
    /// install or data directory. See scripts/init_db.sh to set that up.
    #[arg(long, default_value_t = format!("host={}/testdata dbname=pgfs", std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()))]
    conn: String,

    /// Optional libpq-style connection string for a physical streaming
    /// standby (see spec/replica.py). When set, reads (getattr/list/read)
    /// are served from the standby once it has caught up with the primary's
    /// WAL; writes always go to the primary. A stale or unreachable standby
    /// silently falls back to the primary. Provision the standby with
    /// scripts/replica_db.sh.
    #[arg(long)]
    replica: Option<String>,
}

fn main() {
    // Initialise the tracing subscriber before any other work.
    // RUST_LOG controls filter levels; RUST_LOG_FORMAT=json for JSON output.
    // Under the `profiling` feature a tracing-chrome span layer is installed
    // too (see init_tracing); its guard is kept alive for the whole process
    // so the trace file is written on clean shutdown.
    #[cfg(feature = "profiling")]
    let _chrome_guard = init_tracing();
    #[cfg(not(feature = "profiling"))]
    init_tracing();

    // Install a panic hook to emit a readable "story" including the
    // call-site (file:line), panic payload, and a backtrace so failures
    // are actionable without digging through logs.  In `--release`
    // `panic = "abort"` prevents a broken daemon from lingering after a
    // panic; the report is written before abort.
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown:0".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string payload>".to_string());
        let bt = std::backtrace::Backtrace::capture();
        // OS/Rust/environment metadata so the report stands alone
        // (spec/app.py ReportsCrashes, spec/observe.py CrashReports #1).
        let version = option_env!("CARGO_PKG_VERSION").unwrap_or("unknown");
        let rustc = option_env!("PGFS_RUSTC_VERSION").unwrap_or("unknown");
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let backtrace_env = std::env::var("RUST_BACKTRACE").unwrap_or_else(|_| "unset".to_string());
        tracing::error!(
            "panic at {}: {}\npgfs v{version} on {os}/{arch}, built with {rustc}, RUST_BACKTRACE={backtrace_env}\nBacktrace:\n{:?}",
            loc, payload, bt
        );
        eprintln!(
            "pgfs (panic): {}: {}\npgfs v{version} on {os}/{arch}, built with {rustc}, RUST_BACKTRACE={backtrace_env}\nBacktrace:\n{:?}",
            loc, payload, bt
        );
    }));

    let args = Args::parse();
    if let Err(err) = run(&args) {
        // {:#} prints the whole chain: what failed, where, and the cause.
        eprintln!("pgfs: {err:#}");
        std::process::exit(1);
    }
}

fn mountpoint_hash(mountpoint: &str) -> String {
    // Simple hash — just hex-encode the path. Good enough for a /tmp
    // filename suffix; collisions across different mountpoints on the
    // same host are astronomically unlikely for this use.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    mountpoint.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn write_atomic(path: &str, contents: &str) {
    if let Err(e) = std::fs::write(path, contents) {
        tracing::warn!("could not write {path}: {e:#}");
    }
}

#[allow(dead_code)]
fn remove_file(path: &str) {
    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!("could not remove {path}: {e:#}");
    }
}

fn run(args: &Args) -> Result<()> {
    // Block SIGINT/SIGTERM/SIGHUP/SIGUSR1/SIGUSR2 process-wide *first*, so
    // a signal delivered during startup (before the sigwait thread exists)
    // queues as pending instead of terminating the daemon with the default
    // disposition. The signal-waiter thread below drains them. This is the
    // StateIntrospection #3 contract: signals never block FUSE dispatch and
    // are only ever handled on the dedicated thread.
    let signals = [
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGUSR1,
        libc::SIGUSR2,
    ];
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(&mut set) != 0
            || signals.iter().any(|s| libc::sigaddset(&mut set, *s) != 0)
        {
            tracing::warn!("could not build the signal set; Ctrl+C will fall back to AutoUnmount");
        } else if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            tracing::warn!("could not block signals; Ctrl+C will fall back to AutoUnmount");
        }
    }

    let db = Db::connect(&args.conn, args.replica.as_deref())?;

    if let Some(replica) = &args.replica {
        tracing::info!("replica mode: reads served from standby ({replica}) when fresh");
    } else {
        tracing::info!("replica mode: off (single primary)");
    }

    // Health: PID file. The path is printed and logged (spec/observe.py
    // HealthEndpoint #1) so supervisors and humans can find it.
    let hash = mountpoint_hash(&args.mountpoint);
    let pid_path = format!("/tmp/pgfs-{hash}.pid");
    let ready_path = format!("/tmp/pgfs-{hash}.ready");
    write_atomic(&pid_path, &std::process::id().to_string());
    println!("health pid file: {pid_path}");
    tracing::info!("health: pid file written to {pid_path}");

    // Clean up health files on normal exit.
    let _pid_guard = FileGuard(pid_path.clone());
    let _ready_guard = FileGuard(ready_path.clone());

    let options = vec![
        fuser::MountOption::FSName("pgfs".to_string()),
        fuser::MountOption::AutoUnmount,
    ];

    // SIGUSR1 → state dump; SIGUSR2 → profiling toggle. The dump flag is
    // shared with PgFs: the signal-waiter thread sets it, the next FUSE
    // callback logs the dump (spec/observe.py StateIntrospection #1).
    let dump_requested = Arc::new(AtomicBool::new(false));

    println!("mounting pgfs at {}", args.mountpoint);
    let mut session = error::ctx(
        fuser::Session::new(
            PgFs::new(db, Arc::clone(&dump_requested)),
            std::path::Path::new(&args.mountpoint),
            &options,
        ),
        &format!("mount at {}", args.mountpoint),
    )?;

    // Health: readiness file.  The supervisor polls for this.
    write_atomic(&ready_path, "ready");

    // Start background threads.
    metrics::start_liveness_heartbeat();
    metrics::start_periodic_export();

    // Shared unmount handle, used by the signal-waiter thread (clean
    // shutdown on SIGINT/TERM/HUP) and the deadlock watchdog below
    // (spec/observe.py HealthEndpoint #2: a stalled heartbeat initiates
    // a clean unmount).
    let unmounter = Arc::new(Mutex::new(session.unmount_callable()));

    // Deadlock watchdog: when a FUSE callback flags a stalled liveness
    // heartbeat, unmount cleanly from this thread (never from inside a
    // callback).
    {
        let unmounter = Arc::clone(&unmounter);
        std::thread::Builder::new()
            .name("deadlock-watchdog".into())
            .spawn(move || {
                // Poll until a callback flags a stalled heartbeat, then
                // unmount cleanly. Retry a few times; if the mount is stuck
                // busy, the daemon exits and AutoUnmount removes the mount
                // with it — the kernel guarantees no ghost mount.
                let mut attempts = 0;
                while !metrics::DEADLOCK_DETECTED.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                tracing::error!("deadlock detected; unmounting cleanly");
                loop {
                    let err = unmounter
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .unmount()
                        .err();
                    if err.is_none() {
                        break;
                    }
                    attempts += 1;
                    tracing::error!(
                        "{:#}",
                        crate::error::failure(format!(
                            "deadlock unmount attempt {attempts} failed: {:?}",
                            err.unwrap()
                        ))
                    );
                    if attempts >= 5 {
                        tracing::error!(
                            "deadlock unmount failed 5 times; exiting (AutoUnmount removes the mount)"
                        );
                        std::process::exit(1);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            })
            .expect("spawn deadlock watchdog thread");
    }

    // Signals are already blocked process-wide (top of run()); the waiter
    // thread below drains them via sigwait.
    let unmounter = Arc::clone(&unmounter);
    let dump_flag = Arc::clone(&dump_requested);

    let _mountpoint = args.mountpoint.clone();
    std::thread::Builder::new()
        .name("signal-waiter".into())
        .spawn(move || unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for s in &signals {
                libc::sigaddset(&mut set, *s);
            }
            loop {
                let mut sig = 0;
                if libc::sigwait(&set, &mut sig) != 0 {
                    tracing::error!(
                        "{:#}",
                        crate::error::failure(format!(
                            "sigwait failed: {}",
                            std::io::Error::last_os_error()
                        ))
                    );
                    break;
                }
                match sig {
                    libc::SIGINT | libc::SIGTERM | libc::SIGHUP => {
                        tracing::info!("received signal {sig}; unmounting cleanly");
                        if let Err(e) = unmounter.lock().unwrap_or_else(|p| p.into_inner()).unmount() {
                            tracing::error!(
                                "{:#}",
                                crate::error::failure(format!(
                                    "clean unmount failed (AutoUnmount will still clean up on exit): {e:?}"
                                ))
                            );
                        }
                        break;
                    }
                    libc::SIGUSR1 => {
                        dump_flag.store(true, Ordering::Release);
                        tracing::info!("SIGUSR1 received; state dump will appear on next FUSE callback");
                    }
                    libc::SIGUSR2 => {
                        // Runs on the signal-waiter thread (a background
                        // thread), never inside a signal handler.
                        profiling::toggle();
                    }
                    _ => {}
                }
            }
        })
        .expect("spawn signal waiter thread");

    let t0 = Instant::now();
    error::ctx(session.run(), "serve FUSE requests")?;

    // On clean unmount, dump a final state snapshot for the log.
    tracing::info!(
        "{}",
        metrics::state_dump(
            0, // ino cache size not accessible from here after session ends
            0,
            "disconnected",
            t0.elapsed(),
        )
    );

    Ok(())
}

/// Build the tracing subscriber. Under the `profiling` feature this also
/// installs a `tracing-chrome` layer that captures every FUSE callback span
/// and nested `db::` span; the trace is written to
/// /tmp/pgfs-trace-{ts}.json when the returned guard drops (clean shutdown).
#[cfg(feature = "profiling")]
fn init_tracing() -> tracing_chrome::FlushGuard {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pgfs=info"));
    let json = std::env::var("RUST_LOG_FORMAT").as_deref() == Ok("json");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let trace_path = format!("/tmp/pgfs-trace-{ts}.json");
    let (chrome_layer, guard) = tracing_chrome::ChromeLayerBuilder::new()
        .file(&trace_path)
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(chrome_layer);
    if json {
        subscriber
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .json(),
            )
            .init();
    } else {
        subscriber
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
    }
    tracing::info!("span trace will be written to {trace_path} on shutdown");
    guard
}

/// Default build: text/JSON fmt layer only, zero profiling code compiled in.
#[cfg(not(feature = "profiling"))]
fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pgfs=info"));
    let json = std::env::var("RUST_LOG_FORMAT").as_deref() == Ok("json");
    let subscriber = tracing_subscriber::registry().with(env_filter);
    if json {
        subscriber
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .json(),
            )
            .init();
    } else {
        subscriber
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
    }
}

/// Remove a file on drop (RAII guard for health files).
struct FileGuard(String);
impl Drop for FileGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            // File may already be gone; only warn about unexpected errors.
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("could not remove {}: {e:#}", self.0);
            }
        }
    }
}

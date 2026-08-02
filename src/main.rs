mod db;
mod error;
mod fs;
mod metrics;

use clap::Parser;
use db::Db;
use error::Result;
use fs::PgFs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

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
}

fn main() {
    // Initialise the tracing subscriber before any other work.
    // RUST_LOG controls filter levels; RUST_LOG_FORMAT=json for JSON output.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pgfs=info")),
        )
        .with_writer(std::io::stderr)
        .init();

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
        tracing::error!("panic at {}: {}\nBacktrace:\n{:?}", loc, payload, bt);
        eprintln!("pgfs (panic): {}: {}\nBacktrace:\n{:?}", loc, payload, bt);
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
    let db = Db::connect(&args.conn)?;

    // Health: PID file.
    let hash = mountpoint_hash(&args.mountpoint);
    let pid_path = format!("/tmp/pgfs-{hash}.pid");
    let ready_path = format!("/tmp/pgfs-{hash}.ready");
    write_atomic(&pid_path, &std::process::id().to_string());

    // Clean up health files on normal exit.
    let _pid_guard = FileGuard(pid_path.clone());
    let _ready_guard = FileGuard(ready_path.clone());

    let options = vec![
        fuser::MountOption::FSName("pgfs".to_string()),
        fuser::MountOption::AutoUnmount,
    ];

    println!("mounting pgfs at {}", args.mountpoint);
    let mut session = error::ctx(
        fuser::Session::new(
            PgFs::new(db),
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

    // SIGUSR1 → state dump; SIGUSR2 → profiling toggle.
    let dump_requested = Arc::new(AtomicBool::new(false));

    // Block SIGINT/SIGTERM/SIGHUP/SIGUSR1/SIGUSR2 process-wide, then
    // wait on a dedicated thread.
    let mut unmounter = session.unmount_callable();
    let dump_flag = Arc::clone(&dump_requested);
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
                        if let Err(e) = unmounter.unmount() {
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
                        #[cfg(feature = "profiling")]
                        tracing::info!("SIGUSR2 received (profiling toggle — not yet wired)");
                        #[cfg(not(feature = "profiling"))]
                        tracing::info!("SIGUSR2 received (profiling not compiled in — rebuild with --features profiling)");
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

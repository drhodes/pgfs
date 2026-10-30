//! pgfs: a POSIX filesystem stored in PostgreSQL.
//!
//! The binary (src/main.rs) is a thin wrapper over [`cli::main`]. The
//! library exists so the reference model (tests/model/) and the
//! integration tests can drive the same storage interface the daemon uses
//! (spec/verify.py ReferenceModel).

pub mod archive;
pub mod backend;
pub mod buffer;
pub mod cache;
pub mod changes;
pub mod check;
pub mod cli;
pub mod conn;
pub mod errno;
pub mod error;
#[cfg(feature = "fault-injection")]
pub mod fault;
pub mod fs;
pub mod health;
pub mod metrics;
pub mod mount;
pub mod names;
pub mod pg;
pub mod profiling;
pub mod redact;
pub mod replica;
pub mod schema;
pub mod systemd;

use tracing_subscriber::prelude::*;

/// Install the tracing subscriber before any other work (spec/app.py
/// UsesStructuredTracing) and the panic hook that writes a crash report to
/// stderr and the log (spec/app.py ReportsCrashes).
pub fn init_tracing_and_panics() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pgfs=info"));
    let json = std::env::var("RUST_LOG_FORMAT").as_deref() == Ok("json");
    let registry = tracing_subscriber::registry().with(env_filter);
    #[cfg(feature = "profiling")]
    let registry = {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = format!("/tmp/pgfs-trace-{ts}.json");
        let (layer, guard) = tracing_chrome::ChromeLayerBuilder::new()
            .file(&path)
            .build();
        // The guard writes the trace when dropped at exit.
        Box::leak(Box::new(guard));
        eprintln!("pgfs: span trace will be written to {path} on shutdown");
        registry.with(layer)
    };
    let r = if json {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .json()
                    .boxed(),
            )
            .try_init()
    } else {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .boxed(),
            )
            .try_init()
    };
    let _ = r;
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
        let bt = std::backtrace::Backtrace::force_capture();
        let version = env!("CARGO_PKG_VERSION");
        let rustc = option_env!("PGFS_RUSTC_VERSION").unwrap_or("unknown");
        let report = format!(
            "panic at {loc}: {payload}\npgfs v{version} on {}/{}, built with {rustc}, RUST_BACKTRACE={}\nlog: stderr of this process\nBacktrace:\n{bt}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::env::var("RUST_BACKTRACE").unwrap_or_else(|_| "unset".into())
        );
        tracing::error!("{report}");
        eprintln!("pgfs (panic): {report}");
    }));
}

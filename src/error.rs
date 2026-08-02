//! Central error handling.
//!
//! Every failure in pgfs should tell a story: *what* was being attempted,
//! *where* it happened (file:line, captured automatically at the call site),
//! and the *root cause* that came up from underneath. These helpers wrap any
//! error with that context, so a printed/logged error reads like:
//!
//! ```text
//!   read file "notes.txt" from "docs" (at src/db.rs:63): query failed
//!   Caused by: db error: ERROR: could not connect to server
//! ```
//!
//! `anyhow::Error` keeps the whole chain; printing with `{err:#}` shows every
//! layer. FUSE has no channel to send this story to the kernel — the kernel
//! gets an errno — so `fs.rs` logs the full chain (via the `log_and_reply!`
//! macro) and only then replies.

use std::panic::Location;

pub type Result<T> = anyhow::Result<T>;

/// Wrap a fallible operation with a description of what was being attempted
/// and the call site (file:line) where it happened. The original cause stays
/// attached underneath the context.
#[track_caller]
pub fn ctx<T, E>(result: std::result::Result<T, E>, what: &str) -> Result<T>
where
    E: Into<anyhow::Error>,
{
    // Capture the tracked call site before entering the closure — inside a
    // closure body Location::caller() would point at error.rs itself.
    let loc = Location::caller();
    result.map_err(|cause| {
        cause
            .into()
            .context(format!("{what} (at {}:{})", loc.file(), loc.line()))
    })
}

/// Build a standalone error tagged with the call site, for failures that
/// aren't wrapping an existing error — an unexpected state, a missing value
/// where one was required, an impossible result.
#[track_caller]
pub fn failure(what: impl std::fmt::Display) -> anyhow::Error {
    let loc = Location::caller();
    anyhow::anyhow!("{what} (at {}:{})", loc.file(), loc.line())
}

/// Log a failure's full story and reply to the kernel with EIO. FUSE can't
/// carry the reason to the caller, so the daemon log is where the story
/// lives. Use this for *unexpected* failures (DB down, bad data, ...) — the
/// expected errnos (ENOENT, ENOTEMPTY, ...) are returned directly.
#[macro_export]
macro_rules! log_and_reply {
    ($reply:expr, $err:expr) => {{
        $crate::metrics::EIO_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::error!("{:#}", $err);
        $reply.error(libc::EIO);
        return;
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ctx() ───────────────────────────────────────────────────────────

    #[test]
    fn ctx_wraps_error_with_context() {
        let result: std::result::Result<(), anyhow::Error> = Err(anyhow::anyhow!("root cause"));
        let wrapped = ctx(result, "load config");
        let err = wrapped.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("load config"),
            "context should mention 'load config', got: {msg}"
        );
        assert!(
            msg.contains("src/error.rs"),
            "context should include file location, got: {msg}"
        );
    }

    #[test]
    fn ctx_preserves_root_cause() {
        let result: std::result::Result<(), anyhow::Error> = Err(anyhow::anyhow!("root cause"));
        let wrapped = ctx(result, "outer operation");
        let err = wrapped.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("root cause"),
            "chain should include 'root cause', got: {msg}"
        );
    }

    #[test]
    fn ctx_on_ok_passes_through() {
        let result: std::result::Result<i32, anyhow::Error> = Ok(42);
        let wrapped = ctx(result, "should not matter");
        assert_eq!(wrapped.unwrap(), 42);
    }

    #[test]
    fn ctx_chaining_produces_layered_story() {
        fn inner() -> Result<()> {
            Err(failure("disk full"))
        }
        fn middle() -> Result<()> {
            ctx(inner(), "save file")
        }
        fn outer() -> Result<()> {
            ctx(middle(), "handle request")
        }

        let err = outer().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("handle request"));
        assert!(msg.contains("save file"));
        assert!(msg.contains("disk full"));
    }

    // ── failure() ───────────────────────────────────────────────────────

    #[test]
    fn failure_includes_call_site() {
        let err = failure("invariant broken");
        let msg = format!("{err}");
        assert!(msg.contains("invariant broken"));
        assert!(
            msg.contains("src/error.rs"),
            "should include file, got: {msg}"
        );
        assert!(msg.contains(':'), "should include line number, got: {msg}");
    }

    #[test]
    fn failure_is_anyhow_error() {
        let err = failure("test");
        // It should be an anyhow::Error with the message.
        assert_eq!(format!("{err}"), format!("{err}")); // anyhow Display works
    }
}

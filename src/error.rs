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
        log::error!("{:#}", $err);
        $reply.error(libc::EIO);
        return;
    }};
}

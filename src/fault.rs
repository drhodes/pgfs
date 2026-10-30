//! Database fault injection for tests (spec/verify.py DbFaultInjection),
//! compiled only with the `fault-injection` feature: fail the Nth call of
//! a statement with any SQLSTATE, or delay it.

use crate::conn::DbError;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Fault {
    /// Statement name, or "*" for any.
    pub stmt: String,
    /// Fail the call after this many matching calls have passed.
    pub after: u64,
    pub sqlstate: String,
    pub delay: Option<Duration>,
    pub seen: u64,
    pub fired: bool,
    /// Fail after the statement ran (and committed): an ambiguous commit.
    pub after_commit: bool,
}

static FAULTS: Mutex<Vec<Fault>> = Mutex::new(Vec::new());

pub fn inject(stmt: &str, after: u64, sqlstate: &str, delay: Option<Duration>) {
    push(stmt, after, sqlstate, delay, false);
}

/// Let the statement run and commit, then report a dropped connection.
pub fn inject_after_commit(stmt: &str, after: u64) {
    push(stmt, after, "08006", None, true);
}

fn push(stmt: &str, after: u64, sqlstate: &str, delay: Option<Duration>, after_commit: bool) {
    FAULTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(Fault {
            stmt: stmt.into(),
            after,
            sqlstate: sqlstate.into(),
            delay,
            seen: 0,
            fired: false,
            after_commit,
        });
}

pub fn clear() {
    FAULTS.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// Called before every prepared statement runs.
pub fn check(stmt: &str) -> Option<DbError> {
    fire(stmt, false)
}

/// Called after a prepared statement succeeded.
pub fn check_after(stmt: &str) -> Option<DbError> {
    fire(stmt, true)
}

fn fire(stmt: &str, after_commit: bool) -> Option<DbError> {
    let mut g = FAULTS.lock().unwrap_or_else(|p| p.into_inner());
    for f in g.iter_mut() {
        if f.fired || f.after_commit != after_commit || (f.stmt != "*" && f.stmt != stmt) {
            continue;
        }
        if f.seen < f.after {
            f.seen += 1;
            continue;
        }
        f.fired = true;
        if let Some(d) = f.delay {
            std::thread::sleep(d);
        }
        if f.sqlstate.is_empty() {
            return None;
        }
        return Some(DbError::synthetic(
            &f.sqlstate,
            &format!("injected fault on {stmt}"),
        ));
    }
    None
}

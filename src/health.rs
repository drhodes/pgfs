//! The mount's database connection state (spec/failure.py
//! DatabaseStateMachine), shared by every worker and the reconnector.

use crate::errno::{FsError, FsResult};
use crate::metrics;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DbState {
    Connected = 0,
    Broken = 1,
    Reconnecting = 2,
    Revalidating = 3,
    Fenced = 4,
}

impl DbState {
    fn from_u8(v: u8) -> DbState {
        match v {
            0 => DbState::Connected,
            1 => DbState::Broken,
            2 => DbState::Reconnecting,
            3 => DbState::Revalidating,
            _ => DbState::Fenced,
        }
    }
}

pub struct Health {
    state: AtomicU8,
    /// Incremented each time revalidation passes; a worker whose
    /// connection predates it reopens before serving.
    generation: AtomicU64,
    lock: Mutex<String>,
    cv: Condvar,
    /// Per worker: the last op_seq whose reply was sent to the kernel, and
    /// the mutation in flight when the connection broke (-1: none)
    /// (spec/failure.py IdempotentMutations).
    pub last_replied: Vec<AtomicI64>,
    pub in_flight: Vec<AtomicI64>,
}

impl Health {
    pub fn new(workers: usize) -> Health {
        Health {
            state: AtomicU8::new(DbState::Connected as u8),
            generation: AtomicU64::new(0),
            lock: Mutex::new(String::new()),
            cv: Condvar::new(),
            last_replied: (0..workers).map(|_| AtomicI64::new(0)).collect(),
            in_flight: (0..workers).map(|_| AtomicI64::new(-1)).collect(),
        }
    }

    pub fn state(&self) -> DbState {
        DbState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn set(&self, to: DbState, why: &str) {
        let mut g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let from = self.state();
        if from == to || from == DbState::Fenced {
            return;
        }
        self.state.store(to as u8, Ordering::Release);
        metrics::DB_STATE.store(to as u8, Ordering::Relaxed);
        if to == DbState::Fenced {
            metrics::FENCED.fetch_add(1, Ordering::Relaxed);
            tracing::error!("database state {from:?} -> Fenced: {why}; every operation replies EIO until remount");
            *g = why.to_string();
        } else {
            tracing::info!(
                "database state {from:?} -> {to:?}{}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(": {why}")
                }
            );
        }
        if to == DbState::Connected {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        self.cv.notify_all();
    }

    /// A connection-class error: Connected -> Broken.
    pub fn broken(&self, why: &str) {
        if self.state() == DbState::Connected {
            self.set(DbState::Broken, why);
        }
    }

    pub fn reconnecting(&self) {
        self.set(DbState::Reconnecting, "");
    }

    pub fn revalidating(&self) {
        self.set(DbState::Revalidating, "");
    }

    pub fn connected(&self) {
        self.set(DbState::Connected, "revalidation passed");
    }

    pub fn fence(&self, why: &str) {
        self.set(DbState::Fenced, why);
    }

    pub fn fenced_reason(&self) -> String {
        self.lock.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// New requests fail fast with EIO unless Connected (never queued).
    pub fn check(&self) -> FsResult<()> {
        match self.state() {
            DbState::Connected => Ok(()),
            DbState::Fenced => Err(FsError::Fenced(self.fenced_reason())),
            s => Err(FsError::Unavailable(format!("{s:?}"))),
        }
    }

    /// Wait until Connected or Fenced, up to `timeout`. True if Connected.
    pub fn wait_connected(&self, timeout: Duration) -> bool {
        let g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let (_g, _) = self
            .cv
            .wait_timeout_while(g, timeout, |_| {
                !matches!(self.state(), DbState::Connected | DbState::Fenced)
            })
            .unwrap_or_else(|p| p.into_inner());
        self.state() == DbState::Connected
    }

    /// Wait until the state leaves Connected (the reconnector's trigger).
    pub fn wait_not_connected(&self, timeout: Duration) -> DbState {
        let g = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let (_g, _) = self
            .cv
            .wait_timeout_while(g, timeout, |_| self.state() == DbState::Connected)
            .unwrap_or_else(|p| p.into_inner());
        self.state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_and_fail_fast() {
        let h = Health::new(2);
        assert!(h.check().is_ok());
        h.broken("socket closed");
        assert_eq!(h.state(), DbState::Broken);
        assert!(matches!(h.check(), Err(FsError::Unavailable(_))));
        h.reconnecting();
        h.revalidating();
        let g = h.generation();
        h.connected();
        assert_eq!(h.generation(), g + 1);
        assert!(h.wait_connected(Duration::from_millis(1)));
        h.fence("lease taken over");
        assert!(matches!(h.check(), Err(FsError::Fenced(_))));
        h.connected();
        assert_eq!(h.state(), DbState::Fenced, "Fenced is final");
    }
}

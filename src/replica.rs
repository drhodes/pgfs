//! Reads from a physical streaming standby under LSN tickets
//! (spec/replica.py LsnTicketReads, VerifiesStandbyIdentity,
//! DetectsReplicationLag, FallsBackToPrimary, ReplicaObservability).

use crate::conn::{Conn, ConnSettings};
use crate::metrics;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Primary,
    Standby,
}

/// The primary's identity, that a standby must share.
#[derive(Debug, Clone)]
pub struct PrimaryIdentity {
    pub system_identifier: i64,
    pub timeline: i32,
    pub fs_uuid: String,
}

struct State {
    usable: bool,
    why: String,
    replay_lsn: u64,
    primary_flush: u64,
    last_poll: Option<Instant>,
    poll_conn: Option<Conn>,
    next_retry: Instant,
    backoff: Duration,
}

pub struct Replica {
    settings: ConnSettings,
    primary: PrimaryIdentity,
    state: Mutex<State>,
    tickets: Mutex<HashMap<u64, u64>>,
}

const MAX_TICKETS: usize = 100_000;

impl Replica {
    /// Connect and verify the standby. An unreachable or unverified standby
    /// is logged at WARN and retried in the background; startup never fails
    /// because of it.
    pub fn new(settings: ConnSettings, primary: PrimaryIdentity) -> Replica {
        let r = Replica {
            settings,
            primary,
            state: Mutex::new(State {
                usable: false,
                why: "not yet connected".into(),
                replay_lsn: 0,
                primary_flush: 0,
                last_poll: None,
                poll_conn: None,
                next_retry: Instant::now(),
                backoff: Duration::from_millis(500),
            }),
            tickets: Mutex::new(HashMap::new()),
        };
        r.try_connect();
        r
    }

    pub fn describe(&self) -> String {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        format!(
            "standby {} usable={} ({}) replay_lsn={} primary_flush={}",
            self.settings.redacted(),
            s.usable,
            s.why,
            s.replay_lsn,
            s.primary_flush
        )
    }

    /// Check that the standby belongs to this cluster and filesystem.
    fn verify(&self, c: &mut Conn) -> Result<()> {
        let row = c
            .query_sql(
                "SELECT (SELECT system_identifier FROM pg_control_system()),
                        (SELECT timeline_id FROM pg_control_checkpoint()),
                        pg_is_in_recovery(),
                        (SELECT value FROM pgfs.meta WHERE key = 'fs_uuid')",
                &[],
            )
            .map_err(|e| anyhow!("read standby identity: {e}"))?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("read standby identity: no row"))?;
        let sysid: i64 = row.get(0);
        let tli: i32 = row.get(1);
        let rec: bool = row.get(2);
        let uuid: Option<String> = row.get(3);
        if sysid != self.primary.system_identifier {
            bail!(
                "system identifier {sysid} differs from the primary's {}",
                self.primary.system_identifier
            );
        }
        if tli > self.primary.timeline {
            bail!(
                "timeline {tli} is ahead of the primary's {}",
                self.primary.timeline
            );
        }
        if !rec {
            bail!("the server is not in recovery, so it is not a standby");
        }
        if uuid.as_deref() != Some(self.primary.fs_uuid.as_str()) {
            bail!(
                "fs_uuid {:?} differs from the primary's {}",
                uuid,
                self.primary.fs_uuid
            );
        }
        Ok(())
    }

    fn try_connect(&self) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if Instant::now() < s.next_retry {
            return;
        }
        match Conn::open(&self.settings).and_then(|mut c| self.verify(&mut c).map(|_| c)) {
            Ok(c) => {
                tracing::info!(
                    "replica mode: standby {} verified",
                    self.settings.redacted()
                );
                s.poll_conn = Some(c);
                s.usable = true;
                s.why = "verified".into();
                s.backoff = Duration::from_millis(500);
            }
            Err(e) => {
                tracing::warn!(
                    "standby {} is not used: {e:#}; retrying in {:?}",
                    self.settings.redacted(),
                    s.backoff
                );
                s.usable = false;
                s.why = format!("{e:#}");
                s.next_retry = Instant::now() + s.backoff;
                s.backoff = (s.backoff * 2).min(Duration::from_secs(30));
            }
        }
    }

    /// Open a read connection for one worker (verified).
    pub fn open_reader(&self) -> Result<Conn> {
        let mut c = Conn::open(&self.settings)?;
        self.verify(&mut c)?;
        for (name, sql) in crate::pg::read_statements() {
            c.prepare(name, &sql)
                .map_err(|e| anyhow!("prepare {name} on the standby: {e}"))?;
        }
        Ok(c)
    }

    pub fn mark_unusable(&self, why: &str) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.usable {
            tracing::warn!("standby {} unusable: {why}", self.settings.redacted());
        }
        s.usable = false;
        s.why = why.to_string();
        s.poll_conn = None;
        s.next_retry = Instant::now() + s.backoff;
    }

    /// Record a mutation's ticket on the inodes it touched.
    pub fn record_ticket(&self, inos: &[u64], insert_lsn: u64, flush_lsn: u64) {
        {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.primary_flush = s.primary_flush.max(flush_lsn);
        }
        let mut t = self.tickets.lock().unwrap_or_else(|p| p.into_inner());
        if t.len() > MAX_TICKETS {
            // Old tickets are below any replay position that matters; keep
            // the newest half.
            let mut v: Vec<u64> = t.values().copied().collect();
            v.sort_unstable();
            let cut = v[v.len() / 2];
            t.retain(|_, l| *l >= cut);
        }
        for &i in inos {
            t.insert(i, insert_lsn);
        }
        let mount_ticket = insert_lsn.max(t.get(&0).copied().unwrap_or(0));
        t.insert(0, mount_ticket);
    }

    fn poll(&self, s: &mut State) {
        if s.last_poll
            .is_some_and(|t| t.elapsed() < Duration::from_millis(1))
        {
            return;
        }
        s.last_poll = Some(Instant::now());
        let Some(c) = s.poll_conn.as_mut() else {
            return;
        };
        let r = c.query_sql(
            "SELECT (pg_last_wal_replay_lsn() - '0/0'::pg_lsn)::bigint, pg_is_in_recovery(),
                    COALESCE((extract(epoch FROM now() - pg_last_xact_replay_timestamp()) * 1000)::bigint, 0)",
            &[],
        );
        match r {
            Ok(rows) if !rows.is_empty() => {
                let rec: bool = rows[0].get(1);
                if !rec {
                    s.usable = false;
                    s.why = "the standby left recovery".into();
                    tracing::warn!(
                        "standby {} is no longer in recovery; not used",
                        self.settings.redacted()
                    );
                    return;
                }
                s.replay_lsn = rows[0].get::<_, Option<i64>>(0).unwrap_or(0).max(0) as u64;
                metrics::REPLICA_LAG_BYTES.store(
                    s.primary_flush.saturating_sub(s.replay_lsn),
                    Ordering::Relaxed,
                );
                metrics::REPLICA_LAG_MS
                    .store(rows[0].get::<_, i64>(2).max(0) as u64, Ordering::Relaxed);
            }
            Ok(_) => {}
            Err(e) => {
                s.usable = false;
                s.why = format!("poll failed: {e}");
                s.poll_conn = None;
                s.next_retry = Instant::now() + s.backoff;
            }
        }
    }

    /// Decide where a read of `ino` goes: the standby once it has replayed
    /// the inode's ticket, else the primary (spec/replica.py LsnTicketReads).
    pub fn route(&self, ino: u64, wait: Duration) -> Route {
        let ticket = self
            .tickets
            .lock()
            .map(|t| t.get(&ino).copied().unwrap_or(0))
            .unwrap_or(u64::MAX);
        let usable = self.state.lock().map(|s| s.usable).unwrap_or(false);
        if !usable {
            self.try_connect();
            if !self.state.lock().map(|s| s.usable).unwrap_or(false) {
                self.record_fallback("standby not usable");
                return Route::Primary;
            }
        }
        let deadline = Instant::now() + wait;
        let mut waited = false;
        loop {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            self.poll(&mut s);
            if !s.usable {
                drop(s);
                self.record_fallback("standby not usable");
                return Route::Primary;
            }
            if s.replay_lsn >= ticket {
                return Route::Standby;
            }
            if s.primary_flush < ticket {
                // Not even flushed on the primary: it cannot be on the standby.
                return Route::Primary;
            }
            drop(s);
            if !waited {
                waited = true;
                metrics::REPLICA_TICKET_WAITS.fetch_add(1, Ordering::Relaxed);
            }
            if Instant::now() >= deadline {
                self.record_fallback("standby behind the ticket");
                return Route::Primary;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn record_fallback(&self, why: &str) {
        metrics::REPLICA_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
        if !metrics::REPLICA_FALLBACK_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!("reads fall back to the primary: {why}");
        }
    }
}

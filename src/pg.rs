//! `PgBackend`: the `PgfsBackend` trait over one owned connection
//! (spec/optimize.py PgfsBackendTraitReq, RemoteAsyncBackendReq).
//!
//! Every mutation is one call to a `pgfs.*` function (one round trip,
//! spec/consistency.py OneRoundTripPerOperation) carrying the idempotency
//! key `(mount_id, worker_id, op_seq)` (spec/failure.py
//! IdempotentMutations). Statements are prepared once per connection
//! (spec/optimize.py PreparedStatementReq).

use crate::backend::*;
use crate::conn::{Canceller, Conn, ConnSettings, DbError};
use crate::errno::{DbClass, FsError, FsResult};
use crate::health::Health;
use crate::metrics;
use crate::replica::{Replica, Route};
use anyhow::anyhow;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_postgres::types::ToSql;
use tokio_postgres::Row;

const ATTR_COLS: &str = "ino, gen, kind, mode, uid, gid, nlink, size, blocks, atime_ns, mtime_ns, ctime_ns, btime_ns, rdev, version";

/// Every statement a worker prepares when its connection opens.
pub fn statements() -> Vec<(&'static str, String)> {
    let a = ATTR_COLS;
    vec![
        ("lookup", format!("SELECT {a} FROM pgfs.lookup($1, $2, $3, $4)")),
        ("getattr", format!("SELECT {a} FROM pgfs.getattr($1, $2, $3, $4)")),
        ("readdir", "SELECT name, ino, kind FROM pgfs.readdir($1, $2, $3, $4, $5)".into()),
        ("readdirplus", "SELECT r.name, (r.a).* FROM pgfs.readdirplus($1, $2, $3, $4, $5) r".into()),
        ("read_blocks", "SELECT block_no, data, crc32c, blob_id, is_inline FROM pgfs.read_blocks($1, $2, $3, $4, $5, $6)".into()),
        ("readlink", "SELECT pgfs.readlink($1, $2, $3)".into()),
        ("getxattr", "SELECT v FROM pgfs.getxattr($1, $2, $3, $4) v".into()),
        ("listxattr", "SELECT n FROM pgfs.listxattr($1, $2, $3) n".into()),
        ("seek_block", "SELECT pgfs.seek_block($1, $2, $3, $4, $5)".into()),
        ("fs_usage", "SELECT physical_bytes, inodes FROM pgfs.fs_usage()".into()),
        ("op_slot", "SELECT op_seq, result FROM pgfs.op_slot($1, $2)".into()),
        ("create", format!("SELECT {a} FROM pgfs.create($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)")),
        ("mkdir", format!("SELECT {a} FROM pgfs.mkdir($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)")),
        ("mknod", format!("SELECT {a} FROM pgfs.mknod($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)")),
        ("symlink", format!("SELECT {a} FROM pgfs.symlink($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)")),
        ("link", format!("SELECT {a} FROM pgfs.link($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)")),
        ("unlink", "SELECT ino, nlink, orphaned FROM pgfs.unlink($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)".into()),
        ("rmdir", "SELECT pgfs.rmdir($1, $2, $3, $4, $5, $6, $7, $8)".into()),
        ("rename", "SELECT ino, replaced_ino, replaced_nlink, replaced_orphaned FROM pgfs.rename($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)".into()),
        ("setattr", format!("SELECT {a} FROM pgfs.setattr($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)")),
        ("write_blocks", format!("SELECT {a} FROM pgfs.write_blocks($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)")),
        ("copy_range", format!("SELECT {a} FROM pgfs.copy_range($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)")),
        ("setxattr", "SELECT pgfs.setxattr($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)".into()),
        ("removexattr", "SELECT pgfs.removexattr($1, $2, $3, $4, $5, $6, $7, $8)".into()),
        ("release_orphan", "SELECT pgfs.release_orphan($1, $2, $3, $4, $5, $6)".into()),
        ("sync_barrier", "SELECT pgfs.sync_barrier($1, $2, $3)".into()),
        ("snapshot", "SELECT layer FROM pgfs.snapshot($1, $2, $3, $4)".into()),
        ("branch", "SELECT layer FROM pgfs.branch($1, $2, NULL, $3, $4, $5)".into()),
        ("promote", "SELECT rows_folded FROM pgfs.promote($1, $2, $3)".into()),
        ("lsn", "SELECT (pg_current_wal_insert_lsn() - '0/0'::pg_lsn)::bigint, (pg_current_wal_flush_lsn() - '0/0'::pg_lsn)::bigint".into()),
    ]
}

/// The statements a standby connection prepares (reads only).
pub fn read_statements() -> Vec<(&'static str, String)> {
    statements()
        .into_iter()
        .filter(|(n, _)| {
            matches!(
                *n,
                "lookup"
                    | "getattr"
                    | "readdir"
                    | "readdirplus"
                    | "read_blocks"
                    | "readlink"
                    | "getxattr"
                    | "listxattr"
                    | "seek_block"
            )
        })
        .collect()
}

pub fn attr_from_row(r: &Row, off: usize) -> Attr {
    Attr {
        ino: r.get::<_, i64>(off) as u64,
        gen: r.get::<_, i64>(off + 1) as u64,
        kind: Kind::from_char(r.get::<_, i8>(off + 2)),
        mode: r.get::<_, i32>(off + 3) as u32,
        uid: r.get::<_, i64>(off + 4) as u32,
        gid: r.get::<_, i64>(off + 5) as u32,
        nlink: r.get::<_, i32>(off + 6).max(0) as u32,
        size: r.get::<_, i64>(off + 7) as u64,
        blocks: r.get::<_, i64>(off + 8) as u64,
        atime_ns: r.get(off + 9),
        mtime_ns: r.get(off + 10),
        ctime_ns: r.get(off + 11),
        btime_ns: r.get(off + 12),
        rdev: r.get::<_, i64>(off + 13) as u64,
        version: r.get(off + 14),
    }
}

/// An attr from its op_log jsonb form (`to_jsonb(pgfs.attr)`).
pub fn attr_from_json(v: &serde_json::Value) -> Option<Attr> {
    let i = |k: &str| v.get(k).and_then(|x| x.as_i64());
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .and_then(|s| s.bytes().next())
        .unwrap_or(b'f');
    Some(Attr {
        ino: i("ino")? as u64,
        gen: i("gen")? as u64,
        kind: Kind::from_char(kind as i8),
        mode: i("mode")? as u32,
        uid: i("uid")? as u32,
        gid: i("gid")? as u32,
        nlink: i("nlink")?.max(0) as u32,
        size: i("size")? as u64,
        blocks: i("blocks").unwrap_or(0) as u64,
        atime_ns: i("atime_ns")?,
        mtime_ns: i("mtime_ns")?,
        ctime_ns: i("ctime_ns")?,
        btime_ns: i("btime_ns")?,
        rdev: i("rdev").unwrap_or(0) as u64,
        version: i("version").unwrap_or(0),
    })
}

/// The identity a mutation carries.
#[derive(Debug, Clone, Copy)]
pub struct MountIdentity {
    pub mount_id: uuid::Uuid,
    pub fence: i64,
    pub worker: i32,
}

/// Where the in-flight request's cancel handle is published for the
/// watchdog (spec/failure.py InFlightAgeWatchdog).
pub type CancelSlot = Arc<Mutex<Option<Canceller>>>;

pub struct PgBackend {
    settings: ConnSettings,
    conn: Option<Conn>,
    conn_gen: u64,
    health: Arc<Health>,
    id: Option<MountIdentity>,
    op_seq: i64,
    replica: Option<Arc<Replica>>,
    replica_conn: Option<Conn>,
    replica_wait: Duration,
    cancel_slot: CancelSlot,
    /// Tree name, for checksum-failure reports.
    pub tree: String,
}

type Params = Vec<Box<dyn ToSql + Sync + Send>>;

/// Builds a mutation's parameters from (mount_id, fence, worker, op_seq).
type BuildParams = Box<dyn Fn(Option<uuid::Uuid>, Option<i64>, i32, i64) -> Params>;

fn refs(p: &Params) -> Vec<&(dyn ToSql + Sync)> {
    p.iter()
        .map(|b| b.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

fn i(v: u64) -> i64 {
    v as i64
}

/// The CRC32C stored with a blob, as a signed int (spec/integrity.py BlobChecksums).
pub fn crc(data: &[u8]) -> i32 {
    crc32c::crc32c(data) as i32
}

impl PgBackend {
    pub fn new(
        settings: ConnSettings,
        health: Arc<Health>,
        id: Option<MountIdentity>,
    ) -> PgBackend {
        PgBackend {
            settings,
            conn: None,
            conn_gen: u64::MAX,
            health,
            id,
            op_seq: 0,
            replica: None,
            replica_conn: None,
            replica_wait: Duration::from_millis(20),
            cancel_slot: Arc::new(Mutex::new(None)),
            tree: String::new(),
        }
    }

    pub fn with_replica(mut self, replica: Arc<Replica>, wait: Duration) -> Self {
        self.replica = Some(replica);
        self.replica_wait = wait;
        self
    }

    pub fn cancel_slot(&self) -> CancelSlot {
        self.cancel_slot.clone()
    }

    pub fn set_op_seq(&mut self, seq: i64) {
        self.op_seq = seq;
    }

    pub fn op_seq(&self) -> i64 {
        self.op_seq
    }

    /// Open the connection now (at startup: connections are never opened on
    /// demand, spec/concurrency.py WorkerOwnsConnection).
    pub fn open(&mut self) -> anyhow::Result<()> {
        let mut c = Conn::open(&self.settings)?;
        for (name, sql) in statements() {
            c.prepare(name, &sql)
                .map_err(|e| anyhow!("prepare {name}: {e}"))?;
        }
        if let Ok(mut s) = self.cancel_slot.lock() {
            *s = Some(c.canceller());
        }
        self.conn = Some(c);
        self.conn_gen = self.health.generation();
        Ok(())
    }

    /// Make sure the connection is open and belongs to the current
    /// connectivity generation; reopen after a revalidated reconnect.
    fn ensure(&mut self) -> FsResult<()> {
        self.health.check()?;
        let stale = self.conn.as_ref().is_none_or(|c| c.is_closed())
            || self.conn_gen != self.health.generation();
        if stale {
            self.conn = None;
            if let Err(e) = self.open() {
                self.health.broken(&format!("{e:#}"));
                return Err(FsError::Unavailable(format!("{e:#}")));
            }
            self.resolve_pending();
        }
        Ok(())
    }

    /// After a reconnect, a mutation that replied EIO while disconnected
    /// may have committed: if the op_log slot shows it, say so
    /// (spec/failure.py IdempotentMutations).
    fn resolve_pending(&mut self) {
        let Some(id) = self.id else { return };
        let w = id.worker as usize;
        let pending = self
            .health
            .in_flight
            .get(w)
            .map(|a| a.load(Ordering::Acquire))
            .unwrap_or(-1);
        if pending < 0 {
            return;
        }
        if let Ok(Some((seq, _))) = self.slot() {
            if seq == pending {
                tracing::warn!(
                    "worker {w}: mutation op_seq {pending} committed although it replied EIO after the disconnect timed out"
                );
                self.health.last_replied[w].store(pending, Ordering::Release);
            }
        }
        self.health.in_flight[w].store(-1, Ordering::Release);
    }

    fn slot(&mut self) -> FsResult<Option<(i64, serde_json::Value)>> {
        let Some(id) = self.id else { return Ok(None) };
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| FsError::Unavailable("no connection".into()))?;
        let rows = conn
            .query("op_slot", None, &[&id.mount_id, &id.worker])
            .map_err(|e| FsError::unexpected(anyhow!("read op_log slot: {e}")))?;
        Ok(rows.first().map(|r| {
            (
                r.get::<_, i64>(0),
                r.get::<_, Option<serde_json::Value>>(1)
                    .unwrap_or(serde_json::Value::Null),
            )
        }))
    }

    /// Map a database error to a reply, updating the state machine
    /// (spec/failure.py MapsDatabaseErrors).
    fn map_err(&mut self, what: &str, e: DbError) -> FsError {
        match e.class() {
            DbClass::UniqueViolation => FsError::Errno(libc::EEXIST),
            DbClass::Function(errno) => FsError::Errno(errno),
            DbClass::Fenced => {
                self.health.fence(&format!("{what}: {e}"));
                FsError::Fenced(e.to_string())
            }
            DbClass::RetryDirty => FsError::RetryDirty(
                e.detail
                    .as_deref()
                    .unwrap_or("")
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect(),
            ),
            DbClass::Serialization => {
                FsError::unexpected(anyhow!("{what}: serialization failure persisted: {e}"))
            }
            DbClass::LockTimeout => {
                tracing::warn!(
                    "{what}: lock timeout after {:?}: {e}",
                    self.settings.lock_timeout
                );
                FsError::Errno(libc::ETIMEDOUT)
            }
            DbClass::Cancelled => {
                let by_us = self.conn.as_ref().is_some_and(|c| c.cancel_was_sent());
                if by_us {
                    FsError::Unexpected {
                        errno: libc::EIO,
                        error: anyhow!("{what}: cancelled by the watchdog: {e}"),
                    }
                } else {
                    tracing::warn!(
                        "{what}: statement timeout after {:?}: {e}",
                        self.settings.statement_timeout
                    );
                    FsError::Errno(libc::ETIMEDOUT)
                }
            }
            DbClass::DiskFull => {
                tracing::error!("{what}: the database is out of disk space: {e}");
                FsError::Errno(libc::ENOSPC)
            }
            DbClass::Resources => {
                tracing::warn!("{what}: server out of resources: {e}");
                std::thread::sleep(Duration::from_millis(50));
                FsError::Unexpected {
                    errno: libc::EIO,
                    error: anyhow!("{what}: {e}"),
                }
            }
            DbClass::ReadOnlyServer | DbClass::Connection => {
                self.health.broken(&format!("{what}: {e}"));
                self.conn = None;
                FsError::Unavailable(format!("{what}: {e}"))
            }
            DbClass::Other => FsError::unexpected(anyhow!("{what}: {e}")),
        }
    }

    /// Run a read on the primary.
    fn read(
        &mut self,
        name: &'static str,
        ino: Option<u64>,
        params: &[&(dyn ToSql + Sync)],
    ) -> FsResult<Vec<Row>> {
        self.ensure()?;
        let r = self
            .conn
            .as_mut()
            .expect("ensured")
            .query(name, ino, params);
        match r {
            Ok(rows) => Ok(rows),
            // The request that discovers a broken connection is in flight:
            // like a mutation, it waits for revalidation (bounded by the
            // statement timeout) and runs once more. Reads are idempotent.
            Err(e) if matches!(e.class(), DbClass::Connection | DbClass::ReadOnlyServer) => {
                let err = self.map_err(name, e);
                if !self.health.wait_connected(self.settings.statement_timeout) {
                    return Err(err);
                }
                self.ensure()?;
                let r = self
                    .conn
                    .as_mut()
                    .expect("ensured")
                    .query(name, ino, params);
                r.map_err(|e| self.map_err(name, e))
            }
            Err(e) => Err(self.map_err(name, e)),
        }
    }

    /// Run a read that may go to the standby under the inode's LSN ticket
    /// (spec/replica.py LsnTicketReads, ServesReadsFromReplica,
    /// FallsBackToPrimary).
    fn read_routed(
        &mut self,
        name: &'static str,
        ino: u64,
        params: &[&(dyn ToSql + Sync)],
    ) -> FsResult<Vec<Row>> {
        if let Some(rep) = self.replica.clone() {
            if let Route::Standby = rep.route(ino, self.replica_wait) {
                if self.replica_conn.as_ref().is_none_or(|c| c.is_closed()) {
                    self.replica_conn = rep.open_reader().ok();
                }
                if let Some(c) = self.replica_conn.as_mut() {
                    match c.query(name, Some(ino), params) {
                        Ok(rows) => {
                            metrics::REPLICA_READ_COUNT.fetch_add(1, Ordering::Relaxed);
                            tracing::debug!("{name} ino={ino} served by standby");
                            return Ok(rows);
                        }
                        Err(e) => {
                            if !matches!(e.class(), DbClass::Function(_) | DbClass::UniqueViolation)
                            {
                                rep.mark_unusable(&format!("{name}: {e}"));
                                self.replica_conn = None;
                            }
                        }
                    }
                }
                rep.record_fallback("standby query failed");
            }
        }
        self.read(name, Some(ino), params)
    }

    /// Run a mutation: allocate its op_seq, retry serialization failures
    /// with the same op_seq, resolve an ambiguous commit through the op_log
    /// slot, and confirm synchronous commits (spec/failure.py
    /// IdempotentMutations, RetriesSerializationFailures;
    /// spec/durability.py FsyncIsDurable).
    #[allow(clippy::too_many_arguments)]
    fn mutate<T>(
        &mut self,
        name: &'static str,
        ino: Option<u64>,
        tickets: &[u64],
        sync: bool,
        build: impl Fn(Option<uuid::Uuid>, Option<i64>, i32, i64) -> Params,
        parse: impl Fn(&[Row]) -> FsResult<T>,
        from_json: impl Fn(&serde_json::Value) -> FsResult<T>,
    ) -> FsResult<T> {
        self.ensure()?;
        self.op_seq += 1;
        let op_seq = self.op_seq;
        let (mount, fence, worker) = match self.id {
            Some(id) => (Some(id.mount_id), Some(id.fence), id.worker),
            None => (None, None, 0),
        };
        let params = build(mount, fence, worker, op_seq);
        let p = refs(&params);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let conn = self.conn.as_mut().expect("ensured");
            conn.clear_cancel();
            let r = if self.replica.is_some() {
                conn.query_pipelined(name, ino, &p, "lsn")
                    .map(|(rows, lsn)| {
                        if let (Some(rep), Some(l)) = (&self.replica, lsn) {
                            rep.record_ticket(
                                tickets,
                                l.get::<_, i64>(0) as u64,
                                l.get::<_, i64>(1) as u64,
                            );
                        }
                        rows
                    })
            } else {
                conn.query(name, ino, &p)
            };
            match r {
                Ok(rows) => {
                    let unconfirmed = sync
                        && self
                            .conn
                            .as_ref()
                            .is_some_and(|c| c.take_unconfirmed_sync());
                    if unconfirmed {
                        self.barrier_after_unconfirmed(name)?;
                    }
                    self.replied(op_seq);
                    return parse(&rows);
                }
                Err(e) => match e.class() {
                    DbClass::Serialization if attempt < 5 => {
                        let jitter = 1
                            + (crc32c::crc32c(&op_seq.to_le_bytes()) as u64 + attempt as u64 * 37)
                                % 100;
                        std::thread::sleep(Duration::from_millis(
                            jitter.min(1u64 << (attempt * 2)),
                        ));
                        continue;
                    }
                    DbClass::Serialization => {
                        tracing::warn!("{name}: serialization failure after 5 attempts: {e}");
                        return Err(FsError::Errno(libc::EIO));
                    }
                    DbClass::Connection | DbClass::ReadOnlyServer if self.id.is_some() => {
                        let what = format!("{name}: {e}");
                        self.health.broken(&what);
                        self.conn = None;
                        return self.await_resolution(name, op_seq, sync, &p, &parse, &from_json);
                    }
                    _ => return Err(self.map_err(name, e)),
                },
            }
        }
    }

    fn replied(&mut self, op_seq: i64) {
        if let Some(id) = self.id {
            if let Some(a) = self.health.last_replied.get(id.worker as usize) {
                a.store(op_seq, Ordering::Release);
            }
        }
    }

    /// The connection broke during a mutation: wait for revalidation up to
    /// the statement timeout, then resolve through the op_log slot or run
    /// the mutation again with the same op_seq.
    fn await_resolution<T>(
        &mut self,
        name: &'static str,
        op_seq: i64,
        sync: bool,
        p: &[&(dyn ToSql + Sync)],
        parse: &impl Fn(&[Row]) -> FsResult<T>,
        from_json: &impl Fn(&serde_json::Value) -> FsResult<T>,
    ) -> FsResult<T> {
        let w = self.id.map(|i| i.worker as usize).unwrap_or(0);
        self.health.in_flight[w].store(op_seq, Ordering::Release);
        if !self.health.wait_connected(self.settings.statement_timeout) {
            return Err(FsError::Unavailable(format!(
                "{name}: the database did not come back in time"
            )));
        }
        self.conn = None;
        if let Err(e) = self.open() {
            return Err(FsError::Unavailable(format!("{e:#}")));
        }
        self.health.in_flight[w].store(-1, Ordering::Release);
        if let Some((seq, result)) = self.slot()? {
            if seq == op_seq {
                if sync {
                    self.barrier_after_unconfirmed(name)?;
                }
                self.replied(op_seq);
                tracing::info!("{name}: op_seq {op_seq} had committed before the disconnect");
                return from_json(&result);
            }
        }
        let conn = self.conn.as_mut().expect("opened");
        match conn.query(name, None, p) {
            Ok(rows) => {
                self.replied(op_seq);
                parse(&rows)
            }
            Err(e) => Err(self.map_err(name, e)),
        }
    }

    /// A synchronous commit returned after a cancel or with a
    /// SyncRepWaitForLSN warning: run the barrier before acknowledging.
    fn barrier_after_unconfirmed(&mut self, name: &str) -> FsResult<()> {
        tracing::warn!("{name}: synchronous replication not confirmed; running pgfs.sync_barrier");
        self.sync_barrier().map_err(|e| FsError::Unexpected {
            errno: libc::EIO,
            error: anyhow!("{name}: sync barrier after unconfirmed commit: {e}"),
        })
    }

    fn verify(
        &self,
        ino: u64,
        block: u64,
        blob: Option<i64>,
        data: &[u8],
        stored: i32,
    ) -> FsResult<()> {
        let actual = crc(data);
        if actual != stored {
            metrics::CHECKSUM_FAILURES.fetch_add(1, Ordering::Relaxed);
            let e = anyhow!(
                "checksum mismatch: tree {} inode {ino} block {block} blob {:?}: expected {:08x}, found {:08x}",
                self.tree,
                blob,
                stored as u32,
                actual as u32
            );
            return Err(FsError::Unexpected {
                errno: libc::EIO,
                error: e,
            });
        }
        Ok(())
    }
}

fn one_attr(rows: &[Row]) -> FsResult<Attr> {
    rows.first()
        .map(|r| attr_from_row(r, 0))
        .ok_or_else(|| FsError::unexpected(anyhow!("function returned no row")))
}

fn json_attr(v: &serde_json::Value) -> FsResult<Attr> {
    attr_from_json(v).ok_or_else(|| FsError::unexpected(anyhow!("unreadable op_log result {v}")))
}

fn epoch(v: &View) -> Option<i64> {
    v.epoch
}

impl PgfsBackend for PgBackend {
    fn backend_pid(&self) -> i32 {
        self.conn.as_ref().map(|c| c.backend_pid).unwrap_or(0)
    }

    fn lookup(&mut self, v: &View, parent: u64, name: &[u8]) -> FsResult<Option<Attr>> {
        let rows = self.read_routed("lookup", parent, &[&v.layer, &epoch(v), &i(parent), &name])?;
        Ok(rows.first().map(|r| attr_from_row(r, 0)))
    }

    fn getattr(&mut self, v: &View, ino: u64, gen: Option<u64>) -> FsResult<Option<Attr>> {
        let gen = gen.map(i);
        let rows = self.read_routed("getattr", ino, &[&v.layer, &epoch(v), &i(ino), &gen])?;
        Ok(rows.first().map(|r| attr_from_row(r, 0)))
    }

    fn readdir(
        &mut self,
        v: &View,
        dir: u64,
        after: Option<&[u8]>,
        limit: u32,
    ) -> FsResult<Vec<DirEntry>> {
        let rows = self.read_routed(
            "readdir",
            dir,
            &[&v.layer, &epoch(v), &i(dir), &after, &(limit as i32)],
        )?;
        Ok(rows
            .iter()
            .map(|r| DirEntry {
                name: r.get(0),
                ino: r.get::<_, i64>(1) as u64,
                kind: Kind::from_char(r.get::<_, i8>(2)),
            })
            .collect())
    }

    fn readdirplus(
        &mut self,
        v: &View,
        dir: u64,
        after: Option<&[u8]>,
        limit: u32,
    ) -> FsResult<Vec<(Vec<u8>, Attr)>> {
        let rows = self.read_routed(
            "readdirplus",
            dir,
            &[&v.layer, &epoch(v), &i(dir), &after, &(limit as i32)],
        )?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), attr_from_row(r, 1)))
            .collect())
    }

    fn read_blocks(
        &mut self,
        v: &View,
        ino: u64,
        gen: Option<u64>,
        first: u64,
        last: u64,
    ) -> FsResult<ReadResult> {
        let gen = gen.map(i);
        let rows = self.read_routed(
            "read_blocks",
            ino,
            &[&v.layer, &epoch(v), &i(ino), &gen, &i(first), &i(last)],
        )?;
        let mut out = ReadResult::default();
        for r in &rows {
            let block = r.get::<_, i64>(0) as u64;
            let data: Vec<u8> = r.get(1);
            let stored: Option<i32> = r.get(2);
            let blob: Option<i64> = r.get(3);
            if r.get::<_, bool>(4) {
                out.inline = true;
            } else if let Some(s) = stored {
                self.verify(ino, block, blob, &data, s)?;
            }
            out.blocks.push((block, data));
        }
        Ok(out)
    }

    fn readlink(&mut self, v: &View, ino: u64) -> FsResult<Vec<u8>> {
        let rows = self.read_routed("readlink", ino, &[&v.layer, &epoch(v), &i(ino)])?;
        Ok(rows
            .first()
            .and_then(|r| r.get::<_, Option<Vec<u8>>>(0))
            .unwrap_or_default())
    }

    fn getxattr(&mut self, v: &View, ino: u64, name: &[u8]) -> FsResult<Option<Vec<u8>>> {
        let rows = self.read_routed("getxattr", ino, &[&v.layer, &epoch(v), &i(ino), &name])?;
        Ok(rows.first().map(|r| r.get(0)))
    }

    fn listxattr(&mut self, v: &View, ino: u64) -> FsResult<Vec<Vec<u8>>> {
        let rows = self.read_routed("listxattr", ino, &[&v.layer, &epoch(v), &i(ino)])?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    fn seek_block(&mut self, v: &View, ino: u64, block: u64, data: bool) -> FsResult<Option<u64>> {
        let rows = self.read_routed(
            "seek_block",
            ino,
            &[&v.layer, &epoch(v), &i(ino), &i(block), &data],
        )?;
        Ok(rows
            .first()
            .and_then(|r| r.get::<_, Option<i64>>(0))
            .map(|b| b as u64))
    }

    fn statfs(&mut self) -> FsResult<Usage> {
        let rows = self.read("fs_usage", None, &[])?;
        Ok(rows
            .first()
            .map(|r| Usage {
                physical_bytes: r.get::<_, i64>(0).max(0) as u64,
                inodes: r.get::<_, i64>(1).max(0) as u64,
            })
            .unwrap_or_default())
    }

    fn make(&mut self, layer: i64, a: &MakeArgs) -> FsResult<Attr> {
        let a2 = a.clone();
        let (stmt, build): (&'static str, BuildParams) = match a.kind {
            Kind::File => (
                "create",
                Box::new(move |m, f, w, s| {
                    vec![
                        Box::new(layer),
                        Box::new(m),
                        Box::new(f),
                        Box::new(w),
                        Box::new(s),
                        Box::new(i(a2.parent)),
                        Box::new(a2.name.clone()),
                        Box::new(a2.mode as i32),
                        Box::new(a2.uid as i64),
                        Box::new(a2.gid as i64),
                        Box::new(a2.now),
                    ]
                }),
            ),
            Kind::Dir => (
                "mkdir",
                Box::new(move |m, f, w, s| {
                    vec![
                        Box::new(layer),
                        Box::new(m),
                        Box::new(f),
                        Box::new(w),
                        Box::new(s),
                        Box::new(i(a2.parent)),
                        Box::new(a2.name.clone()),
                        Box::new(a2.mode as i32),
                        Box::new(a2.uid as i64),
                        Box::new(a2.gid as i64),
                        Box::new(a2.now),
                    ]
                }),
            ),
            Kind::Symlink => (
                "symlink",
                Box::new(move |m, f, w, s| {
                    vec![
                        Box::new(layer),
                        Box::new(m),
                        Box::new(f),
                        Box::new(w),
                        Box::new(s),
                        Box::new(i(a2.parent)),
                        Box::new(a2.name.clone()),
                        Box::new(a2.target.clone().unwrap_or_default()),
                        Box::new(a2.uid as i64),
                        Box::new(a2.gid as i64),
                        Box::new(a2.now),
                    ]
                }),
            ),
            k => (
                "mknod",
                Box::new(move |m, f, w, s| {
                    vec![
                        Box::new(layer),
                        Box::new(m),
                        Box::new(f),
                        Box::new(w),
                        Box::new(s),
                        Box::new(i(a2.parent)),
                        Box::new(a2.name.clone()),
                        Box::new(k.as_char()),
                        Box::new(a2.mode as i32),
                        Box::new(a2.uid as i64),
                        Box::new(a2.gid as i64),
                        Box::new(a2.rdev as i64),
                        Box::new(a2.now),
                    ]
                }),
            ),
        };
        self.mutate(
            stmt,
            Some(a.parent),
            &[a.parent],
            false,
            build,
            one_attr,
            json_attr,
        )
    }

    fn link(
        &mut self,
        layer: i64,
        ino: u64,
        gen: Option<u64>,
        newparent: u64,
        newname: &[u8],
        now: i64,
    ) -> FsResult<Attr> {
        let name = newname.to_vec();
        self.mutate(
            "link",
            Some(ino),
            &[ino, newparent],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(ino)),
                    Box::new(gen.map(i)),
                    Box::new(i(newparent)),
                    Box::new(name.clone()),
                    Box::new(now),
                ]
            },
            one_attr,
            json_attr,
        )
    }

    fn unlink(
        &mut self,
        layer: i64,
        parent: u64,
        name: &[u8],
        open: &[u64],
        dirty: &[u64],
        now: i64,
    ) -> FsResult<UnlinkOutcome> {
        let name = name.to_vec();
        let open: Vec<i64> = open.iter().map(|&x| i(x)).collect();
        let dirty: Vec<i64> = dirty.iter().map(|&x| i(x)).collect();
        self.mutate(
            "unlink",
            Some(parent),
            &[parent],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(parent)),
                    Box::new(name.clone()),
                    Box::new(open.clone()),
                    Box::new(dirty.clone()),
                    Box::new(now),
                ]
            },
            |rows| {
                let r = rows
                    .first()
                    .ok_or_else(|| FsError::unexpected(anyhow!("unlink returned no row")))?;
                Ok(UnlinkOutcome {
                    ino: r.get::<_, i64>(0) as u64,
                    nlink: r.get::<_, i32>(1).max(0) as u32,
                    orphaned: r.get(2),
                })
            },
            |v| {
                Ok(UnlinkOutcome {
                    ino: v.get("ino").and_then(|x| x.as_u64()).unwrap_or(0),
                    nlink: v.get("nlink").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    orphaned: v.get("orphaned").and_then(|x| x.as_bool()).unwrap_or(false),
                })
            },
        )
    }

    fn rmdir(&mut self, layer: i64, parent: u64, name: &[u8], now: i64) -> FsResult<u64> {
        let name = name.to_vec();
        self.mutate(
            "rmdir",
            Some(parent),
            &[parent],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(parent)),
                    Box::new(name.clone()),
                    Box::new(now),
                ]
            },
            |rows| Ok(rows.first().map(|r| r.get::<_, i64>(0) as u64).unwrap_or(0)),
            |v| Ok(v.get("ino").and_then(|x| x.as_u64()).unwrap_or(0)),
        )
    }

    fn rename(&mut self, layer: i64, a: &RenameArgs) -> FsResult<RenameOutcome> {
        let a2 = a.clone();
        let open: Vec<i64> = a.open.iter().map(|&x| i(x)).collect();
        let dirty: Vec<i64> = a.dirty.iter().map(|&x| i(x)).collect();
        self.mutate(
            "rename",
            Some(a.parent),
            &[a.parent, a.newparent],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(a2.parent)),
                    Box::new(a2.name.clone()),
                    Box::new(i(a2.newparent)),
                    Box::new(a2.newname.clone()),
                    Box::new(a2.flags as i32),
                    Box::new(open.clone()),
                    Box::new(dirty.clone()),
                    Box::new(a2.now),
                ]
            },
            |rows| {
                let r = rows
                    .first()
                    .ok_or_else(|| FsError::unexpected(anyhow!("rename returned no row")))?;
                let rep: Option<i64> = r.get(1);
                Ok(RenameOutcome {
                    ino: r.get::<_, i64>(0) as u64,
                    replaced: rep.map(|x| {
                        (
                            x as u64,
                            r.get::<_, Option<i32>>(2).unwrap_or(0).max(0) as u32,
                            r.get::<_, Option<bool>>(3).unwrap_or(false),
                        )
                    }),
                })
            },
            |v| {
                Ok(RenameOutcome {
                    ino: v.get("ino").and_then(|x| x.as_u64()).unwrap_or(0),
                    replaced: v.get("replaced_ino").and_then(|x| x.as_u64()).map(|r| {
                        (
                            r,
                            v.get("replaced_nlink")
                                .and_then(|x| x.as_u64())
                                .unwrap_or(0) as u32,
                            v.get("replaced_orphaned")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(false),
                        )
                    }),
                })
            },
        )
    }

    fn setattr(&mut self, layer: i64, a: &SetAttrArgs) -> FsResult<Attr> {
        let a2 = a.clone();
        self.mutate(
            "setattr",
            Some(a.ino),
            &[a.ino],
            false,
            move |m, f, w, s| {
                let (tb, td, tc) = match &a2.tail {
                    Some((b, d)) => (Some(i(*b)), Some(d.clone()), Some(crc(d))),
                    None => (None, None, None),
                };
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(a2.ino)),
                    Box::new(a2.gen.map(i)),
                    Box::new(a2.mode.map(|x| x as i32)),
                    Box::new(a2.uid.map(|x| x as i64)),
                    Box::new(a2.gid.map(|x| x as i64)),
                    Box::new(a2.size.map(i)),
                    Box::new(tb),
                    Box::new(td),
                    Box::new(tc),
                    Box::new(a2.atime_ns),
                    Box::new(a2.mtime_ns),
                    Box::new(a2.caller_uid.map(|x| x as i64)),
                    Box::new(a2.now),
                ]
            },
            one_attr,
            json_attr,
        )
    }

    fn write_blocks(&mut self, layer: i64, a: &WriteArgs) -> FsResult<Attr> {
        let a2 = a.clone();
        let bytes: u64 = a.blocks.iter().map(|(_, d)| d.len() as u64).sum::<u64>()
            + a.inline.as_ref().map_or(0, |d| d.len() as u64);
        let r = self.mutate(
            "write_blocks",
            Some(a.ino),
            &[a.ino],
            a.sync,
            move |m, f, w, s| {
                let nos: Option<Vec<i64>> =
                    (!a2.blocks.is_empty()).then(|| a2.blocks.iter().map(|(b, _)| i(*b)).collect());
                let datas: Option<Vec<Vec<u8>>> = (!a2.blocks.is_empty())
                    .then(|| a2.blocks.iter().map(|(_, d)| d.clone()).collect());
                let crcs: Option<Vec<i32>> = (!a2.blocks.is_empty())
                    .then(|| a2.blocks.iter().map(|(_, d)| crc(d)).collect());
                let uids: Vec<i64> = a2.writer_uids.iter().map(|&u| u as i64).collect();
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(a2.ino)),
                    Box::new(a2.gen.map(i)),
                    Box::new(i(a2.size)),
                    Box::new(a2.inline.clone()),
                    Box::new(nos),
                    Box::new(datas),
                    Box::new(crcs),
                    Box::new(a2.hole.map(|h| i(h.0))),
                    Box::new(a2.hole.map(|h| i(h.1))),
                    Box::new(a2.mtime_ns),
                    Box::new(a2.now),
                    Box::new(uids),
                    Box::new(a2.sync),
                ]
            },
            one_attr,
            json_attr,
        );
        if r.is_ok() {
            metrics::USER_BYTES_WRITTEN.fetch_add(bytes, Ordering::Relaxed);
        }
        r
    }

    fn copy_range(&mut self, layer: i64, a: &CopyArgs) -> FsResult<Attr> {
        let a2 = a.clone();
        self.mutate(
            "copy_range",
            Some(a.ino_out),
            &[a.ino_out],
            false,
            move |m, f, w, s| {
                let nos: Vec<i64> = a2.blocks.iter().map(|(b, _)| i(*b)).collect();
                let datas: Vec<Vec<u8>> = a2.blocks.iter().map(|(_, d)| d.clone()).collect();
                let crcs: Vec<i32> = a2.blocks.iter().map(|(_, d)| crc(d)).collect();
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(a2.ino_in)),
                    Box::new(a2.gen_in.map(i)),
                    Box::new(i(a2.ino_out)),
                    Box::new(a2.gen_out.map(i)),
                    Box::new(i(a2.src_first)),
                    Box::new(i(a2.dst_first)),
                    Box::new(i(a2.count)),
                    Box::new(nos),
                    Box::new(datas),
                    Box::new(crcs),
                    Box::new(i(a2.size)),
                    Box::new(a2.now),
                ]
            },
            one_attr,
            json_attr,
        )
    }

    fn setxattr(
        &mut self,
        layer: i64,
        ino: u64,
        name: &[u8],
        value: &[u8],
        flags: i32,
        now: i64,
    ) -> FsResult<()> {
        let (name, value) = (name.to_vec(), value.to_vec());
        self.mutate(
            "setxattr",
            Some(ino),
            &[ino],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(ino)),
                    Box::new(name.clone()),
                    Box::new(value.clone()),
                    Box::new(flags),
                    Box::new(now),
                ]
            },
            |_| Ok(()),
            |_| Ok(()),
        )
    }

    fn removexattr(&mut self, layer: i64, ino: u64, name: &[u8], now: i64) -> FsResult<()> {
        let name = name.to_vec();
        self.mutate(
            "removexattr",
            Some(ino),
            &[ino],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(ino)),
                    Box::new(name.clone()),
                    Box::new(now),
                ]
            },
            |_| Ok(()),
            |_| Ok(()),
        )
    }

    fn release_orphan(&mut self, layer: i64, ino: u64) -> FsResult<bool> {
        self.mutate(
            "release_orphan",
            Some(ino),
            &[ino],
            false,
            move |m, f, w, s| {
                vec![
                    Box::new(layer),
                    Box::new(m),
                    Box::new(f),
                    Box::new(w),
                    Box::new(s),
                    Box::new(i(ino)),
                ]
            },
            |rows| {
                Ok(rows
                    .first()
                    .and_then(|r| r.get::<_, Option<bool>>(0))
                    .unwrap_or(false))
            },
            |_| Ok(true),
        )
    }

    fn sync_barrier(&mut self) -> FsResult<()> {
        let Some(id) = self.id else { return Ok(()) };
        self.ensure()?;
        self.op_seq += 1;
        let seq = self.op_seq;
        let conn = self.conn.as_mut().expect("ensured");
        conn.clear_cancel();
        let t0 = Instant::now();
        let r = conn.query("sync_barrier", None, &[&id.mount_id, &id.worker, &seq]);
        match r {
            Ok(_) => {
                if self
                    .conn
                    .as_ref()
                    .is_some_and(|c| c.take_unconfirmed_sync())
                {
                    return Err(FsError::Unexpected {
                        errno: libc::EIO,
                        error: anyhow!(
                            "sync barrier was not confirmed by the synchronous standbys after {:?}",
                            t0.elapsed()
                        ),
                    });
                }
                self.replied(seq);
                Ok(())
            }
            Err(e) => Err(self.map_err("sync_barrier", e)),
        }
    }

    fn snapshot(&mut self, tree: &str, name: &str) -> FsResult<()> {
        let (t, n) = (tree.to_string(), name.to_string());
        let id = self.id;
        self.mutate(
            "snapshot",
            None,
            &[],
            true,
            move |_, _, _, _| {
                vec![
                    Box::new(t.clone()),
                    Box::new(n.clone()),
                    Box::new(id.map(|x| x.mount_id)),
                    Box::new(id.map(|x| x.fence)),
                ]
            },
            |_| Ok(()),
            |_| Ok(()),
        )
    }

    fn branch(&mut self, source: &str, snapshot: Option<&str>, new: &str) -> FsResult<()> {
        let (s, sn, n) = (
            source.to_string(),
            snapshot.map(str::to_string),
            new.to_string(),
        );
        let id = self.id;
        self.mutate(
            "branch",
            None,
            &[],
            true,
            move |_, _, _, _| {
                vec![
                    Box::new(s.clone()),
                    Box::new(sn.clone()),
                    Box::new(n.clone()),
                    Box::new(id.map(|x| x.mount_id)),
                    Box::new(id.map(|x| x.fence)),
                ]
            },
            |_| Ok(()),
            |_| Ok(()),
        )
    }

    fn promote(&mut self, branch: &str, tree: &str, force: bool) -> FsResult<()> {
        let (b, t) = (branch.to_string(), tree.to_string());
        self.mutate(
            "promote",
            None,
            &[],
            true,
            move |_, _, _, _| vec![Box::new(b.clone()), Box::new(t.clone()), Box::new(force)],
            |_| Ok(()),
            |_| Ok(()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_json_round_trip() {
        let v = serde_json::json!({"ino": 7, "gen": 99, "kind": "d", "mode": 493, "uid": 1, "gid": 2,
            "nlink": 2, "size": 0, "blocks": 0, "atime_ns": 1, "mtime_ns": 2, "ctime_ns": 3,
            "btime_ns": 4, "rdev": 0, "version": 5});
        let a = attr_from_json(&v).unwrap();
        assert_eq!(a.ino, 7);
        assert_eq!(a.kind, Kind::Dir);
        assert_eq!(a.version, 5);
    }

    #[test]
    fn crc_is_crc32c() {
        assert_eq!(crc(b"123456789") as u32, 0xe3069283);
    }

    #[test]
    fn every_statement_has_a_unique_name() {
        let s = statements();
        let mut names: Vec<_> = s.iter().map(|(n, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), s.len());
    }
}

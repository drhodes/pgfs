//! Everything that talks to Postgres lives here. The FUSE layer (fs.rs)
//! never sees a SQL statement — it only calls these methods. This is the
//! seam you'd extend later (chunked blocks, indices, search) without
//! touching the filesystem plumbing at all.
//!
//! The tree is keyed by (parent, name) where `parent` is the full path of
//! the containing directory and `name` is the entry's own name. The root
//! directory has parent = "". Names never contain '/', so the full path is
//! unambiguous.
//!
//! Replicated mode: an optional second client to a physical streaming
//! standby (spec/replica.py). Pure reads route to the standby when it has
//! caught up with the primary's WAL; every mutation and every read a
//! mutation depends on stays on the primary.
//!
//! Every method returns `error::Result`, wrapping the underlying
//! `postgres::Error` with a description of what was being attempted and the
//! file:line where it happened, so failures read as a story.

use crate::error::{self, Result};
use postgres::{Client, NoTls, Statement};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tracing::debug_span;

use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;

pub enum DbClient {
    #[allow(dead_code)]
    Direct(Client),
    Pooled(r2d2::PooledConnection<PostgresConnectionManager<NoTls>>),
}

impl std::ops::Deref for DbClient {
    type Target = Client;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Direct(c) => c,
            Self::Pooled(c) => c,
        }
    }
}

impl std::ops::DerefMut for DbClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Direct(c) => c,
            Self::Pooled(c) => c,
        }
    }
}

#[derive(Clone)]
pub struct DbPool {
    primary: Pool<PostgresConnectionManager<NoTls>>,
    replica: Option<Pool<PostgresConnectionManager<NoTls>>>,
}

#[derive(Debug)]
struct DbCustomizer {
    #[allow(dead_code)]
    unlogged: bool,
}

impl r2d2::CustomizeConnection<postgres::Client, postgres::Error> for DbCustomizer {
    fn on_acquire(
        &self,
        client: &mut postgres::Client,
    ) -> std::result::Result<(), postgres::Error> {
        client.batch_execute("SET synchronous_commit = off;")?;
        Ok(())
    }
}

impl DbPool {
    pub fn connect_opts(
        conn_str: &str,
        replica_conn: Option<&str>,
        unlogged: bool,
        max_size: u32,
    ) -> Result<Self> {
        let config: postgres::Config = conn_str.parse().map_err(|e| {
            error::failure(format!("invalid postgres conn string {conn_str:?}: {e}"))
        })?;

        // Initialize schema once per process initialization
        static DDL_INIT: std::sync::Once = std::sync::Once::new();
        DDL_INIT.call_once(|| {
            if let Ok(mut init_client) = config.connect(NoTls) {
                let unlogged_kw = if unlogged { "UNLOGGED " } else { "" };
                let ddl = format!(
                    "BEGIN;
                    CREATE {unlogged_kw}TABLE IF NOT EXISTS entries (
                        ino bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                        parent_ino bigint NOT NULL DEFAULT 1,
                        parent text NOT NULL DEFAULT '',
                        name   text NOT NULL,
                        kind   text NOT NULL DEFAULT 'file',
                        data   bytea NOT NULL DEFAULT '',
                        size   bigint NOT NULL DEFAULT 0,
                        mode   integer NOT NULL DEFAULT 33188,
                        uid    integer NOT NULL DEFAULT 1000,
                        gid    integer NOT NULL DEFAULT 1000,
                        atime  timestamptz NOT NULL DEFAULT now(),
                        mtime  timestamptz NOT NULL DEFAULT now(),
                        ctime  timestamptz NOT NULL DEFAULT now(),
                        CONSTRAINT unique_parent_name UNIQUE (parent, name)
                    );
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS ino bigint GENERATED ALWAYS AS IDENTITY;
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS parent_ino bigint NOT NULL DEFAULT 1;
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS mode integer NOT NULL DEFAULT 33188;
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS uid integer NOT NULL DEFAULT 1000;
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS gid integer NOT NULL DEFAULT 1000;
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS atime timestamptz NOT NULL DEFAULT now();
                    ALTER TABLE entries ADD COLUMN IF NOT EXISTS ctime timestamptz NOT NULL DEFAULT now();

                    DROP INDEX IF EXISTS idx_entries_parent_ino_name;
                    CREATE INDEX IF NOT EXISTS idx_entries_parent_ino ON entries(parent_ino);
                    CREATE INDEX IF NOT EXISTS idx_entries_parent ON entries(parent);

                    CREATE {unlogged_kw}TABLE IF NOT EXISTS blocks (
                        ino      bigint NOT NULL DEFAULT 1,
                        parent   text NOT NULL DEFAULT '',
                        name     text NOT NULL DEFAULT '',
                        block_no integer NOT NULL,
                        data     bytea NOT NULL,
                        PRIMARY KEY (parent, name, block_no)
                    );
                    ALTER TABLE blocks ADD COLUMN IF NOT EXISTS ino bigint NOT NULL DEFAULT 1;
                    DROP INDEX IF EXISTS idx_blocks_ino_block_no;
                    CREATE INDEX IF NOT EXISTS idx_blocks_ino ON blocks(ino, block_no);

                    INSERT INTO entries (ino, parent_ino, parent, name, kind, size, mode, uid, gid)
                    OVERRIDING SYSTEM VALUE
                    VALUES (1, 1, '', '', 'dir', 0, 16877, 1000, 1000)
                    ON CONFLICT (parent, name) DO NOTHING;

                    COMMIT;"
                );
                if let Err(e) = init_client.batch_execute(&ddl) {
                    eprintln!("DDL INIT ERROR: {e:?}");
                }
            }
        });

        let manager = PostgresConnectionManager::new(config, NoTls);
        let primary = r2d2::Pool::builder()
            .max_size(max_size)
            .connection_customizer(Box::new(DbCustomizer { unlogged }))
            .build(manager)
            .map_err(|e| error::failure(format!("failed to build primary connection pool: {e}")))?;

        let replica = match replica_conn {
            Some(s) => match s.parse() {
                Ok(c) => {
                    let m = PostgresConnectionManager::new(c, NoTls);
                    r2d2::Pool::builder().max_size(max_size).build(m).ok()
                }
                Err(_) => None,
            },
            None => None,
        };

        Ok(Self { primary, replica })
    }

    pub fn get(&self) -> Result<Db> {
        let mut client = error::ctx(self.primary.get(), "acquire primary connection from pool")?;

        let stmt_getattr = error::ctx(
            client.prepare(
                "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 AND name = $2",
            ),
            "prepare stmt_getattr",
        )?;
        let stmt_create = error::ctx(
            client.prepare("INSERT INTO entries (parent, name, kind, data, size) VALUES ($1, $2, 'file', '', 0) ON CONFLICT (parent, name) DO NOTHING"),
            "prepare stmt_create",
        )?;
        let stmt_mkdir = error::ctx(
            client.prepare("INSERT INTO entries (parent, name, kind, data, size) VALUES ($1, $2, 'dir', '', 0) ON CONFLICT (parent, name) DO NOTHING"),
            "prepare stmt_mkdir",
        )?;
        let stmt_unlink = error::ctx(
            client.prepare("DELETE FROM entries WHERE parent = $1 AND name = $2"),
            "prepare stmt_unlink",
        )?;
        let stmt_rmdir = error::ctx(
            client.prepare("DELETE FROM entries WHERE parent = $1 AND name = $2"),
            "prepare stmt_rmdir",
        )?;
        let stmt_list = error::ctx(
            client.prepare(
                "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 AND name != '' ORDER BY name",
            ),
            "prepare stmt_list",
        )?;
        let stmt_read_blocks = error::ctx(
            client.prepare("SELECT block_no, data FROM blocks WHERE parent = $1 AND name = $2 AND block_no >= $3 AND block_no <= $4 ORDER BY block_no"),
            "prepare stmt_read_blocks",
        )?;
        let stmt_upsert_block = error::ctx(
            client.prepare("INSERT INTO blocks (parent, name, block_no, data) VALUES ($1, $2, $3, $4) ON CONFLICT (parent, name, block_no) DO UPDATE SET data = EXCLUDED.data"),
            "prepare stmt_upsert_block",
        )?;
        let stmt_prune_blocks = error::ctx(
            client.prepare("DELETE FROM blocks WHERE parent = $1 AND name = $2 AND block_no > $3"),
            "prepare stmt_prune_blocks",
        )?;
        let stmt_delete_blocks = error::ctx(
            client.prepare("DELETE FROM blocks WHERE parent = $1 AND name = $2"),
            "prepare stmt_delete_blocks",
        )?;

        let stmt_lookup_ino = error::ctx(
            client.prepare("SELECT ino, size, kind, mode, uid, gid, atime, mtime, ctime FROM entries WHERE parent_ino = $1 AND name = $2"),
            "prepare stmt_lookup_ino",
        )?;
        let stmt_getattr_ino = error::ctx(
            client.prepare("SELECT ino, size, kind, mode, uid, gid, atime, mtime, ctime FROM entries WHERE ino = $1"),
            "prepare stmt_getattr_ino",
        )?;
        let stmt_read_block_ino = error::ctx(
            client.prepare("SELECT data FROM blocks WHERE ino = $1 AND block_no = $2"),
            "prepare stmt_read_block_ino",
        )?;
        let stmt_upsert_block_ino = error::ctx(
            client.prepare("INSERT INTO blocks (ino, parent, name, block_no, data) VALUES ($1::bigint, '', $1::text, $2, $3) ON CONFLICT (parent, name, block_no) DO UPDATE SET data = EXCLUDED.data"),
            "prepare stmt_upsert_block_ino",
        )?;
        let stmt_prune_blocks_ino = error::ctx(
            client.prepare("DELETE FROM blocks WHERE ino = $1 AND block_no > $2"),
            "prepare stmt_prune_blocks_ino",
        )?;
        let stmt_rename_ino = error::ctx(
            client.prepare("UPDATE entries SET parent_ino = $3, name = $4, ctime = NOW() WHERE parent_ino = $1 AND name = $2"),
            "prepare stmt_rename_ino",
        )?;

        let replica = match &self.replica {
            Some(p) => p.get().ok(),
            None => None,
        };

        Ok(Db {
            client: Arc::new(std::sync::Mutex::new(DbClient::Pooled(client))),
            replica: replica.map(|r| Arc::new(std::sync::Mutex::new(DbClient::Pooled(r)))),
            stmt_getattr,
            stmt_create,
            stmt_mkdir,
            stmt_unlink,
            stmt_rmdir,
            stmt_list,
            stmt_read_blocks,
            stmt_upsert_block,
            stmt_prune_blocks,
            stmt_delete_blocks,
            stmt_lookup_ino,
            stmt_getattr_ino,
            stmt_read_block_ino,
            stmt_upsert_block_ino,
            stmt_prune_blocks_ino,
            stmt_rename_ino,
            freshness_cached: false,
            freshness_checked_at: None,
        })
    }
}

#[derive(Clone)]
pub struct Db {
    /// Primary connection — every write goes here.
    client: Arc<std::sync::Mutex<DbClient>>,

    /// Optional connection to a read replica.
    replica: Option<Arc<std::sync::Mutex<DbClient>>>,

    // Pre-compiled SQL statement handles on the primary connection.
    stmt_getattr: Statement,
    stmt_create: Statement,
    stmt_mkdir: Statement,
    stmt_unlink: Statement,
    stmt_rmdir: Statement,
    stmt_list: Statement,

    stmt_read_blocks: Statement,
    stmt_upsert_block: Statement,
    stmt_prune_blocks: Statement,
    stmt_delete_blocks: Statement,

    #[allow(dead_code)]
    stmt_lookup_ino: Statement,
    #[allow(dead_code)]
    stmt_getattr_ino: Statement,
    #[allow(dead_code)]
    stmt_read_block_ino: Statement,
    #[allow(dead_code)]
    stmt_upsert_block_ino: Statement,
    #[allow(dead_code)]
    stmt_prune_blocks_ino: Statement,
    #[allow(dead_code)]
    stmt_rename_ino: Statement,

    /// Cached replica freshness verdict and monotonic timestamp of last check (millis).
    pub freshness_cached: bool,
    pub freshness_checked_at: Option<u64>,
}

pub const BLOCK_SIZE: usize = 64 * 1024;
const FRESHNESS_TTL_MS: u64 = 500;

fn now_mono_millis() -> u64 {
    static BOOT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    BOOT.get_or_init(Instant::now).elapsed().as_millis() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    pub mtime: SystemTime,
}

/// Full path of an entry given its (parent, name) key.
pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

impl Db {
    /// Connects and makes sure the schema exists. `conn_str` is a normal
    /// libpq-style connection string, e.g.
    /// "host=/path/to/project/testdata dbname=pgfs" for a local Unix socket.
    ///
    /// `replica_conn` is an optional libpq string for a physical streaming
    /// standby. When present, read operations (getattr/list/read) are served
    /// from the standby once it has replayed WAL up to the primary's current
    /// position; writes always go to the primary. A failure to reach the
    /// replica is not fatal: reads fall back to the primary (see
    /// `FallsBackToPrimary` in spec/replica.py).
    #[allow(dead_code)]
    pub fn connect(conn_str: &str, replica_conn: Option<&str>) -> Result<Self> {
        Self::connect_opts(conn_str, replica_conn, false)
    }

    pub fn connect_opts(
        conn_str: &str,
        replica_conn: Option<&str>,
        unlogged: bool,
    ) -> Result<Self> {
        let pool = DbPool::connect_opts(conn_str, replica_conn, unlogged, 10)?;
        pool.get()
    }

    fn client_guard(&self) -> std::sync::MutexGuard<'_, DbClient> {
        self.client.lock().unwrap()
    }

    fn replica_guard(&self) -> Option<std::sync::MutexGuard<'_, DbClient>> {
        self.replica.as_ref().map(|r| r.lock().unwrap())
    }

    /// Is the replica fresh enough to serve reads? True iff a replica is
    /// configured, reachable, and its last replayed WAL position is at or
    /// ahead of the primary's current position. A standby that has never
    /// replayed anything, or any error on either node, means not fresh.
    fn replica_fresh(&self) -> bool {
        let Some(mut replica) = self.replica_guard() else {
            return false;
        };

        // A node that is not in recovery is itself a primary: authoritative.
        let in_recovery: Option<bool> = replica
            .query_opt("SELECT pg_is_in_recovery()", &[])
            .ok()
            .flatten()
            .map(|r| r.get(0));
        match in_recovery {
            Some(false) => return true,
            None => return false, // replica query failed
            Some(true) => {}
        }

        let primary_lsn: Option<i64> = self
            .client_guard()
            .query_opt("SELECT (pg_current_wal_lsn() - '0/0'::pg_lsn)::bigint", &[])
            .ok()
            .flatten()
            .map(|r| r.get(0));
        let replay_lsn: Option<Option<i64>> = replica
            .query_opt(
                "SELECT (pg_last_wal_replay_lsn() - '0/0'::pg_lsn)::bigint",
                &[],
            )
            .ok()
            .flatten()
            .map(|r| r.get(0));

        match (primary_lsn, replay_lsn) {
            (Some(p), Some(Some(r))) => r >= p,
            _ => false,
        }
    }

    /// Cached replica freshness check. Reuses the cached verdict if checked
    /// within FRESHNESS_TTL_MS to eliminate 3-query pre-check overhead.
    pub fn replica_fresh_cached(&mut self) -> bool {
        let now = now_mono_millis();
        if let Some(last_check) = self.freshness_checked_at {
            if now.saturating_sub(last_check) < FRESHNESS_TTL_MS {
                return self.freshness_cached;
            }
        }
        let fresh = self.replica_fresh();
        self.freshness_cached = fresh;
        self.freshness_checked_at = Some(now);
        fresh
    }

    /// Human-readable replica health for the SIGUSR1 state dump
    /// (spec/replica.py ReplicaObservability: the SIGUSR1 state dump
    /// reports replica freshness). One of: "none" (no --replica),
    /// "fresh", or "stale or unreachable".
    pub fn replica_state(&mut self) -> String {
        if self.replica.is_none() {
            return "none".to_string();
        }
        if self.replica_fresh_cached() {
            "fresh".to_string()
        } else {
            "stale or unreachable".to_string()
        }
    }

    /// getattr that is forced to read the primary. Used by mutation
    /// decision paths (create/mkdir EEXIST checks, rmdir existence,
    /// rename's source/target checks) so a stale replica can never
    /// influence a write's outcome.
    pub fn getattr_primary(&mut self, parent: &str, name: &str) -> Result<Option<FileMeta>> {
        let _span = debug_span!("db::getattr", parent, name).entered();
        let _t0 = Instant::now();
        let row = error::ctx(
            self.client_guard()
                .query_opt(&self.stmt_getattr, &[&parent, &name]),
            &format!("get attributes of entry {name:?} in {parent:?}"),
        )?;
        let result = row.as_ref().map(row_to_meta).transpose();
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        result
    }

    /// list that is forced to read the primary. Used by rename's
    /// target-emptiness check so a stale replica can never let a rename
    /// overwrite a directory that is actually non-empty on the primary.
    pub fn list_primary(&mut self, parent: &str) -> Result<Vec<FileMeta>> {
        let _span = debug_span!("db::list", parent).entered();
        let _t0 = Instant::now();
        let rows = error::ctx(
            self.client_guard().query(&self.stmt_list, &[&parent]),
            &format!("list children of directory {parent:?}"),
        )?;
        let result: Result<Vec<FileMeta>> = rows.iter().map(row_to_meta).collect();
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        result
    }

    /// Children of a directory, sorted by name. Ordering is cosmetic here;
    /// fs.rs re-iterates this on every readdir.
    pub fn list(&mut self, parent: &str) -> Result<Vec<FileMeta>> {
        if self.replica.is_some() && self.replica_fresh_cached() {
            crate::metrics::REPLICA_READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut replica = self.replica_guard().expect("replica present");
            let _span = debug_span!("db::list", parent).entered();
            let _t0 = Instant::now();
            let rows = error::ctx(
                replica.query(
                    "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 AND name != '' ORDER BY name",
                    &[&parent],
                ),
                &format!("list children of directory {parent:?}"),
            )?;
            let result: Result<Vec<FileMeta>> = rows.iter().map(row_to_meta).collect();
            crate::metrics::DB_LATENCY.record(_t0.elapsed());
            result
        } else {
            self.record_replica_fallback_if_configured();
            self.list_primary(parent)
        }
    }

    pub fn getattr(&mut self, parent: &str, name: &str) -> Result<Option<FileMeta>> {
        if self.replica.is_some() && self.replica_fresh_cached() {
            crate::metrics::REPLICA_READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut replica = self.replica_guard().expect("replica present");
            let _span = debug_span!("db::getattr", parent, name).entered();
            let _t0 = Instant::now();
            let row = error::ctx(
                replica.query_opt(
                    "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 AND name = $2",
                    &[&parent, &name],
                ),
                &format!("get attributes of entry {name:?} in {parent:?}"),
            )?;
            let result = row.as_ref().map(row_to_meta).transpose();
            crate::metrics::DB_LATENCY.record(_t0.elapsed());
            result
        } else {
            self.record_replica_fallback_if_configured();
            self.getattr_primary(parent, name)
        }
    }

    fn record_replica_fallback_if_configured(&mut self) {
        if self.replica.is_some() {
            crate::metrics::REPLICA_FALLBACK_COUNT
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if crate::metrics::REPLICA_FALLBACK_WARNED
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                tracing::warn!("reads falling back to primary (standby stale or unreachable)");
            } else {
                tracing::debug!("read served from primary (replica stale)");
            }
        }
    }

    pub fn read_range_primary(
        &mut self,
        parent: &str,
        name: &str,
        offset: u64,
        len: usize,
    ) -> Result<Option<Vec<u8>>> {
        let _span = debug_span!("db::read_range", parent, name, offset, len).entered();
        let _t0 = Instant::now();
        let meta = match self.getattr_primary(parent, name)? {
            Some(m) if m.kind == Kind::File => m,
            _ => return Ok(None),
        };
        if offset >= meta.size {
            return Ok(Some(Vec::new()));
        }
        let actual_len = (len as u64).min(meta.size - offset) as usize;
        if actual_len == 0 {
            return Ok(Some(Vec::new()));
        }
        let start_block = (offset / BLOCK_SIZE as u64) as i32;
        let end_block = ((offset + actual_len as u64 - 1) / BLOCK_SIZE as u64) as i32;

        let rows = error::ctx(
            self.client_guard().query(
                &self.stmt_read_blocks,
                &[&parent, &name, &start_block, &end_block],
            ),
            &format!("read blocks for {name:?} in {parent:?}"),
        )?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            let b_no: i32 = row.get("block_no");
            let b_data: Vec<u8> = row.get("data");
            map.insert(b_no, b_data);
        }

        let mut result = Vec::with_capacity(actual_len);
        let mut current_offset = offset;
        let end_offset = offset + actual_len as u64;

        while current_offset < end_offset {
            let block_no = (current_offset / BLOCK_SIZE as u64) as i32;
            let block_start = (block_no as u64) * (BLOCK_SIZE as u64);
            let block_offset = (current_offset - block_start) as usize;
            let bytes_in_block =
                ((block_start + BLOCK_SIZE as u64).min(end_offset) - current_offset) as usize;

            if let Some(b_data) = map.get(&block_no) {
                let slice_end = (block_offset + bytes_in_block).min(b_data.len());
                if block_offset < b_data.len() {
                    result.extend_from_slice(&b_data[block_offset..slice_end]);
                    if slice_end < block_offset + bytes_in_block {
                        result.resize(
                            result.len() + (block_offset + bytes_in_block - slice_end),
                            0,
                        );
                    }
                } else {
                    result.resize(result.len() + bytes_in_block, 0);
                }
            } else {
                result.resize(result.len() + bytes_in_block, 0);
            }
            current_offset += bytes_in_block as u64;
        }
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(Some(result))
    }

    pub fn read_range(
        &mut self,
        parent: &str,
        name: &str,
        offset: u64,
        len: usize,
    ) -> Result<Option<Vec<u8>>> {
        self.record_replica_fallback_if_configured();
        self.read_range_primary(parent, name, offset, len)
    }

    pub fn write_range(
        &mut self,
        parent: &str,
        name: &str,
        offset: u64,
        data: &[u8],
    ) -> Result<()> {
        let _span =
            debug_span!("db::write_range", parent, name, offset, len = data.len()).entered();
        let _t0 = Instant::now();
        if data.is_empty() {
            return Ok(());
        }
        let start_block = (offset / BLOCK_SIZE as u64) as i32;
        let end_block = ((offset + data.len() as u64 - 1) / BLOCK_SIZE as u64) as i32;

        let meta = self.getattr_primary(parent, name)?;
        let old_size = meta.as_ref().map(|m| m.size).unwrap_or(0);
        let new_size = old_size.max(offset + data.len() as u64);

        let mut client = self.client_guard();
        let mut txn = error::ctx(
            client.transaction(),
            &format!("begin write_range transaction for {name:?} in {parent:?}"),
        )?;

        for b_no in start_block..=end_block {
            let block_start = (b_no as u64) * (BLOCK_SIZE as u64);
            let block_end = block_start + BLOCK_SIZE as u64;

            let write_start = offset.max(block_start);
            let write_end = (offset + data.len() as u64).min(block_end);

            let in_data_start = (write_start - offset) as usize;
            let in_data_end = (write_end - offset) as usize;
            let chunk = &data[in_data_start..in_data_end];

            let block_offset = (write_start - block_start) as usize;

            let mut block_data = if block_offset > 0 || chunk.len() < BLOCK_SIZE {
                let rows = error::ctx(
                    txn.query(&self.stmt_read_blocks, &[&parent, &name, &b_no, &b_no]),
                    &format!("read block {b_no} inside write_range transaction"),
                )?;
                rows.first()
                    .map(|r| r.get::<_, Vec<u8>>("data"))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            if block_data.len() < block_offset + chunk.len() {
                block_data.resize(block_offset + chunk.len(), 0);
            }
            block_data[block_offset..block_offset + chunk.len()].copy_from_slice(chunk);

            error::ctx(
                txn.execute(
                    &self.stmt_upsert_block,
                    &[&parent, &name, &b_no, &block_data],
                ),
                &format!("upsert block {b_no} for {name:?} in {parent:?}"),
            )?;
        }

        error::ctx(
            txn.execute(
                "UPDATE entries SET size = $3, mtime = now() WHERE parent = $1 AND name = $2",
                &[&parent, &name, &(new_size as i64)],
            ),
            &format!("update size of {name:?} in {parent:?}"),
        )?;
        error::ctx(
            txn.commit(),
            &format!("commit write_range transaction for {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn truncate(&mut self, parent: &str, name: &str, new_size: u64) -> Result<()> {
        let _span = debug_span!("db::truncate", parent, name, new_size).entered();
        let _t0 = Instant::now();
        let meta = match self.getattr_primary(parent, name)? {
            Some(m) if m.kind == Kind::File => m,
            _ => {
                return Err(error::failure(format!(
                    "truncate target {name:?} in {parent:?} not a file"
                )))
            }
        };

        let mut client = self.client_guard();
        let mut txn = error::ctx(
            client.transaction(),
            &format!("begin truncate transaction for {name:?} in {parent:?}"),
        )?;

        if new_size < meta.size {
            let last_valid_block = (new_size / BLOCK_SIZE as u64) as i32;
            error::ctx(
                txn.execute(
                    &self.stmt_prune_blocks,
                    &[&parent, &name, &last_valid_block],
                ),
                &format!("prune blocks past {new_size} for {name:?} in {parent:?}"),
            )?;

            let block_rem = (new_size % BLOCK_SIZE as u64) as usize;
            if block_rem > 0 {
                let rows = error::ctx(
                    txn.query(
                        &self.stmt_read_blocks,
                        &[&parent, &name, &last_valid_block, &last_valid_block],
                    ),
                    &format!("read boundary block {last_valid_block} inside truncate transaction"),
                )?;
                if let Some(row) = rows.first() {
                    let mut block_data: Vec<u8> = row.get("data");
                    block_data.truncate(block_rem);
                    error::ctx(
                        txn.execute(
                            &self.stmt_upsert_block,
                            &[&parent, &name, &last_valid_block, &block_data],
                        ),
                        &format!("patch boundary block for {name:?} in {parent:?}"),
                    )?;
                }
            }
        }

        error::ctx(
            txn.execute(
                "UPDATE entries SET size = $3, mtime = now() WHERE parent = $1 AND name = $2",
                &[&parent, &name, &(new_size as i64)],
            ),
            &format!("update size of {name:?} in {parent:?}"),
        )?;
        error::ctx(
            txn.commit(),
            &format!("commit truncate transaction for {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn write_blocks_batch(
        &mut self,
        parent: &str,
        name: &str,
        blocks: &std::collections::HashMap<i32, Vec<u8>>,
        new_size: u64,
    ) -> Result<()> {
        let _span = debug_span!(
            "db::write_blocks_batch",
            parent,
            name,
            count = blocks.len(),
            new_size
        )
        .entered();
        let _t0 = Instant::now();
        if blocks.is_empty() && new_size == 0 {
            return Ok(());
        }

        let meta = if new_size > 0 {
            self.getattr_primary(parent, name)?
        } else {
            None
        };

        let mut client = self.client_guard();
        let mut txn = error::ctx(
            client.transaction(),
            &format!("begin write_blocks_batch for {name:?} in {parent:?}"),
        )?;

        for (b_no, block_data) in blocks {
            error::ctx(
                txn.execute(&self.stmt_upsert_block, &[&parent, &name, b_no, block_data]),
                &format!("upsert block {b_no} for {name:?} in {parent:?}"),
            )?;
        }

        if new_size > 0 {
            let old_size = meta.as_ref().map(|m| m.size).unwrap_or(0);
            let updated_size = old_size.max(new_size);

            error::ctx(
                txn.execute(
                    "UPDATE entries SET size = $3, mtime = now() WHERE parent = $1 AND name = $2",
                    &[&parent, &name, &(updated_size as i64)],
                ),
                &format!("update size of {name:?} in {parent:?}"),
            )?;
        }

        error::ctx(
            txn.commit(),
            &format!("commit write_blocks_batch for {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    #[allow(dead_code)]
    pub fn read(&mut self, parent: &str, name: &str) -> Result<Option<Vec<u8>>> {
        self.read_range(parent, name, 0, usize::MAX)
    }

    #[allow(dead_code)]
    pub fn read_primary(&mut self, parent: &str, name: &str) -> Result<Option<Vec<u8>>> {
        self.read_range_primary(parent, name, 0, usize::MAX)
    }

    #[allow(dead_code)]
    pub fn write(&mut self, parent: &str, name: &str, data: &[u8]) -> Result<()> {
        self.truncate(parent, name, 0)?;
        self.write_range(parent, name, 0, data)
    }

    pub fn create(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::create", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client_guard()
                .execute(&self.stmt_create, &[&parent, &name]),
            &format!("create file {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn mkdir(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::mkdir", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client_guard()
                .execute(&self.stmt_mkdir, &[&parent, &name]),
            &format!("create directory {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn unlink(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::unlink", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client_guard()
                .execute(&self.stmt_unlink, &[&parent, &name]),
            &format!("remove file {name:?} from {parent:?}"),
        )?;
        error::ctx(
            self.client_guard()
                .execute(&self.stmt_delete_blocks, &[&parent, &name]),
            &format!("remove blocks of file {name:?} from {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    /// Rename (move) an entry. Follows POSIX semantics: if the target
    /// already exists it is atomically replaced within a transaction.
    /// Replacing a non-empty directory returns an error (ENOTEMPTY in
    /// fs.rs). The caller is responsible for source-existence and
    /// kind-compatibility checks (e.g. file → dir / dir → file).
    pub fn rename(
        &mut self,
        old_parent: &str,
        old_name: &str,
        new_parent: &str,
        new_name: &str,
    ) -> Result<()> {
        let _span = debug_span!("db::rename", old_parent, old_name, new_parent, new_name).entered();
        let _t0 = Instant::now();
        let old_path = join(old_parent, old_name);
        let new_path = join(new_parent, new_name);

        // fs.rs handles this before calling us, but be defensive.
        if old_parent == new_parent && old_name == new_name {
            return Ok(());
        }

        // Source-existence check must read the PRIMARY: the decision feeds
        // a write transaction, so a stale replica must never influence it.
        let meta = self
            .getattr_primary(old_parent, old_name)?
            .ok_or_else(|| error::failure(format!("rename source {old_path:?} does not exist")))?;

        let mut client = self.client_guard();
        let mut txn = error::ctx(
            client.transaction(),
            &format!("begin rename of {old_path:?} -> {new_path:?}"),
        )?;

        // POSIX: atomically replace an existing target.
        if let Some(target_row) = error::ctx(
            txn.query_opt(
                "SELECT kind FROM entries WHERE parent = $1 AND name = $2",
                &[&new_parent, &new_name],
            ),
            &format!("check target existence for rename to {new_path:?}"),
        )? {
            let target_kind: String = target_row.get("kind");
            if target_kind == "dir" {
                let has_children = error::ctx(
                    txn.query_opt(
                        "SELECT 1 FROM entries WHERE parent = $1 LIMIT 1",
                        &[&new_path],
                    ),
                    &format!("check whether target directory {new_path:?} is empty"),
                )?;
                if has_children.is_some() {
                    return Err(error::failure(format!(
                        "rename target directory {new_path:?} is not empty"
                    )));
                }
            }
            error::ctx(
                txn.execute(
                    "DELETE FROM entries WHERE parent = $1 AND name = $2",
                    &[&new_parent, &new_name],
                ),
                &format!("remove target {new_path:?} before overwrite"),
            )?;
            error::ctx(
                txn.execute(
                    "DELETE FROM blocks WHERE parent = $1 AND name = $2",
                    &[&new_parent, &new_name],
                ),
                &format!("remove target blocks for {new_path:?} before overwrite"),
            )?;
        }

        if meta.kind == Kind::Dir {
            error::ctx(
                txn.execute(
                    "UPDATE entries
                     SET parent = $1 || substring(parent FROM $2 + 1)
                     WHERE parent = $3 OR parent LIKE $3 || '/%'",
                    &[&new_path, &(old_path.len() as i32), &old_path],
                ),
                &format!("rewrite descendant parent paths for directory {old_path:?}"),
            )?;
            error::ctx(
                txn.execute(
                    "UPDATE blocks
                     SET parent = $1 || substring(parent FROM $2 + 1)
                     WHERE parent = $3 OR parent LIKE $3 || '/%'",
                    &[&new_path, &(old_path.len() as i32), &old_path],
                ),
                &format!("rewrite descendant block parent paths for directory {old_path:?}"),
            )?;
        }

        let moved = error::ctx(
            txn.execute(
                "UPDATE entries SET parent = $1, name = $2, mtime = now()
                 WHERE parent = $3 AND name = $4",
                &[&new_parent, &new_name, &old_parent, &old_name],
            ),
            &format!("move entry {old_path:?} -> {new_path:?}"),
        )?;

        error::ctx(
            txn.execute(
                "UPDATE blocks SET parent = $1, name = $2
                 WHERE parent = $3 AND name = $4",
                &[&new_parent, &new_name, &old_parent, &old_name],
            ),
            &format!("move blocks {old_path:?} -> {new_path:?}"),
        )?;

        if moved == 0 {
            return Err(error::failure(format!(
                "rename of {old_path:?} updated zero rows"
            )));
        }

        error::ctx(
            txn.commit(),
            &format!("commit rename of {old_path:?} -> {new_path:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    /// Removes an empty directory. Returns Ok(true) if deleted, Ok(false) if
    /// it does not exist or still has children — the caller distinguishes
    /// those two cases by checking `getattr` first.
    pub fn rmdir(&mut self, parent: &str, name: &str) -> Result<bool> {
        let _span = debug_span!("db::rmdir", parent, name).entered();
        let _t0 = Instant::now();
        let path = join(parent, name);
        let has_children = error::ctx(
            self.client_guard()
                .query_opt("SELECT 1 FROM entries WHERE parent = $1 LIMIT 1", &[&path]),
            &format!("check whether {name:?} in {parent:?} is empty"),
        )?;
        if has_children.is_some() {
            crate::metrics::DB_LATENCY.record(_t0.elapsed());
            return Ok(false);
        }
        let deleted = error::ctx(
            self.client_guard()
                .execute(&self.stmt_rmdir, &[&parent, &name]),
            &format!("remove directory {name:?} from {parent:?}"),
        )?;

        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(deleted > 0)
    }
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    pub(super) fn db_connect() -> Db {
        let conn_str = format!(
            "host={}/testdata dbname=pgfs",
            std::env::current_dir().unwrap().display()
        );
        Db::connect(&conn_str, None).expect("connect")
    }

    /// Create a unique root-level directory for test isolation and return its path.
    fn root_dir(db: &mut Db) -> String {
        let id = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = format!("_t_{}_{}", std::process::id(), id);
        db.mkdir("", &dir).expect("create test namespace dir");
        dir
    }

    /// Clean up a root-level test directory and all its children.
    fn cleanup(db: &mut Db, root: &str) {
        // Remove children first (files and subdirs recursively).
        fn remove_all(db: &mut Db, parent: &str) {
            if let Ok(children) = db.list(parent) {
                for child in children {
                    let child_path = join(parent, &child.name);
                    match child.kind {
                        Kind::File => {
                            db.unlink(parent, &child.name).ok();
                        }
                        Kind::Dir => {
                            remove_all(db, &child_path);
                            db.rmdir(parent, &child.name).ok();
                        }
                    }
                }
            }
        }
        remove_all(db, root);
        db.rmdir("", root).ok();
    }

    // ── Connection ──────────────────────────────────────────────────────

    #[test]
    fn connect_and_schema_exists() {
        let mut db = db_connect();
        let result = db.list("");
        // The entries table exists; listing root should succeed.
        assert!(result.is_ok(), "list root should work: {:?}", result.err());
    }

    #[test]
    fn connect_disables_synchronous_commit() {
        let db = db_connect();
        let row = db
            .client_guard()
            .query_one("SHOW synchronous_commit;", &[])
            .expect("query synchronous_commit");
        let val: String = row.get(0);
        assert_eq!(
            val, "off",
            "synchronous_commit must be tuned off on connection"
        );
    }

    #[test]
    fn connect_with_unlogged_tables() {
        let conn_str = format!(
            "host={}/testdata dbname=pgfs",
            std::env::current_dir().unwrap().display()
        );
        let db = Db::connect_opts(&conn_str, None, true);
        assert!(
            db.is_ok(),
            "connect_opts with unlogged = true must succeed: {:?}",
            db.err()
        );
    }

    #[test]
    fn replica_freshness_caches_verdict() {
        let mut db = db_connect();
        // Without replica, replica_fresh_cached returns false and sets cache timestamp.
        assert!(!db.replica_fresh_cached());
        let first_check = db.freshness_checked_at;
        assert!(
            first_check.is_some(),
            "freshness timestamp must be initialized"
        );
        // Second immediate call within TTL should return cached verdict without updating timestamp.
        assert!(!db.replica_fresh_cached());
        assert_eq!(db.freshness_checked_at, first_check);
    }

    #[test]
    fn block_storage_partial_write_and_read_range() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "block.bin").unwrap();
        db.write_range(&root, "block.bin", 0, b"Hello ").unwrap();
        db.write_range(&root, "block.bin", 6, b"World!").unwrap();

        let full = db.read_range(&root, "block.bin", 0, 12).unwrap().unwrap();
        assert_eq!(full, b"Hello World!");

        let slice = db.read_range(&root, "block.bin", 6, 5).unwrap().unwrap();
        assert_eq!(slice, b"World");

        cleanup(&mut db, &root);
    }

    #[test]
    fn block_storage_truncate_prunes_blocks() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "trunc.bin").unwrap();
        let payload = vec![1u8; 100 * 1024]; // 100 KB spanning two 64KB blocks
        db.write_range(&root, "trunc.bin", 0, &payload).unwrap();

        db.truncate(&root, "trunc.bin", 10).unwrap();
        let truncated = db.read_range(&root, "trunc.bin", 0, 100).unwrap().unwrap();
        assert_eq!(truncated, vec![1u8; 10]);

        cleanup(&mut db, &root);
    }

    #[test]
    fn transaction_batching_multi_block_write() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "tx_batch.bin").unwrap();
        let payload = vec![42u8; 150 * 1024]; // 150 KB across 3 blocks
        db.write_range(&root, "tx_batch.bin", 0, &payload).unwrap();

        let read_back = db
            .read_range(&root, "tx_batch.bin", 0, payload.len())
            .unwrap()
            .unwrap();
        assert_eq!(read_back, payload);

        cleanup(&mut db, &root);
    }

    #[test]
    fn write_blocks_batch_flushes_dirty_blocks() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "buffered.bin").unwrap();

        let mut dirty = std::collections::HashMap::new();
        dirty.insert(0, b"Hello Buffered ".to_vec());
        db.write_blocks_batch(&root, "buffered.bin", &dirty, 15)
            .unwrap();

        let content = db
            .read_range(&root, "buffered.bin", 0, 15)
            .unwrap()
            .unwrap();
        assert_eq!(content, b"Hello Buffered ");

        cleanup(&mut db, &root);
    }

    #[test]
    fn db_pool_concurrent_queries() {
        let conn_str = format!(
            "host={}/testdata dbname=pgfs",
            std::env::current_dir().unwrap().display()
        );
        let pool = DbPool::connect_opts(&conn_str, None, false, 5).expect("connect DbPool");
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let p = pool.clone();
                std::thread::spawn(move || {
                    let mut conn = p.get().expect("get pooled conn");
                    let _ = conn.list("");
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    /// With no replica configured, every read must be served from the
    /// primary (replica routing degrades to a single-node mount).
    #[test]
    fn no_replica_serves_primary() {
        let conn_str = format!(
            "host={}/testdata dbname=pgfs",
            std::env::current_dir().unwrap().display()
        );
        let mut db = Db::connect(&conn_str, None).expect("connect without replica");
        assert!(db.replica.is_none(), "no replica client should be opened");

        // A write + read round-trip through the public API must succeed,
        // which exercises the fallback-to-primary path in reader().
        let root = root_dir(&mut db);
        db.create(&root, "r.txt").unwrap();
        db.write(&root, "r.txt", b"primary").unwrap();
        assert_eq!(db.read(&root, "r.txt").unwrap().unwrap(), b"primary");
        cleanup(&mut db, &root);
    }

    // ── Create / Getattr ────────────────────────────────────────────────

    #[test]
    fn create_file_and_getattr() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "hello.txt").unwrap();
        let meta = db
            .getattr(&root, "hello.txt")
            .unwrap()
            .expect("entry should exist");
        assert_eq!(meta.name, "hello.txt");
        assert_eq!(meta.kind, Kind::File);
        assert_eq!(meta.size, 0);

        // Nonexistent entry returns None.
        assert!(db.getattr(&root, "nope.txt").unwrap().is_none());

        cleanup(&mut db, &root);
    }

    #[test]
    fn getattr_root_level_file() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "top.txt").unwrap();
        let meta = db.getattr(&root, "top.txt").unwrap().expect("exists");
        assert_eq!(meta.name, "top.txt");
        assert_eq!(meta.kind, Kind::File);

        cleanup(&mut db, &root);
    }

    // ── Read / Write ────────────────────────────────────────────────────

    #[test]
    fn read_empty_file() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "empty.bin").unwrap();
        let data = db.read(&root, "empty.bin").unwrap().expect("exists");
        assert!(data.is_empty());

        cleanup(&mut db, &root);
    }

    #[test]
    fn write_and_read_roundtrip() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "data.bin").unwrap();
        db.write(&root, "data.bin", b"hello world").unwrap();

        let data = db.read(&root, "data.bin").unwrap().expect("exists");
        assert_eq!(data, b"hello world");
        assert_eq!(
            db.getattr(&root, "data.bin").unwrap().expect("exists").size,
            11
        );

        cleanup(&mut db, &root);
    }

    #[test]
    fn write_overwrites_previous_content() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "over.bin").unwrap();
        db.write(&root, "over.bin", b"first").unwrap();
        db.write(&root, "over.bin", b"second").unwrap();

        let data = db.read(&root, "over.bin").unwrap().expect("exists");
        assert_eq!(data, b"second");

        cleanup(&mut db, &root);
    }

    #[test]
    fn read_nonexistent_file() {
        let mut db = db_connect();
        let root = root_dir(&mut db);
        assert!(db.read(&root, "ghost").unwrap().is_none());
        cleanup(&mut db, &root);
    }

    // ── Mkdir / List ────────────────────────────────────────────────────

    #[test]
    fn mkdir_and_list_children() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "sub").unwrap();
        let children = db.list(&root).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "sub");
        assert_eq!(children[0].kind, Kind::Dir);

        cleanup(&mut db, &root);
    }

    #[test]
    fn list_mixed_children_sorted_by_name() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "b_dir").unwrap();
        db.create(&root, "a_file").unwrap();
        db.create(&root, "c_file").unwrap();

        let children = db.list(&root).unwrap();
        let names: Vec<&str> = children.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["a_file", "b_dir", "c_file"]);

        cleanup(&mut db, &root);
    }

    #[test]
    fn list_empty_directory() {
        let mut db = db_connect();
        let root = root_dir(&mut db);
        let children = db.list(&root).unwrap();
        assert!(children.is_empty());
        cleanup(&mut db, &root);
    }

    #[test]
    fn list_root_directory_does_not_contain_empty_name() {
        let mut db = db_connect();
        let children = db.list("").unwrap();
        for child in &children {
            assert!(
                !child.name.is_empty(),
                "db.list('') should never return an entry with empty name"
            );
        }
    }

    #[test]
    fn getattr_directory() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "mydir").unwrap();
        let meta = db
            .getattr(&root, "mydir")
            .unwrap()
            .expect("directory should exist");
        assert_eq!(meta.kind, Kind::Dir);
        assert_eq!(meta.size, 0);

        cleanup(&mut db, &root);
    }

    // ── Unlink ──────────────────────────────────────────────────────────

    #[test]
    fn unlink_removes_file() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "gone.txt").unwrap();
        db.unlink(&root, "gone.txt").unwrap();
        assert!(db.getattr(&root, "gone.txt").unwrap().is_none());

        cleanup(&mut db, &root);
    }

    // ── Rmdir ───────────────────────────────────────────────────────────

    #[test]
    fn rmdir_empty_succeeds() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "empty").unwrap();
        assert!(db.rmdir(&root, "empty").unwrap());
        assert!(db.getattr(&root, "empty").unwrap().is_none());

        cleanup(&mut db, &root);
    }

    #[test]
    fn rmdir_not_empty_fails() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "populated").unwrap();
        db.create(&join(&root, "populated"), "child.txt").unwrap();
        assert!(!db.rmdir(&root, "populated").unwrap());
        // Directory should still exist.
        assert!(db.getattr(&root, "populated").unwrap().is_some());

        cleanup(&mut db, &root);
    }

    #[test]
    fn rmdir_nonexistent_returns_false() {
        let mut db = db_connect();
        let root = root_dir(&mut db);
        assert!(!db.rmdir(&root, "nope").unwrap());
        cleanup(&mut db, &root);
    }

    #[test]
    fn rmdir_nested_empty_dirs() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        let _a = join(&root, "a");
        db.mkdir(&root, "a").unwrap();
        db.mkdir(&_a, "b").unwrap();

        // Deleting non-empty fails.
        assert!(!db.rmdir(&root, "a").unwrap());
        // Delete inner first.
        assert!(db.rmdir(&_a, "b").unwrap());
        assert!(db.rmdir(&root, "a").unwrap());

        cleanup(&mut db, &root);
    }

    // ── Rename ──────────────────────────────────────────────────────────

    #[test]
    fn rename_file_simple() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "old.txt").unwrap();
        db.write(&root, "old.txt", b"payload").unwrap();

        db.rename(&root, "old.txt", &root, "new.txt").unwrap();

        assert!(db.getattr(&root, "old.txt").unwrap().is_none());
        let meta = db
            .getattr(&root, "new.txt")
            .unwrap()
            .expect("new should exist");
        assert_eq!(meta.name, "new.txt");
        let data = db.read(&root, "new.txt").unwrap().expect("exists");
        assert_eq!(data, b"payload");

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_file_cross_directory() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "src").unwrap();
        db.mkdir(&root, "dst").unwrap();
        db.create(&join(&root, "src"), "f.txt").unwrap();
        db.write(&join(&root, "src"), "f.txt", b"x").unwrap();

        db.rename(&join(&root, "src"), "f.txt", &join(&root, "dst"), "f.txt")
            .unwrap();

        assert!(db.getattr(&join(&root, "src"), "f.txt").unwrap().is_none());
        assert!(db.getattr(&join(&root, "dst"), "f.txt").unwrap().is_some());

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_file_overwrite_existing_file() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "a.txt").unwrap();
        db.write(&root, "a.txt", b"A").unwrap();
        db.create(&root, "b.txt").unwrap();
        db.write(&root, "b.txt", b"B").unwrap();

        db.rename(&root, "a.txt", &root, "b.txt").unwrap();

        // a.txt should be gone, b.txt should have "A".
        assert!(db.getattr(&root, "a.txt").unwrap().is_none());
        let data = db.read(&root, "b.txt").unwrap().expect("exists");
        assert_eq!(data, b"A");

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_dir_with_children_cascades() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        let src = join(&root, "src");
        db.mkdir(&root, "src").unwrap();
        db.mkdir(&src, "sub").unwrap();
        db.create(&src, "top.txt").unwrap();
        db.create(&join(&src, "sub"), "deep.txt").unwrap();

        db.rename(&root, "src", &root, "dest").unwrap();

        // Old path should be gone.
        assert!(db.getattr(&root, "src").unwrap().is_none());

        // New paths should work.
        let dest = join(&root, "dest");
        assert!(db.getattr(&root, "dest").unwrap().is_some());
        assert!(db.getattr(&dest, "top.txt").unwrap().is_some());
        assert!(db.getattr(&dest, "sub").unwrap().is_some());
        assert!(db
            .getattr(&join(&dest, "sub"), "deep.txt")
            .unwrap()
            .is_some());

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_dir_overwrite_empty_dir() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "src").unwrap();
        db.create(&join(&root, "src"), "payload.txt").unwrap();
        db.mkdir(&root, "dst").unwrap(); // empty

        db.rename(&root, "src", &root, "dst").unwrap();

        assert!(db.getattr(&root, "src").unwrap().is_none());
        assert!(db.getattr(&root, "dst").unwrap().is_some());
        assert!(db
            .getattr(&join(&root, "dst"), "payload.txt")
            .unwrap()
            .is_some());

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_nonexistent_source_errors() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        let err = db.rename(&root, "ghost", &root, "any").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("does not exist"),
            "expected 'does not exist' in: {msg}"
        );

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_over_non_empty_dir_errors() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "empty").unwrap();
        db.mkdir(&root, "full").unwrap();
        db.create(&join(&root, "full"), "guard.txt").unwrap();

        let err = db.rename(&root, "empty", &root, "full").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not empty"), "expected 'not empty' in: {msg}");

        // Non-empty dir should be untouched.
        assert!(db
            .getattr(&join(&root, "full"), "guard.txt")
            .unwrap()
            .is_some());

        cleanup(&mut db, &root);
    }

    #[test]
    fn rename_onto_self_is_noop() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "self.txt").unwrap();
        db.write(&root, "self.txt", b"hi").unwrap();

        // Rename to self: db.rename would be given same (parent, name).
        db.rename(&root, "self.txt", &root, "self.txt").unwrap();

        let data = db.read(&root, "self.txt").unwrap().expect("exists");
        assert_eq!(data, b"hi");

        cleanup(&mut db, &root);
    }

    // ── Join helper ─────────────────────────────────────────────────────

    #[test]
    fn join_root_entry() {
        assert_eq!(join("", "foo"), "foo");
    }

    #[test]
    fn join_nested() {
        assert_eq!(join("a", "b"), "a/b");
        assert_eq!(join("a/b", "c"), "a/b/c");
    }

    // ── Create existing (ON CONFLICT DO NOTHING) ────────────────────────

    #[test]
    fn create_existing_is_silent_noop() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.create(&root, "dup.txt").unwrap();
        // Creating again should not error (ON CONFLICT DO NOTHING).
        db.create(&root, "dup.txt").unwrap();

        cleanup(&mut db, &root);
    }

    #[test]
    fn mkdir_existing_is_silent_noop() {
        let mut db = db_connect();
        let root = root_dir(&mut db);

        db.mkdir(&root, "dup").unwrap();
        db.mkdir(&root, "dup").unwrap();

        cleanup(&mut db, &root);
    }
}

fn row_to_meta(row: &postgres::Row) -> Result<FileMeta> {
    let kind_raw: String = row.get("kind");
    let kind = match kind_raw.as_str() {
        "file" => Kind::File,
        "dir" => Kind::Dir,
        other => {
            return Err(error::failure(format!(
                "unexpected entry kind {other:?} for row name={name:?}",
                other = other,
                name = row.get::<_, String>("name"),
            )))
        }
    };
    let size: i64 = row.get("size");
    if size < 0 {
        return Err(error::failure(format!(
            "negative size {size} for entry {:?}",
            row.get::<_, String>("name")
        )));
    }
    let mtime: std::time::SystemTime = row.get("mtime");
    Ok(FileMeta {
        name: row.get("name"),
        kind,
        size: size as u64,
        mtime,
    })
}

pub mod backend_impl {
    use super::*;
    use crate::backend::{
        prepare_sparse_block_write, DirEntry, FileAttr, FileKind, PgfsBackend, SetAttrInput,
        StorageError,
    };

    #[allow(dead_code)]
    fn row_to_file_attr(row: &postgres::Row) -> std::result::Result<FileAttr, StorageError> {
        let ino: i64 = row.get("ino");
        let size: i64 = row.get("size");
        let kind_raw: String = row.get("kind");
        let kind = match kind_raw.as_str() {
            "file" => FileKind::File,
            "dir" => FileKind::Directory,
            "symlink" => FileKind::Symlink,
            _ => FileKind::File,
        };
        let mode: i32 = row
            .try_get("mode")
            .unwrap_or(if kind == FileKind::Directory {
                16877
            } else {
                33188
            });
        let uid: i32 = row.try_get("uid").unwrap_or(1000);
        let gid: i32 = row.try_get("gid").unwrap_or(1000);
        let mtime: SystemTime = row.try_get("mtime").unwrap_or_else(|_| SystemTime::now());
        let atime: SystemTime = row.try_get("atime").unwrap_or(mtime);
        let ctime: SystemTime = row.try_get("ctime").unwrap_or(mtime);

        let blocks_count = (size as u64).div_ceil(512);

        Ok(FileAttr {
            ino: ino as u64,
            size: size as u64,
            blocks: blocks_count,
            atime,
            mtime,
            ctime,
            kind,
            perm: (mode & 0o777) as u16,
            nlink: if kind == FileKind::Directory { 2 } else { 1 },
            uid: uid as u32,
            gid: gid as u32,
            rdev: 0,
            flags: 0,
        })
    }

    #[async_trait::async_trait]
    impl PgfsBackend for Db {
        async fn lookup(
            &self,
            parent_ino: u64,
            name: &str,
        ) -> std::result::Result<Option<FileAttr>, StorageError> {
            let rows = self
                .client_guard()
                .query(&self.stmt_lookup_ino, &[&(parent_ino as i64), &name])
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if rows.is_empty() {
                Ok(None)
            } else {
                Ok(Some(row_to_file_attr(&rows[0])?))
            }
        }

        async fn getattr(&self, ino: u64) -> std::result::Result<FileAttr, StorageError> {
            let rows = self
                .client_guard()
                .query(&self.stmt_getattr_ino, &[&(ino as i64)])
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if rows.is_empty() {
                Err(StorageError::NotFound)
            } else {
                row_to_file_attr(&rows[0])
            }
        }

        async fn setattr(
            &self,
            ino: u64,
            attr: SetAttrInput,
        ) -> std::result::Result<FileAttr, StorageError> {
            let current = self.getattr(ino).await?;
            if let Some(sz) = attr.size {
                if sz < current.size {
                    self.truncate_blocks(ino, sz).await?;
                }
            }
            let mode_val = attr.mode.map(|m| m as i32);
            let uid_val = attr.uid.map(|u| u as i32);
            let gid_val = attr.gid.map(|g| g as i32);
            let size_val = attr.size.map(|s| s as i64);

            let row = self
            .client_guard()
            .query_one(
                "UPDATE entries SET mode = COALESCE($2, mode), uid = COALESCE($3, uid), gid = COALESCE($4, gid), size = COALESCE($5, size), mtime = NOW(), ctime = NOW() WHERE ino = $1 RETURNING ino, size, kind, mode, uid, gid, atime, mtime, ctime",
                &[&(ino as i64), &mode_val, &uid_val, &gid_val, &size_val],
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            row_to_file_attr(&row)
        }

        async fn readdir(
            &self,
            ino: u64,
            offset: i64,
        ) -> std::result::Result<Vec<DirEntry>, StorageError> {
            let rows = self
            .client_guard()
            .query(
                "SELECT ino, name, kind FROM entries WHERE parent_ino = $1 AND name != '' ORDER BY name OFFSET $2",
                &[&(ino as i64), &offset],
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            let mut entries = Vec::with_capacity(rows.len());
            for row in rows {
                let child_ino: i64 = row.get("ino");
                let name: String = row.get("name");
                let kind_raw: String = row.get("kind");
                let kind = match kind_raw.as_str() {
                    "dir" => FileKind::Directory,
                    "symlink" => FileKind::Symlink,
                    _ => FileKind::File,
                };
                entries.push(DirEntry {
                    ino: child_ino as u64,
                    name,
                    kind,
                });
            }
            Ok(entries)
        }

        async fn mkdir(
            &self,
            parent_ino: u64,
            name: &str,
            mode: u32,
            uid: u32,
            gid: u32,
        ) -> std::result::Result<FileAttr, StorageError> {
            let row = self
            .client_guard()
            .query_opt(
                "INSERT INTO entries (parent_ino, parent, name, kind, size, mode, uid, gid) VALUES ($1, '', $2, 'dir', 0, $3, $4, $5) ON CONFLICT (parent, name) DO NOTHING RETURNING ino, size, kind, mode, uid, gid, atime, mtime, ctime",
                &[&(parent_ino as i64), &name, &(mode as i32), &(uid as i32), &(gid as i32)],
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            match row {
                Some(r) => row_to_file_attr(&r),
                None => {
                    let existing = self.lookup(parent_ino, name).await?;
                    existing.ok_or(StorageError::AlreadyExists)
                }
            }
        }

        async fn rmdir(
            &self,
            parent_ino: u64,
            name: &str,
        ) -> std::result::Result<(), StorageError> {
            let target = self
                .lookup(parent_ino, name)
                .await?
                .ok_or(StorageError::NotFound)?;
            if target.kind != FileKind::Directory {
                return Err(StorageError::InvalidArgument("not a directory".into()));
            }
            let children = self.readdir(target.ino, 0).await?;
            if !children.is_empty() {
                return Err(StorageError::NotEmpty);
            }
            let count = self
                .client_guard()
                .execute(
                    "DELETE FROM entries WHERE ino = $1",
                    &[&(target.ino as i64)],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if count == 0 {
                Err(StorageError::NotFound)
            } else {
                Ok(())
            }
        }

        async fn create(
            &self,
            parent_ino: u64,
            name: &str,
            mode: u32,
            uid: u32,
            gid: u32,
        ) -> std::result::Result<FileAttr, StorageError> {
            let row = self
            .client_guard()
            .query_opt(
                "INSERT INTO entries (parent_ino, parent, name, kind, size, mode, uid, gid) VALUES ($1, '', $2, 'file', 0, $3, $4, $5) ON CONFLICT (parent, name) DO NOTHING RETURNING ino, size, kind, mode, uid, gid, atime, mtime, ctime",
                &[&(parent_ino as i64), &name, &(mode as i32), &(uid as i32), &(gid as i32)],
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            match row {
                Some(r) => row_to_file_attr(&r),
                None => {
                    let existing = self.lookup(parent_ino, name).await?;
                    existing.ok_or(StorageError::AlreadyExists)
                }
            }
        }

        async fn unlink(
            &self,
            parent_ino: u64,
            name: &str,
        ) -> std::result::Result<(), StorageError> {
            let count = self
                .client_guard()
                .execute(
                    "DELETE FROM entries WHERE parent_ino = $1 AND name = $2 AND kind != 'dir'",
                    &[&(parent_ino as i64), &name],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if count == 0 {
                Err(StorageError::NotFound)
            } else {
                Ok(())
            }
        }

        async fn rename(
            &self,
            old_parent: u64,
            old_name: &str,
            new_parent: u64,
            new_name: &str,
        ) -> std::result::Result<(), StorageError> {
            let count = self
                .client_guard()
                .execute(
                    &self.stmt_rename_ino,
                    &[
                        &(old_parent as i64),
                        &old_name,
                        &(new_parent as i64),
                        &new_name,
                    ],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if count == 0 {
                Err(StorageError::NotFound)
            } else {
                Ok(())
            }
        }

        async fn read_block(
            &self,
            ino: u64,
            block_no: u32,
        ) -> std::result::Result<Vec<u8>, StorageError> {
            let rows = self
                .client_guard()
                .query(
                    &self.stmt_read_block_ino,
                    &[&(ino as i64), &(block_no as i32)],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            if rows.is_empty() {
                Ok(Vec::new())
            } else {
                let data: Vec<u8> = rows[0].get("data");
                Ok(data)
            }
        }

        async fn write_block(
            &self,
            ino: u64,
            block_no: u32,
            data: &[u8],
            offset_in_block: u32,
        ) -> std::result::Result<u32, StorageError> {
            let existing = self
                .read_block(ino, block_no)
                .await
                .ok()
                .filter(|b| !b.is_empty());
            let updated = prepare_sparse_block_write(existing, offset_in_block as usize, data);
            let bytes_len = updated.len() as u32;

            self.client_guard()
                .execute(
                    &self.stmt_upsert_block_ino,
                    &[&(ino as i64), &(block_no as i32), &updated],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            let new_min_size = (block_no as u64) * 131072 + bytes_len as u64;
            self.client_guard()
                .execute(
                    "UPDATE entries SET size = GREATEST(size, $2), mtime = NOW() WHERE ino = $1",
                    &[&(ino as i64), &(new_min_size as i64)],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            Ok(data.len() as u32)
        }

        async fn truncate_blocks(
            &self,
            ino: u64,
            new_size: u64,
        ) -> std::result::Result<(), StorageError> {
            let target_block = (new_size / 131072) as i32;
            self.client_guard()
                .execute(&self.stmt_prune_blocks_ino, &[&(ino as i64), &target_block])
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

            let partial_offset = (new_size % 131072) as usize;
            if partial_offset > 0 {
                if let Ok(mut data) = self.read_block(ino, target_block as u32).await {
                    if data.len() > partial_offset {
                        data.truncate(partial_offset);
                        self.client_guard()
                            .execute(
                                &self.stmt_upsert_block_ino,
                                &[&(ino as i64), &target_block, &data],
                            )
                            .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
                    }
                }
            }

            self.client_guard()
                .execute(
                    "UPDATE entries SET size = $2, mtime = NOW() WHERE ino = $1",
                    &[&(ino as i64), &(new_size as i64)],
                )
                .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            Ok(())
        }

        async fn copy_file_range(
            &self,
            ino_in: u64,
            off_in: u64,
            ino_out: u64,
            off_out: u64,
            len: u64,
        ) -> std::result::Result<u64, StorageError> {
            if len == 0 {
                return Ok(0);
            }
            let block_size = 131072u64;
            if off_in.is_multiple_of(block_size)
                && off_out.is_multiple_of(block_size)
                && len.is_multiple_of(block_size)
            {
                let start_b_in = (off_in / block_size) as i32;
                let start_b_out = (off_out / block_size) as i32;
                let num_blocks = (len / block_size) as i32;

                self.client_guard()
                    .execute(
                        "INSERT INTO blocks (ino, parent, name, block_no, data)
                         SELECT DISTINCT ON (block_no) $3::bigint, '', $3::text, $4 + (block_no - $2), data
                         FROM blocks WHERE ino = $1 AND block_no >= $2 AND block_no < $2 + $5 ORDER BY block_no
                         ON CONFLICT (parent, name, block_no) DO UPDATE SET data = EXCLUDED.data",
                        &[
                            &(ino_in as i64),
                            &start_b_in,
                            &(ino_out as i64),
                            &start_b_out,
                            &num_blocks,
                        ],
                    )
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

                let new_min_size = off_out + len;
                self.client_guard()
                    .execute(
                        "UPDATE entries SET size = GREATEST(size, $2), mtime = NOW() WHERE ino = $1",
                        &[&(ino_out as i64), &(new_min_size as i64)],
                    )
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

                Ok(len)
            } else {
                let start_b_in = (off_in / block_size) as u32;
                let end_b_in = ((off_in + len - 1) / block_size) as u32;
                let mut copied = 0u64;

                for b_in in start_b_in..=end_b_in {
                    let b_data = self.read_block(ino_in, b_in).await?;
                    if b_data.is_empty() {
                        continue;
                    }
                    let b_start_in = (b_in as u64) * block_size;
                    let slice_start = if off_in > b_start_in {
                        (off_in - b_start_in) as usize
                    } else {
                        0
                    };
                    let slice_end = ((off_in + len) - b_start_in).min(b_data.len() as u64) as usize;
                    if slice_start < slice_end && slice_start < b_data.len() {
                        let chunk = &b_data[slice_start..slice_end];
                        let target_off = off_out + (b_start_in + slice_start as u64 - off_in);
                        let target_b_no = (target_off / block_size) as u32;
                        let target_b_off = (target_off % block_size) as u32;
                        self.write_block(ino_out, target_b_no, chunk, target_b_off)
                            .await?;
                        copied += chunk.len() as u64;
                    }
                }
                Ok(copied)
            }
        }

        async fn fallocate(
            &self,
            ino: u64,
            offset: u64,
            length: u64,
            mode: u32,
        ) -> std::result::Result<(), StorageError> {
            let block_size = 131072u64;
            let is_punch_hole = (mode & 0x02) != 0; // FALLOC_FL_PUNCH_HOLE
            let keep_size = (mode & 0x01) != 0; // FALLOC_FL_KEEP_SIZE

            if is_punch_hole {
                let start_block = (offset / block_size) as i32;
                let end_block = ((offset + length - 1) / block_size) as i32;
                self.client_guard()
                    .execute(
                        "DELETE FROM blocks WHERE ino = $1 AND block_no >= $2 AND block_no <= $3",
                        &[&(ino as i64), &start_block, &end_block],
                    )
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            } else if !keep_size {
                let new_min_size = offset + length;
                self.client_guard()
                    .execute(
                        "UPDATE entries SET size = GREATEST(size, $2), mtime = NOW() WHERE ino = $1",
                        &[&(ino as i64), &(new_min_size as i64)],
                    )
                    .map_err(|e| StorageError::DatabaseError(e.to_string()))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests_offload {
    use crate::backend::PgfsBackend;

    fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn raw_waker() -> RawWaker {
            fn noop(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                raw_waker()
            }
            static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        let waker = unsafe { Waker::from_raw(raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(res) => res,
            Poll::Pending => panic!("future pending"),
        }
    }

    #[test]
    fn test_backend_offloaded_copy_file_range_and_fallocate() {
        let db = super::tests::db_connect();

        // Create src and dst files
        let src_attr = block_on(db.create(1, "src.bin", 0o644, 1000, 1000)).unwrap();
        let dst_attr = block_on(db.create(1, "dst.bin", 0o644, 1000, 1000)).unwrap();

        // Write 1 block of data to src
        let payload = vec![7u8; 131072];
        block_on(db.write_block(src_attr.ino, 0, &payload, 0)).unwrap();

        // Offloaded copy_file_range
        let copied = block_on(db.copy_file_range(src_attr.ino, 0, dst_attr.ino, 0, 131072))
            .expect("copy_file_range failed");
        assert_eq!(copied, 131072);

        // Verify dst content
        let read_back = block_on(db.read_block(dst_attr.ino, 0)).unwrap();
        assert_eq!(read_back, payload);

        // Test fallocate punch hole
        block_on(db.fallocate(dst_attr.ino, 0, 131072, 0x02)).unwrap(); // FALLOC_FL_PUNCH_HOLE
        let punched_read = block_on(db.read_block(dst_attr.ino, 0)).unwrap();
        assert!(punched_read.is_empty());
    }
}

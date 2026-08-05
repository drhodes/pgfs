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
use std::time::{Instant, SystemTime};
use tracing::debug_span;

pub struct Db {
    /// Primary connection — every write goes here.
    client: Client,
    /// Optional physical streaming standby (see spec/replica.py). Reads
    /// are served from it when it has caught up with the primary's WAL.
    replica: Option<Client>,

    // Pre-compiled SQL statement handles on the primary connection.
    stmt_getattr: Statement,
    stmt_read: Statement,
    stmt_write: Statement,
    stmt_create: Statement,
    stmt_mkdir: Statement,
    stmt_unlink: Statement,
    stmt_rmdir: Statement,
    stmt_list: Statement,
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
    pub fn connect(conn_str: &str, replica_conn: Option<&str>) -> Result<Self> {
        let mut client = error::ctx(
            Client::connect(conn_str, NoTls),
            &format!("connect to Postgres ({conn_str})"),
        )?;
        error::ctx(
            client.batch_execute(
                "SET synchronous_commit = off;
                CREATE TABLE IF NOT EXISTS entries (
                    parent text NOT NULL,
                    name   text NOT NULL,
                    kind   text NOT NULL DEFAULT 'file',
                    data   bytea NOT NULL DEFAULT '',
                    size   bigint NOT NULL DEFAULT 0,
                    mtime  timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (parent, name)
                )",
            ),
            "ensure schema exists and configure synchronous_commit = off",
        )?;

        let stmt_getattr = error::ctx(
            client.prepare(
                "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 AND name = $2",
            ),
            "prepare stmt_getattr",
        )?;
        let stmt_read = error::ctx(
            client.prepare("SELECT data FROM entries WHERE parent = $1 AND name = $2"),
            "prepare stmt_read",
        )?;
        let stmt_write = error::ctx(
            client.prepare("UPDATE entries SET data = $3, size = $4, mtime = now() WHERE parent = $1 AND name = $2"),
            "prepare stmt_write",
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
                "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 ORDER BY name",
            ),
            "prepare stmt_list",
        )?;

        let replica = match replica_conn {
            Some(s) => match Client::connect(s, NoTls) {
                Ok(c) => Some(c),
                // Best-effort: an unreachable standby must never prevent the
                // mount from starting (spec/replica.py ReplicaObservability).
                // The mount runs primary-only and every read falls back.
                Err(e) => {
                    tracing::warn!(
                        "replica standby unreachable at startup ({s}); running primary-only: {e}"
                    );
                    None
                }
            },
            None => None,
        };
        Ok(Db {
            client,
            replica,
            stmt_getattr,
            stmt_read,
            stmt_write,
            stmt_create,
            stmt_mkdir,
            stmt_unlink,
            stmt_rmdir,
            stmt_list,
        })
    }

    /// Is the replica fresh enough to serve reads? True iff a replica is
    /// configured, reachable, and its last replayed WAL position is at or
    /// ahead of the primary's current position. A standby that has never
    /// replayed anything, or any error on either node, means not fresh.
    fn replica_fresh(&mut self) -> bool {
        let Some(replica) = self.replica.as_mut() else {
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
            .client
            .query_opt("SELECT (pg_current_wal_lsn() - '0/0'::pg_lsn)::bigint", &[])
            .ok()
            .flatten()
            .map(|r| r.get(0));
        let replay_lsn: Option<Option<i64>> = self
            .replica
            .as_mut()
            .expect("replica present")
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

    /// Human-readable replica health for the SIGUSR1 state dump
    /// (spec/replica.py ReplicaObservability: the SIGUSR1 state dump
    /// reports replica freshness). One of: "none" (no --replica),
    /// "fresh", or "stale or unreachable".
    pub fn replica_state(&mut self) -> String {
        if self.replica.is_none() {
            return "none".to_string();
        }
        if self.replica_fresh() {
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
            self.client.query_opt(&self.stmt_getattr, &[&parent, &name]),
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
            self.client.query(&self.stmt_list, &[&parent]),
            &format!("list children of directory {parent:?}"),
        )?;
        let result: Result<Vec<FileMeta>> = rows.iter().map(row_to_meta).collect();
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        result
    }

    /// Children of a directory, sorted by name. Ordering is cosmetic here;
    /// fs.rs re-iterates this on every readdir.
    pub fn list(&mut self, parent: &str) -> Result<Vec<FileMeta>> {
        if self.replica.is_some() && self.replica_fresh() {
            crate::metrics::REPLICA_READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let replica = self.replica.as_mut().expect("replica present");
            let _span = debug_span!("db::list", parent).entered();
            let _t0 = Instant::now();
            let rows = error::ctx(
                replica.query(
                    "SELECT name, kind, size, mtime FROM entries WHERE parent = $1 ORDER BY name",
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
        if self.replica.is_some() && self.replica_fresh() {
            crate::metrics::REPLICA_READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let replica = self.replica.as_mut().expect("replica present");
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

    pub fn read(&mut self, parent: &str, name: &str) -> Result<Option<Vec<u8>>> {
        if self.replica.is_some() && self.replica_fresh() {
            crate::metrics::REPLICA_READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let replica = self.replica.as_mut().expect("replica present");
            let _span = debug_span!("db::read", parent, name).entered();
            let _t0 = Instant::now();
            let row = error::ctx(
                replica.query_opt(
                    "SELECT data FROM entries WHERE parent = $1 AND name = $2",
                    &[&parent, &name],
                ),
                &format!("read contents of {name:?} in {parent:?}"),
            )?;
            crate::metrics::DB_LATENCY.record(_t0.elapsed());
            Ok(row.map(|r| r.get::<_, Vec<u8>>("data")))
        } else {
            self.record_replica_fallback_if_configured();
            self.read_primary(parent, name)
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

    /// read forced to the primary. Used by the write path's
    /// read-modify-write (fs.rs) so a stale replica can never supply the
    /// "old" bytes a write is about to patch over — that would silently
    /// lose concurrent primary writes.
    pub fn read_primary(&mut self, parent: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let _span = debug_span!("db::read", parent, name).entered();
        let _t0 = Instant::now();
        let row = error::ctx(
            self.client.query_opt(&self.stmt_read, &[&parent, &name]),
            &format!("read contents of {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(row.map(|r| r.get::<_, Vec<u8>>("data")))
    }

    /// Whole-blob write. Simplest possible thing that works: read-modify-write
    /// happens in fs.rs (it fetches, patches the byte range, calls this with
    /// the full new contents). No chunking yet — that's a later upgrade.
    pub fn write(&mut self, parent: &str, name: &str, data: &[u8]) -> Result<()> {
        let _span = debug_span!("db::write", parent, name, len = data.len()).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client.execute(
                &self.stmt_write,
                &[&parent, &name, &data, &(data.len() as i64)],
            ),
            &format!("write {} bytes to {name:?} in {parent:?}", data.len()),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn create(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::create", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client.execute(&self.stmt_create, &[&parent, &name]),
            &format!("create file {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn mkdir(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::mkdir", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client.execute(&self.stmt_mkdir, &[&parent, &name]),
            &format!("create directory {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn unlink(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::unlink", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client.execute(&self.stmt_unlink, &[&parent, &name]),
            &format!("remove file {name:?} from {parent:?}"),
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

        let mut txn = error::ctx(
            self.client.transaction(),
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
        }

        let moved = error::ctx(
            txn.execute(
                "UPDATE entries SET parent = $1, name = $2, mtime = now()
                 WHERE parent = $3 AND name = $4",
                &[&new_parent, &new_name, &old_parent, &old_name],
            ),
            &format!("move entry {old_path:?} -> {new_path:?}"),
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
            self.client
                .query_opt("SELECT 1 FROM entries WHERE parent = $1 LIMIT 1", &[&path]),
            &format!("check whether {name:?} in {parent:?} is empty"),
        )?;
        if has_children.is_some() {
            crate::metrics::DB_LATENCY.record(_t0.elapsed());
            return Ok(false);
        }
        let deleted = error::ctx(
            self.client.execute(&self.stmt_rmdir, &[&parent, &name]),
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

    fn db_connect() -> Db {
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
        let mut db = db_connect();
        let row = db
            .client
            .query_one("SHOW synchronous_commit;", &[])
            .expect("query synchronous_commit");
        let val: String = row.get(0);
        assert_eq!(
            val, "off",
            "synchronous_commit must be tuned off on connection"
        );
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

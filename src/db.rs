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
//! Every method returns `error::Result`, wrapping the underlying
//! `postgres::Error` with a description of what was being attempted and the
//! file:line where it happened, so failures read as a story.

use crate::error::{self, Result};
use postgres::{Client, NoTls};
use std::time::{Instant, SystemTime};
use tracing::debug_span;

pub struct Db {
    client: Client,
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
    pub fn connect(conn_str: &str) -> Result<Self> {
        let mut client = error::ctx(
            Client::connect(conn_str, NoTls),
            &format!("connect to Postgres ({conn_str})"),
        )?;
        error::ctx(
            client.batch_execute(
                "CREATE TABLE IF NOT EXISTS entries (
                    parent text NOT NULL,
                    name   text NOT NULL,
                    kind   text NOT NULL DEFAULT 'file',
                    data   bytea NOT NULL DEFAULT '',
                    size   bigint NOT NULL DEFAULT 0,
                    mtime  timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (parent, name)
                )",
            ),
            "ensure the entries schema exists",
        )?;
        Ok(Db { client })
    }

    /// Children of a directory, sorted by name. Ordering is cosmetic here;
    /// fs.rs re-iterates this on every readdir.
    pub fn list(&mut self, parent: &str) -> Result<Vec<FileMeta>> {
        let _span = debug_span!("db::list", parent).entered();
        let _t0 = Instant::now();
        let rows = error::ctx(
            self.client.query(
                "SELECT name, kind, size, mtime FROM entries
                 WHERE parent = $1 ORDER BY name",
                &[&parent],
            ),
            &format!("list children of directory {parent:?}"),
        )?;
        let result: Result<Vec<FileMeta>> = rows.iter().map(row_to_meta).collect();
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        result
    }

    pub fn getattr(&mut self, parent: &str, name: &str) -> Result<Option<FileMeta>> {
        let _span = debug_span!("db::getattr", parent, name).entered();
        let _t0 = Instant::now();
        let row = error::ctx(
            self.client.query_opt(
                "SELECT name, kind, size, mtime FROM entries
                 WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
            &format!("get attributes of entry {name:?} in {parent:?}"),
        )?;
        let result = row.as_ref().map(row_to_meta).transpose();
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        result
    }

    pub fn read(&mut self, parent: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let _span = debug_span!("db::read", parent, name).entered();
        let _t0 = Instant::now();
        let row = error::ctx(
            self.client.query_opt(
                "SELECT data FROM entries WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
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
                "UPDATE entries SET data = $3, size = $4, mtime = now()
                 WHERE parent = $1 AND name = $2",
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
            self.client.execute(
                "INSERT INTO entries (parent, name, kind, data, size) VALUES ($1, $2, 'file', '', 0)
                 ON CONFLICT (parent, name) DO NOTHING",
                &[&parent, &name],
            ),
            &format!("create file {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn mkdir(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::mkdir", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client.execute(
                "INSERT INTO entries (parent, name, kind, data, size) VALUES ($1, $2, 'dir', '', 0)
                 ON CONFLICT (parent, name) DO NOTHING",
                &[&parent, &name],
            ),
            &format!("create directory {name:?} in {parent:?}"),
        )?;
        crate::metrics::DB_LATENCY.record(_t0.elapsed());
        Ok(())
    }

    pub fn unlink(&mut self, parent: &str, name: &str) -> Result<()> {
        let _span = debug_span!("db::unlink", parent, name).entered();
        let _t0 = Instant::now();
        error::ctx(
            self.client.execute(
                "DELETE FROM entries WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
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

        let meta = self
            .getattr(old_parent, old_name)?
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
            self.client.execute(
                "DELETE FROM entries WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
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
        Db::connect(&conn_str).expect("connect")
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

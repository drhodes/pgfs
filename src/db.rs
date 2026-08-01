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
use std::time::SystemTime;

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
        let rows = error::ctx(
            self.client.query(
                "SELECT name, kind, size, mtime FROM entries
                 WHERE parent = $1 ORDER BY name",
                &[&parent],
            ),
            &format!("list children of directory {parent:?}"),
        )?;
        rows.iter().map(row_to_meta).collect()
    }

    pub fn getattr(&mut self, parent: &str, name: &str) -> Result<Option<FileMeta>> {
        let row = error::ctx(
            self.client.query_opt(
                "SELECT name, kind, size, mtime FROM entries
                 WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
            &format!("get attributes of entry {name:?} in {parent:?}"),
        )?;
        row.as_ref().map(row_to_meta).transpose()
    }

    pub fn read(&mut self, parent: &str, name: &str) -> Result<Option<Vec<u8>>> {
        let row = error::ctx(
            self.client.query_opt(
                "SELECT data FROM entries WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
            &format!("read contents of {name:?} in {parent:?}"),
        )?;
        Ok(row.map(|r| r.get::<_, Vec<u8>>("data")))
    }

    /// Whole-blob write. Simplest possible thing that works: read-modify-write
    /// happens in fs.rs (it fetches, patches the byte range, calls this with
    /// the full new contents). No chunking yet — that's a later upgrade.
    pub fn write(&mut self, parent: &str, name: &str, data: &[u8]) -> Result<()> {
        error::ctx(
            self.client.execute(
                "UPDATE entries SET data = $3, size = $4, mtime = now()
                 WHERE parent = $1 AND name = $2",
                &[&parent, &name, &data, &(data.len() as i64)],
            ),
            &format!("write {} bytes to {name:?} in {parent:?}", data.len()),
        )?;
        Ok(())
    }

    pub fn create(&mut self, parent: &str, name: &str) -> Result<()> {
        error::ctx(
            self.client.execute(
                "INSERT INTO entries (parent, name, kind, data, size) VALUES ($1, $2, 'file', '', 0)
                 ON CONFLICT (parent, name) DO NOTHING",
                &[&parent, &name],
            ),
            &format!("create file {name:?} in {parent:?}"),
        )?;
        Ok(())
    }

    pub fn mkdir(&mut self, parent: &str, name: &str) -> Result<()> {
        error::ctx(
            self.client.execute(
                "INSERT INTO entries (parent, name, kind, data, size) VALUES ($1, $2, 'dir', '', 0)
                 ON CONFLICT (parent, name) DO NOTHING",
                &[&parent, &name],
            ),
            &format!("create directory {name:?} in {parent:?}"),
        )?;
        Ok(())
    }

    pub fn unlink(&mut self, parent: &str, name: &str) -> Result<()> {
        error::ctx(
            self.client.execute(
                "DELETE FROM entries WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
            &format!("remove file {name:?} from {parent:?}"),
        )?;
        Ok(())
    }

    /// Removes an empty directory. Returns Ok(true) if deleted, Ok(false) if
    /// it does not exist or still has children — the caller distinguishes
    /// those two cases by checking `getattr` first.
    pub fn rmdir(&mut self, parent: &str, name: &str) -> Result<bool> {
        let path = join(parent, name);
        let has_children = error::ctx(
            self.client.query_opt(
                "SELECT 1 FROM entries WHERE parent = $1 LIMIT 1",
                &[&path],
            ),
            &format!("check whether {name:?} in {parent:?} is empty"),
        )?;
        if has_children.is_some() {
            return Ok(false);
        }
        let deleted = error::ctx(
            self.client.execute(
                "DELETE FROM entries WHERE parent = $1 AND name = $2",
                &[&parent, &name],
            ),
            &format!("remove directory {name:?} from {parent:?}"),
        )?;
        Ok(deleted > 0)
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

//! The FUSE side. This module never runs SQL directly — it only calls
//! methods on `Db`. Its own job is entirely about the inode <-> path
//! translation FUSE requires, since Postgres rows are keyed by
//! (parent, name) but every FUSE callback after `lookup` operates on an
//! inode number.
//!
//! Layout: inode 1 is the mount root (path ""). A path like "a/b" means
//! the entry named "b" inside the directory named "a". The in-memory maps
//! are built lazily and never fully evicted; for this single-threaded,
//! proof-of-concept scale that's fine.
//!
//! Error handling: the kernel can only receive an errno, not a story. So
//! every *unexpected* failure (database error, bad data) is logged in full
//! — what failed, where (file:line), and the root cause — before replying
//! EIO. Expected conditions (file absent, dir not empty, ...) reply their
//! errno directly without log noise.

use crate::db::{self, Db, Kind};
use crate::metrics;
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use libc::{EINVAL, EISDIR, ENOENT, ENOTEMPTY};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tracing::info_span;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;
const ROOT_PATH: &str = "";

pub struct PgFs {
    db: Db,
    /// path -> inode, allocated lazily and never reused.
    ino_by_path: HashMap<String, u64>,
    path_by_ino: HashMap<u64, String>,
    next_ino: u64,
}

/// Split a full path into its (parent, name) key. Root entries have parent "".
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

/// Re-key the in-memory path→ino and ino→path maps from an old tree
/// prefix to a new one. Extracted as a free function so it can be tested
/// without constructing a full `PgFs` (which requires a live `Db`).
fn rekey_maps(
    ino_by_path: &mut HashMap<String, u64>,
    path_by_ino: &mut HashMap<u64, String>,
    old_path: &str,
    new_path: &str,
) {
    if let Some(ino) = ino_by_path.remove(old_path) {
        ino_by_path.insert(new_path.to_string(), ino);
        path_by_ino.insert(ino, new_path.to_string());
    }

    let prefix_old = format!("{old_path}/");
    let prefix_new = format!("{new_path}/");
    let updates: Vec<(String, String)> = ino_by_path
        .keys()
        .filter(|p| p.starts_with(&prefix_old))
        .map(|p| (p.clone(), format!("{prefix_new}{}", &p[prefix_old.len()..])))
        .collect();
    for (old_p, new_p) in updates {
        if let Some(ino) = ino_by_path.remove(&old_p) {
            ino_by_path.insert(new_p.clone(), ino);
            path_by_ino.insert(ino, new_p);
        }
    }
}

impl PgFs {
    pub fn new(db: Db) -> Self {
        PgFs {
            db,
            ino_by_path: HashMap::new(),
            path_by_ino: HashMap::new(),
            next_ino: 2, // 1 is reserved for the root directory
        }
    }

    fn ino_for(&mut self, path: &str) -> u64 {
        *self.ino_by_path.entry(path.to_string()).or_insert_with(|| {
            let ino = self.next_ino;
            self.next_ino += 1;
            self.path_by_ino.insert(ino, path.to_string());
            ino
        })
    }

    fn attr(ino: u64, meta: &db::FileMeta) -> FileAttr {
        match meta.kind {
            Kind::Dir => Self::dir_attr(ino),
            Kind::File => Self::file_attr(ino, meta.size, meta.mtime),
        }
    }

    fn file_attr(ino: u64, size: u64, mtime: std::time::SystemTime) -> FileAttr {
        FileAttr {
            ino,
            size,
            blocks: size.div_ceil(512),
            atime: mtime,
            mtime,
            ctime: mtime,
            crtime: mtime,
            kind: FileType::RegularFile,
            perm: 0o644,
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn dir_attr(ino: u64) -> FileAttr {
        FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn root_attr() -> FileAttr {
        Self::dir_attr(ROOT_INO)
    }

    /// Resolve an inode to its full path, or None for the root/unknown.
    fn path_of_ino(&self, ino: u64) -> Option<String> {
        match ino {
            ROOT_INO => Some(ROOT_PATH.to_string()),
            _ => self.path_by_ino.get(&ino).cloned(),
        }
    }

    /// The inode of the directory containing `path` (".." of path).
    fn parent_ino(&mut self, path: &str) -> u64 {
        let (parent, _) = split_path(path);
        self.ino_for(parent)
    }

    /// After a rename, re-key every cached path (the entry itself and, for
    /// directories, all descendants) from `old_path` to `new_path`.
    fn rekey_path_maps(&mut self, old_path: &str, new_path: &str) {
        rekey_maps(
            &mut self.ino_by_path,
            &mut self.path_by_ino,
            old_path,
            new_path,
        );
    }
}

impl Filesystem for PgFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let _span = info_span!("lookup", parent, name = %name.to_string_lossy()).entered();
        let t0 = Instant::now();
        metrics::LOOKUP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => {
                metrics::ENOENT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                metrics::FUSE_LATENCY.record(t0.elapsed());
                reply.error(e);
                return;
            }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(meta)) => {
                let path = db::join(&parent_path, &name);
                let ino = self.ino_for(&path);
                reply.entry(&TTL, &Self::attr(ino, &meta), 0);
            }
            Ok(None) => reply.error(ENOENT),
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        let _span = info_span!("getattr", ino).entered();
        let _t0 = Instant::now();
        metrics::GETATTR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if ino == ROOT_INO {
            reply.attr(&TTL, &Self::root_attr());
            return;
        }
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let (parent, name) = split_path(&path);
        match self.db.getattr(parent, name) {
            Ok(Some(meta)) => reply.attr(&TTL, &Self::attr(ino, &meta)),
            Ok(None) => reply.error(ENOENT),
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let _span = info_span!("setattr", ino, ?size).entered();
        let _t0 = Instant::now();
        metrics::SETATTR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if ino == ROOT_INO {
            reply.attr(&TTL, &Self::root_attr());
            return;
        }
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let (parent, name) = split_path(&path);

        let meta = match self.db.getattr(parent, name) {
            Ok(Some(m)) => m,
            Ok(None) => {
                reply.error(ENOENT);
                return;
            }
            Err(e) => crate::log_and_reply!(reply, e),
        };

        if let Some(new_size) = size {
            if meta.kind == Kind::Dir {
                reply.error(EISDIR);
                return;
            }
            let mut current = match self.db.read(parent, name) {
                Ok(Some(d)) => d,
                Ok(None) => Vec::new(),
                Err(e) => crate::log_and_reply!(reply, e),
            };
            current.resize(new_size as usize, 0);
            if let Err(e) = self.db.write(parent, name, &current) {
                crate::log_and_reply!(reply, e);
            }
        }

        match self.db.getattr(parent, name) {
            Ok(Some(meta)) => reply.attr(&TTL, &Self::attr(ino, &meta)),
            Ok(None) => reply.error(ENOENT),
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let _span = info_span!("read", ino, offset, size).entered();
        let _t0 = Instant::now();
        metrics::READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let (parent, name) = split_path(&path);
        match self.db.read(parent, name) {
            Ok(Some(data)) => {
                let start = (offset as usize).min(data.len());
                let end = (start + size as usize).min(data.len());
                reply.data(&data[start..end]);
            }
            Ok(None) => reply.error(ENOENT),
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    /// Whole-blob read-modify-write, exactly as blunt as it sounds. Every
    /// write re-reads the full file, patches the byte range, and writes the
    /// full thing back. Fine for small files and for getting this working;
    /// the natural next step is chunked storage once this is the bottleneck.
    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let _span = info_span!("write", ino, offset, len = data.len()).entered();
        let _t0 = Instant::now();
        metrics::WRITE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let (parent, name) = split_path(&path);

        let mut current = match self.db.read(parent, name) {
            Ok(Some(d)) => d,
            Ok(None) => {
                reply.error(ENOENT);
                return;
            }
            Err(e) => crate::log_and_reply!(reply, e),
        };

        let offset = offset as usize;
        if current.len() < offset + data.len() {
            current.resize(offset + data.len(), 0);
        }
        current[offset..offset + data.len()].copy_from_slice(data);

        match self.db.write(parent, name, &current) {
            Ok(()) => reply.written(data.len() as u32),
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn open(&mut self, _req: &Request, ino: u64, _flags: i32, reply: ReplyOpen) {
        let _span = info_span!("open", ino).entered();
        let t0 = Instant::now();
        metrics::OPEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        metrics::FUSE_LATENCY.record(t0.elapsed());
        reply.opened(0, 0);
    }

    fn fsync(&mut self, _req: &Request, ino: u64, _fh: u64, _datasync: bool, reply: ReplyEmpty) {
        let _span = info_span!("fsync", ino).entered();
        let t0 = Instant::now();
        metrics::FSYNC_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        metrics::FUSE_LATENCY.record(t0.elapsed());
        // Whole-blob writes go straight to Postgres; nothing buffered in userspace.
        reply.ok();
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        let _span = info_span!("rename", parent, name = %name.to_string_lossy(), newparent, newname = %newname.to_string_lossy(), flags).entered();
        let _t0 = Instant::now();
        metrics::RENAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if flags != 0 {
            reply.error(EINVAL);
            return;
        }

        let (old_parent_path, old_name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => {
                reply.error(e);
                return;
            }
        };
        let (new_parent_path, new_name) = match self.resolve(newparent, newname) {
            Ok(k) => k,
            Err(e) => {
                reply.error(e);
                return;
            }
        };

        let old_path = db::join(&old_parent_path, &old_name);
        let new_path = db::join(&new_parent_path, &new_name);

        if new_path == old_path {
            reply.ok();
            return;
        }
        if new_path.starts_with(&format!("{old_path}/")) {
            reply.error(EINVAL);
            return;
        }

        let source_meta = match self.db.getattr(&old_parent_path, &old_name) {
            Ok(Some(m)) => m,
            Ok(None) => {
                reply.error(ENOENT);
                return;
            }
            Err(e) => crate::log_and_reply!(reply, e),
        };

        // POSIX: rename replaces the target; validate kind compatibility.
        if let Ok(Some(target_meta)) = self.db.getattr(&new_parent_path, &new_name) {
            match (source_meta.kind, target_meta.kind) {
                (Kind::File, Kind::Dir) => {
                    reply.error(libc::EISDIR);
                    return;
                }
                (Kind::Dir, Kind::File) => {
                    reply.error(libc::ENOTDIR);
                    return;
                }
                (Kind::Dir, Kind::Dir) => {
                    // Target is a directory — only allow overwrite if empty.
                    let target_path = db::join(&new_parent_path, &new_name);
                    match self.db.list(&target_path) {
                        Ok(children) if !children.is_empty() => {
                            reply.error(ENOTEMPTY);
                            return;
                        }
                        Ok(_) => {} // empty dir, allow overwrite
                        Err(e) => crate::log_and_reply!(reply, e),
                    }
                }
                _ => {} // file→file: db handles overwrite
            }
        }

        match self
            .db
            .rename(&old_parent_path, &old_name, &new_parent_path, &new_name)
        {
            Ok(()) => {
                self.rekey_path_maps(&old_path, &new_path);
                reply.ok();
            }
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let _span = info_span!("create", parent, name = %name.to_string_lossy()).entered();
        let _t0 = Instant::now();
        metrics::CREATE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => {
                reply.error(e);
                return;
            }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(_)) => {
                reply.error(libc::EEXIST);
                return;
            }
            Ok(None) => {}
            Err(e) => crate::log_and_reply!(reply, e),
        }

        if let Err(e) = self.db.create(&parent_path, &name) {
            crate::log_and_reply!(reply, e);
        }

        let path = db::join(&parent_path, &name);
        let ino = self.ino_for(&path);
        let attr = Self::file_attr(ino, 0, std::time::SystemTime::now());
        reply.created(&TTL, &attr, 0, 0, 0);
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let _span = info_span!("mkdir", parent, name = %name.to_string_lossy()).entered();
        let _t0 = Instant::now();
        metrics::MKDIR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => {
                reply.error(e);
                return;
            }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(_)) => {
                reply.error(libc::EEXIST);
                return;
            }
            Ok(None) => {}
            Err(e) => crate::log_and_reply!(reply, e),
        }

        if let Err(e) = self.db.mkdir(&parent_path, &name) {
            crate::log_and_reply!(reply, e);
        }

        let path = db::join(&parent_path, &name);
        let ino = self.ino_for(&path);
        reply.entry(&TTL, &Self::dir_attr(ino), 0);
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let _span = info_span!("unlink", parent, name = %name.to_string_lossy()).entered();
        let _t0 = Instant::now();
        metrics::UNLINK_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => {
                reply.error(e);
                return;
            }
        };

        match self.db.unlink(&parent_path, &name) {
            Ok(()) => {
                let path = db::join(&parent_path, &name);
                if let Some(ino) = self.ino_by_path.remove(&path) {
                    self.path_by_ino.remove(&ino);
                }
                reply.ok();
            }
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let _span = info_span!("rmdir", parent, name = %name.to_string_lossy()).entered();
        let _t0 = Instant::now();
        metrics::RMDIR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => {
                reply.error(e);
                return;
            }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(_)) => {}
            Ok(None) => {
                reply.error(ENOENT);
                return;
            }
            Err(e) => crate::log_and_reply!(reply, e),
        }

        match self.db.rmdir(&parent_path, &name) {
            Ok(true) => {
                let path = db::join(&parent_path, &name);
                if let Some(ino) = self.ino_by_path.remove(&path) {
                    self.path_by_ino.remove(&ino);
                }
                reply.ok();
            }
            Ok(false) => reply.error(ENOTEMPTY),
            Err(e) => crate::log_and_reply!(reply, e),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let _span = info_span!("readdir", ino, offset).entered();
        let _t0 = Instant::now();
        metrics::READDIR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut entries: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".to_string()),
            (
                self.parent_ino(&path),
                FileType::Directory,
                "..".to_string(),
            ),
        ];

        match self.db.list(&path) {
            Ok(children) => {
                for child in children {
                    let child_path = db::join(&path, &child.name);
                    let child_ino = self.ino_for(&child_path);
                    let kind = match child.kind {
                        Kind::Dir => FileType::Directory,
                        Kind::File => FileType::RegularFile,
                    };
                    entries.push((child_ino, kind, child.name));
                }
            }
            Err(e) => crate::log_and_reply!(reply, e),
        }

        for (i, (child_ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            // The `i + 1` becomes the offset the kernel passes back on the
            // next readdir call — it must be non-zero and monotonically
            // increasing, not a real byte offset.
            if reply.add(child_ino, (i + 1) as i64, kind, name) {
                break; // reply buffer full; kernel will call again with a new offset
            }
        }
        reply.ok();
    }
}

impl PgFs {
    /// Turn a FUSE (parent ino, name) pair into a Db (parent path, name) key.
    fn resolve(&mut self, parent: u64, name: &OsStr) -> Result<(String, String), i32> {
        let parent_path = match self.path_of_ino(parent) {
            Some(p) => p,
            None => return Err(ENOENT),
        };
        let name = match name.to_str() {
            Some(n) => n.to_string(),
            None => return Err(ENOENT),
        };
        Ok((parent_path, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── split_path ──────────────────────────────────────────────────────

    #[test]
    fn split_path_root_entry() {
        assert_eq!(split_path("foo"), ("", "foo"));
    }

    #[test]
    fn split_path_nested() {
        assert_eq!(split_path("a/b"), ("a", "b"));
    }

    #[test]
    fn split_path_deeply_nested() {
        assert_eq!(split_path("a/b/c/d"), ("a/b/c", "d"));
    }

    #[test]
    fn split_path_single_char() {
        assert_eq!(split_path("x"), ("", "x"));
    }

    // ── join (db module, but used extensively in fs) ────────────────────

    #[test]
    fn join_root_entry() {
        assert_eq!(db::join("", "foo"), "foo");
    }

    #[test]
    fn join_nested_entry() {
        assert_eq!(db::join("a", "b"), "a/b");
        assert_eq!(db::join("a/b", "c"), "a/b/c");
    }

    // ── ino_for / path_of_ino (tested via the standalone logic) ─────────

    #[test]
    fn ino_for_is_stable() {
        let mut ino_by_path: HashMap<String, u64> = HashMap::new();
        let mut path_by_ino: HashMap<u64, String> = HashMap::new();
        let mut next_ino = 5u64;

        let mut alloc = |path: &str| -> u64 {
            *ino_by_path.entry(path.to_string()).or_insert_with(|| {
                let ino = next_ino;
                next_ino += 1;
                path_by_ino.insert(ino, path.to_string());
                ino
            })
        };

        let a = alloc("foo");
        let b = alloc("foo"); // same path
        let c = alloc("bar");

        assert_eq!(a, b, "same path should get the same inode");
        assert_ne!(a, c, "different paths should get different inodes");
        assert_eq!(next_ino, 7);
        assert_eq!(path_by_ino.get(&a), Some(&"foo".to_string()));
    }

    // ── rekey_maps ──────────────────────────────────────────────────────

    #[test]
    fn rekey_single_file() {
        let mut ino_by_path: HashMap<String, u64> = {
            let mut m = HashMap::new();
            m.insert("old.txt".to_string(), 10);
            m
        };
        let mut path_by_ino: HashMap<u64, String> = {
            let mut m = HashMap::new();
            m.insert(10, "old.txt".to_string());
            m
        };

        rekey_maps(&mut ino_by_path, &mut path_by_ino, "old.txt", "new.txt");

        assert!(ino_by_path.get("old.txt").is_none());
        assert_eq!(ino_by_path.get("new.txt"), Some(&10));
        assert_eq!(path_by_ino.get(&10), Some(&"new.txt".to_string()));
    }

    #[test]
    fn rekey_dir_with_descendants() {
        let mut ino_by_path: HashMap<String, u64> = {
            let mut m = HashMap::new();
            m.insert("src".to_string(), 10);
            m.insert("src/file.txt".to_string(), 11);
            m.insert("src/sub/deep.txt".to_string(), 12);
            m.insert("unrelated.txt".to_string(), 99);
            m
        };
        let mut path_by_ino: HashMap<u64, String> = {
            let mut m = HashMap::new();
            m.insert(10, "src".to_string());
            m.insert(11, "src/file.txt".to_string());
            m.insert(12, "src/sub/deep.txt".to_string());
            m.insert(99, "unrelated.txt".to_string());
            m
        };

        rekey_maps(&mut ino_by_path, &mut path_by_ino, "src", "dest");

        // Old paths gone.
        assert!(ino_by_path.get("src").is_none());
        assert!(ino_by_path.get("src/file.txt").is_none());
        assert!(ino_by_path.get("src/sub/deep.txt").is_none());

        // New paths mapped.
        assert_eq!(ino_by_path.get("dest"), Some(&10));
        assert_eq!(ino_by_path.get("dest/file.txt"), Some(&11));
        assert_eq!(ino_by_path.get("dest/sub/deep.txt"), Some(&12));

        // path_by_ino updated.
        assert_eq!(path_by_ino.get(&10), Some(&"dest".to_string()));
        assert_eq!(path_by_ino.get(&11), Some(&"dest/file.txt".to_string()));

        // Unrelated path untouched.
        assert_eq!(ino_by_path.get("unrelated.txt"), Some(&99));
    }

    #[test]
    fn rekey_nonexistent_path_is_noop() {
        let mut ino_by_path: HashMap<String, u64> = {
            let mut m = HashMap::new();
            m.insert("a.txt".to_string(), 5);
            m
        };
        let mut path_by_ino: HashMap<u64, String> = {
            let mut m = HashMap::new();
            m.insert(5, "a.txt".to_string());
            m
        };

        rekey_maps(&mut ino_by_path, &mut path_by_ino, "ghost", "nope");

        assert_eq!(ino_by_path.get("a.txt"), Some(&5));
        assert!(ino_by_path.get("ghost").is_none());
        assert!(ino_by_path.get("nope").is_none());
    }

    // ── attr generation ─────────────────────────────────────────────────

    #[test]
    fn root_attr_is_directory() {
        let attr = PgFs::root_attr();
        assert_eq!(attr.ino, ROOT_INO);
        assert_eq!(attr.kind, FileType::Directory);
        assert_eq!(attr.perm, 0o755);
    }

    #[test]
    fn file_attr_has_correct_permissions() {
        let mtime = UNIX_EPOCH;
        let attr = PgFs::file_attr(42, 1024, mtime);
        assert_eq!(attr.ino, 42);
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.perm, 0o644);
        assert_eq!(attr.size, 1024);
        assert_eq!(attr.nlink, 1);
        assert_eq!(attr.blocks, 1024_u64.div_ceil(512));
    }

    #[test]
    fn dir_attr_has_correct_permissions() {
        let attr = PgFs::dir_attr(7);
        assert_eq!(attr.ino, 7);
        assert_eq!(attr.kind, FileType::Directory);
        assert_eq!(attr.perm, 0o755);
        assert_eq!(attr.nlink, 2);
        assert_eq!(attr.size, 0);
    }
}

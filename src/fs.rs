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
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use libc::{EISDIR, ENOENT, ENOTEMPTY};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};

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
}

impl Filesystem for PgFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => { reply.error(e); return; }
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
        if ino == ROOT_INO {
            reply.attr(&TTL, &Self::root_attr());
            return;
        }
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
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
        if ino == ROOT_INO {
            reply.attr(&TTL, &Self::root_attr());
            return;
        }
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        };
        let (parent, name) = split_path(&path);

        let meta = match self.db.getattr(parent, name) {
            Ok(Some(m)) => m,
            Ok(None) => { reply.error(ENOENT); return; }
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
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
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
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        };
        let (parent, name) = split_path(&path);

        let mut current = match self.db.read(parent, name) {
            Ok(Some(d)) => d,
            Ok(None) => { reply.error(ENOENT); return; }
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

    fn open(&mut self, _req: &Request, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, 0);
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
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => { reply.error(e); return; }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(_)) => { reply.error(libc::EEXIST); return; }
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
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => { reply.error(e); return; }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(_)) => { reply.error(libc::EEXIST); return; }
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
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => { reply.error(e); return; }
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
        let (parent_path, name) = match self.resolve(parent, name) {
            Ok(k) => k,
            Err(e) => { reply.error(e); return; }
        };

        match self.db.getattr(&parent_path, &name) {
            Ok(Some(_)) => {}
            Ok(None) => { reply.error(ENOENT); return; }
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
        let path = match self.path_of_ino(ino) {
            Some(p) => p,
            None => { reply.error(ENOENT); return; }
        };

        let mut entries: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".to_string()),
            (self.parent_ino(&path), FileType::Directory, "..".to_string()),
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

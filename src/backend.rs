//! The storage interface the FUSE layer talks to (spec/optimize.py
//! PgfsBackendTraitReq): one method per `pgfs.*` SQL function. The daemon
//! cannot tell `PgBackend` (src/pg.rs) from the in-memory reference model
//! (tests/model/), which implements the same trait.
//!
//! The trait is synchronous: each dispatch worker owns one backend and
//! calls it from its own thread (spec/concurrency.py WorkerOwnsConnection).

use crate::errno::FsResult;

/// The kind of an inode, as stored in `inodes.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Fifo,
    Socket,
    CharDev,
    BlockDev,
}

impl Kind {
    pub fn from_char(c: i8) -> Kind {
        match c as u8 {
            b'd' => Kind::Dir,
            b'l' => Kind::Symlink,
            b'p' => Kind::Fifo,
            b's' => Kind::Socket,
            b'c' => Kind::CharDev,
            b'b' => Kind::BlockDev,
            _ => Kind::File,
        }
    }

    pub fn as_char(self) -> i8 {
        (match self {
            Kind::File => b'f',
            Kind::Dir => b'd',
            Kind::Symlink => b'l',
            Kind::Fifo => b'p',
            Kind::Socket => b's',
            Kind::CharDev => b'c',
            Kind::BlockDev => b'b',
        }) as i8
    }

    /// From the S_IFMT bits of a mode.
    pub fn from_mode(mode: u32) -> Option<Kind> {
        Some(match mode & libc::S_IFMT {
            libc::S_IFREG | 0 => Kind::File,
            libc::S_IFDIR => Kind::Dir,
            libc::S_IFLNK => Kind::Symlink,
            libc::S_IFIFO => Kind::Fifo,
            libc::S_IFSOCK => Kind::Socket,
            libc::S_IFCHR => Kind::CharDev,
            libc::S_IFBLK => Kind::BlockDev,
            _ => return None,
        })
    }

    pub fn fuse(self) -> fuser::FileType {
        match self {
            Kind::File => fuser::FileType::RegularFile,
            Kind::Dir => fuser::FileType::Directory,
            Kind::Symlink => fuser::FileType::Symlink,
            Kind::Fifo => fuser::FileType::NamedPipe,
            Kind::Socket => fuser::FileType::Socket,
            Kind::CharDev => fuser::FileType::CharDevice,
            Kind::BlockDev => fuser::FileType::BlockDevice,
        }
    }
}

/// Attributes of an inode (`pgfs.attr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub ino: u64,
    pub gen: u64,
    pub kind: Kind,
    /// Permission and set-id bits, without the type bits.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub size: u64,
    /// 512-byte units of stored data.
    pub blocks: u64,
    pub atime_ns: i64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub btime_ns: i64,
    pub rdev: u64,
    pub version: i64,
}

/// A view of a tree: its layer, the epoch (None: the live view) and the
/// directory that kernel inode 1 maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub layer: i64,
    pub epoch: Option<i64>,
    pub root: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: Vec<u8>,
    pub ino: u64,
    pub kind: Kind,
}

/// Committed content of a byte range, block by block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadResult {
    /// The file stores its content inline (block 0 only).
    pub inline: bool,
    /// (block number, content); a missing block is a hole.
    pub blocks: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub struct MakeArgs {
    pub parent: u64,
    pub name: Vec<u8>,
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
    pub target: Option<Vec<u8>>,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnlinkOutcome {
    pub ino: u64,
    pub nlink: u32,
    pub orphaned: bool,
}

#[derive(Debug, Clone)]
pub struct RenameArgs {
    pub parent: u64,
    pub name: Vec<u8>,
    pub newparent: u64,
    pub newname: Vec<u8>,
    pub flags: u32,
    pub open: Vec<u64>,
    pub dirty: Vec<u64>,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenameOutcome {
    pub ino: u64,
    /// The replaced target: (ino, remaining links, orphaned).
    pub replaced: Option<(u64, u32, bool)>,
}

#[derive(Debug, Clone, Default)]
pub struct SetAttrArgs {
    pub ino: u64,
    pub gen: Option<u64>,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    /// The block now holding the end of the file, tail zeroed, or the
    /// inline content as block 0 when an inline file grows past inline_max.
    pub tail: Option<(u64, Vec<u8>)>,
    pub atime_ns: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub caller_uid: Option<u32>,
    pub now: i64,
}

#[derive(Debug, Clone, Default)]
pub struct WriteArgs {
    pub ino: u64,
    pub gen: Option<u64>,
    pub size: u64,
    /// New inline content (the file stays inline).
    pub inline: Option<Vec<u8>>,
    /// Whole blocks to store as new blobs.
    pub blocks: Vec<(u64, Vec<u8>)>,
    /// Blocks to make read as zero, applied first.
    pub hole: Option<(u64, u64)>,
    pub mtime_ns: Option<i64>,
    pub now: i64,
    pub writer_uids: Vec<u32>,
    pub sync: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CopyArgs {
    pub ino_in: u64,
    pub gen_in: Option<u64>,
    pub ino_out: u64,
    pub gen_out: Option<u64>,
    pub src_first: u64,
    pub dst_first: u64,
    pub count: u64,
    pub blocks: Vec<(u64, Vec<u8>)>,
    pub size: u64,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub physical_bytes: u64,
    pub inodes: u64,
}

/// XATTR_CREATE / XATTR_REPLACE.
pub const XATTR_CREATE: i32 = 1;
pub const XATTR_REPLACE: i32 = 2;

/// RENAME_NOREPLACE / RENAME_EXCHANGE / RENAME_WHITEOUT.
pub const RENAME_NOREPLACE: u32 = 1;
pub const RENAME_EXCHANGE: u32 = 2;
pub const RENAME_WHITEOUT: u32 = 4;

pub trait PgfsBackend: Send {
    // ── Reads ────────────────────────────────────────────────────────────
    fn lookup(&mut self, v: &View, parent: u64, name: &[u8]) -> FsResult<Option<Attr>>;
    fn getattr(&mut self, v: &View, ino: u64, gen: Option<u64>) -> FsResult<Option<Attr>>;
    fn readdir(
        &mut self,
        v: &View,
        dir: u64,
        after: Option<&[u8]>,
        limit: u32,
    ) -> FsResult<Vec<DirEntry>>;
    fn readdirplus(
        &mut self,
        v: &View,
        dir: u64,
        after: Option<&[u8]>,
        limit: u32,
    ) -> FsResult<Vec<(Vec<u8>, Attr)>>;
    fn read_blocks(
        &mut self,
        v: &View,
        ino: u64,
        gen: Option<u64>,
        first: u64,
        last: u64,
    ) -> FsResult<ReadResult>;
    fn readlink(&mut self, v: &View, ino: u64) -> FsResult<Vec<u8>>;
    fn getxattr(&mut self, v: &View, ino: u64, name: &[u8]) -> FsResult<Option<Vec<u8>>>;
    fn listxattr(&mut self, v: &View, ino: u64) -> FsResult<Vec<Vec<u8>>>;
    fn seek_block(&mut self, v: &View, ino: u64, block: u64, data: bool) -> FsResult<Option<u64>>;
    fn statfs(&mut self) -> FsResult<Usage>;

    // ── Mutations, each one transaction ──────────────────────────────────
    fn make(&mut self, layer: i64, a: &MakeArgs) -> FsResult<Attr>;
    fn link(
        &mut self,
        layer: i64,
        ino: u64,
        gen: Option<u64>,
        newparent: u64,
        newname: &[u8],
        now: i64,
    ) -> FsResult<Attr>;
    fn unlink(
        &mut self,
        layer: i64,
        parent: u64,
        name: &[u8],
        open: &[u64],
        dirty: &[u64],
        now: i64,
    ) -> FsResult<UnlinkOutcome>;
    fn rmdir(&mut self, layer: i64, parent: u64, name: &[u8], now: i64) -> FsResult<u64>;
    fn rename(&mut self, layer: i64, a: &RenameArgs) -> FsResult<RenameOutcome>;
    fn setattr(&mut self, layer: i64, a: &SetAttrArgs) -> FsResult<Attr>;
    fn write_blocks(&mut self, layer: i64, a: &WriteArgs) -> FsResult<Attr>;
    fn copy_range(&mut self, layer: i64, a: &CopyArgs) -> FsResult<Attr>;
    fn setxattr(
        &mut self,
        layer: i64,
        ino: u64,
        name: &[u8],
        value: &[u8],
        flags: i32,
        now: i64,
    ) -> FsResult<()>;
    fn removexattr(&mut self, layer: i64, ino: u64, name: &[u8], now: i64) -> FsResult<()>;
    fn release_orphan(&mut self, layer: i64, ino: u64) -> FsResult<bool>;
    /// The synchronous barrier (spec/durability.py FsyncForcesEarlierCommits).
    fn sync_barrier(&mut self) -> FsResult<()>;

    // ── Trees (spec/branch.py) ───────────────────────────────────────────
    fn snapshot(&mut self, tree: &str, name: &str) -> FsResult<()>;
    fn branch(&mut self, source: &str, snapshot: Option<&str>, new: &str) -> FsResult<()>;
    fn promote(&mut self, branch: &str, tree: &str, force: bool) -> FsResult<()>;

    /// The database backend pid serving this backend, for logs and the
    /// state dump (0 when there is none).
    fn backend_pid(&self) -> i32 {
        0
    }
}

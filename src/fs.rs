//! The FUSE layer: a dispatcher that routes every request to the worker
//! that owns its inode or name (spec/concurrency.py InodeShardedWorkers),
//! and the workers that run them against their own backend.
//!
//! The dispatcher (fuser's session thread) never blocks on the database:
//! it only routes. Each worker owns its inodes' open handles and write
//! buffers, so no lock protects them; work that needs another worker's
//! state is requested by message (spec/concurrency.py
//! NoLockAcrossRoundTrip).

use crate::backend::*;
use crate::buffer::InodeBuffer;
use crate::cache::Cache;
use crate::errno::{errno_name, FsError, FsResult};
use crate::health::Health;
use crate::metrics::{self, count_errno, count_op, FuseLatencyGuard};
use crate::names::check_name;
use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender};
use fuser::{
    FileAttr, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyDirectoryPlus, ReplyEmpty, ReplyEntry, ReplyLseek, ReplyOpen, ReplyStatfs, ReplyWrite,
    ReplyXattr, Request, TimeOrNow,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Nanoseconds since the Unix epoch.
pub fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn ns_to_time(ns: i64) -> SystemTime {
    if ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(ns.unsigned_abs())
    }
}

fn time_to_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

/// The largest file (spec/posix.py FileSizeLimit).
pub const MAX_FILE_SIZE: u64 = i64::MAX as u64;

/// Mount configuration the workers need.
#[derive(Debug, Clone)]
pub struct FsConfig {
    pub workers: usize,
    /// Entry and attribute TTL (spec/consistency.py KernelCacheContract).
    pub ttl: Duration,
    pub read_only: bool,
    pub snapshot: bool,
    pub buffer_per_file: usize,
    pub buffer_total: usize,
    pub block_size: u64,
    pub inline_max: u64,
    pub capacity: Option<u64>,
}

/// One worker's in-flight request, read by the watchdog and the state dump
/// without locks (spec/failure.py InFlightAgeWatchdog).
#[derive(Default)]
pub struct Inflight {
    /// Milliseconds since `EPOCH_START` when the request started; 0: idle.
    pub started_ms: AtomicU64,
    pub op: AtomicUsize,
    pub ino: AtomicU64,
    pub warned: AtomicBool,
    pub cancelled: AtomicBool,
}

static EPOCH_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn mono_ms() -> u64 {
    EPOCH_START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

pub fn op_name(i: usize) -> &'static str {
    metrics::OPS.get(i).copied().unwrap_or("?")
}

/// Messages between workers, and from control threads to workers.
pub enum Peer {
    /// Commit these inodes' buffers (owned by the receiver).
    Flush(Vec<u64>, Sender<FsResult<()>>),
    /// Commit every buffer; reply with the number of failures.
    FlushAll(Sender<usize>),
    /// Drain barrier: commit everything, then defer new mutating requests
    /// (spec/branch.py DrainsOwnerBeforeSnapshot).
    Hold(Sender<usize>),
    Release,
    /// Run the synchronous barrier on this worker's slot.
    Barrier(Sender<FsResult<()>>),
    /// Forget per-inode state the kernel no longer holds.
    Evict(u64),
    /// Open a handle on an inode this worker owns (after a create elsewhere).
    Open(u64, i32, Sender<FsResult<(u64, u32)>>),
    Stop,
}

type JobFn = Box<dyn FnOnce(&mut Worker) + Send>;

struct Job {
    mutating: bool,
    op: usize,
    ino: u64,
    f: JobFn,
}

/// State shared by the dispatcher, the workers and the control threads.
pub struct Shared {
    pub cfg: FsConfig,
    pub view: View,
    pub tree: String,
    pub cache: Cache,
    pub health: Arc<Health>,
    jobs: Vec<Sender<Job>>,
    pub peers: Vec<Sender<Peer>>,
    pub buffered_total: AtomicUsize,
    pub inflight: Vec<Inflight>,
    pub notifier: Mutex<Option<fuser::Notifier>>,
    next_fh: AtomicU64,
    usage: Mutex<Option<(Instant, Usage)>>,
    pub handles_open: AtomicUsize,
    pub cancel_slots: Vec<crate::pg::CancelSlot>,
    pub backend_pids: Vec<AtomicU64>,
}

impl Shared {
    fn op_index(op: &str) -> usize {
        metrics::OPS.iter().position(|o| *o == op).unwrap_or(0)
    }

    /// Kernel inode 1 is the view's root (spec/branch.py ViewsRootedAtDirectory).
    pub fn db(&self, k: u64) -> u64 {
        if k == fuser::FUSE_ROOT_ID {
            self.view.root
        } else {
            k
        }
    }

    pub fn kino(&self, d: u64) -> u64 {
        if d == self.view.root {
            fuser::FUSE_ROOT_ID
        } else {
            d
        }
    }

    pub fn owner(&self, ino: u64) -> usize {
        (ino % self.cfg.workers as u64) as usize
    }

    pub fn name_owner(&self, parent: u64, name: &[u8]) -> usize {
        let mut h = crc32c::crc32c(&parent.to_le_bytes());
        h = crc32c::crc32c_append(h, name);
        (h as u64 % self.cfg.workers as u64) as usize
    }

    fn submit(&self, w: usize, mutating: bool, op: &'static str, ino: u64, f: JobFn) {
        count_op(op);
        let job = Job {
            mutating,
            op: Self::op_index(op),
            ino,
            f,
        };
        if let Err(e) = self.jobs[w].send(job) {
            tracing::error!("worker {w} is gone; request {op} dropped: {e}");
        }
    }

    pub fn invalidate_kernel(&self, ino: u64) {
        if let Ok(g) = self.notifier.lock() {
            if let Some(n) = g.as_ref() {
                let _ = n.inval_inode(self.kino(ino), 0, 0);
            }
        }
    }

    fn fattr(&self, a: &Attr) -> FileAttr {
        let a = self.cache.with_buffered(a.clone());
        FileAttr {
            ino: self.kino(a.ino),
            size: a.size,
            blocks: a.blocks,
            atime: ns_to_time(a.atime_ns),
            mtime: ns_to_time(a.mtime_ns),
            ctime: ns_to_time(a.ctime_ns),
            crtime: ns_to_time(a.btime_ns),
            kind: a.kind.fuse(),
            perm: (a.mode & 0o7777) as u16,
            nlink: a.nlink,
            uid: a.uid,
            gid: a.gid,
            rdev: a.rdev as u32,
            blksize: self.cfg.block_size as u32,
            flags: 0,
        }
    }

    pub fn alloc_fh(&self) -> u64 {
        self.next_fh.fetch_add(1, Ordering::Relaxed)
    }

    /// Ask every worker something and wait for all answers (control threads).
    pub fn broadcast<T: Send + 'static>(
        &self,
        make: impl Fn(Sender<T>) -> Peer,
        timeout: Duration,
    ) -> Vec<Option<T>> {
        let mut rxs = Vec::new();
        for p in &self.peers {
            let (tx, rx) = bounded(1);
            let _ = p.send(make(tx));
            rxs.push(rx);
        }
        let deadline = Instant::now() + timeout;
        rxs.into_iter()
            .map(|rx| {
                rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok()
            })
            .collect()
    }

    pub fn release_all(&self) {
        for p in &self.peers {
            let _ = p.send(Peer::Release);
        }
    }

    /// Cached usage for statfs and ENOSPC, refreshed at most every 5 s
    /// (spec/space.py ReportsStatfs, CapacityAndEnospc).
    fn cached_usage(&self) -> Option<Usage> {
        self.usage.lock().ok().and_then(|g| g.map(|(_, u)| u))
    }

    fn usage_stale(&self) -> bool {
        self.usage
            .lock()
            .map(|g| g.is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(5)))
            .unwrap_or(true)
    }

    fn set_usage(&self, u: Usage) {
        if let Ok(mut g) = self.usage.lock() {
            *g = Some((Instant::now(), u));
        }
    }
}

/// A file handle, owned by the inode's worker.
#[derive(Debug, Clone)]
struct Handle {
    ino: u64,
    readable: bool,
    writable: bool,
    append: bool,
    /// O_SYNC / O_DSYNC: commit synchronously at every write.
    sync: bool,
    /// O_DIRECT: commit at every write, asynchronously.
    direct: bool,
    last_write_seq: u64,
    seen_err_seq: u64,
}

/// A directory handle: offsets handed to the kernel map to names, so the
/// next page resumes with `name > cookie` (spec/app.py ListsDirectories).
#[derive(Debug, Default)]
struct DirHandle {
    names: HashMap<i64, Vec<u8>>,
    next: i64,
}

pub struct Worker {
    pub id: usize,
    sh: Arc<Shared>,
    be: Box<dyn PgfsBackend>,
    buffers: HashMap<u64, InodeBuffer>,
    buffer_bytes: HashMap<u64, usize>,
    handles: HashMap<u64, Handle>,
    dirs: HashMap<u64, DirHandle>,
    jobs: Receiver<Job>,
    peer_rx: Receiver<Peer>,
    deferred: VecDeque<Job>,
    held: bool,
}

const READDIR_PAGE: u32 = 256;

impl Worker {
    fn view(&self) -> View {
        self.sh.view
    }

    fn rw(&self) -> FsResult<()> {
        if self.sh.cfg.read_only {
            Err(FsError::Errno(libc::EROFS))
        } else {
            Ok(())
        }
    }

    fn gen(&self, ino: u64) -> Option<u64> {
        self.sh.cache.gen(ino)
    }

    fn layer(&self) -> i64 {
        self.sh.view.layer
    }

    // ── Worker loop ─────────────────────────────────────────────────────

    fn run(mut self) {
        loop {
            while let Ok(m) = self.peer_rx.try_recv() {
                if !self.handle_peer(m) {
                    return;
                }
            }
            if !self.held {
                if let Some(j) = self.deferred.pop_front() {
                    self.run_job(j);
                    continue;
                }
            }
            select! {
                recv(self.peer_rx) -> m => match m {
                    Ok(m) => if !self.handle_peer(m) { return },
                    Err(_) => return,
                },
                recv(self.jobs) -> j => match j {
                    Ok(j) => {
                        if self.held && j.mutating {
                            self.deferred.push_back(j);
                        } else {
                            self.run_job(j);
                        }
                    }
                    Err(_) => return,
                },
            }
        }
    }

    fn run_job(&mut self, j: Job) {
        let slot = &self.sh.inflight[self.id];
        slot.op.store(j.op, Ordering::Relaxed);
        slot.ino.store(j.ino, Ordering::Relaxed);
        slot.warned.store(false, Ordering::Relaxed);
        slot.cancelled.store(false, Ordering::Relaxed);
        slot.started_ms.store(mono_ms(), Ordering::Release);
        self.sh.backend_pids[self.id].store(self.be.backend_pid() as u64, Ordering::Relaxed);
        let span = tracing::info_span!("fuse", op = op_name(j.op), ino = j.ino, worker = self.id, view = %self.sh.tree);
        let _g = span.enter();
        let _lat = FuseLatencyGuard::new();
        (j.f)(self);
        self.sh.inflight[self.id]
            .started_ms
            .store(0, Ordering::Release);
    }

    /// Returns false to stop the worker.
    fn handle_peer(&mut self, m: Peer) -> bool {
        match m {
            Peer::Flush(inos, tx) => {
                let mut r = Ok(());
                for ino in inos {
                    if let Err(e) = self.commit(ino, false) {
                        r = Err(e);
                    }
                }
                let _ = tx.send(r);
            }
            Peer::FlushAll(tx) => {
                let _ = tx.send(self.commit_all());
            }
            Peer::Hold(tx) => {
                let failures = self.commit_all();
                self.held = true;
                let _ = tx.send(failures);
            }
            Peer::Release => self.held = false,
            Peer::Barrier(tx) => {
                let _ = tx.send(self.be.sync_barrier());
            }
            Peer::Evict(ino) => {
                if !self.handles.values().any(|h| h.ino == ino)
                    && self.buffers.get(&ino).is_none_or(|b| !b.is_dirty())
                {
                    self.buffers.remove(&ino);
                    self.buffer_bytes.remove(&ino);
                }
            }
            Peer::Open(ino, flags, tx) => {
                let _ = tx.send(self.open(ino, flags));
            }
            Peer::Stop => return false,
        }
        true
    }

    /// Ask another worker and wait, servicing our own peer mailbox meanwhile
    /// so that wait chains have depth 1 and cannot deadlock.
    fn ask<T>(&mut self, target: usize, make: impl FnOnce(Sender<T>) -> Peer) -> Option<T> {
        let (tx, rx) = bounded(1);
        if self.sh.peers[target].send(make(tx)).is_err() {
            return None;
        }
        loop {
            select! {
                recv(rx) -> r => return r.ok(),
                recv(self.peer_rx) -> m => if let Ok(m) = m {
                    // A Stop while waiting is honoured after the answer.
                    if !matches!(m, Peer::Stop) {
                        self.handle_peer(m);
                    }
                },
            }
        }
    }

    // ── Buffers ─────────────────────────────────────────────────────────

    fn account(&mut self, ino: u64) {
        let now = self.buffers.get(&ino).map(InodeBuffer::bytes).unwrap_or(0);
        let before = self.buffer_bytes.insert(ino, now).unwrap_or(0);
        if now >= before {
            self.sh
                .buffered_total
                .fetch_add(now - before, Ordering::Relaxed);
        } else {
            self.sh
                .buffered_total
                .fetch_sub(before - now, Ordering::Relaxed);
        }
        let (size, mtime) = match self.buffers.get(&ino) {
            Some(b) if b.is_dirty() => (Some(b.size), b.mtime_ns),
            _ => (None, None),
        };
        self.sh.cache.set_summary(ino, size, mtime);
    }

    fn buffer(&mut self, ino: u64) -> FsResult<&mut InodeBuffer> {
        if !self.buffers.contains_key(&ino) {
            let a = self.attr(ino)?;
            self.buffers.insert(
                ino,
                InodeBuffer::new(ino, Some(a.gen), a.size, self.sh.cfg.block_size),
            );
        }
        Ok(self.buffers.get_mut(&ino).expect("inserted"))
    }

    /// Commit the inode's buffer as one transaction (spec/optimize.py
    /// BufferedWriteFlushReq).
    fn commit(&mut self, ino: u64, sync: bool) -> FsResult<()> {
        let Some(buf) = self.buffers.get(&ino) else {
            if sync {
                return self.be.sync_barrier();
            }
            return Ok(());
        };
        if !buf.is_dirty() {
            if sync {
                return self.be.sync_barrier();
            }
            return Ok(());
        }
        let bs = self.sh.cfg.block_size;
        let inline_max = self.sh.cfg.inline_max;
        let plan = buf.plan(inline_max);
        let seq = buf.write_seq;
        let gen = buf.gen;
        let view = self.view();
        let mut committed = BTreeMap::new();
        let mut inline = None;
        let mut first = plan.partial.first().copied();
        let mut last = plan.partial.last().copied();
        if plan.probe_inline {
            first = Some(0);
            last = Some(last.unwrap_or(0));
        }
        let result = (|| -> FsResult<Attr> {
            if let (Some(f), Some(l)) = (first, last) {
                let r = self.be.read_blocks(&view, ino, gen, f, l)?;
                if r.inline {
                    inline = r
                        .blocks
                        .first()
                        .map(|(_, d)| d.clone())
                        .or(Some(Vec::new()));
                } else {
                    committed.extend(r.blocks);
                }
            }
            let buf = self.buffers.get(&ino).expect("checked");
            let args = buf.build(inline_max, inline.as_deref(), &committed, now_ns(), sync);
            let _ = bs;
            self.be.write_blocks(self.layer(), &args)
        })();
        match result {
            Ok(attr) => {
                let buf = self.buffers.get_mut(&ino).expect("checked");
                buf.committed(seq);
                self.sh.cache.put(&attr, 0);
                self.account(ino);
                Ok(())
            }
            Err(e) => {
                if let Some(buf) = self.buffers.get_mut(&ino) {
                    buf.failed();
                }
                tracing::error!("commit of inode {ino} failed: {e}");
                Err(e)
            }
        }
    }

    fn commit_all(&mut self) -> usize {
        let mut inos: Vec<(Option<Instant>, u64)> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.is_dirty())
            .map(|(&i, b)| (b.since, i))
            .collect();
        inos.sort();
        let mut failures = 0;
        for (_, ino) in inos {
            if self.commit(ino, false).is_err() {
                failures += 1;
                tracing::error!("inode {ino}: buffered data could not be committed");
            }
        }
        failures
    }

    /// Commit the buffers of `inos` wherever they are owned
    /// (spec/durability.py FlushesBeforeNaming).
    fn flush_inos(&mut self, inos: &[u64]) -> FsResult<()> {
        let mut by_owner: HashMap<usize, Vec<u64>> = HashMap::new();
        for &i in inos {
            by_owner.entry(self.sh.owner(i)).or_default().push(i);
        }
        for (w, list) in by_owner {
            if w == self.id {
                for i in list {
                    self.commit(i, false)?;
                }
            } else {
                match self.ask(w, |tx| Peer::Flush(list, tx)) {
                    Some(r) => r?,
                    None => return Err(FsError::Errno(libc::EIO)),
                }
            }
        }
        Ok(())
    }

    /// Bounded buffers (spec/concurrency.py BoundedWriteBuffers).
    fn relieve_pressure(&mut self, ino: u64) -> FsResult<()> {
        if self.buffer_bytes.get(&ino).copied().unwrap_or(0) >= self.sh.cfg.buffer_per_file {
            self.commit(ino, false)?;
        }
        if self.sh.buffered_total.load(Ordering::Relaxed) < self.sh.cfg.buffer_total {
            return Ok(());
        }
        self.commit_all();
        for w in 0..self.sh.cfg.workers {
            if self.sh.buffered_total.load(Ordering::Relaxed) < self.sh.cfg.buffer_total {
                break;
            }
            if w != self.id {
                self.ask(w, Peer::FlushAll);
            }
        }
        Ok(())
    }

    // ── Attributes ──────────────────────────────────────────────────────

    /// Attributes of an inode the kernel holds: from the cache when fresh,
    /// else from the database; ESTALE if it is gone or its generation
    /// changed (spec/storage.py PersistentInodes).
    fn attr(&mut self, ino: u64) -> FsResult<Attr> {
        if let Some(a) = self.sh.cache.fresh(ino, self.sh.cfg.ttl) {
            return Ok(a);
        }
        let gen = self.gen(ino);
        let view = self.view();
        match self.be.getattr(&view, ino, gen)? {
            Some(a) => {
                self.sh.cache.put(&a, 0);
                Ok(a)
            }
            None => {
                if ino == self.sh.view.root {
                    tracing::error!("the root directory {ino} of the view is gone");
                }
                self.sh.invalidate_kernel(ino);
                Err(FsError::Errno(libc::ESTALE))
            }
        }
    }

    fn entry_attr(&self, a: &Attr) -> FileAttr {
        self.sh.fattr(a)
    }

    // ── Operations ──────────────────────────────────────────────────────

    fn lookup(&mut self, parent: u64, name: &[u8]) -> FsResult<Option<Attr>> {
        check_name(name).map_err(FsError::Errno)?;
        let view = self.view();
        let r = self.be.lookup(&view, parent, name)?;
        if let Some(a) = &r {
            self.sh.cache.put(a, 1);
        }
        Ok(r)
    }

    #[allow(clippy::too_many_arguments)]
    fn make(
        &mut self,
        req: (u32, u32),
        parent: u64,
        name: &[u8],
        kind: Kind,
        mode: u32,
        rdev: u64,
        target: Option<Vec<u8>>,
    ) -> FsResult<Attr> {
        self.rw()?;
        check_name(name).map_err(FsError::Errno)?;
        self.check_space(0)?;
        let a = self.be.make(
            self.layer(),
            &MakeArgs {
                parent,
                name: name.to_vec(),
                kind,
                mode: mode & 0o7777,
                uid: req.0,
                gid: req.1,
                rdev,
                target,
                now: now_ns(),
            },
        )?;
        self.sh.cache.put(&a, 1);
        self.sh.cache.invalidate(parent);
        Ok(a)
    }

    fn check_space(&mut self, extra: u64) -> FsResult<()> {
        let Some(cap) = self.sh.cfg.capacity else {
            return Ok(());
        };
        if self.sh.usage_stale() {
            if let Ok(u) = self.be.statfs() {
                self.sh.set_usage(u);
            }
        }
        if let Some(u) = self.sh.cached_usage() {
            if u.physical_bytes + extra > cap {
                return Err(FsError::Errno(libc::ENOSPC));
            }
        }
        Ok(())
    }

    /// Run a name operation that may ask us to flush dirty inodes first.
    fn with_dirty_retry<T>(
        &mut self,
        mut f: impl FnMut(&mut Self, &[u64], &[u64]) -> FsResult<T>,
    ) -> FsResult<T> {
        for _ in 0..4 {
            let open = self.sh.cache.open();
            let dirty = self.sh.cache.dirty();
            match f(self, &open, &dirty) {
                Err(FsError::RetryDirty(inos)) => self.flush_inos(&inos)?,
                r => return r,
            }
        }
        Err(FsError::Errno(libc::EIO))
    }

    fn unlink(&mut self, parent: u64, name: &[u8]) -> FsResult<()> {
        self.rw()?;
        check_name(name).map_err(FsError::Errno)?;
        let layer = self.layer();
        let name_v = name.to_vec();
        let out = self.with_dirty_retry(|w, open, dirty| {
            w.be.unlink(layer, parent, &name_v, open, dirty, now_ns())
        })?;
        self.sh.cache.set_nlink(out.ino, out.nlink, out.orphaned);
        self.sh.cache.invalidate(parent);
        Ok(())
    }

    fn rmdir(&mut self, parent: u64, name: &[u8]) -> FsResult<()> {
        self.rw()?;
        check_name(name).map_err(FsError::Errno)?;
        let ino = self.be.rmdir(self.layer(), parent, name, now_ns())?;
        self.sh.cache.set_nlink(ino, 0, false);
        self.sh.cache.invalidate(parent);
        Ok(())
    }

    fn rename(
        &mut self,
        parent: u64,
        name: &[u8],
        newparent: u64,
        newname: &[u8],
        flags: u32,
    ) -> FsResult<()> {
        self.rw()?;
        if flags & !(RENAME_NOREPLACE | RENAME_EXCHANGE) != 0 {
            return Err(FsError::Errno(libc::EINVAL));
        }
        check_name(name).map_err(FsError::Errno)?;
        check_name(newname).map_err(FsError::Errno)?;
        let layer = self.layer();
        let base = RenameArgs {
            parent,
            name: name.to_vec(),
            newparent,
            newname: newname.to_vec(),
            flags,
            open: Vec::new(),
            dirty: Vec::new(),
            now: 0,
        };
        let out = self.with_dirty_retry(|w, open, dirty| {
            let mut a = base.clone();
            a.open = open.to_vec();
            a.dirty = dirty.to_vec();
            a.now = now_ns();
            w.be.rename(layer, &a)
        })?;
        self.sh.cache.invalidate(out.ino);
        if let Some((r, nlink, orphan)) = out.replaced {
            self.sh.cache.set_nlink(r, nlink, orphan);
        }
        self.sh.cache.invalidate(parent);
        self.sh.cache.invalidate(newparent);
        Ok(())
    }

    fn link(&mut self, ino: u64, newparent: u64, newname: &[u8]) -> FsResult<Attr> {
        self.rw()?;
        check_name(newname).map_err(FsError::Errno)?;
        self.flush_inos(&[ino])?;
        let gen = self.gen(ino);
        let a = self
            .be
            .link(self.layer(), ino, gen, newparent, newname, now_ns())?;
        self.sh.cache.put(&a, 1);
        self.sh.cache.invalidate(newparent);
        Ok(a)
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &mut self,
        ino: u64,
        caller: u32,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
    ) -> FsResult<Attr> {
        self.rw()?;
        let now = now_ns();
        let t = |x: Option<TimeOrNow>| {
            x.map(|v| match v {
                TimeOrNow::Now => now,
                TimeOrNow::SpecificTime(s) => time_to_ns(s),
            })
        };
        let mut args = SetAttrArgs {
            ino,
            gen: self.gen(ino),
            mode,
            uid,
            gid,
            size,
            tail: None,
            atime_ns: t(atime),
            mtime_ns: t(mtime),
            caller_uid: Some(caller),
            now,
        };
        if let Some(new) = size {
            if new > MAX_FILE_SIZE {
                return Err(FsError::Errno(libc::EFBIG));
            }
            let a = self.attr(ino)?;
            if a.kind == Kind::Dir {
                return Err(FsError::Errno(libc::EISDIR));
            }
            // Trim buffered writes past the new end, commit the rest.
            if let Some(b) = self.buffers.get_mut(&ino) {
                if b.size > new {
                    b.truncate(new);
                }
                // The size change itself is the truncate below, with its
                // zeroed tail; the commit only stores the remaining data.
                if new < b.committed_size {
                    b.size = b.committed_size;
                }
            }
            self.account(ino);
            self.commit(ino, false)?;
            let committed = self
                .buffers
                .get(&ino)
                .map(|b| b.committed_size)
                .unwrap_or(a.size);
            args.tail = self.truncate_tail(ino, committed, new)?;
            if let Some(b) = self.buffers.get_mut(&ino) {
                b.size = new;
                b.committed_size = new;
            }
        }
        let a = self.be.setattr(self.layer(), &args)?;
        self.sh.cache.put(&a, 0);
        if mode.is_some() || uid.is_some() || gid.is_some() {
            self.sh.cache.update(ino, |e| e.no_capability = false);
        }
        self.account(ino);
        Ok(a)
    }

    /// The block that will hold the end of the file after a truncate from
    /// `old` to `new`, read before the call (spec/app.py TruncatesFiles).
    fn truncate_tail(&mut self, ino: u64, old: u64, new: u64) -> FsResult<Option<(u64, Vec<u8>)>> {
        let bs = self.sh.cfg.block_size;
        let inline_max = self.sh.cfg.inline_max;
        let view = self.view();
        let gen = self.gen(ino);
        if old <= inline_max {
            let r = self.be.read_blocks(&view, ino, gen, 0, 0)?;
            if r.inline {
                if new <= inline_max {
                    return Ok(None);
                }
                let content = r.blocks.first().map(|(_, d)| d.clone()).unwrap_or_default();
                return Ok(Some((0, content)));
            }
        }
        if new >= old || new.is_multiple_of(bs) {
            return Ok(None);
        }
        let bno = new / bs;
        let r = self.be.read_blocks(&view, ino, gen, bno, bno)?;
        Ok(r.blocks
            .into_iter()
            .find(|(b, _)| *b == bno)
            .map(|(b, mut d)| {
                d.truncate((new % bs) as usize);
                (b, d)
            }))
    }

    fn open(&mut self, ino: u64, flags: i32) -> FsResult<(u64, u32)> {
        let acc = flags & libc::O_ACCMODE;
        let writable = acc == libc::O_WRONLY || acc == libc::O_RDWR;
        if writable {
            self.rw()?;
        }
        let a = if self.sh.cfg.read_only && !self.sh.cfg.snapshot {
            // A read-only live mount reads the inode fresh at open.
            self.sh.cache.invalidate(ino);
            self.attr(ino)?
        } else {
            self.attr(ino)?
        };
        if a.kind == Kind::Dir && writable {
            return Err(FsError::Errno(libc::EISDIR));
        }
        let fh = self.sh.alloc_fh();
        let err_seq = self.buffers.get(&ino).map(|b| b.err_seq).unwrap_or(0);
        self.handles.insert(
            fh,
            Handle {
                ino,
                readable: acc != libc::O_WRONLY,
                writable,
                append: flags & libc::O_APPEND != 0,
                sync: flags & (libc::O_SYNC | libc::O_DSYNC) != 0,
                direct: flags & libc::O_DIRECT != 0,
                last_write_seq: 0,
                seen_err_seq: err_seq,
            },
        );
        self.sh.cache.open_handle(ino, 1);
        self.sh.handles_open.fetch_add(1, Ordering::Relaxed);
        let keep = if self.sh.cfg.read_only && !self.sh.cfg.snapshot {
            let prev = self.sh.cache.get(ino).and_then(|e| e.opened_version);
            self.sh
                .cache
                .update(ino, |e| e.opened_version = Some(a.version));
            prev == Some(a.version)
        } else {
            true
        };
        let mut open_flags = if keep {
            fuser::consts::FOPEN_KEEP_CACHE
        } else {
            0
        };
        if flags & libc::O_DIRECT != 0 {
            open_flags |= fuser::consts::FOPEN_DIRECT_IO;
        }
        Ok((fh, open_flags))
    }

    fn read(&mut self, ino: u64, fh: u64, offset: u64, size: u32) -> FsResult<Vec<u8>> {
        if let Some(h) = self.handles.get(&fh) {
            if !h.readable {
                return Err(FsError::Errno(libc::EBADF));
            }
        }
        let a = self.attr(ino)?;
        if a.kind == Kind::Dir {
            return Err(FsError::Errno(libc::EISDIR));
        }
        let file_size = self.buffers.get(&ino).map(|b| b.size).unwrap_or(a.size);
        if offset >= file_size || size == 0 {
            return Ok(Vec::new());
        }
        let end = (offset + size as u64).min(file_size);
        let committed_size = self
            .buffers
            .get(&ino)
            .map(|b| b.committed_size)
            .unwrap_or(a.size);
        let mut out = vec![0u8; (end - offset) as usize];
        let bs = self.sh.cfg.block_size;
        let cend = end.min(committed_size);
        if offset < cend {
            let view = self.view();
            let r = self
                .be
                .read_blocks(&view, ino, Some(a.gen), offset / bs, (cend - 1) / bs)?;
            for (bno, data) in r.blocks {
                let bstart = bno * bs;
                let s = bstart.max(offset);
                let e = (bstart + data.len() as u64).min(cend);
                if s < e {
                    out[(s - offset) as usize..(e - offset) as usize]
                        .copy_from_slice(&data[(s - bstart) as usize..(e - bstart) as usize]);
                }
            }
        }
        if let Some(b) = self.buffers.get(&ino) {
            b.overlay_read(offset, &mut out);
        }
        Ok(out)
    }

    fn write(&mut self, ino: u64, fh: u64, offset: u64, data: &[u8], uid: u32) -> FsResult<u32> {
        self.rw()?;
        let h = self
            .handles
            .get(&fh)
            .cloned()
            .ok_or(FsError::Errno(libc::EBADF))?;
        if !h.writable {
            return Err(FsError::Errno(libc::EBADF));
        }
        self.check_space(data.len() as u64)?;
        let a = self.attr(ino)?;
        if a.kind != Kind::File {
            return Err(FsError::Errno(libc::EINVAL));
        }
        let buf = self.buffer(ino)?;
        let pos = if h.append { buf.size } else { offset };
        if pos
            .checked_add(data.len() as u64)
            .is_none_or(|e| e > MAX_FILE_SIZE)
        {
            return Err(FsError::Errno(libc::EFBIG));
        }
        let seq = buf.write(pos, data, uid, now_ns());
        if let Some(hh) = self.handles.get_mut(&fh) {
            hh.last_write_seq = seq;
        }
        self.account(ino);
        if h.sync || h.direct {
            // Commit through this write's write_seq (spec/durability.py SyncHandles).
            self.commit(ino, h.sync)?;
        } else {
            self.relieve_pressure(ino)?;
        }
        Ok(data.len() as u32)
    }

    /// Report a commit error once per descriptor (errseq_t).
    fn check_errseq(&mut self, fh: u64, ino: u64) -> FsResult<()> {
        let err = self.buffers.get(&ino).map(|b| b.err_seq).unwrap_or(0);
        if let Some(h) = self.handles.get_mut(&fh) {
            if err > h.seen_err_seq {
                h.seen_err_seq = err;
                return Err(FsError::Errno(libc::EIO));
            }
        }
        Ok(())
    }

    fn flush(&mut self, ino: u64, fh: u64) -> FsResult<()> {
        let need = self.handles.get(&fh).map(|h| h.last_write_seq).unwrap_or(0);
        let done = self.buffers.get(&ino).map(|b| b.committed_seq).unwrap_or(0);
        if need > done {
            let _ = self.commit(ino, false);
        }
        self.check_errseq(fh, ino)
    }

    fn release(&mut self, ino: u64, fh: u64) -> FsResult<()> {
        let r = self.flush(ino, fh);
        self.handles.remove(&fh);
        self.sh.handles_open.fetch_sub(1, Ordering::Relaxed);
        let left = self.sh.cache.open_handle(ino, -1);
        if left == 0 && !self.sh.cfg.read_only {
            let orphan = self.sh.cache.get(ino).is_some_and(|e| e.orphan);
            if orphan {
                if self.buffers.get(&ino).is_some_and(|b| b.is_dirty()) {
                    tracing::debug!("dropping buffered writes of unlinked inode {ino}");
                }
                self.buffers.remove(&ino);
                self.account(ino);
                self.be.release_orphan(self.layer(), ino)?;
                self.sh.cache.update(ino, |e| e.orphan = false);
            }
        }
        r
    }

    fn fsync(&mut self, ino: u64, fh: Option<u64>) -> FsResult<()> {
        if self.sh.cfg.read_only {
            return Ok(());
        }
        let needs_barrier = self.buffers.get(&ino).is_some_and(|b| b.needs_barrier);
        let r = self.commit(ino, true);
        let r = match r {
            Ok(()) if needs_barrier => self.be.sync_barrier(),
            r => r,
        };
        if let Some(b) = self.buffers.get_mut(&ino) {
            b.needs_barrier = r.is_err();
        }
        if let Err(e) = &r {
            tracing::error!("fsync of inode {ino} failed: {e}");
            if let Some(b) = self.buffers.get_mut(&ino) {
                if !b.is_dirty() {
                    b.failed();
                }
            }
        }
        match fh {
            Some(fh) => {
                let seen = self.check_errseq(fh, ino);
                r.and(seen)
            }
            None => r,
        }
    }

    fn readdir_page(
        &mut self,
        ino: u64,
        fh: u64,
        offset: i64,
    ) -> FsResult<(Vec<(i64, DirEntry)>, bool)> {
        let a = self.attr(ino)?;
        if a.kind != Kind::Dir {
            return Err(FsError::Errno(libc::ENOTDIR));
        }
        let dh = self.dirs.entry(fh).or_insert_with(|| DirHandle {
            next: 2,
            ..Default::default()
        });
        let after = if offset <= 2 {
            None
        } else {
            dh.names.get(&offset).cloned()
        };
        if offset > 2 && after.is_none() {
            return Err(FsError::Errno(libc::EINVAL));
        }
        let view = self.view();
        let list = self
            .be
            .readdir(&view, ino, after.as_deref(), READDIR_PAGE)?;
        let more = list.len() == READDIR_PAGE as usize;
        let dh = self.dirs.get_mut(&fh).expect("inserted");
        let out = list
            .into_iter()
            .map(|e| {
                dh.next += 1;
                dh.names.insert(dh.next, e.name.clone());
                (dh.next, e)
            })
            .collect();
        Ok((out, more))
    }

    fn statfs(&mut self) -> FsResult<Usage> {
        if self.sh.usage_stale() {
            let u = self.be.statfs()?;
            self.sh.set_usage(u);
            return Ok(u);
        }
        Ok(self.sh.cached_usage().unwrap_or_default())
    }

    fn check_xattr_name(name: &[u8]) -> FsResult<()> {
        if name.starts_with(b"system.posix_acl_") {
            return Err(FsError::Errno(libc::EOPNOTSUPP));
        }
        if !(name.starts_with(b"user.")
            || name.starts_with(b"trusted.")
            || name.starts_with(b"security."))
        {
            return Err(FsError::Errno(libc::EOPNOTSUPP));
        }
        if name.len() > 255 {
            return Err(FsError::Errno(libc::ERANGE));
        }
        Ok(())
    }

    fn getxattr(&mut self, ino: u64, name: &[u8]) -> FsResult<Vec<u8>> {
        Self::check_xattr_name(name)?;
        if name == b"security.capability" && self.sh.cache.get(ino).is_some_and(|e| e.no_capability)
        {
            return Err(FsError::Errno(libc::ENODATA));
        }
        let view = self.view();
        match self.be.getxattr(&view, ino, name)? {
            Some(v) => Ok(v),
            None => {
                if name == b"security.capability" {
                    self.sh.cache.update(ino, |e| e.no_capability = true);
                }
                Err(FsError::Errno(libc::ENODATA))
            }
        }
    }

    fn setxattr(&mut self, ino: u64, name: &[u8], value: &[u8], flags: i32) -> FsResult<()> {
        self.rw()?;
        Self::check_xattr_name(name)?;
        if value.len() > 65536 {
            return Err(FsError::Errno(libc::E2BIG));
        }
        self.be
            .setxattr(self.layer(), ino, name, value, flags, now_ns())?;
        self.sh.cache.update(ino, |e| {
            e.no_capability = false;
            e.fetched = None;
        });
        Ok(())
    }

    fn removexattr(&mut self, ino: u64, name: &[u8]) -> FsResult<()> {
        self.rw()?;
        Self::check_xattr_name(name)?;
        self.be.removexattr(self.layer(), ino, name, now_ns())?;
        self.sh.cache.invalidate(ino);
        Ok(())
    }

    /// fallocate (spec/offload.py FallocatePunchHoleReq).
    fn fallocate(&mut self, ino: u64, offset: u64, length: u64, mode: i32) -> FsResult<()> {
        self.rw()?;
        const KEEP_SIZE: i32 = libc::FALLOC_FL_KEEP_SIZE;
        const PUNCH: i32 = libc::FALLOC_FL_PUNCH_HOLE;
        const ZERO: i32 = libc::FALLOC_FL_ZERO_RANGE;
        let end = offset
            .checked_add(length)
            .filter(|&e| e <= MAX_FILE_SIZE)
            .ok_or(FsError::Errno(libc::EFBIG))?;
        if length == 0 {
            return Err(FsError::Errno(libc::EINVAL));
        }
        let zeroing = match mode {
            0 => false,
            KEEP_SIZE => return Ok(()),
            m if m == PUNCH | KEEP_SIZE => true,
            m if m == ZERO || m == ZERO | KEEP_SIZE => true,
            _ => return Err(FsError::Errno(libc::EOPNOTSUPP)),
        };
        self.commit(ino, false)?;
        let a = self.attr(ino)?;
        if a.kind != Kind::File {
            return Err(FsError::Errno(if a.kind == Kind::Dir {
                libc::EISDIR
            } else {
                libc::ENODEV
            }));
        }
        let keep = mode & KEEP_SIZE != 0;
        let new_size = if keep { a.size } else { a.size.max(end) };
        if !zeroing {
            if new_size > a.size {
                self.setattr(ino, 0, None, None, None, Some(new_size), None, None)?;
            }
            return Ok(());
        }
        let bs = self.sh.cfg.block_size;
        let view = self.view();
        let gen = Some(a.gen);
        let zend = end.min(a.size.max(new_size));
        let mut args = WriteArgs {
            ino,
            gen,
            size: new_size,
            now: now_ns(),
            ..Default::default()
        };
        let r0 = self.be.read_blocks(&view, ino, gen, 0, 0)?;
        if r0.inline {
            let mut content = r0
                .blocks
                .first()
                .map(|(_, d)| d.clone())
                .unwrap_or_default();
            if new_size <= self.sh.cfg.inline_max {
                content.resize(new_size as usize, 0);
                let s = (offset as usize).min(content.len());
                let e = (zend as usize).min(content.len());
                content[s..e].fill(0);
                args.inline = Some(content);
                let at = self.be.write_blocks(self.layer(), &args)?;
                self.sh.cache.put(&at, 0);
                self.reset_buffer_size(ino, at.size);
                return Ok(());
            }
            let s = (offset as usize).min(content.len());
            let e = (zend as usize).min(content.len());
            content[s..e].fill(0);
            content.truncate((new_size.min(bs)) as usize);
            args.blocks.push((0, content));
        } else if offset < zend {
            let first_full = offset.div_ceil(bs);
            let last_full = zend / bs; // exclusive
            if first_full < last_full {
                args.hole = Some((first_full, last_full - 1));
            }
            let mut edges = vec![offset / bs];
            if zend % bs != 0 {
                edges.push(zend / bs);
            }
            edges.dedup();
            for bno in edges {
                let bstart = bno * bs;
                if offset <= bstart && bstart + bs <= zend {
                    continue;
                }
                let r = self.be.read_blocks(&view, ino, gen, bno, bno)?;
                if let Some((_, mut d)) = r.blocks.into_iter().find(|(b, _)| *b == bno) {
                    let s = offset.saturating_sub(bstart).min(d.len() as u64) as usize;
                    let e = (zend - bstart).min(d.len() as u64) as usize;
                    if s < e {
                        d[s..e].fill(0);
                        args.blocks.push((bno, d));
                    }
                }
            }
        }
        let at = self.be.write_blocks(self.layer(), &args)?;
        self.sh.cache.put(&at, 0);
        self.reset_buffer_size(ino, at.size);
        Ok(())
    }

    fn reset_buffer_size(&mut self, ino: u64, size: u64) {
        if let Some(b) = self.buffers.get_mut(&ino) {
            if !b.is_dirty() {
                b.size = size;
                b.committed_size = size;
            }
        }
    }

    /// SEEK_DATA / SEEK_HOLE at block granularity (spec/posix.py SparseFiles).
    fn lseek(&mut self, ino: u64, offset: i64, whence: i32) -> FsResult<i64> {
        if whence != libc::SEEK_DATA && whence != libc::SEEK_HOLE {
            return Err(FsError::Errno(libc::EINVAL));
        }
        if !self.sh.cfg.read_only {
            self.commit(ino, false)?;
        }
        let a = self.attr(ino)?;
        let off = offset.max(0) as u64;
        if off >= a.size {
            return Err(FsError::Errno(libc::ENXIO));
        }
        let bs = self.sh.cfg.block_size;
        let view = self.view();
        let data = whence == libc::SEEK_DATA;
        match self.be.seek_block(&view, ino, off / bs, data)? {
            Some(b) => {
                let pos = (b * bs).max(off);
                if data {
                    Ok(pos as i64)
                } else {
                    Ok(pos.min(a.size) as i64)
                }
            }
            None if data => Err(FsError::Errno(libc::ENXIO)),
            None => Ok(a.size as i64),
        }
    }

    /// copy_file_range (spec/offload.py CopyFileRangeReq): whole blocks at
    /// the same in-block offset are shared; the rest is read and written
    /// here as new blobs. At most one batch per call.
    fn copy_range(
        &mut self,
        ino_in: u64,
        off_in: u64,
        ino_out: u64,
        off_out: u64,
        len: u64,
    ) -> FsResult<u32> {
        self.rw()?;
        self.flush_inos(&[ino_in, ino_out])?;
        let ai = self.attr(ino_in)?;
        let ao = self.attr(ino_out)?;
        if ai.kind != Kind::File || ao.kind != Kind::File {
            return Err(FsError::Errno(libc::EISDIR));
        }
        if off_in >= ai.size || len == 0 {
            return Ok(0);
        }
        let bs = self.sh.cfg.block_size;
        const BATCH_BLOCKS: u64 = 4096;
        let len = len
            .min(ai.size - off_in)
            .min(BATCH_BLOCKS * bs)
            .min(u32::MAX as u64 / 2);
        if ino_in == ino_out && off_in < off_out + len && off_out < off_in + len {
            return Err(FsError::Errno(libc::EINVAL));
        }
        let end_out = off_out
            .checked_add(len)
            .filter(|&e| e <= MAX_FILE_SIZE)
            .ok_or(FsError::Errno(libc::EFBIG))?;
        self.check_space(len)?;
        let view = self.view();
        let aligned = off_in % bs == off_out % bs;
        let (mut first_full, mut last_full) = (off_out.div_ceil(bs), end_out / bs);
        if !aligned || first_full >= last_full {
            first_full = 0;
            last_full = 0;
        }
        let inline_src = self
            .be
            .read_blocks(&view, ino_in, Some(ai.gen), 0, 0)?
            .inline;
        if inline_src {
            first_full = 0;
            last_full = 0;
        }
        let shift = |bo: u64| (bo * bs - off_out + off_in) / bs;
        let mut blocks = Vec::new();
        // Destination blocks the range touches but does not share.
        let first_b = off_out / bs;
        let last_b = (end_out - 1) / bs;
        let out_inline = self.be.read_blocks(&view, ino_out, Some(ao.gen), 0, 0)?;
        for bno in first_b..=last_b {
            if bno >= first_full && bno < last_full {
                continue;
            }
            let bstart = bno * bs;
            let s = bstart.max(off_out);
            let e = (bstart + bs).min(end_out);
            let mut content = if out_inline.inline && bno == 0 {
                out_inline
                    .blocks
                    .first()
                    .map(|(_, d)| d.clone())
                    .unwrap_or_default()
            } else {
                self.be
                    .read_blocks(&view, ino_out, Some(ao.gen), bno, bno)?
                    .blocks
                    .into_iter()
                    .find(|(b, _)| *b == bno)
                    .map(|(_, d)| d)
                    .unwrap_or_default()
            };
            let need = (e - bstart) as usize;
            let keep = (ao.size.saturating_sub(bstart)).min(bs) as usize;
            content.resize(keep.max(need), 0);
            let src = self.read(ino_in, u64::MAX, s - off_out + off_in, (e - s) as u32)?;
            let mut src = src;
            src.resize((e - s) as usize, 0);
            content[(s - bstart) as usize..(e - bstart) as usize].copy_from_slice(&src);
            blocks.push((bno, content));
        }
        // An inline destination whose first block is shared still needs
        // its bytes moved out of the inode.
        if out_inline.inline
            && first_full == 0
            && last_full > 0
            && !blocks.iter().any(|(b, _)| *b == 0)
        {
            first_full = first_full.max(1);
            let mut b0 = out_inline
                .blocks
                .first()
                .map(|(_, d)| d.clone())
                .unwrap_or_default();
            let src = self.read(ino_in, u64::MAX, off_in - off_out, bs as u32)?;
            b0.resize(bs as usize, 0);
            b0[..src.len()].copy_from_slice(&src);
            blocks.push((0, b0));
        }
        let args = CopyArgs {
            ino_in,
            gen_in: Some(ai.gen),
            ino_out,
            gen_out: Some(ao.gen),
            src_first: if last_full > first_full {
                shift(first_full)
            } else {
                0
            },
            dst_first: first_full,
            count: last_full.saturating_sub(first_full),
            blocks,
            size: end_out.max(ao.size),
            now: now_ns(),
        };
        let a = self.be.copy_range(self.layer(), &args)?;
        self.sh.cache.put(&a, 0);
        self.reset_buffer_size(ino_out, a.size);
        Ok(len as u32)
    }
}

/// The FUSE filesystem: routes requests to workers.
pub struct PgFs {
    pub sh: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Build the shared state and start one worker per backend.
pub fn start(
    cfg: FsConfig,
    view: View,
    tree: String,
    health: Arc<Health>,
    backends: Vec<Box<dyn PgfsBackend>>,
    cancel_slots: Vec<crate::pg::CancelSlot>,
) -> PgFs {
    let n = backends.len();
    let mut job_tx = Vec::new();
    let mut job_rx = Vec::new();
    let mut peer_tx = Vec::new();
    let mut peer_rx = Vec::new();
    for _ in 0..n {
        let (a, b) = unbounded();
        job_tx.push(a);
        job_rx.push(b);
        let (c, d) = unbounded();
        peer_tx.push(c);
        peer_rx.push(d);
    }
    let sh = Arc::new(Shared {
        cfg: FsConfig { workers: n, ..cfg },
        view,
        tree,
        cache: Cache::new(),
        health,
        jobs: job_tx,
        peers: peer_tx,
        buffered_total: AtomicUsize::new(0),
        inflight: (0..n).map(|_| Inflight::default()).collect(),
        notifier: Mutex::new(None),
        next_fh: AtomicU64::new(1),
        usage: Mutex::new(None),
        handles_open: AtomicUsize::new(0),
        cancel_slots,
        backend_pids: (0..n).map(|_| AtomicU64::new(0)).collect(),
    });
    let mut threads = Vec::new();
    for (id, ((be, jobs), peer_rx)) in backends.into_iter().zip(job_rx).zip(peer_rx).enumerate() {
        let w = Worker {
            id,
            sh: sh.clone(),
            be,
            buffers: HashMap::new(),
            buffer_bytes: HashMap::new(),
            handles: HashMap::new(),
            dirs: HashMap::new(),
            jobs,
            peer_rx,
            deferred: VecDeque::new(),
            held: false,
        };
        threads.push(
            std::thread::Builder::new()
                .name(format!("worker-{id}"))
                .spawn(move || w.run())
                .expect("spawn worker thread"),
        );
    }
    PgFs { sh, threads }
}

impl PgFs {
    /// Stop the workers (after the session ended).
    pub fn stop(self) {
        for p in &self.sh.peers {
            let _ = p.send(Peer::Stop);
        }
        for t in self.threads {
            let _ = t.join();
        }
    }

    pub fn shared(&self) -> Arc<Shared> {
        self.sh.clone()
    }
}

/// Reply an error, logging and counting it (spec/app.py
/// DistinguishesExpectedErrors: expected errnos at DEBUG at most).
macro_rules! fail {
    ($reply:expr, $e:expr) => {{
        let e: FsError = $e;
        let errno = e.errno();
        count_errno(errno);
        if e.is_expected() {
            tracing::debug!("reply {}", errno_name(errno));
        } else {
            crate::log_and_reply!(e);
        }
        $reply.error(errno);
    }};
}

fn bytes(s: &OsStr) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// The `fuser::Filesystem` the session drives: it only routes.
pub struct Dispatcher {
    sh: Arc<Shared>,
}

impl Dispatcher {
    pub fn new(sh: Arc<Shared>) -> Self {
        Dispatcher { sh }
    }
}

impl Filesystem for Dispatcher {
    fn init(&mut self, _req: &Request<'_>, config: &mut KernelConfig) -> Result<(), libc::c_int> {
        let _ = config.add_capabilities(
            fuser::consts::FUSE_DO_READDIRPLUS
                | fuser::consts::FUSE_READDIRPLUS_AUTO
                | fuser::consts::FUSE_PARALLEL_DIROPS,
        );
        let _ = config.set_max_write(1 << 20);
        tracing::info!("FUSE session initialised; writeback cache not negotiated (spec/posix.py WritebackCacheDisabled)");
        Ok(())
    }

    fn destroy(&mut self) {
        tracing::info!("FUSE session destroyed");
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let p = self.sh.db(parent);
        let name = bytes(name);
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            false,
            "lookup",
            p,
            Box::new(move |wk| match wk.lookup(p, &name) {
                Ok(Some(a)) => reply.entry(&wk.sh.cfg.ttl, &wk.entry_attr(&a), a.gen),
                Ok(None) => {
                    count_errno(libc::ENOENT);
                    // A negative entry, cached for the same TTL (spec/consistency.py KernelCacheContract).
                    let mut fa = wk.sh.fattr(&Attr {
                        ino: 0,
                        gen: 0,
                        kind: Kind::File,
                        mode: 0,
                        uid: 0,
                        gid: 0,
                        nlink: 0,
                        size: 0,
                        blocks: 0,
                        atime_ns: 0,
                        mtime_ns: 0,
                        ctime_ns: 0,
                        btime_ns: 0,
                        rdev: 0,
                        version: 0,
                    });
                    fa.ino = 0;
                    reply.entry(&wk.sh.cfg.ttl, &fa, 0)
                }
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn forget(&mut self, _req: &Request<'_>, ino: u64, nlookup: u64) {
        count_op("forget");
        let d = self.sh.db(ino);
        if self.sh.cache.forget(d, nlookup) {
            let _ = self.sh.peers[self.sh.owner(d)].send(Peer::Evict(d));
        }
    }

    fn batch_forget(&mut self, req: &Request<'_>, nodes: &[fuser::fuse_forget_one]) {
        for n in nodes {
            self.forget(req, n.nodeid, n.nlookup);
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "getattr",
            d,
            Box::new(move |wk| match wk.attr(d) {
                Ok(a) => reply.attr(&wk.sh.cfg.ttl, &wk.entry_attr(&a)),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn setattr(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let d = self.sh.db(ino);
        let caller = req.uid();
        self.sh.submit(
            self.sh.owner(d),
            true,
            "setattr",
            d,
            Box::new(
                move |wk| match wk.setattr(d, caller, mode, uid, gid, size, atime, mtime) {
                    Ok(a) => reply.attr(&wk.sh.cfg.ttl, &wk.entry_attr(&a)),
                    Err(e) => fail!(reply, e),
                },
            ),
        );
    }

    fn readlink(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyData) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "readlink",
            d,
            Box::new(move |wk| {
                let view = wk.view();
                match wk.be.readlink(&view, d) {
                    Ok(t) => reply.data(&t),
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }

    fn mknod(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        rdev: u32,
        reply: ReplyEntry,
    ) {
        let p = self.sh.db(parent);
        let name = bytes(name);
        let who = (req.uid(), req.gid());
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            true,
            "mknod",
            p,
            Box::new(move |wk| {
                let Some(kind) = Kind::from_mode(mode) else {
                    return fail!(reply, FsError::Errno(libc::EINVAL));
                };
                if kind == Kind::Dir || kind == Kind::Symlink {
                    return fail!(reply, FsError::Errno(libc::EINVAL));
                }
                match wk.make(who, p, &name, kind, mode, rdev as u64, None) {
                    Ok(a) => reply.entry(&wk.sh.cfg.ttl, &wk.entry_attr(&a), a.gen),
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }

    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let p = self.sh.db(parent);
        let name = bytes(name);
        let who = (req.uid(), req.gid());
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            true,
            "mkdir",
            p,
            Box::new(
                move |wk| match wk.make(who, p, &name, Kind::Dir, mode, 0, None) {
                    Ok(a) => reply.entry(&wk.sh.cfg.ttl, &wk.entry_attr(&a), a.gen),
                    Err(e) => fail!(reply, e),
                },
            ),
        );
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let p = self.sh.db(parent);
        let name = bytes(name);
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            true,
            "unlink",
            p,
            Box::new(move |wk| match wk.unlink(p, &name) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let p = self.sh.db(parent);
        let name = bytes(name);
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            true,
            "rmdir",
            p,
            Box::new(move |wk| match wk.rmdir(p, &name) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn symlink(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        link_name: &OsStr,
        target: &std::path::Path,
        reply: ReplyEntry,
    ) {
        let p = self.sh.db(parent);
        let name = bytes(link_name);
        let target = target.as_os_str().as_bytes().to_vec();
        let who = (req.uid(), req.gid());
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            true,
            "symlink",
            p,
            Box::new(move |wk| {
                match wk.make(who, p, &name, Kind::Symlink, 0o777, 0, Some(target)) {
                    Ok(a) => reply.entry(&wk.sh.cfg.ttl, &wk.entry_attr(&a), a.gen),
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        let (p, np) = (self.sh.db(parent), self.sh.db(newparent));
        let (name, newname) = (bytes(name), bytes(newname));
        let (a, b) = (
            self.sh.name_owner(p, &name),
            self.sh.name_owner(np, &newname),
        );
        let w = if (p, &name) <= (np, &newname) { a } else { b };
        self.sh.submit(
            w,
            true,
            "rename",
            p,
            Box::new(move |wk| match wk.rename(p, &name, np, &newname, flags) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn link(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEntry,
    ) {
        let (d, np) = (self.sh.db(ino), self.sh.db(newparent));
        let newname = bytes(newname);
        let w = self.sh.name_owner(np, &newname);
        self.sh.submit(
            w,
            true,
            "link",
            d,
            Box::new(move |wk| match wk.link(d, np, &newname) {
                Ok(a) => reply.entry(&wk.sh.cfg.ttl, &wk.entry_attr(&a), a.gen),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "open",
            d,
            Box::new(move |wk| match wk.open(d, flags) {
                Ok((fh, f)) => reply.opened(fh, f),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "read",
            d,
            Box::new(move |wk| match wk.read(d, fh, offset.max(0) as u64, size) {
                Ok(data) => reply.data(&data),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn write(
        &mut self,
        req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _wf: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyWrite,
    ) {
        let d = self.sh.db(ino);
        let data = data.to_vec();
        let uid = req.uid();
        self.sh.submit(
            self.sh.owner(d),
            true,
            "write",
            d,
            Box::new(
                move |wk| match wk.write(d, fh, offset.max(0) as u64, &data, uid) {
                    Ok(n) => reply.written(n),
                    Err(e) => fail!(reply, e),
                },
            ),
        );
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            true,
            "flush",
            d,
            Box::new(move |wk| match wk.flush(d, fh) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _flags: i32,
        _lock: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            true,
            "release",
            d,
            Box::new(move |wk| match wk.release(d, fh) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn fsync(&mut self, _req: &Request<'_>, ino: u64, fh: u64, _datasync: bool, reply: ReplyEmpty) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            true,
            "fsync",
            d,
            Box::new(move |wk| match wk.fsync(d, Some(fh)) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn opendir(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(self.sh.alloc_fh(), 0);
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "readdir",
            d,
            Box::new(move |wk| {
                let k = wk.sh.kino(d);
                if offset < 1 && reply.add(k, 1, fuser::FileType::Directory, ".") {
                    return reply.ok();
                }
                if offset < 2 && reply.add(k, 2, fuser::FileType::Directory, "..") {
                    return reply.ok();
                }
                let mut off = offset;
                loop {
                    match wk.readdir_page(d, fh, off) {
                        Ok((entries, more)) => {
                            for (o, e) in &entries {
                                if reply.add(
                                    wk.sh.kino(e.ino),
                                    *o,
                                    e.kind.fuse(),
                                    OsStr::from_bytes(&e.name),
                                ) {
                                    return reply.ok();
                                }
                            }
                            match entries.last() {
                                Some((o, _)) if more => off = *o,
                                _ => return reply.ok(),
                            }
                        }
                        Err(e) => return fail!(reply, e),
                    }
                }
            }),
        );
    }

    fn readdirplus(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectoryPlus,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "readdirplus",
            d,
            Box::new(move |wk| {
                let ttl = wk.sh.cfg.ttl;
                let dir = match wk.attr(d) {
                    Ok(a) => a,
                    Err(e) => return fail!(reply, e),
                };
                if dir.kind != Kind::Dir {
                    return fail!(reply, FsError::Errno(libc::ENOTDIR));
                }
                let fa = wk.entry_attr(&dir);
                if offset < 1 && reply.add(fa.ino, 1, ".", &ttl, &fa, 0) {
                    return reply.ok();
                }
                if offset < 2 && reply.add(fa.ino, 2, "..", &ttl, &fa, 0) {
                    return reply.ok();
                }
                let dh = wk.dirs.entry(fh).or_insert_with(|| DirHandle {
                    next: 2,
                    ..Default::default()
                });
                let mut after = if offset <= 2 {
                    None
                } else {
                    dh.names.get(&offset).cloned()
                };
                if offset > 2 && after.is_none() {
                    return fail!(reply, FsError::Errno(libc::EINVAL));
                }
                loop {
                    let view = wk.view();
                    let list = match wk.be.readdirplus(&view, d, after.as_deref(), READDIR_PAGE) {
                        Ok(l) => l,
                        Err(e) => return fail!(reply, e),
                    };
                    let more = list.len() == READDIR_PAGE as usize;
                    for (name, a) in list {
                        let dh = wk.dirs.get_mut(&fh).expect("inserted");
                        let o = dh.next + 1;
                        let fa = wk.sh.fattr(&a);
                        if reply.add(fa.ino, o, OsStr::from_bytes(&name), &ttl, &fa, a.gen) {
                            return reply.ok();
                        }
                        let dh = wk.dirs.get_mut(&fh).expect("inserted");
                        dh.next = o;
                        dh.names.insert(o, name.clone());
                        // The kernel counts a lookup for every entry it accepts.
                        wk.sh.cache.put(&a, 1);
                        after = Some(name);
                    }
                    if !more {
                        return reply.ok();
                    }
                }
            }),
        );
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "readdir",
            d,
            Box::new(move |wk| {
                wk.dirs.remove(&fh);
                reply.ok();
            }),
        );
    }

    fn fsyncdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            true,
            "fsyncdir",
            d,
            Box::new(move |wk| match wk.fsync(d, None) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        let root = self.sh.view.root;
        self.sh.submit(
            self.sh.owner(root),
            false,
            "statfs",
            root,
            Box::new(move |wk| match wk.statfs() {
                Ok(u) => {
                    let s = statfs_figures(&wk.sh.cfg, u);
                    reply.statfs(
                        s.blocks, s.bfree, s.bfree, s.files, s.ffree, s.bsize, 255, s.bsize,
                    );
                }
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn setxattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        name: &OsStr,
        value: &[u8],
        flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        let d = self.sh.db(ino);
        let (name, value) = (bytes(name), value.to_vec());
        self.sh.submit(
            self.sh.owner(d),
            true,
            "setxattr",
            d,
            Box::new(move |wk| match wk.setxattr(d, &name, &value, flags) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        name: &OsStr,
        size: u32,
        reply: ReplyXattr,
    ) {
        let d = self.sh.db(ino);
        let name = bytes(name);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "getxattr",
            d,
            Box::new(move |wk| match wk.getxattr(d, &name) {
                Ok(v) if size == 0 => reply.size(v.len() as u32),
                Ok(v) if v.len() > size as usize => fail!(reply, FsError::Errno(libc::ERANGE)),
                Ok(v) => reply.data(&v),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn listxattr(&mut self, _req: &Request<'_>, ino: u64, size: u32, reply: ReplyXattr) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "listxattr",
            d,
            Box::new(move |wk| {
                let view = wk.view();
                match wk.be.listxattr(&view, d) {
                    Ok(names) => {
                        let mut out = Vec::new();
                        for n in names {
                            out.extend_from_slice(&n);
                            out.push(0);
                        }
                        if size == 0 {
                            reply.size(out.len() as u32)
                        } else if out.len() > size as usize {
                            fail!(reply, FsError::Errno(libc::ERANGE))
                        } else {
                            reply.data(&out)
                        }
                    }
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }

    fn removexattr(&mut self, _req: &Request<'_>, ino: u64, name: &OsStr, reply: ReplyEmpty) {
        let d = self.sh.db(ino);
        let name = bytes(name);
        self.sh.submit(
            self.sh.owner(d),
            true,
            "removexattr",
            d,
            Box::new(move |wk| match wk.removexattr(d, &name) {
                Ok(()) => reply.ok(),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let p = self.sh.db(parent);
        let name = bytes(name);
        let who = (req.uid(), req.gid());
        let w = self.sh.name_owner(p, &name);
        self.sh.submit(
            w,
            true,
            "create",
            p,
            Box::new(move |wk| {
                let made = match wk.make(who, p, &name, Kind::File, mode, 0, None) {
                    Ok(a) => Ok(a),
                    // Without O_EXCL an existing file is opened (the kernel's
                    // lookup raced with another create).
                    Err(FsError::Errno(libc::EEXIST)) if flags & libc::O_EXCL == 0 => {
                        match wk.lookup(p, &name) {
                            Ok(Some(a)) => Ok(a),
                            Ok(None) => Err(FsError::Errno(libc::ENOENT)),
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                };
                let a = match made {
                    Ok(a) => a,
                    Err(e) => return fail!(reply, e),
                };
                // The handle belongs to the inode's worker; register it there.
                let ino = a.ino;
                let owner = wk.sh.owner(ino);
                let open = if owner == wk.id {
                    wk.open(ino, flags & !libc::O_TRUNC)
                } else {
                    wk.ask(owner, |tx| Peer::Open(ino, flags & !libc::O_TRUNC, tx))
                        .unwrap_or(Err(FsError::Errno(libc::EIO)))
                };
                match open {
                    Ok((fh, f)) => reply.created(&wk.sh.cfg.ttl, &wk.entry_attr(&a), a.gen, fh, f),
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }

    fn ioctl(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: u32,
        _cmd: u32,
        _in: &[u8],
        _out: u32,
        reply: fuser::ReplyIoctl,
    ) {
        count_errno(libc::ENOTTY);
        reply.error(libc::ENOTTY);
    }

    fn bmap(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _bs: u32,
        _idx: u64,
        reply: fuser::ReplyBmap,
    ) {
        count_errno(libc::ENOSYS);
        reply.error(libc::ENOSYS);
    }

    fn fallocate(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        length: i64,
        mode: i32,
        reply: ReplyEmpty,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            true,
            "fallocate",
            d,
            Box::new(move |wk| {
                if offset < 0 || length <= 0 {
                    return fail!(reply, FsError::Errno(libc::EINVAL));
                }
                match wk.fallocate(d, offset as u64, length as u64, mode) {
                    Ok(()) => reply.ok(),
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }

    fn lseek(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        whence: i32,
        reply: ReplyLseek,
    ) {
        let d = self.sh.db(ino);
        self.sh.submit(
            self.sh.owner(d),
            false,
            "lseek",
            d,
            Box::new(move |wk| match wk.lseek(d, offset, whence) {
                Ok(o) => reply.offset(o),
                Err(e) => fail!(reply, e),
            }),
        );
    }

    fn copy_file_range(
        &mut self,
        _req: &Request<'_>,
        ino_in: u64,
        _fh_in: u64,
        offset_in: i64,
        ino_out: u64,
        _fh_out: u64,
        offset_out: i64,
        len: u64,
        _flags: u32,
        reply: ReplyWrite,
    ) {
        let (i_in, i_out) = (self.sh.db(ino_in), self.sh.db(ino_out));
        self.sh.submit(
            self.sh.owner(i_out),
            true,
            "copy_file_range",
            i_out,
            Box::new(move |wk| {
                if offset_in < 0 || offset_out < 0 {
                    return fail!(reply, FsError::Errno(libc::EINVAL));
                }
                match wk.copy_range(i_in, offset_in as u64, i_out, offset_out as u64, len) {
                    Ok(n) => reply.written(n),
                    Err(e) => fail!(reply, e),
                }
            }),
        );
    }
}

/// statfs figures (spec/space.py ReportsStatfs).
pub struct StatfsFigures {
    pub blocks: u64,
    pub bfree: u64,
    pub files: u64,
    pub ffree: u64,
    pub bsize: u32,
}

pub fn statfs_figures(cfg: &FsConfig, u: Usage) -> StatfsFigures {
    let bs = cfg.block_size;
    let capacity = cfg.capacity.unwrap_or(u.physical_bytes + (1u64 << 40));
    let blocks = capacity / bs;
    let used = u.physical_bytes.div_ceil(bs);
    let files = 1u64 << 48;
    StatfsFigures {
        blocks,
        bfree: blocks.saturating_sub(used),
        files,
        ffree: files.saturating_sub(u.inodes),
        bsize: bs as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statfs_reports_capacity_minus_used() {
        let cfg = FsConfig {
            capacity: Some(4096 * 100),
            ..cfg()
        };
        let s = statfs_figures(
            &cfg,
            Usage {
                physical_bytes: 4096 * 10 + 1,
                inodes: 7,
            },
        );
        assert_eq!(s.blocks, 100);
        assert_eq!(s.bfree, 89);
        assert_eq!(s.files, 1 << 48);
        assert_eq!(s.ffree, (1 << 48) - 7);
    }

    pub fn cfg() -> FsConfig {
        FsConfig {
            workers: 1,
            ttl: Duration::from_secs(1),
            read_only: false,
            snapshot: false,
            buffer_per_file: 1,
            buffer_total: 1,
            block_size: 4096,
            inline_max: 0,
            capacity: None,
        }
    }

    /// Every callback the dispatcher implements is instrumented and listed
    /// (spec/app.py InstrumentsFuseCallbacks): the list cannot drift.
    #[test]
    fn instrumented_callbacks_match_the_filesystem_impl() {
        let src = include_str!("fs.rs");
        let start = src.find("impl Filesystem for Dispatcher {").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let implemented: Vec<&str> = body
            .lines()
            .filter_map(|l| l.trim().strip_prefix("fn "))
            .map(|l| l.split('(').next().unwrap())
            .collect();
        // Callbacks answered without a database round trip and without a span.
        let unspanned = [
            "init",
            "destroy",
            "batch_forget",
            "opendir",
            "ioctl",
            "bmap",
        ];
        for f in &implemented {
            if unspanned.contains(f) {
                continue;
            }
            let op = match *f {
                "releasedir" => "readdir",
                other => other,
            };
            assert!(
                metrics::OPS.contains(&op),
                "callback {f} is not instrumented"
            );
        }
        for required in [
            "lookup",
            "forget",
            "batch_forget",
            "getattr",
            "setattr",
            "readlink",
            "mknod",
            "mkdir",
            "unlink",
            "rmdir",
            "symlink",
            "rename",
            "link",
            "open",
            "read",
            "write",
            "flush",
            "release",
            "fsync",
            "opendir",
            "readdir",
            "readdirplus",
            "releasedir",
            "fsyncdir",
            "statfs",
            "setxattr",
            "getxattr",
            "listxattr",
            "removexattr",
            "create",
            "fallocate",
            "lseek",
            "copy_file_range",
            "init",
            "destroy",
        ] {
            assert!(
                implemented.contains(&required),
                "{required} is not implemented"
            );
        }
    }

    #[test]
    fn time_conversions_round_trip() {
        for ns in [0i64, 1, 1_700_000_000_123_456_789, -5_000_000_000] {
            assert_eq!(time_to_ns(ns_to_time(ns)), ns);
        }
    }
}

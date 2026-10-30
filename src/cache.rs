//! The daemon's metadata cache, shared by every worker
//! (spec/concurrency.py InodeShardedWorkers): inode attributes and
//! generations, the kernel's lookup counts (spec/storage.py ForgetsInodes)
//! and the per-inode summary `(buffered_size, buffered_mtime_ns,
//! open_handles)` that lets any worker report buffered sizes without a
//! cross-worker round trip (spec/consistency.py
//! DrainsBuffersBeforeExternalising).

use crate::backend::Attr;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Entry {
    pub attr: Attr,
    /// When `attr` was read; None once something invalidated it.
    pub fetched: Option<Instant>,
    /// Kernel lookup count.
    pub lookups: u64,
    pub buffered_size: Option<u64>,
    pub buffered_mtime_ns: Option<i64>,
    pub open_handles: u32,
    /// The inode was unlinked while open (spec/posix.py OpenAfterUnlink).
    pub orphan: bool,
    /// `security.capability` is known to be absent (spec/posix.py
    /// ExtendedAttributes: answered from the cache).
    pub no_capability: bool,
    /// `version` seen at the last open, for read-only live mounts
    /// (spec/consistency.py ReadOnlyLiveMounts).
    pub opened_version: Option<i64>,
}

pub struct Cache {
    map: Mutex<HashMap<u64, Entry>>,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    pub fn new() -> Cache {
        Cache {
            map: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Entry>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Non-blocking snapshot of the size, for the state dump.
    pub fn try_len(&self) -> Option<usize> {
        self.map.try_lock().ok().map(|m| m.len())
    }

    /// Store fresh attributes; `lookups` is added to the kernel's count.
    pub fn put(&self, attr: &Attr, lookups: u64) {
        let mut m = self.lock();
        let e = m.entry(attr.ino).or_insert_with(|| Entry {
            attr: attr.clone(),
            fetched: None,
            lookups: 0,
            buffered_size: None,
            buffered_mtime_ns: None,
            open_handles: 0,
            orphan: false,
            no_capability: false,
            opened_version: None,
        });
        if e.attr.gen != attr.gen {
            e.no_capability = false;
            e.orphan = false;
            e.opened_version = None;
        }
        e.attr = attr.clone();
        e.fetched = Some(Instant::now());
        e.lookups += lookups;
    }

    pub fn get(&self, ino: u64) -> Option<Entry> {
        self.lock().get(&ino).cloned()
    }

    /// Fresh attributes, if read within `ttl`.
    pub fn fresh(&self, ino: u64, ttl: Duration) -> Option<Attr> {
        let m = self.lock();
        let e = m.get(&ino)?;
        let t = e.fetched?;
        (t.elapsed() < ttl).then(|| e.attr.clone())
    }

    pub fn gen(&self, ino: u64) -> Option<u64> {
        self.lock().get(&ino).map(|e| e.attr.gen)
    }

    pub fn invalidate(&self, ino: u64) {
        if let Some(e) = self.lock().get_mut(&ino) {
            e.fetched = None;
        }
    }

    pub fn invalidate_all(&self) {
        for e in self.lock().values_mut() {
            e.fetched = None;
            e.opened_version = None;
        }
    }

    pub fn set_nlink(&self, ino: u64, nlink: u32, orphan: bool) {
        if let Some(e) = self.lock().get_mut(&ino) {
            e.attr.nlink = nlink;
            e.orphan = orphan;
            e.fetched = None;
        }
    }

    /// The kernel forgot `n` lookups; evict when the count reaches zero
    /// and nothing else holds the entry. Returns true if evicted.
    pub fn forget(&self, ino: u64, n: u64) -> bool {
        let mut m = self.lock();
        let Some(e) = m.get_mut(&ino) else {
            return false;
        };
        e.lookups = e.lookups.saturating_sub(n);
        if e.lookups == 0 && e.open_handles == 0 && e.buffered_size.is_none() {
            m.remove(&ino);
            return true;
        }
        false
    }

    pub fn summary(&self, ino: u64) -> (Option<u64>, Option<i64>) {
        self.lock()
            .get(&ino)
            .map(|e| (e.buffered_size, e.buffered_mtime_ns))
            .unwrap_or((None, None))
    }

    pub fn set_summary(&self, ino: u64, size: Option<u64>, mtime: Option<i64>) {
        if let Some(e) = self.lock().get_mut(&ino) {
            e.buffered_size = size;
            e.buffered_mtime_ns = mtime;
        }
    }

    pub fn open_handle(&self, ino: u64, delta: i32) -> u32 {
        let mut m = self.lock();
        match m.get_mut(&ino) {
            Some(e) => {
                e.open_handles = (e.open_handles as i64 + delta as i64).max(0) as u32;
                e.open_handles
            }
            None => 0,
        }
    }

    pub fn update<F: FnOnce(&mut Entry)>(&self, ino: u64, f: F) {
        if let Some(e) = self.lock().get_mut(&ino) {
            f(e);
        }
    }

    /// Inodes with dirty buffers anywhere on the mount.
    pub fn dirty(&self) -> Vec<u64> {
        self.lock()
            .iter()
            .filter(|(_, e)| e.buffered_size.is_some())
            .map(|(&i, _)| i)
            .collect()
    }

    /// Inodes with open handles.
    pub fn open(&self) -> Vec<u64> {
        self.lock()
            .iter()
            .filter(|(_, e)| e.open_handles > 0)
            .map(|(&i, _)| i)
            .collect()
    }

    pub fn inos(&self) -> Vec<u64> {
        self.lock().keys().copied().collect()
    }

    /// Attributes as the kernel should see them: size and mtime include
    /// buffered writes.
    pub fn with_buffered(&self, mut a: Attr) -> Attr {
        let (size, mtime) = self.summary(a.ino);
        if let Some(s) = size {
            a.size = s;
        }
        if let Some(m) = mtime {
            a.mtime_ns = m;
            a.ctime_ns = a.ctime_ns.max(m);
        }
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Kind;

    fn attr(ino: u64, gen: u64) -> Attr {
        Attr {
            ino,
            gen,
            kind: Kind::File,
            mode: 0o644,
            uid: 0,
            gid: 0,
            nlink: 1,
            size: 10,
            blocks: 0,
            atime_ns: 0,
            mtime_ns: 0,
            ctime_ns: 0,
            btime_ns: 0,
            rdev: 0,
            version: 1,
        }
    }

    #[test]
    fn forget_evicts_at_zero_unless_held() {
        let c = Cache::new();
        c.put(&attr(5, 1), 2);
        assert!(!c.forget(5, 1));
        c.open_handle(5, 1);
        assert!(!c.forget(5, 1));
        c.open_handle(5, -1);
        c.put(&attr(5, 1), 1);
        assert!(c.forget(5, 1));
        assert!(c.get(5).is_none());
    }

    #[test]
    fn buffered_size_overrides_committed() {
        let c = Cache::new();
        c.put(&attr(5, 1), 1);
        c.set_summary(5, Some(99), Some(7));
        let a = c.with_buffered(attr(5, 1));
        assert_eq!(a.size, 99);
        assert_eq!(a.mtime_ns, 7);
        assert_eq!(c.dirty(), vec![5]);
    }

    #[test]
    fn freshness_and_invalidation() {
        let c = Cache::new();
        c.put(&attr(5, 1), 1);
        assert!(c.fresh(5, Duration::from_secs(60)).is_some());
        c.invalidate(5);
        assert!(c.fresh(5, Duration::from_secs(60)).is_none());
        assert_eq!(c.gen(5), Some(1));
    }
}

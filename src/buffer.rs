//! Per-inode write buffers (spec/optimize.py BufferedWriteFlushReq).
//!
//! One buffer per inode, owned by the inode's worker. Buffers are
//! block-granular with dirty byte intervals per block. Committing a partly
//! dirty block overlays the dirty intervals on the committed block, which
//! the caller reads first; a block with nothing committed starts as zeros.
//! That read-modify-write rule is the fix for Bug 1
//! (spec/verify.py BugRegressions). Every write advances a per-inode
//! `write_seq`; commit failures advance `err_seq` (the Linux `errseq_t`
//! model, spec/durability.py FsyncFailureSemantics).

use crate::backend::WriteArgs;
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Debug, Clone)]
struct DirtyBlock {
    data: Vec<u8>,
    /// Sorted, non-overlapping, non-adjacent [start, end) intervals.
    dirty: Vec<(usize, usize)>,
}

impl DirtyBlock {
    fn new(bs: usize) -> Self {
        DirtyBlock {
            data: vec![0; bs],
            dirty: Vec::new(),
        }
    }

    fn mark(&mut self, start: usize, end: usize) {
        let mut s = start;
        let mut e = end;
        let mut out = Vec::with_capacity(self.dirty.len() + 1);
        for &(a, b) in &self.dirty {
            if b < s || a > e {
                out.push((a, b));
            } else {
                s = s.min(a);
                e = e.max(b);
            }
        }
        out.push((s, e));
        out.sort_unstable();
        self.dirty = out;
    }

    /// Drop dirty bytes at or past `len`.
    fn clip(&mut self, len: usize) {
        self.dirty = self
            .dirty
            .iter()
            .filter(|(a, _)| *a < len)
            .map(|&(a, b)| (a, b.min(len)))
            .collect();
        for b in &mut self.data[len..] {
            *b = 0;
        }
    }

    fn covers(&self, len: usize) -> bool {
        len == 0 || self.dirty.first().is_some_and(|&(a, b)| a == 0 && b >= len)
    }

    fn dirty_bytes(&self) -> usize {
        self.dirty.iter().map(|(a, b)| b - a).sum()
    }

    /// Overlay the dirty bytes on `base` (the committed block, possibly
    /// shorter than the block size).
    fn overlay(&self, base: &[u8], len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        let n = base.len().min(len);
        out[..n].copy_from_slice(&base[..n]);
        for &(a, b) in &self.dirty {
            let b = b.min(len);
            if a < b {
                out[a..b].copy_from_slice(&self.data[a..b]);
            }
        }
        out
    }
}

/// The write buffer of one inode.
#[derive(Debug, Clone)]
pub struct InodeBuffer {
    pub ino: u64,
    pub gen: Option<u64>,
    bs: usize,
    blocks: BTreeMap<u64, DirtyBlock>,
    /// The file size including buffered writes.
    pub size: u64,
    /// The size last committed.
    pub committed_size: u64,
    pub mtime_ns: Option<i64>,
    pub write_seq: u64,
    pub committed_seq: u64,
    pub err_seq: u64,
    pub writer_uids: Vec<u32>,
    pub since: Option<Instant>,
    /// A synchronous commit whose replication was not confirmed: the next
    /// fsync must run the barrier.
    pub needs_barrier: bool,
}

/// What a commit needs read from the database first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPlan {
    /// Blocks only partly covered by dirty bytes, whose committed content
    /// must be read and overlaid.
    pub partial: Vec<u64>,
    /// Whether block 0 must be read to learn if the file is inline.
    pub probe_inline: bool,
}

impl InodeBuffer {
    pub fn new(ino: u64, gen: Option<u64>, committed_size: u64, block_size: u64) -> Self {
        InodeBuffer {
            ino,
            gen,
            bs: block_size as usize,
            blocks: BTreeMap::new(),
            size: committed_size,
            committed_size,
            mtime_ns: None,
            write_seq: 0,
            committed_seq: 0,
            err_seq: 0,
            writer_uids: Vec::new(),
            since: None,
            needs_barrier: false,
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.blocks.is_empty() || self.size != self.committed_size
    }

    /// Bytes held in the buffer.
    pub fn bytes(&self) -> usize {
        self.blocks.values().map(DirtyBlock::dirty_bytes).sum()
    }

    /// Buffer a write; returns its write_seq. Mtime is stamped when the
    /// write is accepted (spec/posix.py Timestamps).
    pub fn write(&mut self, offset: u64, data: &[u8], uid: u32, now: i64) -> u64 {
        let bs = self.bs as u64;
        let mut pos = 0usize;
        while pos < data.len() {
            let abs = offset + pos as u64;
            let bno = abs / bs;
            let within = (abs % bs) as usize;
            let n = (self.bs - within).min(data.len() - pos);
            let blk = self
                .blocks
                .entry(bno)
                .or_insert_with(|| DirtyBlock::new(self.bs));
            blk.data[within..within + n].copy_from_slice(&data[pos..pos + n]);
            blk.mark(within, within + n);
            pos += n;
        }
        self.size = self.size.max(offset + data.len() as u64);
        self.mtime_ns = Some(now);
        if !self.writer_uids.contains(&uid) {
            self.writer_uids.push(uid);
        }
        if self.since.is_none() {
            self.since = Some(Instant::now());
        }
        self.write_seq += 1;
        self.write_seq
    }

    /// Overlay buffered bytes onto `out`, which holds committed content of
    /// [offset, offset + out.len()).
    pub fn overlay_read(&self, offset: u64, out: &mut [u8]) {
        let bs = self.bs as u64;
        let end = offset + out.len() as u64;
        if out.is_empty() {
            return;
        }
        for (&bno, blk) in self.blocks.range(offset / bs..=(end - 1) / bs) {
            let bstart = bno * bs;
            for &(a, b) in &blk.dirty {
                let s = (bstart + a as u64).max(offset);
                let e = (bstart + b as u64).min(end);
                if s < e {
                    let src = (s - bstart) as usize..(e - bstart) as usize;
                    out[(s - offset) as usize..(e - offset) as usize]
                        .copy_from_slice(&blk.data[src]);
                }
            }
        }
    }

    /// Truncate the buffer to `new_size`: buffered writes past it are
    /// discarded so no later flush can bring them back
    /// (spec/consistency.py DrainsBuffersBeforeExternalising).
    pub fn truncate(&mut self, new_size: u64) {
        let bs = self.bs as u64;
        let first_gone = new_size.div_ceil(bs);
        self.blocks.retain(|&b, _| b < first_gone);
        if !new_size.is_multiple_of(bs) {
            if let Some(blk) = self.blocks.get_mut(&(new_size / bs)) {
                blk.clip((new_size % bs) as usize);
                if blk.dirty.is_empty() {
                    self.blocks.remove(&(new_size / bs));
                }
            }
        }
        self.size = new_size;
    }

    /// What must be read before committing.
    pub fn plan(&self, inline_max: u64) -> CommitPlan {
        let bs = self.bs as u64;
        let partial = self
            .blocks
            .iter()
            .filter(|(&bno, blk)| {
                let len = self.size.saturating_sub(bno * bs).min(bs) as usize;
                !blk.covers(len)
            })
            .map(|(&b, _)| b)
            .collect();
        CommitPlan {
            partial,
            probe_inline: self.committed_size <= inline_max,
        }
    }

    /// Build the commit from the committed content of the partial blocks
    /// (and block 0 when `committed_inline` is the inline content).
    pub fn build(
        &self,
        inline_max: u64,
        committed_inline: Option<&[u8]>,
        committed: &BTreeMap<u64, Vec<u8>>,
        now: i64,
        sync: bool,
    ) -> WriteArgs {
        let bs = self.bs as u64;
        let mut args = WriteArgs {
            ino: self.ino,
            gen: self.gen,
            size: self.size,
            mtime_ns: self.mtime_ns,
            now,
            writer_uids: self.writer_uids.clone(),
            sync,
            ..Default::default()
        };
        if let Some(inl) = committed_inline {
            if self.size <= inline_max {
                // Stays inline: the whole content in one value.
                let len = self.size as usize;
                let mut content = vec![0u8; len];
                let n = inl.len().min(len);
                content[..n].copy_from_slice(&inl[..n]);
                self.overlay_read(0, &mut content);
                args.inline = Some(content);
                return args;
            }
        }
        let mut wrote_zero = false;
        for (&bno, blk) in &self.blocks {
            let len = self.size.saturating_sub(bno * bs).min(bs) as usize;
            if len == 0 {
                continue;
            }
            let base: &[u8] = match (bno, committed_inline) {
                (0, Some(inl)) => inl,
                _ => committed.get(&bno).map(Vec::as_slice).unwrap_or(&[]),
            };
            args.blocks.push((bno, blk.overlay(base, len)));
            wrote_zero |= bno == 0;
        }
        // An inline file growing past inline_max moves its bytes to block 0.
        if let Some(inl) = committed_inline {
            if !wrote_zero && !inl.is_empty() {
                let len = self.size.min(bs) as usize;
                let mut b0 = vec![0u8; len];
                let n = inl.len().min(len);
                b0[..n].copy_from_slice(&inl[..n]);
                args.blocks.insert(0, (0, b0));
            }
        }
        args
    }

    /// The commit succeeded: everything up to `seq` is in the database.
    pub fn committed(&mut self, seq: u64) {
        self.blocks.clear();
        self.committed_size = self.size;
        self.committed_seq = seq;
        self.writer_uids.clear();
        self.mtime_ns = None;
        self.since = None;
    }

    /// The commit failed: data stays, and every descriptor open now sees
    /// EIO once at its next fsync or close.
    pub fn failed(&mut self) {
        self.err_seq += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(
        buf: &InodeBuffer,
        committed: &[(u64, Vec<u8>)],
        inline: Option<&[u8]>,
        inline_max: u64,
    ) -> WriteArgs {
        let map: BTreeMap<u64, Vec<u8>> = committed.iter().cloned().collect();
        buf.build(inline_max, inline, &map, 1, false)
    }

    /// Bug 1: a partial-block buffered write must not zero untouched bytes.
    #[test]
    fn bug1_partial_block_write_keeps_untouched_bytes() {
        let mut b = InodeBuffer::new(5, None, 10, 8);
        b.write(2, b"XY", 0, 1);
        let plan = b.plan(0);
        assert_eq!(plan.partial, vec![0]);
        let w = commit(
            &b,
            &[(0, b"abcdefgh".to_vec()), (1, b"ij".to_vec())],
            None,
            0,
        );
        assert_eq!(w.blocks, vec![(0, b"abXYefgh".to_vec())]);
        assert_eq!(w.size, 10);
    }

    #[test]
    fn whole_blocks_need_no_read() {
        let mut b = InodeBuffer::new(1, None, 0, 4);
        b.write(0, b"abcdefgh", 0, 1);
        assert!(b.plan(0).partial.is_empty());
        let w = commit(&b, &[], None, 0);
        assert_eq!(w.blocks, vec![(0, b"abcd".to_vec()), (1, b"efgh".to_vec())]);
    }

    #[test]
    fn gaps_read_as_zero_and_last_block_is_short() {
        let mut b = InodeBuffer::new(1, None, 0, 4);
        b.write(6, b"Z", 0, 1);
        let w = commit(&b, &[], None, 0);
        assert_eq!(w.size, 7);
        assert_eq!(w.blocks, vec![(1, vec![0, 0, b'Z'])]);
    }

    #[test]
    fn later_writes_win_in_arrival_order() {
        let mut b = InodeBuffer::new(1, None, 0, 4);
        let s1 = b.write(0, b"aaaa", 0, 1);
        let s2 = b.write(1, b"bb", 0, 2);
        assert!(s2 > s1);
        let mut out = vec![0u8; 4];
        b.overlay_read(0, &mut out);
        assert_eq!(&out, b"abba");
        assert_eq!(b.mtime_ns, Some(2));
    }

    #[test]
    fn truncate_discards_writes_past_the_end() {
        let mut b = InodeBuffer::new(1, None, 0, 4);
        b.write(0, b"abcdefgh", 0, 1);
        b.truncate(5);
        let w = commit(&b, &[], None, 0);
        assert_eq!(w.size, 5);
        assert_eq!(w.blocks, vec![(0, b"abcd".to_vec()), (1, b"e".to_vec())]);
        b.write(6, b"Q", 0, 2);
        let mut out = [9u8; 7];
        b.overlay_read(0, &mut out[..]);
        assert_eq!(out[5], 9, "byte 5 is not buffered");
    }

    #[test]
    fn inline_files_stay_inline_below_the_limit() {
        let mut b = InodeBuffer::new(1, None, 3, 8);
        b.write(1, b"X", 0, 1);
        assert!(b.plan(16).probe_inline);
        let w = commit(&b, &[], Some(b"abc"), 16);
        assert_eq!(w.inline.as_deref(), Some(&b"aXc"[..]));
        assert!(w.blocks.is_empty());
    }

    #[test]
    fn inline_files_move_to_block_zero_past_the_limit() {
        let mut b = InodeBuffer::new(1, None, 3, 8);
        b.write(10, b"Z", 0, 1);
        let w = commit(&b, &[], Some(b"abc"), 4);
        assert!(w.inline.is_none());
        assert_eq!(w.blocks[0], (0, b"abc\0\0\0\0\0".to_vec()));
        assert_eq!(w.blocks[1], (1, vec![0, 0, b'Z']));
    }

    #[test]
    fn errseq_advances_on_failure_and_data_stays() {
        let mut b = InodeBuffer::new(1, None, 0, 4);
        b.write(0, b"x", 0, 1);
        b.failed();
        assert_eq!(b.err_seq, 1);
        assert!(b.is_dirty());
        b.committed(1);
        assert!(!b.is_dirty());
        assert_eq!(b.committed_size, 1);
    }
}

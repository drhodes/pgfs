//! The reference model (spec/verify.py ReferenceModel): an in-memory
//! filesystem implementing `PgfsBackend` by the simplest possible means.
//! Snapshots and branches are whole copies of a tree's state, so the
//! model states what layers and epochs must look like from outside,
//! without sharing any of their mechanism.

#![allow(dead_code)]

use pgfs::backend::*;
use pgfs::errno::{FsError, FsResult};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Inode {
    pub attr: Attr,
    pub data: Vec<u8>,
    pub target: Vec<u8>,
    pub xattrs: BTreeMap<Vec<u8>, Vec<u8>>,
    pub children: BTreeMap<Vec<u8>, u64>,
    pub parent: u64,
}

/// One tree's (or snapshot's) contents.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub inodes: HashMap<u64, Inode>,
    pub orphans: HashSet<u64>,
    pub root: u64,
}

pub struct Model {
    /// layer -> tree name, for views and mutations.
    pub layers: HashMap<i64, String>,
    pub trees: HashMap<String, (i64, State)>,
    pub snapshots: HashMap<(String, String), (i64, State)>,
    /// (layer, epoch) of a snapshot view -> its state key.
    pub snapshot_epochs: HashMap<(i64, i64), (String, String)>,
    /// Trees changed since they were branched from: branch -> (base tree, base layer).
    pub bases: HashMap<String, (String, bool)>,
    next_ino: u64,
    next_layer: i64,
    next_epoch: HashMap<i64, i64>,
    pub block_size: u64,
    pub inline_max: u64,
}

fn errno<T>(e: i32) -> FsResult<T> {
    Err(FsError::Errno(e))
}

fn new_attr(ino: u64, kind: Kind, mode: u32, uid: u32, gid: u32, now: i64) -> Attr {
    Attr {
        ino,
        gen: ino.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 1,
        kind,
        mode: mode & 0o7777,
        uid,
        gid,
        nlink: if kind == Kind::Dir { 2 } else { 1 },
        size: 0,
        blocks: 0,
        atime_ns: now,
        mtime_ns: now,
        ctime_ns: now,
        btime_ns: now,
        rdev: 0,
        version: 1,
    }
}

impl Model {
    pub fn new(block_size: u64, inline_max: u64) -> Model {
        let mut st = State {
            root: 1,
            ..Default::default()
        };
        st.inodes.insert(
            1,
            Inode {
                attr: new_attr(1, Kind::Dir, 0o755, 0, 0, 0),
                data: vec![],
                target: vec![],
                xattrs: BTreeMap::new(),
                children: BTreeMap::new(),
                parent: 1,
            },
        );
        let mut m = Model {
            layers: HashMap::new(),
            trees: HashMap::new(),
            snapshots: HashMap::new(),
            snapshot_epochs: HashMap::new(),
            bases: HashMap::new(),
            next_ino: 2,
            next_layer: 2,
            next_epoch: HashMap::new(),
            block_size,
            inline_max,
        };
        m.layers.insert(1, "main".into());
        m.trees.insert("main".into(), (1, st));
        m.next_epoch.insert(1, 1);
        m
    }

    pub fn layer_of(&self, tree: &str) -> Option<i64> {
        self.trees.get(tree).map(|t| t.0)
    }

    fn state(&self, v: &View) -> FsResult<&State> {
        if let Some(e) = v.epoch {
            if let Some(key) = self.snapshot_epochs.get(&(v.layer, e)) {
                return Ok(&self.snapshots[key].1);
            }
        }
        let name = self
            .layers
            .get(&v.layer)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        Ok(&self.trees[name].1)
    }

    fn state_mut(&mut self, layer: i64) -> FsResult<&mut State> {
        let name = self
            .layers
            .get(&layer)
            .cloned()
            .ok_or(FsError::Errno(libc::EROFS))?;
        if let Some((base, changed)) = self.bases.get_mut(&name) {
            let _ = base;
            *changed = true;
        }
        Ok(&mut self.trees.get_mut(&name).expect("layer maps to a tree").1)
    }

    fn live(st: &State, ino: u64) -> Option<&Inode> {
        st.inodes
            .get(&ino)
            .filter(|i| i.attr.nlink > 0 || st.orphans.contains(&ino))
    }

    fn attr_of(&self, st: &State, ino: u64) -> Attr {
        let i = &st.inodes[&ino];
        let mut a = i.attr.clone();
        if a.kind == Kind::File {
            a.size = i.data.len() as u64;
        }
        a
    }

    fn drop_link(st: &mut State, ino: u64, open: &[u64]) -> (u32, bool) {
        let i = st.inodes.get_mut(&ino).expect("exists");
        i.attr.nlink = i.attr.nlink.saturating_sub(1);
        let n = i.attr.nlink;
        if n == 0 {
            if open.contains(&ino) {
                st.orphans.insert(ino);
                return (0, true);
            }
            st.inodes.remove(&ino);
        }
        (n, false)
    }

    fn is_within(st: &State, mut dir: u64, ancestor: u64) -> bool {
        for _ in 0..4096 {
            if dir == ancestor {
                return true;
            }
            if dir == st.root {
                return false;
            }
            dir = st.inodes[&dir].parent;
        }
        false
    }

    /// Read a whole file's content.
    pub fn content(&self, v: &View, ino: u64) -> Vec<u8> {
        self.state(v)
            .ok()
            .and_then(|s| s.inodes.get(&ino))
            .map(|i| i.data.clone())
            .unwrap_or_default()
    }
}

impl PgfsBackend for Model {
    fn lookup(&mut self, v: &View, parent: u64, name: &[u8]) -> FsResult<Option<Attr>> {
        let st = self.state(v)?;
        let Some(p) = Self::live(st, parent) else {
            return Ok(None);
        };
        Ok(p.children.get(name).map(|&c| self.attr_of(st, c)))
    }

    fn getattr(&mut self, v: &View, ino: u64, gen: Option<u64>) -> FsResult<Option<Attr>> {
        let st = self.state(v)?;
        Ok(Self::live(st, ino)
            .filter(|i| gen.is_none_or(|g| g == i.attr.gen))
            .map(|_| self.attr_of(st, ino)))
    }

    fn readdir(
        &mut self,
        v: &View,
        dir: u64,
        after: Option<&[u8]>,
        limit: u32,
    ) -> FsResult<Vec<DirEntry>> {
        let st = self.state(v)?;
        let Some(d) = Self::live(st, dir) else {
            return Ok(vec![]);
        };
        Ok(d.children
            .iter()
            .filter(|(n, _)| after.is_none_or(|a| n.as_slice() > a))
            .take(limit as usize)
            .map(|(n, &c)| DirEntry {
                name: n.clone(),
                ino: c,
                kind: st.inodes[&c].attr.kind,
            })
            .collect())
    }

    fn readdirplus(
        &mut self,
        v: &View,
        dir: u64,
        after: Option<&[u8]>,
        limit: u32,
    ) -> FsResult<Vec<(Vec<u8>, Attr)>> {
        let list = self.readdir(v, dir, after, limit)?;
        let st = self.state(v)?;
        Ok(list
            .into_iter()
            .map(|e| (e.name, self.attr_of(st, e.ino)))
            .collect())
    }

    fn read_blocks(
        &mut self,
        v: &View,
        ino: u64,
        gen: Option<u64>,
        first: u64,
        last: u64,
    ) -> FsResult<ReadResult> {
        let st = self.state(v)?;
        let i = Self::live(st, ino)
            .filter(|i| gen.is_none_or(|g| g == i.attr.gen))
            .ok_or(FsError::Errno(libc::ESTALE))?;
        if i.attr.kind == Kind::Dir {
            return errno(libc::EISDIR);
        }
        let bs = self.block_size;
        let mut out = ReadResult::default();
        for b in first..=last.min(i.data.len() as u64 / bs + 1) {
            let s = (b * bs) as usize;
            if s >= i.data.len() {
                break;
            }
            let e = (s + bs as usize).min(i.data.len());
            let chunk = &i.data[s..e];
            if chunk.iter().any(|&x| x != 0) {
                out.blocks.push((b, chunk.to_vec()));
            }
        }
        Ok(out)
    }

    fn readlink(&mut self, v: &View, ino: u64) -> FsResult<Vec<u8>> {
        let st = self.state(v)?;
        Self::live(st, ino)
            .map(|i| i.target.clone())
            .ok_or(FsError::Errno(libc::ESTALE))
    }

    fn getxattr(&mut self, v: &View, ino: u64, name: &[u8]) -> FsResult<Option<Vec<u8>>> {
        let st = self.state(v)?;
        Ok(Self::live(st, ino).and_then(|i| i.xattrs.get(name).cloned()))
    }

    fn listxattr(&mut self, v: &View, ino: u64) -> FsResult<Vec<Vec<u8>>> {
        let st = self.state(v)?;
        Ok(Self::live(st, ino)
            .map(|i| i.xattrs.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn seek_block(&mut self, v: &View, ino: u64, block: u64, data: bool) -> FsResult<Option<u64>> {
        let r = self.read_blocks(v, ino, None, block, u64::MAX / 2)?;
        let st = self.state(v)?;
        let size = st.inodes[&ino].data.len() as u64;
        let last = size.div_ceil(self.block_size);
        let have: HashSet<u64> = r.blocks.iter().map(|(b, _)| *b).collect();
        Ok(if data {
            (block..last).find(|b| have.contains(b))
        } else {
            Some(
                (block..last)
                    .find(|b| !have.contains(b))
                    .unwrap_or(last.max(block)),
            )
        })
    }

    fn statfs(&mut self) -> FsResult<Usage> {
        Ok(Usage::default())
    }

    fn make(&mut self, layer: i64, a: &MakeArgs) -> FsResult<Attr> {
        if pgfs::names::check_name(&a.name).is_err() {
            return errno(pgfs::names::check_name(&a.name).unwrap_err());
        }
        if a.kind == Kind::Symlink && a.target.as_ref().is_some_and(|t| t.len() > 4095) {
            return errno(libc::ENAMETOOLONG);
        }
        let ino = self.next_ino;
        let st = self.state_mut(layer)?;
        let p = st
            .inodes
            .get(&a.parent)
            .filter(|i| i.attr.nlink > 0)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        if p.attr.kind != Kind::Dir {
            return errno(libc::ENOTDIR);
        }
        if p.children.contains_key(&a.name) {
            return errno(libc::EEXIST);
        }
        let (mut gid, mut mode) = (a.gid, a.mode & 0o7777);
        if p.attr.mode & 0o2000 != 0 {
            gid = p.attr.gid;
            if a.kind == Kind::Dir {
                mode |= 0o2000;
            }
        }
        if a.kind == Kind::Symlink {
            mode = 0o777;
        }
        let mut attr = new_attr(ino, a.kind, mode, a.uid, gid, a.now);
        attr.rdev = a.rdev;
        let target = a.target.clone().unwrap_or_default();
        if a.kind == Kind::Symlink {
            attr.size = target.len() as u64;
        }
        st.inodes.insert(
            ino,
            Inode {
                attr: attr.clone(),
                data: vec![],
                target,
                xattrs: BTreeMap::new(),
                children: BTreeMap::new(),
                parent: a.parent,
            },
        );
        let p = st.inodes.get_mut(&a.parent).expect("parent");
        p.children.insert(a.name.clone(), ino);
        if a.kind == Kind::Dir {
            p.attr.nlink += 1;
        }
        self.next_ino += 1;
        Ok(attr)
    }

    fn link(
        &mut self,
        layer: i64,
        ino: u64,
        gen: Option<u64>,
        newparent: u64,
        newname: &[u8],
        _now: i64,
    ) -> FsResult<Attr> {
        pgfs::names::check_name(newname).map_err(FsError::Errno)?;
        let st = self.state_mut(layer)?;
        let i = st
            .inodes
            .get(&ino)
            .filter(|i| gen.is_none_or(|g| g == i.attr.gen))
            .ok_or(FsError::Errno(libc::ESTALE))?;
        if i.attr.nlink == 0 {
            return errno(libc::ENOENT);
        }
        if i.attr.kind == Kind::Dir {
            return errno(libc::EPERM);
        }
        if i.attr.nlink >= 65000 {
            return errno(libc::EMLINK);
        }
        let p = st
            .inodes
            .get(&newparent)
            .filter(|i| i.attr.nlink > 0)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        if p.attr.kind != Kind::Dir {
            return errno(libc::ENOTDIR);
        }
        if p.children.contains_key(newname) {
            return errno(libc::EEXIST);
        }
        st.inodes
            .get_mut(&newparent)
            .expect("p")
            .children
            .insert(newname.to_vec(), ino);
        let i = st.inodes.get_mut(&ino).expect("i");
        i.attr.nlink += 1;
        let a = i.attr.clone();
        let size = i.data.len() as u64;
        Ok(Attr {
            size: if a.kind == Kind::File { size } else { a.size },
            ..a
        })
    }

    fn unlink(
        &mut self,
        layer: i64,
        parent: u64,
        name: &[u8],
        open: &[u64],
        _dirty: &[u64],
        _now: i64,
    ) -> FsResult<UnlinkOutcome> {
        pgfs::names::check_name(name).map_err(FsError::Errno)?;
        let st = self.state_mut(layer)?;
        let p = st
            .inodes
            .get(&parent)
            .filter(|i| i.attr.nlink > 0)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let &c = p.children.get(name).ok_or(FsError::Errno(libc::ENOENT))?;
        if st.inodes[&c].attr.kind == Kind::Dir {
            return errno(libc::EISDIR);
        }
        st.inodes.get_mut(&parent).expect("p").children.remove(name);
        let (nlink, orphaned) = Self::drop_link(st, c, open);
        Ok(UnlinkOutcome {
            ino: c,
            nlink,
            orphaned,
        })
    }

    fn rmdir(&mut self, layer: i64, parent: u64, name: &[u8], _now: i64) -> FsResult<u64> {
        pgfs::names::check_name(name).map_err(FsError::Errno)?;
        let st = self.state_mut(layer)?;
        let p = st
            .inodes
            .get(&parent)
            .filter(|i| i.attr.nlink > 0)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let &c = p.children.get(name).ok_or(FsError::Errno(libc::ENOENT))?;
        let ci = &st.inodes[&c];
        if ci.attr.kind != Kind::Dir {
            return errno(libc::ENOTDIR);
        }
        if c == st.root {
            return errno(libc::EBUSY);
        }
        if !ci.children.is_empty() {
            return errno(libc::ENOTEMPTY);
        }
        st.inodes.remove(&c);
        let p = st.inodes.get_mut(&parent).expect("p");
        p.children.remove(name);
        p.attr.nlink -= 1;
        Ok(c)
    }

    fn rename(&mut self, layer: i64, a: &RenameArgs) -> FsResult<RenameOutcome> {
        if a.flags & !3 != 0 || a.flags == 3 {
            return errno(libc::EINVAL);
        }
        pgfs::names::check_name(&a.name).map_err(FsError::Errno)?;
        pgfs::names::check_name(&a.newname).map_err(FsError::Errno)?;
        let st = self.state_mut(layer)?;
        let p = st
            .inodes
            .get(&a.parent)
            .filter(|i| i.attr.nlink > 0)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let &src = p
            .children
            .get(&a.name)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let np = st
            .inodes
            .get(&a.newparent)
            .filter(|i| i.attr.nlink > 0)
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let tgt = np.children.get(&a.newname).copied();
        if src == st.root || tgt == Some(st.root) {
            return errno(libc::EBUSY);
        }
        if tgt == Some(src) {
            return Ok(RenameOutcome {
                ino: src,
                replaced: None,
            });
        }
        let exchange = a.flags & RENAME_EXCHANGE != 0;
        if exchange && tgt.is_none() {
            return errno(libc::ENOENT);
        }
        if a.flags & RENAME_NOREPLACE != 0 && tgt.is_some() {
            return errno(libc::EEXIST);
        }
        let sk = st.inodes[&src].attr.kind;
        let tk = tgt.map(|t| st.inodes[&t].attr.kind);
        if !exchange {
            if let Some(tk) = tk {
                if sk == Kind::Dir && tk != Kind::Dir {
                    return errno(libc::ENOTDIR);
                }
                if sk != Kind::Dir && tk == Kind::Dir {
                    return errno(libc::EISDIR);
                }
            }
        }
        if a.parent != a.newparent {
            if sk == Kind::Dir && Self::is_within(st, a.newparent, src) {
                return errno(libc::EINVAL);
            }
            if exchange && tk == Some(Kind::Dir) && Self::is_within(st, a.parent, tgt.unwrap()) {
                return errno(libc::EINVAL);
            }
        }
        if st.inodes[&a.newparent].attr.kind != Kind::Dir {
            return errno(libc::ENOTDIR);
        }
        if exchange {
            let t = tgt.unwrap();
            st.inodes
                .get_mut(&a.parent)
                .unwrap()
                .children
                .insert(a.name.clone(), t);
            st.inodes
                .get_mut(&a.newparent)
                .unwrap()
                .children
                .insert(a.newname.clone(), src);
            st.inodes.get_mut(&t).unwrap().parent = a.parent;
            st.inodes.get_mut(&src).unwrap().parent = a.newparent;
            if a.parent != a.newparent && (sk == Kind::Dir) != (tk == Some(Kind::Dir)) {
                let d: i64 = if sk == Kind::Dir { 1 } else { -1 };
                let p = st.inodes.get_mut(&a.parent).unwrap();
                p.attr.nlink = (p.attr.nlink as i64 - d) as u32;
                let np = st.inodes.get_mut(&a.newparent).unwrap();
                np.attr.nlink = (np.attr.nlink as i64 + d) as u32;
            }
            return Ok(RenameOutcome {
                ino: src,
                replaced: None,
            });
        }
        let mut replaced = None;
        if let Some(t) = tgt {
            if tk == Some(Kind::Dir) {
                if !st.inodes[&t].children.is_empty() {
                    return errno(libc::ENOTEMPTY);
                }
                st.inodes.remove(&t);
                st.inodes.get_mut(&a.newparent).unwrap().attr.nlink -= 1;
                replaced = Some((t, 0, false));
            } else {
                let (n, o) = Self::drop_link(st, t, &a.open);
                replaced = Some((t, n, o));
            }
        }
        st.inodes
            .get_mut(&a.parent)
            .unwrap()
            .children
            .remove(&a.name);
        st.inodes
            .get_mut(&a.newparent)
            .unwrap()
            .children
            .insert(a.newname.clone(), src);
        st.inodes.get_mut(&src).unwrap().parent = a.newparent;
        if sk == Kind::Dir && a.parent != a.newparent {
            st.inodes.get_mut(&a.parent).unwrap().attr.nlink -= 1;
            st.inodes.get_mut(&a.newparent).unwrap().attr.nlink += 1;
        }
        Ok(RenameOutcome { ino: src, replaced })
    }

    fn setattr(&mut self, layer: i64, a: &SetAttrArgs) -> FsResult<Attr> {
        let bs = self.block_size;
        let st = self.state_mut(layer)?;
        let i = st
            .inodes
            .get_mut(&a.ino)
            .filter(|i| a.gen.is_none_or(|g| g == i.attr.gen))
            .ok_or(FsError::Errno(libc::ESTALE))?;
        let _ = bs;
        if let Some(m) = a.mode {
            i.attr.mode = m & 0o7777;
        }
        if a.uid.is_some_and(|u| u != i.attr.uid) || a.gid.is_some_and(|g| g != i.attr.gid) {
            if i.attr.kind == Kind::File && a.mode.is_none() {
                i.attr.mode &= if i.attr.mode & 0o10 != 0 {
                    !0o6000
                } else {
                    !0o4000
                };
            }
            i.attr.uid = a.uid.unwrap_or(i.attr.uid);
            i.attr.gid = a.gid.unwrap_or(i.attr.gid);
        }
        if let Some(s) = a.size {
            if i.attr.kind == Kind::Dir {
                return errno(libc::EISDIR);
            }
            if i.attr.kind != Kind::File {
                return errno(libc::EINVAL);
            }
            i.data.resize(s as usize, 0);
            i.attr.mtime_ns = a.now;
        }
        if let Some(t) = a.atime_ns {
            i.attr.atime_ns = t;
        }
        if let Some(t) = a.mtime_ns {
            i.attr.mtime_ns = t;
        }
        i.attr.ctime_ns = a.now;
        let mut r = i.attr.clone();
        if r.kind == Kind::File {
            r.size = i.data.len() as u64;
        }
        Ok(r)
    }

    fn write_blocks(&mut self, layer: i64, a: &WriteArgs) -> FsResult<Attr> {
        let bs = self.block_size;
        let st = self.state_mut(layer)?;
        let i = st
            .inodes
            .get_mut(&a.ino)
            .filter(|i| a.gen.is_none_or(|g| g == i.attr.gen))
            .ok_or(FsError::Errno(libc::ESTALE))?;
        if i.attr.kind == Kind::Dir {
            return errno(libc::EISDIR);
        }
        if let Some(inl) = &a.inline {
            i.data = inl.clone();
        } else {
            if let Some((f, l)) = a.hole {
                for b in f..=l {
                    let s = (b * bs) as usize;
                    let e = ((b + 1) * bs) as usize;
                    if s < i.data.len() {
                        let e = e.min(i.data.len());
                        i.data[s..e].fill(0);
                    }
                }
            }
            for (b, d) in &a.blocks {
                let s = (b * bs) as usize;
                if i.data.len() < s + bs as usize {
                    i.data.resize((s + bs as usize).max(i.data.len()), 0);
                }
                i.data[s..s + bs as usize].fill(0);
                i.data[s..s + d.len()].copy_from_slice(d);
            }
        }
        i.data.resize(a.size as usize, 0);
        i.attr.mtime_ns = a.mtime_ns.unwrap_or(a.now);
        i.attr.ctime_ns = a.now;
        let mut r = i.attr.clone();
        r.size = i.data.len() as u64;
        Ok(r)
    }

    fn copy_range(&mut self, layer: i64, a: &CopyArgs) -> FsResult<Attr> {
        let bs = self.block_size as usize;
        let st = self.state_mut(layer)?;
        let src = st
            .inodes
            .get(&a.ino_in)
            .ok_or(FsError::Errno(libc::ESTALE))?
            .data
            .clone();
        let o = st
            .inodes
            .get_mut(&a.ino_out)
            .ok_or(FsError::Errno(libc::ESTALE))?;
        for k in 0..a.count as usize {
            let s = (a.src_first as usize + k) * bs;
            let d = (a.dst_first as usize + k) * bs;
            if o.data.len() < d + bs {
                o.data.resize(d + bs, 0);
            }
            let mut chunk = vec![0u8; bs];
            if s < src.len() {
                let n = (src.len() - s).min(bs);
                chunk[..n].copy_from_slice(&src[s..s + n]);
            }
            o.data[d..d + bs].copy_from_slice(&chunk);
        }
        for (b, d) in &a.blocks {
            let s = *b as usize * bs;
            if o.data.len() < s + bs {
                o.data.resize(s + bs, 0);
            }
            o.data[s..s + bs].fill(0);
            o.data[s..s + d.len()].copy_from_slice(d);
        }
        let size = (a.size as usize).max(o.attr.size as usize);
        o.data
            .resize(a.size.max(o.data.len().min(size) as u64) as usize, 0);
        o.data.truncate(size);
        let mut r = o.attr.clone();
        r.size = o.data.len() as u64;
        Ok(r)
    }

    fn setxattr(
        &mut self,
        layer: i64,
        ino: u64,
        name: &[u8],
        value: &[u8],
        flags: i32,
        _now: i64,
    ) -> FsResult<()> {
        if value.len() > 65536 {
            return errno(libc::E2BIG);
        }
        let st = self.state_mut(layer)?;
        let i = st
            .inodes
            .get_mut(&ino)
            .ok_or(FsError::Errno(libc::ESTALE))?;
        let exists = i.xattrs.contains_key(name);
        if flags & XATTR_CREATE != 0 && exists {
            return errno(libc::EEXIST);
        }
        if flags & XATTR_REPLACE != 0 && !exists {
            return errno(libc::ENODATA);
        }
        i.xattrs.insert(name.to_vec(), value.to_vec());
        Ok(())
    }

    fn removexattr(&mut self, layer: i64, ino: u64, name: &[u8], _now: i64) -> FsResult<()> {
        let st = self.state_mut(layer)?;
        let i = st
            .inodes
            .get_mut(&ino)
            .ok_or(FsError::Errno(libc::ESTALE))?;
        i.xattrs
            .remove(name)
            .map(|_| ())
            .ok_or(FsError::Errno(libc::ENODATA))
    }

    fn release_orphan(&mut self, layer: i64, ino: u64) -> FsResult<bool> {
        let st = self.state_mut(layer)?;
        if st.orphans.remove(&ino) {
            st.inodes.remove(&ino);
            return Ok(true);
        }
        Ok(false)
    }

    fn sync_barrier(&mut self) -> FsResult<()> {
        Ok(())
    }

    fn snapshot(&mut self, tree: &str, name: &str) -> FsResult<()> {
        let (layer, st) = self
            .trees
            .get(tree)
            .cloned()
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let key = (tree.to_string(), name.to_string());
        if self.snapshots.contains_key(&key) {
            return errno(libc::EEXIST);
        }
        let e = self.next_epoch.get(&layer).copied().unwrap_or(1);
        self.next_epoch.insert(layer, e + 1);
        self.snapshot_epochs.insert((layer, e), key.clone());
        self.snapshots.insert(key, (layer, st));
        Ok(())
    }

    fn branch(&mut self, source: &str, snapshot: Option<&str>, new: &str) -> FsResult<()> {
        if self.trees.contains_key(new) {
            return errno(libc::EEXIST);
        }
        let mut depth = 1;
        let mut t = source.to_string();
        while let Some((b, _)) = self.bases.get(&t) {
            depth += 1;
            t = b.clone();
        }
        if depth > 8 {
            return errno(libc::EMLINK);
        }
        let st = match snapshot {
            Some(s) => self
                .snapshots
                .get(&(source.to_string(), s.to_string()))
                .map(|x| x.1.clone()),
            None => self.trees.get(source).map(|x| x.1.clone()),
        }
        .ok_or(FsError::Errno(libc::ENOENT))?;
        if snapshot.is_none() {
            let l = self.trees[source].0;
            *self.next_epoch.entry(l).or_insert(1) += 1;
        }
        let layer = self.next_layer;
        self.next_layer += 1;
        self.layers.insert(layer, new.to_string());
        self.trees.insert(new.to_string(), (layer, st));
        self.next_epoch.insert(layer, 1);
        self.bases
            .insert(new.to_string(), (source.to_string(), false));
        if let Some((_, changed)) = self.bases.get_mut(source) {
            let _ = changed;
        }
        Ok(())
    }

    fn promote(&mut self, branch: &str, tree: &str, force: bool) -> FsResult<()> {
        let (b_layer, b_state) = self
            .trees
            .get(branch)
            .cloned()
            .ok_or(FsError::Errno(libc::ENOENT))?;
        let (t_layer, t_state) = self
            .trees
            .get(tree)
            .cloned()
            .ok_or(FsError::Errno(libc::ENOENT))?;
        if self.bases.get(branch).map(|b| b.0.as_str()) != Some(tree) {
            return errno(libc::EINVAL);
        }
        if self.snapshots.keys().any(|(t, _)| t == branch)
            || self.bases.values().any(|(b, _)| b == branch)
        {
            return errno(libc::EBUSY);
        }
        let _ = t_state;
        let stamp = format!("pre-promote-{}", self.snapshots.len());
        self.snapshot(tree, &stamp)?;
        self.trees.remove(branch);
        self.layers.remove(&b_layer);
        self.bases.remove(branch);
        if force {
            self.layers.insert(b_layer, tree.to_string());
            self.layers.remove(&t_layer);
            self.trees.insert(tree.to_string(), (b_layer, b_state));
        } else {
            self.trees.insert(tree.to_string(), (t_layer, b_state));
        }
        Ok(())
    }
}

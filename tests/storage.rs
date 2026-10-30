//! Storage requirements exercised through `PgBackend` against a real
//! database (spec/storage.py, spec/posix.py, spec/branch.py,
//! spec/space.py, spec/changefeed.py, spec/consistency.py,
//! spec/failure.py). Every test ends with `pgfs check`
//! (spec/verify.py CheckAfterEveryTest).

mod common;

use common::*;
use pgfs::backend::*;
use pgfs::errno::FsError;

macro_rules! db {
    ($tag:expr) => {
        match TestDb::new($tag) {
            Some(d) => d,
            None => return,
        }
    };
}

fn errno<T: std::fmt::Debug>(r: Result<T, FsError>) -> i32 {
    match r {
        Err(e) => e.errno(),
        Ok(v) => panic!("expected an error, got {v:?}"),
    }
}

#[test]
fn names_are_bytes_and_rename_matches_no_wildcards() {
    // Bug 2: a rename treated `%` and `_` as wildcards.
    let d = db!("names");
    let mut be = d.backend();
    let v = d.view("main");
    let l = v.layer;
    let a = mk(&mut be, l, 1, b"a_b", Kind::Dir).unwrap();
    let b = mk(&mut be, l, 1, b"axb", Kind::Dir).unwrap();
    mk(&mut be, l, b.ino, b"keep", Kind::File).unwrap();
    mk(&mut be, l, a.ino, b"%", Kind::File).unwrap();
    mk(&mut be, l, a.ino, &[0xff, 0xfe, b'\\'], Kind::File).unwrap();
    mk(&mut be, l, a.ino, &[b'n'; 255], Kind::File).unwrap();
    assert_eq!(
        errno(mk(&mut be, l, a.ino, &[b'n'; 256], Kind::File)),
        libc::ENAMETOOLONG
    );
    assert_eq!(
        errno(mk(&mut be, l, a.ino, b"a/b", Kind::File)),
        libc::EINVAL
    );
    be.rename(
        l,
        &RenameArgs {
            parent: 1,
            name: b"a_b".to_vec(),
            newparent: 1,
            newname: b"moved".to_vec(),
            flags: 0,
            open: vec![],
            dirty: vec![],
            now: now(),
        },
    )
    .unwrap();
    assert!(
        lookup(&mut be, &v, &[b"axb", b"keep"]).is_some(),
        "the other directory was touched"
    );
    assert!(lookup(&mut be, &v, &[b"moved", &[0xff, 0xfe, b'\\']]).is_some());
    assert_eq!(
        lookup(&mut be, &v, &[b"moved"]).unwrap().ino,
        a.ino,
        "a rename keeps the inode"
    );
    d.assert_check();
}

#[test]
fn root_is_inode_one_and_never_its_own_child() {
    // Bug 4: the root identity collision.
    let d = db!("root");
    let mut be = d.backend();
    let v = d.view("main");
    let r = be.getattr(&v, 1, None).unwrap().unwrap();
    assert_eq!((r.ino, r.kind, r.nlink), (1, Kind::Dir, 2));
    mk(&mut be, v.layer, 1, b"x", Kind::Dir).unwrap();
    let list = be.readdir(&v, 1, None, 100).unwrap();
    assert!(list.iter().all(|e| e.ino != 1));
    assert_eq!(be.getattr(&v, 1, None).unwrap().unwrap().nlink, 3);
    d.assert_check();
}

#[test]
fn exactly_one_exclusive_create_wins() {
    let d = db!("excl");
    let l = d.layer("main");
    let wins: Vec<_> = (0..8)
        .map(|_| {
            let mut be = d.backend();
            std::thread::spawn(move || {
                mk(&mut be, l, 1, b"index.lock", Kind::File)
                    .map(|_| ())
                    .map_err(|e| e.errno())
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert_eq!(wins.iter().filter(|r| r.is_ok()).count(), 1, "{wins:?}");
    assert!(wins.iter().all(|r| r.is_ok() || *r == Err(libc::EEXIST)));
    d.assert_check();
}

#[test]
fn inode_numbers_persist_and_generations_catch_reuse() {
    let d = db!("ino");
    let mut be = d.backend();
    let v = d.view("main");
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    let mut be2 = d.backend();
    let again = be2.lookup(&v, 1, b"f").unwrap().unwrap();
    assert_eq!((again.ino, again.gen), (f.ino, f.gen));
    assert!(
        be.getattr(&v, f.ino, Some(f.gen ^ 1)).unwrap().is_none(),
        "wrong gen is ESTALE"
    );
    assert_eq!(
        errno(be.read_blocks(&v, f.ino, Some(f.gen ^ 1), 0, 0)),
        libc::ESTALE
    );
    assert!(f.gen < (1 << 63));
    d.assert_check();
}

#[test]
fn hard_links_count_names_and_refuse_directories() {
    let d = db!("links");
    let mut be = d.backend();
    let v = d.view("main");
    let l = v.layer;
    let f = mk(&mut be, l, 1, b"f", Kind::File).unwrap();
    let dir = mk(&mut be, l, 1, b"d", Kind::Dir).unwrap();
    let a = be
        .link(l, f.ino, Some(f.gen), dir.ino, b"g", now())
        .unwrap();
    assert_eq!(a.nlink, 2);
    assert_eq!(
        errno(be.link(l, dir.ino, None, 1, b"dl", now())),
        libc::EPERM
    );
    let u = be.unlink(l, 1, b"f", &[], &[], now()).unwrap();
    assert_eq!((u.nlink, u.orphaned), (1, false));
    assert!(lookup(&mut be, &v, &[b"d", b"g"]).is_some());
    assert_eq!(errno(be.unlink(l, 1, b"d", &[], &[], now())), libc::EISDIR);
    d.assert_check();
}

#[test]
fn symlinks_and_special_files() {
    let d = db!("special");
    let mut be = d.backend();
    let v = d.view("main");
    let l = v.layer;
    let s = be
        .make(
            l,
            &MakeArgs {
                parent: 1,
                name: b"s".to_vec(),
                kind: Kind::Symlink,
                mode: 0,
                uid: 1,
                gid: 1,
                rdev: 0,
                target: Some(b"../x\xff".to_vec()),
                now: now(),
            },
        )
        .unwrap();
    assert_eq!(s.mode, 0o777);
    assert_eq!(be.readlink(&v, s.ino).unwrap(), b"../x\xff");
    assert_eq!(
        errno(be.make(
            l,
            &MakeArgs {
                parent: 1,
                name: b"long".to_vec(),
                kind: Kind::Symlink,
                mode: 0,
                uid: 1,
                gid: 1,
                rdev: 0,
                target: Some(vec![b'x'; 4096]),
                now: now()
            }
        )),
        libc::ENAMETOOLONG
    );
    let c = be
        .make(
            l,
            &MakeArgs {
                parent: 1,
                name: b"tty".to_vec(),
                kind: Kind::CharDev,
                mode: 0o600,
                uid: 0,
                gid: 0,
                rdev: 0x0501,
                target: None,
                now: now(),
            },
        )
        .unwrap();
    assert_eq!((c.kind, c.rdev), (Kind::CharDev, 0x0501));
    let p = be
        .make(
            l,
            &MakeArgs {
                parent: 1,
                name: b"fifo".to_vec(),
                kind: Kind::Fifo,
                mode: 0o644,
                uid: 0,
                gid: 0,
                rdev: 0,
                target: None,
                now: now(),
            },
        )
        .unwrap();
    assert_eq!(p.kind, Kind::Fifo);
    d.assert_check();
}

#[test]
fn open_after_unlink_keeps_the_inode_until_release() {
    let d = db!("orphan");
    let mut be = d.backend();
    let v = d.view("main");
    let l = v.layer;
    let f = mk(&mut be, l, 1, b"tmp", Kind::File).unwrap();
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        0,
        &[7u8; 10000],
        d.block_size,
        d.inline_max,
    )
    .unwrap();
    let u = be.unlink(l, 1, b"tmp", &[f.ino], &[], now()).unwrap();
    assert!(u.orphaned);
    assert!(be.lookup(&v, 1, b"tmp").unwrap().is_none());
    assert_eq!(
        read_all(&mut be, &v, f.ino, 10000, d.block_size),
        vec![7u8; 10000]
    );
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        10000,
        10000,
        b"more",
        d.block_size,
        d.inline_max,
    )
    .unwrap();
    assert!(be.release_orphan(l, f.ino).unwrap());
    assert!(be.getattr(&v, f.ino, None).unwrap().is_none());
    assert_eq!(
        d.sql("SELECT count(*) FROM pgfs.file_blocks WHERE died = 9223372036854775807")[0]
            .get::<_, i64>(0),
        0
    );
    d.assert_check();
}

#[test]
fn rename_semantics_and_flags() {
    let d = db!("rename");
    let mut be = d.backend();
    let v = d.view("main");
    let l = v.layer;
    let r = |be: &mut pgfs::pg::PgBackend, p: u64, n: &[u8], np: u64, nn: &[u8], flags: u32| {
        be.rename(
            l,
            &RenameArgs {
                parent: p,
                name: n.to_vec(),
                newparent: np,
                newname: nn.to_vec(),
                flags,
                open: vec![],
                dirty: vec![],
                now: now(),
            },
        )
    };
    let a = mk(&mut be, l, 1, b"a", Kind::Dir).unwrap();
    let b = mk(&mut be, l, a.ino, b"b", Kind::Dir).unwrap();
    let f = mk(&mut be, l, 1, b"f", Kind::File).unwrap();
    let g = mk(&mut be, l, 1, b"g", Kind::File).unwrap();
    assert_eq!(
        errno(r(&mut be, 1, b"a", b.ino, b"x", 0)),
        libc::EINVAL,
        "into its own subtree"
    );
    assert_eq!(
        errno(r(&mut be, 1, b"f", 1, b"g", RENAME_NOREPLACE)),
        libc::EEXIST
    );
    assert_eq!(
        errno(r(&mut be, 1, b"f", 1, b"g", RENAME_WHITEOUT)),
        libc::EINVAL
    );
    assert_eq!(errno(r(&mut be, 1, b"f", 1, b"a", 0)), libc::EISDIR);
    assert_eq!(errno(r(&mut be, 1, b"a", 1, b"f", 0)), libc::ENOTDIR);
    assert_eq!(errno(r(&mut be, 1, b"nope", 1, b"x", 0)), libc::ENOENT);
    mk(&mut be, l, 1, b"empty", Kind::Dir).unwrap();
    assert_eq!(errno(r(&mut be, 1, b"empty", 1, b"a", 0)), libc::ENOTEMPTY);
    // Exchange swaps; nothing else changes.
    r(&mut be, 1, b"f", 1, b"g", RENAME_EXCHANGE).unwrap();
    assert_eq!(be.lookup(&v, 1, b"f").unwrap().unwrap().ino, g.ino);
    assert_eq!(be.lookup(&v, 1, b"g").unwrap().unwrap().ino, f.ino);
    // Replace a file; a directory over an empty directory.
    let out = r(&mut be, 1, b"f", 1, b"g", 0).unwrap();
    assert_eq!(out.replaced.map(|x| x.0), Some(f.ino));
    r(&mut be, a.ino, b"b", 1, b"empty", 0).unwrap();
    assert_eq!(be.getattr(&v, a.ino, None).unwrap().unwrap().nlink, 2);
    assert_eq!(be.getattr(&v, 1, None).unwrap().unwrap().nlink, 4);
    // Same inode under both names: success, nothing changes.
    be.link(l, g.ino, None, 1, b"h", now()).unwrap();
    r(&mut be, 1, b"g", 1, b"h", 0).unwrap();
    assert!(be.lookup(&v, 1, b"g").unwrap().is_some());
    d.assert_check();
}

#[test]
fn setattr_stores_every_field_and_chown_clears_set_id() {
    let d = db!("setattr");
    let mut be = d.backend();
    let v = d.view("main");
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    let a = be
        .setattr(
            v.layer,
            &SetAttrArgs {
                ino: f.ino,
                mode: Some(0o6755),
                atime_ns: Some(11),
                mtime_ns: Some(22),
                now: now(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((a.mode, a.atime_ns, a.mtime_ns), (0o6755, 11, 22));
    let a = be
        .setattr(
            v.layer,
            &SetAttrArgs {
                ino: f.ino,
                uid: Some(5),
                gid: Some(6),
                now: now(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        (a.uid, a.gid, a.mode),
        (5, 6, 0o0755),
        "chown clears suid and sgid (group exec set)"
    );
    let g = be.getattr(&v, f.ino, None).unwrap().unwrap();
    assert_eq!((g.uid, g.gid, g.mode, g.atime_ns), (5, 6, 0o755, 11));
    // A write by a non-owner clears the set-user-id bit.
    be.setattr(
        v.layer,
        &SetAttrArgs {
            ino: f.ino,
            mode: Some(0o4644),
            now: now(),
            ..Default::default()
        },
    )
    .unwrap();
    let a = be
        .write_blocks(
            v.layer,
            &WriteArgs {
                ino: f.ino,
                size: 1,
                inline: Some(b"x".to_vec()),
                writer_uids: vec![77],
                now: now(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(a.mode, 0o644);
    d.assert_check();
}

#[test]
fn writes_keep_untouched_bytes_and_holes_read_zero() {
    let d = db!("writes");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    // Inline while small.
    let a = write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        0,
        b"hello",
        bs,
        d.inline_max,
    )
    .unwrap();
    assert_eq!(a.size, 5);
    assert!(be.read_blocks(&v, f.ino, None, 0, 0).unwrap().inline);
    // Past inline_max: moves to block 0, keeps the bytes.
    let a = write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        5,
        3 * bs + 10,
        b"END",
        bs,
        d.inline_max,
    )
    .unwrap();
    let data = read_all(&mut be, &v, f.ino, a.size, bs);
    assert_eq!(&data[..5], b"hello");
    assert!(data[5..(3 * bs + 10) as usize].iter().all(|&x| x == 0));
    assert_eq!(&data[(3 * bs + 10) as usize..], b"END");
    assert!(!be.read_blocks(&v, f.ino, None, 0, 0).unwrap().inline);
    // Partial-block overwrite keeps the rest of the block (Bug 1).
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        a.size,
        2,
        b"LL",
        bs,
        d.inline_max,
    )
    .unwrap();
    let data = read_all(&mut be, &v, f.ino, a.size, bs);
    assert_eq!(&data[..6], b"heLLo\0");
    // Holes: SEEK_DATA / SEEK_HOLE at block granularity, st_blocks counts data only.
    assert_eq!(be.seek_block(&v, f.ino, 1, true).unwrap(), Some(3));
    assert_eq!(be.seek_block(&v, f.ino, 0, false).unwrap(), Some(1));
    let g = be.getattr(&v, f.ino, None).unwrap().unwrap();
    assert_eq!(g.blocks, 2 * bs / 512);
    // Truncate down mid-block, then extend: the cut bytes read as zero.
    truncate(&mut be, &v, f.ino, Some(f.gen), a.size, 3, bs, d.inline_max).unwrap();
    truncate(&mut be, &v, f.ino, Some(f.gen), 3, 2 * bs, bs, d.inline_max).unwrap();
    let data = read_all(&mut be, &v, f.ino, 2 * bs, bs);
    assert_eq!(&data[..3], b"heL");
    assert!(data[3..].iter().all(|&x| x == 0));
    d.assert_check();
}

#[test]
fn xattrs_follow_the_flags() {
    let d = db!("xattr");
    let mut be = d.backend();
    let v = d.view("main");
    let l = v.layer;
    let f = mk(&mut be, l, 1, b"f", Kind::File).unwrap();
    be.setxattr(l, f.ino, b"user.a", b"1", XATTR_CREATE, now())
        .unwrap();
    assert_eq!(
        errno(be.setxattr(l, f.ino, b"user.a", b"2", XATTR_CREATE, now())),
        libc::EEXIST
    );
    assert_eq!(
        errno(be.setxattr(l, f.ino, b"user.b", b"2", XATTR_REPLACE, now())),
        libc::ENODATA
    );
    be.setxattr(l, f.ino, b"user.a", b"", XATTR_REPLACE, now())
        .unwrap();
    assert_eq!(
        be.getxattr(&v, f.ino, b"user.a").unwrap(),
        Some(vec![]),
        "empty is not absent"
    );
    assert_eq!(
        errno(be.setxattr(l, f.ino, b"user.big", &vec![0; 65537], 0, now())),
        libc::E2BIG
    );
    be.removexattr(l, f.ino, b"user.a", now()).unwrap();
    assert_eq!(
        errno(be.removexattr(l, f.ino, b"user.a", now())),
        libc::ENODATA
    );
    assert!(be.listxattr(&v, f.ino).unwrap().is_empty());
    d.assert_check();
}

#[test]
fn snapshots_freeze_and_branches_copy_on_write() {
    let d = db!("branch");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let l = v.layer;
    let dir = mk(&mut be, l, 1, b"dir", Kind::Dir).unwrap();
    let f = mk(&mut be, l, dir.ino, b"big", Kind::File).unwrap();
    let content: Vec<u8> = (0..(5 * bs)).map(|i| (i % 251) as u8).collect();
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        0,
        &content,
        bs,
        d.inline_max,
    )
    .unwrap();
    be.setxattr(l, f.ino, b"user.k", b"v", 0, now()).unwrap();
    mk(&mut be, l, 1, b"gone", Kind::Dir).unwrap();
    be.snapshot("main", "s1").unwrap();
    let s1 = d.snapshot_view("main", "s1");
    let before = tree_listing(&mut be, &s1, bs);
    be.branch("main", Some("s1"), "dev").unwrap();
    let dv = d.view("dev");
    assert_eq!(
        tree_listing(&mut be, &dv, bs),
        before,
        "a new branch equals its base"
    );
    // Change one block, remove an inherited xattr, an inherited name and an inherited directory.
    write(
        &mut be,
        &dv,
        f.ino,
        Some(f.gen),
        5 * bs,
        bs + 1,
        b"Z",
        bs,
        d.inline_max,
    )
    .unwrap();
    be.removexattr(dv.layer, f.ino, b"user.k", now()).unwrap();
    be.rmdir(dv.layer, 1, b"gone", now()).unwrap();
    be.unlink(dv.layer, dir.ino, b"big", &[], &[], now())
        .unwrap();
    assert!(be.lookup(&dv, dir.ino, b"big").unwrap().is_none());
    // A new name over a whiteout.
    let n = mk(&mut be, dv.layer, dir.ino, b"big", Kind::File).unwrap();
    assert_ne!(n.ino, f.ino);
    // The base and the snapshot never changed.
    assert_eq!(tree_listing(&mut be, &s1, bs), before);
    assert_eq!(read_all(&mut be, &v, f.ino, 5 * bs, bs), content);
    // Copy-up cost: one map row per written block.
    let rows: i64 = d.sql(&format!(
        "SELECT count(*) FROM pgfs.file_blocks WHERE layer = {} AND ino = {}",
        dv.layer, f.ino
    ))[0]
        .get(0);
    assert!(rows <= 1, "only the written block was copied: {rows}");
    d.assert_check();
}

#[test]
fn flatten_keeps_every_view() {
    let d = db!("flatten");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        0,
        &vec![1u8; (3 * bs) as usize],
        bs,
        d.inline_max,
    )
    .unwrap();
    mk(&mut be, v.layer, 1, b"d", Kind::Dir).unwrap();
    be.branch("main", None, "dev").unwrap();
    let dv = d.view("dev");
    truncate(
        &mut be,
        &dv,
        f.ino,
        Some(f.gen),
        3 * bs,
        bs + 5,
        bs,
        d.inline_max,
    )
    .unwrap();
    be.rmdir(dv.layer, 1, b"d", now()).unwrap();
    be.snapshot("dev", "mid").unwrap();
    write(
        &mut be,
        &dv,
        f.ino,
        Some(f.gen),
        bs + 5,
        0,
        b"X",
        bs,
        d.inline_max,
    )
    .unwrap();
    let mid = d.snapshot_view("dev", "mid");
    let (live_before, mid_before) = (
        tree_listing(&mut be, &dv, bs),
        tree_listing(&mut be, &mid, bs),
    );
    let mut c = d.conn();
    loop {
        let n: i64 = c
            .query_sql("SELECT pgfs.flatten_batch('dev', 2)", &[])
            .unwrap()[0]
            .get(0);
        if n == 0
            && c.query_sql("SELECT pgfs.flatten_finish('dev')", &[])
                .unwrap()[0]
                .get::<_, bool>(0)
        {
            break;
        }
    }
    let depth: i32 = d.sql(&format!(
        "SELECT depth FROM pgfs.layers WHERE layer = {}",
        dv.layer
    ))[0]
        .get(0);
    assert_eq!(depth, 0);
    assert_eq!(tree_listing(&mut be, &dv, bs), live_before);
    assert_eq!(tree_listing(&mut be, &mid, bs), mid_before);
    d.assert_check();
}

#[test]
fn promote_folds_back_and_diff_spans_it() {
    let d = db!("promote");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let a = mk(&mut be, v.layer, 1, b"a", Kind::File).unwrap();
    write(
        &mut be,
        &v,
        a.ino,
        Some(a.gen),
        0,
        0,
        b"old",
        bs,
        d.inline_max,
    )
    .unwrap();
    mk(&mut be, v.layer, 1, b"rm", Kind::File).unwrap();
    be.branch("main", None, "dev").unwrap();
    let dv = d.view("dev");
    write(
        &mut be,
        &dv,
        a.ino,
        Some(a.gen),
        3,
        0,
        b"NEW",
        bs,
        d.inline_max,
    )
    .unwrap();
    be.unlink(dv.layer, 1, b"rm", &[], &[], now()).unwrap();
    mk(&mut be, dv.layer, 1, b"added", Kind::File).unwrap();
    let want = tree_listing(&mut be, &dv, bs);
    be.promote("dev", "main", false).unwrap();
    let v = d.view("main");
    assert_eq!(
        v.layer,
        d.layer("main"),
        "a normal promote keeps the tree's layer"
    );
    assert_eq!(tree_listing(&mut be, &v, bs), want);
    // The previous contents are a snapshot, and diff spans the promote.
    let snap: String = d.sql("SELECT name FROM pgfs.snapshots WHERE tree = 'main'")[0].get(0);
    let pv = d.snapshot_view("main", &snap);
    let rows = d.sql(&format!("SELECT change, pgfs.name_text(path) FROM pgfs.diff({}, {}, (SELECT epoch FROM pgfs.layers WHERE layer = {}))", v.layer, pv.epoch.unwrap(), v.layer));
    let changes: Vec<(String, String)> = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    assert!(
        changes.contains(&("added".into(), "/added".into())),
        "{changes:?}"
    );
    assert!(
        changes.contains(&("removed".into(), "/rm".into())),
        "{changes:?}"
    );
    assert!(
        changes.contains(&("modified".into(), "/a".into())),
        "{changes:?}"
    );
    // A tree that changed after the branch point refuses without --force.
    be.branch("main", None, "dev2").unwrap();
    mk(&mut be, v.layer, 1, b"later", Kind::File).unwrap();
    assert_eq!(errno(be.promote("dev2", "main", false)), libc::ESTALE);
    be.promote("dev2", "main", true).unwrap();
    assert!(be.lookup(&d.view("main"), 1, b"later").unwrap().is_none());
    d.assert_check();
}

#[test]
fn branch_depth_is_capped() {
    let d = db!("depth");
    let mut be = d.backend();
    let mut src = "main".to_string();
    for i in 1..=8 {
        let n = format!("b{i}");
        be.branch(&src, None, &n).unwrap();
        src = n;
    }
    assert_eq!(errno(be.branch(&src, None, "b9")), libc::EMLINK);
    d.assert_check();
}

#[test]
fn deleting_snapshots_and_trees_and_reclaiming_space() {
    let d = db!("reclaim");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        0,
        &vec![9u8; (4 * bs) as usize],
        bs,
        d.inline_max,
    )
    .unwrap();
    be.snapshot("main", "s").unwrap();
    be.branch("main", Some("s"), "dev").unwrap();
    let mut c = d.conn();
    assert_eq!(
        c.query_sql("SELECT pgfs.snapshot_delete('main', 's', 0)", &[])
            .unwrap_err()
            .sqlstate
            .as_deref(),
        Some("PF018"),
        "a base snapshot is busy"
    );
    c.query_sql("SELECT pgfs.tree_delete('dev', 0)", &[])
        .unwrap();
    be.unlink(v.layer, 1, b"f", &[], &[], now()).unwrap();
    c.query_sql("SELECT pgfs.snapshot_delete('main', 's', 0)", &[])
        .unwrap();
    for _ in 0..5 {
        c.query_sql("SELECT pgfs.reclaim_batch(10000)", &[])
            .unwrap();
        c.query_sql("SELECT pgfs.gc_batch(1000)", &[]).unwrap();
    }
    let blobs: i64 = d.sql("SELECT count(*) FROM pgfs.blobs")[0].get(0);
    let rows: i64 = d.sql("SELECT count(*) FROM pgfs.file_blocks")[0].get(0);
    assert_eq!((blobs, rows), (0, 0), "every byte came back");
    c.query_sql("SELECT pgfs.fold_usage()", &[]).unwrap();
    d.assert_check();
}

#[test]
fn copy_range_shares_blobs() {
    let d = db!("copy");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let a = mk(&mut be, v.layer, 1, b"a", Kind::File).unwrap();
    let content: Vec<u8> = (0..(4 * bs)).map(|i| (i % 7) as u8 + 1).collect();
    write(
        &mut be,
        &v,
        a.ino,
        Some(a.gen),
        0,
        0,
        &content,
        bs,
        d.inline_max,
    )
    .unwrap();
    let b = mk(&mut be, v.layer, 1, b"b", Kind::File).unwrap();
    be.copy_range(
        v.layer,
        &CopyArgs {
            ino_in: a.ino,
            ino_out: b.ino,
            src_first: 0,
            dst_first: 0,
            count: 4,
            size: 4 * bs,
            now: now(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(read_all(&mut be, &v, b.ino, 4 * bs, bs), content);
    let shared: i64 = d.sql("SELECT count(*) FROM pgfs.blobs WHERE shared")[0].get(0);
    assert_eq!(shared, 4);
    let blobs_before: i64 = d.sql("SELECT count(*) FROM pgfs.blobs")[0].get(0);
    assert_eq!(blobs_before, 4, "no data was copied");
    be.unlink(v.layer, 1, b"a", &[], &[], now()).unwrap();
    d.sql("SELECT pgfs.gc_batch(1000)");
    assert_eq!(
        read_all(&mut be, &v, b.ino, 4 * bs, bs),
        content,
        "GC kept referenced blobs"
    );
    be.unlink(v.layer, 1, b"b", &[], &[], now()).unwrap();
    d.sql("SELECT pgfs.gc_batch(1000)");
    assert_eq!(
        d.sql("SELECT count(*) FROM pgfs.blobs")[0].get::<_, i64>(0),
        0
    );
    d.assert_check();
}

#[test]
fn a_mount_that_lost_its_lease_commits_nothing() {
    let d = db!("fence");
    let (mut bs, id, _fence, health) = d.mount_backend("main", 1);
    let l = d.layer("main");
    mk(&mut bs[0], l, 1, b"before", Kind::File).unwrap();
    // A second read-write mount is refused while the lease is held.
    let mut c = d.conn();
    let e = c
        .query_sql(
            "SELECT * FROM pgfs.mount_acquire('main', NULL, false, 'other', 2, 1, 30)",
            &[],
        )
        .unwrap_err();
    assert_eq!(e.sqlstate.as_deref(), Some("PF018"));
    // Another mount takes over (the lease row is replaced with a new fence).
    c.batch(&format!(
        "UPDATE pgfs.mounts SET lease_until = now() - interval '1 second' WHERE mount_id = '{id}'"
    ))
    .unwrap();
    c.query_sql(
        "SELECT * FROM pgfs.mount_acquire('main', NULL, false, 'other', 2, 1, 30)",
        &[],
    )
    .unwrap();
    let r = mk(&mut bs[0], l, 1, b"after", Kind::File);
    assert!(matches!(r, Err(FsError::Fenced(_))), "{r:?}");
    assert_eq!(health.state(), pgfs::health::DbState::Fenced);
    assert!(d
        .sql("SELECT 1 FROM pgfs.dentries WHERE name = 'after'::bytea")
        .is_empty());
    d.assert_check();
}

#[test]
fn op_log_records_each_mutation_once() {
    let d = db!("oplog");
    let (mut bs, id, _f, _h) = d.mount_backend("main", 2);
    let l = d.layer("main");
    let a = mk(&mut bs[1], l, 1, b"x", Kind::File).unwrap();
    let rows = d.sql(&format!("SELECT worker_id, op_seq, result FROM pgfs.op_log WHERE mount_id = '{id}' ORDER BY worker_id"));
    assert_eq!(rows.len(), 2, "one slot per worker");
    assert_eq!(rows[1].get::<_, i64>(1), 1);
    let res: serde_json::Value = rows[1].get(2);
    assert_eq!(
        pgfs::pg::attr_from_json(&res).unwrap().ino,
        a.ino,
        "the slot holds the reply"
    );
    bs[0].sync_barrier().unwrap();
    assert_eq!(
        d.sql(&format!(
            "SELECT op_seq FROM pgfs.op_log WHERE mount_id = '{id}' AND worker_id = 0"
        ))[0]
            .get::<_, i64>(0),
        1
    );
    d.assert_check();
}

#[test]
fn change_feed_delivers_in_order_exactly_once() {
    let d = db!("changes");
    let mut be = d.backend();
    let v = d.view("main");
    let mut c = d.conn();
    let tok: serde_json::Value = c
        .query_sql("SELECT pgfs.changes_register('c', 'main')", &[])
        .unwrap()[0]
        .get(0);
    let _ = tok;
    for i in 0..25 {
        mk(&mut be, v.layer, 1, format!("f{i}").as_bytes(), Kind::File).unwrap();
    }
    be.rename(
        v.layer,
        &RenameArgs {
            parent: 1,
            name: b"f0".to_vec(),
            newparent: 1,
            newname: b"g0".to_vec(),
            flags: 0,
            open: vec![],
            dirty: vec![],
            now: now(),
        },
    )
    .unwrap();
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    // Delivery waits for older open write transactions anywhere in the
    // cluster (spec/changefeed.py HorizonOrderedDelivery), so poll.
    while seen.len() < 26 && std::time::Instant::now() < deadline {
        let rows = c.query_sql("SELECT seq, op, pgfs.name_text(path), pgfs.name_text(new_path), watermark FROM pgfs.changes_next('c', 7)", &[]).unwrap();
        let wm: serde_json::Value = rows.last().unwrap().get(4);
        for r in &rows[..rows.len() - 1] {
            seen.push((
                r.get::<_, i64>(0),
                r.get::<_, String>(1),
                r.get::<_, Option<String>>(2),
                r.get::<_, Option<String>>(3),
            ));
        }
        c.query_sql("SELECT pgfs.changes_ack('c', $1)", &[&wm])
            .unwrap();
        if rows.len() == 1 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    assert_eq!(seen.len(), 26, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0].0 < w[1].0), "in seq order");
    assert_eq!(seen[0].2.as_deref(), Some("/f0"));
    assert_eq!(
        seen[25],
        (
            seen[25].0,
            "rename".into(),
            Some("/f0".into()),
            Some("/g0".into())
        )
    );
    // A --force promote moves the tree: the consumer must resync.
    be.branch("main", None, "dev").unwrap();
    be.promote("dev", "main", true).unwrap();
    let e = c
        .query_sql("SELECT * FROM pgfs.changes_next('c', 7)", &[])
        .unwrap_err();
    assert_eq!(e.sqlstate.as_deref(), Some("PF011"));
    d.assert_check();
}

#[test]
fn subtree_branch_is_rooted_at_its_directory() {
    let d = db!("subtree");
    let mut be = d.backend();
    let v = d.view("main");
    let p = mk(&mut be, v.layer, 1, b"projects", Kind::Dir).unwrap();
    let w = mk(&mut be, v.layer, p.ino, b"web", Kind::Dir).unwrap();
    mk(&mut be, v.layer, w.ino, b"index.html", Kind::File).unwrap();
    d.sql("SELECT * FROM pgfs.branch('main', NULL, '/projects/web'::bytea, 'job')");
    let jv = d.view("job");
    assert_eq!(jv.root, w.ino);
    let list = be.readdir(&jv, jv.root, None, 10).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, b"index.html");
    let e = d
        .conn()
        .query_sql(
            "SELECT * FROM pgfs.branch('main', NULL, '/projects/web/index.html'::bytea, 'bad')",
            &[],
        )
        .unwrap_err();
    assert_eq!(e.sqlstate.as_deref(), Some("PF006"), "ENOTDIR");
    d.assert_check();
}

#[test]
fn check_catches_writes_behind_the_functions() {
    // spec/verify.py DivergenceTests: a write through SQL as the owner.
    let d = db!("diverge");
    let mut be = d.backend();
    let v = d.view("main");
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    d.sql(&format!(
        "UPDATE pgfs.inodes SET nlink = 5 WHERE ino = {}",
        f.ino
    ));
    let v2 = pgfs::check::violations(&mut d.conn(), None).unwrap();
    assert!(v2.iter().any(|x| x.class == 3), "{v2:?}");
}

#[test]
fn usage_is_accounted_by_deltas() {
    let d = db!("usage");
    let mut be = d.backend();
    let v = d.view("main");
    let f = mk(&mut be, v.layer, 1, b"f", Kind::File).unwrap();
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        0,
        &vec![1u8; 10000],
        d.block_size,
        d.inline_max,
    )
    .unwrap();
    let (bytes, files): (i64, i64) = {
        let r = d.sql(&format!(
            "SELECT logical_bytes, inodes FROM pgfs.layer_usage({})",
            v.layer
        ));
        (r[0].get(0), r[0].get(1))
    };
    assert_eq!((bytes, files), (10000, 2));
    d.sql("SELECT pgfs.fold_usage()");
    let r = d.sql(&format!(
        "SELECT logical_bytes, inodes FROM pgfs.layer_usage({})",
        v.layer
    ));
    assert_eq!((r[0].get::<_, i64>(0), r[0].get::<_, i64>(1)), (10000, 2));
    let u = be.statfs().unwrap();
    assert!(u.physical_bytes >= 10000);
    d.assert_check();
}

#[test]
fn export_then_import_reproduces_a_snapshot() {
    let d = db!("archive");
    let mut be = d.backend();
    let v = d.view("main");
    let bs = d.block_size;
    let dir = mk(&mut be, v.layer, 1, b"dir", Kind::Dir).unwrap();
    let f = mk(&mut be, v.layer, dir.ino, b"f\xff", Kind::File).unwrap();
    write(
        &mut be,
        &v,
        f.ino,
        Some(f.gen),
        0,
        3 * bs,
        b"tail",
        bs,
        d.inline_max,
    )
    .unwrap();
    be.setxattr(v.layer, f.ino, b"user.x", b"y", 0, now())
        .unwrap();
    be.link(v.layer, f.ino, None, 1, b"hard", now()).unwrap();
    be.make(
        v.layer,
        &MakeArgs {
            parent: 1,
            name: b"s".to_vec(),
            kind: Kind::Symlink,
            mode: 0,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            target: Some(b"dir".to_vec()),
            now: now(),
        },
    )
    .unwrap();
    be.snapshot("main", "s").unwrap();
    let want = tree_listing(&mut be, &d.snapshot_view("main", "s"), bs);
    let mut tar = Vec::new();
    pgfs::archive::export(&d.settings, "main", "s", None, &mut tar).unwrap();
    // Import into a fresh tree.
    d.sql("SELECT * FROM pgfs.branch('main', NULL, NULL, 'scratch')");
    d.sql("SELECT pgfs.tree_delete('scratch', 0)");
    let d2 = db!("archive2");
    pgfs::archive::import(&d2.settings, "main", Some("/restored"), &tar[..]).unwrap();
    let mut be2 = d2.backend();
    let v2 = d2.view("main");
    let r = be2.lookup(&v2, 1, b"restored").unwrap().unwrap();
    let got = tree_listing(&mut be2, &View { root: r.ino, ..v2 }, bs);
    assert_eq!(got, want);
    // Holes stay holes.
    let ff = lookup(&mut be2, &View { root: r.ino, ..v2 }, &[b"dir", b"f\xff"]).unwrap();
    assert!(be2
        .read_blocks(&v2, ff.ino, None, 0, 2)
        .unwrap()
        .blocks
        .is_empty());
    d2.assert_check();
}

#[test]
fn scratch_filesystems_use_unlogged_tables_and_reset() {
    let d = match TestDb::with(
        "scratch",
        pgfs::schema::MkfsOptions {
            scratch: true,
            block_size: 4096,
            inline_max: 64,
            ..Default::default()
        },
    ) {
        Some(d) => d,
        None => return,
    };
    let unlogged: Vec<String> = d
        .sql("SELECT relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'pgfs' AND c.relkind = 'r' AND c.relpersistence = 'u' ORDER BY 1")
        .iter()
        .map(|r| r.get(0))
        .collect();
    for t in pgfs::schema::SCRATCH_UNLOGGED {
        assert!(
            unlogged.contains(&t.to_string()),
            "{t} should be unlogged: {unlogged:?}"
        );
    }
    // What a crash leaves: empty unlogged tables. A fresh mount resets.
    d.sql("TRUNCATE pgfs.inodes, pgfs.dentries, pgfs.file_blocks, pgfs.blobs, pgfs.xattrs, pgfs.orphans, pgfs.changes, pgfs.usage_deltas, pgfs.usage_totals, pgfs.gc_queue");
    assert_eq!(d.sql("SELECT pgfs.scratch_reset()")[0].get::<_, i32>(0), 1);
    let mut be = d.backend();
    assert!(be.getattr(&d.view("main"), 1, None).unwrap().is_some());
}

#[test]
fn readdir_pages_by_name_and_sees_branches_merged() {
    let d = db!("readdir");
    let mut be = d.backend();
    let v = d.view("main");
    for i in 0..50 {
        mk(
            &mut be,
            v.layer,
            1,
            format!("n{i:02}").as_bytes(),
            Kind::File,
        )
        .unwrap();
    }
    be.branch("main", None, "dev").unwrap();
    let dv = d.view("dev");
    be.unlink(dv.layer, 1, b"n10", &[], &[], now()).unwrap();
    mk(&mut be, dv.layer, 1, b"n10x", Kind::File).unwrap();
    let mut names = Vec::new();
    let mut after: Option<Vec<u8>> = None;
    loop {
        let page = be.readdir(&dv, 1, after.as_deref(), 7).unwrap();
        if page.is_empty() {
            break;
        }
        after = page.last().map(|e| e.name.clone());
        names.extend(page.into_iter().map(|e| String::from_utf8(e.name).unwrap()));
    }
    assert_eq!(names.len(), 50);
    assert!(names.windows(2).all(|w| w[0] < w[1]));
    assert!(!names.contains(&"n10".to_string()) && names.contains(&"n10x".to_string()));
    d.assert_check();
}

#[test]
fn the_runtime_role_can_only_go_through_the_functions() {
    // spec/security.py LeastPrivilegeRoles.
    let role = format!("pgfs_rt_{}", std::process::id());
    let admin = pgfs::conn::ConnSettings::new(&format!("{} dbname=postgres", cluster()));
    let Ok(mut a) = pgfs::conn::Conn::open(&admin) else {
        return;
    };
    a.batch(&format!(
        "DROP ROLE IF EXISTS {role}; CREATE ROLE {role} LOGIN"
    ))
    .unwrap();
    let d = TestDb::with(
        "roles",
        pgfs::schema::MkfsOptions {
            runtime_role: Some(role.clone()),
            block_size: 4096,
            inline_max: 64,
            ..Default::default()
        },
    )
    .unwrap();
    let rt = pgfs::conn::ConnSettings::new(&format!("{} dbname={} user={role}", cluster(), d.name));
    let mut c = pgfs::conn::Conn::open(&rt).unwrap();
    let r = c.query_sql(
        "SELECT ino FROM pgfs.create(1, NULL, NULL, 0, 0, 1, 'ok'::bytea, 420, 1, 1, 1)",
        &[],
    );
    assert!(r.is_ok(), "{r:?}");
    let e = c
        .query_sql("UPDATE pgfs.inodes SET nlink = 9", &[])
        .unwrap_err();
    assert_eq!(e.sqlstate.as_deref(), Some("42501"), "{e}");
    let e = c.query_sql("SELECT pgfs._inode(1, 1, 1)", &[]).unwrap_err();
    assert_eq!(
        e.sqlstate.as_deref(),
        Some("42501"),
        "internal helpers are not granted: {e}"
    );
    assert!(
        c.query_sql("SELECT count(*) FROM pgfs.files", &[]).is_ok(),
        "the views are readable"
    );
    drop(c);
    d.assert_check();
    drop(d);
    let _ = a.batch(&format!("DROP ROLE IF EXISTS {role}"));
}

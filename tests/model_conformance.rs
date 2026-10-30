//! Generated operation sequences run against the reference model and a
//! real database; after every step the replies and the whole tree must
//! agree (spec/verify.py ModelConformance). Names include `%`, `_`, `\`,
//! non-UTF-8 bytes and 255-byte names; operations include snapshot,
//! branch and promote.

mod common;
mod model;

use common::*;
use pgfs::backend::*;
use pgfs::errno::FsResult;
use proptest::prelude::*;

fn names() -> Vec<Vec<u8>> {
    vec![
        b"d".to_vec(),
        b"e".to_vec(),
        b"%".to_vec(),
        b"_".to_vec(),
        b"\\".to_vec(),
        vec![0xff, 0x80],
        vec![b'n'; 255],
    ]
}

#[derive(Debug, Clone)]
enum Op {
    Create(u8, u8),
    Mkdir(u8, u8),
    Symlink(u8, u8),
    Unlink(u8, u8),
    Rmdir(u8, u8),
    Rename(u8, u8, u8, u8, u32),
    Link(u8, u8, u8, u8),
    Write(u8, u8, u16, u16, u8),
    Truncate(u8, u8, u16),
    SetX(u8, u8, u8, u8),
    RmX(u8, u8, u8),
    Chmod(u8, u8, u16),
    Snapshot,
    Branch,
    Promote,
}

fn op() -> impl Strategy<Value = Op> {
    let d = 0u8..3;
    let n = 0u8..7;
    prop_oneof![
        3 => (d.clone(), n.clone()).prop_map(|(a, b)| Op::Create(a, b)),
        3 => (d.clone(), n.clone()).prop_map(|(a, b)| Op::Mkdir(a, b)),
        1 => (d.clone(), n.clone()).prop_map(|(a, b)| Op::Symlink(a, b)),
        2 => (d.clone(), n.clone()).prop_map(|(a, b)| Op::Unlink(a, b)),
        1 => (d.clone(), n.clone()).prop_map(|(a, b)| Op::Rmdir(a, b)),
        3 => (d.clone(), n.clone(), d.clone(), n.clone(), prop_oneof![Just(0u32), Just(1), Just(2), Just(4)])
            .prop_map(|(a, b, c, e, f)| Op::Rename(a, b, c, e, f)),
        1 => (d.clone(), n.clone(), d.clone(), n.clone()).prop_map(|(a, b, c, e)| Op::Link(a, b, c, e)),
        4 => (d.clone(), n.clone(), 0u16..20000, 1u16..9000, any::<u8>()).prop_map(|(a, b, o, l, x)| Op::Write(a, b, o, l, x)),
        2 => (d.clone(), n.clone(), 0u16..20000).prop_map(|(a, b, s)| Op::Truncate(a, b, s)),
        1 => (d.clone(), n.clone(), 0u8..3, any::<u8>()).prop_map(|(a, b, x, v)| Op::SetX(a, b, x, v)),
        1 => (d.clone(), n.clone(), 0u8..3).prop_map(|(a, b, x)| Op::RmX(a, b, x)),
        1 => (d.clone(), n.clone(), 0u16..0o7777).prop_map(|(a, b, m)| Op::Chmod(a, b, m)),
        1 => Just(Op::Snapshot),
        1 => Just(Op::Branch),
        1 => Just(Op::Promote),
    ]
}

struct Side<'a> {
    be: &'a mut dyn PgfsBackend,
    view: View,
}

const DIRS: [&[&[u8]]; 3] = [&[], &[b"d"], &[b"d", b"e"]];

fn res<T>(r: FsResult<T>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(e) => format!("E{}", pgfs::errno::errno_name(e.errno())),
    }
}

fn dir(s: &mut Side, i: u8) -> Option<u64> {
    let a = lookup(s.be, &s.view, DIRS[i as usize])?;
    (a.kind == Kind::Dir).then_some(a.ino)
}

fn apply(
    s: &mut Side,
    op: &Op,
    bs: u64,
    inline_max: u64,
    tree: &str,
    step: usize,
    base: Option<&str>,
) -> String {
    let ns = names();
    let l = s.view.layer;
    let nm = |i: u8| ns[i as usize].clone();
    let file = |s: &mut Side, d: u8, n: u8| -> Option<Attr> {
        let p = dir(s, d)?;
        s.be.lookup(&s.view, p, &nm(n)).unwrap()
    };
    match op {
        Op::Create(d, n) | Op::Mkdir(d, n) | Op::Symlink(d, n) => {
            let Some(p) = dir(s, *d) else {
                return "nodir".into();
            };
            let kind = match op {
                Op::Create(..) => Kind::File,
                Op::Mkdir(..) => Kind::Dir,
                _ => Kind::Symlink,
            };
            res(mk(s.be, l, p, &nm(*n), kind))
        }
        Op::Unlink(d, n) => {
            let Some(p) = dir(s, *d) else {
                return "nodir".into();
            };
            res(s.be.unlink(l, p, &nm(*n), &[], &[], now()))
        }
        Op::Rmdir(d, n) => {
            let Some(p) = dir(s, *d) else {
                return "nodir".into();
            };
            res(s.be.rmdir(l, p, &nm(*n), now()))
        }
        Op::Rename(d, n, d2, n2, f) => {
            let (Some(p), Some(p2)) = (dir(s, *d), dir(s, *d2)) else {
                return "nodir".into();
            };
            res(s.be.rename(
                l,
                &RenameArgs {
                    parent: p,
                    name: nm(*n),
                    newparent: p2,
                    newname: nm(*n2),
                    flags: *f,
                    open: vec![],
                    dirty: vec![],
                    now: now(),
                },
            ))
        }
        Op::Link(d, n, d2, n2) => {
            let Some(a) = file(s, *d, *n) else {
                return "nofile".into();
            };
            let Some(p2) = dir(s, *d2) else {
                return "nodir".into();
            };
            res(s.be.link(l, a.ino, None, p2, &nm(*n2), now()))
        }
        Op::Write(d, n, off, len, x) => {
            let Some(a) = file(s, *d, *n) else {
                return "nofile".into();
            };
            if a.kind != Kind::File {
                return "notfile".into();
            }
            let data = vec![*x; *len as usize];
            res(write(
                s.be,
                &s.view,
                a.ino,
                None,
                a.size,
                *off as u64,
                &data,
                bs,
                inline_max,
            ))
        }
        Op::Truncate(d, n, size) => {
            let Some(a) = file(s, *d, *n) else {
                return "nofile".into();
            };
            if a.kind != Kind::File {
                return "notfile".into();
            }
            res(truncate(
                s.be,
                &s.view,
                a.ino,
                None,
                a.size,
                *size as u64,
                bs,
                inline_max,
            ))
        }
        Op::SetX(d, n, x, v) => {
            let Some(a) = file(s, *d, *n) else {
                return "nofile".into();
            };
            res(s
                .be
                .setxattr(l, a.ino, format!("user.x{x}").as_bytes(), &[*v], 0, now()))
        }
        Op::RmX(d, n, x) => {
            let Some(a) = file(s, *d, *n) else {
                return "nofile".into();
            };
            res(s
                .be
                .removexattr(l, a.ino, format!("user.x{x}").as_bytes(), now()))
        }
        Op::Chmod(d, n, m) => {
            let Some(a) = file(s, *d, *n) else {
                return "nofile".into();
            };
            res(s.be.setattr(
                l,
                &SetAttrArgs {
                    ino: a.ino,
                    mode: Some(*m as u32),
                    now: now(),
                    ..Default::default()
                },
            ))
        }
        Op::Snapshot => res(s.be.snapshot(tree, &format!("s{step}"))),
        Op::Branch => res(s.be.branch(tree, None, &format!("b{step}"))),
        Op::Promote => match base {
            Some(b) => res(s.be.promote(tree, b, false)),
            None => "nobase".into(),
        },
    }
}

fn run(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let Some(d) = TestDb::new("model") else {
        return Ok(());
    };
    let (bs, im) = (d.block_size, d.inline_max);
    let mut db = d.backend();
    let mut m = model::Model::new(bs, im);
    let mut tree = "main".to_string();
    let mut bases: Vec<String> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let base = bases.last().cloned();
        let dv = d.view(&tree);
        let mv = View {
            layer: m.layer_of(&tree).unwrap(),
            epoch: None,
            root: 1,
        };
        let a = apply(
            &mut Side {
                be: &mut db,
                view: dv,
            },
            op,
            bs,
            im,
            &tree,
            i,
            base.as_deref(),
        );
        let b = apply(
            &mut Side {
                be: &mut m,
                view: mv,
            },
            op,
            bs,
            im,
            &tree,
            i,
            base.as_deref(),
        );
        prop_assert_eq!(
            &a,
            &b,
            "step {} {:?} on {}: database {} model {}",
            i,
            op,
            tree,
            a,
            b
        );
        if a == "ok" {
            match op {
                Op::Branch => {
                    bases.push(tree.clone());
                    tree = format!("b{i}");
                }
                Op::Promote => tree = bases.pop().unwrap(),
                _ => {}
            }
        }
        let dv = d.view(&tree);
        let mv = View {
            layer: m.layer_of(&tree).unwrap(),
            epoch: None,
            root: 1,
        };
        let dl = tree_listing(&mut db, &dv, bs);
        let ml = tree_listing(&mut m, &mv, bs);
        prop_assert_eq!(dl, ml, "after step {} {:?} on {}", i, op, tree);
    }
    let v = pgfs::check::violations(&mut d.conn(), None).unwrap();
    prop_assert!(v.is_empty(), "pgfs check: {:?}", v);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, max_shrink_iters: 200, ..ProptestConfig::default() })]
    #[test]
    fn database_matches_the_model(ops in prop::collection::vec(op(), 1..40)) {
        run(ops)?;
    }
}

/// Targeted scenarios on a branch (spec/verify.py ModelConformance).
#[test]
fn branch_scenarios_match_the_model() {
    use Op::*;
    let scenarios: Vec<Vec<Op>> = vec![
        // rename-back then unlink, on a branch.
        vec![
            Create(0, 1),
            Branch,
            Rename(0, 1, 0, 2, 0),
            Rename(0, 2, 0, 1, 0),
            Unlink(0, 1),
            Promote,
        ],
        // unlink-create-unlink.
        vec![
            Create(0, 1),
            Write(0, 1, 0, 5000, 7),
            Branch,
            Unlink(0, 1),
            Create(0, 1),
            Unlink(0, 1),
            Promote,
        ],
        // chmod then unlink; set then remove an inherited xattr.
        vec![
            Create(0, 1),
            SetX(0, 1, 0, 9),
            Branch,
            Chmod(0, 1, 0o600),
            RmX(0, 1, 0),
            Unlink(0, 1),
            Promote,
        ],
        // rmdir of an inherited directory with a removed child.
        vec![
            Mkdir(0, 0),
            Create(1, 1),
            Branch,
            Unlink(1, 1),
            Rmdir(0, 0),
            Promote,
        ],
        // truncate an inherited file below its blocks, then extend.
        vec![
            Create(0, 1),
            Write(0, 1, 0, 9000, 3),
            Branch,
            Truncate(0, 1, 100),
            Write(0, 1, 15000, 10, 4),
            Promote,
        ],
        // branch of a branch, folded back twice.
        vec![
            Create(0, 1),
            Branch,
            Write(0, 1, 0, 10, 1),
            Branch,
            Write(0, 1, 5, 10, 2),
            Promote,
            Promote,
        ],
    ];
    for s in scenarios {
        run(s.clone()).unwrap_or_else(|e| panic!("{s:?}: {e}"));
    }
}

//! A fresh pgfs database per test, on the project-local cluster under
//! testdata/ (spec/app.py ProvidesProjectLocalCluster). Tests skip when
//! the cluster is not running.

#![allow(dead_code)]

use pgfs::backend::*;
use pgfs::buffer::InodeBuffer;
use pgfs::conn::{Conn, ConnSettings};
use pgfs::errno::FsResult;
use pgfs::health::Health;
use pgfs::pg::{MountIdentity, PgBackend};
use pgfs::schema::{self, MkfsOptions};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

static SEQ: AtomicU32 = AtomicU32::new(0);

pub fn cluster() -> String {
    format!("host={}/testdata", env!("CARGO_MANIFEST_DIR"))
}

pub struct TestDb {
    pub name: String,
    pub settings: ConnSettings,
    pub block_size: u64,
    pub inline_max: u64,
}

impl TestDb {
    /// A freshly formatted database, or None (with a message) when the
    /// project-local cluster is not running.
    pub fn new(tag: &str) -> Option<TestDb> {
        Self::with(
            tag,
            MkfsOptions {
                block_size: 4096,
                inline_max: 64,
                ..Default::default()
            },
        )
    }

    pub fn with(tag: &str, opts: MkfsOptions) -> Option<TestDb> {
        let admin = ConnSettings::new(&format!("{} dbname=postgres", cluster()));
        let mut c = match Conn::open(&admin) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "SKIP: project-local cluster not running ({e:#}); run scripts/ensure_db.sh"
                );
                return None;
            }
        };
        let name = format!(
            "pgfs_t_{tag}_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        c.batch(&format!("DROP DATABASE IF EXISTS {name}")).unwrap();
        c.batch(&format!("CREATE DATABASE {name}")).unwrap();
        let settings = ConnSettings::new(&format!("{} dbname={name}", cluster()));
        let mut fc = Conn::open(&settings).unwrap();
        schema::mkfs(&mut fc, &opts).expect("mkfs");
        Some(TestDb {
            name,
            settings,
            block_size: opts.block_size,
            inline_max: opts.inline_max,
        })
    }

    pub fn conn(&self) -> Conn {
        Conn::open(&self.settings).unwrap()
    }

    pub fn backend(&self) -> PgBackend {
        let mut b = PgBackend::new(self.settings.clone(), Arc::new(Health::new(1)), None);
        b.open().unwrap();
        b
    }

    /// A backend acting as worker `worker` of a read-write mount of `tree`.
    pub fn mount_backend(
        &self,
        tree: &str,
        workers: i32,
    ) -> (Vec<PgBackend>, uuid::Uuid, i64, Arc<Health>) {
        let mut c = self.conn();
        let r = c
            .query_sql(
                "SELECT mount_id, fence FROM pgfs.mount_acquire($1, NULL, false, 'test', 1, $2, 30)",
                &[&tree, &workers],
            )
            .unwrap();
        let (id, fence): (uuid::Uuid, i64) = (r[0].get(0), r[0].get(1));
        let health = Arc::new(Health::new(workers as usize));
        let bs = (0..workers)
            .map(|w| {
                let mut b = PgBackend::new(
                    self.settings.clone(),
                    health.clone(),
                    Some(MountIdentity {
                        mount_id: id,
                        fence,
                        worker: w,
                    }),
                );
                b.open().unwrap();
                b
            })
            .collect();
        (bs, id, fence, health)
    }

    pub fn layer(&self, tree: &str) -> i64 {
        self.conn()
            .query_sql("SELECT layer FROM pgfs.trees WHERE name = $1", &[&tree])
            .unwrap()[0]
            .get(0)
    }

    pub fn view(&self, tree: &str) -> View {
        let r = self
            .conn()
            .query_sql(
                "SELECT layer, root_ino FROM pgfs.trees WHERE name = $1",
                &[&tree],
            )
            .unwrap();
        View {
            layer: r[0].get(0),
            epoch: None,
            root: r[0].get::<_, i64>(1) as u64,
        }
    }

    pub fn snapshot_view(&self, tree: &str, name: &str) -> View {
        let r = self
            .conn()
            .query_sql(
                "SELECT layer, epoch, root_ino FROM pgfs.snapshots WHERE tree = $1 AND name = $2",
                &[&tree, &name],
            )
            .unwrap();
        View {
            layer: r[0].get(0),
            epoch: Some(r[0].get(1)),
            root: r[0].get::<_, i64>(2) as u64,
        }
    }

    /// `pgfs check` must find nothing (spec/verify.py CheckAfterEveryTest).
    pub fn assert_check(&self) {
        let mut c = self.conn();
        let v = pgfs::check::violations(&mut c, None).unwrap();
        assert!(v.is_empty(), "pgfs check found violations: {v:#?}");
    }

    pub fn sql(&self, q: &str) -> Vec<tokio_postgres::Row> {
        self.conn().query_sql(q, &[]).unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if std::thread::panicking() && std::env::var("PGFS_KEEP_FAILED_DB").is_ok() {
            eprintln!("kept database {}", self.name);
            return;
        }
        if let Ok(mut c) = Conn::open(&ConnSettings::new(&format!(
            "{} dbname=postgres",
            cluster()
        ))) {
            let _ = c.batch(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.name
            ));
        }
    }
}

pub fn now() -> i64 {
    pgfs::fs::now_ns()
}

/// Write through the same buffer-and-commit path as the daemon
/// (spec/optimize.py BufferedWriteFlushReq).
#[allow(clippy::too_many_arguments)]
pub fn write(
    be: &mut dyn PgfsBackend,
    v: &View,
    ino: u64,
    gen: Option<u64>,
    size: u64,
    offset: u64,
    data: &[u8],
    bs: u64,
    inline_max: u64,
) -> FsResult<Attr> {
    let mut buf = InodeBuffer::new(ino, gen, size, bs);
    buf.write(offset, data, 0, now());
    let plan = buf.plan(inline_max);
    let mut committed = BTreeMap::new();
    let mut inline = None;
    let mut first = plan.partial.first().copied();
    let mut last = plan.partial.last().copied();
    if plan.probe_inline {
        first = Some(0);
        last = Some(last.unwrap_or(0));
    }
    if let (Some(f), Some(l)) = (first, last) {
        let r = be.read_blocks(v, ino, gen, f, l)?;
        if r.inline {
            inline = Some(r.blocks.first().map(|(_, d)| d.clone()).unwrap_or_default());
        } else {
            committed.extend(r.blocks);
        }
    }
    let args = buf.build(inline_max, inline.as_deref(), &committed, now(), false);
    be.write_blocks(v.layer, &args)
}

/// Truncate through the daemon's tail rule (spec/app.py TruncatesFiles).
#[allow(clippy::too_many_arguments)]
pub fn truncate(
    be: &mut dyn PgfsBackend,
    v: &View,
    ino: u64,
    gen: Option<u64>,
    old: u64,
    new: u64,
    bs: u64,
    inline_max: u64,
) -> FsResult<Attr> {
    let mut tail = None;
    let mut done = false;
    if old <= inline_max {
        let r = be.read_blocks(v, ino, gen, 0, 0)?;
        if r.inline {
            done = true;
            if new > inline_max {
                tail = Some((
                    0,
                    r.blocks.first().map(|(_, d)| d.clone()).unwrap_or_default(),
                ));
            }
        }
    }
    if !done && new < old && !new.is_multiple_of(bs) {
        let b = new / bs;
        tail = be
            .read_blocks(v, ino, gen, b, b)?
            .blocks
            .into_iter()
            .find(|(x, _)| *x == b)
            .map(|(x, mut d)| {
                d.truncate((new % bs) as usize);
                (x, d)
            });
    }
    be.setattr(
        v.layer,
        &SetAttrArgs {
            ino,
            gen,
            size: Some(new),
            tail,
            now: now(),
            ..Default::default()
        },
    )
}

/// The whole content of a file.
pub fn read_all(be: &mut dyn PgfsBackend, v: &View, ino: u64, size: u64, bs: u64) -> Vec<u8> {
    let mut out = vec![0u8; size as usize];
    if size == 0 {
        return out;
    }
    let r = be.read_blocks(v, ino, None, 0, (size - 1) / bs).unwrap();
    for (b, d) in r.blocks {
        let off = (b * bs) as usize;
        let n = d.len().min(out.len().saturating_sub(off));
        out[off..off + n].copy_from_slice(&d[..n]);
    }
    out
}

pub fn mk(
    be: &mut dyn PgfsBackend,
    layer: i64,
    parent: u64,
    name: &[u8],
    kind: Kind,
) -> FsResult<Attr> {
    be.make(
        layer,
        &MakeArgs {
            parent,
            name: name.to_vec(),
            kind,
            mode: if kind == Kind::Dir { 0o755 } else { 0o644 },
            uid: 1000,
            gid: 1000,
            rdev: 0,
            target: (kind == Kind::Symlink).then(|| b"target".to_vec()),
            now: now(),
        },
    )
}

pub fn lookup(be: &mut dyn PgfsBackend, v: &View, path: &[&[u8]]) -> Option<Attr> {
    let mut ino = v.root;
    let mut a = be.getattr(v, ino, None).unwrap();
    for p in path {
        a = be.lookup(v, ino, p).unwrap();
        ino = a.as_ref()?.ino;
    }
    a
}

/// A canonical listing of a view: path, kind, size, nlink, mode, content
/// digest, xattrs and symlink target.
pub fn tree_listing(be: &mut dyn PgfsBackend, v: &View, bs: u64) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue = vec![(v.root, String::new())];
    while let Some((dir, path)) = queue.pop() {
        let entries = be.readdirplus(v, dir, None, 100_000).unwrap();
        for (name, a) in entries {
            let p = format!("{path}/{}", pgfs::names::display_bytes(&name));
            let content = match a.kind {
                Kind::File => format!(
                    "{:08x}",
                    crc32c::crc32c(&read_all(be, v, a.ino, a.size, bs))
                ),
                Kind::Symlink => pgfs::names::display_bytes(&be.readlink(v, a.ino).unwrap()),
                _ => String::new(),
            };
            let mut x: Vec<String> = be
                .listxattr(v, a.ino)
                .unwrap()
                .into_iter()
                .map(|n| {
                    let val = be.getxattr(v, a.ino, &n).unwrap().unwrap_or_default();
                    format!(
                        "{}={}",
                        pgfs::names::display_bytes(&n),
                        pgfs::names::display_bytes(&val)
                    )
                })
                .collect();
            x.sort();
            out.push(format!(
                "{p} {:?} size={} nlink={} mode={:o} {content} [{}]",
                a.kind,
                a.size,
                a.nlink,
                a.mode,
                x.join(",")
            ));
            if a.kind == Kind::Dir {
                queue.push((a.ino, p));
            }
        }
    }
    out.sort();
    out
}

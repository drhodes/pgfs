//! `pgfs mount`: everything a mounted daemon does besides serving
//! requests. Startup checks and the mount session (spec/app.py
//! ConnectsToPostgres, MountsCleanly), the lease renewer and self-fencing
//! (spec/consistency.py ExclusiveTreeLease, FencesStaleWriters), the
//! reconnector and revalidation (spec/failure.py ReconnectsWithBackoff,
//! RevalidatesAfterReconnect), housekeeping (spec/consistency.py
//! HousekeepingInSeparateTransactions), owner-drained snapshots
//! (spec/branch.py DrainsOwnerBeforeSnapshot), the in-flight watchdog
//! (spec/failure.py InFlightAgeWatchdog), signals and clean shutdown
//! (spec/app.py HandlesShutdownSignals, spec/durability.py
//! FlushesOnShutdown), and the health files (spec/app.py WritesHealthFiles).

use crate::backend::{PgfsBackend, View};
use crate::conn::{Conn, ConnSettings};
use crate::fs::{self, mono_ms, op_name, FsConfig, Peer, Shared};
use crate::health::{DbState, Health};
use crate::metrics;
use crate::names::ViewSpec;
use crate::pg::{MountIdentity, PgBackend};
use crate::replica::{PrimaryIdentity, Replica};
use crate::schema::{self, ServerInfo};
use crate::systemd;
use anyhow::{anyhow, bail, Context, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Options of `pgfs mount`.
#[derive(Debug, Clone)]
pub struct MountOptions {
    pub view: ViewSpec,
    pub mountpoint: PathBuf,
    pub read_only: bool,
    pub takeover: bool,
    pub workers: usize,
    pub lease: Duration,
    pub attr_ttl: Option<Duration>,
    pub buffer_per_file: usize,
    pub buffer_total: usize,
    pub stall_warn: Duration,
    pub stall_error: Duration,
    pub shutdown_timeout: Duration,
    pub allow_other: bool,
    /// Honour set-id bits (spec/security.py SetIdAndDevicesOptIn).
    pub suid: bool,
    /// Allow device nodes to be opened.
    pub dev: bool,
    pub accept_data_loss: bool,
    pub replicas: Vec<String>,
    pub replica_wait: Duration,
    pub auto_flatten_depth: i32,
    pub change_retention: Duration,
    pub consumer_expiry: Duration,
    pub conn: ConnSettings,
}

/// What every background thread needs to know about the mount.
pub struct MountCtx {
    pub opts: MountOptions,
    pub info: Mutex<ServerInfo>,
    pub mount_id: uuid::Uuid,
    pub fence: i64,
    pub layer: i64,
    pub epoch: Option<i64>,
    pub root: u64,
    pub tree: String,
    pub snapshot: Option<String>,
    pub read_only: bool,
    pub health: Arc<Health>,
    pub stopping: AtomicBool,
    pub started: Instant,
    pub replica: Option<Arc<Replica>>,
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for its length.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if r != 0 {
        return "unknown".into();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

pub fn mountpoint_hash(mountpoint: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    mountpoint.hash(&mut h);
    format!("{:016x}", h.finish())
}

struct FileGuard(String);

impl Drop for FileGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("could not remove {}: {e}", self.0);
            }
        }
    }
}

/// Block the signals the waiter thread handles, before any other thread
/// exists, so a signal during startup queues instead of killing the daemon.
pub fn block_signals() -> [i32; 5] {
    let signals = [
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGUSR1,
        libc::SIGUSR2,
    ];
    // SAFETY: plain libc calls on a zeroed sigset_t.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in &signals {
            libc::sigaddset(&mut set, *s);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            tracing::warn!("could not block signals; Ctrl+C will fall back to AutoUnmount");
        }
    }
    signals
}

/// The FUSE mount options. Every mount is `nosuid,nodev` unless `--suid`
/// or `--dev` lifts one; asking explicitly keeps a mount made by root from
/// honouring them by default (spec/security.py SetIdAndDevicesOptIn).
pub fn mount_options(
    fsname: String,
    allow_other: bool,
    read_only: bool,
    suid: bool,
    dev: bool,
) -> Vec<fuser::MountOption> {
    use fuser::MountOption as M;
    let mut o = vec![
        M::FSName(fsname),
        M::Subtype("pgfs".into()),
        M::DefaultPermissions,
        M::AutoUnmount,
    ];
    if allow_other {
        o.push(M::AllowOther);
    }
    if read_only {
        o.push(M::RO);
    }
    o.push(if suid { M::Suid } else { M::NoSuid });
    o.push(if dev { M::Dev } else { M::NoDev });
    o
}

/// `--suid` and `--dev` take effect only for a root mount; refuse them
/// otherwise, before anything is mounted.
pub fn check_privilege(suid: bool, dev: bool, euid: u32) -> Result<()> {
    if (suid || dev) && euid != 0 {
        let flags: Vec<&str> = [(suid, "--suid"), (dev, "--dev")]
            .iter()
            .filter(|f| f.0)
            .map(|f| f.1)
            .collect();
        bail!(
            "{} {} the daemon to run as root: FUSE honours set-id bits and device nodes only on mounts made by root",
            flags.join(" "),
            if flags.len() == 1 { "needs" } else { "need" }
        );
    }
    Ok(())
}

fn check_mountpoint(p: &std::path::Path) -> Result<()> {
    let md = std::fs::metadata(p).with_context(|| format!("mountpoint {}", p.display()))?;
    if !md.is_dir() {
        bail!("mountpoint {} is not a directory", p.display());
    }
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Ok(mi) = std::fs::read_to_string("/proc/self/mountinfo") {
        let c = canon.to_string_lossy();
        if mi.lines().any(|l| l.split(' ').nth(4) == Some(&*c)) {
            bail!("{} is already a mount point", p.display());
        }
    }
    if std::fs::read_dir(p)?.next().is_some() {
        bail!("mountpoint {} is not empty", p.display());
    }
    Ok(())
}

/// Acquire the mount session (or the tree's lease), waiting for an expired
/// lease with --takeover.
fn acquire(
    c: &mut Conn,
    o: &MountOptions,
    workers: usize,
) -> Result<(uuid::Uuid, i64, i64, Option<i64>, u64, bool)> {
    let host = hostname();
    let pid = std::process::id() as i32;
    let ro = o.read_only || o.view.snapshot.is_some();
    let lease = o.lease.as_secs_f64();
    let started = Instant::now();
    loop {
        let r = c.query_sql(
            "SELECT mount_id, fence, layer, epoch, root_ino, scratch FROM pgfs.mount_acquire($1, $2, $3, $4, $5, $6, $7)",
            &[&o.view.tree, &o.view.snapshot, &ro, &host, &pid, &(workers as i32), &lease],
        );
        match r {
            Ok(rows) => {
                let r = &rows[0];
                return Ok((
                    r.get(0),
                    r.get(1),
                    r.get(2),
                    r.get(3),
                    r.get::<_, i64>(4) as u64,
                    r.get(5),
                ));
            }
            Err(e) if e.sqlstate.as_deref() == Some("PF018") && o.takeover => {
                if started.elapsed() > o.lease * 4 + Duration::from_secs(60) {
                    bail!("--takeover gave up: {e}");
                }
                tracing::info!(
                    "waiting for the lease to expire (--takeover): {}",
                    e.message
                );
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(e) => return Err(schema::cli_err("acquire the mount session", e)),
        }
    }
}

/// Mount and serve until unmounted. Returns the process exit status.
pub fn run(o: MountOptions) -> Result<i32> {
    // SAFETY: geteuid cannot fail.
    check_privilege(o.suid, o.dev, unsafe { libc::geteuid() })?;
    let signals = block_signals();
    let ro = o.read_only || o.view.snapshot.is_some();
    let workers = if ro {
        o.workers.max(1)
    } else {
        o.workers.clamp(1, 64)
    };

    // ── Connect and check, before anything is mounted ─────────────────
    let mut c = Conn::open(&o.conn)
        .with_context(|| format!("connect to Postgres ({})", o.conn.redacted()))?;
    let info = schema::server_info(&mut c)?;
    schema::check_version(&info)?;
    schema::check_durability(&info, o.accept_data_loss, !o.replicas.is_empty())?;
    schema::check_schema(&info)?;
    if info.is_superuser || info.schema_owner.as_deref() == Some(info.current_user.as_str()) {
        tracing::warn!(
            "connected as {} ({}): use a runtime role with only the pgfs grants (pgfs mkfs --runtime-role)",
            info.current_user,
            if info.is_superuser { "a superuser" } else { "the schema owner" }
        );
    }
    if info.scratch() {
        if !o.replicas.is_empty() {
            bail!(
                "--replica is refused on a scratch filesystem: unlogged tables are not replicated"
            );
        }
        tracing::warn!(
            "scratch: contents are lost on a database crash or failover; fsync promises nothing"
        );
        let empty = c
            .query_sql("SELECT NOT EXISTS (SELECT 1 FROM pgfs.inodes)", &[])
            .map_err(|e| anyhow!("{e}"))?;
        if empty[0].get::<_, bool>(0) {
            c.query_sql("SELECT pgfs.scratch_reset()", &[])
                .map_err(|e| anyhow!("reset scratch filesystem: {e}"))?;
            tracing::warn!(
                "scratch filesystem was emptied by a crash or failover and has been reset"
            );
        }
    }
    let fs_uuid = info.fs_uuid().unwrap_or_default().to_string();
    tracing::info!(
        "server: PostgreSQL {} (system {}), fs {fs_uuid}, block size {}, inline max {}, async commit window {} ms, synchronous_standby_names = {:?}",
        info.version_num / 10000,
        info.system_identifier,
        info.block_size(),
        info.inline_max(),
        info.async_window_ms(),
        info.synchronous_standby_names
    );
    if info.capacity().is_none() {
        tracing::info!("no capacity set (pgfs tune --capacity): statfs reports used space plus a nominal 1 TiB free");
    }
    if info.synchronous_standby_names.is_empty() {
        tracing::info!("no synchronous standbys: an fsync survives the loss of the primary's disk only through backups");
    }
    check_mountpoint(&o.mountpoint)?;

    let (mount_id, fence, layer, epoch, mut root, scratch) = acquire(&mut c, &o, workers)?;
    let _ = scratch;
    if let Some(p) = &o.view.path {
        let rows = c
            .query_sql(
                "SELECT pgfs.resolve_dir($1, $2, $3, $4)",
                &[&layer, &epoch, &(root as i64), &p.as_bytes()],
            )
            .map_err(|e| schema::cli_err(&format!("resolve {p}"), e))?;
        root = rows[0].get::<_, i64>(0) as u64;
    }
    tracing::info!(
        "mount session {mount_id} fence {fence}: {} ({}), layer {layer}{}",
        o.view,
        if ro { "read-only" } else { "read-write" },
        epoch.map(|e| format!(" epoch {e}")).unwrap_or_default()
    );
    let health = Arc::new(Health::new(workers));
    let release_on_error = |c: &mut Conn| {
        let _ = c.query_sql("SELECT pgfs.mount_release($1, $2)", &[&mount_id, &fence]);
    };

    // Orphans of a previous holder (spec/space.py ReclaimsOrphans).
    if !ro {
        let mut n = 0;
        loop {
            match c.query_sql(
                "SELECT pgfs.reclaim_orphan($1, $2, $3)",
                &[&layer, &mount_id, &fence],
            ) {
                Ok(rows) if rows[0].get::<_, Option<i64>>(0).is_some() => n += 1,
                Ok(_) => break,
                Err(e) => {
                    tracing::warn!("orphan reclaim stopped: {e}");
                    break;
                }
            }
        }
        if n > 0 {
            tracing::info!("reclaimed {n} orphaned inode(s) of the previous mount");
        }
    }

    // Replica.
    let replica = if let Some(r) = o.replicas.first() {
        let mut s = o.conn.clone();
        s.conn = r.clone();
        let rep = Arc::new(Replica::new(
            s,
            PrimaryIdentity {
                system_identifier: info.system_identifier,
                timeline: info.timeline,
                fs_uuid: fs_uuid.clone(),
            },
        ));
        tracing::info!("replica mode: {}", rep.describe());
        Some(rep)
    } else {
        tracing::info!("replica mode: off (single primary)");
        None
    };

    // Workers, each with its own connection, opened now.
    let mut backends: Vec<Box<dyn PgfsBackend>> = Vec::new();
    let mut slots = Vec::new();
    for w in 0..workers {
        let id = (!ro).then_some(MountIdentity {
            mount_id,
            fence,
            worker: w as i32,
        });
        let mut b = PgBackend::new(o.conn.clone(), health.clone(), id);
        b.tree = o.view.tree.clone();
        if let Some(rep) = &replica {
            b = b.with_replica(rep.clone(), o.replica_wait);
        }
        if let Err(e) = b.open() {
            release_on_error(&mut c);
            return Err(e.context(format!("open worker {w}'s connection")));
        }
        slots.push(b.cancel_slot());
        backends.push(Box::new(b));
    }
    tracing::info!(
        "connections: {} on the primary ({workers} workers + renewer, housekeeping, listener){}",
        workers + 3,
        if replica.is_some() {
            format!(", {} on the standby", workers + 1)
        } else {
            String::new()
        }
    );

    let ttl = o.attr_ttl.unwrap_or(if o.view.snapshot.is_some() {
        Duration::from_secs(365 * 86400)
    } else if ro {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(3600)
    });
    let cfg = FsConfig {
        workers,
        ttl,
        read_only: ro,
        snapshot: o.view.snapshot.is_some(),
        buffer_per_file: o.buffer_per_file,
        buffer_total: o.buffer_total,
        block_size: info.block_size(),
        inline_max: info.inline_max(),
        capacity: info.capacity(),
    };
    let view = View { layer, epoch, root };
    let pgfs = fs::start(
        cfg,
        view,
        o.view.tree.clone(),
        health.clone(),
        backends,
        slots,
    );
    let sh = pgfs.shared();

    // Health files.
    let mp = o.mountpoint.to_string_lossy().to_string();
    let hash = mountpoint_hash(&mp);
    let pid_path = format!("/tmp/pgfs-{hash}.pid");
    let ready_path = format!("/tmp/pgfs-{hash}.ready");
    if let Err(e) = std::fs::write(&pid_path, std::process::id().to_string()) {
        tracing::warn!("could not write {pid_path}: {e}");
    }
    println!("health pid file: {pid_path}");
    tracing::info!("health: pid file written to {pid_path}");
    let _pid_guard = FileGuard(pid_path);
    let _ready_guard = FileGuard(ready_path.clone());

    // The FUSE session.
    let fsname = format!("pgfs-{}", &fs_uuid[..fs_uuid.len().min(8)]);
    let mut options = mount_options(fsname, o.allow_other, ro, o.suid, o.dev);
    if o.suid {
        tracing::warn!(
            "--suid: set-uid and set-gid programs on this mount run with their owner's privileges; \
             anyone who can write tree {} (the runtime role, the schema owner, pgfs import, a promoted branch) can gain root here",
            o.view.tree
        );
    }
    if o.dev {
        tracing::warn!(
            "--dev: device nodes on this mount can be opened; anyone who can write tree {} can reach this host's devices",
            o.view.tree
        );
    }
    println!("mounting pgfs {} at {}", o.view, o.mountpoint.display());
    let session = fuser::Session::new(fs::Dispatcher::new(sh.clone()), &o.mountpoint, &options).or_else(|e| {
        tracing::warn!("mount with AutoUnmount failed ({e}); mounting without it: a crashed daemon will leave a stale mount (add user_allow_other to /etc/fuse.conf)");
        options.retain(|x| !matches!(x, fuser::MountOption::AutoUnmount));
        fuser::Session::new(fs::Dispatcher::new(sh.clone()), &o.mountpoint, &options)
    });
    let mut session = match session {
        Ok(s) => s,
        Err(e) => {
            release_on_error(&mut c);
            pgfs.stop();
            return Err(anyhow!("mount at {}: {e}", o.mountpoint.display()));
        }
    };
    if let Ok(mut n) = sh.notifier.lock() {
        *n = Some(session.notifier());
    }
    let _ = std::fs::write(&ready_path, "ready");
    systemd::notify("READY=1\nSTATUS=serving");

    let ctx = Arc::new(MountCtx {
        info: Mutex::new(info),
        mount_id,
        fence,
        layer,
        epoch,
        root,
        tree: o.view.tree.clone(),
        snapshot: o.view.snapshot.clone(),
        read_only: ro,
        health: health.clone(),
        stopping: AtomicBool::new(false),
        started: Instant::now(),
        replica: replica.clone(),
        opts: o.clone(),
    });
    let outcome = Arc::new(Mutex::new(ShutdownOutcome::default()));

    metrics::start_periodic_export();
    spawn("renewer", {
        let (ctx, c) = (ctx.clone(), c);
        move || renewer(ctx, c)
    });
    spawn("reconnector", {
        let (ctx, sh) = (ctx.clone(), sh.clone());
        move || reconnector(ctx, sh)
    });
    spawn("watchdog", {
        let (ctx, sh) = (ctx.clone(), sh.clone());
        move || watchdog(ctx, sh)
    });
    if !ro {
        spawn("housekeeping", {
            let ctx = ctx.clone();
            move || housekeeping(ctx)
        });
        spawn("control", {
            let (ctx, sh) = (ctx.clone(), sh.clone());
            move || control_listener(ctx, sh)
        });
    }
    let unmounter = Arc::new(Mutex::new(session.unmount_callable()));
    spawn("signal-waiter", {
        let (ctx, sh, outcome, unmounter) =
            (ctx.clone(), sh.clone(), outcome.clone(), unmounter.clone());
        move || signal_waiter(signals, ctx, sh, outcome, unmounter)
    });

    let r = session.run();
    drop(session);
    ctx.stopping.store(true, Ordering::Release);
    systemd::notify("STOPPING=1");
    if let Err(e) = &r {
        tracing::error!("FUSE session ended with an error: {e}");
    }

    // The session is gone: if no signal ran the shutdown (an external
    // `fusermount -u`), commit what is left now.
    let mut out = outcome.lock().map(|g| g.clone()).unwrap_or_default();
    if !out.ran && !ro {
        out = flush_everything(&sh, &ctx);
    }
    pgfs.stop();
    // Release the session synchronously, after the unmount.
    match Conn::open(&ctx.opts.conn) {
        Ok(mut c2) => {
            if let Err(e) = c2.query_sql("SELECT pgfs.mount_release($1, $2)", &[&mount_id, &fence])
            {
                tracing::error!(
                    "could not release the mount session: {e}; it expires in {:?}",
                    ctx.opts.lease
                );
            } else {
                tracing::info!("mount session released");
            }
        }
        Err(e) => {
            tracing::error!("could not release the mount session: {e:#}; it expires on its own")
        }
    }
    tracing::info!("{}", state_dump(&ctx, &sh));
    if out.failures > 0 {
        tracing::error!(
            "{} inode buffer(s) could not be committed before unmount",
            out.failures
        );
        return Ok(1);
    }
    Ok(0)
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("spawn thread");
}

#[derive(Debug, Clone, Default)]
struct ShutdownOutcome {
    ran: bool,
    failures: usize,
}

/// Commit every buffer and run the synchronous barrier
/// (spec/durability.py FlushesOnShutdown).
fn flush_everything(sh: &Arc<Shared>, ctx: &MountCtx) -> ShutdownOutcome {
    let t = ctx.opts.shutdown_timeout;
    let answers = sh.broadcast(Peer::FlushAll, t);
    let mut failures: usize = answers.iter().map(|a| a.unwrap_or(1)).sum();
    if !ctx.read_only {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = sh.peers[0].send(Peer::Barrier(tx));
        match rx.recv_timeout(t) {
            Ok(Ok(())) => tracing::info!("shutdown: every buffer committed and flushed to WAL"),
            Ok(Err(e)) => {
                tracing::error!("shutdown: synchronous barrier failed: {e}");
                failures += 1;
            }
            Err(_) => {
                tracing::error!("shutdown: synchronous barrier timed out after {t:?}");
                failures += 1;
            }
        }
    }
    ShutdownOutcome {
        ran: true,
        failures,
    }
}

fn signal_waiter(
    signals: [i32; 5],
    ctx: Arc<MountCtx>,
    sh: Arc<Shared>,
    outcome: Arc<Mutex<ShutdownOutcome>>,
    unmounter: Arc<Mutex<fuser::SessionUnmounter>>,
) {
    // SAFETY: sigwait on the set blocked at startup.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        for s in &signals {
            libc::sigaddset(&mut set, *s);
        }
    }
    loop {
        let mut sig = 0;
        // SAFETY: set and sig are valid.
        if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
            tracing::error!("sigwait failed: {}", std::io::Error::last_os_error());
            return;
        }
        match sig {
            libc::SIGINT | libc::SIGTERM | libc::SIGHUP => {
                tracing::info!("received signal {sig}; committing buffers, then unmounting");
                systemd::notify("STOPPING=1\nSTATUS=committing buffers");
                let out = flush_everything(&sh, &ctx);
                if let Ok(mut g) = outcome.lock() {
                    *g = out;
                }
                let r = unmounter
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .unmount();
                if let Err(e) = r {
                    tracing::error!(
                        "clean unmount failed ({e}); exiting without releasing the lease (it expires on its own, AutoUnmount removes the mount)"
                    );
                    std::process::exit(1);
                }
                return;
            }
            libc::SIGUSR1 => tracing::info!("{}", state_dump(&ctx, &sh)),
            libc::SIGUSR2 => {
                crate::profiling::toggle();
            }
            _ => {}
        }
    }
}

/// The state dump, from atomics and try_lock only, written by whichever
/// thread asks (spec/app.py DumpsStateOnSigusr1).
pub fn state_dump(ctx: &MountCtx, sh: &Shared) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "state|mount={} view={}{}{} fence={} layer={} db={} uptime_secs={}\n",
        ctx.mount_id,
        ctx.tree,
        ctx.snapshot
            .as_ref()
            .map(|s| format!("@{s}"))
            .unwrap_or_default(),
        if ctx.read_only { " (read-only)" } else { "" },
        ctx.fence,
        ctx.layer,
        metrics::db_state_name(ctx.health.state() as u8),
        ctx.started.elapsed().as_secs()
    ));
    let now = mono_ms();
    for (i, s) in sh.inflight.iter().enumerate() {
        let started = s.started_ms.load(Ordering::Acquire);
        let pid = sh
            .backend_pids
            .get(i)
            .map(|p| p.load(Ordering::Relaxed))
            .unwrap_or(0);
        if started == 0 {
            out.push_str(&format!("state|worker={i} idle pid={pid}\n"));
        } else {
            out.push_str(&format!(
                "state|worker={i} op={} ino={} age_ms={} pid={pid}\n",
                op_name(s.op.load(Ordering::Relaxed)),
                s.ino.load(Ordering::Relaxed),
                now.saturating_sub(started)
            ));
        }
    }
    out.push_str(&format!(
        "state|cache={} handles={} buffered_bytes={}\n",
        sh.cache
            .try_len()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "locked".into()),
        sh.handles_open.load(Ordering::Relaxed),
        sh.buffered_total.load(Ordering::Relaxed)
    ));
    if let Some(r) = &ctx.replica {
        out.push_str(&format!("state|replica={}\n", r.describe()));
    }
    out.push_str(&format!(
        "state|fuse_latency={}\nstate|db_latency={}",
        metrics::FUSE_LATENCY.summary("fuse"),
        metrics::DB_LATENCY.summary("db")
    ));
    out
}

// ── Lease renewal ──────────────────────────────────────────────────────

fn renewer(ctx: Arc<MountCtx>, conn: Conn) {
    let lease = ctx.opts.lease;
    let period = lease / 3;
    let margin = Duration::from_secs(5);
    let mut last_ok = Instant::now();
    let pid = conn.backend_pid;
    let mut conn = Some(conn);
    let mut backoff = Duration::from_millis(100);
    while !ctx.stopping.load(Ordering::Acquire) {
        std::thread::sleep(
            period
                .min(Duration::from_secs(1))
                .max(Duration::from_millis(100)),
        );
        if last_ok.elapsed() < period && conn.is_some() {
            continue;
        }
        if ctx.health.state() == DbState::Fenced {
            return;
        }
        if conn.as_ref().is_none_or(|c| c.is_closed()) {
            conn = Conn::open(&ctx.opts.conn).ok();
        }
        let sent = Instant::now();
        let r = conn.as_mut().map(|c| {
            c.query_sql(
                "SELECT pgfs.mount_renew($1, $2, $3), pg_backend_pid()",
                &[&ctx.mount_id, &ctx.fence, &lease.as_secs_f64()],
            )
        });
        match r {
            Some(Ok(rows)) => {
                let version: String = rows[0].get(0);
                let backend: i32 = rows[0].get(1);
                last_ok = sent;
                backoff = Duration::from_millis(100);
                if version != schema::SCHEMA_VERSION.to_string() {
                    ctx.health.fence(&format!(
                        "the schema was migrated to version {version}; remount"
                    ));
                    return;
                }
                if backend != pid && conn.as_ref().is_some_and(|c| c.backend_pid == pid) {
                    ctx.health.fence("a connection pooler swapped backends mid-session; use session-mode connections");
                    return;
                }
            }
            Some(Err(e)) if e.sqlstate.as_deref() == Some("PF001") => {
                ctx.health.fence(&format!("the mount session is gone: {e}"));
                return;
            }
            other => {
                if let Some(Err(e)) = other {
                    tracing::warn!("lease renewal failed: {e}");
                }
                conn = None;
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(10));
            }
        }
        // Fail closed once the lease may have lapsed (spec/consistency.py FencesStaleWriters).
        if last_ok.elapsed() + margin > lease && ctx.health.state() == DbState::Connected {
            ctx.health.broken("lease renewal lapsed");
        }
    }
}

// ── Reconnect and revalidate ───────────────────────────────────────────

enum Reval {
    Retry(String),
    Fence(String),
}

fn revalidate(ctx: &MountCtx, sh: &Shared, c: &mut Conn) -> Result<(), Reval> {
    let info = schema::server_info(c).map_err(|e| Reval::Retry(format!("{e:#}")))?;
    let mut orig = ctx.info.lock().unwrap_or_else(|p| p.into_inner());
    if info.schema_version() != orig.schema_version() {
        return Err(Reval::Fence(format!(
            "schema version changed from {:?} to {:?}",
            orig.schema_version(),
            info.schema_version()
        )));
    }
    if info.fs_uuid() != orig.fs_uuid() {
        return Err(Reval::Fence(format!(
            "fs_uuid changed from {:?} to {:?}",
            orig.fs_uuid(),
            info.fs_uuid()
        )));
    }
    if info.system_identifier != orig.system_identifier {
        return Err(Reval::Fence(format!(
            "system identifier changed from {} to {}",
            orig.system_identifier, info.system_identifier
        )));
    }
    if info.timeline < orig.timeline {
        return Err(Reval::Fence(format!(
            "timeline went back from {} to {}",
            orig.timeline, info.timeline
        )));
    }
    if let Err(e) = schema::check_durability(
        &info,
        ctx.opts.accept_data_loss,
        !ctx.opts.replicas.is_empty(),
    ) {
        return Err(Reval::Fence(format!("{e:#}")));
    }
    let rows = c
        .query_sql(
            "SELECT schema_version, fs_uuid, session_ok, root_exists, snapshot_ok, slots FROM pgfs.revalidate($1, $2)",
            &[&ctx.mount_id, &ctx.fence],
        )
        .map_err(|e| Reval::Retry(e.to_string()))?;
    let r = &rows[0];
    let session_ok: bool = r.get(2);
    let root_exists: bool = r.get(3);
    let snapshot_ok: bool = r.get(4);
    let slots: serde_json::Value = r.get(5);
    if info.scratch() && !root_exists {
        return Err(Reval::Fence(
            "the scratch filesystem was emptied by a database crash".into(),
        ));
    }
    if !ctx.read_only {
        if !session_ok {
            return Err(Reval::Fence("this mount's lease row (mount_id, fence) is gone: another mount took over, or history was lost".into()));
        }
        for (w, last) in ctx.health.last_replied.iter().enumerate() {
            let last = last.load(Ordering::Acquire);
            let slot = slots.get(w.to_string()).and_then(|v| v.as_i64());
            match slot {
                Some(s) if s >= last => {}
                _ => {
                    return Err(Reval::Fence(format!(
                        "worker {w}'s op_log slot is at {slot:?} but op_seq {last} was acknowledged: committed history was lost"
                    )))
                }
            }
        }
    } else {
        if ctx.snapshot.is_some() && !snapshot_ok {
            return Err(Reval::Fence("the mounted snapshot no longer exists".into()));
        }
        if session_ok {
            let _ = c.query_sql(
                "SELECT pgfs.mount_renew($1, $2, $3)",
                &[&ctx.mount_id, &ctx.fence, &ctx.opts.lease.as_secs_f64()],
            );
        } else {
            return Err(Reval::Fence("the read-only mount session expired".into()));
        }
        if ctx.snapshot.is_none() {
            // History may have been rewound: forget versions and invalidate
            // every inode the kernel holds (spec/consistency.py ReadOnlyLiveMounts).
            sh.cache.invalidate_all();
            for ino in sh.cache.inos() {
                sh.invalidate_kernel(ino);
            }
        }
    }
    orig.timeline = info.timeline;
    Ok(())
}

fn reconnector(ctx: Arc<MountCtx>, sh: Arc<Shared>) {
    while !ctx.stopping.load(Ordering::Acquire) {
        let s = ctx.health.wait_not_connected(Duration::from_secs(1));
        if s == DbState::Connected || s == DbState::Fenced {
            continue;
        }
        ctx.health.reconnecting();
        metrics::RECONNECTS.fetch_add(1, Ordering::Relaxed);
        let mut backoff = Duration::from_millis(100);
        let mut last_warn: Option<Instant> = None;
        loop {
            if ctx.stopping.load(Ordering::Acquire) {
                return;
            }
            match Conn::open(&ctx.opts.conn) {
                Ok(mut c) => {
                    ctx.health.revalidating();
                    match revalidate(&ctx, &sh, &mut c) {
                        Ok(()) => {
                            ctx.health.connected();
                            break;
                        }
                        Err(Reval::Fence(why)) => {
                            ctx.health.fence(&why);
                            systemd::notify(&format!("STATUS=fenced: {why}"));
                            break;
                        }
                        Err(Reval::Retry(why)) => {
                            ctx.health.reconnecting();
                            tracing::warn!("revalidation could not run: {why}");
                        }
                    }
                }
                Err(e) => {
                    if last_warn.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                        tracing::warn!("reconnect failed: {e:#}");
                        last_warn = Some(Instant::now());
                    }
                }
            }
            // Exponential backoff with full jitter, 100 ms to 10 s.
            let jitter = (crc32c::crc32c(&mono_ms().to_le_bytes()) as u64)
                % (backoff.as_millis() as u64 + 1);
            std::thread::sleep(Duration::from_millis(jitter));
            backoff = (backoff * 2).min(Duration::from_secs(10));
        }
    }
}

// ── In-flight watchdog ─────────────────────────────────────────────────

fn watchdog(ctx: Arc<MountCtx>, sh: Arc<Shared>) {
    let warn = ctx.opts.stall_warn.as_millis() as u64;
    let error = ctx.opts.stall_error.as_millis() as u64;
    while !ctx.stopping.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_secs(1));
        let now = mono_ms();
        let mut oldest = 0;
        for (i, s) in sh.inflight.iter().enumerate() {
            let started = s.started_ms.load(Ordering::Acquire);
            if started == 0 {
                continue;
            }
            let age = now.saturating_sub(started);
            oldest = oldest.max(age);
            let op = op_name(s.op.load(Ordering::Relaxed));
            let ino = s.ino.load(Ordering::Relaxed);
            let pid = sh
                .cancel_slots
                .get(i)
                .map(|_| sh.backend_pids[i].load(Ordering::Relaxed))
                .unwrap_or(0);
            if age > warn && !s.warned.swap(true, Ordering::AcqRel) {
                tracing::warn!("worker {i}: {op} on inode {ino} running for {age} ms (threshold {warn} ms), backend pid {pid}");
            }
            if age > error && !s.cancelled.swap(true, Ordering::AcqRel) {
                tracing::error!("worker {i}: {op} on inode {ino} stuck for {age} ms (threshold {error} ms); cancelling its query");
                tracing::info!("{}", state_dump(&ctx, &sh));
                let canceller = sh
                    .cancel_slots
                    .get(i)
                    .and_then(|s| s.lock().ok().and_then(|g| g.clone()));
                if let Some(c) = canceller {
                    if let Err(e) = c.cancel() {
                        tracing::warn!("cancel failed: {e:#}");
                    }
                }
            }
        }
        metrics::OLDEST_INFLIGHT_MS.store(oldest, Ordering::Relaxed);
        if oldest <= warn && systemd::enabled() {
            systemd::notify("WATCHDOG=1");
        }
    }
}

// ── Housekeeping ───────────────────────────────────────────────────────

fn housekeeping(ctx: Arc<MountCtx>) {
    let mut settings = ctx.opts.conn.clone();
    settings.statement_timeout = Duration::from_secs(60);
    settings.async_commit = true;
    let mut conn: Option<Conn> = None;
    let mut last_seq: i64 = -1;
    let mut t_fold = Instant::now();
    let mut t_reclaim = Instant::now();
    let mut t_minute = Instant::now() - Duration::from_secs(55);
    let mut flatten_backoff = Duration::from_secs(1);
    let mut t_flatten = Instant::now();
    let mut reclaim_busy = false;
    while !ctx.stopping.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(75));
        if ctx.health.state() != DbState::Connected {
            conn = None;
            continue;
        }
        if conn.as_ref().is_none_or(|c| c.is_closed()) {
            conn = Conn::open(&settings).ok();
        }
        let Some(c) = conn.as_mut() else { continue };

        // Coalesced change notifications (spec/changefeed.py ChangeNotifications).
        if let Ok(rows) = c.query_sql("SELECT last_value FROM pgfs.change_seq", &[]) {
            let seq: i64 = rows[0].get(0);
            if seq != last_seq {
                if last_seq >= 0 {
                    let _ = c.batch(&format!("NOTIFY pgfs_changes, '{}'", ctx.layer));
                }
                last_seq = seq;
            }
        }
        let mut step = |name: &str, sql: &str| -> Option<i64> {
            match c.query_sql(sql, &[]) {
                Ok(rows) => rows.first().and_then(|r| {
                    r.try_get::<_, i64>(0)
                        .ok()
                        .or_else(|| r.try_get::<_, i32>(0).ok().map(i64::from))
                }),
                Err(e) => {
                    tracing::warn!("housekeeping {name}: {e}");
                    None
                }
            }
        };
        if t_fold.elapsed() >= Duration::from_secs(10) {
            t_fold = Instant::now();
            step("fold usage", "SELECT pgfs.fold_usage()");
            step("reap sessions", "SELECT pgfs.reap_sessions()");
        }
        if t_reclaim.elapsed()
            >= if reclaim_busy {
                Duration::from_millis(200)
            } else {
                Duration::from_secs(2)
            }
        {
            t_reclaim = Instant::now();
            let n = step("reclaim", "SELECT pgfs.reclaim_batch(10000)").unwrap_or(0);
            let g = step("gc", "SELECT pgfs.gc_batch(1000)").unwrap_or(0);
            reclaim_busy = n > 0 || g > 0;
        }
        if t_minute.elapsed() >= Duration::from_secs(60) {
            t_minute = Instant::now();
            match c.query_sql(
                "SELECT pgfs.prune_changes($1, $2)",
                &[
                    &ctx.opts.change_retention.as_secs_f64(),
                    &ctx.opts.consumer_expiry.as_secs_f64(),
                ],
            ) {
                Ok(rows) => {
                    for r in rows {
                        let name: String = r.get(0);
                        tracing::warn!("change consumer {name} was idle past --consumer-expiry and is unregistered");
                    }
                }
                Err(e) => tracing::warn!("housekeeping prune changes: {e}"),
            }
            if let Ok(line) = space_line(c, ctx.layer) {
                if let Ok(mut g) = metrics::SPACE_LINE.lock() {
                    *g = line;
                }
            }
        }
        // Background auto-flatten (spec/branch.py LimitsBranchDepth).
        if t_flatten.elapsed() >= flatten_backoff {
            t_flatten = Instant::now();
            match c.query_sql(
                "SELECT t FROM pgfs.deep_trees($1) t",
                &[&ctx.opts.auto_flatten_depth],
            ) {
                Ok(rows) => {
                    let trees: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
                    for t in trees.into_iter().filter(|t| *t == ctx.tree) {
                        let r = c
                            .query_sql("SELECT pgfs.flatten_batch($1, 10000)", &[&t])
                            .and_then(|rows| {
                                let n: i64 = rows[0].get(0);
                                if n == 0 {
                                    c.query_sql("SELECT pgfs.flatten_finish($1)", &[&t])
                                        .map(|_| 0)
                                } else {
                                    Ok(n)
                                }
                            });
                        match r {
                            Ok(_) => flatten_backoff = Duration::from_secs(1),
                            Err(e) => {
                                flatten_backoff =
                                    (flatten_backoff * 2).min(Duration::from_secs(300));
                                tracing::warn!("auto-flatten of {t} failed ({e}); retrying in {flatten_backoff:?}");
                            }
                        }
                    }
                }
                Err(e) => tracing::debug!("deep trees: {e}"),
            }
        }
    }
}

/// The `metrics|space|…` line (spec/space.py ReportsSpace).
pub fn space_line(c: &mut Conn, layer: i64) -> Result<String> {
    let rows = c
        .query_sql(
            "SELECT (SELECT logical_bytes FROM pgfs.layer_usage($1)),
                    (SELECT physical_bytes FROM pgfs.fs_usage()),
                    (SELECT COALESCE(sum(octet_length(inline_data)), 0)::bigint FROM pgfs.inodes WHERE layer = $1 AND died = 9223372036854775807),
                    (SELECT COALESCE(sum(b.len), 0)::bigint FROM pgfs.file_blocks f JOIN pgfs.blobs b ON b.blob_id = f.blob_id
                       WHERE f.died <> 9223372036854775807 AND NOT EXISTS (SELECT 1 FROM pgfs.file_blocks g WHERE g.blob_id = f.blob_id AND g.died = 9223372036854775807)),
                    (SELECT retired_rows FROM pgfs.backlog()), (SELECT gc_queue FROM pgfs.backlog()),
                    (SELECT COALESCE(sum(n_dead_tup), 0)::bigint FROM pg_stat_user_tables WHERE schemaname = 'pgfs' AND relname IN ('blobs', 'file_blocks', 'inodes')),
                    (SELECT wal_bytes::bigint FROM pg_stat_wal)",
            &[&layer],
        )
        .map_err(|e| anyhow!("{e}"))?;
    let r = &rows[0];
    let wal: i64 = r.get(7);
    let user = metrics::USER_BYTES_WRITTEN.load(Ordering::Relaxed);
    static START_WAL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let start = *START_WAL.get_or_init(|| wal);
    let amp = if user > 0 {
        (wal - start) as f64 / user as f64
    } else {
        0.0
    };
    Ok(format!(
        "live_bytes={} physical_bytes={} inline_bytes={} snapshot_only_bytes={} reclaim_backlog_rows={} gc_queue={} dead_tuples={} wal_per_user_byte={amp:.2}",
        r.get::<_, i64>(0),
        r.get::<_, i64>(1),
        r.get::<_, i64>(2),
        r.get::<_, i64>(3),
        r.get::<_, i64>(4),
        r.get::<_, i64>(5),
        r.get::<_, i64>(6),
    ))
}

// ── Owner-drained snapshots and branches ───────────────────────────────

fn control_listener(ctx: Arc<MountCtx>, sh: Arc<Shared>) {
    let mut conn: Option<Conn> = None;
    while !ctx.stopping.load(Ordering::Acquire) {
        if ctx.health.state() != DbState::Connected {
            conn = None;
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        if conn.as_ref().is_none_or(|c| c.is_closed()) {
            conn = Conn::open(&ctx.opts.conn)
                .ok()
                .and_then(|mut c| c.batch("LISTEN pgfs_control").ok().map(|_| c));
            if conn.is_none() {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }
        let c = conn.as_mut().expect("opened");
        let _ = c.wait_notification(Duration::from_secs(5));
        let pending = match c.query_sql("SELECT r FROM pgfs.control_pending($1) r", &[&ctx.tree]) {
            Ok(rows) => rows.iter().map(|r| r.get::<_, i64>(0)).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!("control requests: {e}");
                conn = None;
                continue;
            }
        };
        for id in pending {
            // Drain barrier: every worker commits and holds new mutations.
            let t0 = Instant::now();
            let answers = sh.broadcast(Peer::Hold, Duration::from_secs(30));
            let failed: usize = answers.iter().map(|a| a.unwrap_or(1)).sum();
            if failed > 0 {
                tracing::error!("control request {id}: {failed} buffer(s) could not be committed; the snapshot proceeds without them");
            }
            let r = c.query_sql(
                "SELECT pgfs.control_execute($1, $2, $3)",
                &[&id, &ctx.mount_id, &ctx.fence],
            );
            sh.release_all();
            match r {
                Ok(rows) => {
                    let v: Option<serde_json::Value> = rows[0].get(0);
                    tracing::info!(
                        "control request {id} executed in {:?}: {}",
                        t0.elapsed(),
                        v.map(|v| v.to_string())
                            .unwrap_or_else(|| "cancelled".into())
                    );
                }
                Err(e) => tracing::error!("control request {id}: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_are_nosuid_and_nodev_unless_asked() {
        // spec/security.py SetIdAndDevicesOptIn.
        use fuser::MountOption as M;
        let o = mount_options("pgfs-x".into(), false, false, false, false);
        assert!(o.contains(&M::NoSuid) && o.contains(&M::NoDev));
        assert!(!o.contains(&M::Suid) && !o.contains(&M::Dev));
        assert!(o.contains(&M::DefaultPermissions) && o.contains(&M::AutoUnmount));
        let o = mount_options("pgfs-x".into(), true, true, true, false);
        assert!(o.contains(&M::Suid) && o.contains(&M::NoDev) && !o.contains(&M::NoSuid));
        assert!(o.contains(&M::AllowOther) && o.contains(&M::RO));
        let o = mount_options("pgfs-x".into(), false, false, false, true);
        assert!(o.contains(&M::Dev) && o.contains(&M::NoSuid) && !o.contains(&M::NoDev));
    }

    #[test]
    fn suid_and_dev_need_root() {
        assert!(check_privilege(false, false, 1000).is_ok());
        assert!(check_privilege(true, false, 0).is_ok());
        assert!(check_privilege(false, true, 0).is_ok());
        let e = check_privilege(true, false, 1000).unwrap_err().to_string();
        assert!(e.contains("--suid") && e.contains("root"), "{e}");
        let e = check_privilege(true, true, 1000).unwrap_err().to_string();
        assert!(e.contains("--suid --dev"), "{e}");
    }

    #[test]
    fn mountpoint_hash_is_16_hex_digits() {
        let h = mountpoint_hash("/tmp/x");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, mountpoint_hash("/tmp/x"));
    }

    #[test]
    fn mountpoints_must_be_empty_directories() {
        let d = std::env::temp_dir().join(format!("pgfs-mp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        assert!(check_mountpoint(&d).is_ok());
        std::fs::write(d.join("x"), "x").unwrap();
        assert!(check_mountpoint(&d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

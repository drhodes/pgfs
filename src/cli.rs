//! The `pgfs` command (spec/ops.py AdminCli). Exit status: 0 on success,
//! 1 when the command ran and found a problem, 2 when it could not run.

use crate::conn::{Conn, ConnSettings, DbError};
use crate::mount::MountOptions;
use crate::names::{check_tree_name, ViewSpec};
use crate::schema::{self, cli_err};
use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "pgfs",
    version,
    about = "A POSIX filesystem stored in PostgreSQL"
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Args, Debug, Clone)]
pub struct Global {
    /// libpq connection string. Passwords come from PGPASSFILE, PGPASSWORD
    /// or --password-file, never from the command line.
    #[arg(long, env = "PGFS_CONN", global = true)]
    pub conn: Option<String>,
    /// File holding the database password.
    #[arg(long, global = true)]
    pub password_file: Option<PathBuf>,
    /// Allow a connection to a remote host without sslmode=verify-full.
    #[arg(long, global = true)]
    pub insecure_transport: bool,
    /// Print data as JSON.
    #[arg(long, global = true)]
    pub json: bool,
    /// Print the full error chain.
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Format a database (spec/ops.py MakesFilesystem).
    Mkfs(MkfsArgs),
    /// Upgrade the schema (spec/ops.py MigratesSchema).
    Migrate {
        /// Print the steps without running them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Change capacity, identity or the runtime role (spec/ops.py TunesFilesystem).
    Tune(TuneArgs),
    /// Mount a tree, snapshot or subdirectory view.
    Mount(MountArgs),
    /// List, delete and flatten trees.
    Tree {
        #[command(subcommand)]
        cmd: TreeCmd,
    },
    /// Create (TREE@NAME), list or delete snapshots.
    Snapshot(SnapshotArgs),
    /// Create a writable tree from a tree or snapshot.
    Branch(BranchArgs),
    /// Make a branch the new contents of a tree, atomically.
    Promote {
        /// Replace the tree even if it changed since the branch point.
        #[arg(long)]
        force: bool,
        branch: String,
        tree: String,
    },
    /// Print or follow the change feed of a tree.
    Changes(ChangesArgs),
    /// List what changed between two views of one tree.
    Diff { from: String, to: String },
    /// Verify the stored state (spec/integrity.py PgfsCheck).
    Check {
        tree: Option<String>,
        /// Fix queued blobs and usage totals only.
        #[arg(long)]
        repair: bool,
    },
    /// Read every blob and verify its checksum (spec/integrity.py Scrub).
    Scrub {
        /// Rate limit in MiB/s.
        #[arg(long, default_value_t = 50)]
        rate: u64,
        /// Keep scrubbing in the background, one pass per --scrub-period.
        #[arg(long)]
        continuous: bool,
        /// Days per pass with --continuous.
        #[arg(long, default_value_t = 30)]
        scrub_period: u64,
    },
    /// Write a snapshot as a pax tar stream to standard output.
    Export {
        /// TREE@SNAPSHOT
        snapshot: String,
        /// Export only this directory.
        #[arg(long)]
        path: Option<String>,
    },
    /// Read a tar stream from standard input into TREE[:/DIR].
    Import { target: String },
    /// Usage, backlog, amplification and the durability window.
    Stats { tree: Option<String> },
    /// Print the reference table of every option (spec/ops.py DocumentsEveryOption).
    Options,
}

#[derive(Args, Debug)]
pub struct MkfsArgs {
    /// Bytes per data block: a power of two from 4096 to 1048576.
    #[arg(long, default_value_t = 65536)]
    pub block_size: u64,
    /// Largest file stored inside its inode, at most 2048.
    #[arg(long, default_value_t = 2048)]
    pub inline_max: u64,
    /// Reported capacity in bytes; writes past it reply ENOSPC.
    #[arg(long)]
    pub capacity: Option<u64>,
    /// Unlogged data tables: fast, and lost on a database crash.
    #[arg(long)]
    pub scratch: bool,
    /// Grant the runtime privileges to this role.
    #[arg(long)]
    pub runtime_role: Option<String>,
    /// Replace an existing pgfs schema, destroying the filesystem.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct TuneArgs {
    /// Set the capacity in bytes ("none" removes it).
    #[arg(long)]
    pub capacity: Option<String>,
    /// Give the filesystem a new fs_uuid (for restored copies).
    #[arg(long)]
    pub new_uuid: bool,
    /// Grant the runtime privileges to this role.
    #[arg(long)]
    pub runtime_role: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct MountArgs {
    /// TREE[@SNAPSHOT][:/PATH]
    pub view: String,
    pub mountpoint: PathBuf,
    /// Mount a live tree read-only.
    #[arg(long)]
    pub read_only: bool,
    /// Wait for an expired read-write lease and take it over.
    #[arg(long)]
    pub takeover: bool,
    /// Dispatch workers (default: CPUs, at most 16).
    #[arg(long)]
    pub workers: Option<usize>,
    /// Lease period in seconds.
    #[arg(long, default_value_t = 30)]
    pub lease: u64,
    /// Entry and attribute TTL in seconds (default 3600 read-write, 1 read-only).
    #[arg(long)]
    pub attr_ttl: Option<f64>,
    /// Commit a file's buffer at this many bytes.
    #[arg(long, default_value_t = 8 << 20)]
    pub buffer_per_file: usize,
    /// Commit the oldest buffers past this many bytes in total.
    #[arg(long, default_value_t = 256 << 20)]
    pub buffer_total: usize,
    /// Warn about a request older than this many seconds.
    #[arg(long, default_value_t = 5)]
    pub stall_warn: u64,
    /// Cancel a request older than this many seconds.
    #[arg(long, default_value_t = 60)]
    pub stall_error: u64,
    /// Seconds to wait for the database at shutdown.
    #[arg(long, default_value_t = 30)]
    pub shutdown_timeout: u64,
    /// Let other users use the mount (needs user_allow_other in /etc/fuse.conf).
    #[arg(long)]
    pub allow_other: bool,
    /// Honour set-uid and set-gid bits. Root only; anyone who can write the tree can gain root.
    #[arg(long)]
    pub suid: bool,
    /// Allow device nodes to be opened. Root only; anyone who can write the tree can reach this host's devices.
    #[arg(long)]
    pub dev: bool,
    /// Run although the server cannot keep data durable.
    #[arg(long)]
    pub i_accept_data_loss: bool,
    /// libpq connection string of a physical streaming standby for reads.
    #[arg(long)]
    pub replica: Vec<String>,
    /// Milliseconds to wait for the standby to reach a read ticket.
    #[arg(long, default_value_t = 20)]
    pub replica_wait: u64,
    /// Flatten this mount's tree in the background above this branch depth.
    #[arg(long, default_value_t = 4)]
    pub auto_flatten_depth: i32,
    /// Keep change rows at least this many seconds.
    #[arg(long, default_value_t = 7 * 86400)]
    pub change_retention: u64,
    /// Unregister change consumers idle this many seconds.
    #[arg(long, default_value_t = 30 * 86400)]
    pub consumer_expiry: u64,
    /// statement_timeout for operations, in seconds.
    #[arg(long, default_value_t = 30)]
    pub statement_timeout: u64,
    /// lock_timeout, in seconds.
    #[arg(long, default_value_t = 5)]
    pub lock_timeout: u64,
    /// connect_timeout, in seconds.
    #[arg(long, default_value_t = 5)]
    pub connect_timeout: u64,
}

#[derive(Subcommand, Debug)]
pub enum TreeCmd {
    /// List trees.
    List,
    /// Delete a tree; space comes back in the background.
    Delete { tree: String },
    /// Give a branch its own copy of every inherited row, then detach it.
    Flatten { tree: String },
}

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    /// TREE@NAME to create, or `list [TREE]`, or `delete TREE@NAME`.
    pub args: Vec<String>,
    /// Do not ask the owning daemon to drain first.
    #[arg(long)]
    pub crash_consistent: bool,
    /// Seconds to wait for the owning daemon.
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,
    /// Compute and store the snapshot's digest.
    #[arg(long)]
    pub seal: bool,
}

#[derive(Args, Debug)]
pub struct BranchArgs {
    /// SOURCE: a tree or TREE@SNAPSHOT, optionally :/PATH.
    pub source: String,
    pub new: String,
    /// Seconds to wait for the owning daemon of a live source.
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,
}

#[derive(Args, Debug)]
pub struct ChangesArgs {
    /// TREE, or `register NAME TREE`, or `unregister NAME`.
    pub args: Vec<String>,
    /// Resume from a printed watermark token.
    #[arg(long)]
    pub since: Option<String>,
    /// Keep printing as changes arrive.
    #[arg(long)]
    pub follow: bool,
    /// Read as this registered consumer and acknowledge each page.
    #[arg(long)]
    pub consumer: Option<String>,
    /// With `register`: start at this snapshot (TREE@NAME).
    #[arg(long)]
    pub at_snapshot: Option<String>,
    /// Poll interval in seconds with --follow.
    #[arg(long, default_value_t = 5)]
    pub poll: u64,
    /// Rows per page.
    #[arg(long, default_value_t = 1000)]
    pub limit: i32,
}

/// Run the CLI; returns the exit status.
pub fn main() -> i32 {
    crate::init_tracing_and_panics();
    let cli = Cli::parse();
    let verbose = cli.global.verbose;
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            if verbose {
                eprintln!("pgfs: {e:#}");
            } else {
                eprintln!("pgfs: {e}");
                if e.chain().count() > 1 {
                    eprintln!("pgfs: {:#}", e.root_cause());
                }
            }
            2
        }
    }
}

fn settings(g: &Global) -> Result<ConnSettings> {
    let conn = g.conn.clone().unwrap_or_else(crate::conn::default_conn);
    let from_argv = std::env::args().any(|a| a.starts_with("--conn"));
    if from_argv && crate::redact::has_password(&conn) {
        bail!("passwords are not accepted on the command line; use PGPASSFILE, PGPASSWORD, a service file or --password-file");
    }
    let mut s = ConnSettings::new(&conn);
    s.password_file = g.password_file.clone();
    s.insecure_transport = g.insecure_transport;
    if g.insecure_transport {
        tracing::warn!("--insecure-transport: the connection is not protected by verified TLS");
    }
    Ok(s)
}

fn connect(g: &Global) -> Result<Conn> {
    let s = settings(g)?;
    Conn::open(&s).with_context(|| format!("connect to Postgres ({})", s.redacted()))
}

/// Connect and check that the database holds a filesystem this binary knows.
fn connect_fs(g: &Global) -> Result<Conn> {
    let mut c = connect(g)?;
    let info = schema::server_info(&mut c)?;
    schema::check_version(&info)?;
    schema::check_schema(&info)?;
    Ok(c)
}

fn q(
    c: &mut Conn,
    what: &str,
    sql: &str,
    params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
) -> Result<Vec<tokio_postgres::Row>> {
    c.query_sql(sql, params).map_err(|e| cli_err(what, e))
}

/// Run a synchronous CLI commit's confirmation: if the standby wait was
/// not confirmed, run the CLI barrier.
fn confirm_sync(c: &mut Conn, what: &str) -> Result<()> {
    if c.take_unconfirmed_sync() {
        tracing::warn!("{what}: synchronous replication not confirmed; running the barrier");
        q(c, "synchronous barrier", "SELECT pgfs.cli_barrier()", &[])?;
        if c.take_unconfirmed_sync() {
            bail!("{what}: committed, but synchronous standbys did not confirm it");
        }
    }
    Ok(())
}

fn notify_changes(c: &mut Conn, tree: &str) {
    let _ = c.query_sql(
        "SELECT pg_notify('pgfs_changes', (SELECT layer::text FROM pgfs.trees WHERE name = $1))",
        &[&tree],
    );
}

pub fn run(cli: Cli) -> Result<i32> {
    let g = cli.global.clone();
    match cli.cmd {
        Cmd::Mkfs(a) => {
            let mut c = connect(&g)?;
            if a.force && atty_stdin() {
                eprint!(
                    "pgfs mkfs --force destroys the existing filesystem. Type 'yes' to continue: "
                );
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if line.trim() != "yes" {
                    bail!("not confirmed");
                }
            }
            let opts = schema::MkfsOptions {
                block_size: a.block_size,
                inline_max: a.inline_max,
                capacity: a.capacity,
                scratch: a.scratch,
                runtime_role: a.runtime_role,
                force: a.force,
                ..Default::default()
            };
            let id = schema::mkfs(&mut c, &opts)?;
            println!(
                "formatted pgfs filesystem {id} (block size {}, inline max {}{})",
                opts.block_size,
                opts.inline_max,
                if opts.scratch { ", scratch" } else { "" }
            );
            Ok(0)
        }
        Cmd::Migrate { dry_run } => {
            let mut c = connect(&g)?;
            if !dry_run {
                eprintln!("pgfs migrate: there are no downgrades; take a backup first (pg_basebackup or pgBackRest)");
            }
            let steps = schema::migrate(&mut c, dry_run)?;
            if steps.is_empty() {
                println!(
                    "schema is at version {}; nothing to do",
                    schema::SCHEMA_VERSION
                );
            }
            for s in steps {
                println!("{}{s}", if dry_run { "would run " } else { "ran " });
            }
            Ok(0)
        }
        Cmd::Tune(a) => {
            let mut c = connect(&g)?;
            let capacity = match a.capacity.as_deref() {
                None => None,
                Some("none") => Some(None),
                Some(v) => {
                    Some(Some(v.parse().map_err(|_| {
                        anyhow!("--capacity takes bytes or \"none\"")
                    })?))
                }
            };
            for line in schema::tune(
                &mut c,
                &schema::TuneOptions {
                    capacity,
                    new_uuid: a.new_uuid,
                    runtime_role: a.runtime_role,
                },
            )? {
                println!("{line}");
            }
            Ok(0)
        }
        Cmd::Mount(a) => mount(&g, a),
        Cmd::Tree { cmd } => tree(&g, cmd),
        Cmd::Snapshot(a) => snapshot(&g, a),
        Cmd::Branch(a) => branch(&g, a),
        Cmd::Promote {
            force,
            branch,
            tree,
        } => {
            let mut c = connect_fs(&g)?;
            let rows = q(
                &mut c,
                "promote",
                "SELECT rows_folded, snapshot FROM pgfs.promote($1, $2, $3)",
                &[&branch, &tree, &force],
            )?;
            confirm_sync(&mut c, "promote")?;
            notify_changes(&mut c, &tree);
            let n: i64 = rows[0].get(0);
            let s: String = rows[0].get(1);
            if force {
                println!("{tree} now points at {branch}'s layer; previous contents are {tree}@{s}");
            } else {
                println!(
                    "folded {n} rows of {branch} into {tree}; previous contents are {tree}@{s}"
                );
            }
            Ok(0)
        }
        Cmd::Changes(a) => crate::changes::run(&g, connect_fs(&g)?, a),
        Cmd::Diff { from, to } => diff(&g, &from, &to),
        Cmd::Check { tree, repair } => {
            crate::check::run(&mut connect_fs(&g)?, tree.as_deref(), repair, g.json)
        }
        Cmd::Scrub {
            rate,
            continuous,
            scrub_period,
        } => {
            if !continuous {
                return crate::check::scrub(&mut connect_fs(&g)?, rate);
            }
            // One pass per period, resuming after interruptions.
            let period = Duration::from_secs(scrub_period.max(1) * 86400);
            loop {
                let started = Instant::now();
                let code = crate::check::scrub(&mut connect_fs(&g)?, rate)?;
                if code != 0 {
                    tracing::error!("scrub pass found checksum failures; continuing");
                }
                std::thread::sleep(period.saturating_sub(started.elapsed()));
            }
        }
        Cmd::Export { snapshot, path } => {
            let v = ViewSpec::parse(&snapshot).map_err(|e| anyhow!(e))?;
            let Some(snap) = v.snapshot.clone() else {
                bail!("export needs TREE@SNAPSHOT")
            };
            let s = settings(&g)?;
            crate::archive::export(
                &s,
                &v.tree,
                &snap,
                path.as_deref().or(v.path.as_deref()),
                std::io::stdout().lock(),
            )?;
            Ok(0)
        }
        Cmd::Import { target } => {
            let v = ViewSpec::parse(&target).map_err(|e| anyhow!(e))?;
            if v.snapshot.is_some() {
                bail!("cannot import into a snapshot");
            }
            let s = settings(&g)?;
            let n =
                crate::archive::import(&s, &v.tree, v.path.as_deref(), std::io::stdin().lock())?;
            let mut c = connect_fs(&g)?;
            notify_changes(&mut c, &v.tree);
            println!("imported {n} entries into {target}");
            Ok(0)
        }
        Cmd::Stats { tree } => stats(&g, tree.as_deref()),
        Cmd::Options => {
            print!("{}", options_reference());
            Ok(0)
        }
    }
}

fn atty_stdin() -> bool {
    // SAFETY: isatty on fd 0.
    unsafe { libc::isatty(0) == 1 }
}

fn mount(g: &Global, a: MountArgs) -> Result<i32> {
    let view = ViewSpec::parse(&a.view).map_err(|e| anyhow!(e))?;
    let mut s = settings(g)?;
    s.async_commit = true;
    s.statement_timeout = Duration::from_secs(a.statement_timeout);
    s.lock_timeout = Duration::from_secs(a.lock_timeout);
    s.connect_timeout = Duration::from_secs(a.connect_timeout);
    s.application_name = format!("pgfs mount {view}");
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let o = MountOptions {
        view,
        mountpoint: a.mountpoint,
        read_only: a.read_only,
        takeover: a.takeover,
        workers: a.workers.unwrap_or(cpus.min(16)),
        lease: Duration::from_secs(a.lease.max(6)),
        attr_ttl: a.attr_ttl.map(Duration::from_secs_f64),
        buffer_per_file: a.buffer_per_file,
        buffer_total: a.buffer_total,
        stall_warn: Duration::from_secs(a.stall_warn),
        stall_error: Duration::from_secs(a.stall_error),
        shutdown_timeout: Duration::from_secs(a.shutdown_timeout),
        allow_other: a.allow_other,
        suid: a.suid,
        dev: a.dev,
        accept_data_loss: a.i_accept_data_loss,
        replicas: a.replica,
        replica_wait: Duration::from_millis(a.replica_wait),
        auto_flatten_depth: a.auto_flatten_depth,
        change_retention: Duration::from_secs(a.change_retention),
        consumer_expiry: Duration::from_secs(a.consumer_expiry),
        conn: s,
    };
    if o.accept_data_loss {
        tracing::warn!("--i-accept-data-loss: durability refusals are overridden for this mount");
    }
    crate::mount::run(o)
}

fn tree(g: &Global, cmd: TreeCmd) -> Result<i32> {
    let mut c = connect_fs(g)?;
    match cmd {
        TreeCmd::List => {
            let rows = q(
                &mut c,
                "list trees",
                "SELECT t.name, t.layer, l.depth, l.epoch, t.root_ino, u.inodes, u.logical_bytes,
                        (SELECT count(*) FROM pgfs.snapshots s WHERE s.tree = t.name),
                        (SELECT m.host || ' pid ' || m.pid FROM pgfs.mounts m WHERE m.tree = t.name AND NOT m.read_only AND m.lease_until > now())
                 FROM pgfs.trees t JOIN pgfs.layers l ON l.layer = t.layer, LATERAL pgfs.layer_usage(t.layer) u
                 ORDER BY t.name",
                &[],
            )?;
            for r in rows {
                let (name, layer, depth, epoch, root): (String, i64, i32, i64, i64) =
                    (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4));
                let (files, bytes, snaps, holder): (i64, i64, i64, Option<String>) =
                    (r.get(5), r.get(6), r.get(7), r.get(8));
                if g.json {
                    println!(
                        "{}",
                        serde_json::json!({"tree": name, "layer": layer, "depth": depth, "epoch": epoch,
                        "root_ino": root, "files": files, "bytes": bytes, "snapshots": snaps, "mounted_by": holder})
                    );
                } else {
                    println!("{name:<24} layer {layer:<4} depth {depth} epoch {epoch:<6} files {files:<8} bytes {bytes:<12} snapshots {snaps}{}",
                        holder.map(|h| format!("  mounted by {h}")).unwrap_or_default());
                }
            }
            Ok(0)
        }
        TreeCmd::Delete { tree } => {
            q(
                &mut c,
                &format!("delete tree {tree}"),
                "SELECT pgfs.tree_delete($1)",
                &[&tree],
            )?;
            println!("deleted tree {tree}; its space is reclaimed in the background");
            Ok(0)
        }
        TreeCmd::Flatten { tree } => {
            let mut total = 0i64;
            loop {
                let n: i64 = q(
                    &mut c,
                    "flatten",
                    "SELECT pgfs.flatten_batch($1, 10000)",
                    &[&tree],
                )?[0]
                    .get(0);
                total += n;
                if n == 0 {
                    let done: bool = q(
                        &mut c,
                        "flatten",
                        "SELECT pgfs.flatten_finish($1)",
                        &[&tree],
                    )?[0]
                        .get(0);
                    if done {
                        break;
                    }
                }
            }
            println!("flattened {tree}: {total} rows copied, now depth 0");
            Ok(0)
        }
    }
}

/// Is the tree mounted read-write right now?
fn has_writer(c: &mut Conn, tree: &str) -> Result<bool> {
    Ok(q(c, "check mounts", "SELECT EXISTS (SELECT 1 FROM pgfs.mounts WHERE tree = $1 AND NOT read_only AND lease_until > now())", &[&tree])?[0].get(0))
}

/// Ask the owning daemon to drain and run a control request
/// (spec/branch.py DrainsOwnerBeforeSnapshot).
fn via_owner(
    c: &mut Conn,
    tree: &str,
    kind: &str,
    args: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value> {
    let id: i64 = q(
        c,
        "submit control request",
        "SELECT pgfs.control_submit($1, $2, $3)",
        &[&tree, &kind, &args],
    )?[0]
        .get(0);
    let deadline = Instant::now() + timeout;
    loop {
        let rows = q(
            c,
            "control status",
            "SELECT state, result FROM pgfs.control_status($1)",
            &[&id],
        )?;
        let state: String = rows[0].get(0);
        let result: Option<serde_json::Value> = rows[0].get(1);
        match state.as_str() {
            "done" => return Ok(result.unwrap_or_default()),
            "failed" => {
                let r = result.unwrap_or_default();
                let code = r.get("sqlstate").and_then(|v| v.as_str()).unwrap_or("");
                let msg = r
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("failed");
                return Err(cli_err(
                    &format!("{kind} by the owning daemon"),
                    DbError::synthetic(code, msg),
                ));
            }
            "cancelled" => bail!("{kind} request was cancelled"),
            _ => {}
        }
        if Instant::now() >= deadline {
            let cancelled: bool = q(
                c,
                "cancel control request",
                "SELECT pgfs.control_cancel($1)",
                &[&id],
            )?[0]
                .get(0);
            if cancelled {
                bail!("the owning daemon did not answer within {timeout:?}; nothing was created (ETIMEDOUT). Use --crash-consistent to bypass it");
            }
            continue;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn snapshot(g: &Global, a: SnapshotArgs) -> Result<i32> {
    let mut c = connect_fs(g)?;
    match a.args.first().map(String::as_str) {
        Some("list") => {
            let tree = a.args.get(1);
            let rows = q(&mut c, "list snapshots",
                "SELECT s.tree, s.name, s.created_at::text, s.epoch, s.digest IS NOT NULL, pgfs.snapshot_unique_bytes(s.layer, s.epoch)
                 FROM pgfs.snapshots s WHERE $1::text IS NULL OR s.tree = $1 ORDER BY s.tree, s.created_at", &[&tree])?;
            for r in rows {
                let (t, n, at, e, sealed, b): (String, String, String, i64, bool, i64) =
                    (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5));
                if g.json {
                    println!(
                        "{}",
                        serde_json::json!({"tree": t, "name": n, "created_at": at, "epoch": e, "sealed": sealed, "bytes_unique": b})
                    );
                } else {
                    println!(
                        "{t}@{n:<32} {at}  epoch {e:<6} unique bytes {b}{}",
                        if sealed { "  sealed" } else { "" }
                    );
                }
            }
            Ok(0)
        }
        Some("delete") => {
            let v = ViewSpec::parse(
                a.args
                    .get(1)
                    .ok_or_else(|| anyhow!("snapshot delete TREE@NAME"))?,
            )
            .map_err(|e| anyhow!(e))?;
            let n = v
                .snapshot
                .ok_or_else(|| anyhow!("snapshot delete TREE@NAME"))?;
            q(
                &mut c,
                "delete snapshot",
                "SELECT pgfs.snapshot_delete($1, $2)",
                &[&v.tree, &n],
            )?;
            println!("deleted {}@{n}", v.tree);
            Ok(0)
        }
        Some(spec) => {
            let v = ViewSpec::parse(spec).map_err(|e| anyhow!(e))?;
            let name = v
                .snapshot
                .clone()
                .ok_or_else(|| anyhow!("snapshot TREE@NAME"))?;
            if has_writer(&mut c, &v.tree)? && !a.crash_consistent {
                via_owner(
                    &mut c,
                    &v.tree,
                    "snapshot",
                    serde_json::json!({"name": name}),
                    Duration::from_secs(a.timeout),
                )?;
            } else {
                q(
                    &mut c,
                    "snapshot",
                    "SELECT layer FROM pgfs.snapshot($1, $2)",
                    &[&v.tree, &name],
                )?;
                confirm_sync(&mut c, "snapshot")?;
            }
            if a.seal {
                crate::check::seal(&mut c, &v.tree, &name)?;
            }
            notify_changes(&mut c, &v.tree);
            println!("created snapshot {}@{name}", v.tree);
            Ok(0)
        }
        None => bail!("snapshot TREE@NAME | list [TREE] | delete TREE@NAME"),
    }
}

fn branch(g: &Global, a: BranchArgs) -> Result<i32> {
    let mut c = connect_fs(g)?;
    check_tree_name(&a.new).map_err(|e| anyhow!(e))?;
    let v = ViewSpec::parse(&a.source).map_err(|e| anyhow!(e))?;
    let path = v.path.as_ref().map(|p| p.as_bytes().to_vec());
    if v.snapshot.is_none() && has_writer(&mut c, &v.tree)? {
        let args = serde_json::json!({"new": a.new, "path": path.as_ref().map(|p| hex(p))});
        via_owner(
            &mut c,
            &v.tree,
            "branch",
            args,
            Duration::from_secs(a.timeout),
        )?;
    } else {
        let rows = q(
            &mut c,
            "branch",
            "SELECT layer, depth FROM pgfs.branch($1, $2, $3, $4)",
            &[&v.tree, &v.snapshot, &path, &a.new],
        )?;
        let depth: i32 = rows[0].get(1);
        if depth > 4 {
            tracing::warn!("{} has branch depth {depth}; lookups walk {depth} layers. Consider `pgfs tree flatten {}`", a.new, a.new);
        }
        confirm_sync(&mut c, "branch")?;
    }
    notify_changes(&mut c, &v.tree);
    println!("created tree {} from {}", a.new, a.source);
    Ok(0)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Resolve a view to (layer, epoch) for diff.
fn view_epoch(c: &mut Conn, v: &ViewSpec) -> Result<(i64, i64)> {
    match &v.snapshot {
        Some(s) => {
            let rows = q(
                c,
                "find snapshot",
                "SELECT layer, epoch FROM pgfs.snapshots WHERE tree = $1 AND name = $2",
                &[&v.tree, s],
            )?;
            let r = rows
                .first()
                .ok_or_else(|| anyhow!("snapshot {}@{s} does not exist (ENOENT)", v.tree))?;
            Ok((r.get(0), r.get(1)))
        }
        None => {
            let rows = q(c, "find tree", "SELECT t.layer, l.epoch FROM pgfs.trees t JOIN pgfs.layers l ON l.layer = t.layer WHERE t.name = $1", &[&v.tree])?;
            let r = rows
                .first()
                .ok_or_else(|| anyhow!("tree {} does not exist (ENOENT)", v.tree))?;
            Ok((r.get(0), r.get(1)))
        }
    }
}

fn diff(g: &Global, from: &str, to: &str) -> Result<i32> {
    let mut c = connect_fs(g)?;
    let (a, b) = (
        ViewSpec::parse(from).map_err(|e| anyhow!(e))?,
        ViewSpec::parse(to).map_err(|e| anyhow!(e))?,
    );
    let (la, ea) = view_epoch(&mut c, &a)?;
    let (lb, eb) = view_epoch(&mut c, &b)?;
    let rows = match c.query_sql(
        "SELECT change, ino, path, old_path FROM pgfs.diff_views($1, $2, $3, $4)",
        &[&la, &ea, &lb, &eb],
    ) {
        Ok(rows) => rows,
        Err(e) if e.sqlstate.as_deref() == Some("PF007") => {
            // Unrelated layers: name the --force promote that moved a tree, if any.
            let pp: Option<String> = q(
                &mut c,
                "find pre-promote snapshot",
                "SELECT tree || '@' || name FROM pgfs.snapshots WHERE tree IN ($1, $2)
                   AND name LIKE 'pre-promote-%' AND layer IN ($3, $4) ORDER BY created_at DESC LIMIT 1",
                &[&a.tree, &b.tree, &la, &lb],
            )?
            .first()
            .map(|r| r.get(0));
            match pp {
                Some(s) => bail!(
                    "{from} and {to} are on unrelated layers: a --force promote moved the tree \
                     (its previous contents are {s}); diff each side against that snapshot"
                ),
                None => bail!("{from} and {to} are unrelated: diff compares two views of one tree, or a branch with its base"),
            }
        }
        Err(e) => return Err(cli_err("diff", e)),
    };
    for r in rows {
        let (ch, ino, p, op): (String, i64, Option<Vec<u8>>, Option<Vec<u8>>) =
            (r.get(0), r.get(1), r.get(2), r.get(3));
        let p = p
            .map(|p| crate::names::display_bytes(&p))
            .unwrap_or_else(|| "?".into());
        let op = op.map(|p| crate::names::display_bytes(&p));
        if g.json {
            println!(
                "{}",
                serde_json::json!({"change": ch, "ino": ino, "path": p, "old_path": op})
            );
        } else {
            match op {
                Some(o) => println!("{ch:<9} {o} -> {p}"),
                None => println!("{ch:<9} {p}"),
            }
        }
    }
    Ok(0)
}

fn stats(g: &Global, tree: Option<&str>) -> Result<i32> {
    let mut c = connect_fs(g)?;
    let info = schema::server_info(&mut c)?;
    let rows = q(&mut c, "stats",
        "SELECT t.name, u.inodes, u.logical_bytes,
                (SELECT COALESCE(sum(octet_length(inline_data)), 0)::bigint FROM pgfs.inodes i WHERE i.layer = t.layer AND i.died = 9223372036854775807),
                (SELECT COALESCE(sum(pgfs.snapshot_unique_bytes(s.layer, s.epoch)), 0)::bigint FROM pgfs.snapshots s WHERE s.tree = t.name)
         FROM pgfs.trees t, LATERAL pgfs.layer_usage(t.layer) u WHERE $1::text IS NULL OR t.name = $1 ORDER BY t.name", &[&tree])?;
    let fsu = q(
        &mut c,
        "usage",
        "SELECT physical_bytes, inodes FROM pgfs.fs_usage()",
        &[],
    )?;
    let bl = q(
        &mut c,
        "backlog",
        "SELECT retired_rows, gc_queue, deleting_layers FROM pgfs.backlog()",
        &[],
    )?;
    let dead = q(&mut c, "dead tuples", "SELECT relname::text, n_dead_tup, n_live_tup FROM pg_stat_user_tables WHERE schemaname = 'pgfs' AND relname IN ('blobs','file_blocks','inodes') ORDER BY 1", &[])?;
    let wal = q(
        &mut c,
        "wal",
        "SELECT wal_bytes::bigint FROM pg_stat_wal",
        &[],
    )?;
    let physical: i64 = fsu[0].get(0);
    let mut out = serde_json::json!({
        "physical_bytes": physical,
        "capacity_bytes": info.capacity(),
        "inodes": fsu[0].get::<_, i64>(1),
        "reclaim_backlog_rows": bl[0].get::<_, i64>(0),
        "gc_queue": bl[0].get::<_, i64>(1),
        "deleting_layers": bl[0].get::<_, i64>(2),
        "async_commit_window_ms": info.async_window_ms(),
        "wal_bytes_since_stats_reset": wal[0].get::<_, i64>(0),
        "write_amplification_wal_per_physical_byte": if physical > 0 { wal[0].get::<_, i64>(0) as f64 / physical as f64 } else { 0.0 },
        "trees": [],
        "tables": [],
    });
    for r in rows {
        out["trees"].as_array_mut().expect("array").push(serde_json::json!({
            "tree": r.get::<_, String>(0), "files": r.get::<_, i64>(1), "live_bytes": r.get::<_, i64>(2),
            "inline_bytes": r.get::<_, i64>(3), "snapshot_only_bytes": r.get::<_, i64>(4)}));
    }
    for r in dead {
        out["tables"].as_array_mut().expect("array").push(serde_json::json!({
            "table": r.get::<_, String>(0), "dead_tuples": r.get::<_, i64>(1), "live_tuples": r.get::<_, i64>(2)}));
    }
    if g.json {
        println!("{out}");
    } else {
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    Ok(0)
}

/// The reference table of every option, generated from the parser
/// (spec/ops.py DocumentsEveryOption).
pub fn options_reference() -> String {
    let mut out = String::from(
        "| Command | Option | Environment | Default | Description |\n|---|---|---|---|---|\n",
    );
    let cmd = Cli::command();
    fn walk(cmd: &clap::Command, path: &str, out: &mut String) {
        for a in cmd.get_arguments() {
            if a.get_id() == "help" || a.get_id() == "version" {
                continue;
            }
            if a.is_global_set() && !path.is_empty() {
                continue;
            }
            let name = match (a.get_long(), a.get_short()) {
                (Some(l), _) => format!("`--{l}`"),
                (None, Some(s)) => format!("`-{s}`"),
                (None, None) => format!("`{}`", a.get_id().as_str().to_uppercase()),
            };
            let env = a
                .get_env()
                .map(|e| format!("`{}`", e.to_string_lossy()))
                .unwrap_or_default();
            let def: Vec<String> = a
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let help = a
                .get_help()
                .map(|h| h.to_string())
                .unwrap_or_default()
                .replace('|', "\\|");
            out.push_str(&format!(
                "| `pgfs{}` | {name} | {env} | {} | {help} |\n",
                if path.is_empty() {
                    String::new()
                } else {
                    format!(" {path}")
                },
                def.join(",")
            ));
        }
        for s in cmd.get_subcommands() {
            let p = if path.is_empty() {
                s.get_name().to_string()
            } else {
                format!("{path} {}", s.get_name())
            };
            walk(s, &p, out);
        }
    }
    walk(&cmd, "", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parser_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_option_is_in_the_reference_table() {
        let table = options_reference();
        let cmd = Cli::command();
        fn check(cmd: &clap::Command, table: &str) {
            for a in cmd.get_arguments() {
                if let Some(l) = a.get_long() {
                    if l != "help" && l != "version" {
                        assert!(
                            table.contains(&format!("`--{l}`")),
                            "--{l} missing from the reference table"
                        );
                    }
                }
            }
            for s in cmd.get_subcommands() {
                check(s, table);
            }
        }
        check(&cmd, &table);
        let doc =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/cli-options.md"))
                .expect("docs/cli-options.md exists; regenerate it with `pgfs options`");
        assert!(
            doc.contains(&table),
            "docs/cli-options.md is stale; regenerate it with `pgfs options`"
        );
    }

    #[test]
    fn mount_defaults_match_the_spec() {
        let c = Cli::try_parse_from(["pgfs", "mount", "main", "/mnt"]).unwrap();
        let Cmd::Mount(m) = c.cmd else { panic!() };
        assert_eq!(m.lease, 30);
        assert_eq!(m.buffer_per_file, 8 << 20);
        assert_eq!(m.buffer_total, 256 << 20);
        assert_eq!(m.stall_warn, 5);
        assert_eq!(m.stall_error, 60);
        assert_eq!(m.replica_wait, 20);
        assert_eq!(m.auto_flatten_depth, 4);
    }
}

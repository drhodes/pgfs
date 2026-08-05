# pgfs

A filesystem that lives in PostgreSQL. You mount a directory, and every
`ls`, `mkdir`, `cat`, `echo >`, and `rm` becomes rows in a Postgres table —
your filesystem is a database, with all the querying and replication that
implies.

Everything runs through [Nix](https://nixos.org): `flake.nix` pins the Rust
toolchain, FUSE3 headers, and PostgreSQL, so `nix develop` gives you the
whole environment, reproducible. The one non-Nix system requirement is a
single line in `/etc/fuse.conf` (see [Setup](#setup)).

It's deliberately small: arbitrary directory nesting, whole files stored as
`bytea` blobs, no chunking, no search. It exists to prove the read/write
path end to end — a playground to poke at, and a foundation to build on.

## Quick start

```bash
# 1. Enter the Nix dev shell (pins Rust 1.9x, Postgres 17, FUSE3)
nix develop

# 2. Bring up Postgres + build + mount (waits until the mount is live)
./scripts/pgfs.sh up

# 3. It's just a directory — mess around
echo "hello world" > testdata/mnt/hello.txt
mkdir -p testdata/mnt/docs
echo "notes" > testdata/mnt/docs/notes.txt
cat testdata/mnt/hello.txt
find testdata/mnt
rm testdata/mnt/docs/notes.txt

# 4. See what Postgres actually stored
psql -h "$(pwd)/testdata" -d pgfs -c "SELECT parent, name, kind, size FROM entries ORDER BY parent, name;"

# 5. Tear it down
./scripts/pgfs.sh down
```

Three commands (`up`, `down`, `status`) are the whole lifecycle. See the
full docs in [MANUAL.md](MANUAL.md).

## Setup

**Nix (recommended).** `nix develop` provides `rustc`/`cargo`, PostgreSQL
server binaries, and FUSE3 — no system packages to install. If you use
direnv, the included `.envrc` enters the shell automatically.

**Non-Nix fallback.** You'll need a current stable Rust (1.85+), `postgres`
binaries, and `libfuse3-dev` + `pkg-config` on your `PATH`.

**The one system line.** The mount uses `auto_unmount` so a dead daemon can
never leave a ghost mount behind. That requires `user_allow_other` in
`/etc/fuse.conf`:

```
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

## How it works, in one paragraph

Two modules meet behind a thin seam: `db.rs` is the only file that speaks
SQL, and `fs.rs` is the only file that speaks FUSE. Postgres stores the tree
in one `entries` table keyed by `(parent, name)`; `fs.rs` keeps an in-memory
map translating FUSE inode numbers (which the kernel uses for everything
after the first lookup) to those keys. Every filesystem syscall becomes a
query over a Unix socket to a Postgres cluster that lives entirely inside
`testdata/` — no TCP, no system-wide Postgres, no data written outside the
project.

## Observability

pgfs emits structured diagnostics at every layer so operators can understand
what the daemon is doing, how fast it's doing it, and why it failed — without
a debugger.

### Structured tracing

Every FUSE callback and every database round-trip opens a `tracing` span:

| Layer | Level | What's recorded |
|-------|-------|-----------------|
| FUSE callback | `INFO` | inode, name (where applicable) |
| DB method | `DEBUG` | (parent, name) key |

Spans are nested: a DB span that fires during a FUSE callback is
automatically attributed to the parent callback. The subscriber renders
human-readable text by default; set `RUST_LOG_FORMAT=json` for
newline-delimited JSON ingestible by `jq`, Elasticsearch, or any log
aggregator.

Filter with the familiar `RUST_LOG` syntax:

```bash
RUST_LOG=pgfs=info                          # lifecycle events only
RUST_LOG=pgfs::fs=trace,pgfs::db=debug     # targeted verbosity
RUST_LOG_FORMAT=json ./scripts/pgfs.sh up   # machine-parseable
```

### Metrics

Counters and latency histograms are available at runtime and are logged
at `INFO` every 60 seconds.

**FUSE operation counters** (atomic, per-callback): `lookup`, `getattr`,
`setattr`, `read`, `write`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`,
`readdir`, `open`, `fsync`.

**Error counters**: separate counters for unexpected failures (`EIO`) and
every expected errno (`ENOENT`, `EEXIST`, `ENOTEMPTY`, `EISDIR`, `ENOTDIR`,
`EINVAL`). A spike in `EIO` means database trouble; a spike in `ENOENT`
is just a busy user.

**Latency histograms**: each FUSE callback and each DB query records its
wall-clock duration into μs-bucketed histograms (≤10μs, ≤100μs, ≤1ms,
≤10ms, ≤100ms, ≤1s, >1s). The 60-second export includes total count and
p50/p95/p99 percentiles:

```
metrics|latency|fuse=total=1234 p50=<=10μs p95=<=100μs p99=<=1ms
metrics|latency|db=total=1200 p50=<=100μs p95=<=1ms p99=<=10ms
```

### Health endpoint

On mount, the daemon writes two files under `/tmp/`:

| File | Purpose |
|------|---------|
| `/tmp/pgfs-{hash}.pid` | PID of the daemon process |
| `/tmp/pgfs-{hash}.ready` | Created once the FUSE session is serving; supervisors poll for this |

Both are removed on clean shutdown via RAII guards. The `{hash}` is a
stable hex digest of the mountpoint path, so repeated mounts to the same
directory produce the same filenames.

A background **liveness heartbeat** bumps an `AtomicU64` every 10 seconds.
Every FUSE callback observes the counter on entry; if it has not advanced
within 30 seconds (3×N), the callback logs `ERROR`, a watchdog thread
performs a clean unmount — a wedged daemon can't linger, while an idle
mount is never falsely flagged.

### Signals

| Signal | Behavior |
|--------|----------|
| `SIGINT` / `SIGTERM` / `SIGHUP` | Clean unmount via `session.unmount()`, then exit |
| `SIGUSR1` | Request a state dump on the next FUSE callback (inode cache size, metrics snapshot, uptime) |
| `SIGUSR2` | Toggle CPU profiling (requires `--features profiling` at build time) |

All signals are handled on a dedicated `sigwait` thread so they never block
FUSE dispatch.

### Crash forensics

If the daemon panics, a custom panic hook emits the panic message,
file:line location, a symbolized backtrace, and OS/Rust/environment
metadata (pgfs version, OS/arch, rustc, `RUST_BACKTRACE`) to both stderr
and the `tracing::error!` channel. In `--release` builds,
`panic = "abort"` in `Cargo.toml` prevents a broken daemon from lingering
— the crash report is written before the process exits.

### Profiling (feature-gated)

Rebuild with `--features profiling` to unlock on-demand CPU and span
profiling:

- **CPU profiling** via the `pprof` crate: SIGUSR2 starts/stops sampling;
  on stop a flamegraph is written to `/tmp/pgfs-profile-{timestamp}.svg`
  plus a per-stack dump at `/tmp/pgfs-profile-{timestamp}.stacks`
  (the `.pb` protobuf codec is not enabled).
- **Span flamegraphs** via `tracing-chrome`: writes a Chrome trace file to
  `/tmp/pgfs-trace-{timestamp}.json` on shutdown. Open in `chrome://tracing`
  or Perfetto to see the full span tree with nanosecond timestamps.

Both are zero-overhead when the feature flag is disabled (the default).

## Testing

Two layers, matching how the ext4 and ZFS projects test their filesystems
(xfstests and the OpenZFS zfs-tests suite):

```bash
make test              # Rust unit tests (cargo test)
make test-integration  # full FUSE integration suite (scripts/run_tests.sh)
```

The integration suite is a set of self-contained scripts that mount pgfs,
exercise one POSIX area end to end, and unmount. They are ports of the
canonical filesystem test suites:

| Script | Ported from | Covers |
|--------|-------------|--------|
| `scripts/test_truncate.sh` | xfstests `generic/014` (truncfile) | truncate down/up/zero, zero-fill, size sweep |
| `scripts/test_append.sh` | ZFS `append/file_append.ksh` | O_APPEND writes always land at EOF, never clobber |
| `scripts/test_integrity.sh` | xfstests `generic/001` | create/write/unlink chains with `cmp` data-integrity checks |
| `scripts/test_dirs.sh` | ZFS mkdir/rmdir functional tests | deep `mkdir -p` nesting, readdir listing, rmdir semantics |
| `scripts/test_rename.sh` | (original) | POSIX rename overwrite rules |
| `scripts/test_git_init.sh` | (original) | `git init` end-to-end |

Each test sources `scripts/test_lib.sh` — the same job xfstests' `common/rc`
or ZFS's `libtest.shlib` does: project layout, Postgres bring-up, one clean
mount, and pass/fail accounting.

## Layout

```
flake.nix          pinned Nix dev environment (Rust + Postgres + FUSE3)
scripts/
  init_db.sh       create/start the project-local Postgres cluster
  pgfs.sh          the up/down/status/run lifecycle control
  replica_db.sh    provision/run the Docker streaming standby (--replica)
  run_tests.sh     run the full integration suite (like xfstests' ./check)
  test_lib.sh      shared test harness (like xfstests' common/rc)
  test_*.sh        one integration test per file (mount, exercise, unmount)
src/
  db.rs            the only module that runs SQL
  fs.rs            the only module that talks FUSE (fuser::Filesystem)
  main.rs          wires them together, CLI args, mount lifecycle
  metrics.rs       counters, histograms, periodic export, liveness
  error.rs         error context chaining + log_and_reply! macro
testdata/          gitignored; the Postgres cluster, logs, pid, mount point
MANUAL.md          the full manual
```

## Limitations (by design, for now)

- **Whole-blob storage** — every write rewrites the entire file.
- **No permissions** — files are always 0644, dirs 0755.
- **Single-threaded** — fine for the kernel's default mount; the in-memory
  inode map would need a lock before going multi-threaded.
- **No search, indices, or chunked blocks yet.**

Observability is fully wired — FUSE latency is recorded for every
callback via an RAII guard, the liveness heartbeat is observed by every
callback (with a watchdog-triggered clean unmount on stall), and SIGUSR1
state dumps fire from the callback preamble.

## Replicated mode

pgfs can be backed by a primary Postgres plus a physical streaming
standby running in Docker (`spec/replica.py`). Writes go to the primary;
reads are served from the standby once it has caught up with the primary's
WAL, and fall back to the primary whenever it lags or is down:

```bash
./scripts/replica_db.sh up     # docker + postgres:17, pg_basebackup + WAL streaming
./scripts/pgfs.sh down
target/debug/pgfs testdata/mnt --replica "$(./scripts/replica_db.sh conn)"
```

`replica_db.sh status` reports container/recovery/lag state. See
[MANUAL.md §7.5](MANUAL.md) for the full read-routing table.

Details, rationale, troubleshooting, and the roadmap are in
[MANUAL.md](MANUAL.md).

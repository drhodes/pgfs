# pgfs — The Database That Pretends to Be a Filesystem

**Your files. In Postgres. Because why not?**

`pgfs` is a FUSE filesystem where every directory listing, every file
creation, every `echo "hello" > foo.txt` becomes a row in a PostgreSQL
table. Your filesystem *is* a database. You can `SELECT` your directory
tree. You can `JOIN` your files with other tables. You can replicate
your entire filesystem to a standby with `pg_basebackup`. The kernel
thinks it's talking to a disk; really it's talking to Postgres.

This manual is the ground truth — how to set it up, how it stays alive,
how it works under the hood, and how to see inside the thing while it's
running.

---

## 1. The idea, in one breath

Mount a directory. Use it like any other directory. Behind the scenes:

```
You type          →  pgfs fires off
───────────────────────────────────────
ls                →  SELECT ... FROM entries WHERE parent = $1
mkdir docs        →  INSERT ... VALUES (parent, name, 'dir')
echo hi > f.txt   →  INSERT then UPDATE a bytea blob
cat f.txt         →  SELECT data FROM entries WHERE ...
rm f.txt          →  DELETE FROM entries WHERE ...
mv a.txt b.txt    →  BEGIN; DELETE target; UPDATE source; COMMIT
git init ./proj   →  All of the above, hundreds of times, correctly
```

The files aren't "mapped" to rows. They *are* rows. There's no translation
layer that drifts out of sync — the database is the authoritative copy.

This build is deliberately small to prove the path end to end: arbitrary
directory nesting, whole files as `bytea` blobs, POSIX rename with atomic
overwrite, full observability. No chunking, no search, no permissions —
those are layers on the same seam.

---

## 2. Architecture: one kernel, two namespaces, three layers

pgfs is a story about **translating between two addressing schemes**. The
kernel addresses files by inode number; Postgres addresses them by
`(parent, name)`. Everything in this project is a layer that mediates
between the two — and the design exists so the database stays the single
source of truth.

```
                 ┌──────────────┐
 kernel syscalls │    fs.rs     │  speaks FUSE only
   (inode-based) │  ino ⇄ path  │  never constructs SQL
                 └──────┬───────┘
                        │ method calls: read / write / create / mkdir / ...
                 ┌──────▼───────┐
                 │    db.rs     │  speaks SQL only
                 │ (parent,name)│  never touches FUSE types
                 └──────┬───────┘
                        │ libpq over Unix socket
                 ┌──────▼───────┐
                 │ Postgres 17  │  cluster in testdata/pgdata
                 └──────────────┘
```

### The two namespaces

- **The kernel's namespace is inode numbers.** After the first `lookup`,
  the kernel never sends paths again — every callback arrives as an
  integer inode. Inodes are a pure in-memory invention of `fs.rs`
  (root = 1, allocated lazily); they are **never stored in Postgres**.
  The `ino_by_path` / `path_by_ino` maps are the heart of the
  translation.
- **The database's namespace is paths.** Each row of the single `entries`
  table is keyed by `(parent, name)`: the full path of the containing
  directory plus the entry's own name. `('', 'docs')` is `docs/` at the
  root; `('docs', 'notes.txt')` is the file inside it.

That's the whole trick. `fs.rs` owns the kernel half of the translation;
`db.rs` owns the database half. They meet at a single seam — plain method
calls — and nothing else crosses it.

### The seam: where future features plug in

`fs.rs` never constructs SQL. `db.rs` never sees a FUSE reply type. The
two modules only ever call each other's methods:

```rust
// fs.rs (FUSE side) — the kernel gets an errno, the log gets the story
self.db.read(parent, name)
// db.rs (SQL side) — every query is wrapped with error context
error::ctx(client.query(...), "read contents of {name} in {parent}")
```

This seam is deliberate. Chunked blocks, search indices, permissions —
each is a new `db.rs` method, and the FUSE layer never changes. Replicated
mode (§7.5) is the same seam extended: `db.rs` gains a second read client
and not a line of FUSE code changed. If you want to understand how pgfs
grows, look at what sits on either side of this seam.

### What happens on a syscall

1. The kernel dispatches a FUSE request — say, `read` on inode 42.
2. `fs.rs` looks up 42 → `"docs/notes.txt"` in its in-memory map and
   splits it into `("docs", "notes.txt")`.
3. `fs.rs` calls `db.read("docs", "notes.txt")`.
4. `db.rs` runs `SELECT data FROM entries WHERE parent = $1 AND name = $2`
   over the Unix socket, wrapping any failure in an error story.
5. `fs.rs` clamps the byte range and replies. Done — one row touched.

Because the database is the authoritative copy, there is no cache to go
stale: what you `cat` is what a `SELECT` returns, and `git init` works
because the syscall surface is real.

### Shape of the runtime

- **Single-threaded, single connection.** FUSE's default mount dispatches
  one request at a time, so the in-memory maps need no locks and the
  daemon needs exactly one Postgres `Client`. (Multi-threading is a later
  step: a `Mutex` on the maps, a connection pool underneath.)
- **Whole-blob writes.** A write re-reads the full file, patches the byte
  range in memory, and writes the whole blob back. Correct and simple —
  and the obvious first performance target (§11).
- **TTL-cached attributes.** Entries and attributes carry a 1-second TTL,
  so the kernel batches `getattr`/`lookup` and Postgres isn't hammered
  per syscall.
- **Observability is woven through the seam.** Every FUSE callback opens
  a span; every `db.rs` method records latency; counters and histograms
  are exported every 60 seconds (§9).
- **Reliability is layered.** `AutoUnmount` removes the mount if the
  daemon dies for any reason; clean shutdown runs through a dedicated
  `sigwait` thread; the launch script escalates when unmount is blocked
  (§6).

The rest of this manual fills in the details: the storage model and
rename semantics in §7, replicated mode in §7.5, implementation notes in
§8, and the observability contract in §9.

---

## 3. Getting set up

### 3.1 Nix (the easy way)

`flake.nix` pins the whole world: Rust toolchain, PostgreSQL 17 server
binaries, FUSE3 with headers, plus `gdb`, `lldb`, `jq`, and friends.

```bash
nix develop
```

If you use direnv, the checked-in `.envrc` enters the shell automatically
whenever you `cd` in. You'll see a banner with the pinned versions.

### 3.2 No Nix? No problem

You need three things on your `PATH`:

- **Rust** stable 1.85+ (`rustup` recommended over distro Rust)
- **PostgreSQL server binaries** — `initdb`, `pg_ctl`, `psql`, `createdb`
  (Debian/Ubuntu hides them under `/usr/lib/postgresql/<ver>/bin`)
- **libfuse3-dev** + `pkg-config` (or your distro's equivalent)

### 3.3 The one system line you have to type

The mount uses `auto_unmount` — the kernel itself removes the mount
whenever the daemon dies, for *any* reason. `kill -9`? Mount's gone.
Segfault? Mount's gone. This one-liner enables it:

```bash
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

Without it you'll see: `option allow_other only allowed if 'user_allow_other' is set`.

### 3.4 Your very own Postgres, no sysadmin required

Nothing touches a system-wide Postgres. The cluster lives entirely inside
`testdata/pgdata/`, listens only on a Unix socket (TCP is blocked with
`-h ''`), and trusts local connections. `rm -rf testdata` is a complete
factory reset.

---

## 4. Five minutes to `ls` in a database

```bash
nix develop                       # or make sure you've got Rust + Postgres + FUSE
./scripts/pgfs.sh up              # Postgres → build → mount (idempotent, waits for live mount)
```

That's it. The mount is live at `testdata/mnt`. Now have fun:

```bash
# The basics
echo "hello" > testdata/mnt/hello.txt
cat testdata/mnt/hello.txt
rm testdata/mnt/hello.txt

# Nested directories, just like you'd expect
mkdir -p testdata/mnt/docs/sub
echo "notes" > testdata/mnt/docs/sub/notes.txt

# Standard Unix tools all work
ls -la testdata/mnt
find testdata/mnt -type f
stat testdata/mnt/docs/sub/notes.txt
cp testdata/mnt/docs/sub/notes.txt testdata/mnt/copy.txt
echo "more" >> testdata/mnt/docs/sub/notes.txt     # append works
truncate -s 100 testdata/mnt/docs/sub/notes.txt    # so does truncation
touch testdata/mnt/new.txt                          # create + setattr

# The grand prize: git init works against a database-backed filesystem
git init testdata/mnt/proj

# Directories enforce their rules
rmdir testdata/mnt/docs/sub          # fails: ENOTEMPTY while notes.txt is inside
rm testdata/mnt/docs/sub/notes.txt
rmdir testdata/mnt/docs/sub          # clean now

# Here's the magic: your files are rows
psql -h "$(pwd)/testdata" -d pgfs -c \
  "SELECT parent, name, kind, size FROM entries ORDER BY parent, name;"
```

Shut down:

```bash
./scripts/pgfs.sh down              # clean unmount, leaves Postgres running
```

---

## 5. One script to rule them all: `pgfs.sh`

You never need to remember `fusermount3` incantations or `pg_ctl` flags.
One script owns the entire lifecycle.

| Command | What happens |
|---------|-------------|
| `./scripts/pgfs.sh up` | Starts Postgres if it's down, builds the binary, sweeps any stale mount, starts the daemon, **waits until the mount appears in `/proc/mounts`**, reports versions. Idempotent. |
| `./scripts/pgfs.sh down` | Signals the daemon, polls for the mount to disappear, verifies. If something blocks it (CWD inside the mount, etc.), escalates: clean unmount → `fusermount3 -u` → report holders → lazy unmount. |
| `./scripts/pgfs.sh status` | Mount state, daemon PID, Postgres status. Flags ghost mounts if it sees one. |
| `./scripts/pgfs.sh run` | Foreground mode for debugging. Ctrl+C unmounts cleanly. |
| `PGFS_MOUNT=/some/dir ./scripts/pgfs.sh up` | Mount somewhere else. |

### What lives where

```
testdata/
  pgdata/           Postgres cluster (created by init_db.sh)
  pgfs.pid          daemon PID
  pgfs.log          daemon stdout/stderr
  postgres.log      Postgres server log
  mnt/              the mount point
```

Postgres is managed separately from the daemon: `init_db.sh` creates and
starts the cluster; `pg_ctl -D testdata/pgdata stop` stops it. `pgfs.sh up`
auto-starts it if needed; `down` leaves it running.

---

## 6. Why your mounts never ghost

Three layers of defense. If the daemon dies — clean exit, `kill -9`,
segfault, OOM killer — you will **never** be left with a dangling,
unusable, "Transport endpoint is not connected" mount.

**Layer 1: `AutoUnmount`.** The kernel itself tracks the daemon process.
When that process disappears, the kernel removes the mount. This is the
load-bearing layer. It works even if the daemon is nuked from orbit.

**Layer 2: Signal handling.** pgfs blocks `SIGINT`, `SIGTERM`, `SIGHUP`,
`SIGUSR1`, and `SIGUSR2` process-wide and waits on them in a dedicated
`sigwait` thread. When shutdown signals arrive, the daemon unmounts cleanly
and `run()` returns normally. No abrupt death, no panic — a graceful exit.

**Layer 3: Script escalation.** `pgfs.sh down` treats the mount as the
source of truth. It polls `/proc/mounts` until the mount is gone. If
something has its CWD inside the mount and blocks unmount, it reports
the offenders and falls back to lazy unmount rather than failing silently.

### When things go sideways

| Symptom | What's happening | Fix |
|---------|-----------------|-----|
| `Device or resource busy` | Something's CWD is in the mount | `cd` out, retry. The script falls back to lazy unmount. |
| `Transport endpoint is not connected` | Ghost mount from a killed daemon | `pgfs.sh down` sweeps it. |
| `failed to connect to Postgres` | Postgres is down or the cluster was wiped | `pgfs.sh up` auto-starts it. |
| `mount failed: option allow_other only allowed...` | Missing `/etc/fuse.conf` line | See §3.3. |
| `permission denied` on `init_db.sh` | Script not executable | `chmod +x scripts/*.sh`. |

---

## 7. The storage model: everything is one table

No schemas with foreign keys. No inode tables with back-references.
Just one table:

```sql
CREATE TABLE entries (
    parent text NOT NULL,      -- full path of the containing directory; '' = root
    name   text NOT NULL,      -- entry name (never contains '/')
    kind   text NOT NULL,      -- 'file' | 'dir'
    data   bytea NOT NULL,     -- the bytes
    size   bigint NOT NULL,    -- cached size in bytes
    mtime  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (parent, name)
);
```

The tree is position-addressed: `('', 'docs')` is the root-level `docs/`
directory; `('docs', 'notes.txt')` is the file inside it. Inode numbers
are **not stored** — `fs.rs` allocates them in memory and translates every
kernel callback from `ino` back to a `(parent, name)` pair. There is no
separate "directory" concept beyond a row with `kind = 'dir'`; a directory
"contains" entries whose `parent` column matches its path. That's the whole
model.

### Rename: `mv` with teeth

Rename follows POSIX semantics, and it's one of the most carefully tested
paths in the codebase. Every rename happens inside a single database
transaction: it either completes atomically or nothing changes.

| You do this | What pgfs does |
|-------------|---------------|
| `mv a.txt b.txt` (new name) | `UPDATE` the row's `(parent, name)` |
| `mv a.txt b.txt` (overwrite) | `DELETE` target, `UPDATE` source — one transaction |
| `mv src/ dest/` (new dir) | `UPDATE` the dir row, then rewrite the `parent` column of every descendant |
| `mv src/ dest/` (overwrite empty dir) | Same cascade, target atomically replaced |
| `mv src/ dest/` (dest not empty) | `ENOTEMPTY` — kernel gets the expected error |
| `mv file.txt dir/` | `EISDIR` — can't rename a file over a directory |
| `mv dir/ file.txt` | `ENOTDIR` — can't rename a directory over a file |
| `mv a.txt a.txt` | No-op, detected before touching the database |
| `mv dir/ dir/sub/` | `EINVAL` — can't move a directory into itself |

After the rename, the in-memory inode⇄path maps are re-keyed so existing
inode numbers stay valid. The kernel never notices.

---

## 7.5 Replicated mode: a filesystem with a standby

pgfs can run against a **primary Postgres plus a physical streaming
standby in Docker** (`spec/replica.py`). Writes always go to the primary;
reads are served from the standby when it has caught up with the
primary's WAL, and silently fall back to the primary when it hasn't.

### Bring up the standby

```bash
./scripts/replica_db.sh up      # provision + start the Docker standby
#   postgres:17 container, pg_basebackup'd from the primary,
#   streaming WAL, listening on 127.0.0.1:5433
./scripts/replica_db.sh status  # container / recovery / replay-lag
./scripts/replica_db.sh conn    # print the --replica connection string
./scripts/replica_db.sh down    # stop the container (data volume kept)
./scripts/replica_db.sh wipe    # stop + delete the volume
```

Requires docker and the `postgres:17` image (same major version as the
Nix primary — `pg_basebackup` refuses cross-version replication). The
primary is restarted to listen on TCP loopback (`-h 127.0.0.1`) so the
WAL sender can stream; it never binds a non-loopback interface.

### Mount with a replica

```bash
./scripts/pgfs.sh down
./scripts/pgfs.sh up -- ... # or directly:
target/debug/pgfs testdata/mnt \
  --replica "$(./scripts/replica_db.sh conn)"
```

With `--replica`, reads route like this:

| Operation | Reads from | Why |
|-----------|-----------|-----|
| `lookup` / `getattr` / `read` / `readdir` | **replica when fresh**, else primary | pure reads |
| `create` / `mkdir` EEXIST checks | primary | mutation decision |
| `write` read-modify-write | primary | patching stale bytes would lose data |
| `rename` source/target/emptiness checks | primary | decision reads feed a transaction |
| `setattr` truncate + reply attrs | primary | post-write truth the kernel caches |
| every write | primary | the replica is never written |

"Fresh" means the standby's `pg_last_wal_replay_lsn()` is at or ahead of
the primary's `pg_current_wal_lsn()` — checked per read. A standby that
is down, unreachable, or lagging degrades to a single-node mount with no
kernel-visible error (`metrics|replica|reads=.. fallbacks=..` in the
60-second export tells you the split).

---

## 8. Under the hood: implementation notes

The architecture diagram, the two namespaces, and the seam discussion
live in §2; this section collects the practical details you'll touch
when hacking.

Supporting cast:
- **`error.rs`** — `#[track_caller]` context chaining so every failure
  prints a breadcrumb trail from root cause to the FUSE callback that
  triggered it.
- **`metrics.rs`** — lock-free atomic counters, latency histograms,
  periodic export, liveness heartbeat.

### Things worth knowing before you hack

- After the first `lookup`, the kernel talks **only in inode numbers**.
  The in-memory `ino_by_path` / `path_by_ino` maps are where all the
  interesting state lives.
- Writes use a **whole-blob read-modify-write**. Correct, simple, and
  the obvious first target when performance matters (§11).
- A 1-second TTL on attributes means the kernel batches `getattr`/`lookup`
  — Postgres isn't hammered on every syscall.
- FUSE dispatches single-threaded by default, which is why the in-memory
  maps need no locks. Multi-threaded mounts want a `Mutex` (and a
  connection pool).

---

## 9. Observability: what the daemon tells you

pgfs doesn't just run — it *reports*. Every FUSE callback, every database
query, every unexpected failure is instrumented. You can see what it's
doing, how fast, and why it failed, all without a debugger.

### Structured tracing

Two layers of spans, nested so DB work is automatically attributed to the
FUSE callback that triggered it:

| Layer | Level | What you see |
|-------|-------|-------------|
| FUSE callback (`lookup`, `read`, `write`, ...) | `INFO` | `ino`, `name` |
| DB method (`getattr`, `read`, `write`, ...) | `DEBUG` | `parent`, `name` |

Filter with the familiar `RUST_LOG` syntax:

```bash
RUST_LOG=pgfs=info                           # just lifecycle events
RUST_LOG=pgfs::fs=trace,pgfs::db=debug      # targeted deep dive
```

For machines, flip on JSON:

```bash
RUST_LOG_FORMAT=json ./scripts/pgfs.sh run 2>&1 | jq '.fields.message'
```

Newline-delimited JSON with `timestamp`, `level`, `target`, `fields`, and
`spans` — ready for `jq`, Elasticsearch, or your log aggregator of choice.

### The metrics heartbeat: every 60 seconds

Every 60 seconds, the daemon dumps a snapshot of its entire operational
state at `INFO` level. This is your dashboard, delivered to the log.

**13 operation counters**, one per FUSE callback: `lookup`, `getattr`,
`setattr`, `read`, `write`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`,
`readdir`, `open`, `fsync`.

**7 error counters**: `EIO` (unexpected — database trouble or invariant
failure) plus the expected errnos: `ENOENT`, `EEXIST`, `ENOTEMPTY`,
`EISDIR`, `ENOTDIR`, `EINVAL`. A spike in `ENOENT` is a busy user. A spike
in `EIO` is a call to action.

**Latency histograms** with buckets at ≤10μs, ≤100μs, ≤1ms, ≤10ms,
≤100ms, ≤1s, and >1s. Two histograms:
- `FUSE_LATENCY` — how long callbacks take wall-clock.
- `DB_LATENCY` — Postgres round-trip time for every `Db` method.

Example output from the 60-second export:

```
metrics|ops|lookup=1523 getattr=8901 read=342 write=67 ...
metrics|errors|eio=0 enoent=12 eexist=0 enotempty=1 ...
metrics|latency|fuse=total=1234 p50=<=10μs p95=<=100μs p99=<=1ms
metrics|latency|db=total=1200 p50=<=100μs p95=<=1ms p99=<=10ms
```

### Health check: is it alive?

On mount, the daemon drops two files under `/tmp/`:

| File | What it means |
|------|--------------|
| `/tmp/pgfs-{hash}.pid` | Here's my PID. |
| `/tmp/pgfs-{hash}.ready` | I am serving FUSE requests. Poll me. |

Both are removed on clean shutdown via RAII guards. The `{hash}` is a
stable digest of the mountpoint path, so repeated mounts produce the
same filenames.

A background thread bumps a liveness counter every 10 seconds. Every FUSE
callback observes the counter on entry; if it has not advanced within
30 seconds (3×N), the callback logs `ERROR`, flags a watchdog thread, and
the daemon unmounts cleanly (the watchdog retries, then exits so
AutoUnmount finishes the job). An idle mount is never falsely flagged: the
check only fires when callbacks are flowing *and* the heartbeat has
stalled. Note the coverage: it detects a dead heartbeat while the
filesystem is in use — if the FUSE dispatch loop itself is completely
wedged, callbacks stop running and `AutoUnmount` plus the process-level
liveness guards in the supervisor handle that case.

### Signals: tap the daemon on the shoulder

All signals are handled on a dedicated thread so they never block FUSE
dispatch:

| Signal | Effect |
|--------|--------|
| `SIGINT` / `SIGTERM` / `SIGHUP` | Clean unmount, graceful exit |
| `SIGUSR1` | State dump on next callback: inode cache size, metrics snapshot, uptime |
| `SIGUSR2` | Toggle CPU profiling (needs `--features profiling`) |

```bash
kill -SIGUSR1 $(cat /tmp/pgfs-*.pid)     # ask for a state report
tail -f testdata/pgfs.log                 # watch it arrive
```

### When it panics

If the daemon panics, it doesn't just die silently. A custom panic hook
fires off a forensic report to **both** stderr and the `tracing::error!`
channel: the panic message, `file:line`, a symbolized backtrace, and
OS/Rust/environment metadata (pgfs version, OS/arch, the rustc it was
built with, `RUST_BACKTRACE` setting). In `--release` builds,
`panic = "abort"` prevents a broken daemon from lingering. The report is
written before the process exits.

### Profiling: see where the time goes

Rebuild with `--features profiling` to unlock on-demand profiling. Both
tools are zero-overhead when the feature flag is off (the default).

- **CPU flamegraphs** (`pprof` crate): SIGUSR2 starts sampling at 997 Hz.
  A second SIGUSR2 stops it and writes two files:
  `/tmp/pgfs-profile-{timestamp}.svg` (inferno flamegraph) and
  `/tmp/pgfs-profile-{timestamp}.stacks` (per-stack sample dump).
  (The `.pb` protobuf codec is deliberately not enabled.)

- **Span flamegraphs** (`tracing-chrome` crate): writes a Chrome trace
  file to `/tmp/pgfs-trace-{timestamp}.json` on shutdown. Open it in
  `chrome://tracing` or Perfetto to see every FUSE callback → DB query →
  SQL round-trip with nanosecond timestamps.

```bash
cargo build --features profiling --release
./scripts/pgfs.sh up
kill -SIGUSR2 $(cat /tmp/pgfs-*.pid)   # start
# ... hammer the filesystem ...
kill -SIGUSR2 $(cat /tmp/pgfs-*.pid)   # stop, flamegraph written
```

### The log level contract

| Level | Reserved for |
|-------|-------------|
| `ERROR` | Database down. Invariant violated. Panic. Something a human needs to know about. |
| `WARN` | Degraded but coping — couldn't write a health file, signal set partially failed. |
| `INFO` | Lifecycle: mounted, unmounted, signal received, 60s metrics dump. |
| `DEBUG` | Per-operation: FUSE callback entered, DB query dispatched. |
| `TRACE` | Byte-level detail. Not used by default. |

Expected conditions — `ENOENT`, `EEXIST`, `ENOTEMPTY` — are **never
logged**. They're not errors; they're Tuesday. Only unexpected failures
reach `ERROR`.

---

## 10. Development workflow

```bash
nix develop                      # or direnv, or your own setup
cargo check                      # fast compile check
cargo build
cargo clippy                     # keep the linter happy
make test                        # Rust unit tests
make test-integration            # full FUSE integration suite
./scripts/pgfs.sh up             # boot the whole stack
# ... experiment in testdata/mnt ...
./scripts/pgfs.sh down
```

### 10.1 The test suites

pgfs is tested the way real filesystems are. ext4's canonical suite is
[xfstests](https://git.kernel.org/pub/scm/fs/xfs/xfstests-dev.git/)
(now "fstests") — numbered shell scripts under `tests/generic/` that mount
the fs, exercise one POSIX behavior, and diff against golden `.out` files.
OpenZFS tests the same way with its own zfs-tests suite
(`tests/zfs-tests/tests/functional/<group>/<test>.ksh`) plus xfstests in
CI. pgfs can't run either suite verbatim — they assume hardlinks,
symlinks, chmod/chown, xattrs, mmap, and their own C helper binaries,
none of which this filesystem implements. So the adoptable parts are the
*test patterns*, ported to plain shell so they run against a real mount:

| Test script | Adopted from | What it proves |
|-------------|-------------|----------------|
| `test_truncate.sh` | xfstests `generic/014` (truncfile) | truncate down preserves bytes; truncate up zero-fills; a 10000-byte file can be re-truncated across a size sweep with no corruption |
| `test_append.sh` | ZFS `append/file_append.ksh` | O_APPEND writes always land at EOF; appends never clobber earlier data; appended blocks read back at their offset |
| `test_integrity.sh` | xfstests `generic/001` | create/write/unlink chains across many sizes and depths; head/tail of each chain must `cmp` identical |
| `test_dirs.sh` | ZFS mkdir/rmdir functional tests | `mkdir -p` at arbitrary depth; readdir listing; rmdir rejects non-empty (ENOTEMPTY) and missing (ENOENT) dirs |
| `test_rename.sh` | (project original) | POSIX rename-overwrite rules (file→file, dir→dir, EISDIR, ENOTDIR, ENOTEMPTY) |
| `test_git_init.sh` | (project original) | `git init` end-to-end through the whole syscall surface |

Each test sources `scripts/test_lib.sh` — the same job xfstests' `common/rc`
or ZFS's `libtest.shlib` does: anchors the project root, brings up
Postgres if needed, mounts pgfs at `testdata/mnt`, and provides `pass` /
`fail`. `scripts/run_tests.sh` runs every `scripts/test_*.sh` in
isolation and prints a pass/fail summary — the shape of `./check -g quick`.
Logs land in `testdata/test_<name>.log`.

The "did I break everything?" checklist:

```bash
./scripts/pgfs.sh status
pg_ctl -D testdata/pgdata status
psql -h "$(pwd)/testdata" -d pgfs -c '\dt'
tail -f testdata/pgfs.log
./scripts/run_tests.sh
```

---

## 11. What's not here yet

Every project has its "deliberately not now" list:

- **Chunked storage.** Whole-blob writes are fine for small files. The plan
  is a `blocks (path, block_no, data)` table for partial reads/writes.
- **Permissions.** 0644 for files, 0755 for dirs, owned by you. No
  `chmod`/`chown`.
- **Multi-threaded mounts.** Works great single-threaded. Adding threads
  means a `Mutex` on the inode maps and a connection pool for Postgres.
- **Search, indices, block-device path.** All future layers on the same
  `db.rs` ⇄ `fs.rs` seam.

That's the complete list — observability (FUSE latency on every callback,
liveness deadlock detection, SIGUSR1 state dumps, panic forensics) is
fully wired. If something in this manual's observability section doesn't
happen in practice, that's a bug.

---

## 12. Clean slate

```bash
./scripts/pgfs.sh down
pg_ctl -D testdata/pgdata stop
rm -rf testdata
```

Next `./scripts/pgfs.sh up` starts from a newborn cluster with an empty
`entries` table. Fresh as the day it was initialized.

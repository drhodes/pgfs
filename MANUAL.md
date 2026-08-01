# pgfs — Manual

**A filesystem backed by PostgreSQL.** `pgfs` is a FUSE filesystem whose
directories and file contents are rows in a Postgres database. This manual
covers setup, the development workflow, the mount lifecycle, how it works
under the hood, and how to debug it when something's off.

---

## 1. What pgfs is

Point a FUSE mount at a directory, and ordinary filesystem operations become
database operations:

| You type              | pgfs does in Postgres                          |
| --------------------- | ---------------------------------------------- |
| `ls` / `find`         | `SELECT ... FROM entries WHERE parent = $1`    |
| `mkdir docs`          | `INSERT ... VALUES ('', 'docs', 'dir')`        |
| `echo hi > f.txt`     | `INSERT`/`UPDATE` a `bytea` blob               |
| `cat f.txt`           | `SELECT data FROM entries WHERE parent+name`   |
| `rm f.txt`            | `DELETE FROM entries WHERE parent+name`        |

The point is the direction of travel: instead of files living on a disk,
they live in a database — you can query them with SQL, replicate them, back
them up, and reason about them with everything Postgres gives you.

This build is intentionally minimal to prove the path end to end:

- arbitrary directory nesting (`mkdir -p a/b/c`),
- whole files stored as `bytea` blobs (no chunking),
- no search, indices, or permissions model.

## 2. Requirements

### 2.1 Nix (recommended, pinned)

`flake.nix` pins everything: a modern stable Rust toolchain, the PostgreSQL
server binaries (`initdb`, `pg_ctl`, `psql`, `createdb`, `pg_ctl`), FUSE3
with its `pkg-config` metadata for the `fuser` crate's build script, plus
debugging and dev tools (`gdb`, `lldb`, `git`, `jq`, `rustfmt`, `clippy`).

```bash
nix develop
```

You'll see the environment banner with the pinned versions. With direnv
installed, the checked-in `.envrc` (`use flake`) enters this shell
automatically whenever you `cd` in.

The `shellHook` also exports `PGFS_ROOT`, `PGFS_TESTDATA`, `PGFS_PGDATA`,
and `PGFS_SOCKDIR` so scripts and tooling have stable anchors.

### 2.2 Non-Nix fallback

If you skip Nix, you need, on your `PATH`:

- **Rust** — stable 1.85+ (the current crates.io ecosystem requires it;
  prefer `rustup` over an old distro rust).
- **PostgreSQL server binaries** — `initdb`, `pg_ctl`, `psql`, `createdb`.
  Debian/Ubuntu hide these under `/usr/lib/postgresql/<version>/bin`; add
  that to `PATH`.
- **libfuse3 dev headers** — `libfuse3-dev` + `pkg-config` on Debian/Ubuntu,
  `fuse3-devel` + `pkgconf-pkg-config` on Fedora, `fuse3` + `pkgconf` on
  Arch.

### 2.3 The one system config: `/etc/fuse.conf`

The mount uses `auto_unmount`, so the kernel removes the mount whenever the
daemon dies — for **any** reason: clean exit, `kill -9`, a crash. No ghost
mounts, ever. That feature is a `fusermount3` capability that requires
`user_allow_other`:

```
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

Without this, mounting fails with
`fusermount3: option allow_other only allowed if 'user_allow_other' is set`.

### 2.4 Project-local Postgres, not system Postgres

Nothing here touches a system-wide Postgres. The cluster is created under
`testdata/pgdata`, listens **only** on a Unix socket in `testdata/`
(`-h ''` blocks TCP entirely), and trusts connections on that socket.
`rm -rf testdata` is a complete clean slate.

## 3. Quick start

```bash
nix develop                                  # enter the pinned environment
./scripts/pgfs.sh up                         # Postgres + build + mount
```

`up` is idempotent and **waits until the mount is genuinely live** in
`/proc/mounts` before returning. Then, in any terminal:

```bash
# write, read, delete
echo "hello" > testdata/mnt/hello.txt
cat testdata/mnt/hello.txt
rm testdata/mnt/hello.txt

# nested directories
mkdir -p testdata/mnt/docs/sub
echo "notes" > testdata/mnt/docs/sub/notes.txt

# filesystem ergonomics
ls -la testdata/mnt
find testdata/mnt -type f
stat testdata/mnt/docs/sub/notes.txt
cp testdata/mnt/docs/sub/notes.txt testdata/mnt/copy.txt
echo "more" >> testdata/mnt/docs/sub/notes.txt   # appends at EOF
truncate -s 100 testdata/mnt/docs/sub/notes.txt  # truncate via setattr
touch testdata/mnt/new.txt                       # create + setattr

# the reason this project exists: git init works against it
git init testdata/mnt/proj

# empty directories enforce the rules
rmdir testdata/mnt/docs/sub          # fails: ENOTEMPTY while notes.txt is inside
rm testdata/mnt/docs/sub/notes.txt
rmdir testdata/mnt/docs/sub          # succeeds now that it's empty
```

Shut down:

```bash
./scripts/pgfs.sh down     # unmounts cleanly, removes the mount
```

`pgfs.sh status` reports mount / daemon / Postgres state at any time.

## 4. Lifecycle control: `scripts/pgfs.sh`

One script owns the whole lifecycle, so you never need to remember
`fusermount3` incantations.

| Command | What it does |
| ------- | ------------ |
| `./scripts/pgfs.sh up` | Ensure Postgres is running, build, sweep any stale mount, start the daemon (pid file + log), **wait and verify** the mount is live, then report versions. Idempotent. |
| `./scripts/pgfs.sh down` | Signal the daemon, wait for the mount to disappear, then verify. Escalates automatically: clean unmount → `fusermount3 -u` → (if busy) report holders via `fuser` → lazy unmount. Idempotent. |
| `./scripts/pgfs.sh status` | Show mount/daemon/Postgres state. Flags a ghost mount if it ever sees one. |
| `./scripts/pgfs.sh run` | Run pgfs in the foreground (for debugging). Ctrl+C unmounts cleanly. |
| `PGFS_MOUNT=/some/dir ./scripts/pgfs.sh up` | Override the mount point. |

Artifacts it manages under `testdata/`:

```
testdata/pgdata      Postgres cluster (created by init_db.sh)
testdata/pgfs.pid    daemon pid file
testdata/pgfs.log    daemon log
testdata/postgres.log  Postgres log
testdata/mnt         default mount point
```

Postgres itself is intentionally separate: `scripts/init_db.sh` creates and
starts the cluster (idempotent), and
`pg_ctl -D testdata/pgdata stop` stops it. `pgfs.sh up` auto-starts it if
it isn't running; `down` leaves it up.

## 5. Reliability model: why mounts never ghost

Three layers of defense, all exercised:

1. **`AutoUnmount`** — the kernel unmounts when the daemon process exits,
   for *any* reason. A `kill -9` leaves no mount behind. This is the load
   bearing layer; it's why `down` needs no heroics.
2. **Signal handling** — pgfs blocks `SIGINT`/`SIGTERM`/`SIGHUP`
   process-wide and waits on them in a dedicated thread
   (`sigwait`). On receipt it unmounts cleanly and `run()` returns normally
   instead of the process dying abruptly.
3. **Script escalation** — `down` treats the mount as the source of truth:
   it polls for the mount to disappear, and if anything is left (say, a
   process has its CWD inside the mount and blocks unmounting), it reports
   the holders and falls back to a lazy unmount rather than failing
   mysteriously.

Common failure modes and their answers:

| Symptom | Cause / fix |
| ------- | ----------- |
| `Device or resource busy` on unmount | Some process has its CWD inside the mount. `cd` out of it (or any shell/daemon's CWD), then retry `down`. The script auto-falls-back to lazy unmount. |
| `Transport endpoint is not connected` | The mount is a ghost left by an old, pre-`AutoUnmount` build or an external unmount. `./scripts/pgfs.sh down` sweeps it. |
| `failed to connect to Postgres` | Postgres isn't running or the cluster was wiped. Run `./scripts/pgfs.sh up` (or `./scripts/init_db.sh`). |
| `mount failed: ... only allowed if 'user_allow_other'` | `/etc/fuse.conf` needs `user_allow_other` (see §2.3). |
| `permission denied` on `./scripts/init_db.sh` | Script isn't executable — `chmod +x scripts/*.sh`. |

## 6. Storage model

Everything is one table:

```sql
CREATE TABLE entries (
    parent text NOT NULL,      -- full path of the containing directory; '' = root
    name   text NOT NULL,      -- entry's own name (never contains '/')
    kind   text NOT NULL,      -- 'file' | 'dir'
    data   bytea NOT NULL,     -- file bytes
    size   bigint NOT NULL,    -- cached size in bytes
    mtime  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (parent, name)
);
```

The tree is just rows keyed by position: `('', 'docs')` is the root-level
`docs` dir, `('docs', 'notes.txt')` is the file inside it. FUSE inode
numbers are **not** stored — `fs.rs` allocates them in memory (root = 1)
and translates every kernel callback from `ino` back to a `(parent, name)`
key via its own maps. Nothing about the tree structure is stored anywhere
except these rows.

## 7. Architecture

```
                 ┌──────────────┐
 kernel syscalls │    fs.rs     │  speaks FUSE only
   (inode-based) │  ino ⇄ path  │  never constructs SQL
                 └──────┬───────┘
                        │ method calls: read / write / create / mkdir / ...
                 ┌──────▼───────┐
                 │    db.rs     │  speaks SQL only
                 │  (parent,name)│  never touches FUSE types
                 └──────┬───────┘
                        │ libpq over Unix socket
                 ┌──────▼───────┐
                 │ Postgres 17  │  cluster in testdata/pgdata, -h ''
                 └──────────────┘
```

The `db.rs` ↔ `fs.rs` boundary is the deliberate seam: `db.rs` exposes plain
methods and owns the schema; `fs.rs` owns FUSE semantics (inode mapping,
entry TTLs, reply types) and the kernel protocol. They meet only through
method calls, so future layers — chunked blocks, search, indices — slot into
`db.rs` without touching the FUSE plumbing.

FUSE specifics to know when you dig in:

- After the first `lookup`, the kernel talks **only in inode numbers**; all
  the interesting state is the `ino_by_path` / `path_by_ino` maps.
- **Whole-blob writes** — `write` reads the full file, patches the byte
  range in memory, and writes the whole file back. Correct, simple, and the
  obvious next performance target (§9).
- **TTL caching** — attributes/entries are served with a 1-second TTL, so
  the kernel batches `getattr`/`lookup` calls and Postgres isn't hammered
  per syscall.
- **Single-threaded** — `fuser`'s default mount dispatches one request at a
  time, which is why the in-memory maps need no lock. Going multi-threaded
  means adding one.

## 8. Development workflow

```bash
nix develop                    # or rely on direnv
cargo check                    # fast type-check
cargo build
cargo clippy                   # keep it clean
./scripts/pgfs.sh up           # boot the whole thing
# ... mess around in testdata/mnt ...
./scripts/pgfs.sh down
```

Troubleshooting from scratch (the "did everything break?" checklist):

```bash
./scripts/pgfs.sh status
pg_ctl -D testdata/pgdata status
psql -h "$(pwd)/testdata" -d pgfs -c '\dt'          # see the schema
tail -f testdata/pgfs.log                           # daemon logs
```

## 9. Roadmap / deliberate omissions

- **Rename (`mv`).** Not implemented. Because directories are keyed by
  their full path, moving a directory means rewriting every descendant's
  `parent` in one transaction. Straightforward; just not wired up yet.
- **Chunked storage.** Whole-blob rewrites are fine for small files. The
  plan is a `blocks (path, block_no, data)` table for efficient partial
  reads/writes.
- **Permissions.** Files report 0644, dirs 0755, owned by the running user.
  No `chmod`/`chown`.
- **Multi-threaded mounts.** The in-memory inode maps need a `Mutex` (and
  the single Postgres `Client` needs a pool) before that's safe.
- **Search / indices / block-device path.** Later layers on the same seam.

## 10. Clean slate

The whole local database lives under `testdata/`:

```bash
./scripts/pgfs.sh down
pg_ctl -D testdata/pgdata stop
rm -rf testdata
```

Next `./scripts/pgfs.sh up` starts from a fresh cluster with an empty
`entries` table.

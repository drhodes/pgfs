# pgfs — The Database That Pretends to Be a Filesystem

**Your files. In Postgres. Because why not?**

`pgfs` is a FUSE filesystem where every directory, every file, every
`echo "hello" > foo.txt` becomes rows in PostgreSQL. You can `SELECT`
your directory tree, snapshot a whole tree in constant time, branch a
writable copy of it, and publish the branch back atomically. The kernel
thinks it's talking to a disk; really it's talking to Postgres.

This manual is the friendly tour. The reference for every behaviour is
the specification in `spec/` and the documentation generated from it in
`docs/` (tutorials, how-to guides, reference and explanation for each
feature).

---

## 1. The idea, in one breath

```
You type             →  pgfs runs, in one transaction each
────────────────────────────────────────────────────────────────
ls                   →  SELECT … FROM pgfs.readdir(layer, epoch, dir, after, 256)
mkdir docs           →  SELECT … FROM pgfs.mkdir(…)
echo hi > f.txt      →  pgfs.create(…), then pgfs.write_blocks(…) at close
mv a.txt b.txt       →  pgfs.rename(…)   -- one dentry row moves
fsync(fd)            →  pgfs.write_blocks(…, sync => true), or pgfs.sync_barrier(…)
pgfs snapshot main@x →  pgfs.snapshot('main', 'x')  -- two rows, any tree size
```

A name is a `dentries` row `(parent_ino, name) → ino`. A file is an
`inodes` row with a permanent number and a random generation. Small
files live inside their inode; larger ones are a block map pointing at
immutable, CRC32C-checksummed blobs, which copies, snapshots and
branches share.

---

## 2. Architecture

```
  kernel ──FUSE──▶ dispatcher ──routes by inode or name──▶ worker 0..N
                                                            │ owns: handles, write buffers,
                                                            │       one Postgres connection
                                                            ▼
                                                  pgfs.* SQL functions
                                                  (sql/v1/, SECURITY DEFINER)
```

- **Workers.** Requests on one inode go to `ino mod workers`; requests
  on one name to `hash(parent, name) mod workers`. One worker owns each
  inode's handles and buffers, so nothing needs a lock. A worker that
  needs another's buffers committed (before a rename, say) asks by
  message.
- **Buffers.** Writes collect per inode and commit as whole blocks at
  close, fsync, a rename, or under memory pressure. A partly written
  block is read and overlaid first.
- **Leases.** A read-write mount holds the tree's lease in `pgfs.mounts`
  and a fence number; every mutation checks it. Read-only and snapshot
  mounts register sessions.
- **Background threads.** Lease renewal, reconnect and revalidation,
  housekeeping (usage, reclaim, GC, change pruning, auto-flatten), the
  control listener (owner-drained snapshots), the in-flight watchdog,
  the signal thread and the metrics exporter.

The source map: `src/fs.rs` (FUSE and workers), `src/pg.rs` (the
backend), `src/conn.rs` (connections), `src/mount.rs` (the daemon),
`src/cli.rs` (commands), `sql/v1/` (the schema), `tests/model/` (the
reference model).

---

## 3. Getting set up

### 3.1 Nix (the easy way)

```bash
nix develop
```

`flake.nix` pins the Rust toolchain, PostgreSQL 17 and FUSE 3.

### 3.2 No Nix?

You need Rust stable, the PostgreSQL 17 server binaries (`initdb`,
`pg_ctl`, `psql`, `createdb`) and `fusermount3`.

### 3.3 The one system line

The mount uses `auto_unmount`, so the kernel removes it whenever the
daemon dies. That needs:

```bash
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

pgfs still refuses other users unless you pass `--allow-other`.

### 3.4 Your own Postgres

Nothing touches a system-wide Postgres. `scripts/init_db.sh` creates a
cluster in `testdata/pgdata` with data checksums, a Unix socket in
`testdata/` and TCP on 127.0.0.1 only, and formats the `pgfs` database
with `pgfs mkfs`. `rm -rf testdata` is a complete reset.

---

## 4. Five minutes to `ls` in a database

```bash
./scripts/pgfs.sh up
cd testdata/mnt

echo "hello" > notes.txt
mkdir -p a/b/c && touch a/b/c/deep.txt
git init proj && cd proj && git commit --allow-empty -m first && cd ..

psql -h "$OLDPWD/testdata" -d pgfs -c \
  "SELECT path, ino, kind, size FROM pgfs.files WHERE tree = 'main' ORDER BY path"
```

Then freeze it, branch it, change the branch, and publish:

```bash
cd ../..
./target/debug/pgfs snapshot main@first
./target/debug/pgfs branch main experiment
./scripts/pgfs.sh down
mkdir -p /tmp/exp && ./target/debug/pgfs mount experiment /tmp/exp &
echo "changed" > /tmp/exp/notes.txt; kill -TERM %1; wait
./target/debug/pgfs promote experiment main
./target/debug/pgfs diff main@first main
```

---

## 5. One script: `pgfs.sh`

```bash
./scripts/pgfs.sh up       # Postgres + mkfs if needed + build + mount, wait until live
./scripts/pgfs.sh down     # SIGTERM (commits buffers), then escalate to fusermount3 -u, then lazy
./scripts/pgfs.sh status   # mount, daemon and Postgres state, ghost mounts flagged
./scripts/pgfs.sh run      # foreground; Ctrl+C quits cleanly
```

| Path | What lives there |
|---|---|
| `testdata/pgdata` | the cluster |
| `testdata/mnt` | the mount point (`PGFS_MOUNT` overrides) |
| `testdata/pgfs.log` | the daemon log |
| `/tmp/pgfs-<hash>.pid`, `.ready` | health files |

---

## 6. Why your mounts never ghost, and your data survives

- **AutoUnmount.** The kernel removes the mount when the daemon dies,
  for any reason.
- **Clean shutdown.** SIGTERM commits every buffer, runs a synchronous
  barrier so earlier asynchronous commits are on disk, unmounts, and
  releases the lease.
- **Durability points.** `fsync` returns once the data is durable on
  the primary and any synchronous standbys. Everything else becomes
  durable within the asynchronous commit window (three times
  `wal_writer_delay`, 600 ms by default), always as a prefix of
  history: write-then-rename is atomic without fsync.
- **Reconnect and revalidate.** If Postgres restarts, new requests fail
  fast with EIO, the in-flight one waits, and the mount reconnects and
  checks the database is the same one with the same history before
  serving again. If an acknowledged commit was lost, it fences itself.
- **Takeover.** `pgfs mount --takeover` waits for a dead mount's lease
  to expire and reclaims its orphans.

| Symptom | What's happening | Fix |
|---|---|---|
| `Device or resource busy` | something's cwd is in the mount | `cd` out; `pgfs.sh down` falls back to lazy unmount |
| `mounted read-write by HOST pid PID (EBUSY)` | another mount holds the lease | unmount it, or `--takeover` after it died |
| `the database has no pgfs filesystem` | unformatted database | `pgfs mkfs` |
| `option allow_other only allowed …` | missing `/etc/fuse.conf` line | §3.3 |

---

## 7. Snapshots, branches, and the change feed

```bash
pgfs snapshot main@nightly                 # constant time, drains the owner's buffers
pgfs mount main@nightly /mnt/nightly       # read-only, any number of mounts
pgfs branch main@nightly:/projects/web job-17   # a writable subtree
pgfs promote job-17 main                   # fold back atomically (ESTALE if main moved)
pgfs changes main --follow                 # JSON lines, in causal order
pgfs diff main@nightly main                # from epochs, after the feed's retention too
pgfs tree flatten job-17                   # detach a deep branch, online
```

Snapshots record an epoch; branches are layers over a frozen base. Read
[Branchable Trees](docs/explanation/branchabletrees.md) for how.

---

## 8. Replicated mode

```bash
./scripts/replica_db.sh up
./target/debug/pgfs mount main testdata/mnt --replica "$(./scripts/replica_db.sh conn)"
```

Reads go to the standby only once it has replayed the inode's last
write (an LSN ticket); otherwise to the primary. The standby's identity
is checked first, and a standby that is down or behind turns into
primary reads, never errors.

---

## 9. Observability

- `RUST_LOG=pgfs=debug` shows a span per FUSE callback (operation,
  inode, worker, view) with nested database spans (statement, backend
  pid, round trip). `RUST_LOG_FORMAT=json` for machines.
- Every 60 s: `metrics|ops|…`, `metrics|errors|…` (every errno),
  `metrics|replica|…`, `metrics|latency|fuse=…`, `metrics|latency|db=…`,
  `metrics|db|…`, `metrics|inflight|…`, `metrics|space|…`,
  `metrics|integrity|…`.
- `kill -USR1 <pid>` writes a state dump from the signal thread, even
  when every worker is stuck.
- `kill -USR2 <pid>` toggles CPU profiling in a `--features profiling`
  build.
- The watchdog warns about a request older than 5 s and cancels one
  older than 60 s, with a state dump.
- `pgfs stats`, `pgfs check`, `pgfs scrub` from any machine.

---

## 10. Development workflow

```bash
make test               # cargo test: unit, storage and model-conformance tests
cargo test --features fault-injection --test fault_injection
make test-integration   # scripts/test_*.sh, each followed by pgfs check
make bench              # benchmark workloads, JSON in testdata/bench.json
cargo clippy --all-targets -- -D warnings
```

The project follows the libspec workflow: change `spec/` first, run
`uv run libspec diff`, implement in dependency order
(`uv run libspec dependencies --topo`), test, and regenerate `docs/`
with `uv run python scripts/build_docs.py`.

---

## 11. What's not here yet

- FUSE INTERRUPT handling (the fuser crate does not deliver it).
- The LazyFS crash matrix, shuttle linearizability checks, pjdfstest and
  xfstests runs, and a benchmark regression gate.
- Sparse regions in `pgfs export` archives.
- Per-tree quotas, content search, the in-database `pgrx` engine,
  FUSE over io_uring, and the multi-machine (sharded) design.

---

## 12. Clean slate

```bash
./scripts/pgfs.sh down
pg_ctl -D testdata/pgdata stop
rm -rf testdata
```

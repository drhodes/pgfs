# pgfs design decisions

Decisions from the step-by-step design discussion that follows the
spec-versus-literature review (`spec-literature-review.md`). The spec in
`spec/` is written from these decisions.

Topics 1 and 1b were decided in discussion. Topics 2–16 were proposed as
recommendations and approved as a set on 2026-10-29. Paper ids such as
(r128) point to `papers/reading-notes.md`.

## Summary

| # | Topic | Decision | State |
|---|-------|----------|-------|
| 1 | Purpose | Build A first: a single-machine POSIX filesystem with database-grade durability, best at small-file and metadata-heavy work. B, the shared multi-machine filesystem (v0.2), is the goal and builds on A. | decided |
| 1b | Killer feature | Branchable trees: instant read-only snapshots and instant writable copy-on-write branches, with a change feed as the supporting capability. | decided |
| 2 | Durability | The POSIX promise: fsync, fdatasync and O_SYNC are durable on return. Everything else is fast and becomes durable in order. A crash always leaves an earlier moment of history. | decided |
| 3 | Consistency | Linearizable within a mount. One read-write mount per tree, enforced by a lease with fencing. | decided |
| 4 | Data model | Inodes and dentries keyed by a persistent 64-bit ino. Names are bytes. Data lives in immutable, checksummed blobs. Snapshots are epochs; branches are layers. | decided |
| 5 | Atomicity | One FUSE operation is one bounded transaction, run as one server-side SQL function. Retries are made safe by idempotency keys. Multi-file atomicity comes from branch and promote. | decided |
| 6 | POSIX surface | Full single-machine POSIX: hard links, symlinks, xattrs, open-after-unlink, renameat2 flags, holes, real statfs. Each unsupported operation is listed with its errno. | decided |
| 7 | Failure handling | Every wait has a timeout. Database connections follow an explicit state machine with reconnect. A watchdog measures the age of in-flight requests. Errors are tested by injecting them. | decided |
| 8 | Performance | Multi-threaded dispatch sharded by inode. One round trip per operation. Long kernel cache TTLs, which are safe because the mount is the only writer. Named benchmarks gate regressions. | decided |
| 9 | Replication | Standbys serve reads only under an LSN ticket. Snapshots can be read from any standby that has them. Failover durability follows Postgres's synchronous standby setting. | decided |
| 10 | Scale-out | Deferred to after A. A keeps every property v0.2 needs; `shard.py` is rewritten later. | decided |
| 11 | Space and GC | Real statfs and ENOSPC. Unshared blobs are freed in the same transaction; shared ones by a background reference check. Snapshot deletion is reclaimed lazily. Write amplification is measured. | decided |
| 12 | Integrity and backup | End-to-end CRC32C on every blob, `pgfs check`, background scrub. Backup is a Postgres backup, plus export and import of a snapshot. | decided |
| 13 | Security | The kernel enforces permissions. The database role has least privilege. TLS for remote databases. Secrets are never logged. Snapshots are immutable in the database itself. | decided |
| 14 | Verification | A reference model, model conformance with proptest, crash conformance on LazyFS, fault injection, `pgfs check` in CI, plus xfstests and pjdfstest. | decided |
| 15 | Operations | Explicit `pgfs mkfs` and `pgfs migrate`. A stated range of Postgres versions. Server settings checked at mount. One CLI for every administrative action. | decided |
| 16 | Spec writing | Requirements carry labelled contract clauses. Modules are split by concern. The spec states the target system, and feature status notes state the gap. | decided |

## The five real trade-offs

Most of what follows is the consensus in the literature. These five are
real trade-offs, where reasonable people choose differently. Each was
approved as written:

1. **Single writer per tree (3).** Two machines cannot both mount the
   same tree read-write in A. Each can mount its own branch of it. In
   return, the kernel caches are coherent with long TTLs, and a stale
   daemon can never corrupt a tree. B lifts this restriction.
2. **Operations as server-side SQL functions (5).** This gives one round
   trip per operation and an API that the CLI, psql and future daemons
   share. The cost is that some of the logic lives in PL/pgSQL rather
   than in Rust.
3. **Names stored as `bytea`, not `text` (4).** This is byte-exact and
   accepts every legal Unix name. The cost is that ad-hoc SQL needs the
   provided views, or `convert_from`, to read names.
4. **No atime updates by default (6).** The alternative turns every read
   into a write.
5. **Backups are Postgres backups (12).** pgfs adds export and import of
   snapshots but no backup system of its own.

---

## 1. Purpose — decided

Build A first, a general-purpose POSIX filesystem with database-grade
durability. Small-file and metadata-heavy work is the sweet spot. B, a
shared multi-machine filesystem, is the goal, and A is its foundation:
B depends on A being correct.

## 1b. Killer feature — decided

**Branchable trees.**

- `pgfs snapshot main@before-upgrade` freezes a tree in constant time.
- `pgfs branch main job-4711` creates a writable copy in constant time.
  The copy shares all data with its base and writes only the blocks it
  changes.
- `pgfs promote job-4711 main` atomically makes a branch the new
  contents of a tree. It succeeds only if the tree has not changed since
  the branch was taken.

The **change feed** is the supporting capability: an ordered,
resumable stream of every change to a tree. It serves sync, indexing
and backup, and it is the cache-coherence channel B needs.

Rationale: no single-machine filesystem offers cheap writable branches of
arbitrary trees on shared storage. The use case is current (CI, test
sandboxes, development environments, AI agents), and the feature gets
better in B, where many machines can branch from one base.

## 2. Durability — decided

**Decision.**

- **What is durable.** fsync, fdatasync, fsyncdir and writes through an
  O_SYNC or O_DSYNC handle reply only after a commit with
  `synchronous_commit = on`. With synchronous standbys configured, that
  commit also waits for the standbys, so the cluster's policy applies
  without pgfs adding one of its own.
- **Everything else is fast.** Other operations commit asynchronously.
  Postgres flushes them within about three times `wal_writer_delay`
  (600 ms by default). The spec states that window as a number.
- **The crash guarantee.** After a crash of any component, the
  filesystem is in the state produced by a prefix of its committed
  operations, in commit order. The only data that can be missing out of
  order is data that was written but not yet flushed, and every rename,
  link, close, fsync or snapshot of a file flushes it first. So
  write-to-temp-then-rename never yields an empty file, even without
  fsync. Ext4 does not promise this.
- **fsync with nothing pending still waits.** Earlier asynchronous
  commits that touched the file may not be on disk yet, so fsync forces
  the flush even when it has nothing new to write.
- **close and flush** commit the handle's buffers, which makes them
  visible, but not durable. A commit failure is reported to `close()` as
  EIO.
- **A failed fsync** keeps the dirty data, reports the error once to
  each descriptor that wrote, and never reports success for data that
  was not committed (r90).
- **Shutdown.** SIGTERM flushes every handle before unmounting.
- **Server checks at mount.** The mount refuses to start if the server
  runs with `fsync = off` or `full_page_writes = off`, and warns if data
  checksums are off (r135, r94, r120).
- **Scratch mode.** `--unlogged` becomes an explicit scratch mode. The
  whole filesystem is emptied by a server crash, fsync promises nothing,
  the mount says so at start, and it cannot be combined with replicas.
- **Durability points of the feature.** Snapshot, branch and promote
  are durable on return.

**Why.** CrashMonkey (r123) found that most filesystems break the fsync
promise, and CAD (r56) shows that asynchronous commit lets readers see
data that is not yet durable. Skyros (r129): ordering may be deferred,
durability may not. ALICE (r118): applications depend on persistence
properties that nobody states.

**Rejected.**

- Making every operation durable before it replies would make metadata
  work several times slower.
- Today's behaviour, where even fsync is asynchronous, breaks the
  contract.

## 3. Consistency — decided

**Decision.**

- **Within a mount, operations are linearizable.** Every operation sees
  the effects of every operation that completed before it. read,
  getattr, lookup, readdir, rename, setattr, link and copy_file_range
  first merge or flush the pending buffers of the files they touch,
  across all handles (r129, r133).
- **One read-write mount per tree.** Each read-write mount holds a lease
  row in `mounts`. The lease is renewed on a heartbeat and carries a
  fencing epoch, which every mutating transaction checks. A daemon that
  lost its lease, for example after a long pause, cannot write; its
  operations fail with EIO (r04, r21, r73).
- **Read-only mounts.** Unlimited read-only mounts of snapshots are
  allowed. A read-only mount of a live tree may lag by up to its
  attribute TTL, and the spec says that plainly.
- **SQL access.** Reading the database directly is supported through
  stable views. Writing to pgfs tables behind the daemon is unsupported,
  and `pgfs check` detects the damage it causes.
- **Across mounts, B will provide close-to-open consistency** (AFS 05,
  Sprite r40). A does not need it, because each tree has a single
  writer.

**Why.** Every consistency model in the literature is fine; leaving it
unstated is not. A single writer makes the kernel cache exact, which is
also the performance win in topic 8. Fencing is the standard defence
against a zombie writer.

**Rejected.** Several writers on one tree in A. That needs the B
machinery: invalidation channels, leases on both sides and direct I/O
for files that several machines write.

## 4. Data model — decided

**Decision.** The schema is rewritten, not migrated; there is no
production data to preserve.

- **Metadata tables.**
  - `pgfs_meta`: schema version, filesystem UUID, block size and
    creation time.
  - `inodes(layer, ino, …attributes…, inline_data, symlink_target)`.
  - `dentries(layer, parent_ino, name bytea, ino)`, unique on
    `(layer, parent_ino, name)`.
  - `xattrs(layer, ino, name, value)`.
- **Persistent inode numbers.** The FUSE inode number *is* the database
  ino. Inos come from one sequence and are never reused, so they are
  stable across restarts. The root directory of every tree is ino 1.
  The daemon keeps only a cache; `forget` evicts it, so memory is
  bounded (r00, r11, r57).
- **Names are `bytea`.** They are byte-exact, 1–255 bytes, and contain
  no `/` and no NUL. Collation questions disappear, and any legal Unix
  name works (r52). Views present names as text for psql users.
- **Small files live in the inode row.** Files up to a threshold fixed
  at mkfs, about 2 KiB and below Postgres's TOAST threshold, are stored
  inline. A small file is then one row (r07, r38).
- **Data lives in immutable blobs.**
  - `blobs(blob_id, data, len, crc32c, shared)` holds the content.
  - `file_blocks(layer, ino, block_no, blob_id)` maps each file to its
    blobs.
  - A write never updates a blob. It inserts a new blob and moves the
    mapping, so data is always committed before the metadata that
    points to it (CalvinFS r10, BlueStore r33, SwitchΔ r133).
  - The block size is fixed at mkfs, with a default of 64 KiB.
  - Blob data uses lz4 TOAST compression, which is cheap, and wins
    heavily on source trees.
- **Snapshots are epochs** (Petal r37, Oracle FSS r91).
  - Every row carries `born` and `died` epochs, and each layer has a
    current epoch. A snapshot records the current epoch and advances it,
    in constant time.
  - A row born in the current epoch is updated in place. A row from an
    earlier epoch is retired (its `died` is set) and replaced by a new
    row.
  - Live reads filter on `died = ∞` through partial indexes, so a tree
    without snapshots pays almost nothing.
- **Branches are layers** (BetrFS clones r55).
  - A branch is a new layer whose base is a `(layer, epoch)` snapshot.
  - A lookup checks the branch's own rows first, then the base, then the
    base's base. Deletions of inherited entries are recorded as
    whiteouts (both dentry whiteouts with `ino IS NULL` and inode
    whiteouts with `nlink = 0, size = 0, inherit_limit = 0`).
  - A downward truncate lowers `inherit_limit` on the inode to
    `min(inherit_limit, floor(s / block_size))`, so blocks beyond it are
    never inherited even if the file is later extended.
  - Branch depth is capped (8 is proposed). `pgfs tree flatten` copies a
    branch's metadata to give it its own base, without copying data.
- **Trees and views.**
  - A tree is a name that points to a layer.
  - A mount serves exactly one view: a live tree, a snapshot, or either
    one rooted at a directory inside it. Kernel inode 1 maps to the
    view's root.
  - "Branch any subtree" is therefore a branch of the whole tree,
    viewed from that directory.
- **Supporting tables.**
  - `mounts`: leases and fencing epochs.
  - `orphans`: unlinked files that are still open.
  - `op_log`: idempotency keys.
  - `changes`: the change feed.
  - `gc_queue`: blobs waiting for a reference check.

**Why.** The ino-keyed inode/dentry split is the consensus (T1 in the
review). It fixes Bugs 2–4 by construction: directory rename becomes
O(1), there is no `LIKE` prefix match, and the root has a unique
identity. Immutable blobs make snapshots, branches, reflinks and
checksums cheap and safe.

**Rejected.**

- *Copying metadata eagerly on branch.* It is simple, but O(files),
  which takes seconds for large trees rather than staying constant.
- *Layers alone, with a snapshot sealing the current layer.* The chain
  of layers would grow with every snapshot, and reads would slow down
  with it.
- *Reference-counted blocks.* A popular blob becomes a hot counter row,
  and counts drift on bugs. A reference check is simpler (topic 11).
- *Content-addressed deduplication.* It costs a hash on every write.
  The schema leaves room for it later.

## 5. Atomicity — decided

**Decision.**

- **One FUSE operation is one transaction.** No operation commits half
  its effects (r61, r68, r91). Any exception carries a crash clause in
  the spec.
- **Transactions are bounded.** No operation's transaction grows with
  the size of the tree. With ino keys this holds for rename too (r91).
  Housekeeping such as GC, epoch reclaim and orphan cleanup runs in
  separate small transactions.
- **One round trip per operation.** Multi-statement operations (create,
  rename, unlink, truncate, snapshot, branch, promote) are server-side
  SQL functions in a `pgfs` schema, versioned with the schema. The
  daemon, the CLI and future B daemons call the same functions (r82).
- **Idempotency keys.** Every mutation carries `(mount_id, worker_id,
  op_seq)` and overwrites that worker's single slot in `op_log` in the
  same transaction (Viewstamped Replication r28, λFS r20). A retry after
  an ambiguous commit, where the connection dropped during COMMIT, finds
  the earlier result instead of applying the operation twice. Slots are
  removed only when the mount row is released or replaced, so
  idempotency never expires during a long outage.
- **Multi-file atomicity comes from branch and promote,** not from
  filesystem transactions. Work in a branch, then promote it; the
  promote is a compare-and-swap on the tree.

**Rejected.** Exposing transactions across operations to applications
(TxF style). It was deprecated in Windows for lack of use, and long
transactions hold locks and bloat tables.

## 6. POSIX surface — decided

**Supported:**

- **Basic operations.** lookup, getattr, and setattr for mode, uid,
  gid, size, atime and mtime (including UTIME_NOW and UTIME_OMIT); ctime
  is maintained.
- **Creating entries.** create, `O_EXCL`, mkdir, and mknod for regular
  files, FIFOs, sockets and devices.
- **Links.** symlink and readlink. Hard links via link, which the
  dentry split makes natural.
- **Removing and renaming.** unlink and rmdir. rename with
  `RENAME_NOREPLACE` and `RENAME_EXCHANGE`.
- **File I/O.** open, read, write, `O_APPEND`, flush, release, fsync and
  fsyncdir.
- **Directories.** readdir and readdirplus with keyset cookies: every
  entry that exists for the whole listing appears exactly once
  (r57, r131).
- **xattrs** in the `user.`, `trusted.` and `security.` namespaces.
- **Open-after-unlink.** An unlinked file that is still open stays
  readable and writable until its last close. A crash leaves the file in
  `orphans`, and the next mount reclaims it (r68).
- **Holes.** Sparse files, `lseek` with `SEEK_DATA` and `SEEK_HOLE`, and
  `fallocate` with `PUNCH_HOLE`, `KEEP_SIZE` and `ZERO_RANGE`.
- **copy_file_range shares blobs.** A plain `cp` (coreutils 9 and later)
  is an instant reflink. `FICLONE` cannot pass through FUSE, so
  `cp --reflink=always` reports EOPNOTSUPP, and coreutils falls back to
  copy_file_range.
- **statfs** reports real figures (topic 11).
- **Times and limits.** Timestamps have nanosecond resolution, plus a
  birth time. Directory mtime, ctime and nlink are maintained. The
  maximum file size is 2^63−1 bytes.

**Explicitly unsupported**, each with its errno in a table in the spec:

| Feature | Reply |
|---|---|
| POSIX ACLs (`system.posix_acl_*`) | EOPNOTSUPP |
| `O_TMPFILE` | EOPNOTSUPP |
| `RENAME_WHITEOUT` | EINVAL |
| `FALLOC_FL_COLLAPSE_RANGE` and `INSERT_RANGE` | EOPNOTSUPP |

**Behaviour to state:**

- **atime.** It is not updated on read by default; it changes only
  through setattr. A relatime mode may come later.
- **Locks.** flock and POSIX locks are handled by the kernel, which is
  exact for a single-writer mount. B has to revisit this.
- **The kernel writeback cache stays off** until its preconditions are
  specified and tested: the kernel owns mtime and size, writes can
  arrive after release, and `O_APPEND` has to be handled.
- **Every errno** pgfs can return is listed: ENOENT, EEXIST, ENOTEMPTY,
  EISDIR, ENOTDIR, EINVAL, ENAMETOOLONG, EXDEV, EROFS (snapshots),
  ENOSPC (capacity, or Postgres `53100`), EINTR, ETIMEDOUT and EIO.

**Why.** Git, editors, package managers and build tools depend on
chmod, hard links, open-after-unlink, `O_EXCL` lockfiles and rename
flags. pgfs silently ignoring chmod is the worst case: the call succeeds
and nothing happens. Metis (r48) and Monarch (r81) show that
differential testing finds the rest.

## 7. Failure handling — decided

**Decision.**

- **Timeouts.** Every wait has a stated bound: `connect_timeout`,
  `statement_timeout`, `lock_timeout`, `idle_in_transaction` timeout and
  TCP keepalives (r78).
- **A database state machine.** Connected → Broken → Reconnecting →
  Revalidating → Connected. Reconnects use exponential backoff with
  jitter and a cap. While disconnected, operations fail fast with EIO;
  they are not queued. Revalidation checks the schema version, the
  filesystem UUID, the system identifier and timeline, and that this
  mount still holds its lease (r70, r15, r67, r125).
- **Buffered writes survive a reconnect.** Pending data stays in memory
  and is committed after the reconnect. Commits whose outcome is unknown
  are resolved through `op_log`.
- **Bounded retries.** Serialization failures and deadlocks (SQLSTATE
  `40001` and `40P01`) are retried with a bound.
- **Interrupts.** A FUSE INTERRUPT cancels the backend query.
- **Watchdog.** Each dispatch worker stamps the start time of its
  current request. A watchdog thread alarms on the age of the oldest
  in-flight request, dumps state from its own thread, and can cancel the
  query. This replaces the heartbeat check, which cannot see a stuck
  dispatcher (r124).
- **A slow-database signal.** A WARN when p99 latency stays above a
  multiple of the baseline (r88, r122).
- **Error handlers are tested.** A feature-gated wrapper fails the Nth
  database call with each class of error (r119, r121).

## 8. Performance and concurrency — decided

**Decision.**

- **Dispatch.**
  - Dispatch is multi-threaded. Inode operations are sharded by
    `ino mod workers`, single-name operations by
    `hash(parent_ino, name) mod workers` so hot directories fan out
    across workers, and each worker owns one connection, so per-file
    order and write buffers need no locks (uFS r130, r12).
  - Operations that touch several rows lock in one fixed order
    (ascending `ino` for inodes, then ascending `(parent_ino, name)` for
    dentries) using a two-phase discover-sort-lock-revalidate protocol
    (Frangipani 12, HopsFS r11, Chardonnay r66).
  - Directory renames across parents take one advisory lock, the single
    point where loops are checked (r16, r127).
  - No daemon lock is held across a database round trip (r42).
- **Kernel caches.**
  - Entry and attribute TTLs are long by default, because the mount is
    the tree's only writer. Changes the kernel did not see, such as
    promote or repair, require a remount or an explicit invalidation.
  - Negative entries are cached.
  - `FOPEN_KEEP_CACHE` is set when a file is unchanged since its last
    open.
- **Round trips.** One round trip per operation (topic 5), with prepared
  statements per connection.
- **Bounded memory.** Write buffers are capped per handle and per mount,
  with backpressure: when a cap is reached, buffers are flushed rather
  than grown.
- **Physical tuning.** `fillfactor` below 100 on `inodes`, so that
  attribute updates are HOT. `STORAGE` and compression settings on
  blobs. Autovacuum is tuned per table.
- **Benchmarks.** Named workloads:
  - fio, sequential and random;
  - mdtest, for creates and stats;
  - `git clone` and `git status` of a pinned repository;
  - an untar of a kernel tarball;
  - snapshot, branch and promote on a tree with 100k files.

  Results are reported against a passthrough FUSE baseline (r53), with
  WAL bytes per byte written. A regression over 10% fails CI. Absolute
  targets are set after the first baseline.

## 9. Replication and read scaling — decided

**Decision.**

- **LSN tickets.** A standby serves a read only once its replay LSN has
  reached the daemon's last commit LSN. Otherwise the daemon waits for a
  bounded time and then uses the primary. This replaces the 500 ms
  freshness cache (r75, r19, r34).
- **Snapshot reads.** Snapshots are immutable, so any standby that has
  replayed past the snapshot's creation can serve its reads with no
  ticket at all.
- **Standby identity.** The daemon verifies the standby's system
  identifier and timeline when it connects and after every reconnect.
  `--replica` together with `--unlogged` is refused.
- **Failover.** The daemon reconnects using libpq multi-host with
  `target_session_attrs=read-write`. The spec states plainly that fsync
  survives the loss of the primary only if synchronous standbys are
  configured.
- **Standbys do not protect against corruption.** A standby replicates
  corruption faithfully (r121); that is what checksums and scrub are for.

## 10. Scale-out (v0.2) — decided

**Decision.** Defer it until A is done. `shard.py` stays as it is for
now, with its feature marked as superseded by
`design/sharded_filesystem.md`, and it is rewritten after A. A must keep
everything v0.2 relies on:

- one ino sequence;
- operations as SQL functions;
- the change feed;
- mount leases with fencing epochs;
- blobs written before the metadata that references them;
- no durable state outside the database.

## 11. Space and garbage collection — decided

**Decision.**

- **statfs.** It reports a capacity set at mkfs (`--capacity`), the
  bytes used, and the free space as capacity minus used. Usage is
  tracked with per-worker delta rows, compacted in the background, to
  avoid a hot counter row (Mantle r127).
- **ENOSPC.** Writes fail with ENOSPC when the capacity is reached, or
  when Postgres reports `disk_full`. In-flight writes may overshoot the
  capacity slightly; the spec states the bound.
- **Freeing blobs.**
  - A blob that was never shared is deleted in the transaction that
    drops its last mapping, so the common path has no GC backlog.
  - copy_file_range marks a blob as shared. Shared blobs go to
    `gc_queue`, where a background worker deletes them once no mapping
    references them.
- **Snapshot deletion.** Deleting a snapshot or branch is constant-time.
  A background reclaimer removes rows no longer visible in any retained
  epoch, in bounded batches, and its backlog is visible (r31, r44).
- **Write amplification is reported** as WAL bytes per user byte
  written, along with table bloat (r22, r33, r51).
- **Per-tree quotas** come later.

## 12. Integrity, backup and snapshots — decided

**Decision.**

- **End-to-end checksums.** The daemon computes CRC32C for every blob on
  write and verifies it on every read. A mismatch is EIO, an ERROR log
  line and a metric (r120, r121).
- **`pgfs check`** verifies SQL invariants:
  - no orphaned dentries;
  - no directory cycles;
  - `nlink` matches the dentries that point to the inode;
  - `size` is consistent with the blocks;
  - every `blob_id` resolves;
  - no unreferenced unshared blobs;
  - epoch and layer consistency.

  It also runs `amcheck`. It runs in CI and on demand (r54, r58, r135).
- **`pgfs scrub`** re-reads every blob and verifies its checksum, at a
  limited rate, in the background.
- **Backups.** pgfs keeps no durable state outside its database, so any
  consistent Postgres backup is a consistent pgfs backup, and PITR gives
  cluster-level time travel. The spec says so and gives the procedure.
- **Export and import.** `pgfs export tree@snapshot` writes a tar
  stream, a portable backup taken from a frozen point.
  `pgfs import` reads one.

## 13. Security and access control — decided

**Decision.**

- **Permissions.**
  - Mounts use `default_permissions`, so the kernel enforces mode bits
    and the rules for chmod and chown.
  - New files take their uid and gid from the caller.
  - A setgid directory passes on its group.
  - A write by a non-owner clears the setuid and setgid bits.
  - `allow_other` is opt-in.
- **Database roles.** An owner role runs mkfs and migrate. The runtime
  role can only use the `pgfs` functions and the DML they need.
- **TLS.** Connections to remote databases require TLS with
  `sslmode=verify-full`.
- **Secrets.** Passwords never appear in argv or in logs; connection
  strings are redacted in every message. Today's error text includes the
  connection string.
- **Snapshots are immutable in the database itself.** The SQL functions
  refuse writes to retained epochs, so a buggy or hostile client cannot
  change one.
- **Names** are validated before any query: length, no `/`, no NUL, and
  not `.` or `..`.
- **Out of scope.** Encryption at rest is delegated to the disk or
  tablespace. Multi-tenancy means one database per filesystem; per-tree
  grants may come later.

## 14. Verification — decided

**Decision.** A new `spec/verify.py` requires:

- **ReferenceModel.** An in-memory filesystem behind `PgfsBackend`,
  covering layers, epochs and orphans. It doubles as the mock in unit
  tests (ShardStore r128).
- **ModelConformance.** proptest generates operation sequences, runs
  them against both the model and pgfs, and compares outputs and
  invariants after every step (r128, r48, r81).
- **CrashConformance.** Postgres runs on LazyFS. The tests lose or tear
  writes and kill Postgres or the daemon. Each recovered state must be
  one of the model's allowed crash states. This covers snapshot, branch,
  promote and open-after-unlink (r135, r123).
- **DbFaultInjection** and a **configuration matrix** (r119, r121, r80).
- **PgfsCheck** after every integration test.
- **Standard suites.** xfstests (generic) and pjdfstest for POSIX
  conformance, with a list of known exclusions.
- **Divergence tests.** Edits through SQL behind the daemon's back,
  standby lag, and a cache past its TTL (r86).
- **LinearizabilityCheck** with shuttle, once dispatch is concurrent
  (r128).

## 15. Operations — decided

**Decision.**

- **One CLI** for every administrative action: `pgfs mkfs`, `mount`,
  `check`, `scrub`, `snapshot`, `branch`, `promote`, `tree`, `changes`,
  `export`, `import`, `migrate` and `stats`.
- **No implicit schema creation.** Mount refuses a database that is
  unformatted or has an unknown schema version.
- **Postgres versions.** 17 and newer majors are supported, and each is
  tested in CI (r76).
- **Server settings** are checked at mount (topic 2).
- **Supervisors.** systemd readiness via `sd_notify`, alongside the
  existing pid and ready files.
- **Logs** are redacted. Metrics stay in the structured log, in a form
  ready for a Prometheus exporter later.
- **Upgrades.** A daemon upgrade is a remount for now. Restarting
  without unmounting, by passing the fd, comes later and depends on
  persistent inode numbers (r35, r46).

## 16. How the spec is written — decided

**Decision.**

- **Contract clauses.** A `Contract` context in `err.py`, which every
  `Req` inherits, defines labelled clauses: Behaviour, Postcondition,
  Invariant, Durability, Crash, Concurrency, Errors, Bounds and Sources.
  A requirement states the clauses that apply to it (r43, r60).
- **The spec states the target system.** Feature status notes say what
  exists today. Libspec diff then turns the gap into implementation
  work.
- **Modules by concern:**
  - `storage`, `durability`, `consistency` and `posix`;
  - `failure` and `concurrency`;
  - `branch`, `changefeed`, `space` and `integrity`;
  - `security`, `verify` and `ops`.

  Existing modules are revised in place. Components the review marked
  wrong are rewritten or retired. Nothing references requirement names
  from code, so renames are free.
- **Features keep their four Diátaxis pages.** New features get them
  too.

# pgfs spec review against the research (2026-10-27)

This is a whole-spec pass over `spec/` (main_spec, err, observe, app, optimize,
replica, offload, perf, shard). It is read against the papers summarised in
`design/papers/reading-notes.md`: GFS, refs 01–12, and r00–r135, 117 entries
from the wider list. Paper ids such as (r128) point to those notes. Wherever this
review states what the code does, I checked it in `src/` during this pass.

**How the papers were read.** The classics and most sweep papers were read
through their abstract, introduction, design and key mechanism sections, using
prose extraction. Evaluation sections were skimmed. About 21 papers in the
bibliography are not on disk, mostly ACM-paywalled, and r96 is among them. Their
entries in the notes are marked.

---

## 1. The verdict in one paragraph

The spec describes the implementation well: what each callback does and which
errno it returns. It does not state the *contracts* that make a filesystem
correct. Nowhere does it say when data is durable, what another reader may see,
what a crash may leave behind, or which operations are atomic. Every known bug
falls in that gap:

- Bug 1, the zeroed partial block, is a missing postcondition on write.
- Bug 2, the LIKE wildcard in rename, is a missing invariant: a rename touches
  exactly the subtree.
- Bug 3, data lost on rename with open buffers, is a missing visibility rule.
- Bug 4, the root identity collision, is a missing identity invariant.
- Bug 5, no reconnect, is a missing failure model.

The research agrees that the specification form is the lever:

- SysSpec (r43) and PoWER (r60) state requirements as preconditions,
  postconditions and invariants, plus crash and concurrency clauses.
- ShardStore (r128) makes those clauses executable as a reference model.
- CrashMonkey (r123), ALICE (r118) and LazyFS (r135) check the crash clause
  mechanically.

The highest-value change is to give each Req that shape, starting with the six
below.

## 2. Top findings, ranked

| # | Finding | Where | Papers |
|---|---|---|---|
| 1 | **fsync has no contract, and is not durable.** No Req covers `flush`, `fsync` or `release`. `SynchronousCommitTuningReq` sets `synchronous_commit=off` on every connection, so an acknowledged fsync can be lost. app.py's own explanation (≈l.1120) concedes that "even `fsync` does not close the gap". Any crash-consistency checker would flag this as a durability violation. | app.py (missing), optimize.py:89 | r123, r56, r90, r118, r129, r135 |
| 2 | **No consistency model is stated.** The per-handle write buffer is invisible to other handles, getattr and rename, which is how Bug 3 happens. Replica reads of `getattr` and `readdir` can miss the daemon's own write for up to 500 ms. | optimize.py:186, replica.py, optimize.py:62 | r129, r133, r06, r24, r75 |
| 3 | **Inode identity is split three ways.** `entries.ino` is an identity column, but FUSE uses an in-memory `path_by_ino` counter (`src/fs.rs:84`) with no `forget`, so it only grows. `blocks.ino` defaults to 1 and the FUSE write path never sets it (`src/db.rs:204`). The unique `(parent_ino, name)` index that `InoBasedRekeyingReq` relies on is *dropped* by the DDL (`src/db.rs:118`). The backend's ino-keyed block path stores blocks under `(parent='', name=<ino>)` (`src/db.rs:229`), which aliases a root-level file with a numeric name. FUSE does not reach that path yet. | app.py:12, 73; optimize.py:33 | r00, r07, r11, r16, r18, r35, r46, r91, r132 |
| 4 | **No failure model.** There is no reconnect, no `statement_timeout`, `lock_timeout`, `connect_timeout` or keepalives, no retry on 40001/40P01, and no FUSE INTERRUPT handling. `DetectsDeadlock` checks liveness *on callback entry* (`src/fs.rs:100`), so a dispatch thread blocked inside a DB call is never detected. | app.py:12, 257 | r28, r20, r67, r78, r79, r122, r124, r125 |
| 5 | **Transaction boundaries are an optimisation, not an invariant.** `TransactionBatchingReq` frames one transaction per operation as a speed-up. The feature docs then list exceptions: unlink, rmdir and the offload aligned path are not single transactions. | optimize.py:111, app.py, offload.py | r68, r82, r91, r118 |
| 6 | **Silent no-ops in the POSIX surface.** `setattr` ignores mode, uid, gid, atime and mtime yet replies success (`src/fs.rs:381-386`), so `chmod +x` and `touch -d` lie. A non-UTF-8 name returns ENOENT, not EILSEQ. Directory times are the epoch. statfs, symlink, link, xattr and flock are not declared anywhere. | app.py:80, 87 | r52, r48, r81 |
| 7 | **Testing is integration-only.** No reference model, no property or crash conformance tests, no fault injection at the Db boundary, no configuration matrix. | app.py:217 | r128, r135, r123, r121, r119, r80, r71, r48, r81 |
| 8 | **shard.py contradicts the v0.2 design.** It still specifies v0.1: `--shards`, RAID-5-like placement, quorum writes with compensating rollback, and `ver = client_id<<32\|seq`. | shard.py:23-152 | r23, r30, r04, r72, r73 |
| 9 | **Factual errors in optimize.py.** FUSE-over-io_uring landed in Linux 6.14, not 6.9, and fuser 0.14 does not support it. The block size reads 64 KiB in one place and 128 KiB in others; the code uses 64 KiB (`src/db.rs:310`). `UnloggedTableOptReq` omits that unlogged tables are truncated after a crash and are not replicated, so the mode is incompatible with `--replica`. | optimize.py:17, 101, 152, 186, 209; offload.py | r17 |
| 10 | **The cluster is not hardened.** `ProvidesProjectLocalCluster` does not require data checksums, `fsync=on` or `full_page_writes=on`. LazyFS showed that PG with `full_page_writes` off restarts after a torn page but fails `amcheck`. | app.py:194 | r14, r94, r120, r135 |

---

## 3. Cross-cutting: change the shape of a Req

`err.py` already defines `PreCondition` and `PostCondition` contexts, but no Req
fills them in. Proposal:

1. Add four contexts next to them in `err.py`:
   - `Invariant`: what holds before and after, under concurrency.
   - `CrashStates`: which states a crash at any point may leave, relative to the
     last acknowledged operation.
   - `DurabilityPoint`: the exact event after which the operation survives a
     crash of the daemon, the host or Postgres.
   - `Concurrency`: what a concurrent observer may see, and the lock order.
2. For each POSIX Req in app.py, write the four clauses in a fixed template.
   For example, `RenamesEntries`:
   - **Pre:** the source exists.
   - **Post:** the source is absent and the target holds the source's inode.
     Exactly the descendants of the source changed parent. The match is
     prefix-exact, never LIKE. That clause closes Bug 2.
   - **Invariant:** no directory is its own ancestor.
   - **Concurrency:** serialised by `pg_advisory_xact_lock(RENAME_LOCK)` for
     directory renames (r16, r127).
   - **Crash:** all or nothing (one transaction).
   - **Durability:** commit with `synchronous_commit` per the policy in §4.1.
3. Make every clause *executable* in the reference model (§4.7). A clause that
   cannot be checked is a comment, not a requirement.

## 4. Proposed new requirements

Each line below gives the module, the Req name, its one-line contract and the
supporting papers.

### 4.1 Durability and visibility (app.py, new section)
- `FlushesOnClose`: `flush`/`release` commit the handle's dirty blocks and size
  in one transaction before replying. They report a commit error to the caller
  (`close` returns EIO). (r90, r118)
- `FsyncIsDurable`: `fsync`/`fdatasync` reply only after the handle's
  transaction commits with `SET LOCAL synchronous_commit = on`. On a replicated
  set, the setting is `remote_apply` or `on`, per the set policy. The cost falls
  only on fsync, so everything else keeps `off`. (r123, r56, r129)
- `FsyncFailureSemantics`: if the commit fails, the dirty buffer is *retained*
  and the error is reported once to every fd that wrote. Uncommitted data is
  never served as committed after a reconnect. (r90)
- `CommitOrderedMetadata`: metadata operations are atomic and persist in commit
  order. A crash leaves a prefix of acknowledged operations, minus the
  `synchronous_commit=off` window, which is bounded by `wal_writer_delay`. The
  spec states this window as a number. (r118, r56)
- `ConsistencyModel`: there are two acceptable options.
  - **(a) close-to-open:** a reader sees writes from other handles after they
    close or fsync, as NFS does.
  - **(b) single-mount linearizable:** all operations on a path drain that
    path's buffers first.

  Pick (b) for one mount and (a) across mounts in v0.2, and write it down.
  (r129, r133, r40, r47)
- `DrainsBuffersBeforeExternalising`: under (b), `read`, `getattr`, `lookup`,
  `readdir`, `rename`, `setattr`, `link` and `copy_file_range` on a path first
  merge or flush the pending buffers of *every* handle on that path. This closes
  Bug 3. (r129)

### 4.2 Identity (app.py and optimize.py)
- `PersistentInodes`: the FUSE inode equals `entries.ino`. It is stable across
  daemon restarts and is never reused while referenced. This replaces "stable
  for the entry's full path" in `LooksUpEntries`, and it is a prerequisite for
  FUSE restart via fd passing (r35, r46).
- `UniqueDentry`: `UNIQUE (parent_ino, name)` and
  `parent_ino REFERENCES entries(ino)`. The root is the only row with
  `ino = parent_ino`, which settles Bug 4 by construction. (r54, r58)
- `BlocksKeyedByIno`: `blocks` has primary key `(ino, block_no)` and nothing
  else. The text keys are dropped, which removes the latent alias between
  `src/db.rs:204` and `:229`.
- `HandlesForget`: `forget` and `batch_forget` decrement lookup counts and evict
  in-memory state, so memory is bounded by the number of live inodes. (r57)
- `SchemaVersion`: a `pgfs_meta(schema_version)` row. The daemon refuses to
  mount an unknown version. Migrations are explicit, replacing ad-hoc
  `ADD COLUMN IF NOT EXISTS`. (r25)

### 4.3 Failure model (app.py, new section)
- `ReconnectsWithBackoff`: on connection loss, enter a reconnect state with
  exponential backoff and jitter. In-flight requests fail with EIO, or EINTR if
  interrupted. Never replay non-idempotent operations blindly. (r67, r125)
- `IdempotentMutations`: each mutating operation carries
  `(mount_id, op_seq)`. A client table dedups retries after an ambiguous commit.
  (r28, r20)
- `BoundsEveryWait`: `connect_timeout`, `statement_timeout`, `lock_timeout` and
  TCP keepalives are set and spec'd with their values. A timeout becomes EIO
  plus a WARN. (r78, r79)
- `RetriesSerializationFailures`: SQLSTATE 40001/40P01 is retried with a bounded
  count. (r68)
- `HonoursInterrupt`: FUSE INTERRUPT cancels the backend query
  (`pg_cancel_backend` or the `cancel_token`). (r78)
- Rewrite `DetectsDeadlock` as `InFlightAgeWatchdog`: track the start time of
  the oldest in-flight request. If it exceeds N seconds, log the stuck op plus
  `pg_stat_activity` for the backend PID. The check runs on a separate thread,
  not on callback entry. (r124, r122)

### 4.4 POSIX surface (app.py)
- `SetsAttributes`: `setattr` applies mode, uid, gid, atime and mtime (and
  ctime), or replies EPERM/ENOSYS. It never reports silent success. (r52)
- `DeclaresUnsupportedOps`: a table of every FUSE op with
  `supported | ENOSYS | EOPNOTSUPP`, so the gaps become a spec decision: link,
  symlink, xattr, flock, mknod, statfs. (r48)
- `ReportsStatfs`: real values from `pg_database_size` and tablespace free
  space, not the fuser zeros. (r22, r44)
- `ByteExactNames`: names are compared under `COLLATE "C"`. Non-UTF-8 names
  return EILSEQ until the column type is `bytea`. Names longer than 255 bytes
  return ENAMETOOLONG. (r52)
- `StableReaddirCookies`: readdir paginates by keyset (`name > $cookie`) and
  keeps no OFFSET into a re-queried list. Each entry appears at most once even
  under concurrent create or unlink. (r57, r131)
- `DirectoryTimes`: directory mtime and ctime are maintained. If contention
  matters, they are kept as appended deltas folded on read, never as an UPDATE
  of a hot row. (r127)

### 4.5 Concurrency (optimize.py, `MultiThreadedConnectionPoolReq` rewritten)
- Workers are sharded **by inode**: each worker owns a connection and the inodes
  hashed to it. That keeps per-file order and buffer coherence without locks
  (r130). Cross-parent metadata goes through one coordinator path.
- `LockOrder`: rows are locked in ascending `ino` order, and directory renames
  take a global advisory lock first. (r11, r65, r66, r16)
- `LinearizabilityCheck`: a shuttle or loom model test against the reference
  model once dispatch is concurrent. (r128)

### 4.6 Replica (replica.py)
- Replace `ReplicaFreshnessCacheReq`'s 500 ms heuristic with
  `LsnTicketReads`. Each mutation records its commit LSN. A replica read is
  allowed only if `pg_last_wal_replay_lsn() >= ticket`; otherwise it waits up to
  X ms or goes to the primary. This gives read-your-writes by construction.
  (r06, r19, r24, r34, r49, r75)
- State plainly in the ReplicatedFilesystem docs that `read` always goes to the
  primary today. They already do; keep that wording.

### 4.7 Verification (app.py, new section, or a new `spec/verify.py`)
- `ReferenceModel`: an in-memory `BTreeMap` filesystem behind the
  `PgfsBackend` trait (which already exists, `src/backend.rs:140`). It is also
  used as the unit-test mock. (r128)
- `ModelConformance`: proptest op sequences run against the model and against
  pgfs. Outputs and invariants are compared after each op. (r128, r48, r81)
- `CrashConformance`: Postgres runs on **LazyFS** in CI. The harness injects
  lost and torn writes and kills the daemon or PG mid-history, then asserts that
  the recovered state is in the model's allowed crash set (§4.1). (r135, r123)
- `DbFaultInjection`: a feature-gated wrapper over `Db` that fails the Nth call
  with each error class. Every error branch must be exercised. (r121, r119)
- `ConfigMatrix`: CI runs at least {replica on/off} × {unlogged on/off} ×
  {attr-ttl 0/1s} × {writeback cache on/off}. (r80)
- `pgfs check`: SQL invariant checks (orphans, cycles, size versus last block,
  dangling blocks) plus `amcheck`. These run on demand and in CI. (r54, r58,
  r131, r135)

### 4.8 Cluster and operations (app.py `ProvidesProjectLocalCluster`, observe)
- initdb uses `--data-checksums`. `fsync=on` and `full_page_writes=on` are
  asserted at connect, with a refusal or WARN if they are off.
  (r14, r94, r120, r135)
- `SupportedPostgresVersions`: an explicit range, checked at connect. (r76)
- Metrics to add (r12, r22, r33, r44, r51, r124):
  - a slow-query WARN above a threshold;
  - WAL bytes per user byte written, to show write amplification;
  - dead-tuple and bloat gauges for `blocks`;
  - the age of the oldest in-flight request.

## 5. Per-module notes

**main_spec.py, err.py, observe.py.** These are sound scaffolding. Add the four
contract contexts (§3). `GlobalMutableState` should name the ino maps and handle
tables explicitly. They are the state that §4.2 and §4.5 change.

**app.py.**
- The Reqs are accurate as descriptions. Re-express them per §3.
- `ConnectsToPostgres` describes ino columns as if they were keys, and omits the
  legacy `entries.data` column.
- `LooksUpEntries` promises a stability that holds only within one process.
- `ListsDirectories` offsets are unstable under concurrent change.
- `InstrumentsFuseCallbacks` lists `copy_file_range` and `fallocate`, which have
  no FUSE handlers (offload.py's status note correctly says so).
- `DetectsDeadlock` overstates what it detects.
- The features docs are honest about their gaps. Each listed gap should become a
  PLANNED Req, so that the gaps are tracked rather than merely explained.

**optimize.py.**
- Correct the io_uring kernel version (6.14) and add a fuser-support caveat.
- Unify the block size at 64 KiB.
- Add the unlogged-table crash and replica caveat.
- Re-frame `SynchronousCommitTuningReq` as a *durability policy*: off by default,
  with an override on fsync.
- Re-frame `TransactionBatchingReq` as a correctness invariant, "one FUSE op =
  one transaction", and move it to app.py.
- `LocalPgrxEngineReq`: the research says to disaggregate only when the cost is
  justified (r134). A bgworker crash restarts the whole postmaster, so the Req
  needs a stated failure blast radius before anyone builds it.
- `RemoteAsyncBackendReq` and `IoUringBufferRingReq` are long-horizon. Keep them
  PLANNED and behind §4.1–4.5.

**replica.py.** The structure is good and the status is honest. Its one design
change is §4.6. Also add `ReplicaRejectsUnlogged`: refuse `--replica` together
with `--unlogged`.

**offload.py.** The status is honest. Its aligned path must be one transaction
(§4.1, `CommitOrderedMetadata`). The 128 KiB references must become 64 KiB.
`ReflinkBlockSharingReq` implies block refcounts. Add a `pgfs check` invariant
for them before building it.

**perf.py.** Fine as written. `ProfileDrivenBenchmark` should name its standard
workloads (fio rand/seq, mdtest, `git clone` of a fixed repo, a kernel tarball
untar) so results are comparable over time. (r12, r17)

**shard.py.** Replace it wholesale with v0.2 from `design/sharded_filesystem.md`:
- an MDB plus data sets, with a placement table carrying failure domains and
  weights (r01, r03, r05);
- up/down × in/out member states (r05);
- ordered writes, where data commits before metadata ever references it (r133);
- seal-then-switch epochs with fencing tokens (r04, r71, r73);
- `synchronous_standby_names = ANY 1 (…)` (r72);
- learner-first set add and rate-limited drain (r08, r95);
- lazy GC;
- NOTIFY coherence with sequence numbers and gap detection (r59);
- a single-set default mode, since disaggregation must be justified (r134).

Retire quorum-with-compensation and client-sequenced versions (r23, r30).

## 6. Suggested order

1. **Contracts first.** §3 contexts, then `FsyncIsDurable`, `FlushesOnClose`,
   `ConsistencyModel` and `DrainsBuffersBeforeExternalising`. These fix Bug 3
   and define what testing checks.
2. **Verification harness.** `ReferenceModel`, `ModelConformance`,
   `DbFaultInjection`, then `CrashConformance` on LazyFS. Bugs 1–4 should fall
   out of it as failing cases.
3. **Identity.** `SchemaVersion`, `UniqueDentry`, `BlocksKeyedByIno`,
   `PersistentInodes`, `HandlesForget`. This is the ino migration and settles
   Bug 4.
4. **Failure model.** `BoundsEveryWait`, `ReconnectsWithBackoff`,
   `IdempotentMutations`, `InFlightAgeWatchdog`. This settles Bug 5.
5. **POSIX surface.** `SetsAttributes`, `DeclaresUnsupportedOps`,
   `StableReaddirCookies`, `ByteExactNames`, `ReportsStatfs`.
6. **Concurrency.** Inode-sharded workers and `LockOrder`.
7. **Replica.** LSN tickets.
8. **shard.py v0.2.** Then the long-horizon optimize items.

Items 1–5 are spec-only edits to existing modules, plus new Req classes. None of
them commits the code to a design that the research argues against.

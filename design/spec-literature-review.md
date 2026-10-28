# pgfs spec against the literature: complete review

Date: 2026-10-28. Branch `opt`, after `d74aa8c`.

This document reviews every component in `spec/` against the research
collected in `design/papers/`. It is the long form of `design/spec-review.md`,
which remains the one-page summary.

## How this review was made

**What was reviewed.** All nine spec modules: `main_spec.py`, `err.py`,
`observe.py`, `app.py`, `optimize.py`, `offload.py`, `replica.py`, `shard.py`
and `perf.py`. That is 106 components: 12 in err.py (coding contexts and the
two base classes), 7 observability contexts, 74 requirements, 12 features and
the module list. The Diátaxis pages inside each
feature were read for the limits they document. Where the review describes
what the code does, the claim was checked against `src/` during this pass, and
a file and line is given.

**What it was reviewed against.** 117 papers whose notes are in
`design/papers/reading-notes.md`:

- the 13 classics (GFS and refs 01–12);
- r00–r95, r100 and r118–r135.

Most papers were read through the abstract, introduction, design and mechanism
sections, using extracted prose. Evaluation sections were skimmed. **21 papers
were not available** (r96–r99 and r101–r117, all ACM-only) and are listed in the
appendix. Of these, Fawkes (r96), CFS (r115) and Volley (r113) are the most
likely to change a conclusion here.

**Verdicts.** Each component gets one of four:

- {sound} The literature supports it as written.
- {revise} The direction is right, but it lacks a contract clause, a bound or a
  correction.
- {wrong} It contradicts the literature, the code, or another part of the spec.
- {retire} Replace it wholesale.

Paper references such as (r128) point to entries in the reading notes. The
appendix maps each id to its title.

---

## Part 1. Verdict

**The spec is a faithful, honest description of the implementation. It is not
yet a specification of a filesystem.** It says what each callback does and
which errno it returns. It does not say:

- when an operation becomes durable;
- what another reader may observe, and when;
- which states a crash may leave behind;
- what holds under concurrency;
- what happens when the database is slow, gone or back.

Filesystem research of the last decade treats those five clauses as the
specification itself (r43, r60, r64, r118, r123, r128). Every known pgfs bug is
one of them missing:

| Bug | Missing clause | Would have been caught by |
|---|---|---|
| 1. A buffered partial-block write zeroes untouched bytes | a postcondition on write: bytes outside the written range are unchanged | differential test against tmpfs (r48, r81); model conformance (r128) |
| 2. Rename matches descendants with LIKE, so `_` and `%` act as wildcards | an invariant: rename touches exactly the subtree | the same, with generated names (r48) |
| 3. Rename with open buffered writes loses data | a visibility rule: an operation that externalises state first drains pending buffers (r129) | the same; ALICE-style persistence check (r118) |
| 4. Root identity collision | an identity invariant: the root is the unique row with `ino = parent_ino` | `pgfs check` invariants (r54) |
| 5. No reconnect | a failure model with a database state machine (r70, r15) | fault injection at the Db boundary (r121, r119) |

The spec's strengths are real, and the literature backs its central bets:

- **Metadata as rows in a transactional store** is the mainstream design: Oracle
  FSS (r91), HopsFS (r11), Tectonic (r14), FoundationDB (r15), 3FS (r18),
  InfiniFS (r16) and Mantle (r127) all use it.
- **Delegating atomicity, recovery and replication to Postgres** is the right
  split (r23, r30, r62, r68).
- The **status notes are candid**: they list every gap they know of.

The work is to turn those candid notes into contracts that can be checked.

### Scorecard

| Module | Components | Sound | Revise | Wrong | Retire |
|---|---:|---:|---:|---:|---:|
| err.py | 12 | 9 | 2 | 1 | 0 |
| observe.py | 7 | 4 | 2 | 1 | 0 |
| app.py: requirements | 28 | 7 | 15 | 6 | 0 |
| app.py: features | 6 | 2 | 4 | 0 | 0 |
| optimize.py | 18 | 2 | 11 | 5 | 0 |
| offload.py | 6 | 3 | 3 | 0 | 0 |
| replica.py | 9 | 4 | 3 | 2 | 0 |
| shard.py | 14 | 0 | 2 | 0 | 12 |
| perf.py | 5 | 4 | 1 | 0 | 0 |
| main_spec.py | 1 | 1 | 0 | 0 | 0 |
| **Total** | **106** | **36** | **43** | **15** | **12** |

The base classes `Feat` and `Req` are counted in err.py, as are the two
composed contexts (`DefensiveProgramming`, `Refactor`).

---

## Part 2. What the literature says, theme by theme

Each theme gives the consensus, the papers behind it, and where pgfs stands.

### T1. Filesystem metadata in a database

**Consensus.** Metadata lives in two kinds of record:

- an **inode** record keyed by a stable 64-bit id;
- a **dentry** record keyed by `(parent_id, name)`.

The two are sometimes split into separate tables. Small files are stored inline
in the inode row. A directory rename is a single-row update of the dentry,
protected by a loop check that is serialised at one point. Each filesystem
operation is one bounded transaction.

**Papers.**

- Inversion (r00) keyed names by object id in Postgres in 1993.
- TableFS (r07) and IndexFS (r09) put `(parent_ino, name)` in one table and
  inline small data.
- HopsFS (r11) uses ino keys, partitions by parent and fixes a lock order.
- Small-files HopsFS (r38) tiers small files inline.
- CalvinFS (r10) and BetrFS (r55) are the only serious full-path-keyed systems.
  Each needed heavy machinery to make directory rename tolerable.
- Tectonic (r14), 3FS (r18), Oracle FSS (r91), SingularFS (r36) and InfiniFS
  (r16) confirm the pattern in production.
- InfiniFS (r16) and Mantle (r127) show that loop detection for cross-directory
  rename needs one serialisation point.
- HMFS (r131) shows that 95% of operations are point lookups. Only readdir
  needs order.

**Where pgfs stands.** pgfs is path-keyed: `entries(parent text, name)` and
`blocks(parent, name, block_no)`. That is the design Inversion avoided and
CalvinFS paid for. Directory rename costs O(subtree), and Bug 2 lives in its
prefix match. The schema already carries `ino` and `parent_ino` columns
(`src/db.rs:95`), but FUSE does not use them (§3.4). `InoBasedRekeyingReq` is
filed as a Tier 1 optimisation. The literature says it should be the schema.

### T2. Durability, persistence and crash consistency

**Consensus.**

- A filesystem must state its **durability point**: the event after which an
  operation survives a crash. fsync must reach it.
- It must state its **persistence properties**: which operations are atomic,
  and whether they persist in order.
- After a failed fsync it must keep the dirty data and report the error
  correctly.
- Crash states are checked mechanically, not argued.

**Papers.**

- ALICE and BOB (r118) define persistence properties and show applications
  depend on them.
- CrashMonkey and ACE (r123) run bounded black-box crash tests and found bugs
  in every filesystem tested.
- CAD (r56) shows that asynchronous commit makes data visible before it is
  durable, which allows non-monotonic reads after a crash.
- The fsync-failure study (r90) shows that most applications mishandle a failed
  fsync, so the filesystem's semantics matter.
- Torturing Databases (r120) and CORDS (r121) show that redundancy does not
  imply fault tolerance.
- Protocol-aware recovery (r94) and LazyFS (r135) cover Postgres specifically.
  PG with `full_page_writes` off restarts after a torn page but fails `amcheck`.
- Skyros (r129) states the rule precisely: ordering may be deferred, durability
  may not.
- SquirrelFS (r64) and ShardStore (r128) encode crash ordering in Rust types.
- Ananke (r46) shows that a FUSE daemon's own crash creates a gap in the crash
  states.

**Where pgfs stands.**

- Every connection runs with `synchronous_commit = off` (`src/db.rs:71`), and
  fsync does not override it. The FlatFilesystem reference concedes that "even
  `fsync` does not close the gap". So pgfs fails B3 (r123) by design.
- Committed but unflushed data is visible to other sessions, which is the CAD
  anomaly (r56).
- No requirement covers `flush`, `fsync` or `release`, although the daemon
  implements all three.
- pgfs's real persistence properties are unusually strong: metadata operations
  are atomic and commit-ordered. They are not stated, so applications cannot
  rely on them.

### T3. Consistency, caching and coherence

**Consensus.** First declare the consistency model. Close-to-open (AFS, NFS,
Sprite) and linearizable are both acceptable; leaving it unstated is not. Then
pick one of two mechanisms:

- **Bounded staleness**, through leases that *both* sides honour.
- **Read-your-writes**, through a version or LSN ticket.

TTL alone is not coherence. Invalidation channels need sequence numbers to
detect lost messages. When two clients write the same file, switch it to direct
I/O.

**Papers.**

- AFS (05), Sprite (r40), Leases (r21) and Coda (06) are the classics. Sprite's
  rule is "write-shared file → no caching".
- Chubby (r04) and ZooKeeper (r24) use invalidation and watches, not TTLs.
- CRAQ (r06), Aurora (r19), TiDB (r34), Ionia (r49) and FlightTracker (r75)
  show cheap read-your-writes on replicas with a version or LSN ticket.
- Skybridge (r59) requires gap detection on change feeds.
- Buffered and direct I/O in Lustre (r47) applies the same rule as Sprite.
- uFS (r130) states the cost of its leases: a writer waits two lease terms.
- Fletch (r132) and SwitchΔ (r133) add two rules: path-dependent invalidation,
  and "deferred state must be visible to every reader".

**Where pgfs stands.**

- No model is declared.
- The per-handle write buffer is visible only to its own handle, so another
  handle's `read` or `getattr` sees old data. That is why rename loses data
  (Bug 3).
- Replica reads use a 500 ms freshness verdict, and a cached "fresh" verdict can
  cover the daemon's own later write. That contradicts `FallsBackToPrimary`,
  which says "a stale replica never produces stale reads".
- `--attr-ttl` is a lease without the write side.
- v0.2 plans LISTEN/NOTIFY, but without gap detection.

### T4. FUSE as a platform

**Consensus.**

- FUSE costs are acceptable for most workloads once several things are on: the
  writeback cache, multi-threaded dispatch, large `max_write`, splice, and
  sensible entry and attribute TTLs.
- Metadata-heavy work stays several times slower than in-kernel filesystems.
- FUSE over io_uring is upstream since Linux 6.14.
- A daemon can restart without losing the mount if it passes `/dev/fuse` to its
  successor and can rebuild its state.

**Papers.**

- To FUSE or Not (r12) measured the cost of each option and found single-thread
  dispatch to be the worst configuration.
- ExtFUSE (r13) shows the xattr tax and negative caching.
- RFUSE (r17) uses per-core queues and gives the io_uring status.
- XFUSE (r35) and Ananke (r46) cover restart and the daemon-crash gap.
- Bento (r53) quantifies the FUSE ceiling for git.
- uFS (r130) shards worker threads by inode.

**Where pgfs stands.**

- Dispatch is single-threaded on one connection.
- The writeback cache is specified but its preconditions are not: kernel-owned
  mtime and size, writes that arrive after close, and `O_APPEND`.
- `IoUringBufferRingReq` gives the wrong kernel version and ignores that fuser
  0.14 has no support.
- There is no `forget` handler (§3.4).
- Restart is not possible because inode numbers are not persistent.

### T5. Failure handling and gray failure

**Consensus.**

- Every wait has a timeout.
- Retries are bounded, backed off and jittered.
- Under overload, shed load rather than queue it.
- Mutations carry idempotency keys so a retry after an ambiguous commit is safe.
- A watchdog measures the age of the oldest in-flight request, not the health of
  a heartbeat thread.
- Error handlers are tested by injecting the errors.

**Papers.**

- Viewstamped Replication (r28) and λFS (r20) use a client table of operation
  ids.
- FoundationDB (r15) puts every failure through one tested recovery path.
- The two metastable-failure papers (r67, r125) show that retries sustain
  overload.
- Gray Failure (r124) is about differential observability.
- Fail-Slow at Scale (r122), Sieve (r79) and DepFast (r87) show that a slow
  dependency is the common case.
- FiDe (r78) shows that timeouts are where failure detection goes wrong.
- Nifty (r71) covers partial partitions.
- Aspirator (r119) found that most critical failures sit in error handlers
  that were never tested.
- CAFault (r80) shows that behaviour under faults depends on the configuration.

**Where pgfs stands.**

- There are no database timeouts of any kind. `src/` contains no
  `statement_timeout`, `lock_timeout`, `connect_timeout` or keepalive.
- There is no reconnect, no retry for SQLSTATE 40001/40P01, and no handling of
  FUSE INTERRUPT.
- `DetectsDeadlock` cannot detect a dispatch thread stuck in a database call
  (§3.7). That is the gray-failure gap r124 describes.

### T6. Replication, placement and sharding

**Consensus.**

- Do not invent a replication protocol. Use a log that some consensus protocol
  orders.
- Keep placement separate from redundancy.
- Use an explicit placement table with failure domains and weights, not pure
  hashing.
- Fence stale writers with epochs ("seal, then switch").
- Add members as learners first.
- Rate-limit data movement.
- Disaggregate only when the communication cost is justified.

**Papers.**

- Chain Replication (r01) measured ring placement as the worst for availability.
- Ceph, CRUSH and RADOS (r02, r03, r05) use placement groups, failure domains
  and the up/down × in/out member states.
- Raft and Paxos (r08, r23), Spanner (r30), Harp (07), Copilot (r72), Delos (r73)
  and Petal (r37) cover log-based replication.
- Okapi (r62) separates placement from redundancy.
- WAS (r27), PolarFS (r32), Pangu (r51), Socrates (r126) and the disaggregation
  vision paper (r134) cover disaggregated designs.
- Dayu (r95) rate-limits movement, and balanced recovery (r84) places data for
  recovery load.
- SwitchΔ (r133) and Tectonic (r14) use ordered writes: commit data, then the
  metadata that references it.

**Where pgfs stands.**

- `shard.py` specifies quorum fan-out with compensating rollback,
  client-sequenced versions and consistent hashing by top-level name. Every one
  of these is a pattern the literature warns against (r01, r23, r30, r62).
- The v0.2 design doc (`design/sharded_filesystem.md`) already follows the
  consensus, but the spec does not.

### T7. Specification, verification and testing

**Consensus.**

- Write requirements as contracts: preconditions, postconditions, invariants,
  crash states and concurrency.
- Make the specification executable as a small reference model in the
  implementation language.
- Check the implementation against it with property-based and differential
  tests, crash conformance tests and model checking of concurrency.
- Inject faults at every boundary.
- Keep an invariant checker for the stored state.

**Papers.**

- SysSpec (r43), PoWER (r60) and SquirrelFS (r64) define contract form and
  crash clauses.
- RefFS (r65), DaisyNFS (r68), GoJournal (r69) and VeriBetrKV (r70) are full
  verification. They are out of reach for pgfs, but they show the shape of the
  obligations.
- ShardStore (r128) is the closest template: Rust, executable reference models,
  proptest, crash conformance and shuttle.
- Metis (r48) and Monarch (r81) do differential POSIX testing.
- CrashMonkey (r123), ALICE (r118) and LazyFS (r135) test crash behaviour.
- T2C (r58) turns tests into runtime checkers, and pFSCK (r54) is an invariant
  checker.
- ScaleCheck (r57) finds structures that grow with scale.
- SQLancer (r76) shows that the database underneath has logic bugs too.

**Where pgfs stands.**

- Tests are Rust unit tests plus shell integration scripts.
- There is no reference model, no generated operation sequences, no crash
  tests, no fault injection and no checker.
- The err.py `PreCondition` and `PostCondition` contexts are coding-style rules
  for functions. They are not semantic contracts for operations.

### T8. Space, write amplification and garbage collection

**Consensus.**

- Logging stores pay for cleaning.
- State the write amplification and measure it.
- Reclaim space lazily but visibly.
- Copy-on-write blocks make clones and snapshots cheap.

**Papers.**

- LFS (r22) introduced the cleaner.
- BlueStore (r33) is the warning about running a filesystem on top of a
  general-purpose store.
- DisCoGC (r44) covers discard-based GC, and WOFS (r61) write-once metadata.
- BetrFS clones (r55) and CalvinFS (r10) show cheap copy-on-write.
- Pangu 2.0 (r51) uses traffic amplification as a metric.
- RubbleDB (r83) and f4 (r31) cover lifecycle tiers.

**Where pgfs stands.**

- Every partial-block write rewrites a 64 KiB TOAST value. Then comes WAL, plus
  full-page images after each checkpoint. A one-byte write can cost about
  65,000 times its size.
- The spec states no amplification and requires no VACUUM or bloat monitoring.
- `statfs` reports zeros.
- The reflink and clone requirements imply block reference counts, but the spec
  states no invariant for them.

### T9. Names and security boundaries

**Consensus.**

- Name comparison is byte-exact unless a deliberate normalisation policy says
  otherwise.
- Copy tools fail in dangerous ways when name semantics differ between source
  and destination.
- Name and length limits are stated.

**Papers.** Unsafe at Any Copy (r52) and NASD (04).

**Where pgfs stands.**

- Names are Postgres `text` under the database collation, which is not
  specified as `C`.
- Non-UTF-8 names return ENOENT where EILSEQ is correct.
- No limit is stated on name length.

### T10. Concurrency inside the daemon and the database

**Consensus.**

- Shard work by inode so per-file order needs no locks.
- Fix a lock order for operations that touch several rows.
- Do not hold a host-side lock across a network round trip.
- Do the reads outside the transaction and the writes inside, with a version
  check.
- Keep hot directory attributes as appended deltas rather than updates of one
  row.

**Papers.**

- uFS (r130) shards workers by inode.
- HopsFS (r11) fixes a lock order, and RefFS (r65) proves liveness under one.
- Chardonnay (r66) reads before taking locks.
- Lockify (r41) uses the database's uniqueness check as the lock.
- CetoFS (r42) argues against host-side locks.
- Mantle (r127) uses delta records and a single serialisation point for loop
  checks. CoFS (r45) is also relevant.

**Where pgfs stands.**

- `MultiThreadedConnectionPoolReq` names a pool size and nothing else.
- err.py's `GlobalMutableState` context assumes single-threaded dispatch, which
  contradicts that requirement.
- The code already holds `Mutex<DbClient>` guards (`src/db.rs:366`).

---

## Part 3. Component by component

### 3.1 main_spec.py

**main_spec module list** {sound}
It lists the modules app, observe, offload, optimize, perf, replica, shard,
diataxis and conventional_commits. Add a `verify` module when Part 4.7 lands.

### 3.2 err.py: coding contexts and base classes

**Err** {sound}
It requires error chains with context, bans silent `let _ =`, and maps errors to
replies through `log_and_reply!`. It allows `fallible().ok(); // intentional:`.
Aspirator (r119) found that most catastrophic failures come from swallowed
errors of exactly this kind. Change: every intentional `.ok()` site must be
listed and driven by a fault-injection test (Part 4.7), and CI should grep for
new ones.

**BoilerPlate, FunctionLines, Indentation, Refactor** {sound}
These are code-style rules. The literature is silent on them.

**PreCondition** {sound}
Validate at boundaries and make invalid states unrepresentable. SquirrelFS
(r64) and ShardStore (r128) take this further with typestate for ordering
obligations. An example is a `DataCommitted` token that the metadata update
requires in v0.2. Add that suggestion.

**PostCondition** {revise}
It is phrased per function: `debug_assert!`, or return an invariant error. The
literature puts postconditions on *operations* (r43, r60). Change: keep this as
the coding rule, and add operation-level contract contexts beside it (Part 4.1).
Otherwise, nothing in the spec states what a successful `write` or `rename`
guarantees.

**GlobalMutableState** {wrong}
It says that "FUSE's default single-threaded dispatch makes interior mutability
unnecessary for this project". This contradicts `MultiThreadedConnectionPoolReq`
and the code, which already uses `Mutex` guards (`src/db.rs:366`) and an r2d2
pool (`src/db.rs:55`). It also omits the state that matters most: the inode map,
the handle table and the per-handle buffers (`src/fs.rs:84-88`). Change: name
that state, and state the rule for it when dispatch is concurrent. The rule is
to shard ownership by inode (r130).

**DefensiveProgramming** {sound}
This is a composition of the three contexts above.

**Robustness** {revise}
"Pass `&mut Db`" assumes a single client. It conflicts with pooling. "Release
external resources deterministically" is right but incomplete. The resources
that hurt pgfs are *unbounded waits* on them (T5). Change: add "every external
wait has a timeout, and every retry is bounded" (r78, r67).

**Feat, Req base classes and the STABLE, PARTIAL and PLANNED statuses** {sound}
The status and status-note mechanism is a strength. Change: add the contract
contexts from Part 4.1 to `Req`, so that every requirement inherits the clause
slots.

### 3.3 observe.py: observability contexts

**Tracing** {sound}
Spans per FUSE callback with nested DB spans. This is the full-stack view that
Fail-Slow at Scale (r122) recommends.

**Metrics** {revise}
Counters plus histograms with decade buckets. Add these:

- a slow-query signal (r88, r122);
- the age of the oldest in-flight request (r124);
- WAL bytes per user byte written (r33, r51);
- dead tuples and bloat for `blocks` (r22, r44);
- replica replay lag in bytes and seconds (r75).

**CrashReports** {sound}
A panic hook with a backtrace, and abort in release builds. This is standard.

**StateIntrospection** {revise}
The SIGUSR1 dump runs on the *next callback*, so on a stuck mount it never
appears. Gray Failure (r124) argues for a dump that the signal thread itself can
produce. Change: dump from the signal thread, using read-only snapshots: atomics,
plus `try_lock` with a "locked" placeholder. Include `pg_stat_activity` for the
backend PID (r122).

**Profiling** {sound}
SIGUSR2 toggles a sampler, and a Chrome trace is written at shutdown.

**LogLevels** {sound}
This is standard.

**HealthEndpoint** {wrong}
The liveness design is inverted. A background thread bumps a counter, and
callbacks check it on entry. That detects a stalled *heartbeat* thread, which
almost never happens. It cannot detect a stalled *dispatch* thread, because no
callback runs to do the check. This is the differential-observability gap in
r124. Change: invert it. Dispatch stamps "request started at T" into an atomic,
and a watchdog thread checks the age of the oldest in-flight request. When that
age passes the threshold, the watchdog logs, dumps state, and optionally cancels
the backend query.

### 3.4 app.py: requirements

**ConnectsToPostgres** {wrong}

- It describes the `entries` columns as `ino, parent_ino, parent, name, …`,
  as though ino were the key. In the code, the FUSE paths key everything by
  `(parent, name)`. The ino columns are populated but not used for identity.
- It omits the legacy `entries.data bytea` column (`src/db.rs:100`).
- It states no checks of server settings: `fsync`, `full_page_writes` and data
  checksums (r94, r120, r135).
- It states no supported Postgres version range (r76).
- It gives no connection timeouts (r78).
- It has no schema-version check (r25). The DDL migrates with ad-hoc
  `ADD COLUMN IF NOT EXISTS`.

Change: state the real schema. Add `SchemaVersion`, `ChecksServerDurability`,
`SupportedPostgresVersions` and `BoundsEveryWait` (Part 4).

**CreatesFiles** {revise}
The semantics are right. Add a postcondition and a crash clause: the entry and
its attributes appear atomically, or not at all. Lockify (r41) shows that the
create should be one `INSERT … ON CONFLICT DO NOTHING RETURNING`. The database's
uniqueness check then decides EEXIST in the same statement, with no prior
existence read. Small-file inlining (r07, r38) belongs here once the schema
changes.

**ReadsFiles** {revise}
It has no visibility clause. Under the single-mount linearizable model
(Part 4.2), a read must first merge pending buffered writes on the same path from
*every* handle (r129). It does not state holes and sparse reads: unwritten ranges
read as zero. It does not state that a replica read is never used for file data
today.

**WritesFiles** {wrong}
"Updates touched block chunks or buffers dirty blocks" leaves the guarantee
undefined, and that gap is how Bug 1 shipped. Add the SysSpec-style
postcondition (r43):

> for every byte *b* outside [offset, offset+len), content′(*b*) = content(*b*);
> size′ = max(size, offset+len); bytes in a gap read as zero.

Also state the durability point: none until flush or fsync (Part 4.2). Also
state that a successful `write()` does not imply persistence. Ananke (r46) shows
how a daemon crash then loses data that was acknowledged without error. Users
must be told that.

**UnlinksFiles** {revise}
"Deletes the row and removes the inode from the path map". Missing:

- One transaction for the entry and its blocks. The feature docs admit that
  unlink is not wrapped. Fix it rather than document it (r61, r68).
- POSIX open-after-unlink: a file that is unlinked while still open must stay
  readable until the last close. pgfs deletes the rows at once.

Either implement open-after-unlink (an orphan list, as in the split delete of
r68) or declare it unsupported in `DeclaresUnsupportedOps`. Git and many editors
rely on it.

**CreatesDirectories** {revise}
Make it a single statement, as for create (r41). It has no requirement for the
parent's mtime or ctime; see `DirectoryTimes` (Part 4.6) and r36 and r127.

**RemovesDirectories** {revise}
The emptiness check and the delete must be one transaction, with the parent row
locked. Otherwise a concurrent create can land between the check and the delete.
The feature docs admit rmdir is not wrapped. The concurrency clause must name
the lock (r11, r65).

**ListsDirectories** {wrong}
"Offsets are synthetic (1-based index); the reply … resumes on the next call."
The code re-runs the full `SELECT … ORDER BY name` on every call
(`src/fs.rs:887`, `src/db.rs:488`) and indexes into the fresh list. A concurrent
create or unlink between calls shifts the index, so entries are skipped or
duplicated. Every directory call costs O(n), so listing a directory of n entries
is O(n²) in total. Change: keyset pagination, where the offset is a cookie for
the last name (`name > $cookie`), and readdirplus where useful (r57, r131, r07).
The postcondition is that each entry present for the whole listing appears
exactly once.

**LooksUpEntries** {wrong}
"Allocates a stable inode number for the entry's full path". The number is
stable only within one process. It is a counter in memory (`src/fs.rs:85`). It
is never released because there is no `forget` handler, so the map grows with
every path ever looked up (r57). After a restart, the same file gets a different
number, which defeats NFS export, `find -inum`, hard links, and restart through
fd passing (r35, r46). Change: inode = `entries.ino`, persistent (Part 4.3).

**ReportsAttributes** {revise}
The attributes it lists are right. Add:

- the attribute TTL contract;
- that `size` reflects pending buffered writes on the same mount;
- that directory times are the epoch today (documented, but not as a
  requirement);
- `blocks` and `blksize` semantics for `du`.

**TruncatesFiles** {revise}
Add the postcondition: bytes beyond the new size are gone, and an extension reads
as zero. Add an atomicity clause. Add the interaction with pending buffers: a
truncate must discard or trim buffered blocks beyond the new size on every
handle, or a later flush resurrects them. That is the same class of bug as Bug 3.

**RenamesEntries** {revise}
The POSIX error matrix is complete and correct. Missing:

- An exact-subtree invariant. The descendant match must be prefix-exact, never
  `LIKE` with unescaped `_` and `%`. That is Bug 2.
- A concurrency clause. Directory renames serialise on one advisory lock, which
  is the loop-check point (r16, r127). Rows lock in ascending-ino order (r11,
  r65).
- A visibility clause. Pending buffers of renamed files are re-keyed or flushed
  first (r129). That is Bug 3.
- A bound. With path keys, rename is one unbounded transaction over the
  subtree. Oracle FSS (r91) bounds transaction size. Ino keys make it O(1).

**RejectsInvalidRenames** {revise}
It rejects all `renameat2` flags. `RENAME_NOREPLACE` is one conditional statement
in a transaction, and `mv -n` and many tools use it. `RENAME_EXCHANGE` is a swap
of two rows. Both are cheap with ino keys. With ino keys, the "into own
descendant" check needs an ancestor walk under the rename lock (r16).

**MountsCleanly** {sound}
AutoUnmount and the empty-mountpoint precondition are right for this stage.
XFUSE (r35) describes the production alternative, which is restart without
unmount. Note it as future work that depends on persistent inode numbers.

**HandlesShutdownSignals** {sound}
A sigwait thread with a clean unmount. Add one clause: on SIGTERM, flush every
handle's buffers before unmounting, and report failures. Otherwise, a normal
stop loses acknowledged writes.

**DistinguishesExpectedErrors** {revise}
Add the errno values the literature says must be distinguished:

- EILSEQ for invalid names (r52);
- ENAMETOOLONG;
- EINTR for an interrupted request (r78);
- ETIMEDOUT or EIO for a timed-out statement, with a WARN;
- ENOSPC when the database is out of space (`53100 disk_full`). Today that would
  surface as EIO.

**ReportsCrashes** {sound}

**UsesStructuredTracing** {sound}

**InstrumentsFuseCallbacks** {wrong}
It lists `copy_file_range` and `fallocate`, but the FUSE layer implements
neither (`src/fs.rs` has no handler, which `offload.py`'s status note confirms).
It omits `release`, `forget` and `statfs`. Change: list exactly the implemented
callbacks, and generate the list from the code.

**InstrumentsDbOperations** {sound}
Add the backend PID and statement name to the span, so that a slow span can be
matched in `pg_stat_activity` (r122).

**SupportsGitInit** {revise}
git init is a good smoke test of rename-overwrite. Git also depends on several
other behaviours, so extend it to cover them:

- `chmod` on hooks, which pgfs silently ignores;
- fsync of objects and refs (`core.fsync`, r118), which is not durable today;
- open-after-unlink;
- `O_EXCL` lockfiles.

Bento (r53) uses git as the FUSE-ceiling benchmark. Add `git clone` and
`git status` on a fixed repository as a regression workload.

**ProvidesProjectLocalCluster** {revise}
`initdb --auth=trust --no-locale --encoding=UTF8` is fine for development. Add
`--data-checksums`, and assert `fsync=on` and `full_page_writes=on` (r94, r120,
r135). Offer an optional mode that puts PGDATA on LazyFS, so that crash
conformance can run locally (r135).

**ProvidesLifecycleScript** {sound}

**RunsIntegrationSuite** {revise}
Isolated shell tests with timeouts are a sound base layer. The literature says
this layer alone misses the bug classes pgfs actually has (r48, r81, r119, r128).
Add the verification requirements in Part 4.7 and a configuration matrix (r80).

**ExportsPeriodicMetrics** {revise}
See Metrics in §3.3. Lifetime counters are fine. The status note says that only
`eio` and `enoent` are incremented among the per-errno counters. Wire up the rest
or remove them.

**DumpsStateOnSigusr1** {revise}
See StateIntrospection in §3.3. A dump that waits for the next callback is
missing in exactly the case it is needed for.

**WritesHealthFiles** {sound}
This is useful supervisor plumbing.

**DetectsDeadlock** {wrong}
This is the implementation of the inverted HealthEndpoint design. Of all the
requirements, it most overstates what it does. The callback check
(`src/fs.rs:100-107`) runs only when a callback *starts*, so a dispatch thread
blocked inside a DB call is never detected. Replace it with
`InFlightAgeWatchdog` (Part 4.5).

### 3.5 app.py: features

**ProjectLocalEnvironment** {sound}
The tutorial and how-to are accurate.

**FlatFilesystem** {revise}
The status note says permissions, ownership and timestamps are "reported but not
settable". The behaviour is worse than that: `setattr` accepts mode, uid, gid,
atime and mtime and *replies success* without applying them
(`src/fs.rs:381-386`). So `chmod +x` exits 0 and changes nothing. Change: apply
them, or reply EPERM or ENOSYS (`SetsAttributes`, Part 4.6). Also move the
Durability explanation into `FsyncIsDurable` and `CommitOrderedMetadata`
(Part 4.2).

**HierarchicalDirectories** {revise}
It documents that non-UTF-8 names get ENOENT and that directory times are the
epoch. Turn both into requirements: `ByteExactNames` and `DirectoryTimes`
(Part 4.6).

**AtomicRename** {revise}
Its claims are right for a single rename. Add the Bug 2 and Bug 3 clauses, and
`RENAME_NOREPLACE`.

**ReliableMount** {sound}
It is honest about not reconnecting. Its how-to should point at
`ReconnectsWithBackoff` once that exists.

**ObservableDaemon** {revise}
It inherits the watchdog and dump problems above.

### 3.6 optimize.py

**ChunkedBlockStorageReq** {wrong}

- It says "64KB or 128KB" and names a `blocks(ino, block_no, data)` table. The
  code uses 64 KiB (`src/db.rs:310`) and keys blocks by `(parent, name,
  block_no)`. The `ino` column exists but defaults to 1 and is never set on the
  FUSE write path (`src/db.rs:204`).
- It states no write amplification. Each partial-block write rewrites a 64 KiB
  TOAST value, then WAL, then full-page images (r22, r33).
- It does not state that space is reclaimed by autovacuum, not immediately (r44).

Change: fix the facts and add `ReportsWriteAmplification`. Consider
`ALTER TABLE blocks ALTER data SET STORAGE EXTERNAL`, which avoids compressing
data that is usually incompressible, and a smaller block size for metadata-heavy
trees. Inline small files below a stated N bytes (r07, r38).

**InoBasedRekeyingReq** {revise}
The literature backs every clause (r00, r07, r11, r14, r16, r18, r91). Three
changes:

- It is the target schema, not an optimisation. Move it to app.py as
  `PersistentInodes`, `UniqueDentry` and `BlocksKeyedByIno`.
- The code dropped the `(parent_ino, name)` index that this requirement relies
  on (`src/db.rs:118`). The unique constraint is still on `(parent, name)`.
- Consider splitting `entries` into `inodes` and `dentries` now, which gives
  hard links for free (r00, r18).

Also note the latent alias. The backend's ino-keyed block path writes
`(parent='', name=<ino as text>)` (`src/db.rs:229`), which is the key a
root-level file named "42" uses. FUSE does not call that path yet.

**SparseWritePaddingReq** {revise}
It describes zero-padding a new block before `offset_in_block`. That is correct
for a *new* block. Bug 1 is the converse: a buffered partial write over an
*existing* block must read-modify-write, or it zeroes the bytes it did not touch.
State both cases as one postcondition on WritesFiles. Chardonnay (r66) adds a
rule: read the existing block before taking locks, then write under a version
check.

**ReplicaFreshnessCacheReq** {wrong}
A 500 ms cached verdict can be "fresh" while the daemon's own write, made after
the verdict, has not yet replayed on the standby. That breaks read-your-writes,
and it contradicts `FallsBackToPrimary`'s claim that "a stale replica never
produces stale reads". The literature's answer is an LSN ticket (r06, r19, r24,
r34, r49, r75). Replace it with `LsnTicketReads` (Part 4.8).

**PreparedStatementReq** {sound}
Prepared statements are standard. With connection pooling, say whether
statements are prepared per connection (the r2d2 customiser does this) or
prepared lazily.

**SynchronousCommitTuningReq** {wrong}
It justifies "a sub-second crash durability window" for "per-operation fsync
latency". That trade is legitimate for metadata operations nobody fsyncs. It
breaks the contract when it applies to fsync itself (r123), and it creates the
CAD visibility anomaly (r56). Group commit is the alternative r25 names:
`commit_delay` and concurrent connections. Change: reframe it as a *durability
policy*. Off by default for ordinary operations, with a stated window bounded by
`wal_writer_delay`. `SET LOCAL synchronous_commit = on` for the fsync, flush and
close transaction (`FsyncIsDurable`).

**UnloggedTableOptReq** {wrong}
It omits two Postgres facts:

- Unlogged tables are **truncated after a crash** of the server.
- Unlogged tables are **not replicated** to physical standbys, so `--unlogged`
  with `--replica` serves empty tables from the standby.

Change: state both, and add `ReplicaRejectsUnlogged`. CAFault (r80) is the
general lesson: flag combinations need tests.

**TransactionBatchingReq** {revise}
It is framed as a latency optimisation. The literature treats "one FUSE
operation = one transaction" as a *correctness* invariant (r61, r68, r91, r118).
Move it to app.py as `OneOpOneTransaction`. Each exception needs a crash-states
clause (r60). Separately, putting each operation's statements into one round trip
is the latency win (r82). That part can stay here.

**PgfsBackendTraitReq** {sound}
The trait exists (`src/backend.rs:140`). This is the seam that makes a reference
model possible (r128). Note the dependency in Part 4.7.

**LocalPgrxEngineReq** {revise}
It proposes a bgworker inside the postmaster with an `shm_mq` ring and SPI
plans. Two cautions from the literature:

- Disaggregate, or in this case co-locate, only where the cost is justified
  (r134). Measure first. Round-trip count (r82) and concurrency (r12, r69) are
  the larger costs today.
- A crashing bgworker restarts the whole postmaster, so the blast radius must be
  stated.

Citus (r39) shows that an extension, not a fork, is the right packaging.

**IoUringBufferRingReq** {wrong}
"Linux 6.9+ fuse-io_uring" is wrong. FUSE over io_uring was merged in Linux 6.14,
and fuser 0.14 does not support it (r17). The "zero-copy into SharedBuffers"
claim also skips the copy that Postgres itself makes into a heap and TOAST tuple.
Change: correct the version, mark the requirement as dependent on crate support,
and drop or qualify the zero-copy claim.

**RemoteAsyncBackendReq** {revise}
Async with pipelining is the right shape (r82, r87). Require an explicit timeout
and cancellation per operation, mapped to FUSE INTERRUPT (r78, r87). The stubs
exist (`src/backend.rs:310`).

**MultiThreadedConnectionPoolReq** {revise}
The literature makes concurrent dispatch the largest available win (r12, r17,
r69). But "configure pool size" is not a specification. Add:

- Ownership: workers are sharded by inode, so all data operations on a file run
  on one worker, which keeps per-file order and buffer coherence without locks
  (r130).
- A lock order: ascending ino, parent before child, and one rename lock (r11,
  r65).
- No daemon lock held across a database round trip (r42).
- A linearizability test (r128).

**BufferedWriteFlushReq** {revise}
It says 128 KB; the code uses 64 KiB. The missing clauses are the main source of
Bugs 1 and 3:

- Merge semantics with existing block contents, which is the read-modify-write
  of Bug 1.
- A visibility rule: operations that externalise state drain or merge first
  (r129).
- Fsync-failure semantics: keep the buffer on failure and report the error once
  (r90).
- A memory bound per handle and per mount, with backpressure (r57).

**KernelAttrCacheTtlReq** {revise}
A TTL is a lease without the write side (r21), and Chubby (r04) argues against
TTL as the coherence mechanism. Change:

- State that cross-mount staleness is bounded by the TTL and that this is not
  consistency.
- Allow a TTL per entry: long for read-mostly trees, zero for files being
  written (r09, r21).
- Cache negative entries (r04).
- Document the cold-cache overload risk when the TTL is raised (r67).

**KernelWritebackCacheReq** {revise}
The writeback cache is the largest single win for the data path (r12). Its
preconditions are not stated:

- the kernel owns mtime and size while the cache is on;
- writes can arrive after `release`;
- `O_APPEND` must be handled;
- a crash test must cover the cache mode (r77).

It also says 128 KB.

**DualInterfaceArchitecture** {revise}
PLANNED. It inherits the cautions on pgrx and io_uring above. The Roadmap option
from the earlier session is still open.

**OptimizationRoadmap** {revise}
PARTIAL. Reorder it: correctness items first (ino keys, one operation per
transaction, the durability policy), then concurrency, then round trips, then
the long-horizon engines.

### 3.7 offload.py

**CopyFileRangeReq** {revise}
Server-side `INSERT … SELECT` is exactly right (r55 shows the clone win). It
requires the "single transaction" it already states. Add the case where the
ranges are unaligned: the head and tail blocks need read-modify-write, the same
rule as WritesFiles. The status note says the aligned path in the backend runs
two statements without a transaction. Fix that to meet this requirement.

**FallocatePunchHoleReq** {revise}
Add partial-block punching: zero the bytes inside a block that is only partly
covered. Add the postcondition: the range reads as zero, and the size is
unchanged with `KEEP_SIZE`. Add space accounting: blocks are freed at VACUUM.

**ReflinkBlockSharingReq** {revise}
Reference-counted CoW blocks are supported (r10, r37, r55). Before building it,
add a refcount invariant to `pgfs check`: the refcount equals the number of
referencing rows. Add a crash clause: the refcount update and the reference
insert happen in one transaction.

**CloneSubtreeReq** {sound}
A set-based clone in one transaction. With ino keys, it needs an ino remap table
inside the transaction. Bound the transaction size (r91), or accept a long
transaction for large trees and say so.

**ContentSearchOffloadReq** {sound}
A trigram index on block data is plausible. TiDB (r34) suggests serving it from a
replica, a learner-style analytics node, so that it does not load the primary.
Indexing binary 64 KiB chunks misses matches that cross block boundaries. Say so.

**KernelOffloadingAcceleration** (feature) {sound}
The status note is accurate: the backend implements it and FUSE does not call it.

### 3.8 replica.py

**StartsDockerStandby** {sound}
`pg_basebackup -R`, idempotent, bound to loopback only. Fine for development.

**RequiresPrimaryTcpListener** {sound}

**AcceptsReplicaConnection** {revise}
Verify the standby's identity at connect: system identifier and timeline. An
LSN comparison alone does not prove that the node still belongs to this cluster
after a failover (r05, r25). Also refuse `--replica` together with `--unlogged`.

**ServesReadsFromReplica** {wrong}
It lists `read` among the routed operations. The ReplicatedFilesystem status
note says `read` always goes to the primary. Make the requirement match the
code, or route `read` under an LSN ticket.

**WritesStayOnPrimary** {sound}
Reading the inputs of a decision on the primary is the correct rule (r06, r34).

**DetectsReplicationLag** {revise}
"Fresh iff replay ≥ the primary's current LSN" is read with two separate queries
on two nodes, so it is not atomic. On a busy primary it is almost never true, so
the standby is rarely used. A ticket compares against *the daemon's own last
commit LSN*, not the primary's head. That comparison is both correct and usually
satisfied (r75).

**FallsBackToPrimary** {wrong}
"A stale replica never produces stale reads" is false under the 500 ms cache
(§3.6). Rewrite it once LSN tickets exist. Add a proportional rule: if the
standby is consistently slower than the primary, stop routing to it (r92, r03).

**ReplicaObservability** {sound}
Add replay lag in bytes and seconds.

**ReplicatedFilesystem** (feature) {revise}
"Eventually consistent with … lag continuously bounded" overstates. Restate it
as read-your-writes under tickets. Note that physical replication copies
corruption faithfully when checksums are off, so a standby is not protection
against corruption (r121).

### 3.9 shard.py

The whole module describes v0.1. `design/sharded_filesystem.md` (v0.2) already
replaces it, and the literature supports v0.2.

**AcceptsShardTopology** {retire}
`--shards` and `--replication-factor` put redundancy in pgfs flags. Okapi (r62)
argues for keeping placement and redundancy separate. Postgres provides
redundancy for each data set in v0.2.

**ComputesDeterministicPlacement** {retire}
Consistent hashing by top-level name has no groups, failure domains or weights.
Chain Replication (r01) measured ring placement as worst for availability, and
CRUSH (r03) adds the indirection this design lacks. v0.2 uses a placement table.

**ReplicatesToSubsetOfHosts** {retire}
RAID-5-like subset mirroring done by pgfs itself. v0.2 delegates redundancy to
Postgres synchronous standbys.

**AllocatesGloballyUniqueInodes** {revise}
Encoding the set in the high bits ties identity to placement, so a file that
moves sets changes meaning. v0.2 keeps inodes in one metadata database (MDB)
sequence, which is simpler. Keep the requirement and rewrite it to that design.

**AllocatesInodesViaSetLeader** {retire}
Leader takeover from a "disjoint reserved region" is ad-hoc leader election. It
is unnecessary when the MDB allocates inodes.

**ServesReadsFromReplicationSet** {retire}
Quorum reads with the highest `ver` are replaced by v0.2 reads from a set's
primary or ticketed standby.

**VersionsMutations** {retire}
`ver = client_id<<32 | seq` is last-writer-wins without causality, and the
literature avoids it (r23, r30). In v0.2, `data_ver` is assigned by the MDB
commit.

**FansOutWritesToSetMembers** {retire}
Client fan-out to replicas is the costly, weak design that Chain Replication
(r01) compares against.

**RequiresWriteQuorum** {retire}
A compensating delete is not a consensus protocol. The rollback can itself
fail, and nothing orders the operations (r23). v0.2 uses
`synchronous_standby_names = 'ANY 1 (…)'` (r72).

**DegradesGracefullyOnHostFailure** {retire}
Healthy and degraded is too coarse. RADOS (r05) uses up/down × in/out. Recovery
by re-applying "missing mutations" has no log to replay from.

**SupportsCrossSetRename** {retire}
Snapshot, apply and delete with compensation is a hand-rolled two-phase commit.
In v0.2 a rename is metadata-only in the MDB, and data moves lazily.

**SupportsSharedMounts** {revise}
Keep it, rewritten for v0.2: NOTIFY coherence with sequence numbers and gap
detection (r59, r24), close-to-open, and direct I/O for write-shared files (r40,
r47). It honestly says that cross-mount visibility is bounded by `attr_ttl`;
keep that sentence.

**ShardObservability** {retire}
Rewrite it for the v0.2 vocabulary: per-set lag, epochs, drain progress, GC
backlog.

**ShardedFilesystem** (feature) {retire}
Replace it with the v0.2 requirement set in Part 4.9.

### 3.10 perf.py

**CpuProfilingOnDemand** {sound}

**SpanTraceOnShutdown** {sound}

**ZeroOverheadWhenDisabled** {sound}

**ProfileDrivenBenchmark** {revise}
Name fixed workloads so that results can be compared over time:

- fio, sequential and random;
- mdtest, for creates and stats;
- `git clone` and `git status` on a pinned repository (r53);
- a kernel tarball untar (r09, r26).

Report results against a passthrough-FUSE baseline, not ext4 (r53). Add WAL
bytes per byte written (r33).

**OnDemandProfiling** (feature) {sound}

---

## Part 4. Requirements the literature says should exist

Each entry gives the module, the name, a one-line contract and the sources.

### 4.1 Contract contexts (err.py)
- `Invariant`: what holds before and after every operation, under concurrency
  (r43, r60).
- `CrashStates`: the set of states a crash at any point may leave, relative to
  the last acknowledged operation (r60, r118, r123).
- `DurabilityPoint`: the exact event after which an effect survives a crash of
  the daemon, the host or Postgres (r56, r129).
- `Concurrency`: what a concurrent observer may see, and the lock order (r11,
  r65).

Every `Req` inherits these four slots.

### 4.2 Durability and visibility (app.py)
- `FlushesOnClose`: flush and release commit the handle's dirty state in one
  transaction before replying. A failure reaches `close()` as EIO (r90, r46).
- `FsyncIsDurable`: fsync replies only after a commit with
  `synchronous_commit = on`, or `remote_apply` on a replicated set (r123, r56).
- `FsyncFailureSemantics`: on failure, keep the dirty buffer, report the error
  once to each fd that wrote, and never present uncommitted data as committed
  after a reconnect (r90).
- `CommitOrderedMetadata`: metadata operations are atomic and persist in commit
  order. The asynchronous-commit window is stated as a number (r118, r56).
- `ConsistencyModel`: linearizable on one mount (buffers drained), close-to-open
  across mounts (r40, r129, r133).
- `DrainsBuffersBeforeExternalising`: read, getattr, lookup, readdir, rename,
  setattr, link and copy_file_range first merge or flush every handle's pending
  buffers for that path (r129).
- `FlushesOnShutdown`: SIGTERM flushes every handle before unmounting.

### 4.3 Identity and schema (app.py)
- `PersistentInodes`: the FUSE inode equals `entries.ino`. It is stable across
  restarts and is not reused while referenced (r00, r11, r35).
- `UniqueDentry`: `UNIQUE (parent_ino, name)` and a foreign key on `parent_ino`.
  The root is the only row with `ino = parent_ino` (r54).
- `BlocksKeyedByIno`: blocks have primary key `(ino, block_no)` only (r07, r11).
- `HandlesForget`: forget and batch_forget evict state, so memory is bounded by
  the number of live inodes (r57).
- `SchemaVersion`: a `pgfs_meta` version row, explicit migrations, and a refusal
  to mount an unknown version (r25).
- `InlinesSmallFiles`: data of N bytes or fewer lives in the inode row (r07,
  r38).
- `OpenAfterUnlink`: implement it with an orphan list, or declare it unsupported
  (r68).

### 4.4 Transactions (app.py)
- `OneOpOneTransaction`: each FUSE operation is one transaction. Each exception
  carries a `CrashStates` clause (r61, r68, r91).
- `BoundedTransactions`: no operation's transaction grows with tree size, which
  ino keys make true for rename (r91).
- `OneRoundTripPerOp`: each operation's statements are sent together (r82).

### 4.5 Failure model (app.py)
- `DatabaseStateMachine`: Connected, Broken, Reconnecting, Revalidating,
  Connected, with each transition specified (r70, r15).
- `ReconnectsWithBackoff`: exponential backoff with jitter and a cap. Fail fast
  with EIO while disconnected; do not queue (r67, r125, r85).
- `IdempotentMutations`: each mutation records `(mount_id, op_seq)` in a client
  table in the same transaction (r28, r20).
- `BoundsEveryWait`: connect, statement and lock timeouts plus keepalives, each
  with a stated value (r78).
- `RetriesSerializationFailures`: SQLSTATE 40001 and 40P01 are retried a bounded
  number of times (r68).
- `HonoursInterrupt`: FUSE INTERRUPT cancels the backend query (r78, r87).
- `InFlightAgeWatchdog`: alarm on the age of the oldest in-flight request, from
  its own thread (r124).
- `SlowDatabaseSignal`: WARN when p99 stays above N times the baseline for M
  minutes (r88, r122).

### 4.6 POSIX surface (app.py)
- `SetsAttributes`: apply mode, uid, gid and times, or reply EPERM. Never succeed
  silently.
- `DeclaresUnsupportedOps`: a table listing each FUSE operation as supported,
  ENOSYS or EOPNOTSUPP (r48, r13).
- `ReportsStatfs`: real figures from the database size and free space (r22,
  r44).
- `ByteExactNames`: names compare under `COLLATE "C"`. Invalid names return
  EILSEQ, and names over 255 bytes return ENAMETOOLONG (r52).
- `StableReaddirCookies`: keyset pagination. Each entry appears once (r57,
  r131).
- `DirectoryTimes`: directory mtime and ctime are maintained, with deltas if hot
  (r36, r127).
- `RenameNoReplace`: support `RENAME_NOREPLACE`, and `RENAME_EXCHANGE` once ino
  keys exist.

### 4.7 Verification (a new spec/verify.py)
- `ReferenceModel`: an in-memory filesystem behind `PgfsBackend`, also used as
  the unit-test mock (r128).
- `ModelConformance`: proptest operation sequences run against the model and
  pgfs, with outputs and invariants compared after each step (r128, r48, r81).
- `CrashConformance`: Postgres runs on LazyFS. Lost and torn writes are
  injected, and PG or the daemon is killed. The recovered state must be in the
  model's crash set (r135, r123).
- `DbFaultInjection`: a feature-gated wrapper that fails the Nth Db call with
  each error class. Every error branch is covered (r121, r119).
- `ConfigMatrix`: replica × unlogged × attr-ttl × writeback cache (r80).
- `PgfsCheck`: SQL invariants (orphans, cycles, size against the last block,
  dangling blocks, refcounts) plus `amcheck`. It runs in CI and on demand (r54,
  r58, r135).
- `DivergenceTests`: force each kind of divergence (rename through SQL behind
  the daemon's back, standby lag, a cache past its TTL) and check that it
  resyncs (r86).
- `LinearizabilityCheck`: shuttle tests once dispatch is concurrent (r128).

### 4.8 Replica (replica.py)
- `LsnTicketReads`: serve from the standby only if its replay LSN has reached
  the daemon's last commit LSN. Otherwise wait for a bounded time, then use the
  primary (r75, r19, r34).
- `VerifiesStandbyIdentity`: check the system identifier and timeline at connect
  and after a reconnect (r05, r25).
- `ReplicaRejectsUnlogged`: refuse `--replica` together with `--unlogged`.

### 4.9 v0.2 sharding (shard.py, rewritten)
- `MetadataDatabase`: one MDB holds the whole namespace and the inode sequence.
  It scales up before it scales out (r36, r91).
- `DataSetsTable`: data-set members with failure domain, weight, and up/down ×
  in/out state (r03, r05).
- `PerFilePlacement`: weighted by capacity and recovery load (r84, r93).
- `OrderedWrites`: data commits on the set before the MDB row references it.
  Unreferenced blocks are harmless (r133, r14, r60).
- `SetEpochsAndFencing`: seal, then switch. Every data write carries the epoch
  and the set fences on it (r73, r71, r04).
- `QuorumSyncCommit`: `synchronous_standby_names = 'ANY 1 (…)'` on each set and
  on the MDB (r72).
- `LearnerFirstAdd`: a new member catches up before it counts (r08).
- `RateLimitedDrain`: data moves in bounded batches that yield to foreground
  traffic (r95).
- `LazyGc`: unreferenced blocks are reclaimed asynchronously and the backlog is
  visible (r31, r44).
- `NotifyCoherence`: notifications carry ino and a sequence number. A gap or a
  reconnect invalidates everything cached (r59, r24).
- `SingleSetDefault`: the MDB and the data set are one database unless the
  configuration says otherwise (r134).

### 4.10 Cluster and operations
- `ChecksServerDurability`: warn or refuse when `fsync`, `full_page_writes` or
  data checksums are off (r94, r120, r135).
- `SupportedPostgresVersions`: a stated range, with CI run against each (r76).
- `ReportsWriteAmplification`: WAL bytes per user byte, and bloat (r33, r51,
  r22).

---

## Part 5. Contradictions inside the spec

1. `GlobalMutableState` assumes single-threaded dispatch, while
   `MultiThreadedConnectionPoolReq` requires a pool.
2. `FallsBackToPrimary` says a stale replica never produces stale reads, while
   `ReplicaFreshnessCacheReq` caches the verdict for 500 ms.
3. `ServesReadsFromReplica` routes `read` to the replica, while the
   ReplicatedFilesystem status note says `read` always goes to the primary.
4. Block size: `ChunkedBlockStorageReq` says "64KB or 128KB", and
   `BufferedWriteFlushReq`, `KernelWritebackCacheReq` and offload.py say 128 KB.
   The code uses 64 KiB.
5. The FlatFilesystem status note says attributes are "not settable", while
   `setattr` returns success.
6. `InstrumentsFuseCallbacks` lists `copy_file_range` and `fallocate`, while the
   offload status note says FUSE does not call them.
7. `ConnectsToPostgres` presents ino as the schema's key, while
   `InoBasedRekeyingReq` describes the rekeying as future work.
8. The HealthEndpoint and DetectsDeadlock wording claims to catch deadlock in the
   FUSE session, while the mechanism they describe cannot (§3.3).
9. `SynchronousCommitTuningReq` cites "fsync latency" as the benefit, while
   FlatFilesystem documents that fsync does not ensure durability.

## Part 6. Spec against code (checked in this pass)

| Spec says | Code does | Where |
|---|---|---|
| Inode stable for the path | per-process counter; no `forget` | `src/fs.rs:84-85`, no `forget` fn |
| B-tree on `(parent_ino, name)` | that index is dropped; uniqueness only on `(parent, name)` | `src/db.rs:118`, `:109` |
| `blocks(ino, block_no, data)` | key `(parent, name, block_no)`; `ino` defaults to 1 on the FUSE path | `src/db.rs:123-130`, `:204` |
| (nothing) | the backend ino path writes blocks at `('', <ino>)`, aliasing root files with numeric names | `src/db.rs:229` |
| 64 or 128 KiB blocks | 64 KiB | `src/db.rs:310` |
| `synchronous_commit` tuned | off on every connection, fsync included | `src/db.rs:71` |
| Deadlock detection | liveness check on callback entry | `src/fs.rs:100-107` |
| Readdir resumes by offset | full re-query per call | `src/fs.rs:887`, `src/db.rs:488` |
| Attributes not settable | `setattr` ignores mode, uid, gid and times and replies success | `src/fs.rs:381-386` |
| (nothing) | no statement, lock or connect timeouts; no keepalives | `src/` |

## Part 7. Recommended order

1. **Contracts.** Part 4.1, then `FsyncIsDurable`, `FlushesOnClose`,
   `ConsistencyModel`, `DrainsBuffersBeforeExternalising` and the WritesFiles
   postcondition. This settles Bugs 1 and 3 in the spec.
2. **Verification.** `ReferenceModel`, `ModelConformance`, `DbFaultInjection`,
   `PgfsCheck`, then `CrashConformance` on LazyFS. Bugs 1–4 should appear as
   failing cases before they are fixed.
3. **Identity.** `SchemaVersion`, `UniqueDentry`, `BlocksKeyedByIno`,
   `PersistentInodes` and `HandlesForget`. This settles Bug 4 and the latent
   alias.
4. **Transactions.** `OneOpOneTransaction`, then fix the unlink, rmdir and
   offload exceptions.
5. **Failure model.** `BoundsEveryWait`, `DatabaseStateMachine`,
   `ReconnectsWithBackoff`, `IdempotentMutations` and `InFlightAgeWatchdog`.
   This settles Bug 5.
6. **POSIX surface.** `SetsAttributes`, `DeclaresUnsupportedOps`,
   `StableReaddirCookies`, `ByteExactNames` and `ReportsStatfs`.
7. **Factual fixes.** io_uring, block size, unlogged-table caveats, and the
   contradictions in Part 5.
8. **Concurrency.** Inode-sharded workers, lock order and the linearizability
   check.
9. **Replica.** `LsnTicketReads` and `VerifiesStandbyIdentity`.
10. **shard.py.** Rewrite it to v0.2 (Part 4.9).
11. **Long horizon.** The pgrx engine, io_uring and the remote async backend.

Steps 1–7 change only the spec until implementation starts. None of them commits
the code to a design that the literature argues against.

---

## Appendix A. Papers cited

| Id | Paper |
|---|---|
| 00 | The Google File System (SOSP03) |
| 01 | xFS: Serverless Network File Systems (SOSP95) |
| 02 | River: Cluster I/O (IOPADS99) |
| 03 | Swift: Distributed Disk Striping (1991) |
| 04 | NASD: Network-Attached Secure Disks (ASPLOS98) |
| 05 | AFS: Scale and Performance in a Distributed File System (TOCS88) |
| 06 | Coda and InterMezzo (Linux Expo 99) |
| 07 | Harp: Replication in the Harp File System (SOSP91) |
| 08 | Lustre whitepaper (2002) |
| 09 | A Case for RAID (SIGMOD88) |
| 10 | GPFS (FAST02) |
| 11 | The Global File System (1996) |
| 12 | Frangipani (SOSP97) |
| r00 | Inversion File System (USENIX 93) |
| r01 | Chain Replication (OSDI04) |
| r02 | Ceph (OSDI06) |
| r03 | CRUSH (SC06) |
| r04 | Chubby (OSDI06) |
| r05 | RADOS (PDSW07) |
| r06 | CRAQ (ATC09) |
| r07 | TableFS (ATC13) |
| r08 | Raft (ATC14) |
| r09 | IndexFS (SC14) |
| r10 | CalvinFS (FAST15) |
| r11 | HopsFS (FAST17) |
| r12 | To FUSE or Not to FUSE (FAST17) |
| r13 | ExtFUSE (ATC19) |
| r14 | Tectonic (FAST21) |
| r15 | FoundationDB (SIGMOD21) |
| r16 | InfiniFS (FAST22) |
| r17 | RFUSE (FAST24) |
| r18 | Fire-Flyer / 3FS (SC24) |
| r19 | Amazon Aurora (SIGMOD17) |
| r20 | λFS (ASPLOS23) |
| r21 | Leases (SOSP89) |
| r22 | Log-Structured File System (SOSP91) |
| r23 | Paxos Made Simple (2001) |
| r24 | ZooKeeper (ATC10) |
| r25 | HDFS (MSST10) |
| r26 | Haystack (OSDI10) |
| r27 | Windows Azure Storage (SOSP11) |
| r28 | Viewstamped Replication Revisited (2012) |
| r29 | Erasure Coding in Azure, LRC (ATC12) |
| r30 | Spanner (OSDI12) |
| r31 | f4 (OSDI14) |
| r32 | PolarFS (VLDB18) |
| r33 | BlueStore (SOSP19) |
| r34 | TiDB (VLDB20) |
| r35 | XFUSE (ATC21) |
| r36 | SingularFS (ATC23) |
| r37 | Petal (ASPLOS96) |
| r38 | Size Matters: small files in HopsFS (Middleware18) |
| r39 | Citus (SIGMOD21) |
| r40 | Caching in the Sprite Network File System (TOCS88) |
| r41 | Lockify (FAST26) |
| r42 | CetoFS (FAST26) |
| r43 | Sharpen the Spec, Cut the Code: SysSpec (FAST26) |
| r44 | DisCoGC (FAST26) |
| r45 | CoFS (FAST26) |
| r46 | Ananke (FAST25) |
| r47 | Combining Buffered and Direct I/O in DFS (FAST24) |
| r48 | Metis (FAST24) |
| r49 | Ionia (FAST24) |
| r50 | Fisc (FAST23) |
| r51 | Pangu 2.0 (FAST23) |
| r52 | Unsafe at Any Copy (FAST23) |
| r53 | Bento (FAST21) |
| r54 | pFSCK (FAST21) |
| r55 | How to Copy Files: BetrFS clones (FAST20) |
| r56 | Consistency-Aware Durability (FAST20) |
| r57 | ScaleCheck (FAST19) |
| r58 | T2C (OSDI25) |
| r59 | Skybridge (OSDI25) |
| r60 | PoWER Never Corrupts (OSDI25) |
| r61 | WOFS (OSDI25) |
| r62 | Okapi (OSDI25) |
| r63 | SpecLog (OSDI25) |
| r64 | SquirrelFS (OSDI24) |
| r65 | RefFS (OSDI24) |
| r66 | Chardonnay (OSDI23) |
| r67 | Metastable Failures in the Wild (OSDI22) |
| r68 | DaisyNFS (OSDI22) |
| r69 | GoJournal (OSDI21) |
| r70 | VeriBetrKV (OSDI20) |
| r71 | Partial Network Partitions (OSDI20) |
| r72 | Copilot (OSDI20) |
| r73 | Virtual Consensus in Delos (OSDI20) |
| r74 | Assise (OSDI20) |
| r75 | FlightTracker (OSDI20) |
| r76 | SQLancer (OSDI20) |
| r77 | Crash Consistency in Block-Level Caching (ATC25) |
| r78 | FiDe (ATC25) |
| r79 | Sieve: fail-slow bugs (ATC25) |
| r80 | CAFault (ATC25) |
| r81 | Monarch (ATC24) |
| r82 | Distributed Transactions in DynamoDB (ATC23) |
| r83 | RubbleDB (ATC23) |
| r84 | Data Placement for Balanced Recovery (ATC23) |
| r85 | Speculative Recovery (ATC22) |
| r86 | Modulo (ATC22) |
| r87 | DepFast (ATC22) |
| r88 | NVMe SSD Failures in the Field (ATC22) |
| r89 | Lodic (ATC21) |
| r90 | Can Applications Recover from fsync Failures? (ATC20) |
| r91 | Oracle Cloud FSS (ATC19) |
| r92 | IASO (ATC19) |
| r93 | FlexGroup Volumes (ATC19) |
| r94 | Protocol-Aware Recovery (FAST18) |
| r95 | Dayu (ATC19) |
| r100 | Morph (SOSP24) |
| r118 | All File Systems Are Not Created Equal (OSDI14) |
| r119 | Simple Testing Can Prevent Most Critical Failures (OSDI14) |
| r120 | Torturing Databases for Fun and Profit (OSDI14) |
| r121 | Redundancy Does Not Imply Fault Tolerance (FAST17) |
| r122 | Fail-Slow at Scale (FAST18) |
| r123 | CrashMonkey and ACE (OSDI18) |
| r124 | Gray Failure (HotOS17) |
| r125 | Metastable Failures in Distributed Systems (HotOS21) |
| r126 | Socrates (SIGMOD19) |
| r127 | Mantle (SOSP25) |
| r128 | Lightweight Formal Methods for ShardStore (SOSP21) |
| r129 | Skyros: nil-externality (SOSP21) |
| r130 | uFS: Filesystem Semi-Microkernel (SOSP21) |
| r131 | HMFS (SoCC25) |
| r132 | Fletch (arXiv25) |
| r133 | SwitchΔ (ICDE26) |
| r134 | Disaggregation (VLDB25) |
| r135 | When Amnesia Strikes: LazyFS (VLDB24) |

## Appendix B. Papers not available (ACM-only)

These were not read. The first three are the most likely to change a conclusion:

- r96 Fawkes (SOSP25): durability bugs in DBMSs found by checking recovered
  state. Bears on T2 and `CrashConformance`.
- r115 CFS (EuroSys23): metadata service with pruned critical sections. Bears on
  T1 and T10.
- r113 Volley (EuroSys24): write-read ordering in disaggregated storage. Bears on
  `OrderedWrites`.

The others:

- r97 cache_ext (SOSP25)
- r98 Aeolia (SOSP25)
- r99 Optimistic Recovery (SOSP25)
- r101 feedback-driven fault injection (SOSP24)
- r102 LazyLog (SOSP24)
- r103 Trio (SOSP23)
- r104 Acto (SOSP23)
- r105 QuePaxa (SOSP23)
- r106 LineFS (SOSP21)
- r107 upgrade failures (SOSP21)
- r108 Shard Manager (SOSP21)
- r109 Log-structured Protocols in Delos (SOSP21)
- r110 Cheetah (EuroSys25)
- r111 Themis (EuroSys25)
- r112 version certificate recovery (EuroSys24)
- r114 SplitFT (EuroSys24)
- r116 Omni-Paxos (EuroSys23)
- r117 Fail through the Cracks (EuroSys23)

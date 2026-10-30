# Reference: Reliable Mount & Shutdown

!!! warning "Status: Partially implemented"
    AutoUnmount, clean shutdown that commits every buffer and runs the synchronous barrier, health files, bounded waits, the connection state machine with reconnect and revalidation (Bug 5), op_log idempotency, serialization retries, the in-flight watchdog, the slow-database signal and the SQLSTATE table are built and tested. The fuser 0.14 crate never delivers FUSE_INTERRUPT to the filesystem, so `failure.HonoursInterrupt` is limited to the watchdog's cancels.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

[Tutorial](../tutorials/reliablemount.md) · [How-to](../how-to/reliablemount.md) · **Reference** · [Explanation](../explanation/reliablemount.md)

---

## Startup

1. Connect with the bounded waits below; check the server version,
   durability settings and schema version.
2. On a scratch filesystem emptied by a crash, run
   `pgfs.scratch_reset()`.
3. Acquire the lease (read-write) or register the session (read-only).
4. Reclaim orphans; open one connection per worker; mount with
   `AutoUnmount` and `default_permissions`.
5. Write the pid and ready files; notify systemd.

## Bounded waits

| Setting | Value |
|---|---|
| `connect_timeout` | 5 s |
| `statement_timeout` | 30 s (housekeeping 60 s) |
| `lock_timeout` | 5 s |
| `idle_in_transaction_session_timeout` | 60 s |
| TCP keepalives | 10 s idle, 5 s interval, 3 probes; `tcp_user_timeout` 30 s |

## Connection states

| State | New requests |
|---|---|
| Connected | served |
| Broken, Reconnecting, Revalidating | EIO at once; the in-flight request waits for revalidation |
| Fenced | EIO until remount |

## Exit status

| Status | Meaning |
|---|---|
| 0 | clean unmount, every buffer committed |
| 1 | the run ended but some buffered data could not be committed |
| 2 | could not start (error chain printed as `pgfs: …`) |


## Specification requirements

`ReliableMount` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "MountsCleanly"
    `spec.app.MountsCleanly`

    `pgfs mount VIEW MOUNTPOINT` mounts with `AutoUnmount`, so the
    kernel removes the mount if the process exits for any reason. It
    also sets `default_permissions` (`security.EnforcesPermissions`) and
    names the mount `pgfs-<first 8 hex digits of fs_uuid>` in
    `/proc/mounts`; FUSE's statfs reply has no field for a filesystem
    id. If `AutoUnmount` is refused,
    for example when `/etc/fuse.conf` lacks `user_allow_other`, the
    daemon mounts without it and logs a WARN saying a crashed daemon
    will leave a stale mount.

    The mountpoint must be an existing, empty directory that is not
    already mounted. The mount session or tree lease is registered in
    `mounts` (`consistency.ExclusiveTreeLease`) before the mount
    appears, and released only after the unmount, so no other mount can
    write the tree while this mount's kernel caches still exist.

    **Errors.** EBUSY if the tree is mounted read-write elsewhere. The
    error names the holder.

??? abstract "HandlesShutdownSignals"
    `spec.app.HandlesShutdownSignals`

    SIGINT, SIGTERM and SIGHUP start a clean shutdown on a dedicated
    `sigwait` thread, which never blocks dispatch. The shutdown commits
    every buffer and runs `pgfs.sync_barrier` so every earlier
    asynchronous commit is flushed to WAL (`durability.FlushesOnShutdown`),
    calls `session.unmount()`, and then releases the mount session or
    lease synchronously (`synchronous_commit = on`, `MountsCleanly`). If
    the unmount fails, the lease is not released: `AutoUnmount` removes
    the mount when the process exits, and the lease expires on its own.

??? abstract "WritesHealthFiles"
    `spec.app.WritesHealthFiles`

    After connecting to Postgres the daemon writes its PID to
    `/tmp/pgfs-<hash>.pid`, prints `health pid file: <path>` on stdout and
    logs it. After the FUSE session is created it writes
    `/tmp/pgfs-<hash>.ready` containing `ready`. `<hash>` is 16 hex digits
    derived from the mountpoint argument exactly as given. Both files are
    removed when `run()` returns.

??? abstract "ProvidesLifecycleScript"
    `spec.app.ProvidesLifecycleScript`

    `scripts/pgfs.sh up|down|status|run` owns the mount lifecycle.
    `up` starts Postgres if needed, runs `cargo build`, sweeps a stale mount
    whose daemon is gone, starts `target/debug/pgfs` detached with output
    appended to `testdata/pgfs.log`, and waits up to 10 s for the mount to
    appear in `/proc/mounts`. `down` sends SIGTERM, waits up to 10 s, then
    escalates to `fusermount3 -u`, reports the holders of a busy mount,
    and finally lazy-unmounts. `status` reports mount, daemon and Postgres
    state and flags ghost mounts. `run` builds and execs the daemon in the
    foreground. `PGFS_MOUNT` overrides the default `testdata/mnt`.

??? abstract "BoundsEveryWait"
    `spec.failure.BoundsEveryWait`

    Every wait on the database has a stated bound, set when the
    connection opens:

    | Setting | Value | Option |
    |---|---|---|
    | `connect_timeout` | 5 s | `--connect-timeout` |
    | `statement_timeout` (operations) | 30 s | `--statement-timeout` |
    | `statement_timeout` (housekeeping) | 60 s | — |
    | `lock_timeout` | 5 s | `--lock-timeout` |
    | `idle_in_transaction_session_timeout` | 60 s | — |
    | TCP keepalive idle, interval, count | 10 s, 5 s, 3 | — |
    | `tcp_user_timeout` | 30 s | — |

    A synchronous commit waiting for a standby is not covered by
    `statement_timeout`. The watchdog covers it (`InFlightAgeWatchdog`).
    When a cancel lands during a synchronous-replication wait
    (`SyncRepWaitForLSN`), Postgres commits locally and returns success
    with a `WARNING`; the daemon detects this and runs the synchronous
    barrier (`durability.FsyncIsDurable`,
    `durability.FsyncForcesEarlierCommits`).

    **Errors.** A statement or lock timeout replies ETIMEDOUT and is
    logged at WARN with the statement name and the elapsed time. A
    synchronous-replication wait cancelled by the watchdog replies EIO.

    **Sources.** FiDe r78 (timeouts are where failure detection goes
    wrong), Fail-Slow at Scale r122.

??? abstract "DatabaseStateMachine"
    `spec.failure.DatabaseStateMachine`

    Each database connection is in exactly one state:

    | State | Operations | Leaves when |
    |---|---|---|
    | Connected | run normally | a connection-class error (SQLSTATE `08*`, `57P01`–`57P03`, socket error) → Broken |
    | Broken | new requests fail fast with EIO; the one in-flight mutation awaits reconnect (`IdempotentMutations`) | at once → Reconnecting |
    | Reconnecting | new requests fail fast with EIO | a connection opens → Revalidating; backoff between attempts |
    | Revalidating | new requests fail fast with EIO | every check passes → Connected; a check fails → Fenced |
    | Fenced | EIO for every operation | remount |

    Every transition is logged at INFO, or at ERROR for Fenced, and
    counted in metrics. New operations arriving while a connection is not
    Connected are never queued: they fail at once with EIO, so a stalled
    database cannot build a backlog that keeps the system overloaded
    after it recovers (the bounded worker hold during an owner-drained
    snapshot in `branch.DrainsOwnerBeforeSnapshot` is an explicit,
    intended pause on a Connected mount, not disconnect queueing). The
    single mutation that was in flight on a connection when it broke
    waits for reconnect up to its own timeout (`BoundsEveryWait`) or a
    FUSE INTERRUPT (`HonoursInterrupt`), so an ambiguous commit can be
    resolved through `op_log` (`IdempotentMutations`).

    Connections must be session-mode. Transaction-mode poolers can keep a
    client socket open across a backend restart or failover, hiding the
    disconnect and bypassing `RevalidatesAfterReconnect`. Each connection
    records `pg_backend_pid()` on open and refuses to mount or moves to
    Fenced if a pooler swaps backends mid-session.

    **Sources.** VeriBetrKV r70, FoundationDB r15, metastable failures
    r67 and r125.

??? abstract "ReconnectsWithBackoff"
    `spec.failure.ReconnectsWithBackoff`

    In Reconnecting, attempts are spaced by exponential backoff with
    full jitter, starting at 100 ms, doubling, and capped at 10 s. libpq
    multi-host connection strings are honoured with
    `target_session_attrs=read-write`, so after a failover the daemon
    finds the new primary (`replica.FollowsFailover`). There is no limit
    on the number of attempts. Each failure is logged at WARN, at most
    once per 10 s.

    **Sources.** r67, r125, Fail-Slow r122.

??? abstract "RevalidatesAfterReconnect"
    `spec.failure.RevalidatesAfterReconnect`

    Before any reconnected worker serves an operation, the daemon
    runs a mount-wide revalidation in one round trip:

    - `meta.schema_version` and `meta.fs_uuid` are unchanged (a running
      mount also re-checks `meta.schema_version` at each lease renewal
      interval, `ops.MigratesSchema`), and on a scratch filesystem the
      tree's root inode row still exists (`durability.ScratchMode`);
    - the server's system identifier is unchanged, and its timeline is
      the same or a successor reached by failover;
    - the server settings still pass `durability.ChecksServerDurability`;
    - for a read-write mount: its row in `mounts` with the exact
      `(mount_id, fence)` still exists (`consistency.FencesStaleWriters`;
      a missing lease row is never re-acquired on reconnect, because
      after a rollback a missing row is the signature of lost history),
      and one mount-wide query
      (`SELECT worker_id, op_seq, result FROM pgfs.op_log WHERE
      mount_id = $1`) verifies that every worker's slot exists and has
      `slot.op_seq ≥ worker.last_replied_op_seq`;
    - for a snapshot mount (`branch.MountsSnapshots`): its row in
      `snapshots(tree, name)` still exists (an asynchronous failover can
      lose a recent snapshot), and its session in `mounts` is renewed;
    - for a read-only live mount (`consistency.ReadOnlyLiveMounts`): its
      session in `mounts` is renewed, every cached inode `version` and
      daemon metadata cache entry is dropped, and `notify_inval_inode` is
      sent for every nodeid the kernel holds so rewound asynchronous
      commits cannot leave stale kernel page caches.

    Why the mount-wide `op_log` check is necessary and sufficient:
    recovery preserves a prefix of WAL
    (`durability.PrefixCrashConsistency`). If any acknowledged
    asynchronous commit of this mount was lost (by a crash within
    `durability.AsyncCommitWindow`, a PITR restore, or an asynchronous
    failover), the mount's latest acknowledged commit was lost, which is
    `last_replied_op_seq` of some worker. Checking all workers' slots in
    one mount-wide query before any worker resumes prevents an idle
    worker that reconnects early from passing its own check and
    allocating a rolled-back `ino` from the sequence. Housekeeping and
    lease renewals need no slot because their work is idempotent and
    redone on reconnect. The check fences only when an acknowledged
    commit was actually lost: a mount that was idle for longer than
    `durability.AsyncCommitWindow` before a Postgres restart passes it
    without remounting.

    Any failed check moves the mount to Fenced and logs at ERROR which
    check failed. Moving to Fenced is mandatory when history regressed,
    because kernel cache invalidation cannot close open file descriptors.

    **Sources.** Ceph r05 and Raft r25 (verify identity after a change
    of membership).

??? abstract "IdempotentMutations"
    `spec.failure.IdempotentMutations`

    Each mutation carries `(mount_id, worker_id, op_seq)`, where
    `op_seq` is monotonic per worker within the mount. Lease acquisition
    (`consistency.ExclusiveTreeLease`) pre-inserts one `op_log` row per
    worker, `(mount_id, worker_id, 0, NULL)`. Because each worker runs at
    most one mutation at a time on its connection
    (`concurrency.WorkerOwnsConnection`), `op_log` is keyed by
    `(mount_id, worker_id)` and stores only that worker's latest
    `(op_seq, result)`, overwritten by an in-place (HOT) `UPDATE` in the
    same transaction (Viewstamped Replication r28). Each worker tracks
    `last_replied_op_seq` (the last `op_seq` whose reply was sent to the
    kernel) separately from `in_flight_op_seq` (the mutation running when
    a connection broke).

    When a connection breaks during COMMIT, the outcome is unknown:

    - If revalidation (`RevalidatesAfterReconnect`) passes before the
      in-flight request times out or is interrupted, the worker resolves
      `in_flight_op_seq` through its `op_log` slot: if `slot.op_seq =
      in_flight_op_seq`, the mutation already committed (for a
      synchronous operation such as `fsync`, `O_SYNC` write, `snapshot`,
      `branch` or `promote`, the worker first runs `pgfs.sync_barrier` to
      confirm standby replication, `durability.FsyncForcesEarlierCommits`),
      advances `last_replied_op_seq`, and returns the stored `result`
      without re-executing; otherwise the mutation re-runs with the same
      `op_seq`.
    - If the in-flight request timed out or was interrupted while
      disconnected, it replies EIO (or EINTR). When reconnect later
      completes, revalidation still inspects `slot.op_seq`: if it equals
      `in_flight_op_seq`, the daemon invalidates the affected kernel and
      daemon cache entries, advances `last_replied_op_seq` to
      `in_flight_op_seq`, and logs at WARN that a committed mutation had
      returned EIO after disconnect timeout.

    A read-write mount's `op_log` rows are deleted only when its lease
    row in `mounts` is released on clean unmount, deleted by `tree
    delete`, or replaced on `--takeover`, so idempotency never expires
    during a long network outage.

    **Postcondition.** No mutation is applied twice, and none is
    reported as failed when it committed while revalidation completes
    within the request timeout.

    **Bounds.** Exactly `workers` rows in `op_log` per active read-write
    mount.

    **Sources.** Viewstamped Replication r28 (client table), Raft r08,
    λFS r20.

??? abstract "PreservesBuffersAcrossReconnect"
    `spec.failure.PreservesBuffersAcrossReconnect`

    Writes buffered in the daemon survive a database outage. They
    stay in memory, still bounded by the buffer limits
    (`concurrency.BoundedWriteBuffers`), and are committed once the
    connection is back. While disconnected, a write that would exceed
    the limits replies EIO rather than block.

    **Durability.** Unchanged: buffered data is not durable until
    committed (`durability.DurabilityPoints`).

??? abstract "RetriesSerializationFailures"
    `spec.failure.RetriesSerializationFailures`

    A transaction that fails with SQLSTATE `40001`
    (serialization_failure) or `40P01` (deadlock_detected) is retried
    with the same `op_seq`, up to 5 attempts in total, with jittered
    backoff from 1 ms to 100 ms. After the last attempt, the operation
    replies EIO and the failure is logged at WARN.

    **Sources.** DaisyNFS r68, Chardonnay r66.

??? abstract "HonoursInterrupt"
    `spec.failure.HonoursInterrupt`

    When the kernel interrupts a request, for example because the
    calling process got a signal, the daemon cancels the backend query
    with `PQcancel` (or `pg_cancel_backend` from another connection).
    It replies EINTR if the cancel won, or the real result if the
    operation had already finished. A cancelled mutation is resolved
    through `op_log` like any other ambiguous commit.

    If a cancel arrives during a synchronous commit's standby wait
    (`SyncRepWaitForLSN`), Postgres commits locally and returns success
    with a warning; because the daemon recorded that it sent a cancel on
    that connection, it treats a synchronous commit that returns success
    after a cancel (or emits a `SyncRepWaitForLSN` warning) as not yet
    replicated (`durability.FsyncIsDurable`) and runs `pgfs.sync_barrier`
    before acknowledging durability (`durability.FsyncForcesEarlierCommits`).

    **Sources.** FiDe r78, DepFast r87.

??? abstract "InFlightAgeWatchdog"
    `spec.failure.InFlightAgeWatchdog`

    Each dispatch worker stores the start time and the operation of
    the request it is serving in an atomic slot. A watchdog thread reads
    the slots every second. When the oldest request is older than
    `--stall-warn` (default 5 s), it logs a WARN naming the worker,
    operation, inode, elapsed time and backend pid. When it is older
    than `--stall-error` (default 60 s), it logs ERROR, writes a state
    dump from its own thread (`observe.StateIntrospection`), and cancels
    the backend query. If the cancelled query was a synchronous commit
    waiting in `SyncRepWaitForLSN` that returns success with a warning,
    the worker treats it as not yet replicated and runs `pgfs.sync_barrier`
    (`durability.FsyncIsDurable`).

    The watchdog never unmounts. A stalled database is reported and
    contained, not turned into a lost mount.

    **Sources.** Gray Failure r124 (differential observability). The
    heartbeat check this replaces can only notice a stalled heartbeat
    thread, never a stalled dispatcher.

??? abstract "SlowDatabaseSignal"
    `spec.failure.SlowDatabaseSignal`

    The daemon keeps a baseline of its database latency: the p99 over
    the previous hour, per statement. When the p99 of the last minute
    stays above 4 times that baseline for 5 consecutive minutes, it logs
    a WARN with both figures and sets a `slow_db` gauge, which it clears
    when the latency recovers.

    **Sources.** NVMe failures in the field r88 (slow devices before
    failed ones), Fail-Slow at Scale r122, IASO r92.

??? abstract "MapsDatabaseErrors"
    `spec.failure.MapsDatabaseErrors`

    Database errors map to replies by SQLSTATE class, in one table in
    the code:

    | SQLSTATE | Meaning | Reply | Also |
    |---|---|---|---|
    | `23505` | unique violation | EEXIST | — |
    | `PF001` | fenced | EIO | state → Fenced |
    | `PF002`–`PF0FF` | pgfs function errors | the errno the function names | — |
    | `40001`, `40P01` | serialization, deadlock | retried, then EIO | `RetriesSerializationFailures` |
    | `55P03` | lock not available | ETIMEDOUT | WARN |
    | `57014` | query cancelled | EINTR if interrupted, else ETIMEDOUT | — |
    | `53100` | disk full | ENOSPC | ERROR |
    | `53200`, `53300` | out of memory, too many connections | EIO | WARN; backoff |
    | `25006` | read-only transaction (server is a standby) | EIO | state → Broken |
    | `08*`, `57P01`–`57P03` | connection lost, server shutdown | EIO (new requests); in-flight mutation awaits reconnect (`IdempotentMutations`) | state → Broken |
    | anything else | — | EIO | ERROR with the full chain |

    On a synchronous operation (`fsync`, `O_SYNC`/`O_DSYNC`, synchronous
    mount, or synchronous CLI command), a `WARNING` `NoticeResponse` with
    `routine = "SyncRepWaitForLSN"` or a commit returning success after a
    daemon-issued cancel means the transaction committed locally without
    standby confirmation; the caller runs `pgfs.sync_barrier` and replies
    EIO (or EINTR if interrupted) if the barrier fails
    (`durability.FsyncIsDurable`).

    **Sources.** Aspirator r119. Every row is driven by a test in
    `verify.DbFaultInjection`.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

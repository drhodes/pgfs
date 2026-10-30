"""
Verification: how pgfs proves it meets this specification.

Decided in design/design-decisions.md, topic 14. The closest template is
ShardStore (r128): Rust, an executable reference model, property-based
conformance checking, crash conformance and model-checked concurrency.
Around it: differential POSIX testing (Metis r48, Monarch r81), crash
testing (CrashMonkey r123, ALICE r118, LazyFS r135), fault injection at
every boundary (CORDS r121, Aspirator r119), configuration coverage
(CAFault r80), and an invariant checker run after every test (pFSCK
r54, T2C r58).

Every known pgfs bug was a missing contract clause, and each of these
checks would have caught one (spec-literature-review.md, Part 1).
"""

from .err import PARTIAL, Feat, Req, md
from .concurrency import (
    InodeShardedWorkers,
)
from .consistency import (
    FencesStaleWriters,
)
from .durability import (
    PrefixCrashConsistency,
)
from .failure import (
    MapsDatabaseErrors,
)
from .integrity import (
    PgfsCheck,
)
from .optimize import (
    PgfsBackendTraitReq,
)
from .posix import (
    DeclaresUnsupportedOps,
)


class ReferenceModel(Req):
    """`tests/model/` contains an in-memory filesystem that implements
    `PgfsBackend` (`PgfsBackendTraitReq`) by the simplest possible
    means: maps of inodes, dentries, bytes and xattrs, plus layers,
    epochs, orphans, leases and the change feed. It is the executable
    form of this specification, written for clarity, never for speed.
    It also serves as the backend for daemon unit tests that need no
    database.

    For every operation the model also gives the set of states a crash
    during it may leave. `CrashConformance` checks against that set.

    **Sources.** ShardStore r128.
    """

    deps = [PgfsBackendTraitReq]


class ModelConformance(Req):
    """A proptest suite generates sequences of operations. They cover
    every callback in `posix.PosixCompatibility` plus snapshot, branch,
    promote, flatten and copy_file_range, with names that include `%`,
    `_`, `\\`, non-UTF-8 bytes and 255-byte names. Generated and targeted
    scenarios include:

    - on a branch: rename-back then unlink, unlink-create-unlink, chmod
      then unlink, set then remove an inherited xattr,
      unlink-while-open then close, and rmdir of an inherited directory,
      each exercised during `branch.FlattensBranch` and across a
      fold-back `branch.PromotesBranch`;
    - 20 cycles of snapshot, branch, edit and normal promote, verifying
      that depth stays 0, `pgfs diff` works across promotes, and
      `space.ReclaimsRetiredRows` reclaims rows overwritten before the
      oldest remaining snapshot;
    - interleaved partial-block buffered writes, whole-block writes, and
      `O_DSYNC` writes across multiple handles on the same inode;
    - out-of-order `xid`/`seq` commit interleavings, windows larger than
      `limit` in `pgfs.changes_next`, `fallocate` `modify` events, and
      registered consumers across fold-back and `--force` promotes.

    Each sequence runs against the model and against a real mount. After
    every step the suite compares replies, errnos, file contents,
    attributes, directory listings and change-feed output. Failures
    shrink to a minimal sequence, which is kept as a regression case.

    **Sources.** ShardStore r128, Metis r48, Monarch r81.
    """

    deps = [ReferenceModel]


class CrashConformance(Req):
    """Postgres runs on LazyFS, which can lose writes that were not
    synced and tear writes that were. A test runs an operation sequence,
    then injects one fault:

    - loses unsynced pages;
    - tears a write;
    - kills Postgres;
    - kills the daemon;
    - kills both.

    It then restarts and checks three things:

    - the recovered state is in the model's crash set
      (`durability.PrefixCrashConsistency`);
    - every fsync'd write survived;
    - `pgfs check` and `amcheck` pass.

    Sequences come from `ModelConformance` and from the patterns in
    ALICE and CrashMonkey: write-then-rename, fsync of a parent
    directory, append then fsync, write without fsync followed by
    `fsync` on a second handle (with nothing pending in the daemon
    buffer) or a clean unmount and then a power-loss crash within the
    async commit window (`durability.FsyncForcesEarlierCommits`,
    `durability.FlushesOnShutdown`), and `kill -9` of Postgres on a
    `--scratch` mount followed by recovery and remount
    (`durability.ScratchMode`). The crash points cover snapshot, branch,
    promote, flatten, reclaim, garbage collection and orphan reclaim.

    **Sources.** LazyFS r135, CrashMonkey r123, ALICE r118, PoWER r60.
    """

    deps = [ModelConformance, PrefixCrashConsistency]


class DbFaultInjection(Req):
    """A Cargo feature, `fault-injection`, wraps the database layer so
    that a test can fail the Nth call with any error class in
    `failure.MapsDatabaseErrors`, delay it, or drop the connection
    during COMMIT. CI drives every row of that table and every
    intentional `.ok()` site, and checks the reply, the state machine
    transition, the log line, and that `pgfs check` passes afterwards.
    Targeted fault tests also cover:

    - crashing Postgres within the async commit window after a create and
      reconnecting (expecting `Fenced` via the `op_log` continuity check,
      and expecting no fence when the mount was idle past the window),
      plus the same on a read-only mount with an open file (expecting
      ESTALE or fresh data, never stale pages);
    - injecting a one-shot commit failure while two handles write to one
      inode, verifying that both handles receive EIO via `errseq_t` on
      `fsync`/`close` and that retry preserves write arrival order;
    - pausing a synchronous standby during `fsync`, cancelling via the
      watchdog, `pg_cancel_backend`, or `pg_terminate_backend` (expecting
      EIO), then resuming the standby and verifying the next `fsync`
      succeeds via `pgfs.sync_barrier`;
    - pausing the daemon at each step of `branch.DrainsOwnerBeforeSnapshot`
      past `--timeout`, verifying that either the CLI succeeded and the
      snapshot exists or the CLI returned ETIMEDOUT and no snapshot was
      created;
    - dropping the connection before and after the backend commit point
      for every mutating callback and verifying `failure.IdempotentMutations`
      resolves via `op_log` without returning a false EIO.

    **Sources.** Aspirator r119 (error handlers that never ran), CORDS
    r121, FiDe r78.
    """

    deps = [MapsDatabaseErrors]


class ConfigurationMatrix(Req):
    """The integration suite runs under each combination of the settings
    that change behaviour:

    - read-write, read-only live, and snapshot mounts;
    - a tree and a branch of depth 3;
    - scratch and logged storage;
    - with and without a standby;
    - 1 worker and 8 workers;
    - block sizes of 4 KiB and 64 KiB;
    - every supported Postgres major version.

    Combinations that are invalid by specification, such as scratch
    with a standby, are tested for their refusal.

    **Sources.** CAFault r80.
    """


class CheckAfterEveryTest(Req):
    """Every integration test, model-conformance run and crash test ends
    with `pgfs check`, and a violation fails the test even if the test's
    own assertions passed.

    **Sources.** T2C r58, pFSCK r54.
    """

    deps = [PgfsCheck]


class PosixSuites(Req):
    """CI runs pjdfstest in full, and the xfstests `generic` group with
    a maintained exclusion list. Each exclusion names the requirement
    in `posix.DeclaresUnsupportedOps` that explains it. A test that
    starts passing while excluded fails the run, so the list cannot go
    stale.
    """

    deps = [DeclaresUnsupportedOps]


class DivergenceTests(Req):
    """Tests force each kind of divergence the design allows, and check
    that the outcome is the specified one:

    - a write through SQL as the owner role, behind a mount, is caught by
      `pgfs check`;
    - a standby lagging behind is not read without a ticket (and when
      `primary_flush_lsn < ticket`, reads go directly to the primary
      without waiting `--replica-wait`, `replica.LsnTicketReads`);
    - a read-only live mount past its TTL serves the new state;
    - a daemon paused past its lease is fenced (`consistency.FencesStaleWriters`),
      and deleting a snapshot or running `pgfs migrate` while a snapshot
      mount is active is refused with EBUSY until its session expires and
      the mount has already failed closed with EIO;
    - a restored database copy (or PITR / asynchronous failover rollback)
      with the same `fs_uuid` is detected by the `op_log` continuity
      check and `(mount_id, fence)` check in
      `failure.RevalidatesAfterReconnect` (moving a read-write mount to
      `Fenced` and invalidating cached versions and nodeids on a
      read-only live mount) and by `inodes.gen` on open file handles
      (`storage.PersistentInodes`).

    **Sources.** Modulo r86.
    """

    deps = [FencesStaleWriters]


class LinearizabilityCheck(Req):
    """Concurrent dispatch is model-checked with `shuttle`. Workers,
    buffers, the lease renewer and housekeeping run under a randomised
    scheduler against the model, including cross-worker renames in
    opposite directions, cross-worker buffer flushes under
    `--buffer-total` pressure, and concurrent rename-over of a file with
    dirty buffers (`concurrency.InodeShardedWorkers`), and every history
    must be linearizable (`consistency.ConsistencyModel`).

    **Sources.** ShardStore r128.
    """

    deps = [ReferenceModel, InodeShardedWorkers]


class BugRegressions(Req):
    """Each known bug has a test that failed before its fix:

    1. a partial-block write that zeroes untouched bytes;
    2. a rename that treats `%` and `_` as wildcards;
    3. a rename that loses buffered writes;
    4. the root identity collision;
    5. no reconnect after a Postgres restart.

    New bugs get a test before their fix.
    """


# ── Feature ───────────────────────────────────────────────────────────

class ConformanceTesting(Feat):
    """pgfs is tested against an executable model of its specification:
    generated operation sequences, crashes at every point, injected
    database faults, every supported configuration, and the standard
    POSIX test suites, with the invariant checker run after every
    test."""

    deps = [
        ReferenceModel,
        ModelConformance,
        CrashConformance,
        DbFaultInjection,
        ConfigurationMatrix,
        CheckAfterEveryTest,
        PosixSuites,
        DivergenceTests,
        LinearizabilityCheck,
        BugRegressions,
    ]

    def title(self):
        return "Conformance Testing"

    def status(self):
        return PARTIAL

    def status_note(self):
        return (
            "The reference model, the proptest model-conformance suite "
            "(with branch scenarios), database fault injection "
            "(`--features fault-injection`), `pgfs check` after every "
            "test, divergence and bug-regression tests, and daemon and "
            "Postgres crash tests ship. Not yet: the LazyFS crash matrix, "
            "shuttle linearizability checks, pjdfstest and xfstests runs, "
            "and the configuration-matrix runner."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will run the model-conformance suite and
            watch it find and shrink a failure.

            ## Step 1: run it

            ```bash
            cargo test --test conformance -- --nocapture
            ```

            ## Step 2: read a failure

            When an operation sequence makes the mount disagree with
            the model, proptest shrinks it to the shortest sequence that
            still fails and prints it, together with the step that
            diverged and both results.

            ## Step 3: keep it

            Shrunk failures are saved under
            `tests/model/regressions/` and run on every build.
        """)

    def how_to(self):
        return md(r"""
            ## Run crash conformance locally

            ```bash
            ./scripts/init_db.sh --lazyfs
            cargo test --test crash -- --nocapture
            ```

            ## Inject a database fault in a test

            ```bash
            cargo test --features fault-injection --test faults
            ```

            ## Run the POSIX suites

            ```bash
            make test-posix     # pjdfstest and xfstests generic
            ```
        """)

    def reference(self):
        return md(r"""
            ## Test layers

            | Layer | Finds | Tool |
            |---|---|---|
            | reference model | spec ambiguity | Rust, `tests/model/` |
            | model conformance | wrong replies and states | proptest |
            | crash conformance | crash-state violations | LazyFS |
            | fault injection | untested error paths | `fault-injection` feature |
            | configuration matrix | flag interactions | CI |
            | invariant check | corrupted state | `pgfs check`, amcheck |
            | POSIX suites | conformance gaps | pjdfstest, xfstests |
            | linearizability | concurrency bugs | shuttle |
        """)

    def explanation(self):
        return md(r"""
            ## Why a model

            A specification in prose can be read two ways. A small
            in-memory filesystem that does exactly what the
            specification says cannot. Running the same random
            operations against the model and the real thing, and
            comparing after every step, turns every disagreement into a
            minimal failing case. Amazon's S3 team built ShardStore
            this way.

            ## Why crash tests need a fake disk

            The interesting crash states are the ones where some writes
            reached the disk and others did not. A process kill cannot
            produce them. LazyFS sits under Postgres and decides which
            unsynced writes survive, so the tests can produce those
            states on demand.
        """)

# Reference: Project-Local Development Environment

!!! warning "Status: Partially implemented"
    `scripts/init_db.sh` creates the cluster with `--data-checksums`, checks `fsync` and `full_page_writes`, and formats the `pgfs` database with `pgfs mkfs`; `scripts/pgfs.sh`, the Makefile targets and the integration suite drive the mount, and `run_tests.sh` runs `pgfs check` after every test. The `--lazyfs` mode needs LazyFS installed and is not yet exercised by the suite.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

[Tutorial](../tutorials/projectlocalenvironment.md) · [How-to](../how-to/projectlocalenvironment.md) · **Reference** · [Explanation](../explanation/projectlocalenvironment.md)

---

## Scripts

| Script | Does |
|---|---|
| `scripts/init_db.sh [--lazyfs]` | create and start the cluster in `testdata/pgdata` (`--data-checksums`, socket in `testdata/`, TCP on 127.0.0.1), check `fsync` and `full_page_writes`, create and `pgfs mkfs` the `pgfs` database, print the connection string |
| `scripts/ensure_db.sh` | `init_db.sh` unless the cluster already answers |
| `scripts/pgfs.sh up\|down\|status\|run` | mount lifecycle, with stale-mount sweeping and escalating unmount |
| `scripts/run_tests.sh` | every `scripts/test_*.sh` under a 300 s timeout, then `pgfs check` |

## Files

| Path | Holds |
|---|---|
| `testdata/pgdata` | the cluster |
| `testdata/postgres.log` | the server log |
| `testdata/mnt` | the default mount point |
| `testdata/pgfs.log` | the daemon log from `pgfs.sh up` |
| `testdata/<test>.log` | each integration test's output |

## Environment

| Variable | Default |
|---|---|
| `PGFS_CONN` | `host=<repo>/testdata dbname=pgfs` |
| `PGFS_MOUNT` | `testdata/mnt` |


## Specification requirements

`ProjectLocalEnvironment` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ProvidesProjectLocalCluster"
    `spec.app.ProvidesProjectLocalCluster`

    `scripts/init_db.sh` creates a PostgreSQL cluster at
    `testdata/pgdata`, with
    `initdb --auth=trust --no-locale --encoding=UTF8 --data-checksums`.
    It starts the cluster with its Unix socket in `testdata/` and a TCP
    listener on 127.0.0.1 only, and checks that `fsync` and
    `full_page_writes` are on. It ensures that a superuser role named
    after the invoking OS user and a database named `pgfs` exist, runs
    `pgfs mkfs` if the database is unformatted, and prints the
    connection string.

    `scripts/ensure_db.sh` is the idempotent wrapper used by
    `make test`: it exits at once when the cluster already answers.
    `init_db.sh --lazyfs` puts the data directory on LazyFS, so that
    crash conformance can run locally (`verify.CrashConformance`).
    Nothing is written outside the project's `testdata/` directory.

    **Sources.** LazyFS r135, protocol-aware recovery r94.

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

??? abstract "RunsIntegrationSuite"
    `spec.app.RunsIntegrationSuite`

    `scripts/run_tests.sh` (`make test-integration`) runs every
    `scripts/test_*.sh` except the shared harness `test_lib.sh`, each in
    isolation under a 300 s timeout, writing each test's output to
    `testdata/<test>.log`. After every test it kills any leftover daemon
    and lazily unmounts `testdata/mnt` so the next test starts clean. It
    then runs `pgfs check`, and a violation fails the test
    (`verify.CheckAfterEveryTest`). It prints PASS or FAIL per test and
    exits non-zero if any test failed. This shell layer is the base. The
    model, crash and fault suites in `verify.py` find the bug classes it
    cannot.

??? abstract "ConnectsToPostgres"
    `spec.app.ConnectsToPostgres`

    The daemon connects to Postgres with a libpq connection string
    from `--conn` or `PGFS_CONN`, with the timeouts and keepalives of
    `failure.BoundsEveryWait`, and requires session-mode connections
    (`failure.RevalidatesAfterReconnect`). Before mounting, in one round
    trip, it checks:

    - the server version (`ops.SupportedPostgresVersions`);
    - the server's durability settings
      (`durability.ChecksServerDurability`);
    - the schema version (`storage.SchemaVersion`).

    It never creates or alters tables (`ops.RefusesUnformattedDatabase`;
    on a scratch filesystem whose unlogged tables were wiped by a crash,
    it calls `pgfs.scratch_reset()`, `durability.ScratchMode`). Then it
    registers its mount session or acquires the tree's lease
    (`consistency.ExclusiveTreeLease`).

    **Errors.** A connection failure exits with status 2 and a full
    error chain: the operation, the host and database, and the root
    cause. The connection string is redacted (`security.RedactsSecrets`).

    The schema is `storage.StorageLayout`. The first schema, the
    path-keyed `entries` and `blocks` tables created implicitly at
    mount, is retired.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

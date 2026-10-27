# Reference: Project-Local Development Environment

!!! success "Status: Stable"
    Everything on this page ships today: `scripts/init_db.sh`, `scripts/pgfs.sh`, the Makefile targets and the integration suite in `scripts/test_*.sh`.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

[Tutorial](../tutorials/projectlocalenvironment.md) · [How-to](../how-to/projectlocalenvironment.md) · **Reference** · [Explanation](../explanation/projectlocalenvironment.md)

---

## Toolchain

| Tool | Provided by `nix develop` | Used for |
|---|---|---|
| `rustc`, `cargo`, `clippy`, `rustfmt` | yes | building and checking the daemon |
| PostgreSQL server binaries (`initdb`, `pg_ctl`, `psql`, `createdb`) | yes (PostgreSQL 17) | the project-local cluster |
| FUSE 3 (`fusermount3`, headers, `pkg-config`) | yes | mounting |
| Docker | no | replicated mode only (`scripts/replica_db.sh`) |
| `uv` | no | the libspec specification and this documentation |

## `testdata/` layout

| Path | Contents |
|---|---|
| `testdata/pgdata/` | PostgreSQL cluster created by `init_db.sh` |
| `testdata/.s.PGSQL.5432` | PostgreSQL Unix socket |
| `testdata/postgres.log` | PostgreSQL server log |
| `testdata/mnt/` | default mount point |
| `testdata/pgfs.pid` | PID of the daemon started by `pgfs.sh up` |
| `testdata/pgfs.log` | daemon stdout and stderr (appended) |
| `testdata/<test>.log` | output of each integration test |
| `testdata/bench-pgfs.log`, `testdata/bench.pid` | `bench_profile.sh` daemon log and PID |

`testdata/` is git-ignored.

## Project-local cluster

| Setting | Value |
|---|---|
| Data directory | `testdata/pgdata` |
| `initdb` flags | `--auth=trust --no-locale --encoding=UTF8` |
| Listen addresses | Unix socket in `testdata/`; TCP `127.0.0.1:5432` |
| Authentication | `trust` for local and loopback connections |
| Roles | bootstrap superuser (owner of `pgdata`); a `SUPERUSER` role named after the invoking OS user if different |
| Database | `pgfs` |
| Daemon default `--conn` | `host=<current working directory>/testdata dbname=pgfs` |

The default `--conn` is derived from the daemon's working
directory. Started from anywhere other than the repository
root, it needs an explicit `--conn`.

## Scripts

| Script | Purpose |
|---|---|
| `scripts/init_db.sh` | create (if absent) and start the cluster; create role and database |
| `scripts/ensure_db.sh` | exit 0 if the cluster answers, otherwise run `init_db.sh` |
| `scripts/pgfs.sh up` | start Postgres if needed, `cargo build`, sweep a stale mount, start the daemon, wait up to 10 s for the mount |
| `scripts/pgfs.sh down` | SIGTERM the daemon, wait up to 10 s, then `fusermount3 -u`, then report holders and lazy-unmount |
| `scripts/pgfs.sh status` | report mount, daemon and Postgres state; warn about ghost mounts |
| `scripts/pgfs.sh run` | build and exec the daemon in the foreground |
| `scripts/run_tests.sh` | run every integration test in isolation (300 s timeout each) |
| `scripts/test_*.sh` | individual integration tests; `test_lib.sh` is the shared harness |
| `scripts/replica_db.sh` | manage the Docker streaming standby (see [Replicated Storage](replicated_filesystem.md)) |
| `scripts/bench_profile.sh` | profiled benchmark (see [On-Demand Profiling](ondemandprofiling.md)) |
| `scripts/build_docs.py` | regenerate `docs/` and the `mkdocs.yml` navigation from `spec/` |

## Integration tests

| Test | Covers |
|---|---|
| `test_append.sh` | `O_APPEND` offset and size correctness |
| `test_copy_performance.sh` | end-to-end copy throughput |
| `test_dirs.sh` | deep nesting, listing, `rmdir` semantics |
| `test_git_init.sh` | `git init` inside the mount |
| `test_integrity.sh` | create/write/unlink chains |
| `test_rename.sh` | rename-overwrite with POSIX semantics |
| `test_replica.sh` | replicated mode end to end |
| `test_truncate.sh` | truncate and extend semantics |

## Make targets

| Target | Action |
|---|---|
| `make up` / `down` / `status` / `run` | `scripts/pgfs.sh` equivalents |
| `make build` / `check` / `clippy` / `fmt` | `cargo build` / `check` / `clippy` / `fmt` |
| `make test` | `ensure_db.sh`, then `cargo test` |
| `make test-integration` | `scripts/run_tests.sh` |
| `make init-db` | `scripts/init_db.sh` |
| `make db-stop` | `pg_ctl -D testdata/pgdata stop` |
| `make shell` | `nix develop` |
| `make clean` | `cargo clean` |

Every target except `down`, `status`, `db-stop`, `shell` and
`clean` requires the Nix shell (`PGFS_ROOT` set).

## Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `PGFS_MOUNT` | `pgfs.sh` | mount point (default `testdata/mnt`) |
| `PGFS_ROOT` | `Makefile` | set by the Nix shell; `make` refuses to run without it |
| `RUST_LOG` | daemon | log filter, e.g. `pgfs=debug` (default `pgfs=info`) |
| `RUST_LOG_FORMAT` | daemon | `json` for newline-delimited JSON logs |
| `RUST_BACKTRACE` | daemon | backtrace detail in crash reports |


## Specification requirements

`ProjectLocalEnvironment` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "ProvidesProjectLocalCluster"
    `spec.app.ProvidesProjectLocalCluster`

    `scripts/init_db.sh` creates a PostgreSQL cluster at
    `testdata/pgdata` (`initdb --auth=trust --no-locale --encoding=UTF8`)
    and starts it with its Unix socket in `testdata/` and a TCP listener on
    127.0.0.1 only. It ensures a superuser role named after the invoking OS
    user and a database named `pgfs` exist, and prints the connection
    string. `scripts/ensure_db.sh` is the idempotent wrapper used by
    `make test`: it exits immediately when the cluster already answers.
    Nothing is written outside the project's `testdata/` directory.

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
    prints PASS/FAIL per test and exits non-zero if any test failed.

??? abstract "ConnectsToPostgres"
    `spec.app.ConnectsToPostgres`

    The daemon connects to a Postgres database via a libpq connection
    string. On first connect it creates the `entries` table (ino, parent_ino,
    parent, name, kind, size, mode, uid, gid, atime, mtime, ctime) and
    `blocks` table (ino, parent, name, block_no, data) if they do not
    already exist. Connection failure produces a full error story
    including the connection string and the root cause.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

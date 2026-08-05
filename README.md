# PGFS(1) | General Commands Manual

## NAME
pgfs - FUSE filesystem backed by PostgreSQL

## SYNOPSIS
`pgfs` [*OPTIONS*] *MOUNTPOINT*

## DESCRIPTION
**pgfs** mounts a POSIX-like directory hierarchy backed by a PostgreSQL database. File and directory metadata, directory relationships, and file contents are stored as rows in an `entries` table (`parent`, `name`, `kind`, `size`, `data`).

File IO operations interact directly with PostgreSQL. FUSE callbacks map kernel inodes to database entries via an in-memory translation table.

## OPTIONS
`-h`, `--host` *HOST*
: PostgreSQL host or Unix socket directory. Default: `$(pwd)/testdata`.

`-d`, `--dbname` *NAME*
: Database name. Default: `pgfs`.

`-U`, `--user` *USER*
: Database user. Default: current OS user.

`--replica` *CONNSTRING*
: Connection string for secondary read replica. Read queries attempt replica execution first, falling back to primary on error or lag.

`--help`
: Display usage summary.

`--version`
: Display version information.

## CONTROL SCRIPTS
`scripts/pgfs.sh up`
: Provision local database cluster, build binary, and mount to `testdata/mnt`.

`scripts/pgfs.sh down`
: Perform clean unmount and stop local database cluster.

`scripts/pgfs.sh status`
: Display mount, daemon PID, and PostgreSQL status.

`scripts/replica_db.sh` {`up`|`down`|`status`|`conn`}
: Manage physical streaming read-replica Docker container.

## DIAGNOSTICS & OBSERVABILITY
**pgfs** outputs structured log messages using `tracing`.

### Environment Variables
`RUST_LOG`
: Log filter directive (e.g., `RUST_LOG=pgfs=info`, `RUST_LOG=pgfs::db=debug`).

`RUST_LOG_FORMAT`
: Log output format (`full`, `compact`, `pretty`, or `json`). Default: `full`.

### Metrics
Daemon logs aggregate statistics at `INFO` level every 60 seconds:
- **Operation Counters**: Counts for `lookup`, `getattr`, `setattr`, `read`, `write`, `create`, `mkdir`, `unlink`, `rmdir`, `rename`, `readdir`, `open`, `fsync`.
- **Error Counters**: Expected errnos (`ENOENT`, `EEXIST`, etc.) and unexpected failures (`EIO`).
- **Replica Counters**: Replica read count and fallback count.
- **Latency Histograms**: Microsecond-bucketed wall-clock distributions for FUSE callbacks and database queries.

### Health Verification
- `/tmp/pgfs-{hash}.pid`: Contains daemon process identifier.
- `/tmp/pgfs-{hash}.ready`: Created when FUSE session begins serving requests.
- **Liveness Watchdog**: Background thread increments heartbeat counter every 10 seconds. Callbacks verify counter progress; if stalled >30 seconds, watchdog unmounts filesystem cleanly.

## SIGNALS
`SIGINT`, `SIGTERM`, `SIGHUP`
: Initiate clean FUSE unmount and process exit.

`SIGUSR1`
: Request metric snapshot and internal state dump on next FUSE callback.

`SIGUSR2`
: Toggle CPU profiling (enabled when compiled with `--features profiling`). Outputs flamegraph SVG and stack dumps to `/tmp/pgfs-profile-*.svg`.

## TESTING
`make test`
: Run Rust unit test suite (`cargo test`).

`make test-integration`
: Execute integration test suite (`scripts/run_tests.sh` covering POSIX rename, append, truncate, integrity, directory operations, and `git init`).

## FILES
`testdata/pgdata`
: Local PostgreSQL database storage directory.

`testdata/mnt`
: Default mountpoint directory.

`/etc/fuse.conf`
: Must contain `user_allow_other` for `auto_unmount` execution.

## SEE ALSO
`MANUAL.md`, `mount(8)`, `fuse(8)`, `psql(1)`

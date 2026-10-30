# PGFS(1) | General Commands Manual

## NAME
pgfs - a POSIX filesystem stored in PostgreSQL

## SYNOPSIS
`pgfs` [`--conn` *CONNSTRING*] *COMMAND* [*ARGS*]

## DESCRIPTION
**pgfs** stores a filesystem in a PostgreSQL database and mounts it with FUSE. Every inode, name, block and extended attribute is a row in the `pgfs` schema; every operation is one call to a `pgfs.*` SQL function, in one transaction. Inode numbers are permanent, names are exact bytes, small files live inside their inode, and file data is immutable, checksummed blocks that files, snapshots and branches share.

A tree can be snapshotted and branched in constant time, and a branch can be published back atomically. Each tree has at most one read-write mount, which holds a lease; any number of read-only and snapshot mounts may run alongside it, on any machine.

## COMMANDS
`mkfs` [`--block-size` *BYTES*] [`--inline-max` *BYTES*] [`--capacity` *BYTES*] [`--scratch`] [`--runtime-role` *ROLE*] [`--force`]
: Format a database. Nothing else ever runs DDL.

`mount` *TREE*[`@`*SNAPSHOT*][`:`*/PATH*] *MOUNTPOINT* [`--read-only`] [`--takeover`] [`--workers` *N*] [`--replica` *CONNSTRING*] ...
: Mount a tree, a snapshot (read-only), or a directory of either.

`snapshot` *TREE*`@`*NAME* | `snapshot list` [*TREE*] | `snapshot delete` *TREE*`@`*NAME*
: Freeze a tree. While the tree is mounted, its daemon drains its buffers first (`--crash-consistent` skips that).

`branch` *SOURCE* *NEW*
: Create a writable tree from a tree or snapshot, optionally rooted at a directory.

`promote` [`--force`] *BRANCH* *TREE*
: Make a branch the new contents of a tree, atomically.

`tree list` | `tree delete` *TREE* | `tree flatten` *TREE*
: Manage trees.

`changes` *TREE* [`--since` *TOKEN*] [`--follow`] [`--consumer` *NAME*] | `changes register` *NAME* *TREE* | `changes unregister` *NAME*
: Print the change feed as JSON lines.

`diff` *TREE*`@`*A* *TREE*[`@`*B*]
: List what changed between two views of one tree.

`check` [*TREE*] [`--repair`], `scrub` [`--rate` *MIB*] [`--continuous`]
: Verify invariants and checksums.

`export` *TREE*`@`*SNAPSHOT* > *FILE.tar*, `import` *TREE*[`:`*/DIR*] < *FILE.tar*
: Portable pax archives.

`migrate` [`--dry-run`], `tune` [`--capacity` *BYTES*|`none`] [`--new-uuid`] [`--runtime-role` *ROLE*], `stats` [*TREE*], `options`
: Administration. `pgfs options` prints every option with its default; the same table is in `docs/cli-options.md`.

## CONNECTION
`--conn` or `PGFS_CONN` gives a libpq connection string (default: the project-local cluster, `host=$(pwd)/testdata dbname=pgfs`). Passwords come from `PGPASSWORD`, a pgpass file or `--password-file`, never from the command line. A remote host needs `sslmode=verify-full` unless `--insecure-transport` is given. Connection strings are redacted in every log line.

## EXIT STATUS
0 on success; 1 when the command ran and found a problem (for example `check`), or a mount ended with buffered data it could not commit; 2 when it could not run.

## SIGNALS (mount)
`SIGINT`, `SIGTERM`, `SIGHUP`
: Commit every buffer, run the synchronous barrier, unmount, release the lease.

`SIGUSR1`
: Write a state dump to the log from the signal thread.

`SIGUSR2`
: Toggle CPU profiling (with `--features profiling`).

## DIAGNOSTICS
Logs go to stderr through `tracing`: `RUST_LOG` filters (`RUST_LOG=pgfs=debug`), `RUST_LOG_FORMAT=json` switches to JSON. Every 60 seconds the daemon logs nine `metrics|…` lines (operations, errnos, replica, latency, connection state, in-flight age, space, integrity).

The daemon writes `/tmp/pgfs-{hash}.pid` and, once serving, `/tmp/pgfs-{hash}.ready`, and talks to systemd when `NOTIFY_SOCKET` is set (`contrib/pgfs@.service`).

## DEVELOPMENT
`scripts/pgfs.sh up|down|status|run`
: Project-local cluster (formatted with `pgfs mkfs`), build and mount at `testdata/mnt`.

`make test`
: Unit tests, storage tests against PostgreSQL, and the model-conformance suite (`cargo test`).

`make test-integration`
: `scripts/test_*.sh`, each followed by `pgfs check`.

`make bench`
: The benchmark workloads (`scripts/bench.sh`).

## FILES
`sql/v1/`
: The schema and its functions, embedded in the binary.

`spec/`
: The specification ([libspec](https://github.com/drhodes/libspec)); `docs/` is generated from it.

`testdata/`
: The project-local cluster, mount point and logs. `rm -rf testdata` resets everything.

## SEE ALSO
`MANUAL.md`, `docs/`, `mount(8)`, `fuse(8)`, `psql(1)`

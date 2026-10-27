# How-To: Project-Local Development Environment

!!! success "Status: Stable"
    Everything on this page ships today: `scripts/init_db.sh`, `scripts/pgfs.sh`, the Makefile targets and the integration suite in `scripts/test_*.sh`.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

[Tutorial](../tutorials/projectlocalenvironment.md) · **How-to** · [Reference](../reference/projectlocalenvironment.md) · [Explanation](../explanation/projectlocalenvironment.md)

---

## Run the Rust unit tests

Inside the Nix shell:

```bash
make test
```

This starts the project-local cluster if it is down
(`scripts/ensure_db.sh`) and then runs `cargo test`. Most
tests in `src/db.rs` talk to the live database. Outside the
Nix shell, `make` refuses to run (`PGFS_ROOT` is unset); run
`./scripts/ensure_db.sh && cargo test` instead.

## Run the integration suite

```bash
./scripts/pgfs.sh down        # the suite mounts testdata/mnt itself
make test-integration
```

Each `scripts/test_*.sh` prints `PASS` or `FAIL`. A failing
test prints the tail of its log; the full output is in
`testdata/<test>.log`. `test_replica.sh` needs Docker.

To run a single test:

```bash
bash scripts/test_rename.sh
```

## Run the daemon in the foreground

```bash
RUST_LOG=pgfs=debug ./scripts/pgfs.sh run
```

Logs go to the terminal. Ctrl+C unmounts cleanly.

## Mount somewhere other than `testdata/mnt`

```bash
PGFS_MOUNT=/tmp/pgfs-demo ./scripts/pgfs.sh up
PGFS_MOUNT=/tmp/pgfs-demo ./scripts/pgfs.sh down
```

Pass the same `PGFS_MOUNT` to every `pgfs.sh` command.

## Use a PostgreSQL server other than the project-local one

Run the binary directly and give it a libpq connection string:

```bash
cargo build
./target/debug/pgfs --conn "host=db.internal port=5432 dbname=pgfs user=alice" /mnt/pgfs
```

The role needs permission to create tables in that database
the first time; pgfs creates its schema on startup.

## Inspect the database directly

```bash
psql -h "$PWD/testdata" -d pgfs
```

## Stop PostgreSQL

```bash
./scripts/pgfs.sh down
make db-stop
```

Unmount first. A running daemon does not reconnect after
PostgreSQL comes back, so every operation fails with
`Input/output error` until the daemon is restarted.

## Reset everything

```bash
./scripts/pgfs.sh down
make db-stop
rm -rf testdata
```

The next `./scripts/pgfs.sh up` creates a fresh cluster.

## Rebuild this documentation

```bash
uv run python scripts/build_docs.py
uv run mkdocs serve
```

The pages are generated from the Diátaxis methods of the
Feature classes in `spec/`; edit those, not `docs/`.

## Check the specification for drift

```bash
uv run libspec diff
```

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

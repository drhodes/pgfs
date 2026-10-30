# How-To: Project-Local Development Environment

!!! warning "Status: Partially implemented"
    `scripts/init_db.sh` creates the cluster with `--data-checksums`, checks `fsync` and `full_page_writes`, and formats the `pgfs` database with `pgfs mkfs`; `scripts/pgfs.sh`, the Makefile targets and the integration suite drive the mount, and `run_tests.sh` runs `pgfs check` after every test. The `--lazyfs` mode needs LazyFS installed and is not yet exercised by the suite.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

[Tutorial](../tutorials/projectlocalenvironment.md) · **How-to** · [Reference](../reference/projectlocalenvironment.md) · [Explanation](../explanation/projectlocalenvironment.md)

---

## Start from scratch

```bash
pg_ctl -D testdata/pgdata stop; rm -rf testdata
./scripts/pgfs.sh up
```

## Run the tests

```bash
make test               # cargo test: unit, storage and model-conformance tests
make test-integration   # scripts/test_*.sh, each followed by pgfs check
```

## Run the daemon in the foreground

```bash
./scripts/pgfs.sh run
```

## Mount somewhere else

```bash
PGFS_MOUNT=/tmp/pgfs ./scripts/pgfs.sh up
```

## Put the cluster on LazyFS for crash tests

```bash
./scripts/init_db.sh --lazyfs
```

This needs [LazyFS](https://github.com/dsrhaslab/lazyfs) installed.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

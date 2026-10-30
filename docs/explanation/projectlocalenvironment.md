# Explanation: Project-Local Development Environment

!!! warning "Status: Partially implemented"
    `scripts/init_db.sh` creates the cluster with `--data-checksums`, checks `fsync` and `full_page_writes`, and formats the `pgfs` database with `pgfs mkfs`; `scripts/pgfs.sh`, the Makefile targets and the integration suite drive the mount, and `run_tests.sh` runs `pgfs check` after every test. The `--lazyfs` mode needs LazyFS installed and is not yet exercised by the suite.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

[Tutorial](../tutorials/projectlocalenvironment.md) · [How-to](../how-to/projectlocalenvironment.md) · [Reference](../reference/projectlocalenvironment.md) · **Explanation**

---

## Why everything lives in the repository

A development filesystem that depends on a system-wide PostgreSQL
leaks state between projects and between people. Keeping the cluster,
the socket, the mount point and the logs under `testdata/` makes a
reset one command, and makes the tests reproducible.

## Why checksums and durability checks at setup

pgfs promises that fsync means durable. That is only true on a server
with `fsync` and `full_page_writes` on, and silent corruption is only
detectable with page checksums, so the local cluster is set up the way
pgfs asks production servers to be.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

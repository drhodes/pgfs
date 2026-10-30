# Tutorial: Project-Local Development Environment

!!! warning "Status: Partially implemented"
    `scripts/init_db.sh` creates the cluster with `--data-checksums`, checks `fsync` and `full_page_writes`, and formats the `pgfs` database with `pgfs mkfs`; `scripts/pgfs.sh`, the Makefile targets and the integration suite drive the mount, and `run_tests.sh` runs `pgfs check` after every test. The `--lazyfs` mode needs LazyFS installed and is not yet exercised by the suite.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

**Tutorial** · [How-to](../how-to/projectlocalenvironment.md) · [Reference](../reference/projectlocalenvironment.md) · [Explanation](../explanation/projectlocalenvironment.md)

---

In this tutorial we will go from a fresh clone to a mounted pgfs
filesystem, write a file, find it in PostgreSQL, and take everything
down again. Everything lives under `testdata/`.

## Step 1: enter the development shell

```bash
nix develop
```

## Step 2: bring it up

```bash
./scripts/pgfs.sh up
```

This creates the cluster in `testdata/pgdata` (with data checksums),
starts it on a Unix socket in `testdata/` and on 127.0.0.1, formats the
`pgfs` database with `pgfs mkfs`, builds the binary and mounts the tree
`main` at `testdata/mnt`.

## Step 3: write a file

```bash
mkdir testdata/mnt/tutorial
echo "hello from a database" > testdata/mnt/tutorial/hello.txt
cat testdata/mnt/tutorial/hello.txt
```

```text
hello from a database
```

## Step 4: find the file in PostgreSQL

```bash
psql -h "$PWD/testdata" -d pgfs -c \
  "SELECT path, ino, kind, size FROM pgfs.files WHERE tree = 'main' AND path LIKE '/tutorial%'"
```

```text
        path         | ino | kind | size
---------------------+-----+------+------
 /tutorial           |   2 | d    |    0
 /tutorial/hello.txt |   3 | f    |   22
```

A file this small lives inside its inode row
([Storage Layout](storagelayout.md)).

## Step 5: take it down

```bash
rm -r testdata/mnt/tutorial
./scripts/pgfs.sh down
./scripts/pgfs.sh status
```

```text
mount:     none
daemon:    not running
postgres:  running
```

## What we did

We built a private PostgreSQL, formatted it, mounted it, and saw a file
become rows. `rm -rf testdata` undoes all of it.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

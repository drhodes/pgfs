# Tutorial: Project-Local Development Environment

!!! success "Status: Stable"
    Everything on this page ships today: `scripts/init_db.sh`, `scripts/pgfs.sh`, the Makefile targets and the integration suite in `scripts/test_*.sh`.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

**Tutorial** · [How-to](../how-to/projectlocalenvironment.md) · [Reference](../reference/projectlocalenvironment.md) · [Explanation](../explanation/projectlocalenvironment.md)

---

In this tutorial we will go from a fresh checkout to a mounted
pgfs filesystem, store a file in it, find that file as rows in
PostgreSQL, and take everything down again. It takes about five
minutes, most of which is the first compile.

All commands run from the root of the repository.

## Before we start

pgfs asks the kernel to remove its mount automatically if the
daemon ever dies. Unprivileged users may only do that when
`/etc/fuse.conf` allows it. Check:

```bash
grep '^user_allow_other' /etc/fuse.conf
```

If that prints nothing, enable it once:

```bash
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

## Step 1: enter the development shell

```bash
nix develop
```

The shell prints a banner with the Rust, PostgreSQL and FUSE
versions it provides. Everything that follows uses those tools.

## Step 2: bring the filesystem up

```bash
./scripts/pgfs.sh up
```

The first run creates a PostgreSQL cluster in `testdata/pgdata`,
compiles pgfs, and mounts it. The last lines look like this:

```text
pgfs:      starting (log: /path/to/pgfs/testdata/pgfs.log)
mounted:   /path/to/pgfs/testdata/mnt
postgres:  PostgreSQL 17.10 on
```

Ask the script what is running:

```bash
./scripts/pgfs.sh status
```

```text
mount:     active
daemon:    running (pid 1519591)
postgres:  running
```

## Step 3: store a file

```bash
mkdir testdata/mnt/tutorial
echo "hello from a database" > testdata/mnt/tutorial/hello.txt
cat testdata/mnt/tutorial/hello.txt
```

```text
hello from a database
```

## Step 4: find the file in PostgreSQL

The file we just wrote is a row in the `entries` table:

```bash
psql -h "$PWD/testdata" -d pgfs -c \
  "SELECT parent, name, kind, size FROM entries WHERE parent = 'tutorial'"
```

```text
  parent  |   name    | kind | size
----------+-----------+------+------
 tutorial | hello.txt | file |   22
```

Its 22 bytes of content are a row in the `blocks` table:

```bash
psql -h "$PWD/testdata" -d pgfs -c \
  "SELECT name, block_no, convert_from(data, 'UTF8') FROM blocks WHERE parent = 'tutorial'"
```

```text
   name    | block_no |     convert_from
-----------+----------+-----------------------
 hello.txt |        0 | hello from a database+
           |          |
```

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

PostgreSQL keeps running so the next `up` is quick.

## What we did

We built pgfs, mounted it, wrote a file through the ordinary
filesystem interface, and saw the same data as rows in two SQL
tables. Next, the [Regular Files](flat_filesystem.md) tutorial
explores reading, writing and truncating files in more depth.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

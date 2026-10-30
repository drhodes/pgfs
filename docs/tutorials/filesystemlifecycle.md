# Tutorial: Administering pgfs

!!! success "Status: Stable"
    `pgfs` has every subcommand: mkfs, migrate, tune, mount, tree, snapshot, branch, promote, changes, diff, check, scrub, export, import, stats and options. Nothing runs DDL implicitly, the option reference in `docs/cli-options.md` is generated from the parser and checked by a test, and `contrib/pgfs@.service` ships. There is one migration so far.

> One command does everything an administrator needs: format a database, mount trees and snapshots, upgrade the schema, tune capacity, and inspect, check and back up the filesystem. Nothing happens implicitly.

**Tutorial** · [How-to](../how-to/filesystemlifecycle.md) · [Reference](../reference/filesystemlifecycle.md) · [Explanation](../explanation/filesystemlifecycle.md)

---

In this tutorial we will format a database, mount it, look
at its statistics and unmount it. We need a PostgreSQL 17
server and an empty database called `pgfs`.

## Step 1: format

```bash
export PGFS_CONN="dbname=pgfs"
pgfs mkfs --capacity 100G
```

```text
formatted pgfs filesystem 6f1c…  block size 65536, inline 2048, capacity 100 GiB
created tree main
```

## Step 2: mount

```bash
mkdir -p /mnt/main
pgfs mount main /mnt/main
df -h /mnt/main
```

## Step 3: inspect

```bash
pgfs stats main
```

## Step 4: unmount

```bash
fusermount3 -u /mnt/main
```

---

<small>Generated from `spec.ops.FilesystemLifecycle` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

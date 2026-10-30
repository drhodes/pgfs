# How-To: Administering pgfs

!!! success "Status: Stable"
    `pgfs` has every subcommand: mkfs, migrate, tune, mount, tree, snapshot, branch, promote, changes, diff, check, scrub, export, import, stats and options. Nothing runs DDL implicitly, the option reference in `docs/cli-options.md` is generated from the parser and checked by a test, and `contrib/pgfs@.service` ships. There is one migration so far.

> One command does everything an administrator needs: format a database, mount trees and snapshots, upgrade the schema, tune capacity, and inspect, check and back up the filesystem. Nothing happens implicitly.

[Tutorial](../tutorials/filesystemlifecycle.md) · **How-to** · [Reference](../reference/filesystemlifecycle.md) · [Explanation](../explanation/filesystemlifecycle.md)

---

## Run a mount under systemd

```bash
systemctl enable --now pgfs@main.service
```

The unit reads `/etc/pgfs/main.env` for `PGFS_CONN` and the
mount point.

## Upgrade pgfs

1. Back up the database.
2. Stop every mount.
3. Install the new binary and run `pgfs migrate`.
4. Mount again.

## Change the capacity

```bash
pgfs tune --capacity 2T
```

---

<small>Generated from `spec.ops.FilesystemLifecycle` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

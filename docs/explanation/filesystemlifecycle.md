# Explanation: Administering pgfs

!!! success "Status: Stable"
    `pgfs` has every subcommand: mkfs, migrate, tune, mount, tree, snapshot, branch, promote, changes, diff, check, scrub, export, import, stats and options. Nothing runs DDL implicitly, the option reference in `docs/cli-options.md` is generated from the parser and checked by a test, and `contrib/pgfs@.service` ships. There is one migration so far.

> One command does everything an administrator needs: format a database, mount trees and snapshots, upgrade the schema, tune capacity, and inspect, check and back up the filesystem. Nothing happens implicitly.

[Tutorial](../tutorials/filesystemlifecycle.md) · [How-to](../how-to/filesystemlifecycle.md) · [Reference](../reference/filesystemlifecycle.md) · **Explanation**

---

## Nothing implicit

The first pgfs created and patched its tables every time it
mounted. That is convenient until the day a mount runs
against the wrong database, or an old binary meets a newer
schema. pgfs now separates formatting, migrating and
mounting, records the schema version, and refuses anything
it does not recognise.

---

<small>Generated from `spec.ops.FilesystemLifecycle` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

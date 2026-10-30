# How-To: Integrity and Backup

!!! warning "Status: Partially implemented"
    Blob checksums are verified on every read, `pgfs check` covers every invariant class plus amcheck and seals snapshots, `pgfs scrub` resumes from its position and runs continuously with `--continuous`, and `pgfs export` / `pgfs import` round-trip names, modes, owners, times, links, special files and xattrs. Not yet: pax sparse headers (export writes holes as zero bytes; import turns zero blocks back into holes).

> Every block pgfs returns is checked against a checksum taken when it was written. `pgfs check` verifies the filesystem's invariants, `pgfs scrub` finds silent corruption before a reader does, and backups use Postgres's own tools or a portable tar export of a snapshot.

[Tutorial](../tutorials/dataintegrity.md) · **How-to** · [Reference](../reference/dataintegrity.md) · [Explanation](../explanation/dataintegrity.md)

---

## Back up the whole filesystem

Back up the Postgres cluster with WAL archiving, for example
with pgBackRest or `pg_basebackup`. Nothing else needs
saving.

## Restore to a point in time

Restore the cluster with point-in-time recovery to the
moment you want, then mount it. The filesystem is exactly
as a crash at that moment would have left it. To mount the
restored copy next to the original on one machine, first
give it a new identity:

```bash
pgfs tune --conn "$RESTORED" --new-uuid
```

## Move a tree to another pgfs filesystem

```bash
pgfs snapshot main@move
pgfs export main@move | ssh other pgfs import main
```

## Respond to a checksum failure

The error log names the path and block. Restore that file
from a snapshot or a backup, and run `pgfs scrub` to find
any others. Check the database host's disks.

---

<small>Generated from `spec.integrity.DataIntegrity` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

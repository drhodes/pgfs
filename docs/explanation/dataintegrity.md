# Explanation: Integrity and Backup

!!! warning "Status: Partially implemented"
    Blob checksums are verified on every read, `pgfs check` covers every invariant class plus amcheck and seals snapshots, `pgfs scrub` resumes from its position and runs continuously with `--continuous`, and `pgfs export` / `pgfs import` round-trip names, modes, owners, times, links, special files and xattrs. Not yet: pax sparse headers (export writes holes as zero bytes; import turns zero blocks back into holes).

> Every block pgfs returns is checked against a checksum taken when it was written. `pgfs check` verifies the filesystem's invariants, `pgfs scrub` finds silent corruption before a reader does, and backups use Postgres's own tools or a portable tar export of a snapshot.

[Tutorial](../tutorials/dataintegrity.md) · [How-to](../how-to/dataintegrity.md) · [Reference](../reference/dataintegrity.md) · **Explanation**

---

## Why checksum on top of Postgres

Postgres's page checksums catch a damaged page on disk. They
do not catch a bug that writes the wrong bytes, a damaged
TOAST chunk read through a valid page, or a standby that
replicated the damage. Studies of real storage stacks found
that redundancy usually spreads corruption rather than
stopping it. A checksum computed by the daemon when data is
written, and checked when it is read, covers the whole path.

## Why fsck is SQL

Every invariant of a pgfs filesystem is a statement about
rows, so each can be written as a query that returns the
rows that break it. The checker runs on a consistent
snapshot of the database while the filesystem stays
mounted.

## Why backup is Postgres's job

pgfs's whole state is in its database. Postgres already has
mature, tested backup and point-in-time recovery, and a
second backup system would only add a second way to be
wrong. pgfs adds what Postgres cannot: a portable,
file-level export of a consistent snapshot.

---

<small>Generated from `spec.integrity.DataIntegrity` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

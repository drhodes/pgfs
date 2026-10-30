# Tutorial: Integrity and Backup

!!! warning "Status: Partially implemented"
    Blob checksums are verified on every read, `pgfs check` covers every invariant class plus amcheck and seals snapshots, `pgfs scrub` resumes from its position and runs continuously with `--continuous`, and `pgfs export` / `pgfs import` round-trip names, modes, owners, times, links, special files and xattrs. Not yet: pax sparse headers (export writes holes as zero bytes; import turns zero blocks back into holes).

> Every block pgfs returns is checked against a checksum taken when it was written. `pgfs check` verifies the filesystem's invariants, `pgfs scrub` finds silent corruption before a reader does, and backups use Postgres's own tools or a portable tar export of a snapshot.

**Tutorial** · [How-to](../how-to/dataintegrity.md) · [Reference](../reference/dataintegrity.md) · [Explanation](../explanation/dataintegrity.md)

---

In this tutorial we will check a filesystem, scrub it, and
take a portable backup of a snapshot.

## Step 1: check

```bash
pgfs check
```

```text
checked 3 trees, 41 210 inodes, 9 snapshots: no violations
amcheck: 14 indexes ok
```

## Step 2: scrub

```bash
pgfs scrub --rate 200M
```

```text
scrubbed 18.4 GiB in 94 s: 0 checksum failures
```

## Step 3: export a snapshot

```bash
pgfs snapshot main@backup-1
pgfs export main@backup-1 | zstd > main-backup-1.tar.zst
```

The archive is a plain tar file that any system can
extract.

---

<small>Generated from `spec.integrity.DataIntegrity` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

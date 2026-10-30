# Hosting PostgreSQL itself

Status: idea, not specified. Recorded 2026-10-30.

pgfs needs a PostgreSQL, and PostgreSQL needs a filesystem that is not
pgfs. What is the least that can sit underneath?

Answer: an ordinary local filesystem (ext4 or XFS) holding nothing but
the PostgreSQL cluster. Nothing smaller works: PostgreSQL has no
raw-device mode, and it cannot sit on the pgfs it serves.

---

## Tutorial

Not applicable: this is a layout decision, not a skill to practise. The
[OS upgrades tutorial](os-upgrades-with-branches.md) shows what the layout
is for.

---

## How-to guides

### Lay out a machine whose root is on pgfs

```
ESP / boot      vfat, ~512 MB     kernel + initramfs
system image    squashfs, ~150 MB read-only: init, postgres, pgfs, CA certificates
/pg             ext4              the PostgreSQL cluster (data and WAL)
/               pgfs              everything else
```

At boot the initramfs:

1. mounts the squashfs image and `/pg`;
2. starts `postgres -D /pg/data`;
3. runs `pgfs mount os /sysroot`;
4. runs `switch_root`.

PostgreSQL and the pgfs daemon run from the read-only image, so they
never depend on the filesystem they serve.

### Size the PostgreSQL partition

Plan for roughly 1.3 to 1.5 times the data you will store, plus
`max_wal_size` (1 GB by default) of WAL headroom. pgfs adds about 256
bytes per inode row, and stores blobs lz4-compressed.

### Make the machine diskless instead

Run PostgreSQL on another host. The client needs only a kernel and an
initramfs (PXE works); use `sslmode=verify-full`
(`security.RequiresTlsForRemote`) and synchronous standbys on the server
if fsync must survive the loss of the server.

---

## Reference

### What PostgreSQL needs from its filesystem

| Needs | Doesn't need |
|---|---|
| `fsync` and `fdatasync` that reach the disk, on files and directories | xattrs, ACLs, set-id, device nodes |
| atomic `rename` | hard links (only `pg_upgrade --link`) |
| `O_EXCL` create (`postmaster.pid`) | sparse files, `mmap` of data files |
| extending files with `write` or `posix_fallocate` | large directories (a few thousand files) |
| symlinks, only with tablespaces | snapshots (pgfs provides them above) |
| shared memory (`/dev/shm` or SysV), not on the data directory | |

pgfs refuses to mount unless the server runs with `fsync = on` and
`full_page_writes = on` (`durability.ChecksServerDurability`).

### Options

| Option | Underneath PostgreSQL | Trade-off |
|---|---|---|
| local ext4 or XFS partition | a few GB | the simplest; one boring partition |
| PostgreSQL on another host | nothing local | needs the network to boot; stalls when it is down |
| PostgreSQL on ZFS | a ZFS pool | ZFS checksums and redundancy below, pgfs features above |
| PostgreSQL on pgfs (nested) | another pgfs | only moves the problem down a level; slow |

---

## Explanation

### Why the floor is a normal local filesystem

PostgreSQL asks for very little: durable fsync, atomic rename and file
extension. Every mainstream journaling filesystem provides that, and
none of it can be provided by pgfs to the PostgreSQL that pgfs depends
on. So the floor is one small partition with one cluster on it and no
OS.

### Why that split is attractive

The partition stays tiny and boring. pgfs supplies what one might
otherwise want from ZFS or btrfs on the root filesystem (snapshots,
branches, atomic promote, checksums), and backups are plain PostgreSQL
backups (`integrity.BackupIsPostgresBackup`) that capture every tree and
snapshot consistently.

### The nested case

A second PostgreSQL can keep its data directory on pgfs: rename,
directory fsync, `fallocate` and durable fsync are all there. It is a
good test of the POSIX layer, but every 8 KB page write becomes a blob
write in the lower database, and the lower database still needs a real
filesystem.

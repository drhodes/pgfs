# An operating system on pgfs

Status: idea, not specified, not yet tried. Recorded 2026-10-30;
`--suid` and `--dev` added the same day (`security.SetIdAndDevicesOptIn`).

Could an operating system live on pgfs: first as a container or chroot
root, then as the real root filesystem? Most of what an OS needs is
already there, including the `--suid` and `--dev` mount flags a container
needs. More is needed before a machine can boot from it.

---

## Tutorial

Not yet written: it needs root, and nobody has run it end to end. The
steps would be: mount a tree as root with `--suid`, `debootstrap` into
it, and boot it with `systemd-nspawn -b` (see the how-to below). Read the
security notes in the reference first. The [OS upgrades
tutorial](os-upgrades-with-branches.md) shows the branching part with a
simulated system tree, without root.

---

## How-to guides (outline)

### Install a container rootfs

```bash
sudo pgfs mount os /srv/os --allow-other --suid
sudo debootstrap stable /srv/os
sudo systemd-nspawn -D /srv/os          # or: chroot /srv/os
```

`debootstrap` creates device nodes; add `--dev` only if something must
open them from the tree (a container gets `/dev` from its runtime).

Run the daemon as root: files must be owned by root inside the tree, and
`default_permissions` lets the kernel apply the usual rules.

### Boot a machine with root on pgfs (sketch)

The full plan, with what is missing and the order to try it in, is in
[Booting an OS on bare metal from pgfs](bare-metal-root.md).

1. Keep `/boot` (kernel and initramfs) on a local disk or on the network.
2. In the initramfs: bring up networking, then run
   `pgfs mount os /sysroot --allow-other --suid` (the booted system mounts
   devtmpfs on `/dev`, so `--dev` is not needed), then
   `switch_root /sysroot`.
3. Start the daemon so systemd treats it as a root storage daemon
   (argv[0] starting with `@`), so it is not killed after `switch_root`
   and is stopped last at shutdown.
4. Upgrade the `pgfs` binary only by rebooting.

---

## Reference

### What an OS needs, and what pgfs does

| Need | pgfs |
|---|---|
| owners, modes, set-id bits | stored; cleared on chown and non-owner writes as Linux does |
| set-uid programs | stored; honoured only with `--suid` (root only) |
| device nodes, FIFOs, sockets | `mknod` stores them; device nodes open only with `--dev` (root only) |
| hard links, symlinks | yes |
| xattrs (`security.capability` for `ping`) | yes (`user.`, `trusted.`, `security.`) |
| POSIX ACLs (journald, logind) | no: EOPNOTSUPP |
| `O_TMPFILE` | no: EOPNOTSUPP; most tools fall back |
| open-after-unlink, atomic rename-over, `O_EXCL` lockfiles | yes |
| durable `fsync` (dpkg, rpm) | yes, each one a synchronous commit |
| file locks | handled by the kernel, local to the machine |
| writable shared `mmap` (SQLite, used by apt and rpm) | untested with the writeback cache off |
| swap file | no: not possible on FUSE |

### Missing pieces

| Piece | Size |
|---|---|
| initramfs hook | moderate |
| root-storage-daemon handling | small |
| in-place daemon upgrade (`ops.UpgradesByRemount`, hand-over of `/dev/fuse`) | planned in the spec, not built |

### Measured cost

About 5 ms per file create in `make bench`, so a debootstrap of tens of
thousands of files would take several minutes.

---

## Explanation

### The hard limit

PostgreSQL cannot live on the filesystem it serves, and the kernel and
bootloader cannot load from a FUSE filesystem. So `/boot`, the
initramfs and the database are always somewhere else: a local disk, or
another machine. See [Hosting PostgreSQL itself](hosting-postgres.md).

### Why a root on pgfs is unforgiving

Every file operation of the whole OS goes through one daemon and the
network to the database. If the database becomes unreachable, the system
stalls, then gets EIO after the timeouts. The daemon fails fast and
recovers by reconnecting and revalidating (`failure.ReconnectsWithBackoff`),
but a root filesystem has nothing to fall back on.

### What `--suid` costs

With `--suid`, anyone who can write the tree can gain root on the host
that mounts it: any holder of the database's runtime role (the SQL
functions take owners and modes as given), the schema owner, `pgfs
import` of a crafted archive, and whoever prepared a branch that is
promoted or a snapshot that is mounted. One runtime role writes every
tree in its database, so keep an OS tree in a database whose writers
you would trust with root, and prefer mounting a snapshot or
`--read-only` on machines that only run it. `--dev` extends the same
writers to the host's devices. See `security.SetIdAndDevicesOptIn`.

### Why it is still interesting

Branches give whole-OS snapshots and atomic upgrades ([OS upgrades with
branches](os-upgrades-with-branches.md)), one PostgreSQL backup captures
every OS version, and any machine can boot or inspect any tree. A
container rootfs is the realistic first step: it needs only the two mount
flags.

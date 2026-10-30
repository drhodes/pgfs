# Booting an OS on bare metal from pgfs

Status: idea, not specified, not yet tried. Recorded 2026-10-30, against
pgfs 0.3.0. This extends [An operating system on pgfs](os-on-pgfs.md),
which covers containers and the filesystem features an OS needs, to a
machine whose root filesystem is a pgfs tree.

Short answer: yes in principle, not today. The filesystem side is mostly
ready; as root with `--suid`, set-id programs and device nodes behave as
an OS needs (checked on 2026-10-30 with a set-uid-root binary and a
device node, see `security.SetIdAndDevicesOptIn`). What is missing is the
plumbing that starts pgfs before the OS has a root filesystem and stops
it after the OS has let go of it.

---

## Tutorial

Not yet possible: pgfs cannot mount without AutoUnmount, and there is no
initramfs module. The [How-to guides](#how-to-guides) give the plan,
starting in a virtual machine.

---

## How-to guides

### Boot a QEMU VM with root on pgfs and PostgreSQL on the host (step 1)

1. Add `--no-auto-unmount` to `pgfs mount` (specify it first, as a
   requirement next to `app.MountsCleanly`).
2. Create a tree and fill it, as root:
   ```bash
   sudo pgfs mount os /srv/os --allow-other --suid
   sudo debootstrap stable /srv/os
   ```
3. Build an initramfs with a dracut module that loads `fuse.ko`, brings
   up networking, and runs
   `exec -a @pgfs pgfs mount os /sysroot --allow-other --suid --no-auto-unmount`
   with the tree and connection string taken from the kernel command
   line (`pgfs.tree=`, `pgfs.conn=`).
4. Boot the VM with that kernel and initramfs, with PostgreSQL reachable
   on the host network, and a password file or `sslmode=verify-full` as
   for any remote mount (`security.RequiresTlsForRemote`).
5. Try what an OS does: boot, log in, install packages, power off
   uncleanly, restart PostgreSQL under it, upgrade in a branch and boot
   the branch with `pgfs.tree=upgrade`.

`debootstrap` and QEMU are not in the project's dev shell; add them to
`flake.nix` or use the host's.

### Move PostgreSQL into the VM (step 2)

Put the cluster on its own small partition
([Hosting PostgreSQL itself](hosting-postgres.md)), start it from the
initramfs before mounting `/sysroot`, and work out how it survives
`switch_root` and how shutdown stops it last (see the reference).

### Go to real hardware (step 3)

Only once a VM survives boots, upgrades, unclean power-offs and a
database restart. Keep the previous tree or snapshot bootable from the
bootloader menu, so a bad boot has a way back.

### Choose where PostgreSQL runs

- **Remote (diskless client).** Simpler, like NFS root: the initramfs
  needs networking and TLS, not a database. The machine freezes whenever
  the network or the database is down.
- **Local, on its own partition.** Self-contained, but the initramfs
  must start a database first, which raises the `switch_root` and
  shutdown problems below.

---

## Reference

### Boot sequence

```
firmware → bootloader → kernel + initramfs        (from /boot on local disk, or PXE)
initramfs:  load fuse.ko
            start PostgreSQL (local /pg partition)  ── or reach a remote one over the network
            pgfs mount os /sysroot --allow-other --suid --no-auto-unmount
            switch_root /sysroot → systemd → the OS
```

### Pieces to build or solve

| Piece | Why | Size |
|---|---|---|
| `--no-auto-unmount` on `pgfs mount` | AutoUnmount goes through `fusermount3` and a helper process that tears the mount down when the daemon dies. A root filesystem wants a plain `mount(2)` as root, and no helper to kill at `switch_root`. | small, in pgfs |
| initramfs module (dracut or mkinitcpio) | ships `pgfs` and its libraries (rustls, no OpenSSL); reads `pgfs.tree=` and `pgfs.conn=`; runs the promote-at-boot step for upgrades ([OS upgrades with branches](os-upgrades-with-branches.md)); mounts `/sysroot` | about a day |
| surviving `switch_root` | systemd kills initramfs processes unless argv[0] starts with `@` (its root storage daemon convention): `exec -a @pgfs` | small |
| PostgreSQL surviving `switch_root`, when local | the same rule, but PostgreSQL rewrites its process title, which may erase the `@`; try `update_process_title = off`, or start it from a unit inside the initramfs | the riskiest piece; needs experiment |
| shutdown order | systemd must unmount root, then pgfs commits its buffers and runs the synchronous barrier, then PostgreSQL stops last: systemd-shutdown hands back to `/run/initramfs` (dracut supports this) | moderate |
| protecting the daemons | `oom_score_adj = -1000` for pgfs and PostgreSQL; if either dies, the root filesystem is gone | small |
| swap | not possible on FUSE: use zram or a swap partition | configuration |

### What to expect once it works

| Aspect | Expectation |
|---|---|
| boot time | slower than ext4: the kernel caches lookups and attributes, but each first lookup is a database query |
| package installs | about 5 ms per file create (`make bench`), plus a synchronous commit for every dpkg fsync |
| upgrades | snapshot first (`pgfs snapshot os@before`), test in a branch, boot it with `pgfs.tree=upgrade` |
| untested features | POSIX ACLs (journald; pgfs answers EOPNOTSUPP), `O_TMPFILE`, writable shared `mmap` (SQLite in apt) |
| database outage | the whole OS stalls, then gets EIO after the timeouts |
| security | with `--suid`, everyone who can write the tree can gain root on the machine (`security.SetIdAndDevicesOptIn`) |

### Kernel command line (proposed)

| Parameter | Meaning |
|---|---|
| `pgfs.tree=NAME[@SNAPSHOT]` | the tree or snapshot to mount as root |
| `pgfs.conn=…` | the connection string (the password from a file in the initramfs, never the command line) |
| `pgfs.promote=BRANCH` | promote BRANCH into the tree before mounting it |

---

## Explanation

### Why the hard part is not the filesystem

An OS needs owners, modes, set-id programs, device nodes, links, xattrs,
durable fsync and atomic rename. pgfs has all of them. What is hard is
that pgfs is a userspace daemon talking to a database: both must be
running before the OS has a root filesystem, must survive the hand-over
from the initramfs to the real system, and must keep running until the
OS has finished with its root filesystem at shutdown. That is the same
problem NFS root, iSCSI root and other network root filesystems solve,
and systemd and dracut have conventions for it: root storage daemons
marked with `@`, and a return to the initramfs at shutdown.

### Why AutoUnmount is wrong for a root filesystem

AutoUnmount protects a normal mount from going stale when the daemon
dies, by having a `fusermount3` helper unmount it. For a root
filesystem there is nothing to fall back on whether or not the mount
goes stale, and the helper is one more process that must survive
`switch_root`. Mounting directly as root, without the helper, is simpler
and safer there.

### Why start in a virtual machine

Every missing piece fails in the same way: the machine does not boot, or
does not shut down cleanly. A VM makes those failures cheap to observe
and repeat, and lets PostgreSQL run on the host first, so the boot path
can be built one problem at a time.

### What it would give

Whole-OS snapshots and branches, atomic upgrades with a check that the
system tree did not change underneath, every OS version in one
PostgreSQL backup, and any machine able to boot or inspect any tree.
ZFS boot environments give most of this on one machine; pgfs adds the
network and SQL.

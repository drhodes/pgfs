# OS upgrades with branches

Status: idea, not specified. Recorded 2026-10-30; the tutorial was run
against pgfs 0.2.0 with a simulated OS tree. Upgrading a real root
filesystem also needs the pieces in [An operating system on
pgfs](os-on-pgfs.md).

The idea: branch the whole system, upgrade the branch next to the running
system, test it, then publish it atomically, with the old system kept as
a snapshot for rollback. ZFS boot environments, ostree and NixOS
generations do the same; pgfs adds that any machine can mount the result.

---

## Tutorial: upgrade a system tree without touching the running one

In this tutorial we will build a tiny "system" tree with a program and a
config file, give it a separate `var` tree for state, upgrade the system
in a branch while state keeps changing, and promote the upgrade. We need
the setup from [the index](README.md).

### Step 1: build the system tree

```bash
mkdir -p /tmp/sys /tmp/upg
pgfs mount main /tmp/sys & MAIN=$!; sleep 1
mkdir -p /tmp/sys/etc /tmp/sys/usr/bin /tmp/sys/var
echo version=1 > /tmp/sys/etc/app.conf
printf '#!/bin/sh\necho tool v1\n' > /tmp/sys/usr/bin/tool
chmod +x /tmp/sys/usr/bin/tool
```

### Step 2: give state its own tree

```bash
pgfs branch main:/var var
pgfs mount var /tmp/sys/var & VAR=$!; sleep 1
mkdir /tmp/sys/var/log
echo "boot 1" >> /tmp/sys/var/log/messages
```

`var` is a branch rooted at the empty `/var` directory, mounted over it.
Writes under `/tmp/sys/var` now go to the `var` tree and never change
`main`. Do this once, before any upgrade.

### Step 3: branch and upgrade

```bash
pgfs snapshot main@before-upgrade
pgfs branch main@before-upgrade upgrade
pgfs mount upgrade /tmp/upg & UPG=$!; sleep 1
printf '#!/bin/sh\necho tool v2\n' > /tmp/upg/usr/bin/tool
echo version=2 > /tmp/upg/etc/app.conf
/tmp/upg/usr/bin/tool; /tmp/sys/usr/bin/tool
```

```text
tool v2
tool v1
```

The running system still runs v1. Meanwhile, state keeps changing:

```bash
echo "boot 2" >> /tmp/sys/var/log/messages
```

### Step 4: promote

Promotion needs both trees unmounted, as at a reboot:

```bash
kill $UPG; wait $UPG; kill $VAR; wait $VAR; kill $MAIN; wait $MAIN
pgfs promote upgrade main
```

```text
folded 2 rows of upgrade into main; previous contents are main@pre-promote-20261030T151842094851Z
```

### Step 5: "reboot" and look

```bash
pgfs mount main /tmp/sys & MAIN=$!; sleep 1
pgfs mount var /tmp/sys/var & VAR=$!; sleep 1
/tmp/sys/usr/bin/tool; cat /tmp/sys/etc/app.conf /tmp/sys/var/log/messages
pgfs diff main@before-upgrade main
```

```text
tool v2
version=2
boot 1
boot 2
modified  /etc/app.conf
modified  /usr/bin/tool
```

The upgrade landed, both log lines survived, and the old system is
`main@before-upgrade` (and `main@pre-promote-…`).

### What we did

We upgraded a system tree in a branch while the running system kept
writing its state, and published the upgrade with a check that nothing in
the system tree changed underneath it.

---

## How-to guides

### Upgrade a container rootfs

As root, with the security notes of [An operating system on
pgfs](os-on-pgfs.md) in mind:

```bash
pgfs snapshot os@before-upgrade
pgfs branch os@before-upgrade upgrade
pgfs mount upgrade /mnt/upgrade --suid
systemd-nspawn -D /mnt/upgrade apt full-upgrade
systemd-nspawn -b -D /mnt/upgrade         # boot-test in a container
```

### Publish, keep, or discard the upgrade

| Goal | Command | When |
|---|---|---|
| discard | `pgfs tree delete upgrade` | any time |
| boot it as is | select the tree at boot (`pgfs.tree=upgrade` on the kernel command line, read by the initramfs) | next boot |
| make it `os` | `pgfs promote upgrade os` | with neither tree mounted read-write, so from the initramfs before root is mounted |

### Recover when `promote` says ESTALE

The running system wrote to the system tree after the branch point. See
what it wrote:

```bash
pgfs diff os@before-upgrade os
```

Then either copy what matters into the branch and promote again, or
replace and keep the old state as a snapshot:

```bash
pgfs promote --force upgrade os       # previous contents kept as os@pre-promote-<ts>
```

To stop it happening, move state to its own trees (tutorial, step 2).

### Roll back

```bash
pgfs branch os@pre-promote-<ts> rollback
pgfs promote --force rollback os
```

---

## Reference

### Outcomes of promote

| Situation | Result |
|---|---|
| the system tree unchanged since the branch point | normal promote: the branch folds into the tree's layer; old state kept as `<tree>@pre-promote-<ts>` |
| the system tree changed | ESTALE; `--force` replaces it (old state kept the same way) |
| either tree mounted read-write | EBUSY |
| the branch has snapshots, or branches of its own | EBUSY |

### Splitting state from the system

| Tree | Mounted at | Changes during normal running |
|---|---|---|
| `os` (system) | `/` | no: only upgrades change it |
| `var` (`pgfs branch os:/var var`) | `/var` | yes |
| `home` (`pgfs branch os:/home home`) | `/home` | yes |

`/etc` is the hard case: keep it in the system tree and treat local
changes like an upgrade, or give it its own tree and merge by hand.

### Missing before a real root filesystem can do this

| Piece | Why |
|---|---|
| initramfs hook for `pgfs.tree=` and `pgfs.promote=` | promote must run before root is mounted |
| systemd root-storage-daemon handling | the daemon must survive `switch_root` and stop last at shutdown |
| an "empty tree" command | today a separate state tree is made by branching an (empty) directory |

---

## Explanation

### Why the system tree must not change while you upgrade

`promote` is a compare-and-swap. It folds the branch back only if the
target has not changed since the branch point, because folding back over
a tree that changed would silently lose those changes. A running OS
writes all the time (logs, caches, `/var/lib`, `/home`), so a single tree
for everything always fails that check. Splitting state into its own
trees, as ostree and NixOS do, keeps the system tree still between
upgrades and lets the normal promote through.

### Why branching at the right moment matters

Branching a *live* tree closes its current epoch, so it counts as a
change for any branch made earlier. Create the state trees once, before
the first upgrade, and branch upgrades from a named snapshot
(`os@before-upgrade`), which also gives `pgfs diff` a fixed point to
compare against.

### What this gives that A/B partitions do not

The branch stores only what the upgrade changed, so there is no second
copy of the system. `pgfs diff` lists every file an upgrade touched. Any
machine can mount `upgrade` to test it, so one upgrade can be tested on
one box and booted on a fleet. And because snapshots are rows, one
PostgreSQL backup captures every OS version at once.

# Can pgfs be used like git?

Status: idea, not specified. Recorded 2026-10-30; the tutorial was run
against pgfs 0.2.0.

Short answer: yes, for git's *branching model*, with one adjustment: **a
commit is a snapshot, not a branch.** It is not a replacement for git
itself (no three-way merge, no distribution).

---

## Tutorial: version a dataset like a repository

In this tutorial we will commit two versions of a small dataset, look at
what changed, work on a feature branch, and fast-forward the main line to
it. We need the setup from [the index](README.md): a fresh `pgfs_tut`
database, `PGFS_CONN` pointing at it, and the `pgfs` alias.

### Step 1: mount the main line and add files

```bash
mkdir -p /tmp/proj /tmp/feat
pgfs mount main /tmp/proj &
sleep 1
mkdir /tmp/proj/data
echo v1 > /tmp/proj/data/notes.txt
head -c 3000000 /dev/urandom > /tmp/proj/data/model.bin
echo x > /tmp/proj/old.txt
```

### Step 2: commit twice

```bash
pgfs snapshot main@c1
echo v2 > /tmp/proj/data/notes.txt
pgfs snapshot main@c2
pgfs snapshot list
pgfs diff main@c1 main@c2
```

```text
created snapshot main@c1
created snapshot main@c2
main@c1   2026-10-30 08:15:22  epoch 1  unique bytes 0
main@c2   2026-10-30 08:15:22  epoch 2  unique bytes 0
modified  /data/notes.txt
```

Each snapshot took constant time while the tree stayed mounted; the
mounted daemon committed its buffered writes first.

### Step 3: branch and work on the branch

```bash
pgfs branch main@c2 feat
kill %1; wait                       # the main line is not needed while we work
pgfs mount feat /tmp/feat &
sleep 1
echo feature >> /tmp/feat/data/notes.txt
dd if=/dev/urandom of=/tmp/feat/data/model.bin bs=65536 count=1 seek=10 conv=notrunc
mv /tmp/feat/old.txt /tmp/feat/renamed.txt
echo new > /tmp/feat/added.txt
kill %1; wait
```

The branch shares every byte with `main@c2` and stores only what we
changed: one 64 KiB block of the 3 MB file, a few rows for the rest.

### Step 4: review and fast-forward

```bash
pgfs diff main@c2 feat
pgfs promote feat main
pgfs snapshot main@c3
pgfs diff main@c2 main@c3
```

```text
added     /added.txt
renamed   /old.txt -> /renamed.txt
modified  /data/notes.txt
modified  /data/model.bin
folded 8 rows of feat into main; previous contents are main@pre-promote-20261030T151704589947Z
added     /added.txt
renamed   /old.txt -> /renamed.txt
modified  /data/notes.txt
modified  /data/model.bin
```

### Step 5: check

```bash
pgfs check
```

`check: 0 violation(s)`. It also sealed each snapshot with a digest, so
any later change to one would be reported.

### What we did

We used snapshots as commits, a branch as a feature branch, `pgfs diff`
as `git diff`, and `pgfs promote` as a fast-forward merge, on a dataset
with a binary file that git would have stored whole.

---

## How-to guides

### Commit

```bash
pgfs snapshot proj@c42        # names: 1-64 of [A-Za-z0-9._-], not starting with "."
```

### Check out an old commit

```bash
pgfs mount proj@c42 /tmp/c42   # read-only; instant; any number of machines at once
```

### Start a feature, then fast-forward

```bash
pgfs branch proj@c42 feat
pgfs mount feat /work/feat     # work; you may snapshot feat@w1, feat@w2 as you go
pgfs diff proj@c42 feat        # review against the branch point
pgfs snapshot delete feat@w1   # a branch with snapshots of its own cannot be promoted
pgfs snapshot delete feat@w2
pgfs promote feat proj         # unmount both first; ESTALE if proj moved
```

### Rebase when `promote` says ESTALE

There is no rebase command. Branch again from the newer line and replay
your changes, using `pgfs diff proj@c42 feat` as the list:

```bash
pgfs snapshot proj@c43
pgfs branch proj@c43 feat2
# re-apply the changes listed by: pgfs diff proj@c42 feat
pgfs promote feat2 proj && pgfs tree delete feat
```

### Throw a line of work away, or reset to it

```bash
pgfs tree delete feat                 # discard; space comes back in the background
pgfs promote --force feat proj        # reset proj to feat; old state kept as proj@pre-promote-…
```

### Prune old commits

```bash
pgfs snapshot delete proj@c1
```

The reclaimer frees the rows and blobs no remaining snapshot can see.

---

## Reference

### Command mapping

| git | pgfs | Notes |
|---|---|---|
| `git commit` | `pgfs snapshot proj@c42` | constant time; a mounted tree drains its buffers first |
| `git branch feat` | `pgfs branch proj feat`, or `proj@c42 feat`, or `proj:/subdir feat` | constant time; any subdirectory |
| `git checkout c42` | `pgfs mount proj@c42 /tmp/c42` | read-only, instant, nothing copied |
| `git diff a b` | `pgfs diff proj@a proj@b`, `pgfs diff proj@a feat` | between two views of one tree, or a branch and its base; `--json` |
| `git log` | `pgfs snapshot list proj`, `pgfs changes proj` | the change feed has every individual change, in order |
| `git merge` (fast-forward) | `pgfs promote feat proj` | ESTALE if `proj` changed since the branch point |
| `git reset --hard` | `pgfs promote --force feat proj` | previous state kept as `proj@pre-promote-<ts>` |
| `git stash` | branch, later promote or delete | |
| `git gc` | automatic | `space.ReclaimsRetiredRows` |

### What `pgfs diff` reports

| Line | Meaning |
|---|---|
| `added PATH` | the inode has a name in the second view and none in the first |
| `removed PATH` | the inode had a name in the first view and has none in the second |
| `renamed OLD -> NEW` | the inode has a different name in the second view |
| `modified PATH` | a non-directory whose content (mtime or size), mode, owner or symlink target differs |

### What git has and pgfs does not

| Feature | State in pgfs |
|---|---|
| three-way merge | not built; the per-inode changes from `pgfs diff` against the branch point are the raw material |
| commit message, author, parent | a snapshot has a name, a time and an epoch only |
| clone, push, pull | none: one shared database (like Perforce or Subversion); Postgres replicas give read copies |
| blame, bisect, signing | none |
| thousands of commits | untested; reclaim and `pgfs check` scan the snapshot list |

### Limits that matter here

| Limit | Value | Spec |
|---|---|---|
| branch depth (branch of a branch) | 8; background flatten above 4 | `branch.LimitsBranchDepth` |
| snapshot cost | two rows | `branch.CreatesSnapshot` |
| promote preconditions | no read-write mount on either tree; the branch has no snapshots and no branches of its own | `branch.PromotesBranch` |

---

## Explanation

### Why a commit is a snapshot

A snapshot records the tree's current epoch and advances it. It costs two
rows whatever the size of the tree, adds nothing to lookups, and stores
nothing until something changes afterwards. A branch is a new layer that
every lookup walks through to its base; branching for every commit would
build a chain that hits the depth cap after eight commits. So snapshots
play git's commits and branches play git's branches.

### Why it is good at what git is bad at

git stores whole objects, so a one-byte change to a 10 GB file is a new
10 GB object (hence git-lfs). pgfs stores files as 64 KiB blocks behind a
block map, and a change writes new blocks only. There is also no working
copy to materialise: the mount *is* the checkout, and mounting a commit
is instant. And it versions the whole filesystem: modes, owners,
nanosecond times, xattrs, hard links, device nodes and empty directories.

### Why `promote` refuses instead of merging

`promote` is a compare-and-swap: it publishes a branch only if the target
has not changed since the branch point, and otherwise says ESTALE. That
makes it safe, because nothing is ever silently overwritten, but it is
only a fast-forward. A real merge needs a three-way comparison per inode
and a way to report conflicts; the epoch diffs make that possible, but it
is not built.

### Where it fits

As a versioned, shared workspace with git's branching model: datasets,
build trees, ML checkpoints, or a whole OS image (see [OS upgrades with
branches](os-upgrades-with-branches.md)). Source code is still better
kept in git, which runs fine on top of pgfs (`app.SupportsGitInit`).

### Possible follow-ups

1. Snapshot metadata: `message`, `author` and the parent snapshot in
   `pgfs.snapshots`.
2. `pgfs merge`: a three-way merge from the two epoch diffs against the
   branch point.
3. Measure many snapshots against reclaim and `pgfs check`.

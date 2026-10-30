# Tutorial: POSIX Compatibility

!!! success "Status: Stable"
    Modes, owners and nanosecond times stick, set-id bits are cleared on chown and non-owner writes, hard links, symlinks, special files, xattrs, open-after-unlink, renameat2 flags, exclusive create, sparse files with SEEK_DATA and SEEK_HOLE and the unsupported operations' errnos are built and tested (`tests/storage.rs`, `test_posix.sh`, the git workload). pjdfstest and xfstests runs belong to `verify.PosixSuites`.

> pgfs behaves like a local Linux filesystem for the operations that real tools use: permissions, ownership and times that stick, hard links, symlinks, xattrs, files that stay open after deletion, atomic rename variants and sparse files. What it does not support, it refuses with a stated error.

**Tutorial** · [How-to](../how-to/posixcompatibility.md) · [Reference](../reference/posixcompatibility.md) · [Explanation](../explanation/posixcompatibility.md)

---

In this tutorial we will run the operations that git and
editors depend on, and see each one behave as on a local
disk. We need a mounted filesystem at `/mnt/pgfs`.

## Step 1: permissions stick

```bash
cd /mnt/pgfs
printf '#!/bin/sh\necho hook ran\n' > hook
chmod +x hook && ./hook
```

```text
hook ran
```

## Step 2: hard links and symlinks

```bash
ln hook hook2 && ln -s hook hook3
stat -c '%h %i %n' hook hook2
readlink hook3
```

```text
2 1002 hook
2 1002 hook2
hook
```

## Step 3: a file survives deletion while open

```bash
exec 3<>scratch; echo kept >&3; rm scratch
cat /proc/self/fd/3 2>/dev/null || cat <&3
exec 3>&-
```

## Step 4: git works end to end

```bash
git init -q repo && cd repo
echo x > f && git add f && git commit -qm init && git status --short
```

## What we did

We used permission bits, links, an unlinked open file and a
full git workflow, all of which depend on details that a
careless filesystem gets wrong.

---

<small>Generated from `spec.posix.PosixCompatibility` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

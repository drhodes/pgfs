# Explanation: POSIX Compatibility

!!! success "Status: Stable"
    Modes, owners and nanosecond times stick, set-id bits are cleared on chown and non-owner writes, hard links, symlinks, special files, xattrs, open-after-unlink, renameat2 flags, exclusive create, sparse files with SEEK_DATA and SEEK_HOLE and the unsupported operations' errnos are built and tested (`tests/storage.rs`, `test_posix.sh`, the git workload). pjdfstest and xfstests runs belong to `verify.PosixSuites`.

> pgfs behaves like a local Linux filesystem for the operations that real tools use: permissions, ownership and times that stick, hard links, symlinks, xattrs, files that stay open after deletion, atomic rename variants and sparse files. What it does not support, it refuses with a stated error.

[Tutorial](../tutorials/posixcompatibility.md) · [How-to](../how-to/posixcompatibility.md) · [Reference](../reference/posixcompatibility.md) · **Explanation**

---

## Refuse, don't pretend

The most damaging filesystem bug is the silent one: a call
that succeeds and does nothing. `chmod +x` that returns 0
while the bit stays clear breaks git hooks, installers and
build scripts in ways that are hard to trace back. pgfs
therefore either implements an operation completely or
refuses it with a documented error.

## Why no atime

Updating the access time on every read turns every read into
a database write, with its WAL and vacuum cost. Linux
already defaults to `relatime`, and almost nothing depends
on atime. pgfs behaves as if every mount had `noatime`.

## Why locks are left to the kernel

With one read-write mount per tree, every process using the
tree goes through one kernel, which already implements
`flock` and POSIX locks exactly. Locks only become pgfs's
problem when several machines write one tree, which is the
multi-machine version's work.

---

<small>Generated from `spec.posix.PosixCompatibility` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

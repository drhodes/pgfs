# How-To: POSIX Compatibility

!!! success "Status: Stable"
    Modes, owners and nanosecond times stick, set-id bits are cleared on chown and non-owner writes, hard links, symlinks, special files, xattrs, open-after-unlink, renameat2 flags, exclusive create, sparse files with SEEK_DATA and SEEK_HOLE and the unsupported operations' errnos are built and tested (`tests/storage.rs`, `test_posix.sh`, the git workload). pjdfstest and xfstests runs belong to `verify.PosixSuites`.

> pgfs behaves like a local Linux filesystem for the operations that real tools use: permissions, ownership and times that stick, hard links, symlinks, xattrs, files that stay open after deletion, atomic rename variants and sparse files. What it does not support, it refuses with a stated error.

[Tutorial](../tutorials/posixcompatibility.md) · **How-to** · [Reference](../reference/posixcompatibility.md) · [Explanation](../explanation/posixcompatibility.md)

---

## Copy a file instantly

```bash
cp big.iso copy.iso
```

Coreutils 9 and later use `copy_file_range`, which pgfs
serves by sharing blocks, so no data is copied. pgfs cannot
support `cp --reflink=always`, because FUSE does not pass
`FICLONE` through. Use plain `cp`.

## Replace a file without losing it to a crash

Write a temporary file and rename it over the original. On
pgfs this is atomic even without fsync.

## Keep access times

pgfs never updates atime on read. Programs that need it, a
few mail readers for example, must set it explicitly with
`touch -a`.

---

<small>Generated from `spec.posix.PosixCompatibility` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

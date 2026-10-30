# Explanation: Durability and Crash Safety

!!! success "Status: Stable"
    fsync, fdatasync and `O_SYNC` writes commit synchronously and confirm replication with `pgfs.sync_barrier`; other operations commit asynchronously, renames commit pending writes first, close reports errors through `errseq_t`, shutdown flushes and barriers, the server's durability settings are checked, and scratch filesystems reset after a crash. `test_crash.sh` kills the daemon and Postgres; the LazyFS crash matrix is part of `verify.CrashConformance`.

> pgfs keeps the POSIX promise: data is durable when fsync returns. Everything else is fast and becomes durable in order, so after any crash the filesystem is an earlier moment of its history, and write-then-rename is atomic even without fsync.

[Tutorial](../tutorials/durablestorage.md) · [How-to](../how-to/durablestorage.md) · [Reference](../reference/durablestorage.md) · **Explanation**

---

## Two speeds

Waiting for the disk on every operation would make
metadata-heavy work several times slower, and almost no
program needs it. So pgfs does what local filesystems do:
ordinary operations return as soon as Postgres has them,
and the disk catches up within a fraction of a second. The
operations that ask for durability, fsync, O_SYNC and the
snapshot commands, wait for it.

## Order is the stronger promise

Postgres writes its log in order, and each pgfs operation is
one transaction. A crash therefore cuts history at a point:
everything before it survives, nothing after it does. Local
filesystems can reorder metadata and data, which is how
renamed files come back empty. pgfs commits a file's data
before any change to its name, so that cannot happen.

## Why a successful write is not enough

pgfs buffers small writes in the daemon to avoid rewriting a
block for every 4 KiB the kernel sends. A crash of the
daemon loses buffered writes, just as a kernel crash loses
dirty pages on ext4. Programs that need their data to
survive call fsync, and on pgfs fsync keeps its promise.

## Why server settings are checked

Postgres can be configured to lie about durability.
`fsync = off` skips disk flushes, and
`full_page_writes = off` lets a torn page survive restart.
Research on Postgres under fault injection found that the
second setting passes recovery yet leaves corrupted indexes.
pgfs refuses to promise durability on top of a server that
cannot keep it.

---

<small>Generated from `spec.durability.DurableStorage` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

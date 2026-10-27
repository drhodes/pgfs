# Explanation: Replicated Storage

!!! warning "Status: Partially implemented"
    Write routing, lag detection, transparent fallback and the Docker standby work today. File contents (`read`) are still always served by the primary, and a standby that is unreachable when the daemon starts is dropped for the daemon's lifetime without a warning.

> A pgfs mount can be backed by a primary Postgres and a physical streaming standby in Docker. Writes go to the primary; reads are served from whichever node is authoritative at that instant, falling back to the primary when the standby lags. The filesystem is eventually consistent with the standby's lag continuously bounded by the freshness check, and a standby failure never causes data loss or a kernel-visible error.

[Tutorial](../tutorials/replicated_filesystem.md) · [How-to](../how-to/replicated_filesystem.md) · [Reference](../reference/replicated_filesystem.md) · **Explanation**

---

## Why a physical standby

PostgreSQL's physical streaming replication ships the primary's
write-ahead log byte for byte, and the standby replays it. The
standby is therefore an exact copy of the whole cluster: same
tables, same rows, same schema, with no extra setup and no
schema to keep in sync. That is why pgfs never initialises the
schema on the standby. Nothing it could create would be
different from what replication already delivers.

## Reads can go anywhere, decisions cannot

A standby is always slightly behind. Serving a stale
directory listing is a small, recoverable inaccuracy. Making a
write decision from stale data is a correctness bug. If
`create` checked a stale standby for an existing file, it could
decide the name was free when it was not.

So pgfs draws the line between answering and deciding. Pure
metadata reads may use the standby. Every mutation, and every
read whose result decides what a mutation does, goes to the
primary. A lagging standby can therefore make the mount look
briefly out of date, but it can never make it write the wrong
thing.

## Measuring lag instead of assuming it

Before using the standby, pgfs compares two positions in the
write-ahead log: how far the primary has written, and how far
the standby has replayed. If the standby has replayed at least
as far, it holds everything the primary holds, and reading from
it is indistinguishable from reading the primary.

That check costs three small queries. Running it before every
read would cost more than the reads it offloads, so the verdict
is cached for half a second. This is the one place where pgfs
trades strictness for speed. Inside that window, a read can see
the standby's slightly older state.

## Failing towards availability

When the standby is stale, unreachable or broken, pgfs reads
from the primary and says so once per minute in the log. The
programs using the mount never see an error. A replicated mount
degrades into a single-node mount, which is still correct and
only less scalable.

## What replication does not give you

The standby offloads reads; it does not protect writes. If the
primary fails, the mount fails. pgfs does not promote the
standby or redirect writes, and a promoted standby is a new
primary that pgfs would have to be restarted against. High
availability for writes is a property of the PostgreSQL
deployment underneath, not of pgfs.

## Why Docker, and why host networking

The standby needs its own PostgreSQL instance with its own data
directory and port. A container provides that without a second
installation, and the named volume survives container restarts.
Host networking lets the container reach the primary on
loopback, so the primary never has to listen on a routable
interface.

---

<small>Generated from `spec.replica.ReplicatedFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

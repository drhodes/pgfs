"""
Optimization requirements for pgfs derived from design/chats/chat-optimize-1.txt.

These granular, single-responsibility Requirement classes formalize the
prioritized performance roadmap: Tier 1 database path optimizations, Tier 2
concurrency & buffering improvements, and Tier 3 kernel/FUSE layer tuning.
"""

from .err import Feat, Req


# ── Tier 1 — Database Path Optimizations ──────────────────────────────

class ChunkedBlockStorageReq(Req):
    """Whole-file read-modify-write is replaced by fixed-size block storage
    in a `blocks` table (keyed by parent, name, block_no). `read` fetches
    only touched blocks, `write` updates only modified blocks, and `setattr`
    truncation prunes blocks past the target size. `entries` retains `size`
    and `mtime` metadata as source of truth."""
    pass


class ReplicaFreshnessCacheReq(Req):
    """When running with `--replica`, the replica freshness check
    (`replica_fresh`) caches its health/LSN verdict with a short TTL
    (e.g., 200ms–1s). Consecutive read operations reuse the cached verdict
    to eliminate per-read 3-query validation overhead."""
    pass


class PreparedStatementReq(Req):
    """All static SQL queries (getattr, read, write, create, mkdir, unlink,
    rmdir, list, rename) are pre-compiled as prepared statements upon
    database connection initialization, avoiding re-parsing and re-planning
    overhead on every query execution."""
    pass


class SynchronousCommitTuningReq(Req):
    """The database layer supports connection-level tuning of
    `synchronous_commit` (e.g. `SET synchronous_commit = off`) to allow
    trade-offs between per-operation WAL fsync latency and crash durability
    window."""
    pass


class UnloggedTableOptReq(Req):
    """The database schema supports creating `entries` and `blocks` as
    `UNLOGGED` tables for high-performance development and testing workloads
    where WAL logging overhead is unneeded."""
    pass


class TransactionBatchingReq(Req):
    """FUSE operations that execute multiple sequential database queries
    (e.g., truncate-and-reread in `setattr`) wrap them in a single database
    transaction to reduce round-trips and WAL commit fsyncs."""
    pass


# ── Tier 2 — Concurrency & Buffering ──────────────────────────────────

class MultiThreadedConnectionPoolReq(Req):
    """The single-threaded `&mut self` database handle is replaced by a
    thread-safe database connection pool, combined with multi-worker FUSE
    session dispatch to process parallel file and directory operations
    concurrently."""
    pass


class BufferedWriteFlushReq(Req):
    """Small or non-aligned FUSE `write` calls buffer dirty byte ranges
    or blocks in memory per open file handle, flushing pending writes
    to PostgreSQL on `flush`, `release`, or explicit `fsync`."""
    pass


# ── Tier 3 — FUSE & Kernel Layer ──────────────────────────────────────

class KernelAttrCacheTtlReq(Req):
    """The kernel attribute and entry cache TTL is configurable via a CLI
    option (`--attr-ttl`), reducing repetitive `lookup` and `getattr`
    FUSE callbacks on metadata-heavy workloads."""
    pass


class KernelWritebackCacheReq(Req):
    """FUSE callback initialization negotiates kernel capabilities including
    `FUSE_WRITEBACK_CACHE` and splice read/write where supported by the kernel
    and `fuser` binding, allowing the kernel page cache to coalesce small
    writes."""
    pass


class IoUringTransportReq(Req):
    """Where supported by kernel and FUSE bindings, pgfs supports an io_uring
    mount transport to eliminate syscall overhead for high-IOPS FUSE operations."""
    pass


# ── Features ──────────────────────────────────────────────────────────

class OptimizationRoadmap(Feat):
    """pgfs incorporates a structured performance optimization pipeline
    targeting database access latency, connection concurrency, userspace
    buffering, and FUSE kernel-transport tuning."""
    pass

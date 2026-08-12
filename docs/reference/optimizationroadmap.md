# Reference: Performance Optimization Pipeline

**Feature**: `spec.optimize.OptimizationRoadmap`

> pgfs incorporates a structured performance optimization pipeline
targeting database access latency, connection concurrency, userspace
buffering, and FUSE kernel-transport tuning.

---

OptimizationRoadmap Requirements & Parameters:
- Schema: Chunked block storage (`ChunkedBlockStorageReq`) + integer inode primary keys (`InoBasedRekeyingReq`).
- Database Tuning: `PreparedStatementReq`, `TransactionBatchingReq`, `SynchronousCommitTuningReq`, `UnloggedTableOptReq`.
- Transport Tuning: `KernelAttrCacheTtlReq` (`--attr-ttl`), `KernelWritebackCacheReq` (`FUSE_WRITEBACK_CACHE`), `BufferedWriteFlushReq`.
- Sparse Data Safety: `SparseWritePaddingReq` (pre-zero padding on uninstantiated block writes).

# How-To: Performance Optimization Pipeline

**Feature**: `spec.optimize.OptimizationRoadmap`

> pgfs incorporates a structured performance optimization pipeline
targeting database access latency, connection concurrency, userspace
buffering, and FUSE kernel-transport tuning.

---

To tune pgfs performance for high IOPS workloads:
- Enable 64KB/128KB chunked block storage (`blocks(ino, block_no, data)`) to prevent full-file blob rewrites.
- Use prepared SQL statements (`PreparedStatementReq`) to eliminate query parsing overhead on hot paths.
- Enable synchronous commit tuning (`SET synchronous_commit = off`) to reduce WAL fsync wait times.
- Set `--attr-ttl <seconds>` to cache inode attributes in Linux VFS and avoid repetitive `getattr` round-trips.

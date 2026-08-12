# Reference: Database-Native Kernel Acceleration

**Feature**: `spec.offload.KernelOffloadingAcceleration`

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing
copy_file_range, fallocate, reflink CoW, and server-side search to eliminate
userspace byte streaming and deliver sub-millisecond execution for large-scale
file operations.

---

KernelOffloadingAcceleration Callbacks & Requirements:
- `CopyFileRangeReq`: Executes `INSERT INTO blocks SELECT ...` inside PostgreSQL.
- `FallocatePunchHoleReq`: Deletes block rows `DELETE FROM blocks WHERE ino = $1 AND block_no BETWEEN $2 AND $3`.
- `ReflinkBlockSharingReq`: Reference-counted Copy-on-Write (`ref_count` increment on reflink, lazy copy on write).
- `CloneSubtreeReq`: Recursive single-transaction set-based subtree copy (`INSERT INTO entries SELECT ...`).
- `ContentSearchOffloadReq`: PostgreSQL GIN & `pg_trgm` index search pushdown.

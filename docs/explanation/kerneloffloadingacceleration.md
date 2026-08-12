# Explanation: Database-Native Kernel Acceleration

**Feature**: `spec.offload.KernelOffloadingAcceleration`

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing
copy_file_range, fallocate, reflink CoW, and server-side search to eliminate
userspace byte streaming and deliver sub-millisecond execution for large-scale
file operations.

---

KernelOffloadingAcceleration moves file manipulation operations into PostgreSQL's query engine.
Instead of reading bytes from disk into FUSE daemon memory and writing them back over socket buffers,
pgfs intercepts Linux VFS acceleration callbacks (`copy_file_range`, `fallocate`) and translates them into
set-based database transactions, executing multi-gigabyte operations in sub-millisecond time.

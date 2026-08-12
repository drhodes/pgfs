# How-To: Database-Native Kernel Acceleration

**Feature**: `spec.offload.KernelOffloadingAcceleration`

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing
copy_file_range, fallocate, reflink CoW, and server-side search to eliminate
userspace byte streaming and deliver sub-millisecond execution for large-scale
file operations.

---

To leverage database-native kernel offloading:
- Use standard `copy_file_range(2)` or `cp --reflink=always` for zero-copy file duplication.
- Use `fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, offset, len)` to deallocate block ranges.
- Execute content searches via GIN/trigram index queries rather than scanning entire files across FUSE.

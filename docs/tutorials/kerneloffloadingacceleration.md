# Tutorial: Database-Native Kernel Acceleration

**Feature**: `spec.offload.KernelOffloadingAcceleration`

> pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing
copy_file_range, fallocate, reflink CoW, and server-side search to eliminate
userspace byte streaming and deliver sub-millisecond execution for large-scale
file operations.

---

In this tutorial, we will use kernel offloading features in pgfs to clone files and directories instantly.
1. Create a large source file: `head -c 100M /dev/urandom > ./mnt/large.bin`.
2. Perform an instant copy using `cp --reflink=always ./mnt/large.bin ./mnt/clone.bin`.
3. Notice that the copy completes instantaneously (<5ms) because PostgreSQL copied block reference IDs.
4. Punch a sparse hole using `fallocate -p -o 1M -l 10M ./mnt/clone.bin`.
5. Verify hole punching freed space in PostgreSQL without re-writing the rest of the file.

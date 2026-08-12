# Explanation: Reliable Mount & Lifecycle Management

**Feature**: `spec.app.ReliableMount`

> The filesystem mounts and unmounts cleanly. Crashing or killing
the daemon never leaves a ghost mount behind.

---

ReliableMount combines FUSE `AutoUnmount` kernel flags with dedicated signal handling threads
to guarantee that process exits, panics, or external termination signals always trigger a clean
kernel unmount, preventing dangling mount points and stale state.

# Reference: Reliable Mount & Lifecycle Management

**Feature**: `spec.app.ReliableMount`

> The filesystem mounts and unmounts cleanly. Crashing or killing
the daemon never leaves a ghost mount behind.

---

ReliableMount Lifecycle Specification:
- Mount flags: `AutoUnmount` enabled.
- Signal management: Dedicated `sigwait` thread handles SIGINT, SIGTERM, SIGHUP.
- Control files: `/tmp/pgfs-{hash}.pid` (daemon PID), `/tmp/pgfs-{hash}.ready` (readiness marker).
- Panic recovery: `panic = "abort"` in release builds ensures instant exit and kernel mount cleanup.

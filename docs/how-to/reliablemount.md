# How-To: Reliable Mount & Lifecycle Management

**Feature**: `spec.app.ReliableMount`

> The filesystem mounts and unmounts cleanly. Crashing or killing
the daemon never leaves a ghost mount behind.

---

To ensure reliable mounting and clean shutdown:
- Mount with `AutoUnmount` flag enabled in FUSE session options.
- Use SIGINT, SIGTERM, or SIGHUP to trigger explicit graceful unmount.
- Poll `/tmp/pgfs-*.ready` to check daemon readiness before performing file I/O.

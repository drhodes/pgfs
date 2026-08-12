# Tutorial: Reliable Mount & Lifecycle Management

**Feature**: `spec.app.ReliableMount`

> The filesystem mounts and unmounts cleanly. Crashing or killing
the daemon never leaves a ghost mount behind.

---

In this tutorial, we will verify clean daemon mount and shutdown behavior.
1. Mount pgfs using `scripts/pgfs.sh up`.
2. Verify mount point status via `scripts/pgfs.sh status` and check PID file at `/tmp/pgfs-*.pid`.
3. Send SIGINT to the daemon process or run `scripts/pgfs.sh down`.
4. Verify the mountpoint is cleanly unmounted and no stale daemon processes remain.

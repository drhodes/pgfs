# Explanation: Reliable Mount & Shutdown

!!! success "Status: Stable"
    AutoUnmount, signal-driven clean unmount, the deadlock watchdog, health files and the `pgfs.sh` escalation path are all implemented.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

[Tutorial](../tutorials/reliablemount.md) · [How-to](../how-to/reliablemount.md) · [Reference](../reference/reliablemount.md) · **Explanation**

---

## What a ghost mount is

A FUSE filesystem is a contract between the kernel and a user
process: the kernel forwards every operation on the mount to
the process and waits for an answer. If the process disappears
and nobody tells the kernel, the mount stays. Every access to
it fails with "Transport endpoint is not connected", and tools
that walk the filesystem tree, such as `df`, `find /` and
backup jobs, hit the dead mount and stall or fail. That is a
ghost mount, and avoiding them shaped how pgfs starts and
stops.

## Three layers, weakest to strongest

**The lifecycle script** treats `/proc/mounts` as the source
of truth rather than trusting PID files. If a mount exists
with no live daemon behind it, `pgfs.sh up` sweeps it before
mounting again. `pgfs.sh down` escalates from a polite signal
to `fusermount3 -u` to a lazy unmount, and on the way it names
the processes holding the mount busy, because "device busy"
with no explanation is the most common frustration.

**Signal handling** turns the polite signals into a clean
unmount. pgfs blocks the signals it cares about in every
thread and receives them with `sigwait` on one dedicated
thread. That thread can safely take locks and call the
unmount routine, which an asynchronous signal handler cannot.
The signals are blocked before anything else happens, so a
Ctrl+C during startup is queued rather than killing a
half-initialised daemon.

**AutoUnmount** is the layer that cannot fail. With it, the
kernel itself watches the daemon process and removes the mount
when the process exits for any reason: a clean return, a
panic, SIGKILL or the OOM killer. Every other layer is a
courtesy that produces a cleaner shutdown and clearer logs.
AutoUnmount is the guarantee.

## Why release builds abort on panic

A panic in a FUSE daemon usually means an invariant has broken.
Unwinding can leave the process alive in an inconsistent state
and still answering kernel requests. In release builds, pgfs
aborts instead. The panic hook writes a crash report first,
and AutoUnmount turns the abort into a clean disappearance.

## Why a deadlock unmounts the filesystem

A wedged daemon is worse than a dead one. The kernel keeps
waiting on it, and every process that touches the mount hangs
in uninterruptible sleep. The liveness heartbeat detects this:
a thread that cannot make progress stops advancing a counter,
and the next request to arrive replies with an error and
triggers an unmount. This converts a hang into an error, which
processes can handle.

## Health files

Supervisors need a cheap way to ask "is it up yet?" without
parsing logs. The PID file appears once the database is
reachable, and the readiness file appears once the mount
exists. Both are keyed by a hash of the mount point, so several
pgfs mounts on one machine do not collide.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

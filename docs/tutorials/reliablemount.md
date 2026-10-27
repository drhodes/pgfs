# Tutorial: Reliable Mount & Shutdown

!!! success "Status: Stable"
    AutoUnmount, signal-driven clean unmount, the deadlock watchdog, health files and the `pgfs.sh` escalation path are all implemented.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

**Tutorial** · [How-to](../how-to/reliablemount.md) · [Reference](../reference/reliablemount.md) · [Explanation](../explanation/reliablemount.md)

---

In this tutorial we will start pgfs, find its health files,
stop it politely, then kill it without warning and see that
the mount still disappears. Along the way we will see the three
layers that keep a mount from becoming a ghost.

## Step 1: start the daemon and find its health files

```bash
./scripts/pgfs.sh up
grep 'health pid file' testdata/pgfs.log | tail -1
```

```text
health pid file: /tmp/pgfs-5049eced64c2821d.pid
```

The hash in the name is derived from the mount point. Next to
the PID file is a readiness file:

```bash
ls /tmp/pgfs-*.pid /tmp/pgfs-*.ready
cat /tmp/pgfs-*.pid
```

The PID it contains matches `./scripts/pgfs.sh status`.

## Step 2: stop it politely

```bash
./scripts/pgfs.sh down
```

```text
pgfs:      stopping (pid 1519591)
unmounted:  /path/to/pgfs/testdata/mnt
```

The script sent SIGTERM. The daemon unmounted itself, removed
its health files and exited:

```bash
grep 'received signal' testdata/pgfs.log | tail -1
ls /tmp/pgfs-*.pid 2>/dev/null || echo "no health files"
```

## Step 3: kill it without warning

```bash
./scripts/pgfs.sh up
kill -9 "$(cat testdata/pgfs.pid)"
sleep 1
grep testdata/mnt /proc/mounts || echo "mount is gone"
```

```text
mount is gone
```

SIGKILL cannot be caught, so the daemon got no chance to clean
up. The kernel removed the mount anyway, because pgfs asked for
AutoUnmount when it mounted.

## Step 4: confirm the state is clean

```bash
./scripts/pgfs.sh status
```

```text
mount:     none
daemon:    not running
postgres:  running
```

Because the daemon was killed, its `/tmp` health files are
still there. They are harmless and are overwritten on the next
start.

## What we did

We saw a clean, signal-driven shutdown and an uncatchable kill,
and both ended with no mount left behind. The
[explanation](../explanation/reliablemount.md) describes why
each layer exists.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

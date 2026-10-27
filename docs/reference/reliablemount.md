# Reference: Reliable Mount & Shutdown

!!! success "Status: Stable"
    AutoUnmount, signal-driven clean unmount, the deadlock watchdog, health files and the `pgfs.sh` escalation path are all implemented.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

[Tutorial](../tutorials/reliablemount.md) · [How-to](../how-to/reliablemount.md) · **Reference** · [Explanation](../explanation/reliablemount.md)

---

## Mount options

| Option | Value |
|---|---|
| `fsname` | `pgfs` |
| `auto_unmount` | requested; if the mount fails with it, pgfs logs a WARN and retries without it |
| Kernel capability negotiated in `init` | `FUSE_BIG_WRITES` |

## Startup sequence

1. Initialise logging; install the panic hook.
2. Block SIGINT, SIGTERM, SIGHUP, SIGUSR1 and SIGUSR2 in every
   thread, so a signal that arrives during startup waits
   instead of killing the process.
3. Connect to PostgreSQL and create the schema if needed.
4. Write `/tmp/pgfs-<hash>.pid`; print and log its path.
5. Create the FUSE session (mount).
6. Write `/tmp/pgfs-<hash>.ready`.
7. Start the liveness, metrics, deadlock-watchdog and
   signal-waiter threads.
8. Serve FUSE requests until unmounted.

## Signals

| Signal | Effect |
|---|---|
| SIGINT, SIGTERM, SIGHUP | clean unmount; the daemon exits with status 0 |
| SIGUSR1 | state dump on the next FUSE callback (see [Observability](observabledaemon.md)) |
| SIGUSR2 | toggle CPU profiling (see [On-Demand Profiling](ondemandprofiling.md)) |
| SIGKILL, crash, OOM kill | the kernel removes the mount (AutoUnmount); health files remain |

Signals are received with `sigwait` on a dedicated
`signal-waiter` thread, never in an asynchronous handler.

## Health files

| File | Written | Contents | Removed |
|---|---|---|---|
| `/tmp/pgfs-<hash>.pid` | after the database connection succeeds | daemon PID | on clean exit |
| `/tmp/pgfs-<hash>.ready` | after the mount is created | `ready` | on clean exit |

`<hash>` is 16 hex digits computed from the mountpoint argument
exactly as given, so `./mnt` and `/abs/path/mnt` produce
different names.

## Deadlock watchdog

| Parameter | Value |
|---|---|
| Heartbeat period | 10 s |
| Stall threshold | 30 s without a heartbeat, observed by a FUSE callback |
| Action | ERROR log; the callback replies `EIO`; the watchdog unmounts |
| Unmount retries | 5, 500 ms apart; then the process exits with status 1 |

## Database connection lifetime

The daemon acquires one primary connection (and one standby
connection with `--replica`) at startup and keeps it until
exit. It does not reconnect. If PostgreSQL restarts or the
connection drops, every operation that reaches the database
replies `EIO` until the daemon is restarted. The mount itself
stays up and still unmounts cleanly.

## Panics

| Build | Behaviour |
|---|---|
| debug | the panic hook writes the crash report, then the panic unwinds |
| release | the panic hook writes the crash report, then the process aborts (`panic = "abort"`) and the kernel removes the mount |

## Exit status

| Status | Cause |
|---|---|
| 0 | clean unmount |
| 1 | startup failure (error chain printed as `pgfs: …`), or the deadlock watchdog gave up |


## Specification requirements

`ReliableMount` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "MountsCleanly"
    `spec.app.MountsCleanly`

    The daemon mounts a FUSE filesystem at the given mountpoint using
    AutoUnmount so the kernel removes the mount if the process exits
    for any reason. The mountpoint must be an existing, empty directory
    not already mounted.

??? abstract "HandlesShutdownSignals"
    `spec.app.HandlesShutdownSignals`

    SIGINT, SIGTERM, and SIGHUP trigger a clean unmount via
    `session.unmount()`. If the clean unmount fails, AutoUnmount
    guarantees the kernel still removes the mount. Signal handling
    uses a dedicated `sigwait` thread so it never blocks FUSE
    dispatch.

??? abstract "DetectsDeadlock"
    `spec.app.DetectsDeadlock`

    A liveness thread increments a counter every 10 s. Every FUSE
    callback checks it on entry; if the counter has not advanced for 30 s
    the callback logs ERROR, replies EIO and flags a watchdog thread. The
    watchdog unmounts cleanly, retrying up to 5 times 500 ms apart, and
    exits the process if every attempt fails so that AutoUnmount removes
    the mount. The check uses a monotonic clock, so wall-clock jumps never
    cause false positives.

??? abstract "WritesHealthFiles"
    `spec.app.WritesHealthFiles`

    After connecting to Postgres the daemon writes its PID to
    `/tmp/pgfs-<hash>.pid`, prints `health pid file: <path>` on stdout and
    logs it. After the FUSE session is created it writes
    `/tmp/pgfs-<hash>.ready` containing `ready`. `<hash>` is 16 hex digits
    derived from the mountpoint argument exactly as given. Both files are
    removed when `run()` returns.

??? abstract "ProvidesLifecycleScript"
    `spec.app.ProvidesLifecycleScript`

    `scripts/pgfs.sh up|down|status|run` owns the mount lifecycle.
    `up` starts Postgres if needed, runs `cargo build`, sweeps a stale mount
    whose daemon is gone, starts `target/debug/pgfs` detached with output
    appended to `testdata/pgfs.log`, and waits up to 10 s for the mount to
    appear in `/proc/mounts`. `down` sends SIGTERM, waits up to 10 s, then
    escalates to `fusermount3 -u`, reports the holders of a busy mount,
    and finally lazy-unmounts. `status` reports mount, daemon and Postgres
    state and flags ghost mounts. `run` builds and execs the daemon in the
    foreground. `PGFS_MOUNT` overrides the default `testdata/mnt`.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

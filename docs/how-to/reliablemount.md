# How-To: Reliable Mount & Shutdown

!!! success "Status: Stable"
    AutoUnmount, signal-driven clean unmount, the deadlock watchdog, health files and the `pgfs.sh` escalation path are all implemented.

> The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.

[Tutorial](../tutorials/reliablemount.md) · **How-to** · [Reference](../reference/reliablemount.md) · [Explanation](../explanation/reliablemount.md)

---

## Stop the daemon cleanly

```bash
./scripts/pgfs.sh down
```

Or signal it directly; SIGINT, SIGTERM and SIGHUP all unmount
cleanly:

```bash
kill -TERM "$(cat testdata/pgfs.pid)"
```

## Wait for the mount to be ready (supervisors, scripts)

Poll for the readiness file. Its name is printed at startup
next to the PID file:

```bash
until [ -e /tmp/pgfs-<hash>.ready ]; do sleep 0.1; done
```

Or poll `/proc/mounts` for the mount point, which is what
`pgfs.sh up` does.

## Fix "Device or resource busy" on unmount

A process has its working directory or an open file inside the
mount. `pgfs.sh down` lists the holders and then lazy-unmounts.
To find them yourself:

```bash
fuser -vm testdata/mnt
```

## Fix "Transport endpoint is not connected"

The daemon died and the mount was not removed, which happens
only if it was started without AutoUnmount (see the
`user_allow_other` warning below). Sweep it:

```bash
./scripts/pgfs.sh down
```

or `fusermount3 -u -z testdata/mnt`.

## Make AutoUnmount available

If the log shows
`mount with AutoUnmount failed (...); retrying without AutoUnmount`,
enable `user_allow_other`:

```bash
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

## Run pgfs under systemd

Use `Type=simple`, start the binary directly with an explicit
`--conn`, and let systemd stop it with SIGTERM:

```ini
[Service]
Type=simple
ExecStart=/opt/pgfs/pgfs --conn "host=/run/postgresql dbname=pgfs" /srv/pgfs
KillSignal=SIGTERM
Restart=on-failure
```

Services that need the mount should wait for the `.ready`
file, or for the mount point to appear in `/proc/mounts`.

---

<small>Generated from `spec.app.ReliableMount` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

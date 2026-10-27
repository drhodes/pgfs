# Explanation: Project-Local Development Environment

!!! success "Status: Stable"
    Everything on this page ships today: `scripts/init_db.sh`, `scripts/pgfs.sh`, the Makefile targets and the integration suite in `scripts/test_*.sh`.

> A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.

[Tutorial](../tutorials/projectlocalenvironment.md) · [How-to](../how-to/projectlocalenvironment.md) · [Reference](../reference/projectlocalenvironment.md) · **Explanation**

---

## Why everything lives in `testdata/`

A filesystem backed by a database needs a database, and the
usual way to get one is to install a system PostgreSQL service
and administer it. pgfs deliberately avoids that. The cluster,
its socket, its logs, the mount point and the daemon's
bookkeeping all live in one git-ignored directory inside the
checkout.

That buys three things. Nothing on the machine changes outside
the project, so it is safe to experiment. Two checkouts never
interfere with each other. And recovery from any mess is one
command, `rm -rf testdata`, because there is no state anywhere
else.

## Trust authentication, and its limits

The cluster uses `trust` authentication: any connection that
reaches it is accepted without a password. The Unix socket sits
in a directory only the developer can enter, so for socket
connections that is no weaker than file permissions. The TCP
listener is different. It only binds 127.0.0.1, but any user on
the same machine can reach loopback. On a single-user
development machine that is acceptable. On a shared host it is
not, and the cluster should not be run there as is.

TCP exists at all because physical replication needs it:
`pg_basebackup` and the standby's WAL receiver connect to the
primary over the network (see [Replicated
Storage](../explanation/replicated_filesystem.md)).

## Why a lifecycle script instead of raw commands

A FUSE mount has more states than "up" and "down". The daemon
can be dead while the kernel still shows the mount. The mount
can be busy because a shell has its working directory inside
it. Postgres can be stopped while the daemon is starting.
`pgfs.sh` treats `/proc/mounts` as the source of truth and
escalates through progressively stronger actions (signal, then
`fusermount3 -u`, then a lazy unmount), so the common failure
states resolve themselves. The reasoning behind clean unmounts
is covered in [Reliable Mount](../explanation/reliablemount.md).

`pgfs.sh` builds and runs the debug binary. Compiles are faster,
and in debug builds a panic unwinds and prints a full report
rather than aborting.

## Why the Makefile insists on the Nix shell

pgfs links against libfuse and talks to a specific PostgreSQL
major version. A build against the wrong libfuse headers, or an
`initdb` from a different major version than the server, fails
in confusing ways. The `check-nix` guard turns that into one
clear message with a fix.

## Where the documentation comes from

These pages are generated. Every user-visible capability of
pgfs is a `Feature` class in `spec/` that inherits
`libspec.diataxis.Diataxis` and must implement four methods,
one per documentation quadrant. `scripts/build_docs.py` renders
them, together with each feature's requirement classes, into
this site. Documentation that lives next to the specification
is reviewed in the same change as the behaviour it describes.

---

<small>Generated from `spec.app.ProjectLocalEnvironment` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

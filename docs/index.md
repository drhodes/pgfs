# pgfs: a PostgreSQL-backed FUSE filesystem

pgfs mounts a directory tree whose files and directories are rows in a
PostgreSQL database. Ordinary programs (`cp`, `git`, editors, build tools)
read and write it like any local directory; SQL clients see the same data
as tables.

```bash
nix develop
./scripts/pgfs.sh up                          # PostgreSQL + build + mount at testdata/mnt
echo hello > testdata/mnt/hello.txt
psql -h "$PWD/testdata" -d pgfs -c "SELECT parent, name, size FROM entries"
./scripts/pgfs.sh down
```

## How these pages are organised

The documentation follows [Diátaxis](https://diataxis.fr). Every feature is
described four ways; pick the one that matches what you are doing.

| If you want to… | Read the | It is |
|---|---|---|
| learn by doing, step by step | **Tutorial** | a guided lesson with a visible result |
| get a specific task done | **How-to guide** | a recipe for someone who knows the basics |
| look up an exact fact | **Reference** | complete, terse description: options, schema, errors |
| understand why it works this way | **Explanation** | background, design reasoning and trade-offs |

## Features

| Feature | Status | | | | |
|---|---|---|---|---|---|
| **Project-Local Development Environment** | Stable | [Tutorial](tutorials/projectlocalenvironment.md) | [How-to](how-to/projectlocalenvironment.md) | [Reference](reference/projectlocalenvironment.md) | [Explanation](explanation/projectlocalenvironment.md) |
| **Regular Files** | Stable | [Tutorial](tutorials/flat_filesystem.md) | [How-to](how-to/flat_filesystem.md) | [Reference](reference/flat_filesystem.md) | [Explanation](explanation/flat_filesystem.md) |
| **Directory Trees** | Stable | [Tutorial](tutorials/hierarchicaldirectories.md) | [How-to](how-to/hierarchicaldirectories.md) | [Reference](reference/hierarchicaldirectories.md) | [Explanation](explanation/hierarchicaldirectories.md) |
| **Atomic Rename** | Stable | [Tutorial](tutorials/atomicrename.md) | [How-to](how-to/atomicrename.md) | [Reference](reference/atomicrename.md) | [Explanation](explanation/atomicrename.md) |
| **Reliable Mount & Shutdown** | Stable | [Tutorial](tutorials/reliablemount.md) | [How-to](how-to/reliablemount.md) | [Reference](reference/reliablemount.md) | [Explanation](explanation/reliablemount.md) |
| **Observability & Diagnostics** | Stable | [Tutorial](tutorials/observabledaemon.md) | [How-to](how-to/observabledaemon.md) | [Reference](reference/observabledaemon.md) | [Explanation](explanation/observabledaemon.md) |
| **On-Demand Profiling** | Stable | [Tutorial](tutorials/ondemandprofiling.md) | [How-to](how-to/ondemandprofiling.md) | [Reference](reference/ondemandprofiling.md) | [Explanation](explanation/ondemandprofiling.md) |
| **Replicated Storage** | Partially implemented | [Tutorial](tutorials/replicated_filesystem.md) | [How-to](how-to/replicated_filesystem.md) | [Reference](reference/replicated_filesystem.md) | [Explanation](explanation/replicated_filesystem.md) |
| **Performance Optimizations** | Partially implemented | [Tutorial](tutorials/optimizationroadmap.md) | [How-to](how-to/optimizationroadmap.md) | [Reference](reference/optimizationroadmap.md) | [Explanation](explanation/optimizationroadmap.md) |
| **Kernel Offloading** | Partially implemented | [Tutorial](tutorials/kerneloffloadingacceleration.md) | [How-to](how-to/kerneloffloadingacceleration.md) | [Reference](reference/kerneloffloadingacceleration.md) | [Explanation](explanation/kerneloffloadingacceleration.md) |
| **Dual-Interface Architecture** | Planned, not implemented | [Tutorial](tutorials/dualinterfacearchitecture.md) | [How-to](how-to/dualinterfacearchitecture.md) | [Reference](reference/dualinterfacearchitecture.md) | [Explanation](explanation/dualinterfacearchitecture.md) |
| **Sharded Filesystem** | Planned, not implemented | [Tutorial](tutorials/sharded_filesystem.md) | [How-to](how-to/sharded_filesystem.md) | [Reference](reference/sharded_filesystem.md) | [Explanation](explanation/sharded_filesystem.md) |

**Stable** features are implemented and tested. **Partially implemented**
and **Planned** pages say on every page what works today; for planned
features, the tutorials and how-to guides describe the intended interface.

## Where to start

1. [Your first pgfs mount](tutorials/projectlocalenvironment.md) gets a mount running in five minutes.
2. [Regular Files](tutorials/flat_filesystem.md), [Directory Trees](tutorials/hierarchicaldirectories.md)
   and [Atomic Rename](tutorials/atomicrename.md) cover everyday use.
3. [Reliable Mount](how-to/reliablemount.md) and [Observability](how-to/observabledaemon.md)
   are for running pgfs and diagnosing it.
4. [Regular Files: Explanation](explanation/flat_filesystem.md) explains the storage model
   everything else builds on.

## Feature summaries

- **Project-Local Development Environment**: A contributor goes from a fresh clone to a live pgfs mount with one command. The Postgres cluster, the mount point, the logs and the pid files all live under the project's `testdata/` directory, so nothing touches a system-wide Postgres and `rm -rf testdata` is a full reset.
- **Regular Files**: Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.
- **Directory Trees**: Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.
- **Atomic Rename**: Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.
- **Reliable Mount & Shutdown**: The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.
- **Observability & Diagnostics**: Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.
- **On-Demand Profiling**: Operators can profile a running pgfs daemon without restarting it: SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a span trace of every FUSE callback and database call is written on shutdown. All of it is compiled in only with the `profiling` feature.
- **Replicated Storage**: A pgfs mount can be backed by a primary Postgres and a physical streaming standby in Docker. Writes go to the primary; reads are served from whichever node is authoritative at that instant, falling back to the primary when the standby lags. The filesystem is eventually consistent with the standby's lag continuously bounded by the freshness check, and a standby failure never causes data loss or a kernel-visible error.
- **Performance Optimizations**: pgfs incorporates a structured performance optimization pipeline targeting database access latency, connection concurrency, userspace buffering, and FUSE kernel-transport tuning.
- **Kernel Offloading**: pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing copy_file_range, fallocate, reflink CoW, and server-side search to eliminate userspace byte streaming and deliver sub-millisecond execution for large-scale file operations.
- **Dual-Interface Architecture**: pgfs features a Dual-Interface Storage Architecture providing a sub-100µs Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy) and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).
- **Sharded Filesystem**: A pgfs mount can be backed by N independent Postgres host nodes with RAID-5-like subset replication: each directory subtree is pinned to a replication set of R of the N hosts (never a full mirror on every host). Writes fan out to every member of the set; reads consult a quorum and return the newest version. Any number of clients can mount the same namespace from different machines and add files to one shared tree, and a host failure degrades reads and writes to the remaining healthy members without stale or corrupt data.

---

<small>Generated from the `libspec.diataxis` Feature classes in `spec/` by
`scripts/build_docs.py`. Edit the specification, not these pages.</small>

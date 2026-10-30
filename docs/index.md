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
| **Project-Local Development Environment** | Partially implemented | [Tutorial](tutorials/projectlocalenvironment.md) | [How-to](how-to/projectlocalenvironment.md) | [Reference](reference/projectlocalenvironment.md) | [Explanation](explanation/projectlocalenvironment.md) |
| **Administering pgfs** | Stable | [Tutorial](tutorials/filesystemlifecycle.md) | [How-to](how-to/filesystemlifecycle.md) | [Reference](reference/filesystemlifecycle.md) | [Explanation](explanation/filesystemlifecycle.md) |
| **Regular Files** | Stable | [Tutorial](tutorials/flat_filesystem.md) | [How-to](how-to/flat_filesystem.md) | [Reference](reference/flat_filesystem.md) | [Explanation](explanation/flat_filesystem.md) |
| **Directory Trees** | Stable | [Tutorial](tutorials/hierarchicaldirectories.md) | [How-to](how-to/hierarchicaldirectories.md) | [Reference](reference/hierarchicaldirectories.md) | [Explanation](explanation/hierarchicaldirectories.md) |
| **Atomic Rename** | Stable | [Tutorial](tutorials/atomicrename.md) | [How-to](how-to/atomicrename.md) | [Reference](reference/atomicrename.md) | [Explanation](explanation/atomicrename.md) |
| **POSIX Compatibility** | Stable | [Tutorial](tutorials/posixcompatibility.md) | [How-to](how-to/posixcompatibility.md) | [Reference](reference/posixcompatibility.md) | [Explanation](explanation/posixcompatibility.md) |
| **Branchable Trees** | Stable | [Tutorial](tutorials/branchabletrees.md) | [How-to](how-to/branchabletrees.md) | [Reference](reference/branchabletrees.md) | [Explanation](explanation/branchabletrees.md) |
| **Change Feed** | Stable | [Tutorial](tutorials/changefeed.md) | [How-to](how-to/changefeed.md) | [Reference](reference/changefeed.md) | [Explanation](explanation/changefeed.md) |
| **Durability and Crash Safety** | Stable | [Tutorial](tutorials/durablestorage.md) | [How-to](how-to/durablestorage.md) | [Reference](reference/durablestorage.md) | [Explanation](explanation/durablestorage.md) |
| **Consistency and Mount Ownership** | Stable | [Tutorial](tutorials/consistentmounts.md) | [How-to](how-to/consistentmounts.md) | [Reference](reference/consistentmounts.md) | [Explanation](explanation/consistentmounts.md) |
| **Storage Layout** | Stable | [Tutorial](tutorials/storagelayout.md) | [How-to](how-to/storagelayout.md) | [Reference](reference/storagelayout.md) | [Explanation](explanation/storagelayout.md) |
| **Space Management** | Stable | [Tutorial](tutorials/spacemanagement.md) | [How-to](how-to/spacemanagement.md) | [Reference](reference/spacemanagement.md) | [Explanation](explanation/spacemanagement.md) |
| **Integrity and Backup** | Partially implemented | [Tutorial](tutorials/dataintegrity.md) | [How-to](how-to/dataintegrity.md) | [Reference](reference/dataintegrity.md) | [Explanation](explanation/dataintegrity.md) |
| **Access Control and Security** | Stable | [Tutorial](tutorials/accesscontrol.md) | [How-to](how-to/accesscontrol.md) | [Reference](reference/accesscontrol.md) | [Explanation](explanation/accesscontrol.md) |
| **Reliable Mount & Shutdown** | Partially implemented | [Tutorial](tutorials/reliablemount.md) | [How-to](how-to/reliablemount.md) | [Reference](reference/reliablemount.md) | [Explanation](explanation/reliablemount.md) |
| **Observability & Diagnostics** | Stable | [Tutorial](tutorials/observabledaemon.md) | [How-to](how-to/observabledaemon.md) | [Reference](reference/observabledaemon.md) | [Explanation](explanation/observabledaemon.md) |
| **On-Demand Profiling** | Stable | [Tutorial](tutorials/ondemandprofiling.md) | [How-to](how-to/ondemandprofiling.md) | [Reference](reference/ondemandprofiling.md) | [Explanation](explanation/ondemandprofiling.md) |
| **Conformance Testing** | Partially implemented | [Tutorial](tutorials/conformancetesting.md) | [How-to](how-to/conformancetesting.md) | [Reference](reference/conformancetesting.md) | [Explanation](explanation/conformancetesting.md) |
| **Replicated Storage** | Partially implemented | [Tutorial](tutorials/replicated_filesystem.md) | [How-to](how-to/replicated_filesystem.md) | [Reference](reference/replicated_filesystem.md) | [Explanation](explanation/replicated_filesystem.md) |
| **Performance Optimizations** | Partially implemented | [Tutorial](tutorials/optimizationroadmap.md) | [How-to](how-to/optimizationroadmap.md) | [Reference](reference/optimizationroadmap.md) | [Explanation](explanation/optimizationroadmap.md) |
| **Kernel Offloading** | Partially implemented | [Tutorial](tutorials/kerneloffloadingacceleration.md) | [How-to](how-to/kerneloffloadingacceleration.md) | [Reference](reference/kerneloffloadingacceleration.md) | [Explanation](explanation/kerneloffloadingacceleration.md) |
| **Dual-Interface Architecture** | Partially implemented | [Tutorial](tutorials/dualinterfacearchitecture.md) | [How-to](how-to/dualinterfacearchitecture.md) | [Reference](reference/dualinterfacearchitecture.md) | [Explanation](explanation/dualinterfacearchitecture.md) |
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
- **Administering pgfs**: One command does everything an administrator needs: format a database, mount trees and snapshots, upgrade the schema, tune capacity, and inspect, check and back up the filesystem. Nothing happens implicitly.
- **Regular Files**: Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.
- **Directory Trees**: Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.
- **Atomic Rename**: Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.
- **POSIX Compatibility**: pgfs behaves like a local Linux filesystem for the operations that real tools use: permissions, ownership and times that stick, hard links, symlinks, xattrs, files that stay open after deletion, atomic rename variants and sparse files. What it does not support, it refuses with a stated error.
- **Branchable Trees**: Freeze any pgfs tree in constant time, branch a writable copy of it in constant time, and atomically publish a branch as the new contents of a tree. Branches share every unchanged byte with their base and store only what they change.
- **Change Feed**: Every change to a tree is recorded in the same transaction that makes it, and delivered in causal order, exactly once, to consumers that can stop and resume. Snapshot comparison (`pgfs diff`, specified in Branchable Trees) complements it for changes older than the feed's retention.
- **Durability and Crash Safety**: pgfs keeps the POSIX promise: data is durable when fsync returns. Everything else is fast and becomes durable in order, so after any crash the filesystem is an earlier moment of its history, and write-then-rename is atomic even without fsync.
- **Consistency and Mount Ownership**: Within a mount, every operation sees every earlier one. Each tree has at most one read-write mount, which holds a lease that a stalled daemon cannot outlive. Every operation is one bounded, atomic transaction.
- **Storage Layout**: Every byte of a pgfs filesystem is a row in PostgreSQL: inodes, names, attributes and data blocks. Inode numbers are permanent, names are exact bytes, small files take one row, and file data is immutable, checksummed and shareable between files, snapshots and branches.
- **Space Management**: pgfs reports real capacity and usage, refuses writes when full, and reclaims space from deleted files, snapshots and trees in the background, with the backlog and the write amplification always visible.
- **Integrity and Backup**: Every block pgfs returns is checked against a checksum taken when it was written. `pgfs check` verifies the filesystem's invariants, `pgfs scrub` finds silent corruption before a reader does, and backups use Postgres's own tools or a portable tar export of a snapshot.
- **Access Control and Security**: Unix permissions on pgfs work as on a local disk, enforced by the kernel. The daemon reaches Postgres with the least privilege it needs, over TLS when the database is remote, and never logs a secret.
- **Reliable Mount & Shutdown**: The filesystem mounts and unmounts cleanly. Crashing or killing the daemon never leaves a ghost mount behind.
- **Observability & Diagnostics**: Operators can see what the daemon is doing, how fast it is doing it, and why something failed, from the log alone: structured spans for every operation, periodic metrics, an on-demand state dump, and self-contained error chains and crash reports.
- **On-Demand Profiling**: Operators can profile a running pgfs daemon without restarting it: SIGUSR2 starts and stops a CPU sampler that writes a flamegraph, and a span trace of every FUSE callback and database call is written on shutdown. All of it is compiled in only with the `profiling` feature.
- **Conformance Testing**: pgfs is tested against an executable model of its specification: generated operation sequences, crashes at every point, injected database faults, every supported configuration, and the standard POSIX test suites, with the invariant checker run after every test.
- **Replicated Storage**: A pgfs mount can be backed by a primary Postgres and physical streaming standbys. Writes go to the primary. A read goes to a standby only when the standby has replayed the mount's own latest commit, so the mount always reads its own writes. Snapshots can be read from any standby that has them. A standby failure never causes a kernel-visible error. A standby is not protection against corruption: physical replication copies a damaged page faithfully, which is what checksums and scrub are for.
- **Performance Optimizations**: The request path from the kernel to PostgreSQL is made fast where it costs most: concurrent dispatch sharded by inode, prepared statements, buffered writes and, later, the kernel's writeback cache. A fixed benchmark suite measures every change.
- **Kernel Offloading**: Copies and hole punching run inside PostgreSQL. `cp` shares blocks instead of copying bytes, and `fallocate` punches holes and zeroes ranges by changing block-map rows, so no file data passes through the daemon. Content search through database indexes is planned.
- **Dual-Interface Architecture**: Two storage engines sit behind one interface: a planned local engine that runs inside PostgreSQL and sends no SQL text on the hot path, and a planned remote engine that pipelines queries over the network. The FUSE layer talks only to the interface, so the engine is chosen at startup.
- **Sharded Filesystem**: A pgfs mount can be backed by N independent Postgres host nodes with RAID-5-like subset replication: each directory subtree is pinned to a replication set of R of the N hosts (never a full mirror on every host). Writes fan out to every member of the set; reads consult a quorum and return the newest version. Any number of clients can mount the same namespace from different machines and add files to one shared tree, and a host failure degrades reads and writes to the remaining healthy members without stale or corrupt data.

---

<small>Generated from the `libspec.diataxis` Feature classes in `spec/` by
`scripts/build_docs.py`. Edit the specification, not these pages.</small>

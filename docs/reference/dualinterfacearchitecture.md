# Reference: Dual-Interface Architecture

!!! danger "Status: Planned, not implemented"
    The `PgfsBackend` trait, its error mapping and an inode-keyed implementation on `Db` exist and are unit-tested, but the FUSE layer does not route through them yet. `RemoteBackend` is a stub, the `pgrx` engine and the `io_uring` transport are at the design stage, and there is no `--backend` option.

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy) and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

[Tutorial](../tutorials/dualinterfacearchitecture.md) · [How-to](../how-to/dualinterfacearchitecture.md) · **Reference** · [Explanation](../explanation/dualinterfacearchitecture.md)

---

## Component status

| Component | Status | Location |
|---|---|---|
| `PgfsBackend` trait | implemented, not used by the FUSE layer | `src/backend.rs` |
| `StorageError` → errno mapping | implemented | `src/backend.rs` |
| inode-keyed implementation on `Db` | implemented; exercised by unit tests only | `src/db.rs`, `backend_impl` |
| `LocalBackend` | wrapper that forwards to `Db`; not the planned `pgrx` engine | `src/backend.rs` |
| `RemoteBackend` | stub; every method returns an error | `src/backend.rs` |
| `pgrx` background-worker engine (`LocalPgrxEngineReq`) | planned | design only |
| `io_uring` buffer rings (`IoUringBufferRingReq`) | planned | design only |
| `tokio-postgres` network engine (`RemoteAsyncBackendReq`) | planned | design only |
| `--backend` command-line option | planned | |

## `PgfsBackend` methods

| Method | Signature (all `async`, all return `Result<_, StorageError>`) |
|---|---|
| `lookup` | `(parent_ino: u64, name: &str) -> Option<FileAttr>` |
| `getattr` | `(ino: u64) -> FileAttr` |
| `setattr` | `(ino: u64, attr: SetAttrInput) -> FileAttr` |
| `readdir` | `(ino: u64, offset: i64) -> Vec<DirEntry>` |
| `mkdir` | `(parent_ino: u64, name: &str, mode: u32, uid: u32, gid: u32) -> FileAttr` |
| `rmdir` | `(parent_ino: u64, name: &str) -> ()` |
| `create` | `(parent_ino: u64, name: &str, mode: u32, uid: u32, gid: u32) -> FileAttr` |
| `unlink` | `(parent_ino: u64, name: &str) -> ()` |
| `rename` | `(old_parent: u64, old_name: &str, new_parent: u64, new_name: &str) -> ()` |
| `read_block` | `(ino: u64, block_no: u32) -> Vec<u8>` |
| `write_block` | `(ino: u64, block_no: u32, data: &[u8], offset_in_block: u32) -> u32` |
| `truncate_blocks` | `(ino: u64, new_size: u64) -> ()` |
| `copy_file_range` | `(ino_in, off_in, ino_out, off_out, len: u64) -> u64`; default: `InvalidArgument` |
| `fallocate` | `(ino: u64, offset: u64, length: u64, mode: u32) -> ()`; default: `InvalidArgument` |

## `StorageError`

| Variant | errno |
|---|---|
| `NotFound` | `ENOENT` |
| `AlreadyExists` | `EEXIST` |
| `NotEmpty` | `ENOTEMPTY` |
| `InvalidArgument(String)` | `EINVAL` |
| `DatabaseError(String)` | `EIO` |
| `IoError(kind)` | `ENOENT`, `EEXIST`, `EACCES` or `EINVAL` for the matching kinds; otherwise `EIO` |

## Data types

| Type | Fields |
|---|---|
| `FileKind` | `File`, `Directory`, `Symlink` |
| `FileAttr` | `ino`, `size`, `blocks`, `atime`, `mtime`, `ctime`, `kind`, `perm`, `nlink`, `uid`, `gid`, `rdev`, `flags` |
| `DirEntry` | `ino`, `name`, `kind` |
| `SetAttrInput` | optional `mode`, `uid`, `gid`, `size`, `atime`, `mtime` |

## Compatibility with the running FUSE path

| Property | FUSE path (`Db` path methods) | Backend path (`Db` as `PgfsBackend`) |
|---|---|---|
| Key | `(parent, name)` text | `ino`, `parent_ino` integers |
| Block size | 65 536 bytes | 131 072 bytes |
| `entries.parent_ino` written | always 1 | the real parent |
| `blocks.ino` written | always 1 | the real inode |

Rows written by one path are not correctly visible through the
other, so the two cannot yet serve the same database.


## Specification requirements

`DualInterfaceArchitecture` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "PgfsBackendTraitReq"
    `spec.optimize.PgfsBackendTraitReq`

    A unified async `PgfsBackend` trait decouples the FUSE driver
    (`src/fs.rs`) from storage engine implementations.

    Technical Details:

    - Exposes async methods: `lookup`, `getattr`, `setattr`, `readdir`, `mkdir`, `rmdir`,
      `create`, `unlink`, `rename`, `read_block`, `write_block`, `truncate_blocks`.
    - Maps database failures into structured `StorageError` variants (`NotFound`, `AlreadyExists`,
      `NotEmpty`, `InvalidArgument`, `DatabaseError`, `IoError`).
    - Enables pluggable backend switching (`LocalBackend` vs `RemoteBackend`) at daemon startup.

??? abstract "LocalPgrxEngineReq"
    `spec.optimize.LocalPgrxEngineReq`

    The local storage engine (`LocalBackend`) operates inside PostgreSQL as
    a `pgrx` background worker (`pg_bgworker`) using pre-compiled SPI plans and binary Datums
    to achieve Zero SQL Text Parsing on the hot path.

    Technical Details:

    - Runs inside PostgreSQL postmaster via `RegisterBackgroundWorker`.
    - Pre-compiles statement handles (`SPIPlanPtr`) during worker startup using `SPI_prepare`.
    - Receives binary operation descriptors over a lockless shared-memory ring buffer (`shm_mq`)
      from the standalone FUSE daemon.
    - Executes `SPI_execute_plan` using raw binary `Datum` values (`Int64GetDatum`, `Int32GetDatum`),
      bypassing SQL text formatting, parsing, and planning entirely.

??? abstract "IoUringBufferRingReq"
    `spec.optimize.IoUringBufferRingReq`

    Where supported by kernel (Linux 6.9+ `fuse-io_uring`), pgfs utilizes
    `io_uring` Submission/Completion queues paired with pre-registered shared
    memory buffer rings (`IORING_OP_PROVIDE_BUFFERS`) for zero-copy data flow.

    Technical Details:

    - Replaces `/dev/fuse` `read()`/`write()` syscalls with asynchronous `io_uring` SQ/CQ rings.
    - Maps POSIX shared memory buffers (`shm_mq`) as pre-registered `io_uring` buffer rings.
    - Kernel VFS places incoming file block payloads directly into shared memory buffers accessible by
      the `pgrx` worker, enabling end-to-end zero-copy data flow into PostgreSQL `SharedBuffers`.

??? abstract "RemoteAsyncBackendReq"
    `spec.optimize.RemoteAsyncBackendReq`

    The remote storage engine (`RemoteBackend`) operates out-of-process using
    `fuse3` and `tokio-postgres` over TCP or Unix sockets.

    Technical Details:

    - Uses `fuse3` async FUSE bindings and `tokio-postgres` connection pools (`bb8`).
    - Implements query pipelining over single connection handles using `query_raw`.
    - Integrates atomic `ReplicaFreshnessCache` for read-replica traffic routing.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

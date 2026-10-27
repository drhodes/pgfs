# How-To: Dual-Interface Architecture

!!! danger "Status: Planned, not implemented"
    The `PgfsBackend` trait, its error mapping and an inode-keyed implementation on `Db` exist and are unit-tested, but the FUSE layer does not route through them yet. `RemoteBackend` is a stub, the `pgrx` engine and the `io_uring` transport are at the design stage, and there is no `--backend` option.

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy) and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

[Tutorial](../tutorials/dualinterfacearchitecture.md) · **How-to** · [Reference](../reference/dualinterfacearchitecture.md) · [Explanation](../explanation/dualinterfacearchitecture.md)

---

## Choose a backend at startup

Not available yet. There is no `--backend` option; the daemon
always uses the in-process `Db` client over libpq.

## Add an operation to the storage interface

1. Add an `async fn` to `PgfsBackend` in `src/backend.rs`. If
   not every backend can support it, give it a default body
   that returns `StorageError::InvalidArgument`, as
   `copy_file_range` and `fallocate` do.
2. Implement it for `Db` in `db::backend_impl` (`src/db.rs`).
3. Forward it in `impl PgfsBackend for LocalBackend`.
4. Add a stub to `RemoteBackend`.
5. Add a test next to `tests_offload` in `src/db.rs`.

## Report a storage failure with the right errno

Return the matching `StorageError` variant and convert it with
`libc::c_int::from(err)` when replying to the kernel. Use
`DatabaseError` (→ `EIO`) only for failures that are not a
normal answer, and log those with their full context.

## Run only the backend tests

```bash
cargo test backend
```

## Follow the design

The full design, including the shared-memory protocol and the
`io_uring` data path, is in
`design/dual_interface_architecture.md`.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

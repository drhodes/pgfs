# Tutorial: Dual-Interface Architecture

!!! danger "Status: Planned, not implemented"
    The `PgfsBackend` trait, its error mapping and an inode-keyed implementation on `Db` exist and are unit-tested, but the FUSE layer does not route through them yet. `RemoteBackend` is a stub, the `pgrx` engine and the `io_uring` transport are at the design stage, and there is no `--backend` option.

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy) and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

**Tutorial** · [How-to](../how-to/dualinterfacearchitecture.md) · [Reference](../reference/dualinterfacearchitecture.md) · [Explanation](../explanation/dualinterfacearchitecture.md)

---

The dual-interface architecture is not wired into the daemon
yet, so this tutorial explores the part that exists: the
storage seam that both future engines will plug into. We will
run its tests, read the trait, and see where the FUSE layer
will connect to it. All we need is the repository and the
project-local cluster.

## Step 1: run the backend tests

```bash
./scripts/ensure_db.sh
cargo test backend
```

```text
test backend::tests::test_storage_error_to_libc_errno ... ok
test backend::tests::test_remote_backend_unimplemented_stubs ... ok
test backend::tests::test_file_kind_conversion ... ok
test backend::tests::test_prepare_sparse_block_write_overwrite_existing ... ok
test backend::tests::test_prepare_sparse_block_write_zero_padding ... ok
test db::tests_offload::test_backend_offloaded_copy_file_range_and_fallocate ... ok
test result: ok. 6 passed; 0 failed; ...
```

## Step 2: read the trait

```bash
grep -n 'async fn' src/backend.rs | head -16
```

Every method takes inode numbers rather than paths. That is the
most important difference from the code that runs today, which
keys everything by `(parent path, name)`.

## Step 3: see how errors become errnos

```bash
grep -n -A12 'impl From<StorageError> for libc::c_int' src/backend.rs
```

A backend reports what went wrong (`NotFound`, `NotEmpty`,
`DatabaseError`, ...), and one function turns that into the
errno the kernel receives.

## Step 4: find the seam in the FUSE layer

```bash
grep -n 'backend' src/fs.rs | head
```

`PgFs` already holds a `backend` field, constructed as a
`LocalBackend` around the same `Db`, but the callbacks still
call `self.db` directly. Switching each callback from `self.db`
to `self.backend` is the next step of this feature.

## What we did

We exercised and read the storage abstraction that will let
pgfs choose between an in-process engine and a network engine.
The [explanation](../explanation/dualinterfacearchitecture.md)
describes where it is heading.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

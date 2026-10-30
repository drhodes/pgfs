# How-To: Dual-Interface Architecture

!!! warning "Status: Partially implemented"
    The FUSE layer reaches storage only through the synchronous `PgfsBackend` trait. `PgBackend` (tokio-postgres, one connection per worker, LSN tickets pipelined in the same round trip) is the remote engine, and the reference model in `tests/model/` implements the same trait. The pgrx engine waits on its measurement condition and the io_uring transport on crate support.

> Two storage engines sit behind one interface: a planned local engine that runs inside PostgreSQL and sends no SQL text on the hot path, and a planned remote engine that pipelines queries over the network. The FUSE layer talks only to the interface, so the engine is chosen at startup.

[Tutorial](../tutorials/dualinterfacearchitecture.md) · **How-to** · [Reference](../reference/dualinterfacearchitecture.md) · [Explanation](../explanation/dualinterfacearchitecture.md)

---

## Add an operation to the storage interface

1. Write the `pgfs.*` SQL function in `sql/v1/` (a new migration for a
   released schema).
2. Add the method to `PgfsBackend` in `src/backend.rs`.
3. Implement it in `PgBackend` (`src/pg.rs`): prepare the statement in
   `statements()` and call it through `mutate` or `read`.
4. Implement it in the reference model (`tests/model/mod.rs`).
5. Add the operation to the model-conformance generator.

## Report a storage failure with the right errno

Raise `PF0xx` from SQL (the codes are listed at the top of
`sql/v1/10_core.sql`); `src/errno.rs` maps every SQLSTATE in one table.

## Run only the backend tests

```bash
cargo test --test storage --test model_conformance
cargo test --features fault-injection --test fault_injection
```

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

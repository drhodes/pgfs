# Tutorial: Dual-Interface Architecture

!!! warning "Status: Partially implemented"
    The FUSE layer reaches storage only through the synchronous `PgfsBackend` trait. `PgBackend` (tokio-postgres, one connection per worker, LSN tickets pipelined in the same round trip) is the remote engine, and the reference model in `tests/model/` implements the same trait. The pgrx engine waits on its measurement condition and the io_uring transport on crate support.

> Two storage engines sit behind one interface: a planned local engine that runs inside PostgreSQL and sends no SQL text on the hot path, and a planned remote engine that pipelines queries over the network. The FUSE layer talks only to the interface, so the engine is chosen at startup.

**Tutorial** · [How-to](../how-to/dualinterfacearchitecture.md) · [Reference](../reference/dualinterfacearchitecture.md) · [Explanation](../explanation/dualinterfacearchitecture.md)

---

In this tutorial we will drive the same storage interface the daemon
uses from two implementations: the PostgreSQL backend and the
in-memory reference model, and watch them agree. We need the
project-local cluster (see [Your first pgfs mount](projectlocalenvironment.md)).

## Step 1: run the backend tests

```bash
cargo test --test storage
```

Every test formats a fresh database, drives `PgBackend` through the
`PgfsBackend` trait, and ends with `pgfs check`.

## Step 2: run the model against the database

```bash
cargo test --test model_conformance
```

The suite generates operation sequences, runs each against
`tests/model/` and against PostgreSQL through the same trait, and
compares replies and whole trees after every step.

## Step 3: read the trait

Open `src/backend.rs`. Each method is one `pgfs.*` SQL function or
prepared read; the FUSE layer in `src/fs.rs` calls nothing else.

## What we did

We saw one interface with two implementations that the daemon cannot
tell apart, which is what lets pgfs add a second engine later without
touching the FUSE layer.

---

<small>Generated from `spec.optimize.DualInterfaceArchitecture` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

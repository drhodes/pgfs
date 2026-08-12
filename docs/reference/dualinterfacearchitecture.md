# Reference: Dual-Interface Storage Architecture

**Feature**: `spec.optimize.DualInterfaceArchitecture`

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs
Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy)
and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

---

DualInterfaceArchitecture Component Reference:
- `PgfsBackendTraitReq`: Unified async trait specifying storage engine methods (`lookup`, `read_block`, `write_block`, etc.).
- `LocalPgrxEngineReq`: Runs inside PostgreSQL via `pgrx` postmaster background worker (`pg_bgworker`), pre-compiling SPI plans (`SPIPlanPtr`) and processing binary `Datum` values.
- `IoUringBufferRingReq`: Uses Linux 6.9+ `fuse-io_uring` SQ/CQ rings with pre-registered buffer rings for zero-copy VFS payloads.
- `RemoteAsyncBackendReq`: Uses `fuse3` async FUSE bindings with `tokio-postgres` connection pools (`bb8`) over TCP/Unix sockets.

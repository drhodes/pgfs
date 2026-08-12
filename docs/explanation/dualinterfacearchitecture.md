# Explanation: Dual-Interface Storage Architecture

**Feature**: `spec.optimize.DualInterfaceArchitecture`

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs
Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy)
and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

---

DualInterfaceArchitecture decouples FUSE filesystem request dispatching from storage engine execution.
The Local Engine embeds processing directly inside PostgreSQL as a background worker, bypassing SQL parsing
and socket serialization for sub-100µs latency. The Remote Engine provides standard out-of-process networking
using async Rust (`tokio-postgres`), allowing pgfs to scale seamlessly across network hosts.

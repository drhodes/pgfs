# Tutorial: Dual-Interface Storage Architecture

**Feature**: `spec.optimize.DualInterfaceArchitecture`

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs
Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy)
and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

---

In this tutorial, we will explore selecting between local in-process and remote network backends.
1. Launch pgfs with the remote async engine: `./target/release/pgfs --backend remote --conn postgres://...`.
2. Benchmark read/write latency using `make test-integration` or custom `fio` workloads.
3. Build the local `pgrx` extension and start PostgreSQL postmaster with the `pgrx` worker enabled.
4. Launch pgfs with the local engine: `./target/release/pgfs --backend local --shm /dev/shm/pgfs`.
5. Compare latency metrics to observe sub-100µs file operation latencies.

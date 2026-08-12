# How-To: Dual-Interface Storage Architecture

**Feature**: `spec.optimize.DualInterfaceArchitecture`

> pgfs features a Dual-Interface Storage Architecture providing a sub-100µs
Local In-Process Engine (`pgrx` + zero SQL parsing + `io_uring` zero copy)
and a scalable Remote Async Network Engine (`tokio-postgres` + pipelining).

---

To configure storage backends in pgfs:
- For single-node maximum performance, use `--backend local` to connect via shared-memory ring buffers (`shm_mq`) to the `pgrx` background worker.
- For networked or distributed setups, use `--backend remote` to connect via `tokio-postgres` TCP connection pools (`bb8`).
- To configure connection pool concurrency for remote backend, pass `--max-pool-size <N>`.

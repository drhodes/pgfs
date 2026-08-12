# Tutorial: Performance Optimization Pipeline

**Feature**: `spec.optimize.OptimizationRoadmap`

> pgfs incorporates a structured performance optimization pipeline
targeting database access latency, connection concurrency, userspace
buffering, and FUSE kernel-transport tuning.

---

In this tutorial, we will apply and benchmark key pgfs performance tuning options.
1. Start pgfs with kernel attribute caching enabled: `./target/release/pgfs --attr-ttl 5.0`.
2. Enable writeback caching in kernel VFS: `./target/release/pgfs --writeback-cache`.
3. Run `scripts/bench_profile.sh` to measure throughput across small-block write and read phases.
4. Observe latency histogram metrics printed in daemon logs to evaluate p50, p95, and p99 performance.

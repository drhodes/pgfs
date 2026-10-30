# How-To: Conformance Testing

!!! warning "Status: Partially implemented"
    The reference model, the proptest model-conformance suite (with branch scenarios), database fault injection (`--features fault-injection`), `pgfs check` after every test, divergence and bug-regression tests, and daemon and Postgres crash tests ship. Not yet: the LazyFS crash matrix, shuttle linearizability checks, pjdfstest and xfstests runs, and the configuration-matrix runner.

> pgfs is tested against an executable model of its specification: generated operation sequences, crashes at every point, injected database faults, every supported configuration, and the standard POSIX test suites, with the invariant checker run after every test.

[Tutorial](../tutorials/conformancetesting.md) · **How-to** · [Reference](../reference/conformancetesting.md) · [Explanation](../explanation/conformancetesting.md)

---

## Run crash conformance locally

```bash
./scripts/init_db.sh --lazyfs
cargo test --test crash -- --nocapture
```

## Inject a database fault in a test

```bash
cargo test --features fault-injection --test faults
```

## Run the POSIX suites

```bash
make test-posix     # pjdfstest and xfstests generic
```

---

<small>Generated from `spec.verify.ConformanceTesting` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

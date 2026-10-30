# Explanation: Conformance Testing

!!! warning "Status: Partially implemented"
    The reference model, the proptest model-conformance suite (with branch scenarios), database fault injection (`--features fault-injection`), `pgfs check` after every test, divergence and bug-regression tests, and daemon and Postgres crash tests ship. Not yet: the LazyFS crash matrix, shuttle linearizability checks, pjdfstest and xfstests runs, and the configuration-matrix runner.

> pgfs is tested against an executable model of its specification: generated operation sequences, crashes at every point, injected database faults, every supported configuration, and the standard POSIX test suites, with the invariant checker run after every test.

[Tutorial](../tutorials/conformancetesting.md) · [How-to](../how-to/conformancetesting.md) · [Reference](../reference/conformancetesting.md) · **Explanation**

---

## Why a model

A specification in prose can be read two ways. A small
in-memory filesystem that does exactly what the
specification says cannot. Running the same random
operations against the model and the real thing, and
comparing after every step, turns every disagreement into a
minimal failing case. Amazon's S3 team built ShardStore
this way.

## Why crash tests need a fake disk

The interesting crash states are the ones where some writes
reached the disk and others did not. A process kill cannot
produce them. LazyFS sits under Postgres and decides which
unsynced writes survive, so the tests can produce those
states on demand.

---

<small>Generated from `spec.verify.ConformanceTesting` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

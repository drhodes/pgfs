# Tutorial: Conformance Testing

!!! warning "Status: Partially implemented"
    The reference model, the proptest model-conformance suite (with branch scenarios), database fault injection (`--features fault-injection`), `pgfs check` after every test, divergence and bug-regression tests, and daemon and Postgres crash tests ship. Not yet: the LazyFS crash matrix, shuttle linearizability checks, pjdfstest and xfstests runs, and the configuration-matrix runner.

> pgfs is tested against an executable model of its specification: generated operation sequences, crashes at every point, injected database faults, every supported configuration, and the standard POSIX test suites, with the invariant checker run after every test.

**Tutorial** · [How-to](../how-to/conformancetesting.md) · [Reference](../reference/conformancetesting.md) · [Explanation](../explanation/conformancetesting.md)

---

In this tutorial we will run the model-conformance suite and
watch it find and shrink a failure.

## Step 1: run it

```bash
cargo test --test conformance -- --nocapture
```

## Step 2: read a failure

When an operation sequence makes the mount disagree with
the model, proptest shrinks it to the shortest sequence that
still fails and prints it, together with the step that
diverged and both results.

## Step 3: keep it

Shrunk failures are saved under
`tests/model/regressions/` and run on every build.

---

<small>Generated from `spec.verify.ConformanceTesting` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

"""
Legendary error handling contexts for a Rust codebase.

Every context below is a mixin that endows Feature and Requirement specs
with disciplined, Rust-idiomatic error handling guidance. The existing
infrastructure in `src/error.rs` already delivers much of this: a
`#[track_caller]`-annotated `ctx()` wrapper that layers file:line context
onto any `Result`, a `failure()` constructor for standalone errors, and a
`log_and_reply!` macro that logs the full chain before replying EIO to the
FUSE kernel. These specs describe the standard to which all new code must
be held.
"""

import textwrap

from libspec import Ctx, Feature, Requirement, UnimplementedMethodError
from libspec.diataxis import Diataxis


def md(text):
    """Dedent a triple-quoted Markdown block so Diátaxis quadrant methods
    can be written at natural indentation inside the class body."""
    return textwrap.dedent(text).strip("\n")


# ── Error handling foundation ────────────────────────────────────────

class Err(Ctx):
    """
    Every failure must tell a story — a narrative tracing from root cause
    through each layer of the call stack to the final observed symptom.

    Rust-specific rules:

    1. **Result<T, E> everywhere.** No function that can fail returns a
       bare `T`. Use `anyhow::Result<T>` for application-level propagation;
       use `thiserror`-derived enums for library/module boundaries where
       callers need to match on specific failure variants.

    2. **Context at every layer.** Each module that propagates an error
       MUST annotate it with what was being attempted at that layer:
       ```rust
       // db.rs
       error::ctx(client.query(...), "fetch entry attributes")
       // fs.rs (receives the enriched error)
       error::ctx(db.getattr(...), "lookup during readdir")
       ```
       The resulting chain with `{:#}` reads:
       ```
       lookup during readdir (at src/fs.rs:180): fetch entry attributes
       Caused by: fetch entry attributes (at src/db.rs:74): query failed
       Caused by: db error: connection closed unexpectedly
       ```

    3. **`#[track_caller]` on all error constructors.** The call site
       (file:line) must be captured where the error is surfaced, not
       inside the utility function. This is already enforced by
       `error::ctx()` and `error::failure()`.

    4. **Never consume an error silently.** Annotate error types with
       `#[must_use]` so the compiler enforces this at compile time.
       Every `Result` must be:
       - Propagated with `?`
       - Handled explicitly with `match` / `if let Err(e)`
       - Logged at an appropriate level before discarding
       A bare `let _ = fallible()` is a spec violation. When an error
       IS intentionally ignored, use the idiomatic escape hatch:
       `fallible().ok();` with a `// intentional: ...` comment.

    5. **Distinguish expected from unexpected.** Expected conditions
       (file absent → ENOENT, dir not empty → ENOTEMPTY) are returned
       directly to the kernel with the correct errno. Unexpected failures
       (DB down, corrupt data, invariant violation) are logged in full
       via `log_and_reply!` and surfaced as EIO. Never log expected
       failures at ERROR level — use DEBUG or omit.

    6. **Document every error variant.** Every `thiserror` variant and
       every `anyhow::bail!` call site must carry a description of when
       that error occurs and what the caller should do about it.

    7. **The program should be debuggable from error messages alone.**
       An operator reading the log should be able to identify:
       - Which operation failed
       - Where in the code it failed (file:line)
       - Why it failed (the root cause)
       - The full propagation path
       Without opening a debugger or reading source code.
    """


# ── Code quality contexts (Rust-idiomatic) ───────────────────────────

class BoilerPlate(Ctx):
    """
    Eliminate boilerplate through Rust's expressive type system.

    - Use `thiserror::Error` derive for error enums — never hand-write
      `Display` or `std::error::Error` impls for error types.
    - Use `anyhow::Context` trait's `.context()` and `.with_context()`
      instead of manual `map_err` chains.
    - Use the `?` operator for propagation — never write `match` just
      to unwrap a single error.
    - Prefer `#[derive(Debug, Clone, ...)]` over manual impls.
    - Use `impl Trait` in return position where a named type adds no
      documentary value.
    """


class FunctionLines(Ctx):
    """
    Keep functions compact and single-purpose.

    - Target: functions under 30 lines for Rust (slightly relaxed from
      other languages due to rustfmt's vertical layout).
    - If a function spills past 30 lines, extract a private helper.
    - Long `match` arms are a sign that each arm should be its own
      function.
    """


class Indentation(Ctx):
    """
    Rust naturally accumulates indentation through `match`, `if let`,
    closures, and chained combinators. Counteract this with:

    - **Early returns.** `let Some(x) = opt else { return ... };` keeps
      the happy path flat.
    - **The `?` operator** removes a level of `match`/`if let` per
      fallible call.
    - **Extract nested closures** into named functions.
    - Target: no more than 4 levels of visual indentation in any function
      body.
    """


# ── Defensive programming (Rust-idiomatic) ───────────────────────────

class PreCondition(Ctx):
    """
    Validate inputs at the function boundary — in Rust, this means
    returning `Err`, not panicking.

    - Public functions MUST validate all arguments and return
      `Result<_, Error>` / `Option<_>` for invalid input. Never
      `panic!` or `assert!` on caller-supplied data.
    - Private functions MAY use `debug_assert!` for invariants that
      the author believes are structurally impossible to violate
      (these are stripped in `--release`).
    - Use the type system to make invalid states unrepresentable:
      newtypes, enums over magic strings, `NonZeroU64` over bare
      integers with sentinel values.
    - Encode ordering obligations as typestate where it is cheap. For
      example, the call that publishes a block mapping takes a
      `BlobCommitted` token that only the blob insert can produce, so
      metadata can never point at data that was not written (SquirrelFS
      r64, ShardStore r128).
    """


class GlobalMutableState(Ctx):
    """
    Rust's ownership model eliminates most global mutable state by
    construction. The remaining concern is explicit shared mutability:

    - **Avoid `static mut`** — it requires `unsafe` and is never
      justified in application code.
    - **Avoid `lazy_static!` / `OnceLock<Mutex<T>>`** as a substitute
      for dependency injection. Pass state explicitly through function
      arguments or struct fields.
    - **Shared daemon state is named and owned.** Dispatch is
      multi-threaded (`concurrency.InodeShardedWorkers`). The state that
      matters is the inode cache, the handle table and the per-inode
      write buffers. Each entry is owned by the worker its inode hashes
      to, so per-file state needs no lock. State that is genuinely
      global (the mount lease, metrics, the in-flight table) is held in
      atomics or behind one lock with a documented scope.
    - **No lock is held across a database round trip**
      (`concurrency.NoLockAcrossRoundTrip`). A guard that must span an
      `.await` or a blocking query is a design error.
    - In the rare case shared state is needed (e.g., a metrics counter),
      isolate it behind a single `AtomicU64` rather than a
      `Mutex<HashMap>`.
    """


class PostCondition(Ctx):
    """
    Verify that functions uphold their contracts before returning.

    - Use `debug_assert!` for invariant checks that are always expected
      to hold (zero-cost in release builds).
    - For invariants that must hold in production, return
      `Err(error::failure("invariant violated: ..."))` rather than
      panicking — the caller (or the FUSE reply path) can log the full
      chain and degrade gracefully.
    - Functions that mutate state should verify the state is internally
      consistent before returning `Ok`.
    - This context is the coding rule for functions. What a filesystem
      *operation* guarantees is stated in each requirement's contract
      clauses (see `Contract`).
    """


# ── Composed contexts ────────────────────────────────────────────────

class DefensiveProgramming(PreCondition, PostCondition, GlobalMutableState):
    """The union of all defensive-programming concerns."""


class Refactor(BoilerPlate, FunctionLines, Indentation):
    """
    Continuously improve structure without changing behavior.

    - Extract reusable logic into private helper functions.
    - One public type (struct + impl) per file where practical.
    - File names match the primary type: `db.rs` for `struct Db`,
      `error.rs` for the error module.
    - Generics and traits should be introduced only when they eliminate
      concrete duplication, not preemptively.
    """


class Robustness(DefensiveProgramming):
    """
    Build systems that resist misuse.

    - Use library-provided constructors (`Client::connect`, not manual
      socket setup). The constructor must establish all invariants
      before returning.
    - Prefer composition over inheritance (struct fields over trait
      object hierarchies).
    - Use dependency injection: pass the worker's database handle to
      functions that need database access rather than reaching for a
      global singleton.
    - Every `unsafe` block must be isolated in its own function with a
      `// SAFETY:` comment documenting the invariant that makes it sound.
    - External resources (database connections, file handles) must be
      released deterministically — implement `Drop` or use RAII wrappers.
    - **Every external wait has a timeout, and every retry is bounded**
      (FiDe r78, metastable failures r67). The values are stated in
      `failure.BoundsEveryWait`, not chosen per call site.
    - **Every intentional `.ok()` is listed and tested.** Each site that
      deliberately discards an error is driven by a fault-injection test
      (`verify.DbFaultInjection`), and CI rejects new unlisted sites.
      Most catastrophic failures sit in error handlers that were never
      run (Aspirator r119).
    """


# ── Operation contracts ──────────────────────────────────────────────

class Contract(Ctx):
    """
    Every requirement is a contract, not a description of code. A
    requirement's docstring opens with its behaviour and then states the
    clauses below that apply to it, each as a paragraph that starts with
    the clause name in bold. A clause that is absent means the
    requirement makes no promise of that kind, never that the promise is
    obvious. (SysSpec r43, PoWER r60, SquirrelFS r64.)

    - **Postcondition.** What holds after a successful reply, in terms of
      the observable filesystem state: bytes, sizes, names, attributes.
    - **Invariant.** What holds before and after every operation, and
      under any interleaving.
    - **Durability.** The exact event after which the effect survives a
      crash of the daemon, the host or Postgres. "None" is a valid
      answer and must be written down.
    - **Crash.** The set of states a crash at any instant may leave,
      relative to the last acknowledged operation.
    - **Concurrency.** What a concurrent observer may see, and which
      locks are taken in which order.
    - **Errors.** Each errno the operation can reply, and when.
    - **Bounds.** Limits and costs that must not grow with the size of
      the tree: round trips, rows touched, memory, time.
    - **Sources.** The papers in `design/papers/reading-notes.md` that
      the requirement relies on.

    Requirements describe the target system. What exists today is stated
    in the status note of the feature that lists the requirement.
    """


# ── Base classes for Feature / Requirement specs ─────────────────────

STABLE = "stable"
PARTIAL = "partial"
PLANNED = "planned"


class Feat(Err, Refactor, Robustness, Diataxis):
    """Feature specification: inherits all error-handling, code-quality, and
    Diátaxis documentation contexts. Every Feature in the codebase must conform to these rules.

    Besides the four Diátaxis quadrants, every Feature declares `title()`
    and `status()`, and lists the Requirements it is built from in `deps`
    so the generated Reference page can enumerate them."""
    __is_base_spec__ = True

    def title(self) -> str:
        """Human-readable feature title used for documentation headings."""
        raise UnimplementedMethodError()

    def status(self) -> str:
        """Implementation status: one of STABLE (implemented and tested),
        PARTIAL (some requirements implemented), or PLANNED (specified,
        not yet implemented). Paired with `status_note()`."""
        raise UnimplementedMethodError()

    def status_note(self) -> str:
        """One or two sentences qualifying `status()`: what works today and
        what does not. Rendered as an admonition on every generated page."""
        raise UnimplementedMethodError()


class Req(Err, Refactor, Robustness, Contract, Requirement):
    """Requirement specification: inherits all error-handling,
    code-quality and contract contexts. Every Requirement in the
    codebase must conform to these rules."""

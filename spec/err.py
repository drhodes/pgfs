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

from libspec import Ctx, Feature, Requirement


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
    - **Prefer `&` and `&mut`** over `Arc<Mutex<T>>` within a
      single-threaded context. FUSE's default single-threaded dispatch
      makes interior mutability unnecessary for this project.
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
    """


# ── Composed contexts ────────────────────────────────────────────────

class DefensiveProgramming(PreCondition, PostCondition, GlobalMutableState):
    """The union of all defensive-programming concerns."""
    pass


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
    pass


class Robustness(DefensiveProgramming):
    """
    Build systems that resist misuse.

    - Use library-provided constructors (`Client::connect`, not manual
      socket setup). The constructor must establish all invariants
      before returning.
    - Prefer composition over inheritance (struct fields over trait
      object hierarchies).
    - Use dependency injection: pass `&mut Db` to functions that need
      database access rather than reaching for a global singleton.
    - Every `unsafe` block must be isolated in its own function with a
      `// SAFETY:` comment documenting the invariant that makes it sound.
    - External resources (database connections, file handles) must be
      released deterministically — implement `Drop` or use RAII wrappers.
    """


# ── Base classes for Feature / Requirement specs ─────────────────────

class Feat(Err, Refactor, Robustness, Feature):
    """Feature specification: inherits all error-handling and code-quality
    contexts. Every Feature in the codebase must conform to these rules."""
    pass


class Req(Err, Refactor, Robustness, Requirement):
    """Requirement specification: inherits all error-handling and
    code-quality contexts. Every Requirement in the codebase must
    conform to these rules."""
    pass

# Explanation: Atomic Renames & Lock-Move Semantics

**Feature**: `spec.app.AtomicRename`

> Users can rename (move) files and directories with standard
`mv` semantics, including overwriting existing files and empty
directories.

---

AtomicRename guarantees POSIX compliance by wrapping entry updates and subtree path re-keying
within a single PostgreSQL transaction (`BEGIN ... COMMIT`). This ensures no intermediate state
is exposed to concurrent readers, preventing file corruption during rename-overwrite operations.

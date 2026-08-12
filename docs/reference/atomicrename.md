# Reference: Atomic Renames & Lock-Move Semantics

**Feature**: `spec.app.AtomicRename`

> Users can rename (move) files and directories with standard
`mv` semantics, including overwriting existing files and empty
directories.

---

AtomicRename Semantics & Error Matrix:
- `rename(old_parent, old_name, new_parent, new_name)`: Atomic move in single SQL transaction.
- Target exists file -> atomically replaced.
- Target exists empty directory -> atomically replaced.
- Target exists non-empty directory -> ENOTEMPTY.
- Source file & target directory -> EISDIR; Source directory & target file -> ENOTDIR.
- Renaming directory updates parent pointers of all descendants in transaction.

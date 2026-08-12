# How-To: Atomic Renames & Lock-Move Semantics

**Feature**: `spec.app.AtomicRename`

> Users can rename (move) files and directories with standard
`mv` semantics, including overwriting existing files and empty
directories.

---

To rename entries safely on pgfs:
- Use standard `mv <src> <dst>` or `rename(oldpath, newpath)` syscall.
- If replacing an existing target, ensure target is a file or empty directory.
- Lock-and-rename atomic patterns (such as those used by `git init` and compilers) are supported out-of-the-box.

# Reference: Atomic Rename

!!! success "Status: Stable"
    File and directory renames, overwrite semantics and the full POSIX error matrix are implemented and covered by `test_rename.sh` and `test_git_init.sh`. `renameat2` flags (`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) are rejected.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

[Tutorial](../tutorials/atomicrename.md) · [How-to](../how-to/atomicrename.md) · **Reference** · [Explanation](../explanation/atomicrename.md)

---

## Outcome matrix

| Source | Target | Result |
|---|---|---|
| file | absent | moved |
| file | file | target atomically replaced |
| file | directory | `EISDIR` |
| directory | absent | moved with all descendants |
| directory | empty directory | target atomically replaced |
| directory | non-empty directory | `ENOTEMPTY` |
| directory | file | `ENOTDIR` |
| directory | inside its own subtree | `EINVAL` |
| any | itself | success, no change |
| absent | any | `ENOENT` |
| any, with `renameat2` flags ≠ 0 | any | `EINVAL` |

## Transaction

Every check that decides the outcome reads the primary
database, never a replica. A successful rename is one
transaction:

1. If the target exists, delete its `entries` row and its
   `blocks` rows.
2. If the source is a directory, rewrite `parent` in every
   `entries` and `blocks` row below it, replacing the old path
   prefix with the new one.
3. Update the source's `entries` row to the new
   `(parent, name)` and set its `mtime`.
4. Move the source's `blocks` rows to the new `(parent, name)`.
5. Commit. If no row moved, roll back and reply `EIO`.

After the commit, the in-memory path ↔ inode maps are re-keyed
for the entry and its whole subtree, so inode numbers the
kernel already holds stay valid.

## Cost

| Rename | Rows written |
|---|---|
| file | 1 entry + its blocks |
| directory | 1 entry + every descendant entry + every descendant block |

## Open files

Pending writes in an open file's buffer are tied to the path
the file had when it was opened. Close or `fsync` a file
before renaming it, otherwise those writes are not carried
over to the new name.

## Isolation

Concurrent SQL readers, and other pgfs daemons on the same
database, see the tree either entirely before or entirely
after the rename (PostgreSQL `READ COMMITTED`).


## Specification requirements

`AtomicRename` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "RenamesEntries"
    `spec.app.RenamesEntries`

    A FUSE `rename` moves an entry from one (parent, name) to
    another. It follows POSIX semantics:

    - If the target exists and is a file, it is atomically replaced.
    - If the target exists and is an empty directory, it is atomically
      replaced.
    - If the target is a non-empty directory, the kernel receives
      ENOTEMPTY.
    - If source is a file and target is a directory, the kernel receives
      EISDIR.
    - If source is a directory and target is a file, the kernel receives
      ENOTDIR.
    - Renaming a directory cascades: every descendant's `parent` path
      is rewritten in the same database transaction.
    - The in-memory inode<->path maps are re-keyed for the entire
      subtree.

??? abstract "RejectsInvalidRenames"
    `spec.app.RejectsInvalidRenames`

    A FUSE `rename` with non-zero flags replies EINVAL. Renaming a
    directory into one of its own descendants replies EINVAL. Renaming
    an entry onto itself is a no-op.

??? abstract "SupportsGitInit"
    `spec.app.SupportsGitInit`

    Running `git init` inside the mounted filesystem succeeds without
    errors. This exercises the full rename-overwrite path: git creates
    `.git/config`, then uses lock+rename to set config values, which
    triggers rename-overwrite with POSIX semantics.

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

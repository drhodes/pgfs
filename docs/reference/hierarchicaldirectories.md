# Reference: Hierarchical Directory Trees

**Feature**: `spec.app.HierarchicalDirectories`

> Users can create nested directories, list their contents, and
remove empty directories through standard POSIX directory
operations.

---

HierarchicalDirectories Callbacks & Rules:
- `mkdir(parent_ino, name, mode) -> (ino, attr)`: Inserts dir entry with nlink=2, permissions 0755.
- `rmdir(parent_ino, name) -> status`: Deletes dir row if child count == 0; else replies ENOTEMPTY.
- `readdir(ino, offset) -> entries`: Synthesizes '.' and '..' then yields child inodes.
- Root directory fixed inode: `ino = 1`.

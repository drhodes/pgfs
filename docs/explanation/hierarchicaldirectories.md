# Explanation: Hierarchical Directory Trees

**Feature**: `spec.app.HierarchicalDirectories`

> Users can create nested directories, list their contents, and
remove empty directories through standard POSIX directory
operations.

---

HierarchicalDirectories models a directory tree within PostgreSQL using parent inode pointers
`(parent_ino, name)` indexed by B-Tree structures. Directory traversals check parent relationships,
enabling efficient hierarchy navigation while enforcing strict POSIX directory semantics.

# How-To: Hierarchical Directory Trees

**Feature**: `spec.app.HierarchicalDirectories`

> Users can create nested directories, list their contents, and
remove empty directories through standard POSIX directory
operations.

---

To work with hierarchical directories in pgfs:
- To create nested directories, run `mkdir -p <dir_path>`.
- To list directory contents, call `readdir` via `ls` or standard directory stream APIs.
- To remove a directory, ensure all children are deleted first, then invoke `rmdir <dir_path>`.

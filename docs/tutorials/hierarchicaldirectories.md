# Tutorial: Hierarchical Directory Trees

**Feature**: `spec.app.HierarchicalDirectories`

> Users can create nested directories, list their contents, and
remove empty directories through standard POSIX directory
operations.

---

In this tutorial, we will work with directory trees in pgfs.
1. Create a nested directory structure: `mkdir -p ./mnt/docs/project/src`.
2. List directory contents with `ls -la ./mnt/docs/project` to see '.' and '..' entries.
3. Create files inside nested subdirectories: `touch ./mnt/docs/project/src/main.rs`.
4. Try removing a non-empty directory with `rmdir ./mnt/docs/project` and observe the `ENOTEMPTY` error.
5. Delete sub-files first, then remove empty directories using `rmdir ./mnt/docs/project/src`.

# Tutorial: Atomic Renames & Lock-Move Semantics

**Feature**: `spec.app.AtomicRename`

> Users can rename (move) files and directories with standard
`mv` semantics, including overwriting existing files and empty
directories.

---

In this tutorial, we will perform atomic file and directory renames.
1. Create a source file: `echo 'v1' > ./mnt/a.txt` and target file `echo 'v2' > ./mnt/b.txt`.
2. Execute `mv ./mnt/a.txt ./mnt/b.txt` to atomically replace `b.txt` with `a.txt`.
3. Verify `./mnt/b.txt` contains 'v1' and `./mnt/a.txt` no longer exists.
4. Test directory subtree rename: `mkdir ./mnt/old_dir` and move it with `mv ./mnt/old_dir ./mnt/new_dir`.

# How-To: POSIX Flat Filesystem

**Feature**: `spec.app.FlatFilesystem`

> Users can create, read, write, and delete files through standard
POSIX file operations on the mounted directory.

---

To manage regular files on a mounted pgfs volume:
- To create an empty file or open with creation flags, use `touch <path>` or standard `open(path, O_CREAT)`.
- To write arbitrary byte sequences, invoke standard POSIX `write()` or redirect output stream to the file path.
- To read file contents, open the path with `O_RDONLY` and issue `read()` calls.
- To remove a file, call `unlink(path)` or use `rm <path>`.

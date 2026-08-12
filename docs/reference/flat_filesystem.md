# Reference: POSIX Flat Filesystem

**Feature**: `spec.app.FlatFilesystem`

> Users can create, read, write, and delete files through standard
POSIX file operations on the mounted directory.

---

FlatFilesystem Interface & Callbacks:
- `create(parent_ino, name, mode, flags) -> (ino, attr)`: Inserts entry with kind='file', size=0.
- `read(ino, offset, size) -> bytes`: Returns byte slice clamped to stored blob length.
- `write(ino, offset, data) -> bytes_written`: Updates or extends byte blob in PostgreSQL.
- `unlink(parent_ino, name) -> status`: Removes row from database and inode map.
- Error codes: ENOENT (file missing), EEXIST (creation conflict), EIO (database failure).

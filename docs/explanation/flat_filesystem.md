# Explanation: POSIX Flat Filesystem

**Feature**: `spec.app.FlatFilesystem`

> Users can create, read, write, and delete files through standard
POSIX file operations on the mounted directory.

---

FlatFilesystem translates POSIX file I/O operations into relational database operations on PostgreSQL.
File metadata and byte payloads are mapped to database records. Reads and writes interact with
stored byte arrays while presenting a standard POSIX filesystem interface to applications through FUSE.

"""
pgfs requirements — granular, single-responsibility, testable.

Each class specifies one capability of the PostgreSQL-backed FUSE
filesystem. Every Requirement and Feature inherits the Rust-idiomatic
error-handling (err.py) and observability (observe.py) contexts.
"""

from .err import Feat, Req


class ConnectsToPostgres(Req):
    """The daemon connects to a Postgres database via a libpq connection
    string. On first connect it creates the `entries` table with the
    (parent, name, kind, data, size, mtime) schema if it does not
    already exist. Connection failure produces a full error story
    including the connection string and the root cause."""
    pass


class CreatesFiles(Req):
    """A FUSE `create` (O_CREAT) inserts a row with kind='file', empty
    data, size=0, and the current timestamp. If the entry already exists
    the kernel receives EEXIST. The reply includes a valid inode and
    file attributes with 0644 permissions."""
    pass


class ReadsFiles(Req):
    """A FUSE `read` on a regular-file inode returns the byte range
    from the `data` column. Offset and size are clamped to the stored
    blob length. Missing inodes reply ENOENT."""
    pass


class WritesFiles(Req):
    """A FUSE `write` performs an in-memory read-modify-write of the
    full blob, then persists the result to Postgres. The write extends
    the file if the offset+length exceeds the current size."""
    pass


class UnlinksFiles(Req):
    """A FUSE `unlink` deletes the row and removes the inode from the
    in-memory path map."""
    pass


class CreatesDirectories(Req):
    """A FUSE `mkdir` inserts a row with kind='dir'. If the entry exists
    the kernel receives EEXIST. The reply includes a directory inode
    with 0755 permissions and nlink=2."""
    pass


class RemovesDirectories(Req):
    """A FUSE `rmdir` deletes the directory row only if the directory is
    empty (no child rows with matching parent path). If children exist
    the kernel receives ENOTEMPTY. If the entry does not exist the
    kernel receives ENOENT."""
    pass


class ListsDirectories(Req):
    """A FUSE `readdir` returns '.' and '..' followed by every child of
    the directory inode, each with the correct inode number and file
    type. Offsets are synthetic (1-based index); the reply stops when
    the kernel's buffer is full and resumes on the next call."""
    pass


class LooksUpEntries(Req):
    """A FUSE `lookup` on a (parent inode, name) pair returns the
    entry's inode and attributes, or ENOENT if absent. Lookup allocates
    a stable inode number for the entry's full path."""
    pass


class ReportsAttributes(Req):
    """A FUSE `getattr` returns size, kind, permissions, timestamps,
    and nlink for any inode. The root inode (ino=1) is always a
    directory."""
    pass


class TruncatesFiles(Req):
    """A FUSE `setattr` with a size parameter truncates or extends a
    regular file to the given size (zero-filled extension). Size changes
    on directories reply EISDIR."""
    pass


class RenamesEntries(Req):
    """A FUSE `rename` moves an entry from one (parent, name) to
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
      subtree."""
    pass


class RejectsInvalidRenames(Req):
    """A FUSE `rename` with non-zero flags replies EINVAL. Renaming a
    directory into one of its own descendants replies EINVAL. Renaming
    an entry onto itself is a no-op."""
    pass


class MountsCleanly(Req):
    """The daemon mounts a FUSE filesystem at the given mountpoint using
    AutoUnmount so the kernel removes the mount if the process exits
    for any reason. The mountpoint must be an existing, empty directory
    not already mounted."""
    pass


class HandlesShutdownSignals(Req):
    """SIGINT, SIGTERM, and SIGHUP trigger a clean unmount via
    `session.unmount()`. If the clean unmount fails, AutoUnmount
    guarantees the kernel still removes the mount. Signal handling
    uses a dedicated `sigwait` thread so it never blocks FUSE
    dispatch."""
    pass


class DistinguishesExpectedErrors(Req):
    """Expected conditions (ENOENT, EEXIST, ENOTEMPTY, EISDIR, ENOTDIR,
    EINVAL) are replied directly to the kernel without logging.
    Unexpected failures (database errors, invariant violations) are
    logged in full via `log_and_reply!` with the complete error chain
    and replied as EIO."""
    pass


class ReportsCrashes(Req):
    """If the daemon panics, the panic hook emits a crash report to both
    stderr and the log channel. The report includes the panic message,
    file:line location, a symbolized backtrace, and OS/Rust metadata.
    In release builds, `panic = \"abort\"` prevents a broken daemon from
    lingering."""
    pass


class UsesStructuredTracing(Req):
    """The daemon uses the `tracing` crate (not bare `log`/`env_logger`)
    for all diagnostic output. The `tracing-subscriber` subscriber is
    initialized before any other work. `RUST_LOG` controls filter levels
    with the same syntax as `env_logger`; `RUST_LOG_FORMAT=json` switches
    to machine-parseable JSON output."""
    pass


class InstrumentsFuseCallbacks(Req):
    """Every `Filesystem` callback (`lookup`, `getattr`, `read`, `write`,
    `create`, `mkdir`, `unlink`, `rmdir`, `rename`, `readdir`, `setattr`,
    `open`, `fsync`) opens a `tracing::info_span!` recording `ino` and
    `name` (where applicable). The span is entered for the duration of
    the callback so nested DB operations are automatically attributed."""
    pass


class InstrumentsDbOperations(Req):
    """Every `Db` method (`getattr`, `read`, `write`, `create`, `mkdir`,
    `unlink`, `rmdir`, `rename`, `list`) opens a `tracing::debug_span!`
    recording the (parent, name) key. The span is entered for the
    duration of the database round-trip."""
    pass


class SupportsGitInit(Req):
    """Running `git init` inside the mounted filesystem succeeds without
    errors. This exercises the full rename-overwrite path: git creates
    `.git/config`, then uses lock+rename to set config values, which
    triggers rename-overwrite with POSIX semantics."""
    pass


# ── Features (user-visible capabilities) ─────────────────────────────

class FlatFilesystem(Feat):
    """Users can create, read, write, and delete files through standard
    POSIX file operations on the mounted directory."""

    def tutorial(self):
        return (
            "In this tutorial, we will mount a pgfs filesystem and perform basic POSIX file operations.\n"
            "1. Start the pgfs daemon mounted at `./mnt`.\n"
            "2. Create a file using `echo 'Hello pgfs' > ./mnt/demo.txt`.\n"
            "3. Read the file back with `cat ./mnt/demo.txt` and verify the content.\n"
            "4. Append data using `echo 'More content' >> ./mnt/demo.txt`.\n"
            "5. Delete the file using `rm ./mnt/demo.txt` and confirm it no longer appears in `ls ./mnt`."
        )

    def how_to(self):
        return (
            "To manage regular files on a mounted pgfs volume:\n"
            "- To create an empty file or open with creation flags, use `touch <path>` or standard `open(path, O_CREAT)`.\n"
            "- To write arbitrary byte sequences, invoke standard POSIX `write()` or redirect output stream to the file path.\n"
            "- To read file contents, open the path with `O_RDONLY` and issue `read()` calls.\n"
            "- To remove a file, call `unlink(path)` or use `rm <path>`."
        )

    def reference(self):
        return (
            "FlatFilesystem Interface & Callbacks:\n"
            "- `create(parent_ino, name, mode, flags) -> (ino, attr)`: Inserts entry with kind='file', size=0.\n"
            "- `read(ino, offset, size) -> bytes`: Returns byte slice clamped to stored blob length.\n"
            "- `write(ino, offset, data) -> bytes_written`: Updates or extends byte blob in PostgreSQL.\n"
            "- `unlink(parent_ino, name) -> status`: Removes row from database and inode map.\n"
            "- Error codes: ENOENT (file missing), EEXIST (creation conflict), EIO (database failure)."
        )

    def explanation(self):
        return (
            "FlatFilesystem translates POSIX file I/O operations into relational database operations on PostgreSQL.\n"
            "File metadata and byte payloads are mapped to database records. Reads and writes interact with\n"
            "stored byte arrays while presenting a standard POSIX filesystem interface to applications through FUSE."
        )


class HierarchicalDirectories(Feat):
    """Users can create nested directories, list their contents, and
    remove empty directories through standard POSIX directory
    operations."""

    def tutorial(self):
        return (
            "In this tutorial, we will work with directory trees in pgfs.\n"
            "1. Create a nested directory structure: `mkdir -p ./mnt/docs/project/src`.\n"
            "2. List directory contents with `ls -la ./mnt/docs/project` to see '.' and '..' entries.\n"
            "3. Create files inside nested subdirectories: `touch ./mnt/docs/project/src/main.rs`.\n"
            "4. Try removing a non-empty directory with `rmdir ./mnt/docs/project` and observe the `ENOTEMPTY` error.\n"
            "5. Delete sub-files first, then remove empty directories using `rmdir ./mnt/docs/project/src`."
        )

    def how_to(self):
        return (
            "To work with hierarchical directories in pgfs:\n"
            "- To create nested directories, run `mkdir -p <dir_path>`.\n"
            "- To list directory contents, call `readdir` via `ls` or standard directory stream APIs.\n"
            "- To remove a directory, ensure all children are deleted first, then invoke `rmdir <dir_path>`."
        )

    def reference(self):
        return (
            "HierarchicalDirectories Callbacks & Rules:\n"
            "- `mkdir(parent_ino, name, mode) -> (ino, attr)`: Inserts dir entry with nlink=2, permissions 0755.\n"
            "- `rmdir(parent_ino, name) -> status`: Deletes dir row if child count == 0; else replies ENOTEMPTY.\n"
            "- `readdir(ino, offset) -> entries`: Synthesizes '.' and '..' then yields child inodes.\n"
            "- Root directory fixed inode: `ino = 1`."
        )

    def explanation(self):
        return (
            "HierarchicalDirectories models a directory tree within PostgreSQL using parent inode pointers\n"
            "`(parent_ino, name)` indexed by B-Tree structures. Directory traversals check parent relationships,\n"
            "enabling efficient hierarchy navigation while enforcing strict POSIX directory semantics."
        )


class AtomicRename(Feat):
    """Users can rename (move) files and directories with standard
    `mv` semantics, including overwriting existing files and empty
    directories."""

    def tutorial(self):
        return (
            "In this tutorial, we will perform atomic file and directory renames.\n"
            "1. Create a source file: `echo 'v1' > ./mnt/a.txt` and target file `echo 'v2' > ./mnt/b.txt`.\n"
            "2. Execute `mv ./mnt/a.txt ./mnt/b.txt` to atomically replace `b.txt` with `a.txt`.\n"
            "3. Verify `./mnt/b.txt` contains 'v1' and `./mnt/a.txt` no longer exists.\n"
            "4. Test directory subtree rename: `mkdir ./mnt/old_dir` and move it with `mv ./mnt/old_dir ./mnt/new_dir`."
        )

    def how_to(self):
        return (
            "To rename entries safely on pgfs:\n"
            "- Use standard `mv <src> <dst>` or `rename(oldpath, newpath)` syscall.\n"
            "- If replacing an existing target, ensure target is a file or empty directory.\n"
            "- Lock-and-rename atomic patterns (such as those used by `git init` and compilers) are supported out-of-the-box."
        )

    def reference(self):
        return (
            "AtomicRename Semantics & Error Matrix:\n"
            "- `rename(old_parent, old_name, new_parent, new_name)`: Atomic move in single SQL transaction.\n"
            "- Target exists file -> atomically replaced.\n"
            "- Target exists empty directory -> atomically replaced.\n"
            "- Target exists non-empty directory -> ENOTEMPTY.\n"
            "- Source file & target directory -> EISDIR; Source directory & target file -> ENOTDIR.\n"
            "- Renaming directory updates parent pointers of all descendants in transaction."
        )

    def explanation(self):
        return (
            "AtomicRename guarantees POSIX compliance by wrapping entry updates and subtree path re-keying\n"
            "within a single PostgreSQL transaction (`BEGIN ... COMMIT`). This ensures no intermediate state\n"
            "is exposed to concurrent readers, preventing file corruption during rename-overwrite operations."
        )


class ReliableMount(Feat):
    """The filesystem mounts and unmounts cleanly. Crashing or killing
    the daemon never leaves a ghost mount behind."""

    def tutorial(self):
        return (
            "In this tutorial, we will verify clean daemon mount and shutdown behavior.\n"
            "1. Mount pgfs using `scripts/pgfs.sh up`.\n"
            "2. Verify mount point status via `scripts/pgfs.sh status` and check PID file at `/tmp/pgfs-*.pid`.\n"
            "3. Send SIGINT to the daemon process or run `scripts/pgfs.sh down`.\n"
            "4. Verify the mountpoint is cleanly unmounted and no stale daemon processes remain."
        )

    def how_to(self):
        return (
            "To ensure reliable mounting and clean shutdown:\n"
            "- Mount with `AutoUnmount` flag enabled in FUSE session options.\n"
            "- Use SIGINT, SIGTERM, or SIGHUP to trigger explicit graceful unmount.\n"
            "- Poll `/tmp/pgfs-*.ready` to check daemon readiness before performing file I/O."
        )

    def reference(self):
        return (
            "ReliableMount Lifecycle Specification:\n"
            "- Mount flags: `AutoUnmount` enabled.\n"
            "- Signal management: Dedicated `sigwait` thread handles SIGINT, SIGTERM, SIGHUP.\n"
            "- Control files: `/tmp/pgfs-{hash}.pid` (daemon PID), `/tmp/pgfs-{hash}.ready` (readiness marker).\n"
            "- Panic recovery: `panic = \"abort\"` in release builds ensures instant exit and kernel mount cleanup."
        )

    def explanation(self):
        return (
            "ReliableMount combines FUSE `AutoUnmount` kernel flags with dedicated signal handling threads\n"
            "to guarantee that process exits, panics, or external termination signals always trigger a clean\n"
            "kernel unmount, preventing dangling mount points and stale state."
        )

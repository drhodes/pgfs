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
    pass


class HierarchicalDirectories(Feat):
    """Users can create nested directories, list their contents, and
    remove empty directories through standard POSIX directory
    operations."""
    pass


class AtomicRename(Feat):
    """Users can rename (move) files and directories with standard
    `mv` semantics, including overwriting existing files and empty
    directories."""
    pass


class ReliableMount(Feat):
    """The filesystem mounts and unmounts cleanly. Crashing or killing
    the daemon never leaves a ghost mount behind."""
    pass

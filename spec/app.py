"""
pgfs requirements — granular, single-responsibility, testable.

Each class specifies one capability of the PostgreSQL-backed FUSE
filesystem. Every Requirement and Feature inherits the Rust-idiomatic
error-handling (err.py) and observability (observe.py) contexts.
"""

from .err import STABLE, Feat, Req, md


class ConnectsToPostgres(Req):
    """The daemon connects to a Postgres database via a libpq connection
    string. On first connect it creates the `entries` table (ino, parent_ino,
    parent, name, kind, size, mode, uid, gid, atime, mtime, ctime) and
    `blocks` table (ino, parent, name, block_no, data) if they do not
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
    from the file's stored block chunks. Offset and size are clamped to
    the stored file length. Missing inodes reply ENOENT."""
    pass


class WritesFiles(Req):
    """A FUSE `write` updates touched block chunks or buffers dirty blocks
    per open file handle, persisting them to PostgreSQL. The write extends
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
    regular file to the given size (zero-filled extension, deleting
    trailing block chunks in PostgreSQL). Size changes on directories
    reply EISDIR."""
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
    `open`, `flush`, `fsync`, `copy_file_range`, `fallocate`) opens a
    `tracing::info_span!` recording `ino` and `name` (where applicable).
    The span is entered for the duration of the callback so nested DB
    operations are automatically attributed."""
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


# ── Development environment ──────────────────────────────────────────

class ProvidesProjectLocalCluster(Req):
    """`scripts/init_db.sh` creates a PostgreSQL cluster at
    `testdata/pgdata` (`initdb --auth=trust --no-locale --encoding=UTF8`)
    and starts it with its Unix socket in `testdata/` and a TCP listener on
    127.0.0.1 only. It ensures a superuser role named after the invoking OS
    user and a database named `pgfs` exist, and prints the connection
    string. `scripts/ensure_db.sh` is the idempotent wrapper used by
    `make test`: it exits immediately when the cluster already answers.
    Nothing is written outside the project's `testdata/` directory."""


class ProvidesLifecycleScript(Req):
    """`scripts/pgfs.sh up|down|status|run` owns the mount lifecycle.
    `up` starts Postgres if needed, runs `cargo build`, sweeps a stale mount
    whose daemon is gone, starts `target/debug/pgfs` detached with output
    appended to `testdata/pgfs.log`, and waits up to 10 s for the mount to
    appear in `/proc/mounts`. `down` sends SIGTERM, waits up to 10 s, then
    escalates to `fusermount3 -u`, reports the holders of a busy mount,
    and finally lazy-unmounts. `status` reports mount, daemon and Postgres
    state and flags ghost mounts. `run` builds and execs the daemon in the
    foreground. `PGFS_MOUNT` overrides the default `testdata/mnt`."""


class RunsIntegrationSuite(Req):
    """`scripts/run_tests.sh` (`make test-integration`) runs every
    `scripts/test_*.sh` except the shared harness `test_lib.sh`, each in
    isolation under a 300 s timeout, writing each test's output to
    `testdata/<test>.log`. After every test it kills any leftover daemon
    and lazily unmounts `testdata/mnt` so the next test starts clean. It
    prints PASS/FAIL per test and exits non-zero if any test failed."""


# ── Observability requirements ───────────────────────────────────────

class ExportsPeriodicMetrics(Req):
    """A background thread logs five INFO lines every 60 s:
    `metrics|ops|…` (per-callback counters), `metrics|errors|…` (EIO and
    per-errno counters), `metrics|replica|reads=… fallbacks=…`,
    `metrics|latency|fuse=…` and `metrics|latency|db=…`. Counters are
    lifetime totals and never reset. Latency histograms bucket wall-clock
    durations at ≤10µs, ≤100µs, ≤1ms, ≤10ms, ≤100ms, ≤1s and >1s and
    report p50/p95/p99 as the upper bound of the bucket that contains the
    percentile."""


class DumpsStateOnSigusr1(Req):
    """SIGUSR1 sets a flag on the signal-waiter thread. The next FUSE
    callback clears it and logs, at INFO, `state|ino_cache=… next_ino=…
    db=connected (replica none|fresh|stale or unreachable) uptime_secs=…`
    followed by the FUSE and DB latency summaries. The dump never runs on
    the signal thread and never blocks dispatch; on an idle mount it
    appears with the next filesystem access."""


class WritesHealthFiles(Req):
    """After connecting to Postgres the daemon writes its PID to
    `/tmp/pgfs-<hash>.pid`, prints `health pid file: <path>` on stdout and
    logs it. After the FUSE session is created it writes
    `/tmp/pgfs-<hash>.ready` containing `ready`. `<hash>` is 16 hex digits
    derived from the mountpoint argument exactly as given. Both files are
    removed when `run()` returns."""


class DetectsDeadlock(Req):
    """A liveness thread increments a counter every 10 s. Every FUSE
    callback checks it on entry; if the counter has not advanced for 30 s
    the callback logs ERROR, replies EIO and flags a watchdog thread. The
    watchdog unmounts cleanly, retrying up to 5 times 500 ms apart, and
    exits the process if every attempt fails so that AutoUnmount removes
    the mount. The check uses a monotonic clock, so wall-clock jumps never
    cause false positives."""


# ── Features (user-visible capabilities) ─────────────────────────────

class ProjectLocalEnvironment(Feat):
    """A contributor goes from a fresh clone to a live pgfs mount with one
    command. The Postgres cluster, the mount point, the logs and the pid
    files all live under the project's `testdata/` directory, so nothing
    touches a system-wide Postgres and `rm -rf testdata` is a full reset."""

    deps = [
        ProvidesProjectLocalCluster,
        ProvidesLifecycleScript,
        RunsIntegrationSuite,
        ConnectsToPostgres,
    ]

    def title(self):
        return "Project-Local Development Environment"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Everything on this page ships today: `scripts/init_db.sh`, "
            "`scripts/pgfs.sh`, the Makefile targets and the integration "
            "suite in `scripts/test_*.sh`."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will go from a fresh checkout to a mounted
            pgfs filesystem, store a file in it, find that file as rows in
            PostgreSQL, and take everything down again. It takes about five
            minutes, most of which is the first compile.

            All commands run from the root of the repository.

            ## Before we start

            pgfs asks the kernel to remove its mount automatically if the
            daemon ever dies. Unprivileged users may only do that when
            `/etc/fuse.conf` allows it. Check:

            ```bash
            grep '^user_allow_other' /etc/fuse.conf
            ```

            If that prints nothing, enable it once:

            ```bash
            sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
            ```

            ## Step 1: enter the development shell

            ```bash
            nix develop
            ```

            The shell prints a banner with the Rust, PostgreSQL and FUSE
            versions it provides. Everything that follows uses those tools.

            ## Step 2: bring the filesystem up

            ```bash
            ./scripts/pgfs.sh up
            ```

            The first run creates a PostgreSQL cluster in `testdata/pgdata`,
            compiles pgfs, and mounts it. The last lines look like this:

            ```text
            pgfs:      starting (log: /path/to/pgfs/testdata/pgfs.log)
            mounted:   /path/to/pgfs/testdata/mnt
            postgres:  PostgreSQL 17.10 on
            ```

            Ask the script what is running:

            ```bash
            ./scripts/pgfs.sh status
            ```

            ```text
            mount:     active
            daemon:    running (pid 1519591)
            postgres:  running
            ```

            ## Step 3: store a file

            ```bash
            mkdir testdata/mnt/tutorial
            echo "hello from a database" > testdata/mnt/tutorial/hello.txt
            cat testdata/mnt/tutorial/hello.txt
            ```

            ```text
            hello from a database
            ```

            ## Step 4: find the file in PostgreSQL

            The file we just wrote is a row in the `entries` table:

            ```bash
            psql -h "$PWD/testdata" -d pgfs -c \
              "SELECT parent, name, kind, size FROM entries WHERE parent = 'tutorial'"
            ```

            ```text
              parent  |   name    | kind | size
            ----------+-----------+------+------
             tutorial | hello.txt | file |   22
            ```

            Its 22 bytes of content are a row in the `blocks` table:

            ```bash
            psql -h "$PWD/testdata" -d pgfs -c \
              "SELECT name, block_no, convert_from(data, 'UTF8') FROM blocks WHERE parent = 'tutorial'"
            ```

            ```text
               name    | block_no |     convert_from
            -----------+----------+-----------------------
             hello.txt |        0 | hello from a database+
                       |          |
            ```

            ## Step 5: take it down

            ```bash
            rm -r testdata/mnt/tutorial
            ./scripts/pgfs.sh down
            ./scripts/pgfs.sh status
            ```

            ```text
            mount:     none
            daemon:    not running
            postgres:  running
            ```

            PostgreSQL keeps running so the next `up` is quick.

            ## What we did

            We built pgfs, mounted it, wrote a file through the ordinary
            filesystem interface, and saw the same data as rows in two SQL
            tables. Next, the [Regular Files](flat_filesystem.md) tutorial
            explores reading, writing and truncating files in more depth.
        """)

    def how_to(self):
        return md(r"""
            ## Run the Rust unit tests

            Inside the Nix shell:

            ```bash
            make test
            ```

            This starts the project-local cluster if it is down
            (`scripts/ensure_db.sh`) and then runs `cargo test`. Most
            tests in `src/db.rs` talk to the live database. Outside the
            Nix shell, `make` refuses to run (`PGFS_ROOT` is unset); run
            `./scripts/ensure_db.sh && cargo test` instead.

            ## Run the integration suite

            ```bash
            ./scripts/pgfs.sh down        # the suite mounts testdata/mnt itself
            make test-integration
            ```

            Each `scripts/test_*.sh` prints `PASS` or `FAIL`. A failing
            test prints the tail of its log; the full output is in
            `testdata/<test>.log`. `test_replica.sh` needs Docker.

            To run a single test:

            ```bash
            bash scripts/test_rename.sh
            ```

            ## Run the daemon in the foreground

            ```bash
            RUST_LOG=pgfs=debug ./scripts/pgfs.sh run
            ```

            Logs go to the terminal. Ctrl+C unmounts cleanly.

            ## Mount somewhere other than `testdata/mnt`

            ```bash
            PGFS_MOUNT=/tmp/pgfs-demo ./scripts/pgfs.sh up
            PGFS_MOUNT=/tmp/pgfs-demo ./scripts/pgfs.sh down
            ```

            Pass the same `PGFS_MOUNT` to every `pgfs.sh` command.

            ## Use a PostgreSQL server other than the project-local one

            Run the binary directly and give it a libpq connection string:

            ```bash
            cargo build
            ./target/debug/pgfs --conn "host=db.internal port=5432 dbname=pgfs user=alice" /mnt/pgfs
            ```

            The role needs permission to create tables in that database
            the first time; pgfs creates its schema on startup.

            ## Inspect the database directly

            ```bash
            psql -h "$PWD/testdata" -d pgfs
            ```

            ## Stop PostgreSQL

            ```bash
            ./scripts/pgfs.sh down
            make db-stop
            ```

            Unmount first. A running daemon does not reconnect after
            PostgreSQL comes back, so every operation fails with
            `Input/output error` until the daemon is restarted.

            ## Reset everything

            ```bash
            ./scripts/pgfs.sh down
            make db-stop
            rm -rf testdata
            ```

            The next `./scripts/pgfs.sh up` creates a fresh cluster.

            ## Rebuild this documentation

            ```bash
            uv run python scripts/build_docs.py
            uv run mkdocs serve
            ```

            The pages are generated from the Diátaxis methods of the
            Feature classes in `spec/`; edit those, not `docs/`.

            ## Check the specification for drift

            ```bash
            uv run libspec diff
            ```
        """)

    def reference(self):
        return md(r"""
            ## Toolchain

            | Tool | Provided by `nix develop` | Used for |
            |---|---|---|
            | `rustc`, `cargo`, `clippy`, `rustfmt` | yes | building and checking the daemon |
            | PostgreSQL server binaries (`initdb`, `pg_ctl`, `psql`, `createdb`) | yes (PostgreSQL 17) | the project-local cluster |
            | FUSE 3 (`fusermount3`, headers, `pkg-config`) | yes | mounting |
            | Docker | no | replicated mode only (`scripts/replica_db.sh`) |
            | `uv` | no | the libspec specification and this documentation |

            ## `testdata/` layout

            | Path | Contents |
            |---|---|
            | `testdata/pgdata/` | PostgreSQL cluster created by `init_db.sh` |
            | `testdata/.s.PGSQL.5432` | PostgreSQL Unix socket |
            | `testdata/postgres.log` | PostgreSQL server log |
            | `testdata/mnt/` | default mount point |
            | `testdata/pgfs.pid` | PID of the daemon started by `pgfs.sh up` |
            | `testdata/pgfs.log` | daemon stdout and stderr (appended) |
            | `testdata/<test>.log` | output of each integration test |
            | `testdata/bench-pgfs.log`, `testdata/bench.pid` | `bench_profile.sh` daemon log and PID |

            `testdata/` is git-ignored.

            ## Project-local cluster

            | Setting | Value |
            |---|---|
            | Data directory | `testdata/pgdata` |
            | `initdb` flags | `--auth=trust --no-locale --encoding=UTF8` |
            | Listen addresses | Unix socket in `testdata/`; TCP `127.0.0.1:5432` |
            | Authentication | `trust` for local and loopback connections |
            | Roles | bootstrap superuser (owner of `pgdata`); a `SUPERUSER` role named after the invoking OS user if different |
            | Database | `pgfs` |
            | Daemon default `--conn` | `host=<current working directory>/testdata dbname=pgfs` |

            The default `--conn` is derived from the daemon's working
            directory. Started from anywhere other than the repository
            root, it needs an explicit `--conn`.

            ## Scripts

            | Script | Purpose |
            |---|---|
            | `scripts/init_db.sh` | create (if absent) and start the cluster; create role and database |
            | `scripts/ensure_db.sh` | exit 0 if the cluster answers, otherwise run `init_db.sh` |
            | `scripts/pgfs.sh up` | start Postgres if needed, `cargo build`, sweep a stale mount, start the daemon, wait up to 10 s for the mount |
            | `scripts/pgfs.sh down` | SIGTERM the daemon, wait up to 10 s, then `fusermount3 -u`, then report holders and lazy-unmount |
            | `scripts/pgfs.sh status` | report mount, daemon and Postgres state; warn about ghost mounts |
            | `scripts/pgfs.sh run` | build and exec the daemon in the foreground |
            | `scripts/run_tests.sh` | run every integration test in isolation (300 s timeout each) |
            | `scripts/test_*.sh` | individual integration tests; `test_lib.sh` is the shared harness |
            | `scripts/replica_db.sh` | manage the Docker streaming standby (see [Replicated Storage](replicated_filesystem.md)) |
            | `scripts/bench_profile.sh` | profiled benchmark (see [On-Demand Profiling](ondemandprofiling.md)) |
            | `scripts/build_docs.py` | regenerate `docs/` and the `mkdocs.yml` navigation from `spec/` |

            ## Integration tests

            | Test | Covers |
            |---|---|
            | `test_append.sh` | `O_APPEND` offset and size correctness |
            | `test_copy_performance.sh` | end-to-end copy throughput |
            | `test_dirs.sh` | deep nesting, listing, `rmdir` semantics |
            | `test_git_init.sh` | `git init` inside the mount |
            | `test_integrity.sh` | create/write/unlink chains |
            | `test_rename.sh` | rename-overwrite with POSIX semantics |
            | `test_replica.sh` | replicated mode end to end |
            | `test_truncate.sh` | truncate and extend semantics |

            ## Make targets

            | Target | Action |
            |---|---|
            | `make up` / `down` / `status` / `run` | `scripts/pgfs.sh` equivalents |
            | `make build` / `check` / `clippy` / `fmt` | `cargo build` / `check` / `clippy` / `fmt` |
            | `make test` | `ensure_db.sh`, then `cargo test` |
            | `make test-integration` | `scripts/run_tests.sh` |
            | `make init-db` | `scripts/init_db.sh` |
            | `make db-stop` | `pg_ctl -D testdata/pgdata stop` |
            | `make shell` | `nix develop` |
            | `make clean` | `cargo clean` |

            Every target except `down`, `status`, `db-stop`, `shell` and
            `clean` requires the Nix shell (`PGFS_ROOT` set).

            ## Environment variables

            | Variable | Read by | Effect |
            |---|---|---|
            | `PGFS_MOUNT` | `pgfs.sh` | mount point (default `testdata/mnt`) |
            | `PGFS_ROOT` | `Makefile` | set by the Nix shell; `make` refuses to run without it |
            | `RUST_LOG` | daemon | log filter, e.g. `pgfs=debug` (default `pgfs=info`) |
            | `RUST_LOG_FORMAT` | daemon | `json` for newline-delimited JSON logs |
            | `RUST_BACKTRACE` | daemon | backtrace detail in crash reports |
        """)

    def explanation(self):
        return md(r"""
            ## Why everything lives in `testdata/`

            A filesystem backed by a database needs a database, and the
            usual way to get one is to install a system PostgreSQL service
            and administer it. pgfs deliberately avoids that. The cluster,
            its socket, its logs, the mount point and the daemon's
            bookkeeping all live in one git-ignored directory inside the
            checkout.

            That buys three things. Nothing on the machine changes outside
            the project, so it is safe to experiment. Two checkouts never
            interfere with each other. And recovery from any mess is one
            command, `rm -rf testdata`, because there is no state anywhere
            else.

            ## Trust authentication, and its limits

            The cluster uses `trust` authentication: any connection that
            reaches it is accepted without a password. The Unix socket sits
            in a directory only the developer can enter, so for socket
            connections that is no weaker than file permissions. The TCP
            listener is different. It only binds 127.0.0.1, but any user on
            the same machine can reach loopback. On a single-user
            development machine that is acceptable. On a shared host it is
            not, and the cluster should not be run there as is.

            TCP exists at all because physical replication needs it:
            `pg_basebackup` and the standby's WAL receiver connect to the
            primary over the network (see [Replicated
            Storage](../explanation/replicated_filesystem.md)).

            ## Why a lifecycle script instead of raw commands

            A FUSE mount has more states than "up" and "down". The daemon
            can be dead while the kernel still shows the mount. The mount
            can be busy because a shell has its working directory inside
            it. Postgres can be stopped while the daemon is starting.
            `pgfs.sh` treats `/proc/mounts` as the source of truth and
            escalates through progressively stronger actions (signal, then
            `fusermount3 -u`, then a lazy unmount), so the common failure
            states resolve themselves. The reasoning behind clean unmounts
            is covered in [Reliable Mount](../explanation/reliablemount.md).

            `pgfs.sh` builds and runs the debug binary. Compiles are faster,
            and in debug builds a panic unwinds and prints a full report
            rather than aborting.

            ## Why the Makefile insists on the Nix shell

            pgfs links against libfuse and talks to a specific PostgreSQL
            major version. A build against the wrong libfuse headers, or an
            `initdb` from a different major version than the server, fails
            in confusing ways. The `check-nix` guard turns that into one
            clear message with a fix.

            ## Where the documentation comes from

            These pages are generated. Every user-visible capability of
            pgfs is a `Feature` class in `spec/` that inherits
            `libspec.diataxis.Diataxis` and must implement four methods,
            one per documentation quadrant. `scripts/build_docs.py` renders
            them, together with each feature's requirement classes, into
            this site. Documentation that lives next to the specification
            is reviewed in the same change as the behaviour it describes.
        """)


class FlatFilesystem(Feat):
    """Users can create, read, write, and delete files through standard
    POSIX file operations on the mounted directory."""

    deps = [
        ConnectsToPostgres,
        CreatesFiles,
        ReadsFiles,
        WritesFiles,
        TruncatesFiles,
        UnlinksFiles,
        LooksUpEntries,
        ReportsAttributes,
        DistinguishesExpectedErrors,
    ]

    def title(self):
        return "Regular Files"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Create, read, write, append, truncate and delete are implemented "
            "and covered by unit and integration tests. Permissions, "
            "ownership and timestamps are reported but not settable (see "
            "Reference)."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will create a file, read it, add to it, cut
            it short, grow it again, and then look at how its bytes are laid
            out in PostgreSQL. By the end we will have seen every basic file
            operation pgfs supports.

            We need a running mount. If `./scripts/pgfs.sh status` does not
            say `mount: active`, follow [Your first pgfs
            mount](projectlocalenvironment.md) first, up to Step 2.

            ## Step 1: make a workspace

            ```bash
            mkdir testdata/mnt/files
            cd testdata/mnt/files
            ```

            ## Step 2: create and read a file

            ```bash
            echo 'Hello pgfs' > demo.txt
            cat demo.txt
            ```

            ```text
            Hello pgfs
            ```

            ## Step 3: append to it

            ```bash
            echo 'More content' >> demo.txt
            cat demo.txt
            wc -c demo.txt
            ```

            ```text
            Hello pgfs
            More content
            24 demo.txt
            ```

            ## Step 4: cut it short, then grow it

            ```bash
            truncate -s 5 demo.txt
            cat demo.txt; echo
            ```

            ```text
            Hello
            ```

            Now grow the file to 10 bytes. The new bytes read back as
            zeros:

            ```bash
            truncate -s 10 demo.txt
            od -c demo.txt
            ```

            ```text
            0000000   H   e   l   l   o  \0  \0  \0  \0  \0
            0000012
            ```

            ## Step 5: write something bigger

            ```bash
            head -c 200000 /dev/urandom > big.bin
            cp big.bin copy.bin
            cmp big.bin copy.bin && echo identical
            ```

            ```text
            identical
            ```

            ## Step 6: look at the bytes in PostgreSQL

            From the repository root, in a second terminal:

            ```bash
            psql -h "$PWD/testdata" -d pgfs -c \
              "SELECT name, block_no, length(data) FROM blocks WHERE parent = 'files' ORDER BY name, block_no"
            ```

            ```text
               name   | block_no | length
            ----------+----------+--------
             big.bin  |        0 |  65536
             big.bin  |        1 |  65536
             big.bin  |        2 |  65536
             big.bin  |        3 |   3392
             copy.bin |        0 |  65536
             ...
             demo.txt |        0 |      5
            ```

            `big.bin` is four rows: three full 64 KiB blocks and a short
            last one. `demo.txt` is a single 5-byte block. Its declared
            size is 10, and the five missing bytes are the zeros we saw in
            Step 4.

            ## Step 7: clean up

            ```bash
            cd ../../..
            rm -r testdata/mnt/files
            ```

            ## What we did

            We used ordinary shell tools to exercise create, read, write,
            append, truncate and delete, and saw that a file's contents are
            stored as fixed-size blocks. To learn why it is built this
            way, read the [explanation](../explanation/flat_filesystem.md).
            Next, [Directory Trees](hierarchicaldirectories.md).
        """)

    def how_to(self):
        return md(r"""
            All paths below are inside a mounted pgfs filesystem.

            ## Create an empty file

            ```bash
            touch notes.txt
            ```

            ## Replace a file's contents

            ```bash
            printf 'new contents\n' > notes.txt
            ```

            To replace contents so that readers see either the old or the
            new version and never a partial one, write a temporary file and
            rename it over the target (see [Atomic
            Rename](atomicrename.md)):

            ```bash
            printf 'new contents\n' > .notes.txt.tmp && mv .notes.txt.tmp notes.txt
            ```

            ## Append

            ```bash
            echo 'another line' >> notes.txt
            ```

            ## Overwrite bytes in the middle of a file

            ```bash
            printf 'XY' | dd of=notes.txt bs=1 seek=4 conv=notrunc
            ```

            ## Make written data visible outside the mount

            Writes are held in memory per open file and committed to
            PostgreSQL when the file is closed, flushed or fsynced.
            Processes reading through the same mount always see their own
            and each other's writes, because a read flushes pending writes
            first. A SQL client, or a second pgfs daemon on the same
            database, sees the data once the writer closes the file or
            calls `fsync`.

            ## Truncate or extend a file

            ```bash
            truncate -s 0 notes.txt      # empty it
            truncate -s 1M notes.txt     # extend with zeros
            ```

            ## Create a sparse file

            ```bash
            truncate -s 1G sparse.img
            ```

            Ranges that were never written have no block rows and read as
            zeros, so this costs one row in `entries` and nothing in
            `blocks`.

            ## Delete a file

            ```bash
            rm notes.txt
            ```

            ## Find a file's rows in PostgreSQL

            A file at `docs/sub/notes.txt` has `parent = 'docs/sub'` and
            `name = 'notes.txt'`:

            ```sql
            SELECT kind, size, mtime FROM entries WHERE parent = 'docs/sub' AND name = 'notes.txt';
            SELECT block_no, length(data) FROM blocks WHERE parent = 'docs/sub' AND name = 'notes.txt' ORDER BY block_no;
            ```

            Files at the top of the mount have `parent = ''`.

            ## Diagnose an `Input/output error`

            EIO means something unexpected failed, usually the database.
            The full error chain is in the daemon log:

            ```bash
            grep ERROR testdata/pgfs.log | tail -5
            ```

            Each entry names the operation, the source location and the
            root cause; see [Observability](observabledaemon.md).
        """)

    def reference(self):
        return md(r"""
            ## FUSE operations

            | Operation | Triggered by | Behaviour | Errors |
            |---|---|---|---|
            | `lookup` | path resolution | reads the entry's attributes; assigns an inode number on first sight | `ENOENT` (absent, or name not valid UTF-8) |
            | `getattr` | `stat`, `ls -l` | reads attributes; the root (inode 1) is synthesised | `ENOENT` |
            | `create` | `open(O_CREAT)` | checks the primary for an existing entry, inserts a `file` row with size 0, opens a write handle | `EEXIST` |
            | `open` | `open` | allocates a handle with an empty dirty-block buffer; flags and permissions are not checked | `ENOENT` |
            | `read` | `read`, `pread` | flushes the handle's pending writes, then fetches only the blocks covering the range; the range is clamped to the file size; holes read as zeros | `ENOENT` |
            | `write` | `write`, `pwrite` | with an open handle, buffers the bytes in memory and returns immediately; without one, writes the touched blocks in one transaction; extends the size if the write ends past it | `ENOENT` |
            | `flush`, `fsync`, `release` | `close`, `fsync` | writes the handle's dirty blocks and new size in one transaction | `EIO` |
            | `setattr` | `truncate`, `ftruncate`, `open(O_TRUNC)` | size changes truncate or extend in one transaction; mode, owner and time changes are accepted and ignored | `EISDIR` (size change on a directory), `ENOENT` |
            | `unlink` | `rm` | deletes the entry row, then its block rows; forgets the inode mapping | `EIO` |

            ## Storage schema

            ```sql
            CREATE TABLE entries (
                ino        bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                parent_ino bigint NOT NULL DEFAULT 1,
                parent     text   NOT NULL DEFAULT '',
                name       text   NOT NULL,
                kind       text   NOT NULL DEFAULT 'file',   -- 'file' | 'dir'
                data       bytea  NOT NULL DEFAULT '',       -- legacy, always empty
                size       bigint NOT NULL DEFAULT 0,
                mode       integer NOT NULL DEFAULT 33188,
                uid        integer NOT NULL DEFAULT 1000,
                gid        integer NOT NULL DEFAULT 1000,
                atime      timestamptz NOT NULL DEFAULT now(),
                mtime      timestamptz NOT NULL DEFAULT now(),
                ctime      timestamptz NOT NULL DEFAULT now(),
                CONSTRAINT unique_parent_name UNIQUE (parent, name)
            );

            CREATE TABLE blocks (
                ino      bigint  NOT NULL DEFAULT 1,
                parent   text    NOT NULL DEFAULT '',
                name     text    NOT NULL DEFAULT '',
                block_no integer NOT NULL,
                data     bytea   NOT NULL,
                PRIMARY KEY (parent, name, block_no)
            );
            ```

            Indexes: `entries(parent_ino)`, `entries(parent)`,
            `blocks(ino, block_no)`. The schema is created on first
            connect with `CREATE TABLE IF NOT EXISTS`, inside one
            transaction that also inserts the root row
            (`ino = 1, parent = '', name = ''`, `kind = 'dir'`).

            | Property | Value |
            |---|---|
            | Block size | 65 536 bytes (`BLOCK_SIZE`) |
            | Block number | `offset / 65536` |
            | Last block | may be shorter than 65 536 bytes |
            | Holes | blocks with no row; read as zeros |
            | Source of truth for size | `entries.size` |
            | `entries.ino`, `parent_ino`, `mode`, `uid`, `gid`, `atime`, `ctime` | written with defaults; not read by the FUSE path |

            ## Reported attributes

            | Attribute | Regular file |
            |---|---|
            | `perm` | `0644` |
            | `nlink` | 1 |
            | `uid`, `gid` | the daemon's own uid and gid |
            | `size` | `entries.size` |
            | `blocks` | `ceil(size / 512)` |
            | `blksize` | 4096 |
            | `atime`, `mtime`, `ctime`, `crtime` | all equal to `entries.mtime` |

            Attribute and entry replies carry the TTL set by `--attr-ttl`
            (default 1 s).

            ## Inode numbers

            Inode numbers are assigned in memory, per mount, the first time
            the kernel sees a path. They start at 2, increase by one, are
            never reused within a mount, and survive renames. They are not
            persisted: a remount assigns new numbers. They are unrelated to
            `entries.ino`.

            ## Durability

            Every connection runs `SET synchronous_commit = off`. A
            transaction is acknowledged once it is in PostgreSQL's WAL
            buffers, before the WAL is flushed to disk. `fsync` on a pgfs
            file waits for the commit, not for the disk flush. If
            PostgreSQL crashes, the most recently acknowledged commits can
            be lost; with default settings that window is under one second.
            Committed data is never partially applied. Buffered writes that
            have not reached `flush`, `fsync` or `close` are lost if the
            daemon dies.

            ## Error mapping

            | errno | Meaning |
            |---|---|
            | `ENOENT` | the entry, or the inode's path, does not exist |
            | `EEXIST` | `O_CREAT|O_EXCL` on an existing name |
            | `EISDIR` | size change requested on a directory |
            | `EIO` | unexpected failure (database error, invariant violation); the full error chain is logged at ERROR |

            ## Not supported

            | Operation | Result |
            |---|---|
            | symbolic links (`symlink`) | `EPERM` from `ln -s` (operation not implemented) |
            | hard links (`link`) | `EPERM` from `ln` (operation not implemented) |
            | FIFOs, sockets, device nodes (`mknod`) | `ENOSYS` |
            | extended attributes | not implemented |
            | `chmod`, `chown`, `utimes`, `touch -d` | succeed but have no effect |
            | `statfs` (`df`) | reports zero size and zero free space |
            | `fallocate` | `EOPNOTSUPP` (see [Kernel Offloading](kerneloffloadingacceleration.md)) |
            | reflink clone (`cp --reflink=always`) | `EOPNOTSUPP` |
        """)

    def explanation(self):
        return md(r"""
            ## A file is a handful of rows

            pgfs stores each file as one row in `entries`, which holds its
            name, kind, size and modification time, plus zero or more rows
            in `blocks`, each holding up to 64 KiB of its contents. Both
            are keyed by the file's location: the path of the directory
            that contains it (`parent`) and its own `name`. A file at
            `docs/sub/notes.txt` is the row `('docs/sub', 'notes.txt')`.

            Path-based keys make the tables readable by a person with
            `psql`. Ordinary SQL answers
            questions like "which files under `docs/` changed today". The
            cost of path keys appears when a directory is renamed, which is
            discussed in [Atomic Rename](atomicrename.md).

            ## Why blocks

            The first version of pgfs stored a whole file in one `bytea`
            column; `entries.data` is the remnant. Every write then meant
            reading the whole file, changing a few bytes, and writing the
            whole file back. Appending a line to a 100 MB log rewrote
            100 MB.

            Fixed-size blocks bound that work. A write touches only the
            blocks it overlaps, and a read fetches only the blocks it
            covers. The block size is a trade-off. Smaller blocks mean less
            rewriting for small writes but more rows and more per-row
            overhead for large files. 64 KiB keeps the row count low while
            keeping a partial-block rewrite cheap. PostgreSQL compresses
            and moves values of that size out of line through TOAST on its
            own.

            Blocks that were never written simply do not exist, so sparse
            files are free. A read fills the gaps with zeros, which is
            exactly what POSIX requires of a hole.

            ## Why writes are buffered per open file

            The kernel delivers a large `write` to FUSE in pieces. Turning
            each piece into its own PostgreSQL transaction would make the
            commit round-trip, not the data, the bottleneck. pgfs instead
            collects writes in memory, per open file, as a set of dirty
            blocks, and commits them all in one transaction when the file
            is flushed, fsynced or closed.

            This is close-to-open consistency, the model NFS uses. A writer
            and anything that opens the file after the writer closes it see
            the same bytes. Readers using the same mount see the data even
            sooner, because a read first flushes that file's pending
            writes.

            ## Durability is a deliberate trade

            pgfs turns off PostgreSQL's `synchronous_commit`. Commits
            return without waiting for the WAL to reach disk, which removes
            the largest fixed cost from every metadata operation. In
            exchange, a PostgreSQL crash can lose the last fraction of a
            second of acknowledged work. This is similar to how a local
            filesystem's page cache behaves without `fsync`, except that
            here even `fsync` does not close the gap. Transactions remain
            atomic: a lost commit disappears entirely and never leaves a
            half-written file. [Performance
            Optimizations](optimizationroadmap.md) covers this and the
            other trade-offs in more depth.

            ## What the prototype does not model

            pgfs reports fixed permissions (`0644` for files) and the
            daemon's own uid and gid, and ignores `chmod`, `chown` and
            explicit timestamps. It has no symbolic or hard links.
            Inode numbers live only in memory. These are scoping
            decisions, not accidents. The schema already carries `mode`,
            `uid`, `gid` and a persistent `ino` column for the planned
            inode-keyed storage engine described in [Dual-Interface
            Architecture](dualinterfacearchitecture.md).
        """)


class HierarchicalDirectories(Feat):
    """Users can create nested directories, list their contents, and
    remove empty directories through standard POSIX directory
    operations."""

    deps = [
        CreatesDirectories,
        RemovesDirectories,
        ListsDirectories,
        LooksUpEntries,
        ReportsAttributes,
    ]

    def title(self):
        return "Directory Trees"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "`mkdir`, `rmdir`, `readdir` and nested paths of any depth are "
            "implemented and covered by `test_dirs.sh`. Directory "
            "timestamps are not tracked."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will build a small directory tree, walk it
            with standard tools, see how it is stored, and take it apart
            again. We will also meet the one rule directories enforce:
            they must be empty before they can be removed.

            We need a running mount; see [Your first pgfs
            mount](projectlocalenvironment.md) if `./scripts/pgfs.sh
            status` does not say `mount: active`.

            ## Step 1: build a tree

            ```bash
            cd testdata/mnt
            mkdir -p project/src/util project/docs
            touch project/src/main.rs project/src/util/strings.rs project/docs/README.md
            ```

            ## Step 2: walk it

            ```bash
            find project | sort
            ```

            ```text
            project
            project/docs
            project/docs/README.md
            project/src
            project/src/main.rs
            project/src/util
            project/src/util/strings.rs
            ```

            ```bash
            ls -la project/src
            ```

            The listing starts with the `.` and `..` entries, followed by
            `main.rs` and `util`.

            ## Step 3: see how the tree is stored

            From the repository root, in a second terminal:

            ```bash
            psql -h "$PWD/testdata" -d pgfs -c \
              "SELECT parent, name, kind FROM entries
               WHERE parent = 'project' OR starts_with(parent, 'project/')
                  OR (parent = '' AND name = 'project')
               ORDER BY parent, name"
            ```

            ```text
                  parent       |    name    | kind
            -------------------+------------+------
                               | project    | dir
             project           | docs       | dir
             project           | src        | dir
             project/docs      | README.md  | file
             project/src       | main.rs    | file
             project/src       | util       | dir
             project/src/util  | strings.rs | file
            ```

            Every row names the directory it lives in. `project` itself
            lives at the top of the mount, where `parent` is empty.

            ## Step 4: try to remove a directory that is not empty

            ```bash
            rmdir project/src
            ```

            ```text
            rmdir: failed to remove 'project/src': Directory not empty
            ```

            ## Step 5: take the tree apart

            Empty the directory from the bottom up, and `rmdir` succeeds:

            ```bash
            rm project/src/util/strings.rs
            rmdir project/src/util
            rm project/src/main.rs
            rmdir project/src
            ```

            Remove the rest in one go:

            ```bash
            rm -r project
            ls
            ```

            `project` no longer appears.

            ## What we did

            We created nested directories, listed them, saw that each
            entry stores the path of its containing directory, and learned
            that `rmdir` only removes empty directories. Next, [Atomic
            Rename](atomicrename.md) moves whole trees in one step.
        """)

    def how_to(self):
        return md(r"""
            ## Create nested directories

            ```bash
            mkdir -p a/b/c
            ```

            ## List a directory, including hidden entries

            ```bash
            ls -la a
            ```

            ## Remove an empty directory

            ```bash
            rmdir a/b/c
            ```

            ## Remove a directory and everything in it

            ```bash
            rm -r a
            ```

            ## Find out why `rmdir` says "Directory not empty"

            Hidden files count. List everything:

            ```bash
            ls -A a/b
            ```

            Or ask the database directly; the result is the full path of
            every child of `a/b`:

            ```sql
            SELECT parent || '/' || name FROM entries WHERE parent = 'a/b';
            ```

            ## Count everything below a directory

            ```sql
            SELECT kind, count(*) FROM entries
            WHERE parent = 'a' OR starts_with(parent, 'a/')
            GROUP BY kind;
            ```

            Use `starts_with` rather than `LIKE 'a/%'`. In a `LIKE`
            pattern, `_` and `%` in directory names act as wildcards.

            ## Find the largest directories by total file size

            ```sql
            SELECT parent, sum(size) AS bytes, count(*) AS files
            FROM entries WHERE kind = 'file'
            GROUP BY parent ORDER BY bytes DESC LIMIT 10;
            ```
        """)

    def reference(self):
        return md(r"""
            ## FUSE operations

            | Operation | Behaviour | Errors |
            |---|---|---|
            | `mkdir` | checks the primary for an existing entry, then inserts a `dir` row | `EEXIST`, `ENOENT` (parent unknown), `EIO` |
            | `rmdir` | checks the primary that the entry exists, then that no row has it as `parent`, then deletes it | `ENOENT`, `ENOTEMPTY`, `EIO` |
            | `readdir` | returns `.`, `..`, then every child ordered by name | `ENOENT`, `EIO` |
            | `lookup` | resolves `(parent inode, name)` to attributes | `ENOENT` |

            The emptiness check and the delete in `rmdir` are separate
            statements, not one transaction.

            ## `readdir` offsets

            Entries are numbered from 1 in the order returned: `.` is 1,
            `..` is 2, and children follow sorted by name. The kernel
            resumes a partial listing by passing back the last number it
            received. Offsets index the current list, so if the directory
            changes between two calls of one listing, entries can be
            skipped or repeated.

            ## Directory attributes

            | Attribute | Value |
            |---|---|
            | `perm` | `0755` |
            | `nlink` | 2, regardless of the number of subdirectories |
            | `size`, `blocks` | 0 |
            | `uid`, `gid` | the daemon's own uid and gid |
            | `atime`, `mtime`, `ctime`, `crtime` | the Unix epoch (shown as 1970-01-01, or 1969-12-31 west of UTC) |

            ## Root directory

            | Property | Value |
            |---|---|
            | Inode | 1 |
            | Path | `''` (empty string) |
            | Row | `parent = ''`, `name = ''`, `kind = 'dir'`, inserted at schema creation |
            | Attributes | always synthesised; never read from the database |
            | Listing | excludes the root's own row |

            ## Paths and names

            | Rule | Detail |
            |---|---|
            | Key | `(parent, name)`, unique |
            | Full path | `parent || '/' || name`, or `name` when `parent = ''` |
            | Valid names | any UTF-8 string without `/` or NUL; non-UTF-8 names are rejected with `ENOENT` |
            | Depth | not limited by pgfs. The unique index on `(parent, name)` limits one key to about 2.7 kB, so a longer combined path fails with `EIO`. |
            | Index used by listings | `entries(parent)` |
        """)

    def explanation(self):
        return md(r"""
            ## Directories are rows too

            A directory is an `entries` row with `kind = 'dir'` and no
            blocks. What makes the tree a tree is the `parent` column:
            every row stores the full path of the directory that contains
            it. Listing a directory is a single indexed query,
            `WHERE parent = 'project/src'`, and finding everything below a
            directory is a prefix match on that column.

            This is the materialised-path pattern. Reading is easy:
            listings, subtree queries and path lookups are each one
            indexed query, and the rows are readable on their own. Moving
            things is harder, because a directory's path is copied into
            every descendant row, so renaming the directory rewrites them
            all. [Atomic Rename](../explanation/atomicrename.md) explains
            how pgfs keeps that atomic, and [Performance
            Optimizations](../explanation/optimizationroadmap.md) describes
            the planned inode-keyed layout that would make it
            constant-time.

            ## The root

            The root has no parent, so it is stored as the row with an
            empty `parent` and an empty `name`, created together with the
            schema and pinned to inode 1, which FUSE requires. Its
            attributes are synthesised and never fetched, so the root can
            always be listed, even when the database is unreachable.

            ## What directories do not track

            On a local filesystem a directory's link count is two plus its
            number of subdirectories, and its modification time changes
            whenever an entry is added or removed. pgfs reports a fixed
            link count of 2 and the epoch as every timestamp. Tracking
            either would mean updating the parent's row on every create,
            delete and rename, adding a write to operations that are
            currently a single statement. Few programs depend on these
            values; tools that do (backup software comparing directory
            mtimes, for example) treat every pgfs directory as unchanged.

            ## Why `rmdir` refuses non-empty directories

            POSIX requires it, and it is a safety feature. `rm -r` is the
            explicit way to delete a subtree, and it walks the tree
            bottom-up, removing each entry through the normal unlink and
            rmdir paths.
        """)


class AtomicRename(Feat):
    """Users can rename (move) files and directories with standard
    `mv` semantics, including overwriting existing files and empty
    directories."""

    deps = [RenamesEntries, RejectsInvalidRenames, SupportsGitInit]

    def title(self):
        return "Atomic Rename"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "File and directory renames, overwrite semantics and the full "
            "POSIX error matrix are implemented and covered by "
            "`test_rename.sh` and `test_git_init.sh`. `renameat2` flags "
            "(`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) are rejected."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will rename files and whole directory
            trees, overwrite a file atomically, see which renames pgfs
            refuses, and finish by running git (which depends on atomic
            rename) inside the mount.

            We need a running mount; see [Your first pgfs
            mount](projectlocalenvironment.md).

            ## Step 1: rename over an existing file

            ```bash
            mkdir testdata/mnt/rename && cd testdata/mnt/rename
            echo v1 > a.txt
            echo v2 > b.txt
            mv a.txt b.txt
            cat b.txt
            ls
            ```

            ```text
            v1
            b.txt
            ```

            `b.txt` now holds what `a.txt` held, and `a.txt` is gone.

            ## Step 2: move a directory with everything in it

            ```bash
            mkdir -p old/inner
            echo deep > old/inner/file.txt
            mv old new
            cat new/inner/file.txt
            ```

            ```text
            deep
            ```

            ## Step 3: see what pgfs refuses

            A directory cannot move inside itself:

            ```bash
            mv new new/inner/x
            ```

            ```text
            mv: cannot move 'new' to a subdirectory of itself, 'new/inner/x'
            ```

            A directory cannot replace a non-empty directory:

            ```bash
            mkdir -p full && touch full/f
            mkdir empty
            mv -T empty full
            ```

            ```text
            mv: cannot overwrite 'full': Directory not empty
            ```

            ## Step 4: run git

            Git takes a lock by creating `config.lock`, writes the new
            configuration into it, and renames it over `config`. Each
            commit does the same with branch references.

            ```bash
            git init -q repo && cd repo
            echo hello > README
            git add README
            git -c user.name=Tutorial -c user.email=tutorial@example.com commit -qm "first commit"
            git log --oneline
            ```

            ```text
            511658a first commit
            ```

            Your commit hash will differ.

            ## Step 5: clean up

            ```bash
            cd ../../../..
            rm -rf testdata/mnt/rename
            ```

            ## What we did

            We replaced a file by renaming over it, moved a populated
            directory in one step, saw the two renames POSIX forbids, and
            ran a git workflow that depends on all of it. The
            [explanation](../explanation/atomicrename.md) describes how one
            database transaction makes this atomic.
        """)

    def how_to(self):
        return md(r"""
            ## Replace a file without exposing a partial write

            Write to a temporary name in the same directory, close it, then
            rename:

            ```bash
            generate-report > .report.tmp && mv .report.tmp report.txt
            ```

            Readers see the old `report.txt` or the new one, never a mix.
            Close the temporary file before renaming it; see Reference for
            why.

            ## Move a directory tree

            ```bash
            mv projects/2024 archive/2024
            ```

            The whole subtree moves in one transaction, however many
            entries it contains. Large trees take time proportional to
            their size.

            ## Replace an empty directory with another directory

            ```bash
            mv -T staging live      # `live` must be empty
            ```

            ## Rename without overwriting

            ```bash
            mv -n draft.txt final.txt
            ```

            GNU `mv -n` first tries `renameat2(RENAME_NOREPLACE)`. pgfs
            rejects that flag with `EINVAL`, and `mv` falls back to
            checking for the target itself. The result is correct but not
            atomic against a concurrent creator.

            ## Swap two entries

            `mv --exchange` (`RENAME_EXCHANGE`) is not supported and fails
            with `Invalid argument`. Use three renames through a temporary
            name.

            ## Check that a rename landed in the database

            ```sql
            SELECT parent, name, kind FROM entries
            WHERE parent = 'archive' AND name = '2024';
            ```
        """)

    def reference(self):
        return md(r"""
            ## Outcome matrix

            | Source | Target | Result |
            |---|---|---|
            | file | absent | moved |
            | file | file | target atomically replaced |
            | file | directory | `EISDIR` |
            | directory | absent | moved with all descendants |
            | directory | empty directory | target atomically replaced |
            | directory | non-empty directory | `ENOTEMPTY` |
            | directory | file | `ENOTDIR` |
            | directory | inside its own subtree | `EINVAL` |
            | any | itself | success, no change |
            | absent | any | `ENOENT` |
            | any, with `renameat2` flags ≠ 0 | any | `EINVAL` |

            ## Transaction

            Every check that decides the outcome reads the primary
            database, never a replica. A successful rename is one
            transaction:

            1. If the target exists, delete its `entries` row and its
               `blocks` rows.
            2. If the source is a directory, rewrite `parent` in every
               `entries` and `blocks` row below it, replacing the old path
               prefix with the new one.
            3. Update the source's `entries` row to the new
               `(parent, name)` and set its `mtime`.
            4. Move the source's `blocks` rows to the new `(parent, name)`.
            5. Commit. If no row moved, roll back and reply `EIO`.

            After the commit, the in-memory path ↔ inode maps are re-keyed
            for the entry and its whole subtree, so inode numbers the
            kernel already holds stay valid.

            ## Cost

            | Rename | Rows written |
            |---|---|
            | file | 1 entry + its blocks |
            | directory | 1 entry + every descendant entry + every descendant block |

            ## Open files

            Pending writes in an open file's buffer are tied to the path
            the file had when it was opened. Close or `fsync` a file
            before renaming it, otherwise those writes are not carried
            over to the new name.

            ## Isolation

            Concurrent SQL readers, and other pgfs daemons on the same
            database, see the tree either entirely before or entirely
            after the rename (PostgreSQL `READ COMMITTED`).
        """)

    def explanation(self):
        return md(r"""
            ## Why rename has to be atomic

            Rename is how Unix programs publish changes safely. An editor
            saves by writing a temporary file and renaming it over the
            original. Package managers install files the same way. Git
            guards every configuration change and every branch update with
            a lock file that is renamed into place. All of them rely on one
            guarantee: at every instant the target name refers to a
            complete file, either the old one or the new one.

            A filesystem that implemented rename as "delete the target,
            then move the source" would open a window in which the name
            refers to nothing. A crash in that window loses the file. So
            rename was the first pgfs feature that had to be transactional
            rather than merely correct.

            ## One transaction, every row

            In pgfs every entry stores the full path of its parent
            directory. Moving a file therefore changes one `entries` row
            and its blocks. Moving a directory changes every row beneath
            it, because every one of those rows has the old path baked in.

            pgfs does all of it, including deleting an overwritten target,
            in a single PostgreSQL transaction. PostgreSQL then supplies
            the guarantees. Atomicity: after a crash the rename either
            happened completely or not at all. Isolation: any other
            connection sees the tree only before or after the move. The
            daemon's own in-memory inode map is updated only after the
            commit succeeds, so a failed rename leaves no trace.

            ## The price of materialised paths

            Rewriting every descendant makes the cost of a directory rename
            proportional to the size of its subtree. Renaming a directory
            holding a million files rewrites a million rows. Local
            filesystems avoid this by naming directories with inode numbers
            instead of paths, which makes a move a single pointer update.
            The schema already carries `ino` and `parent_ino` columns for
            exactly that change, described as `InoBasedRekeyingReq` in
            [Performance Optimizations](optimizationroadmap.md).

            ## Why `git` is the test

            `git init` exercises rename-overwrite immediately: it creates
            `config`, then rewrites it several times through
            `config.lock`. It was also the first real program run against
            pgfs, and it failed until directories were implemented.
            Git's workflow, made of lock files, renames and readers that
            expect complete files, is a compact stand-in for the guarantees
            real applications need, so `test_git_init.sh` is part of the
            integration suite.

            ## Why `renameat2` flags are refused

            `RENAME_NOREPLACE` and `RENAME_EXCHANGE` need their own atomic
            implementations: "fail if the target exists" and "swap two
            entries". Rather than approximate them, pgfs rejects them with
            `EINVAL`, which well-behaved tools such as GNU `mv` already
            handle by falling back to plain rename.
        """)


class ReliableMount(Feat):
    """The filesystem mounts and unmounts cleanly. Crashing or killing
    the daemon never leaves a ghost mount behind."""

    deps = [
        MountsCleanly,
        HandlesShutdownSignals,
        DetectsDeadlock,
        WritesHealthFiles,
        ProvidesLifecycleScript,
    ]

    def title(self):
        return "Reliable Mount & Shutdown"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "AutoUnmount, signal-driven clean unmount, the deadlock "
            "watchdog, health files and the `pgfs.sh` escalation path are "
            "all implemented."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will start pgfs, find its health files,
            stop it politely, then kill it without warning and see that
            the mount still disappears. Along the way we will see the three
            layers that keep a mount from becoming a ghost.

            ## Step 1: start the daemon and find its health files

            ```bash
            ./scripts/pgfs.sh up
            grep 'health pid file' testdata/pgfs.log | tail -1
            ```

            ```text
            health pid file: /tmp/pgfs-5049eced64c2821d.pid
            ```

            The hash in the name is derived from the mount point. Next to
            the PID file is a readiness file:

            ```bash
            ls /tmp/pgfs-*.pid /tmp/pgfs-*.ready
            cat /tmp/pgfs-*.pid
            ```

            The PID it contains matches `./scripts/pgfs.sh status`.

            ## Step 2: stop it politely

            ```bash
            ./scripts/pgfs.sh down
            ```

            ```text
            pgfs:      stopping (pid 1519591)
            unmounted:  /path/to/pgfs/testdata/mnt
            ```

            The script sent SIGTERM. The daemon unmounted itself, removed
            its health files and exited:

            ```bash
            grep 'received signal' testdata/pgfs.log | tail -1
            ls /tmp/pgfs-*.pid 2>/dev/null || echo "no health files"
            ```

            ## Step 3: kill it without warning

            ```bash
            ./scripts/pgfs.sh up
            kill -9 "$(cat testdata/pgfs.pid)"
            sleep 1
            grep testdata/mnt /proc/mounts || echo "mount is gone"
            ```

            ```text
            mount is gone
            ```

            SIGKILL cannot be caught, so the daemon got no chance to clean
            up. The kernel removed the mount anyway, because pgfs asked for
            AutoUnmount when it mounted.

            ## Step 4: confirm the state is clean

            ```bash
            ./scripts/pgfs.sh status
            ```

            ```text
            mount:     none
            daemon:    not running
            postgres:  running
            ```

            Because the daemon was killed, its `/tmp` health files are
            still there. They are harmless and are overwritten on the next
            start.

            ## What we did

            We saw a clean, signal-driven shutdown and an uncatchable kill,
            and both ended with no mount left behind. The
            [explanation](../explanation/reliablemount.md) describes why
            each layer exists.
        """)

    def how_to(self):
        return md(r"""
            ## Stop the daemon cleanly

            ```bash
            ./scripts/pgfs.sh down
            ```

            Or signal it directly; SIGINT, SIGTERM and SIGHUP all unmount
            cleanly:

            ```bash
            kill -TERM "$(cat testdata/pgfs.pid)"
            ```

            ## Wait for the mount to be ready (supervisors, scripts)

            Poll for the readiness file. Its name is printed at startup
            next to the PID file:

            ```bash
            until [ -e /tmp/pgfs-<hash>.ready ]; do sleep 0.1; done
            ```

            Or poll `/proc/mounts` for the mount point, which is what
            `pgfs.sh up` does.

            ## Fix "Device or resource busy" on unmount

            A process has its working directory or an open file inside the
            mount. `pgfs.sh down` lists the holders and then lazy-unmounts.
            To find them yourself:

            ```bash
            fuser -vm testdata/mnt
            ```

            ## Fix "Transport endpoint is not connected"

            The daemon died and the mount was not removed, which happens
            only if it was started without AutoUnmount (see the
            `user_allow_other` warning below). Sweep it:

            ```bash
            ./scripts/pgfs.sh down
            ```

            or `fusermount3 -u -z testdata/mnt`.

            ## Make AutoUnmount available

            If the log shows
            `mount with AutoUnmount failed (...); retrying without AutoUnmount`,
            enable `user_allow_other`:

            ```bash
            sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
            ```

            ## Run pgfs under systemd

            Use `Type=simple`, start the binary directly with an explicit
            `--conn`, and let systemd stop it with SIGTERM:

            ```ini
            [Service]
            Type=simple
            ExecStart=/opt/pgfs/pgfs --conn "host=/run/postgresql dbname=pgfs" /srv/pgfs
            KillSignal=SIGTERM
            Restart=on-failure
            ```

            Services that need the mount should wait for the `.ready`
            file, or for the mount point to appear in `/proc/mounts`.
        """)

    def reference(self):
        return md(r"""
            ## Mount options

            | Option | Value |
            |---|---|
            | `fsname` | `pgfs` |
            | `auto_unmount` | requested; if the mount fails with it, pgfs logs a WARN and retries without it |
            | Kernel capability negotiated in `init` | `FUSE_BIG_WRITES` |

            ## Startup sequence

            1. Initialise logging; install the panic hook.
            2. Block SIGINT, SIGTERM, SIGHUP, SIGUSR1 and SIGUSR2 in every
               thread, so a signal that arrives during startup waits
               instead of killing the process.
            3. Connect to PostgreSQL and create the schema if needed.
            4. Write `/tmp/pgfs-<hash>.pid`; print and log its path.
            5. Create the FUSE session (mount).
            6. Write `/tmp/pgfs-<hash>.ready`.
            7. Start the liveness, metrics, deadlock-watchdog and
               signal-waiter threads.
            8. Serve FUSE requests until unmounted.

            ## Signals

            | Signal | Effect |
            |---|---|
            | SIGINT, SIGTERM, SIGHUP | clean unmount; the daemon exits with status 0 |
            | SIGUSR1 | state dump on the next FUSE callback (see [Observability](observabledaemon.md)) |
            | SIGUSR2 | toggle CPU profiling (see [On-Demand Profiling](ondemandprofiling.md)) |
            | SIGKILL, crash, OOM kill | the kernel removes the mount (AutoUnmount); health files remain |

            Signals are received with `sigwait` on a dedicated
            `signal-waiter` thread, never in an asynchronous handler.

            ## Health files

            | File | Written | Contents | Removed |
            |---|---|---|---|
            | `/tmp/pgfs-<hash>.pid` | after the database connection succeeds | daemon PID | on clean exit |
            | `/tmp/pgfs-<hash>.ready` | after the mount is created | `ready` | on clean exit |

            `<hash>` is 16 hex digits computed from the mountpoint argument
            exactly as given, so `./mnt` and `/abs/path/mnt` produce
            different names.

            ## Deadlock watchdog

            | Parameter | Value |
            |---|---|
            | Heartbeat period | 10 s |
            | Stall threshold | 30 s without a heartbeat, observed by a FUSE callback |
            | Action | ERROR log; the callback replies `EIO`; the watchdog unmounts |
            | Unmount retries | 5, 500 ms apart; then the process exits with status 1 |

            ## Database connection lifetime

            The daemon acquires one primary connection (and one standby
            connection with `--replica`) at startup and keeps it until
            exit. It does not reconnect. If PostgreSQL restarts or the
            connection drops, every operation that reaches the database
            replies `EIO` until the daemon is restarted. The mount itself
            stays up and still unmounts cleanly.

            ## Panics

            | Build | Behaviour |
            |---|---|
            | debug | the panic hook writes the crash report, then the panic unwinds |
            | release | the panic hook writes the crash report, then the process aborts (`panic = "abort"`) and the kernel removes the mount |

            ## Exit status

            | Status | Cause |
            |---|---|
            | 0 | clean unmount |
            | 1 | startup failure (error chain printed as `pgfs: …`), or the deadlock watchdog gave up |
        """)

    def explanation(self):
        return md(r"""
            ## What a ghost mount is

            A FUSE filesystem is a contract between the kernel and a user
            process: the kernel forwards every operation on the mount to
            the process and waits for an answer. If the process disappears
            and nobody tells the kernel, the mount stays. Every access to
            it fails with "Transport endpoint is not connected", and tools
            that walk the filesystem tree, such as `df`, `find /` and
            backup jobs, hit the dead mount and stall or fail. That is a
            ghost mount, and avoiding them shaped how pgfs starts and
            stops.

            ## Three layers, weakest to strongest

            **The lifecycle script** treats `/proc/mounts` as the source
            of truth rather than trusting PID files. If a mount exists
            with no live daemon behind it, `pgfs.sh up` sweeps it before
            mounting again. `pgfs.sh down` escalates from a polite signal
            to `fusermount3 -u` to a lazy unmount, and on the way it names
            the processes holding the mount busy, because "device busy"
            with no explanation is the most common frustration.

            **Signal handling** turns the polite signals into a clean
            unmount. pgfs blocks the signals it cares about in every
            thread and receives them with `sigwait` on one dedicated
            thread. That thread can safely take locks and call the
            unmount routine, which an asynchronous signal handler cannot.
            The signals are blocked before anything else happens, so a
            Ctrl+C during startup is queued rather than killing a
            half-initialised daemon.

            **AutoUnmount** is the layer that cannot fail. With it, the
            kernel itself watches the daemon process and removes the mount
            when the process exits for any reason: a clean return, a
            panic, SIGKILL or the OOM killer. Every other layer is a
            courtesy that produces a cleaner shutdown and clearer logs.
            AutoUnmount is the guarantee.

            ## Why release builds abort on panic

            A panic in a FUSE daemon usually means an invariant has broken.
            Unwinding can leave the process alive in an inconsistent state
            and still answering kernel requests. In release builds, pgfs
            aborts instead. The panic hook writes a crash report first,
            and AutoUnmount turns the abort into a clean disappearance.

            ## Why a deadlock unmounts the filesystem

            A wedged daemon is worse than a dead one. The kernel keeps
            waiting on it, and every process that touches the mount hangs
            in uninterruptible sleep. The liveness heartbeat detects this:
            a thread that cannot make progress stops advancing a counter,
            and the next request to arrive replies with an error and
            triggers an unmount. This converts a hang into an error, which
            processes can handle.

            ## Health files

            Supervisors need a cheap way to ask "is it up yet?" without
            parsing logs. The PID file appears once the database is
            reachable, and the readiness file appears once the mount
            exists. Both are keyed by a hash of the mount point, so several
            pgfs mounts on one machine do not collide.
        """)


class ObservableDaemon(Feat):
    """Operators can see what the daemon is doing, how fast it is doing
    it, and why something failed, from the log alone: structured spans
    for every operation, periodic metrics, an on-demand state dump, and
    self-contained error chains and crash reports."""

    deps = [
        UsesStructuredTracing,
        InstrumentsFuseCallbacks,
        InstrumentsDbOperations,
        DistinguishesExpectedErrors,
        ExportsPeriodicMetrics,
        DumpsStateOnSigusr1,
        WritesHealthFiles,
        ReportsCrashes,
    ]

    def title(self):
        return "Observability & Diagnostics"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Tracing, the 60-second metrics, the SIGUSR1 state dump, error "
            "chains and crash reports all ship today. Of the per-errno "
            "counters, only `eio` and `enoent` are incremented yet."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will watch pgfs work: follow its log, ask a
            running daemon for a state dump, read a metrics report, and
            then break the database on purpose to see what a real failure
            looks like. We need a terminal for the daemon and a second one
            for commands.

            ## Step 1: run the daemon in the foreground

            In the first terminal:

            ```bash
            ./scripts/pgfs.sh run
            ```

            The startup lines show the replica mode, the health file and
            the negotiated FUSE capability:

            ```text
            INFO pgfs: replica mode: off (single primary)
            health pid file: /tmp/pgfs-5049eced64c2821d.pid
            INFO init: pgfs::fs: FUSE init: negotiated FUSE_BIG_WRITES
            ```

            ## Step 2: see that normal misses are quiet

            In the second terminal:

            ```bash
            cat testdata/mnt/does-not-exist
            ```

            `cat` reports "No such file or directory", and the daemon's log
            stays silent. A missing file is an expected answer, not a
            failure, so pgfs does not log it.

            ## Step 3: ask for a state dump

            ```bash
            kill -USR1 "$(pgrep -f 'target/debug/pgfs')"
            ls testdata/mnt > /dev/null
            ```

            The dump is written by the next filesystem operation, here the
            `ls`:

            ```text
            INFO pgfs: SIGUSR1 received; state dump will appear on next FUSE callback
            INFO getattr{ino=1}: pgfs::fs: state|ino_cache=3 next_ino=4 db=connected (replica none) uptime_secs=41
            state|fuse_latency=fuse total=12 p50=<=1kµs p95=<=10kµs p99=<=10kµs
            state|db_latency=db total=15 p50=<=1kµs p95=<=1kµs p99=<=10kµs
            ```

            ## Step 4: read a metrics report

            Do some work, then wait for the minute to turn over:

            ```bash
            for i in $(seq 1 50); do echo $i > testdata/mnt/m$i; done
            rm testdata/mnt/m*
            ```

            Within 60 seconds, five `metrics|` lines appear. The
            `metrics|ops` line counts the 50 creates and 50 unlinks among
            the other operations.

            ## Step 5: break the database

            Stop PostgreSQL underneath the running mount, then touch a name
            the kernel has not cached:

            ```bash
            make db-stop
            ls testdata/mnt/after-stop
            ```

            ```text
            ls: cannot access 'testdata/mnt/after-stop': Input/output error
            ```

            This time the first terminal shows an ERROR. Read it left to
            right: the FUSE operation and its arguments (the span), what
            pgfs was doing and where (`src/db.rs:453`), and the root cause
            reported by PostgreSQL:

            ```text
            ERROR lookup{parent=1 name=after-stop}: pgfs::fs: get attributes of entry "after-stop" in "" (at src/db.rs:453): db error: FATAL: terminating connection due to administrator command
            ```

            ## Step 6: recover

            Start PostgreSQL again, then restart the daemon. pgfs holds a
            single database connection for its whole lifetime and does not
            reconnect, so it needs the restart:

            ```bash
            ./scripts/init_db.sh
            ```

            Press Ctrl+C in the first terminal. The daemon logs
            `received signal 2; unmounting cleanly` and exits. Run
            `./scripts/pgfs.sh run` again, and `ls testdata/mnt` works.

            ## What we did

            We saw that expected misses stay out of the log, pulled live
            state out of a running daemon, read its periodic metrics, and
            followed a real failure from the kernel's bare `EIO` to its
            root cause. The [reference](../reference/observabledaemon.md)
            lists every field; the [explanation](../explanation/observabledaemon.md)
            covers the design.
        """)

    def how_to(self):
        return md(r"""
            ## Change the log level

            ```bash
            RUST_LOG=pgfs=debug ./scripts/pgfs.sh run              # every DB query
            RUST_LOG=pgfs::fs=debug,pgfs::db=info ./scripts/pgfs.sh run
            ```

            The default is `pgfs=info`. The level is read at startup, so
            changing it requires a restart.

            ## Get JSON logs for a log pipeline

            ```bash
            RUST_LOG_FORMAT=json ./target/debug/pgfs testdata/mnt 2> pgfs.jsonl
            jq -c 'select(.level == "ERROR")' pgfs.jsonl
            ```

            ## Find out why an operation returned "Input/output error"

            ```bash
            grep ' ERROR ' testdata/pgfs.log | tail -5
            ```

            Read each line left to right: the FUSE span that failed, then
            each layer's context with its `file:line`, then the root cause
            after the last `: `.

            ## Recover from "Input/output error" on every operation

            If every access fails and the log shows connection errors, the
            daemon's database connection is gone. pgfs does not reconnect.
            Make sure PostgreSQL is running, then restart the daemon:

            ```bash
            ./scripts/ensure_db.sh
            ./scripts/pgfs.sh down && ./scripts/pgfs.sh up
            ```

            ## Take a snapshot of a running daemon

            ```bash
            kill -USR1 <pid>
            ls <mountpoint> > /dev/null     # any access triggers the dump
            ```

            ## Extract metrics from the log

            ```bash
            grep -o 'metrics|ops|.*' testdata/pgfs.log | tail -1
            grep -o 'metrics|latency|.*' testdata/pgfs.log | tail -2
            ```

            ## Check that the daemon is alive from a script

            ```bash
            pid=$(cat /tmp/pgfs-<hash>.pid) && kill -0 "$pid" && test -e /tmp/pgfs-<hash>.ready
            ```

            ## Get a useful crash report

            Run with `RUST_BACKTRACE=1` (or `full`) so the report includes a
            symbolised backtrace. The report is printed to stderr and
            logged at ERROR, so it ends up in `testdata/pgfs.log` when the
            daemon was started by `pgfs.sh`.
        """)

    def reference(self):
        return md(r"""
            ## Logging

            | Setting | Values | Default |
            |---|---|---|
            | `RUST_LOG` | `tracing-subscriber` `EnvFilter` syntax | `pgfs=info` |
            | `RUST_LOG_FORMAT` | `json` for newline-delimited JSON; anything else for text | text |
            | Destination | stderr | |

            ## Log levels

            | Level | Used for |
            |---|---|
            | ERROR | unexpected failures (with the full error chain), crash reports, deadlock detection |
            | WARN | degraded but working: replica fallback, AutoUnmount unavailable, health file write failures |
            | INFO | lifecycle (startup, mount, signals, shutdown), metrics, state dumps, profiling |
            | DEBUG | FUSE callback and database spans, reads served from the primary |

            Expected conditions (`ENOENT`, `EEXIST`, `ENOTEMPTY`, `EISDIR`,
            `ENOTDIR`, `EINVAL`) are replied without logging.

            ## Spans

            | Span | Level | Fields |
            |---|---|---|
            | `lookup`, `create`, `mkdir`, `unlink`, `rmdir` | INFO | `parent`, `name` |
            | `getattr`, `open`, `readdir` | INFO | `ino` (`readdir`: + `offset`) |
            | `read`, `write` | INFO | `ino`, `fh`, `offset`, `size`/`len` |
            | `setattr` | INFO | `ino`, `size` |
            | `fsync` | INFO | `fh` |
            | `rename` | INFO | `parent`, `name`, `newparent`, `newname`, `flags` |
            | `db::getattr`, `db::list`, `db::create`, `db::mkdir`, `db::unlink`, `db::rmdir` | DEBUG | `parent`, `name` |
            | `db::read_range`, `db::write_range` | DEBUG | `parent`, `name`, `offset`, `len` |
            | `db::truncate` | DEBUG | `parent`, `name`, `new_size` |
            | `db::write_blocks_batch` | DEBUG | `parent`, `name`, `count`, `new_size` |
            | `db::rename` | DEBUG | `old_parent`, `old_name`, `new_parent`, `new_name` |

            ## Periodic metrics

            Logged at INFO every 60 s by the `metrics-export` thread:

            ```text
            metrics|ops|lookup=N getattr=N setattr=N read=N write=N create=N mkdir=N unlink=N rmdir=N rename=N readdir=N open=N fsync=N
            metrics|errors|eio=N enoent=N eexist=N enotempty=N eisdir=N enotdir=N einval=N
            metrics|replica|reads=N fallbacks=N
            metrics|latency|fuse=fuse total=N p50=<=Xµs p95=<=Xµs p99=<=Xµs
            metrics|latency|db=db total=N p50=<=Xµs p95=<=Xµs p99=<=Xµs
            ```

            | Counter | Incremented when |
            |---|---|
            | `ops` fields | the corresponding FUSE callback runs (`flush` and `release` are not counted) |
            | `eio` | an unexpected failure is logged and replied as `EIO` |
            | `enoent` | `lookup` cannot resolve its parent inode or name |
            | `eexist`, `enotempty`, `eisdir`, `enotdir`, `einval` | declared; not yet incremented |
            | `replica reads` | a `getattr`, `lookup` or `readdir` is served by the standby |
            | `replica fallbacks` | a read goes to the primary while a standby is configured |

            All counters are totals since startup.

            ## Latency histograms

            | Histogram | Records |
            |---|---|
            | `fuse` | wall-clock time of every instrumented FUSE callback |
            | `db` | wall-clock time of every `Db` method |

            Buckets: `<=10`, `<=100`, `<=1k`, `<=10k`, `<=100k`, `<=1M`,
            `>1M` microseconds. Each percentile is reported as the upper
            bound of the bucket that contains it.

            ## State dump (SIGUSR1)

            ```text
            state|ino_cache=N next_ino=N db=connected (replica none|fresh|stale or unreachable) uptime_secs=N
            state|fuse_latency=fuse total=N p50=… p95=… p99=…
            state|db_latency=db total=N p50=… p95=… p99=…
            ```

            | Field | Meaning |
            |---|---|
            | `ino_cache` | paths with an assigned inode number |
            | `next_ino` | next inode number to be assigned |
            | `replica` | `none` without `--replica`; otherwise the cached freshness verdict |
            | `uptime_secs` | seconds since the mount was created |

            A final dump is logged on clean shutdown.

            ## Error chain format

            Unexpected failures are logged at ERROR as one line, the
            `anyhow` alternate (`{:#}`) rendering of the chain, inside the
            span of the FUSE callback that failed:

            ```text
            ERROR <callback>{<span fields>}: pgfs::fs: <what was attempted> (at src/<file>.rs:<line>): <deeper context> (at …): <root cause>
            ```

            Example:

            ```text
            ERROR lookup{parent=1 name=after-stop}: pgfs::fs: get attributes of entry "after-stop" in "" (at src/db.rs:453): db error: FATAL: terminating connection due to administrator command
            ```

            Startup failures are printed to stderr in the same form,
            prefixed `pgfs: `, and the process exits with status 1.

            ## Crash report

            Written to stderr and logged at ERROR before the process
            unwinds or aborts:

            ```text
            panic at <file>:<line>: <message>
            pgfs v<version> on <os>/<arch>, built with <rustc version>, RUST_BACKTRACE=<value>
            Backtrace:
            <backtrace>
            ```

            ## Background threads

            | Thread | Role |
            |---|---|
            | `signal-waiter` | receives SIGINT, SIGTERM, SIGHUP, SIGUSR1, SIGUSR2 |
            | `metrics-export` | logs metrics every 60 s |
            | `liveness` | heartbeat every 10 s |
            | `deadlock-watchdog` | unmounts when a stall is detected |
        """)

    def explanation(self):
        return md(r"""
            ## The kernel only hears an errno

            When something fails inside a FUSE callback, the kernel can be
            told exactly one thing: an error number. `EIO` carries no
            message, no location and no cause. The process that called
            `read` learns that something went wrong and nothing else.
            Everything an operator needs to fix the problem therefore has
            to be in the daemon's own log, and pgfs is built around that.

            ## Errors that tell a story

            Every fallible step in pgfs wraps its error with what it was
            trying to do and where, using `#[track_caller]` so the location
            is the caller's line, not the helper's. As an error travels up
            from the database driver through the storage layer to the FUSE
            callback, it accumulates a chain: "lookup during readdir, at
            this line, because fetching attributes, at that line, failed
            because the connection closed". The rule is that an operator
            should never need a debugger or the source code to understand a
            failure.

            The other half of the rule is to not log what is normal. A
            shell probing for a file that does not exist is not an error,
            and logging every `ENOENT` would bury real failures. Expected
            conditions are answered with their errno and nothing else.
            Only unexpected failures are logged, always at ERROR and always
            with the full chain.

            ## Spans instead of log lines

            pgfs uses the `tracing` crate rather than plain logging. Each
            FUSE callback opens a span, and each database call opens a span
            inside it. Any message emitted underneath carries the full
            context, and timing tools can attribute time to "FUSE"
            versus "database". JSON output keeps the same structure for
            machines.

            ## Metrics that are cheap enough to keep on

            The counters are atomic integers and the histograms are seven
            atomic buckets, so recording costs a few nanoseconds and is
            always on. Fixed buckets and percentiles approximated to a
            bucket bound are coarse, but they answer the questions that
            matter in operation: is it a 100 µs filesystem or a 10 ms one,
            and is it getting worse. Separating `eio` from expected
            errnos lets an operator tell a spike in "file not found" from a
            spike in "database down". Exporting through the log keeps the
            first iteration free of extra endpoints; the counters are plain
            statics, so an exporter can be added without changing the
            instrumentation.

            ## Asking a running daemon

            SIGUSR1 requests a snapshot without restarting anything. Signal
            handlers are severely restricted in what they may do, so the
            signal only sets a flag. The next FUSE callback, which runs in
            a normal context, writes the dump. On an idle mount that means
            the dump waits for the next access, a deliberate trade for
            never doing real work inside signal handling.

            ## Crash reports

            A panic report is written to both stderr and the log, because
            either may be the one an operator actually has. It includes the
            version, platform and compiler, so it can be pasted into a bug
            report as is.
        """)

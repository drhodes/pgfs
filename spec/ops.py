"""
Operations: creating, mounting, upgrading and administering a pgfs
filesystem.

Decided in design/design-decisions.md, topic 15. One command, `pgfs`,
does every administrative action. Schema creation and migration are
explicit, never a side effect of mounting (Chubby r04 and Raft r25 on
explicit, versioned state transitions). The supported Postgres versions
are stated and tested, because the database underneath has bugs of its
own (SQLancer r76).
"""

from .err import STABLE, Feat, Req, md
from .consistency import (
    ExclusiveTreeLease,
)
from .durability import (
    ChecksServerDurability,
    FlushesOnShutdown,
)
from .failure import (
    InFlightAgeWatchdog,
)
from .security import (
    LeastPrivilegeRoles,
)
from .storage import (
    FilesystemIdentity,
    SchemaVersion,
)


class AdminCli(Req):
    """The `pgfs` binary has these subcommands, and every one accepts
    `--conn` (or `PGFS_CONN`), `--help`, and `--json` where it prints
    data:

    | Subcommand | Purpose |
    |---|---|
    | `mkfs` | format a database (`MakesFilesystem`) |
    | `migrate` | upgrade the schema (`MigratesSchema`) |
    | `tune` | change capacity or identity (`TunesFilesystem`) |
    | `mount` | mount a tree, snapshot or subdirectory view |
    | `tree` | list, delete, flatten trees |
    | `snapshot` | create, list, delete snapshots |
    | `branch`, `promote` | branch and publish trees |
    | `changes`, `diff` | the change feed and snapshot comparison |
    | `check`, `scrub` | integrity |
    | `export`, `import` | portable archives |
    | `stats` | usage, backlog, amplification, durability window |

    Exit status is 0 on success, 1 when the command ran and found a
    problem (for example `check`), and 2 when it could not run.
    Errors print one line naming the operation and the cause, then the
    full chain at `-v`.
    """


class MakesFilesystem(Req):
    """`pgfs mkfs` creates the `pgfs` schema, its tables, indexes and
    functions, and the tree `main`, in one transaction. It writes the
    `meta` rows (`storage.FilesystemIdentity`) and grants the runtime
    role (`security.LeastPrivilegeRoles`). Its options are
    `--block-size`, `--inline-max`, `--capacity`, `--scratch` and
    `--runtime-role`. It checks the server first
    (`durability.ChecksServerDurability`,
    `SupportedPostgresVersions`).

    **Errors.** It refuses a database that already has a `pgfs` schema
    unless `--force` is given. `--force` drops the old schema in the
    same transaction and asks for confirmation on a terminal.

    **Crash.** All or nothing: one transaction.
    """

    deps = [SchemaVersion, FilesystemIdentity, LeastPrivilegeRoles, ChecksServerDurability]


class RefusesUnformattedDatabase(Req):
    """`mount` and every command except `mkfs` refuse a database with no
    `pgfs` schema. The message suggests `pgfs mkfs`. No command other
    than `mkfs` and `migrate` runs DDL (the SQL function
    `pgfs.scratch_reset()` on a scratch filesystem only truncates and
    re-seeds the unlogged data tables, `durability.ScratchMode`). The
    first schema created tables implicitly on every mount; that
    behaviour is removed.
    """

    deps = [SchemaVersion]


class MigratesSchema(Req):
    """`pgfs migrate` upgrades the schema from the version in `meta` to
    the version this binary uses, one numbered migration at a time. Each
    migration is a transaction that also updates `schema_version`. The
    command refuses while any unexpired mount session (read-write,
    read-only live, or snapshot) exists in `mounts`
    (`consistency.ExclusiveTreeLease`), and running mounts re-check
    `schema_version` on reconnect and at each lease renewal
    (`failure.RevalidatesAfterReconnect`). `--dry-run` prints the steps.
    There are no downgrades: the documented rollback is restoring a
    backup taken first, which the command reminds the user to do.

    **Crash.** A crash leaves the schema at the last completed
    migration, and running the command again resumes.
    """

    deps = [SchemaVersion, ExclusiveTreeLease]


class TunesFilesystem(Req):
    """`pgfs tune` changes what may change after mkfs:

    - `--capacity` sets or removes `capacity_bytes`;
    - `--new-uuid` replaces `fs_uuid`, and refuses while any unexpired
      mount session exists in `mounts`;
    - `--runtime-role` grants the runtime privileges to another role.

    Everything else fixed at mkfs stays fixed.
    """

    deps = [FilesystemIdentity]


class SupportedPostgresVersions(Req):
    """pgfs supports PostgreSQL 17 and every later major version that CI
    tests, and the release notes list them. `mkfs` and `mount` refuse an
    older server, and warn about a newer major version that has not been
    tested. Required extensions: `amcheck`, for `pgfs check` only.

    **Sources.** SQLancer r76 (the database has logic bugs of its own,
    so the versions matter).
    """


class NotifiesSystemd(Req):
    """When `NOTIFY_SOCKET` is set, the daemon sends `READY=1` once the
    mount is serving, `STATUS=` lines on state changes, and `STOPPING=1`
    at shutdown. It sends `WATCHDOG=1` from the in-flight watchdog
    (`failure.InFlightAgeWatchdog`) only while no request is past the
    stall threshold. The repository ships `pgfs@.service`, a template
    unit for one tree per instance. The pid and ready files
    (`WritesHealthFiles`) stay for other supervisors.
    """

    deps = [InFlightAgeWatchdog]


class UpgradesByRemount(Req):
    """Upgrading the daemon binary means unmounting and mounting again,
    and a clean unmount commits every buffer first
    (`durability.FlushesOnShutdown`). Restarting the daemon without
    unmounting, by passing `/dev/fuse` to the new process, depends on
    persistent inode numbers and is planned (XFUSE r35, Ananke r46).
    """

    deps = [FlushesOnShutdown]


class DocumentsEveryOption(Req):
    """Every command-line option, environment variable and default value
    appears in one reference table, generated from the argument parser,
    so the documentation cannot drift from the binary. A test fails if
    an option is missing from the table.
    """

    deps = [AdminCli]


# ── Feature ───────────────────────────────────────────────────────────

class FilesystemLifecycle(Feat):
    """One command does everything an administrator needs: format a
    database, mount trees and snapshots, upgrade the schema, tune
    capacity, and inspect, check and back up the filesystem. Nothing
    happens implicitly."""

    deps = [
        AdminCli,
        MakesFilesystem,
        RefusesUnformattedDatabase,
        MigratesSchema,
        TunesFilesystem,
        SupportedPostgresVersions,
        NotifiesSystemd,
        UpgradesByRemount,
        DocumentsEveryOption,
    ]

    def title(self):
        return "Administering pgfs"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "`pgfs` has every subcommand: mkfs, migrate, tune, mount, "
            "tree, snapshot, branch, promote, changes, diff, check, "
            "scrub, export, import, stats and options. Nothing runs DDL "
            "implicitly, the option reference in `docs/cli-options.md` is "
            "generated from the parser and checked by a test, and "
            "`contrib/pgfs@.service` ships. There is one migration so "
            "far."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will format a database, mount it, look
            at its statistics and unmount it. We need a PostgreSQL 17
            server and an empty database called `pgfs`.

            ## Step 1: format

            ```bash
            export PGFS_CONN="dbname=pgfs"
            pgfs mkfs --capacity 100G
            ```

            ```text
            formatted pgfs filesystem 6f1c…  block size 65536, inline 2048, capacity 100 GiB
            created tree main
            ```

            ## Step 2: mount

            ```bash
            mkdir -p /mnt/main
            pgfs mount main /mnt/main
            df -h /mnt/main
            ```

            ## Step 3: inspect

            ```bash
            pgfs stats main
            ```

            ## Step 4: unmount

            ```bash
            fusermount3 -u /mnt/main
            ```
        """)

    def how_to(self):
        return md(r"""
            ## Run a mount under systemd

            ```bash
            systemctl enable --now pgfs@main.service
            ```

            The unit reads `/etc/pgfs/main.env` for `PGFS_CONN` and the
            mount point.

            ## Upgrade pgfs

            1. Back up the database.
            2. Stop every mount.
            3. Install the new binary and run `pgfs migrate`.
            4. Mount again.

            ## Change the capacity

            ```bash
            pgfs tune --capacity 2T
            ```
        """)

    def reference(self):
        return md(r"""
            Every option of every subcommand, with its environment
            variable and default, is in [Command-line
            options](../cli-options.md), generated from the binary.

            ## Subcommands

            | Command | Purpose |
            |---|---|
            | `pgfs mkfs` | format a database |
            | `pgfs migrate` | upgrade the schema |
            | `pgfs tune` | capacity, identity, runtime role |
            | `pgfs mount` | mount a view |
            | `pgfs tree` / `snapshot` / `branch` / `promote` | branchable trees |
            | `pgfs changes` / `diff` | change feed |
            | `pgfs check` / `scrub` | integrity |
            | `pgfs export` / `import` | archives |
            | `pgfs stats` | statistics |

            ## mkfs options

            | Option | Default |
            |---|---|
            | `--block-size` | 65 536 |
            | `--inline-max` | 2 048 |
            | `--capacity` | none (reported from free disk space) |
            | `--scratch` | off |
            | `--runtime-role` | none |

            ## Supported servers

            PostgreSQL 17 and later majors tested in CI. `amcheck` for
            `pgfs check`.
        """)

    def explanation(self):
        return md(r"""
            ## Nothing implicit

            The first pgfs created and patched its tables every time it
            mounted. That is convenient until the day a mount runs
            against the wrong database, or an old binary meets a newer
            schema. pgfs now separates formatting, migrating and
            mounting, records the schema version, and refuses anything
            it does not recognise.
        """)

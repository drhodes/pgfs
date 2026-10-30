# Reference: Administering pgfs

!!! success "Status: Stable"
    `pgfs` has every subcommand: mkfs, migrate, tune, mount, tree, snapshot, branch, promote, changes, diff, check, scrub, export, import, stats and options. Nothing runs DDL implicitly, the option reference in `docs/cli-options.md` is generated from the parser and checked by a test, and `contrib/pgfs@.service` ships. There is one migration so far.

> One command does everything an administrator needs: format a database, mount trees and snapshots, upgrade the schema, tune capacity, and inspect, check and back up the filesystem. Nothing happens implicitly.

[Tutorial](../tutorials/filesystemlifecycle.md) · [How-to](../how-to/filesystemlifecycle.md) · **Reference** · [Explanation](../explanation/filesystemlifecycle.md)

---

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


## Specification requirements

`FilesystemLifecycle` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "AdminCli"
    `spec.ops.AdminCli`

    The `pgfs` binary has these subcommands, and every one accepts
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

??? abstract "MakesFilesystem"
    `spec.ops.MakesFilesystem`

    `pgfs mkfs` creates the `pgfs` schema, its tables, indexes and
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

??? abstract "RefusesUnformattedDatabase"
    `spec.ops.RefusesUnformattedDatabase`

    `mount` and every command except `mkfs` refuse a database with no
    `pgfs` schema. The message suggests `pgfs mkfs`. No command other
    than `mkfs` and `migrate` runs DDL (the SQL function
    `pgfs.scratch_reset()` on a scratch filesystem only truncates and
    re-seeds the unlogged data tables, `durability.ScratchMode`). The
    first schema created tables implicitly on every mount; that
    behaviour is removed.

??? abstract "MigratesSchema"
    `spec.ops.MigratesSchema`

    `pgfs migrate` upgrades the schema from the version in `meta` to
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

??? abstract "TunesFilesystem"
    `spec.ops.TunesFilesystem`

    `pgfs tune` changes what may change after mkfs:

    - `--capacity` sets or removes `capacity_bytes`;
    - `--new-uuid` replaces `fs_uuid`, and refuses while any unexpired
      mount session exists in `mounts`;
    - `--runtime-role` grants the runtime privileges to another role.

    Everything else fixed at mkfs stays fixed.

??? abstract "SupportedPostgresVersions"
    `spec.ops.SupportedPostgresVersions`

    pgfs supports PostgreSQL 17 and every later major version that CI
    tests, and the release notes list them. `mkfs` and `mount` refuse an
    older server, and warn about a newer major version that has not been
    tested. Required extensions: `amcheck`, for `pgfs check` only.

    **Sources.** SQLancer r76 (the database has logic bugs of its own,
    so the versions matter).

??? abstract "NotifiesSystemd"
    `spec.ops.NotifiesSystemd`

    When `NOTIFY_SOCKET` is set, the daemon sends `READY=1` once the
    mount is serving, `STATUS=` lines on state changes, and `STOPPING=1`
    at shutdown. It sends `WATCHDOG=1` from the in-flight watchdog
    (`failure.InFlightAgeWatchdog`) only while no request is past the
    stall threshold. The repository ships `pgfs@.service`, a template
    unit for one tree per instance. The pid and ready files
    (`WritesHealthFiles`) stay for other supervisors.

??? abstract "UpgradesByRemount"
    `spec.ops.UpgradesByRemount`

    Upgrading the daemon binary means unmounting and mounting again,
    and a clean unmount commits every buffer first
    (`durability.FlushesOnShutdown`). Restarting the daemon without
    unmounting, by passing `/dev/fuse` to the new process, depends on
    persistent inode numbers and is planned (XFUSE r35, Ananke r46).

??? abstract "DocumentsEveryOption"
    `spec.ops.DocumentsEveryOption`

    Every command-line option, environment variable and default value
    appears in one reference table, generated from the argument parser,
    so the documentation cannot drift from the binary. A test fails if
    an option is missing from the table.

---

<small>Generated from `spec.ops.FilesystemLifecycle` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

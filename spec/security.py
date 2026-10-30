"""
Security and access control.

Decided in design/design-decisions.md, topic 13. File permissions are
enforced by the kernel, which already implements them exactly. The
database is reached with least privilege, over TLS when remote, and no
secret ever reaches a log. Snapshot immutability is enforced in the
database itself, not only in the daemon (NASD 04: the storage, not the
client, enforces the rules that matter).
"""

from .err import STABLE, Feat, Req, md
from .posix import (
    SpecialFiles,
)
from .storage import (
    InodeTable,
    OperationsAsSqlFunctions,
)


class EnforcesPermissions(Req):
    """Every mount uses the FUSE `default_permissions` option, so the
    kernel checks mode bits, ownership, sticky directories, and the
    rules for chmod, chown and utimes before a request reaches the
    daemon. The daemon does not repeat these checks.

    **Postcondition.** Access decisions match a local Linux filesystem
    with the same modes and owners. pjdfstest verifies this
    (`verify.PosixSuites`).
    """


class OwnershipFromCaller(Req):
    """A new inode takes its uid from the calling process. Its gid comes
    from the caller, unless the parent directory has the set-group-id
    bit. In that case the gid comes from the parent, and a new
    subdirectory also inherits the bit. The mode is the requested mode
    with the caller's umask applied, as the kernel passes it.
    """

    deps = [InodeTable]


class ClearsSetIdBits(Req):
    """A write or truncate by a caller other than the owner clears the
    set-user-id bit of a regular file, and the set-group-id bit if group
    execute is set, in the same transaction. Where the FUSE crate
    supports `FUSE_HANDLE_KILLPRIV_V2`, the daemon declares it and
    clears the bits when the kernel says so. Otherwise it applies the
    rule itself on every write and truncate.

    **Sources.** POSIX and Linux `should_remove_suid`. Without this rule,
    a writable set-uid binary could be replaced and keep its privilege.
    """

    deps = [InodeTable]


class AllowOtherOptIn(Req):
    """Only the user who mounted the filesystem can use it, unless the
    mount is given `--allow-other`, which also requires
    `user_allow_other` in `/etc/fuse.conf`. Because `default_permissions`
    is always on, `--allow-other` applies normal Unix permissions to
    other users. It does not bypass them.
    """

    deps = [EnforcesPermissions]


class SetIdAndDevicesOptIn(Req):
    """Every mount is `nosuid` and `nodev`: the kernel ignores set-user-id
    and set-group-id bits when a program runs from it, and refuses to
    open device nodes on it. pgfs still stores the bits and the device
    numbers (`ClearsSetIdBits`, `posix.SpecialFiles`); the mount flags
    decide only whether they take effect. The daemon asks for `nosuid`
    and `nodev` explicitly, so a mount made by root is no exception.

    `pgfs mount --suid` and `pgfs mount --dev` lift one flag each, for a
    container or root filesystem that needs set-uid programs or device
    nodes. Both need the daemon to run as root (effective uid 0), the only
    case in which FUSE honours them; otherwise the mount is refused before
    anything is mounted, naming the flag. Each such mount logs a WARN,
    on every mount, naming the flag and what it grants.

    **Security.** With `--suid`, anyone who can write the mounted tree can
    gain root on the mounting host: any holder of the runtime role (the
    `pgfs.*` functions trust the kernel's checks and take owners and modes
    as given), the schema owner, `pgfs import` of a crafted archive, and
    whoever prepared a branch that is promoted into the tree or a snapshot
    that is mounted. One runtime role can write every tree in its
    database, so the trust domain is the database. With `--dev`, the same
    writers gain whatever the device numbers mean on the mounting host,
    for example raw disk or memory access. The reference documentation
    says so. Read-only and snapshot mounts may use both flags, which
    limits the writers to whoever produced the snapshot.

    **Invariant.** No mount honours set-id bits or device nodes unless it
    was given `--suid` or `--dev`.

    **Errors.** Without root, `--suid` or `--dev` exits with status 2
    before mounting.

    **Sources.** fusermount and `mount.fuse` (unprivileged FUSE mounts are
    always `nosuid,nodev`); mount(8).
    """

    deps = [EnforcesPermissions, ClearsSetIdBits, SpecialFiles]


class LeastPrivilegeRoles(Req):
    """pgfs uses two database roles:

    - **owner**: owns the `pgfs` schema; used by `mkfs`, `migrate` and
      `tune`;
    - **runtime**: used by mounts and every other command. It may only
      execute the `pgfs` functions and read the views and tables the
      daemon queries. It cannot alter the schema or write tables
      directly. The functions are `SECURITY DEFINER`, owned by the owner
      role, with a fixed `search_path`.

    `pgfs mkfs --runtime-role NAME` grants exactly this. At mount the
    daemon logs a WARN if it is connected as a superuser or as the
    owner.

    **Invariant.** A runtime-role connection cannot change any pgfs row
    except through a pgfs function, so it cannot bypass leases, epochs,
    fencing or snapshot immutability.
    """

    deps = [OperationsAsSqlFunctions]


class RequiresTlsForRemote(Req):
    """A connection to anything other than a Unix socket or loopback
    requires `sslmode=verify-full`. The daemon refuses a weaker mode
    unless `--insecure-transport` is given, which it logs at WARN on
    every mount.
    """


class RedactsSecrets(Req):
    """Passwords are never accepted in argv. They come from `PGPASSFILE`,
    a libpq service file, `PGPASSWORD`, or a file named by
    `--password-file`. Every connection string that appears in a log
    line, error message, state dump or crash report has its `password`
    parameter and any URI userinfo replaced by `***`.

    **Invariant.** No log line contains a secret. A test greps the log
    of the whole integration suite for the test password.

    The current `ConnectsToPostgres` error text includes the connection
    string unredacted.
    """


class EncryptionAtRestDelegated(Req):
    """pgfs does not encrypt data itself. Encryption at rest is the job
    of the disk, the volume or Postgres's storage layer, and the
    documentation says so. Data in transit to the database is protected
    by `RequiresTlsForRemote`.
    """


# ── Feature ───────────────────────────────────────────────────────────

class AccessControl(Feat):
    """Unix permissions on pgfs work as on a local disk, enforced by the
    kernel. The daemon reaches Postgres with the least privilege it
    needs, over TLS when the database is remote, and never logs a
    secret."""

    deps = [
        EnforcesPermissions,
        OwnershipFromCaller,
        ClearsSetIdBits,
        AllowOtherOptIn,
        SetIdAndDevicesOptIn,
        LeastPrivilegeRoles,
        RequiresTlsForRemote,
        RedactsSecrets,
        EncryptionAtRestDelegated,
    ]

    def title(self):
        return "Access Control and Security"

    def status(self):
        return STABLE

    def status_note(self):
        return (
            "Mounts use `default_permissions` (other users are refused "
            "unless `--allow-other`), owners come from the caller with "
            "set-group-id inheritance, set-id bits are cleared as Linux "
            "does, `mkfs --runtime-role` grants only the functions and "
            "reads (tested), remote connections need "
            "`sslmode=verify-full` through rustls, and connection strings "
            "are redacted. `FUSE_HANDLE_KILLPRIV_V2` is not available in "
            "fuser 0.14, so the daemon applies the set-id rule itself."
        )

    def tutorial(self):
        return md(r"""
            In this tutorial we will set up the two database roles and
            see permissions enforced.

            ## Step 1: make a filesystem with a runtime role

            ```bash
            createuser pgfs_runtime
            pgfs mkfs --conn "dbname=pgfs user=pgfs_owner" --runtime-role pgfs_runtime
            ```

            ## Step 2: mount as the runtime role

            ```bash
            pgfs mount --conn "dbname=pgfs user=pgfs_runtime" main /mnt/main
            ```

            ## Step 3: see permissions enforced

            ```bash
            echo secret > /mnt/main/private && chmod 600 /mnt/main/private
            sudo -u nobody cat /mnt/main/private
            ```

            ```text
            cat: /mnt/main/private: Permission denied
            ```

            (Run with `--allow-other` for another user to reach the mount
            at all.)
        """)

    def how_to(self):
        return md(r"""
            ## Connect to a remote database securely

            ```bash
            pgfs mount --conn "host=db.example.com dbname=pgfs user=pgfs_runtime sslmode=verify-full sslrootcert=/etc/pgfs/ca.pem" \
              --password-file /etc/pgfs/password main /mnt/main
            ```

            ## Share a mount with other users

            Add `user_allow_other` to `/etc/fuse.conf`, then mount with
            `--allow-other`. Normal permissions still apply.

            ## Run set-uid programs or use device nodes from a tree

            For a container or root filesystem whose every writer you
            trust with root on this host, run the daemon as root:

            ```bash
            sudo pgfs mount os /srv/os --allow-other --suid
            ```

            Add `--dev` only if programs must open device nodes stored in
            the tree; a booted system or a container normally gets `/dev`
            from devtmpfs or its runtime. To limit who can plant set-uid
            programs, mount a snapshot (`os@release-42`) or `--read-only`,
            and keep such trees in a database of their own.

            ## Encrypt data at rest

            Put the Postgres data directory on an encrypted volume.
        """)

    def reference(self):
        return md(r"""
            ## Roles

            | Role | Used by | May |
            |---|---|---|
            | owner | `mkfs`, `migrate`, `tune` | everything in the `pgfs` schema |
            | runtime | `mount` and all other commands | execute `pgfs` functions; read views |

            ## Options

            | Option | Effect |
            |---|---|
            | `--allow-other` | let other users reach the mount (permissions still apply) |
            | `--suid` | honour set-user-id and set-group-id bits; root only; logged at WARN |
            | `--dev` | allow device nodes to be opened; root only; logged at WARN |

            ## Mount flags

            Every mount is `nosuid,nodev` unless `--suid` or `--dev` is
            given. With `--suid`, these can gain root on the mounting
            host:

            | Writer | How |
            |---|---|
            | any holder of the runtime role | `pgfs.setattr` or `pgfs.create` with owner 0 and mode 04755, called directly |
            | the schema owner | writing the tables |
            | `pgfs import` | an archive with set-uid-root files |
            | the author of a promoted branch or a mounted snapshot | whatever they put in it |

            With `--dev`, the same writers gain the devices their nodes'
            numbers name on the mounting host. One runtime role writes
            every tree in its database.
            | `--password-file PATH` | read the database password from PATH |
            | `--insecure-transport` | allow a non-TLS remote connection (logged) |
        """)

    def explanation(self):
        return md(r"""
            ## The kernel enforces permissions

            Linux's permission rules have many corners: sticky
            directories, set-group-id inheritance, who may chown, when
            set-id bits drop. The kernel implements all of them, and
            FUSE's `default_permissions` option makes it apply them
            before a request reaches pgfs. Reimplementing them in the
            daemon would only add a second chance to get them wrong.

            ## Why set-id bits and devices are opt-in

            A set-uid-root program runs as root whoever wrote it, and
            the kernel checks permissions only for writes that pass
            through a mount. pgfs trees can also be written through SQL,
            through `pgfs import`, and from other machines, so with
            `--suid` the database's writers become root on the mounting
            host. That is the bargain a container or root filesystem on
            pgfs has to make, and it is why the flags need root, are
            logged on every mount, and are off unless asked for.
            Unprivileged FUSE mounts have always been `nosuid,nodev` for
            the same reason.

            ## The database enforces pgfs's rules

            A mount's database role can only call pgfs's functions. So
            the rules that keep a filesystem consistent, one writer per
            tree, frozen snapshots, ordered epochs, hold even for a
            client that is buggy or hostile.
        """)

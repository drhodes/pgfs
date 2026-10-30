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

            ## The database enforces pgfs's rules

            A mount's database role can only call pgfs's functions. So
            the rules that keep a filesystem consistent, one writer per
            tree, frozen snapshots, ordered epochs, hold even for a
            client that is buggy or hostile.
        """)

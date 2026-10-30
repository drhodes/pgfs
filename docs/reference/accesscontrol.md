# Reference: Access Control and Security

!!! success "Status: Stable"
    Mounts use `default_permissions` (other users are refused unless `--allow-other`), owners come from the caller with set-group-id inheritance, set-id bits are cleared as Linux does, `mkfs --runtime-role` grants only the functions and reads (tested), remote connections need `sslmode=verify-full` through rustls, and connection strings are redacted. `FUSE_HANDLE_KILLPRIV_V2` is not available in fuser 0.14, so the daemon applies the set-id rule itself.

> Unix permissions on pgfs work as on a local disk, enforced by the kernel. The daemon reaches Postgres with the least privilege it needs, over TLS when the database is remote, and never logs a secret.

[Tutorial](../tutorials/accesscontrol.md) · [How-to](../how-to/accesscontrol.md) · **Reference** · [Explanation](../explanation/accesscontrol.md)

---

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


## Specification requirements

`AccessControl` is built from these requirement classes. Each is declared in `spec/` and listed in the feature's `deps`.

??? abstract "EnforcesPermissions"
    `spec.security.EnforcesPermissions`

    Every mount uses the FUSE `default_permissions` option, so the
    kernel checks mode bits, ownership, sticky directories, and the
    rules for chmod, chown and utimes before a request reaches the
    daemon. The daemon does not repeat these checks.

    **Postcondition.** Access decisions match a local Linux filesystem
    with the same modes and owners. pjdfstest verifies this
    (`verify.PosixSuites`).

??? abstract "OwnershipFromCaller"
    `spec.security.OwnershipFromCaller`

    A new inode takes its uid from the calling process. Its gid comes
    from the caller, unless the parent directory has the set-group-id
    bit. In that case the gid comes from the parent, and a new
    subdirectory also inherits the bit. The mode is the requested mode
    with the caller's umask applied, as the kernel passes it.

??? abstract "ClearsSetIdBits"
    `spec.security.ClearsSetIdBits`

    A write or truncate by a caller other than the owner clears the
    set-user-id bit of a regular file, and the set-group-id bit if group
    execute is set, in the same transaction. Where the FUSE crate
    supports `FUSE_HANDLE_KILLPRIV_V2`, the daemon declares it and
    clears the bits when the kernel says so. Otherwise it applies the
    rule itself on every write and truncate.

    **Sources.** POSIX and Linux `should_remove_suid`. Without this rule,
    a writable set-uid binary could be replaced and keep its privilege.

??? abstract "AllowOtherOptIn"
    `spec.security.AllowOtherOptIn`

    Only the user who mounted the filesystem can use it, unless the
    mount is given `--allow-other`, which also requires
    `user_allow_other` in `/etc/fuse.conf`. Because `default_permissions`
    is always on, `--allow-other` applies normal Unix permissions to
    other users. It does not bypass them.

??? abstract "LeastPrivilegeRoles"
    `spec.security.LeastPrivilegeRoles`

    pgfs uses two database roles:

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

??? abstract "RequiresTlsForRemote"
    `spec.security.RequiresTlsForRemote`

    A connection to anything other than a Unix socket or loopback
    requires `sslmode=verify-full`. The daemon refuses a weaker mode
    unless `--insecure-transport` is given, which it logs at WARN on
    every mount.

??? abstract "RedactsSecrets"
    `spec.security.RedactsSecrets`

    Passwords are never accepted in argv. They come from `PGPASSFILE`,
    a libpq service file, `PGPASSWORD`, or a file named by
    `--password-file`. Every connection string that appears in a log
    line, error message, state dump or crash report has its `password`
    parameter and any URI userinfo replaced by `***`.

    **Invariant.** No log line contains a secret. A test greps the log
    of the whole integration suite for the test password.

    The current `ConnectsToPostgres` error text includes the connection
    string unredacted.

??? abstract "EncryptionAtRestDelegated"
    `spec.security.EncryptionAtRestDelegated`

    pgfs does not encrypt data itself. Encryption at rest is the job
    of the disk, the volume or Postgres's storage layer, and the
    documentation says so. Data in transit to the database is protected
    by `RequiresTlsForRemote`.

---

<small>Generated from `spec.security.AccessControl` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

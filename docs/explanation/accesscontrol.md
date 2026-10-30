# Explanation: Access Control and Security

!!! success "Status: Stable"
    Mounts use `default_permissions` (other users are refused unless `--allow-other`), owners come from the caller with set-group-id inheritance, set-id bits are cleared as Linux does, `mkfs --runtime-role` grants only the functions and reads (tested), remote connections need `sslmode=verify-full` through rustls, and connection strings are redacted. `FUSE_HANDLE_KILLPRIV_V2` is not available in fuser 0.14, so the daemon applies the set-id rule itself.

> Unix permissions on pgfs work as on a local disk, enforced by the kernel. The daemon reaches Postgres with the least privilege it needs, over TLS when the database is remote, and never logs a secret.

[Tutorial](../tutorials/accesscontrol.md) · [How-to](../how-to/accesscontrol.md) · [Reference](../reference/accesscontrol.md) · **Explanation**

---

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

---

<small>Generated from `spec.security.AccessControl` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

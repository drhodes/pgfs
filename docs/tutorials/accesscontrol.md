# Tutorial: Access Control and Security

!!! success "Status: Stable"
    Mounts use `default_permissions` (other users are refused unless `--allow-other`), owners come from the caller with set-group-id inheritance, set-id bits are cleared as Linux does, `mkfs --runtime-role` grants only the functions and reads (tested), remote connections need `sslmode=verify-full` through rustls, and connection strings are redacted. `FUSE_HANDLE_KILLPRIV_V2` is not available in fuser 0.14, so the daemon applies the set-id rule itself.

> Unix permissions on pgfs work as on a local disk, enforced by the kernel. The daemon reaches Postgres with the least privilege it needs, over TLS when the database is remote, and never logs a secret.

**Tutorial** · [How-to](../how-to/accesscontrol.md) · [Reference](../reference/accesscontrol.md) · [Explanation](../explanation/accesscontrol.md)

---

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

---

<small>Generated from `spec.security.AccessControl` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

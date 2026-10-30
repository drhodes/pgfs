# How-To: Access Control and Security

!!! success "Status: Stable"
    Mounts use `default_permissions` (other users are refused unless `--allow-other`), owners come from the caller with set-group-id inheritance, set-id bits are cleared as Linux does, `mkfs --runtime-role` grants only the functions and reads (tested), remote connections need `sslmode=verify-full` through rustls, and connection strings are redacted. `FUSE_HANDLE_KILLPRIV_V2` is not available in fuser 0.14, so the daemon applies the set-id rule itself.

> Unix permissions on pgfs work as on a local disk, enforced by the kernel. The daemon reaches Postgres with the least privilege it needs, over TLS when the database is remote, and never logs a secret.

[Tutorial](../tutorials/accesscontrol.md) · **How-to** · [Reference](../reference/accesscontrol.md) · [Explanation](../explanation/accesscontrol.md)

---

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

---

<small>Generated from `spec.security.AccessControl` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

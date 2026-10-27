# How-To: Atomic Rename

!!! success "Status: Stable"
    File and directory renames, overwrite semantics and the full POSIX error matrix are implemented and covered by `test_rename.sh` and `test_git_init.sh`. `renameat2` flags (`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) are rejected.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

[Tutorial](../tutorials/atomicrename.md) · **How-to** · [Reference](../reference/atomicrename.md) · [Explanation](../explanation/atomicrename.md)

---

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

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

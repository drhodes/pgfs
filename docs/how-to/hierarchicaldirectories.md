# How-To: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and `readdirplus` keep link counts and directory times, page by name with keyset cookies, and check emptiness in the same transaction as the delete. Covered by `tests/storage.rs`, the model-conformance suite and `test_dirs.sh`.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

[Tutorial](../tutorials/hierarchicaldirectories.md) · **How-to** · [Reference](../reference/hierarchicaldirectories.md) · [Explanation](../explanation/hierarchicaldirectories.md)

---

## List a large directory

`ls` and `find` page through it 256 names at a time, resuming after
the last name returned, so a listing costs O(n) whatever the size and
never repeats an entry.

## Count everything below a directory

```sql
SELECT kind, count(*) FROM pgfs.files
WHERE tree = 'main' AND starts_with(path, '/a/')
GROUP BY kind;
```

## Find the largest directories

```sql
SELECT regexp_replace(path, '/[^/]*$', '') AS dir, sum(size) AS bytes
FROM pgfs.files WHERE tree = 'main' AND kind = 'f'
GROUP BY dir ORDER BY bytes DESC LIMIT 10;
```

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

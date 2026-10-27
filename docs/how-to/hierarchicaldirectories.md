# How-To: Directory Trees

!!! success "Status: Stable"
    `mkdir`, `rmdir`, `readdir` and nested paths of any depth are implemented and covered by `test_dirs.sh`. Directory timestamps are not tracked.

> Users can create nested directories, list their contents, and remove empty directories through standard POSIX directory operations.

[Tutorial](../tutorials/hierarchicaldirectories.md) · **How-to** · [Reference](../reference/hierarchicaldirectories.md) · [Explanation](../explanation/hierarchicaldirectories.md)

---

## Create nested directories

```bash
mkdir -p a/b/c
```

## List a directory, including hidden entries

```bash
ls -la a
```

## Remove an empty directory

```bash
rmdir a/b/c
```

## Remove a directory and everything in it

```bash
rm -r a
```

## Find out why `rmdir` says "Directory not empty"

Hidden files count. List everything:

```bash
ls -A a/b
```

Or ask the database directly; the result is the full path of
every child of `a/b`:

```sql
SELECT parent || '/' || name FROM entries WHERE parent = 'a/b';
```

## Count everything below a directory

```sql
SELECT kind, count(*) FROM entries
WHERE parent = 'a' OR starts_with(parent, 'a/')
GROUP BY kind;
```

Use `starts_with` rather than `LIKE 'a/%'`. In a `LIKE`
pattern, `_` and `%` in directory names act as wildcards.

## Find the largest directories by total file size

```sql
SELECT parent, sum(size) AS bytes, count(*) AS files
FROM entries WHERE kind = 'file'
GROUP BY parent ORDER BY bytes DESC LIMIT 10;
```

---

<small>Generated from `spec.app.HierarchicalDirectories` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

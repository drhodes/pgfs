# Tutorial: Atomic Rename

!!! success "Status: Stable"
    File and directory renames, overwrite semantics and the full POSIX error matrix are implemented and covered by `test_rename.sh` and `test_git_init.sh`. `renameat2` flags (`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) are rejected.

> Users can rename (move) files and directories with standard `mv` semantics, including overwriting existing files and empty directories.

**Tutorial** · [How-to](../how-to/atomicrename.md) · [Reference](../reference/atomicrename.md) · [Explanation](../explanation/atomicrename.md)

---

In this tutorial we will rename files and whole directory
trees, overwrite a file atomically, see which renames pgfs
refuses, and finish by running git (which depends on atomic
rename) inside the mount.

We need a running mount; see [Your first pgfs
mount](projectlocalenvironment.md).

## Step 1: rename over an existing file

```bash
mkdir testdata/mnt/rename && cd testdata/mnt/rename
echo v1 > a.txt
echo v2 > b.txt
mv a.txt b.txt
cat b.txt
ls
```

```text
v1
b.txt
```

`b.txt` now holds what `a.txt` held, and `a.txt` is gone.

## Step 2: move a directory with everything in it

```bash
mkdir -p old/inner
echo deep > old/inner/file.txt
mv old new
cat new/inner/file.txt
```

```text
deep
```

## Step 3: see what pgfs refuses

A directory cannot move inside itself:

```bash
mv new new/inner/x
```

```text
mv: cannot move 'new' to a subdirectory of itself, 'new/inner/x'
```

A directory cannot replace a non-empty directory:

```bash
mkdir -p full && touch full/f
mkdir empty
mv -T empty full
```

```text
mv: cannot overwrite 'full': Directory not empty
```

## Step 4: run git

Git takes a lock by creating `config.lock`, writes the new
configuration into it, and renames it over `config`. Each
commit does the same with branch references.

```bash
git init -q repo && cd repo
echo hello > README
git add README
git -c user.name=Tutorial -c user.email=tutorial@example.com commit -qm "first commit"
git log --oneline
```

```text
511658a first commit
```

Your commit hash will differ.

## Step 5: clean up

```bash
cd ../../../..
rm -rf testdata/mnt/rename
```

## What we did

We replaced a file by renaming over it, moved a populated
directory in one step, saw the two renames POSIX forbids, and
ran a git workflow that depends on all of it. The
[explanation](../explanation/atomicrename.md) describes how one
database transaction makes this atomic.

---

<small>Generated from `spec.app.AtomicRename` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

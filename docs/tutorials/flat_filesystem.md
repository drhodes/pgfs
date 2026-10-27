# Tutorial: Regular Files

!!! success "Status: Stable"
    Create, read, write, append, truncate and delete are implemented and covered by unit and integration tests. Permissions, ownership and timestamps are reported but not settable (see Reference).

> Users can create, read, write, and delete files through standard POSIX file operations on the mounted directory.

**Tutorial** · [How-to](../how-to/flat_filesystem.md) · [Reference](../reference/flat_filesystem.md) · [Explanation](../explanation/flat_filesystem.md)

---

In this tutorial we will create a file, read it, add to it, cut
it short, grow it again, and then look at how its bytes are laid
out in PostgreSQL. By the end we will have seen every basic file
operation pgfs supports.

We need a running mount. If `./scripts/pgfs.sh status` does not
say `mount: active`, follow [Your first pgfs
mount](projectlocalenvironment.md) first, up to Step 2.

## Step 1: make a workspace

```bash
mkdir testdata/mnt/files
cd testdata/mnt/files
```

## Step 2: create and read a file

```bash
echo 'Hello pgfs' > demo.txt
cat demo.txt
```

```text
Hello pgfs
```

## Step 3: append to it

```bash
echo 'More content' >> demo.txt
cat demo.txt
wc -c demo.txt
```

```text
Hello pgfs
More content
24 demo.txt
```

## Step 4: cut it short, then grow it

```bash
truncate -s 5 demo.txt
cat demo.txt; echo
```

```text
Hello
```

Now grow the file to 10 bytes. The new bytes read back as
zeros:

```bash
truncate -s 10 demo.txt
od -c demo.txt
```

```text
0000000   H   e   l   l   o  \0  \0  \0  \0  \0
0000012
```

## Step 5: write something bigger

```bash
head -c 200000 /dev/urandom > big.bin
cp big.bin copy.bin
cmp big.bin copy.bin && echo identical
```

```text
identical
```

## Step 6: look at the bytes in PostgreSQL

From the repository root, in a second terminal:

```bash
psql -h "$PWD/testdata" -d pgfs -c \
  "SELECT name, block_no, length(data) FROM blocks WHERE parent = 'files' ORDER BY name, block_no"
```

```text
   name   | block_no | length
----------+----------+--------
 big.bin  |        0 |  65536
 big.bin  |        1 |  65536
 big.bin  |        2 |  65536
 big.bin  |        3 |   3392
 copy.bin |        0 |  65536
 ...
 demo.txt |        0 |      5
```

`big.bin` is four rows: three full 64 KiB blocks and a short
last one. `demo.txt` is a single 5-byte block. Its declared
size is 10, and the five missing bytes are the zeros we saw in
Step 4.

## Step 7: clean up

```bash
cd ../../..
rm -r testdata/mnt/files
```

## What we did

We used ordinary shell tools to exercise create, read, write,
append, truncate and delete, and saw that a file's contents are
stored as fixed-size blocks. To learn why it is built this
way, read the [explanation](../explanation/flat_filesystem.md).
Next, [Directory Trees](hierarchicaldirectories.md).

---

<small>Generated from `spec.app.FlatFilesystem` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

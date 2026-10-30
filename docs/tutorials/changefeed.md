# Tutorial: Change Feed

!!! success "Status: Stable"
    Every mutating function records its change with paths resolved in the transaction; delivery by transaction-id horizon and sequence cap, resumable consumers, retention, coalesced notifications from housekeeping and `pgfs changes` are built and tested in `tests/storage.rs`.

> Every change to a tree is recorded in the same transaction that makes it, and delivered in causal order, exactly once, to consumers that can stop and resume. Snapshot comparison (`pgfs diff`, specified in Branchable Trees) complements it for changes older than the feed's retention.

**Tutorial** · [How-to](../how-to/changefeed.md) · [Reference](../reference/changefeed.md) · [Explanation](../explanation/changefeed.md)

---

In this tutorial we will follow changes to a tree as they
happen, then compare two snapshots. We need `main` mounted at
`/mnt/main`.

## Step 1: start following

```bash
pgfs changes main --follow
```

## Step 2: make changes

In another terminal:

```bash
mkdir /mnt/main/docs
echo draft > /mnt/main/docs/a.md
mv /mnt/main/docs/a.md /mnt/main/docs/b.md
```

The first terminal prints:

```text
{"op":"mkdir","path":"/docs","ino":1040,"watermark":"v1:1:771:771:0:101:"}
{"op":"create","path":"/docs/a.md","ino":1041,"watermark":"v1:1:772:772:0:102:"}
{"op":"modify","path":"/docs/a.md","ino":1041,"watermark":"v1:1:773:773:0:103:"}
{"op":"rename","path":"/docs/a.md","new_path":"/docs/b.md","ino":1041,"watermark":"v1:1:774:774:0:104:"}
```

## Step 3: compare snapshots

```bash
pgfs snapshot main@before
rm /mnt/main/docs/b.md
pgfs diff main@before main
```

```text
removed  /docs/b.md
```

## What we did

We watched changes arrive in order and compared two points
in time, without walking the tree.

---

<small>Generated from `spec.changefeed.ChangeFeed` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

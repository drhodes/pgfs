# How-To: Change Feed

!!! success "Status: Stable"
    Every mutating function records its change with paths resolved in the transaction; delivery by transaction-id horizon and sequence cap, resumable consumers, retention, coalesced notifications from housekeeping and `pgfs changes` are built and tested in `tests/storage.rs`.

> Every change to a tree is recorded in the same transaction that makes it, and delivered in causal order, exactly once, to consumers that can stop and resume. Snapshot comparison (`pgfs diff`, specified in Branchable Trees) complements it for changes older than the feed's retention.

[Tutorial](../tutorials/changefeed.md) · **How-to** · [Reference](../reference/changefeed.md) · [Explanation](../explanation/changefeed.md)

---

## Keep an external index in sync

```bash
pgfs changes register search-indexer main
pgfs changes main --consumer search-indexer --follow | my-indexer
```

After a restart the indexer resumes where it left off.

## Recover a consumer that fell too far behind

If a fetch fails with `cursor expired`, rebuild from a
snapshot and restart from its watermark:

```bash
pgfs snapshot main@resync
# rebuild the index from a read-only mount of main@resync
pgfs changes register --at-snapshot main@resync search-indexer main
```

## Read the feed from SQL

```sql
SELECT * FROM pgfs.changes_next('search-indexer', 1000);
SELECT pgfs.changes_ack('search-indexer', 'v1:1:774:774:0:104:');
```

---

<small>Generated from `spec.changefeed.ChangeFeed` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

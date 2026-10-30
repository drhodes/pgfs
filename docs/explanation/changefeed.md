# Explanation: Change Feed

!!! success "Status: Stable"
    Every mutating function records its change with paths resolved in the transaction; delivery by transaction-id horizon and sequence cap, resumable consumers, retention, coalesced notifications from housekeeping and `pgfs changes` are built and tested in `tests/storage.rs`.

> Every change to a tree is recorded in the same transaction that makes it, and delivered in causal order, exactly once, to consumers that can stop and resume. Snapshot comparison (`pgfs diff`, specified in Branchable Trees) complements it for changes older than the feed's retention.

[Tutorial](../tutorials/changefeed.md) · [How-to](../how-to/changefeed.md) · [Reference](../reference/changefeed.md) · **Explanation**

---

## Written with the change, not after it

Each change record is inserted by the same transaction that
makes the change. If the change commits, so does its record;
if it rolls back, neither exists. A feed built any other way
can disagree with the filesystem after a crash.

## Why a horizon instead of a sequence number

Sequence numbers are handed out when rows are inserted, but
transactions commit in a different order. A consumer that
reads "everything after 100" can miss 99 if its transaction
commits a moment later. The feed advances by the oldest
transaction still running instead: below that point nothing
can change any more, so each batch is final.

## Why expired consumers are told

A consumer that silently skips events builds a wrong index
and never knows. pgfs keeps changes until every registered
consumer has them, up to a limit, and tells a consumer that
passed the limit to resync.

---

<small>Generated from `spec.changefeed.ChangeFeed` by `scripts/build_docs.py`. Edit the specification, not this page.</small>

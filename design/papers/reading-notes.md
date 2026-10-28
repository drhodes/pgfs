# Reading notes: the GFS paper and its bibliography

Notes taken while reviewing `design/sharded_filesystem.md` against the GFS
paper (SOSP 2003) and the twelve papers it cites. Section numbers match the
GFS bibliography; PDFs are in `open-papers/` and `closed-papers/` (see the
READMEs there for sources). Each section ends with what the paper implies for
pgfs. The conclusions are applied in `design/sharded_filesystem.md` v0.2.


## 00 GFS — The Google File System (SOSP03)
Single master (in-memory metadata, op log, ~100B/file), chunkservers 64MB chunks R=3.
Leases → primary orders mutations; chunk version numbers detect stale replicas.
Relaxed consistency (defined/consistent/inconsistent); record append; snapshot COW.
Namespace path rwlocks; lazy GC (hidden rename, 3 days); checksums per 64KB block.
Re-replication prioritized + throttled. §8: chose central master over Frangipani/xFS-style
distributed algorithms for simplicity; "could adapt primary-copy like Harp".
Workload: overwrites ~0; pgfs (POSIX, git/editors) is the opposite.

## 01 xFS — Serverless Network File Systems (SOSP95)
- Everything anywhere: clients, storage servers, managers, cleaners on any machine.
- Manager map: globally replicated small table, index-number bits → manager.
  Rule: >=10x more map entries than managers so load can be rebalanced by
  reassigning entries (indirection, not hashing).
- Imap (index number → disk log addr of inode) split among managers.
- Storage = LFS + Zebra: each client writes its own append-only log, striped in
  segments across a STRIPE GROUP (subset of storage servers, one parity per group).
  Stripe group map globally replicated; entries marked current/obsolete so membership
  change does not require moving data: write only to current groups, read from either,
  cleaner migrates live data from obsolete groups lazily.
  -> Directly the pgfs "replication set = R of N" idea; the obsolete-group trick is the
     answer to design §3.4 (membership change).
- Stripe groups: smaller groups survive multiple failures in different groups;
  8→4 servers/group costs 8–22% bandwidth (parity overhead).
- Token-based per-BLOCK cache consistency (write ownership via manager), like Sprite/AFS.
  Index nodes cached only at managers.
- First Writer policy: file's manager co-located with creating client → 43% fewer
  network hops; 96% of overwrites are by the previous writer (write sharing rare).
- Deltas in logs carry version numbers to order conflicting updates from different
  clients during roll-forward recovery.
- Recovery bottom-up: storage server logs → manager imap (checkpoint+roll forward)
  → cache consistency state (query clients) → cleaner state. O(N^2) messages but
  parallel. Consensus for reconfiguring maps NOT implemented (prototype status).
- Prototype: 32 SPARCs, 13.9 MB/s write, 1,122 creates/s vs NFS 86/s. Manager
  count matters: 1→2 managers +80% small-write throughput.
- Lesson for pgfs: indirection table (manager map) > pure consistent hashing for
  rebalancing; per-client logs avoid write coordination; the hard unfinished part
  was consensus on the maps and recovery.

## 02 River — Cluster I/O with River (IOPADS99)
- Premise: "performance faults" (slow disk, hot spot, CPU/memory hog on one node) are
  the norm; one perturbed node out of 8 slows NOW-Sort 1.5–5x. Design for it rather
  than trying to prevent it.
- Distributed queue (DQ): randomized, credit-based push; producers send to consumers
  with few outstanding messages → faster consumers get more work, no global coordination.
  Pull-based variant for large ordered units. 97% of peak at 32 nodes.
- Graduated declustering (GD): generalizes chained declustering. Each mirrored segment
  is read from BOTH mirrors; each disk gets feedback from its consumers and biases
  toward the lagging client, so bandwidth per client equalizes (~93% of non-mirrored
  peak unperturbed; graceful degradation under perturbation instead of a cliff).
- Mirrored writes run at the speed of the slower mirror — "fundamental cost of mirroring".
- Static hash partitioning (Teradata): "performance is bad, but it never gets worse" (Gray).
- Lesson for pgfs: with R replicas, don't pick one replica per read (design §5.2 reads
  "in ring order"); spread/adapt reads across all healthy members by observed latency.
  Writes waiting on all R members inherit the slowest host's latency — the write-quorum
  rule helps but the design fans out to every member. A slow (not dead) host is the
  common case; the design only models dead/degraded after K failures.

## 03 Swift — Distributed Disk Striping (Computing Systems 91 / UCSC-CRL-91-46)
- Client + distribution agent (striping, parity, encryption — done client-side) +
  storage mediator (central: directory service, access rights, keys, resource
  reservation, transfer plan) + storage agents. Mediator off the data path; consulted
  only at session start.
- Transfer plan: striping unit chosen per request from required data rate (low rate →
  big unit on few agents; high rate → small unit over many). Pessimistic resource
  preallocation: reject requests that can't be satisfied.
- Cache coherence via mediator callbacks (flush other client's cache on open).
- Mediator metadata must be fault tolerant: "let Swift administer the metadata with high
  redundancy, or use standard database techniques such as a write-ahead log".
  Object descriptors redundant so objects can be rebuilt by scavenging storage agents.
- Prototype: UDP + sliding window (TCP+select couldn't exceed 45% of Ethernet);
  3 servers > 2x local SCSI; scales ~linearly until client NIC/CPU saturates.
- Simulation: data rate ~linear in #agents and transfer-unit size; seek + rotation
  dominate small units; software parity (5 instr/byte) makes client CPU the write
  bottleneck.
- Lesson for pgfs: the "central metadata service off the data path" split again
  (same as GFS master); block size matters most for throughput (pgfs 64 KiB);
  per-request cost dominates small I/O → batching/pipelining multi-host fan-out
  matters more than bandwidth. Client-side redundancy computation costs client CPU.

## 04 NASD — Network-Attached Secure Disks (ASPLOS98; Gibson, Gobioff et al. — Gobioff later co-authored GFS)
- Separate the FILE MANAGER (naming, directories, access control, consistency) from
  data transfer: clients talk to drives directly. Four properties: direct transfer,
  asynchronous oversight (policy encoded in capabilities, enforced by drives without
  calling the manager), cryptographic integrity, variable-length OBJECTS not blocks.
- Capabilities: public part (rights, object version, expiry, byte region) + private key
  derived from drive secret; the object's logical version number lets the manager revoke
  capabilities instantly (bump version → old caps fail → client goes back to manager).
- Cheops storage manager: striped "logical objects" over NASD objects; manager keeps the
  mapping, clients access components directly. NASD PFS scales linearly to 45 MB/s
  on 8 drives (6.2 MB/s/drive) where a beefy NFS server topped at ~22 MB/s.
- NFS/AFS ports: read/write/getattr go to drive; everything else to file manager.
  AFS callbacks broken when a write capability is issued; capability expiry bounds
  callback wait. Quota handled by escrowing byte range in the write capability.
- Cost: server "store-and-forward" overhead 80%–1300% of disk cost.
- Per-op cost dominated by communications code (70–97% of instructions).
- Lesson for pgfs: this is GFS's lineage (master = file manager, chunkservers = NASD).
  Strong case for keeping namespace/metadata in one authority and pushing only block
  I/O to many hosts. Object-version-number revocation = cheap lease invalidation idea.

## 05 AFS — Scale and Performance in a Distributed File System (TOCS88)
- Vice servers + Venus client cache manager; whole-file caching on local disk; Venus
  only involved on open/close.
- Prototype pain: stat-validation on every open → 90% of server calls were cache
  validation (TestAuth) + status; one process per client; server-side full-pathname
  traversal; location info embedded in stub directories made moving users hard.
- Redesign: CALLBACKS (server promises to notify before another client modifies) —
  removes almost all validation traffic; FIDs (volume, vnode, uniquifier) so clients do
  path resolution and servers never see pathnames; uniquifier means a fid is never
  reused; single-process LWP server; access files by inode.
- Volumes: partial subtrees, small (one per user), glued by mount points; Volume
  Location Database replicated on every server and slow-changing. Volume move by
  copy-on-write clone + incremental clones, brief freeze, atomic, forwarding left at
  old server. Per-volume quotas, read-only replicated volumes (no callbacks needed),
  clones as cheap backups/user-visible "OldFiles".
- Consistency: writes visible locally immediately, to others on close (open/close
  consistency); metadata ops visible everywhere at completion; no implicit locking.
- Result: load 1→20 only 1.9x slower vs prototype 4x at 10; beats NFS beyond ~3 load
  units; NFS hit 100% server CPU at 18 loads and lost replies (non-idempotent retries).
- Lesson for pgfs: (a) the VOLUME + replicated location DB is exactly the indirection
  the design's §3 hash ring lacks: small units, explicit mapping, movable online by
  clone+incremental. Subtree-sticky placement is already AFS-like; make the unit
  a volume with a location table rather than a hash of the top-level name. (b) Cache
  validation traffic dominates—pgfs's attr_ttl polling is the AFS-prototype mistake at
  scale; callbacks/leases (design Phase 5) matter earlier than the design assumes.
  (c) Cross-volume rename is ILLEGAL in AFS — a legitimate simplification for §8.

## 06 Coda & InterMezzo — Removing Bottlenecks (Linux Expo 99; Braam & Nelson)
- Thesis: a networked FS client can approach local-disk speed if (1) it minimizes RPCs,
  (2) the user-level cache manager is NOT consulted synchronously, (3) the local disk FS
  is used as the persistent cache.
- Consistency toolbox: guess-by-timeout (NFS), callbacks/leases/oplocks (AFS, Coda,
  SMB, Sprite), version stamps for revalidation after reconnect (check per volume
  first, then per file). Replicated servers: per-server VERSION ARRAY (vector) so
  divergent replicas can be compared and the latest picked automatically.
- ls -lR on 1,500 files: warm Coda 0.5 s vs ext2 0.26 s vs NFS ~10 s. Half of Coda's
  time = open/close round-tripping through the user-level cache manager (50x ext2).
  Fix: kernel-level callbacks so the kernel serves open/close itself until invalidated.
- Write-back permit: server breaks other clients' callbacks for a fileset, then the
  client logs modifications (CML) and ships them asynchronously; log can be optimized
  (create+delete cancel). 1000 create/close/unlink: Coda 89.6 s → 6.0 s with WB;
  ext2 0.25 s; InterMezzo (kernel journal filter over ext2) 0.5 s for mkdir/rmdir.
  Bad when many clients share a volume (callback-break storms).
- Semantics: close-to-open (AFS/Coda) rather than UNIX write/write sharing; Coda turns
  concurrent writer conflict into an explicit conflict rather than silent LWW.
- Lesson for pgfs: pgfs is the "user-level cache manager in the synchronous path"
  design (FUSE daemon doing an SQL round trip per op). The measured 4,192 vs 157 FUSE
  callbacks for attr-ttl 0 vs 30 is the same effect. Kernel-side caching with
  explicit invalidation (FUSE notify_inval_entry/inode = kernel callbacks) and
  write-back permits with a batched op log are the levers. For replicas: per-replica
  version VECTOR rather than the design's single (client_id<<32|seq) scalar.

## 07 Harp — Replication in the Harp File System (SOSP91; Liskov, Ghemawat et al.)
- Primary-copy replication (viewstamped replication lineage) behind the VFS/NFS server.
  Every operation atomic. 2n+1 servers tolerate n failures but only n+1 store data;
  the other n are WITNESSES that only vote in view changes (and log when promoted).
  3-node group = primary + backup + witness. Groups arranged so each node is primary
  of one group, backup of another, witness of a third (load spreading).
- Normal case: primary appends event record to an in-memory log, sends to backup,
  backup acks in log order, primary advances commit point (CP) and replies; second
  phase in background. Apply process writes committed records to the local Unix FS
  asynchronously. Pointers: GLB <= LB <= AP <= CP <= top. Redo log only.
- Volatile log made durable by replication + UPS (copy log to disk on power failure):
  disk writes replaced by a network round trip → faster than unreplicated NFS.
- Reads served by primary alone; external-consistency risk under partition handled by
  loosely synchronized clocks: backup promises not to start a new view before
  time+δ, primary only answers reads locally while within that promise (a LEASE).
- View change: two-phase, coordinator gathers missing log suffix; witness promoted
  receives log beyond GLB, keeps it (tape) until demoted.
- Event records must be deterministic: record the OUTCOME, not the request
  (permission checks, directory slot positions); shadows of inode/dir pages.
  Duplicate-request table replicated so retries after failover are suppressed.
- Performance: Andrew bench Harp 6:29 vs unreplicated NFS 6:54; better capacity on
  write-heavy mix.
- Lesson for pgfs: this is the model GFS §8 points to. For R=3 the design could use
  primary-copy per set (Postgres primary + synchronous standby + a witness/arbiter),
  i.e., Postgres's own synchronous replication with a quorum commit and an external
  failover arbiter, instead of client-side fan-out. Record outcomes (inode numbers,
  positions) in the replicated log, not requests. Witness idea = 2 data copies with
  3-node availability (cheaper than R=3 full copies).

## 08 Lustre whitepaper (CFS 2002)
- MDS (active + failover pair on shared storage) holds namespace and a transactional
  journal; OSTs hold file objects (on ext3/XFS via OBD drivers), striped RAID-0-like.
  Inode on MDS references objects on OSTs (in an EA). After open, I/O goes
  client↔OST directly. File locking for data distributed to OSTs (each locks its objects).
- Failure: client timeout → ask LDAP config server for replacement → reconnect.
  Epoch/incarnation/generation numbers detect restarts. A failed OST is skipped for
  NEW file creation; existing files on it error until it returns.
- Intent locks: lookup carries the intended op (e.g. mkdir), MDS executes it under the
  lock → one round trip instead of lookup + mkdir.
- MDS pre-allocates objects on OSTs to speed creates.
- Roadmap: metadata clustering (client computes which MDS), client metadata writeback
  cache with preallocated inodes + batched reintegration, collaborative read cache.
- Lesson for pgfs: the same MDS/OST split; "intent" = fold lookup+create into one SQL
  statement/transaction (pgfs already does some of this). Skip-failed-target-on-create
  is a simple degraded-mode rule. Pre-allocated inode ranges per client fixes the
  design's set-leader allocation problem (§4.3) — each client leases a range.

## 09 RAID — A Case for RAID (SIGMOD88)
- Argument: arrays of small cheap disks beat one large expensive disk on cost,
  power, performance — but MTTF of an array = MTTF_disk / N (100 disks → ~300 h),
  so redundancy is mandatory. Reliability groups of G data + C check disks.
- MTTF_group ≈ MTTF_disk² / ((G+C)(G+C−1)·MTTR): reliability depends on MTTR
  quadratically-ish → fast repair (hot spares) matters as much as redundancy.
- Levels: L1 mirroring (100% overhead, best small writes); L2 Hamming; L3 single parity
  disk (bit-interleaved, good for large transfers, terrible for small); L4 sector-
  interleaved + parity disk (independent small reads, small write = 4 I/Os
  read-modify-write, parity disk bottleneck); L5 rotate parity across all disks
  (parallel small writes). L1 best when <50% capacity matters; L5 best space-speed.
- Assumes independent, exponential failures (they flag correlated failures —
  power, earthquake — as caveat).
- Lesson for pgfs: the design calls subset replication "RAID-5-like" but it is RAID-1
  (whole copies, R=3) on a subset — RAID-5 means parity. Correct the terminology.
  Real parity (erasure coding) makes small writes a read-modify-write across hosts —
  over a network that is a distributed transaction per 64 KiB block; keep it out of
  scope as the design does. Re-sync/MTTR speed belongs in the reliability math:
  shorter repair → fewer data-loss windows (GFS's prioritized re-replication).

## 10 GPFS — Shared-Disk File System for Large Clusters (FAST02)
- Shared-disk: every node sees every disk (SAN or VSD); data striped round-robin in
  large blocks (256 KB typical; small files in 1/32 sub-blocks); prefetch/write-behind.
- Large directories via extensible hashing (one block read per lookup).
- Per-node metadata write-ahead log stored on shared disk → any surviving node can
  replay a failed node's log; user data not logged.
- Distributed locking with a central TOKEN MANAGER: once a node holds a token, repeated
  ops are local; tokens double as cache-coherence. Byte-range tokens start as whole
  file [0,∞) → zero overhead without sharing; on conflict, split by required vs desired
  range so sequential writers carve disjoint ranges with one exchange. Rounded to
  block size (false sharing → read-modify-write). For fine-grained sharing, "data
  shipping" mode (block owner by round-robin) beats locking.
- METANODE: dynamically elected per file (first accessor); writers take a shared write
  lock, send size/mtime/indirect-block updates to the metanode, which merges (max size,
  max mtime); only trunc/utimes need exclusive lock.
- Allocation map split into n regions each covering 1/n of every disk; allocation
  manager hands out regions to avoid contention; deallocations shipped to the region's
  current user.
- Token manager scaling: avoid conflicts (hints), batch/prefetch tokens, reuse tokens
  for recycled inodes; revoking node collects replies (≤2 msgs to manager).
- Fault tolerance: group membership with heartbeats; majority quorum; DISK FENCING of
  minority before log recovery; tokens of failed node held until its log is replayed.
  Replication optional (2 copies of data and/or metadata); stale replica catch-up
  tracked per block.
- Experiences: work granules must be small and equal (skew); intra-node parallelism;
  inode prefetch for `ls -l`/stat scans (~10x); rare failures DO happen (RAID microcode
  bug → metadata replication on top of RAID); correlated failures (bad disk batch)
  → online rebalancing, don't assume independent failures.
- Lesson for pgfs: (a) "metanode" = per-file single writer for metadata, elected via a
  token — the right fix for the design's per-entry LWW conflicts. (b) Disk fencing
  before recovery = in pgfs, the failed primary must be fenced (e.g. revoke/terminate
  its sessions, or epoch number checked in every write) before another takes over.
  (c) stat/readdir-heavy workloads (git, ls -l) need batched attribute prefetch —
  pgfs could return attrs with readdir (READDIRPLUS) in one query.

## 11 The Global File System (NASA Goddard 96; Soltis, Ruwart, O'Keefe — Minnesota, later Red Hat GFS/GFS2)
- Shared-storage (shared-disk) vs message-based survey: NFS (stateless server),
  Sprite (server callbacks, disable caching when write-shared), AFS/Coda, xFS,
  VAXcluster (DLM with lock modes; per-lock SEQUENCE NUMBERS for cache coherence),
  Cray SFS (semaphore server; single writer/multiple readers).
- GFS: nodes share Fibre-Channel disks in a Network Storage Pool split into subpools.
  Caches data on the DEVICES, not node memory (node caches only during a request).
- Resource groups = mini file systems (bitmaps, dinodes, data) distributed across
  devices; superblock holds RG index + per-node unique-id bitmaps.
- Dinode = whole block, all data at uniform tree height.
- Device locks (Dlock): test-and-set / clear on a 1-bit lock stored on the disk, each
  with a logical CLOCK incremented on use. Stale lock recovery: if a lock's clock hasn't
  moved for a long time, a node may clear it — but only by presenting the current clock
  value (compare-and-clear), so a racing second clearer is ignored. Locks acquired in
  increasing order (no deadlock). ~1024 locks/device: one for superblock, one per RG,
  rest hashed over dinodes.
- Prototype on parallel SCSI with Reserve/Release (whole-device) — contention kills
  speedup unless requests are spaced out; 2–10x faster than NFS over 10 Mb Ethernet.
- Lesson for pgfs: Postgres IS the "smart shared device" — row locks and
  SELECT … FOR UPDATE / advisory locks are the Dlock analogue. Clock-guarded lock
  clearing = compare-and-swap on a version/epoch column: a recovering client must
  present the epoch it saw. Resource groups ≈ per-set inode/alloc ranges.

## 12 Frangipani (SOSP97; Thekkath, Mann, Lee — DEC SRC)
- Two layers: Petal (replicated, sparse 2^64-byte virtual disk; snapshots) + identical
  Frangipani servers sharing it, coordinated by a distributed lock service. Servers
  never talk to each other — only to Petal and the lock service → trivial add/remove.
- Disk layout exploits sparse address space: per-server log regions (256 logs),
  per-server allocation-bitmap segments (each server allocates inodes from its own
  portion → no contention, any server can read/write any inode), 512 B inodes,
  4 KB small blocks, 1 TB large blocks.
- Per-server write-ahead redo log of METADATA only (user data not logged), in Petal so
  another server can run recovery. Per-512B-block version numbers prevent replaying
  a log record already applied (freed metadata blocks reused only as metadata).
- Locks: multiple-reader/single-writer, sticky; one lock per file/dir (inode+data),
  per log, per bitmap segment. Deadlock avoided by two-phase: discover locks needed,
  sort by inode address, acquire, re-validate, retry. Dirty data flushed to Petal on
  write-lock release/downgrade (no cache-to-cache transfer) → recovery only ever needs
  the failed server's log.
- Lock service: leases (30 s); lost lease → discard cache, refuse writes until
  remount. Three implementations; final = Paxos-replicated global state, ~100 lock
  groups, clerks. Known hazard: a server whose lease expired may still have an
  in-flight write reach Petal — mitigated by a 15 s margin; proper fix = include
  lease id/expiry in each write and have storage reject stale ones (= FENCING TOKEN).
- Backup: crash-consistent Petal snapshot; or barrier lock for FS-consistent snapshot.
- Performance: comparable to AdvFS on Andrew/Connectathon; linear read scaling;
  write scaling to network limit. Whole-file locks collapse under write-sharing
  (15 MB/s → ~1 MB/s with 2+ writers); read-ahead hurts under contention.
- Lesson for pgfs: closest model to what pgfs could become — many FUSE clients over
  one shared consistent store (Postgres playing Petal), coordination via locks/leases,
  per-client inode ranges, and the fencing-token fix for expired leases. Postgres
  already gives transactions, so pgfs needs much less: no client logs, no recovery
  daemon. What it lacks is the lease/callback layer for client cache coherence.


# Part 2: the wider reading list (r00–r135)

Read against the whole specification in `spec/` (app, optimize, offload,
replica, shard, perf, observe, err), not only the sharding design. Each
entry ends with **For the spec**, naming the spec classes it bears on.
Paper numbers match `README.md`.

## r00 Inversion — The Design and Implementation of the Inversion File System (USENIX Winter 93; Olson)
- The direct ancestor of pgfs: a file system inside POSTGRES 4.0.1. Namespace table
  `naming(filename, parentid, file)` keyed by OBJECT ID, not path; attributes in a
  separate `fileatt(file, owner, type, size, ctime, mtime, atime)`; data in one table
  per file (`inv<oid>`) of ~8 KB chunks (sized to fit one page) with a B-tree on chunk#.
- Small sequential writes inside one transaction are coalesced into full chunks.
- Services "for free" from the DBMS: transactions spanning many files (p_begin/
  p_commit/p_abort), instant crash recovery (no fsck), time travel (no-overwrite
  storage; p_open takes a timestamp; old versions read-only), typed files with
  user-defined functions, ad hoc queries over metadata AND contents.
- Self-identifying blocks (tag each chunk with file id + chunk#) proposed to detect
  media garbage; space reserved but not implemented.
- Performance: 30–80% of ULTRIX NFS (which had NVRAM). Costs: B-tree index maintenance
  interleaved with data writes on create (36% of NFS), extra buffer allocation/copy,
  heavyweight TCP client protocol. Running the workload as functions INSIDE the data
  manager (no client/server hop) beat NFS on most tests — the original argument for
  in-database execution.
- Unsolved even then: transactions over NFS (every NFS op is its own transaction).
- **For the spec:**
  - Inversion keyed names by object id from day one; pgfs's path-keyed `entries`/
    `blocks` (ConnectsToPostgres, RenamesEntries) is the design Inversion avoided.
    Supports InoBasedRekeyingReq as the default, not an optimisation.
  - Separate naming and attribute tables is worth considering when hard links come
    (one inode, many names) — FlatFilesystem lists `link` as unsupported.
  - Chunk sized to a page: pgfs's 64 KiB block is TOASTed (compressed, out of line,
    chunked again at ~2 KB). Block size should be measured, not chosen; ChunkedBlockStorageReq
    still says "64KB or 128KB" and the two paths disagree.
  - LocalPgrxEngineReq is Inversion's "run it in the data manager" result, 30 years on.
  - Time travel is free in a no-overwrite store; PostgreSQL vacuums old versions away,
    so a pgfs snapshot feature would need explicit versioned blocks (ties to
    ReflinkBlockSharingReq/CloneSubtreeReq).
  - Self-identifying blocks: `blocks` rows already carry (ino, block_no); a checksum
    column would give end-to-end detection (see GFS notes).

## r01 Chain Replication (OSDI04; van Renesse, Schneider)
- Replicas of an object form a chain: updates enter at the head and flow down reliable
  FIFO links; the TAIL answers both queries and update replies. Strong consistency
  because all replies come from one server; the head computes non-deterministic
  updates once and ships state, so replicas only apply writes.
- Fail-stop servers; a (Paxos-replicated) master detects failures and splices the
  chain. Invariants: each node's history is a prefix of its predecessor's; `Sent_i`
  lists forwarded-but-unacked updates, re-sent on splice. New replicas join at the tail
  and receive state while the chain keeps serving.
- Outages: tail failure 2 message delays, head 2 (updates only), middle 0; primary/
  backup primary failure 5. Failure DETECTION (10 s in their sims) dominates.
- Weak variants (read any replica) only win below ~15% updates.
- Placement: consistent-hash "ring" placement limits parallel re-replication to t, so
  MTBU falls as servers grow; random placement + parallel recovery (GFS-style) wins.
- ROWAA (read one, write all available) beats quorum systems for availability and
  throughput except write-only loads.
- **For the spec:**
  - shard.py's ComputesDeterministicPlacement (consistent hashing over a ring of sets)
    is exactly the "ring" strategy this paper measures as worst for availability at
    scale; v0.2 already moved to a placement table — the spec must follow.
  - RequiresWriteQuorum/ServesReadsFromReplicationSet (quorum + `ver`) is a weaker,
    costlier design than a primary-ordered log; the v0.2 choice (Postgres primary +
    synchronous standbys per data set, failover by an external master) is the
    primary/backup row of this paper's comparison.
  - A client cannot distinguish a transient outage from a lost reply: pgfs must make
    retried mutations idempotent (create/rename after reconnect) — nothing in the spec
    says so (see Bug 5, no reconnection).

## r02 Ceph (OSDI06; Weil et al.)
- Three parts: clients (user space, FUSE or linked library), a metadata server (MDS)
  cluster, and RADOS object storage. No allocation tables: object name = (ino, stripe#);
  CRUSH maps placement groups → OSDs from a compact cluster map, so anyone can compute
  where data lives.
- Capabilities (read, cache, write, buffer) issued by the MDS on open; with multiple
  writers or mixed readers/writers the MDS revokes caching → synchronous I/O. O_LAZY
  and lazyio_propagate/synchronize as opt-out. Close returns the new size (largest
  offset written) to the MDS.
- stat on a file open for writing by several clients: MDS revokes write caps to collect
  size/mtime. readdir returns inodes (readdirplus); a stat right after readdir is
  served from that brief cache. Long attribute TTLs rejected because "stat to detect
  change" is common.
- MDS: large lazily flushed journals absorb repeated updates; inodes embedded in
  directory objects (one read fetches a directory); inode numbers allocated in ranges
  per MDS; anchor table only for multiply-linked inodes. Dynamic subtree partitioning
  by decayed popularity counters; hot directories replicated (reads) or hashed (creates).
- RADOS: primary-copy per PG, primary assigns version, forwards to replicas; ack when in
  all replicas' memory, commit when on disk ("synchronization vs safety"). Cluster map
  epochs tag every request; OSDs peer per PG using version + recent-change log; down vs
  out distinction avoids mass re-replication after e.g. a power loss.
- FUSE lessons: DIRECT_IO broke mmap; FUSE's own permission checks cause copious
  getattrs; page-sized transfers cap throughput → "an in-kernel client is inevitable".
- **For the spec:**
  - Ack-vs-commit is what pgfs's `synchronous_commit = off` collapses: fsync returns at
    "ack". A durable fsync needs the commit notification. SynchronousCommitTuningReq
    should become per-operation (off for metadata batches, on/remote_apply for fsync).
  - Close-to-open + "client reports size at close" matches v0.2's
    `size = GREATEST(size, ...)` rule; FlatFilesystem's Reference should state it.
  - readdirplus: fuser exposes `readdirplus`; ListsDirectories + LooksUpEntries cost a
    lookup per entry today; one query already returns all attributes.
  - Epoch-tagged requests = v0.2's set epochs/fencing; down vs out = v0.2 "drain".
  - Inode ranges per allocator = v0.2 §5 option.

## r03 CRUSH (SC06; Weil, Brandt, Miller, Maltzahn)
- Placement as a deterministic pseudo-random function of (object group id, weighted
  hierarchical cluster map, placement rule); no per-object directory. Rules
  (take/select(n,type)/emit) separate replicas across failure domains (row, cabinet,
  shelf) — the thing plain consistent hashing (Sorrento) lacks.
- Bucket types trade mapping speed against data movement on change: uniform (O(1),
  full reshuffle), list (optimal for additions only), tree (O(log n)), straw
  (optimal add/remove). Failed devices stay in the map and are skipped (movement =
  w_failed/W, optimal); overload correction diverts a fraction of an overfull or slow
  device's data ("partial failure").
- Declustering (many peers per device) speeds parallel recovery but raises the chance
  a correlated event hits shared data; failure domains are the answer.
- **For the spec:**
  - Even CRUSH maps objects to placement GROUPS first (~100 per OSD), then groups to
    devices: indirection again. shard.py's top-level-name hashing has neither groups
    nor failure domains nor weights. v0.2's `data_sets` table should record a failure
    domain per member and a weight per set, and placement should pick by weight.
  - "Treat a slow device as partial failure and divert some of its load" — pgfs's
    replica routing (FallsBackToPrimary) is binary fresh/stale; a weight would allow
    proportional diversion.


## r04 Chubby (OSDI06; Burrows)
- Coarse-grained lock service + small-file store, 5 replicas per cell, Paxos-elected
  master holding a master lease; reads served by the master alone while the lease holds.
- File-system-like namespace deliberately simplified for distribution: no cross-
  directory move, no directory mtimes, no path-dependent permissions, no links,
  no atime. Per-node monotonically increasing instance / content-generation / lock-
  generation / ACL-generation numbers + content checksum so clients detect change.
- SEQUENCERS: lock holder passes (lock name, mode, generation) to the server it writes
  to; server rejects stale ones. Simple fencing: "acquire a lock, pass the acquisition
  count, reject writes with a lower count". Lock-delay for servers that can't check.
- Consistent write-through client caching via invalidations piggybacked on
  KeepAlives; a write blocks until all caching clients ack or their lease expires.
  Developers loved not choosing a TTL (contrast DNS). Negative caching (absence)
  turned out essential.
- Sessions: lease (12 s default), jeopardy → grace period (45 s) with cache disabled
  and calls blocked; new master picks a new epoch, rejects old-epoch calls, rebuilds
  state from clients, waits a worst-case lease before proceeding. "Fail-over code …
  a rich source of interesting bugs". Only a SUFFIX of a handle's operations can be lost.
- Ops lessons: data lost 6 times in dozens of cell-years — software (4) and operator
  (2), none hardware. Clients crashed on fail-over events (worse availability).
  TCP backoff ignores lease deadlines → KeepAlives over UDP. Quotas needed (one
  service rewrote a 1.5 MB file per user action).
- **For the spec:**
  - KernelAttrCacheTtlReq picks a TTL — the approach Chubby argues against. v0.2's
    LISTEN/NOTIFY + FUSE invalidation is the Chubby model; `--attr-ttl` should become
    the fallback, not the coherence mechanism. Negative entries (ENOENT with TTL) are
    worth caching too: FUSE supports negative dentries via a zero-ino entry reply.
  - Generation numbers per entry ≈ v0.2 `data_ver`; exposing a generation via
    `st_ino` reuse prevention and `FileAttr.generation` (FUSE has a generation field)
    would let NFS-export and caches detect reuse. Spec never mentions generation.
  - Sequencers = v0.2's `set_identity`/epoch fencing; good precedent.
  - "Clients crash on fail-over events" warns against making pgfs exit on a lost
    connection; DetectsDeadlock exits the process — check it can't fire on a DB outage.
  - Quotas: nothing in the spec bounds a user's rows; the operator dashboard needs it.

## r05 RADOS (PDSW07; Weil, Leung, Brandt, Maltzahn)
- Monitor cluster (Paxos, leases) owns a versioned cluster map; every message carries
  the sender's map EPOCH; stale peers are sent incremental map diffs lazily
  (gossip-like, O(log n) spread). Objects → PG (hash & mask) → OSD list (CRUSH).
- Device state has two axes: up/down (reachable) and in/out (assigned data). "Down but
  in" = degraded, no data movement yet; "up but out" = burn-in or draining.
- Replication schemes: primary-copy, chain, and "splay" (parallel updates, reads at
  tail). Reads need recent heartbeats from PG peers (H s) — a primary cut off from its
  peers stops serving reads before a new primary takes over (a read lease).
- Peering: new primary gathers PG logs from every OSD that may have served the PG since
  the last successful peering (the "prior set"), so intermediate epochs are not lost;
  I/O resumes once logs agree, objects recover in the background, primary pushes each
  recovered object once to all replicas.
- OSDs that learn they were marked down sync and kill themselves.
- **For the spec:**
  - The up/down × in/out matrix is the right shape for v0.2 `data_sets` member state
    (shard.py's DegradesGracefullyOnHostFailure has only healthy/degraded).
  - Read leases: a pgfs replica read (ServesReadsFromReplica) after a failover must not
    come from an ex-primary that lost its role; freshness (LSN compare) alone does not
    prove the node is still in the cluster — a timeline/system-identifier check does.
  - "Kill yourself when marked down" is the self-fencing behaviour v0.2 wants from a
    demoted data-set primary; with Postgres that is pg_auto_failover/Patroni's job.

## r06 CRAQ — Chain Replication with Apportioned Queries (ATC09; Terrace, Freedman)
- Any chain node serves reads: each node keeps versions marked clean/dirty; a clean
  latest version is returned locally; if dirty, the node asks the tail for its committed
  version number (a tiny "version query") and returns that version. Strong consistency
  kept; read throughput scales with chain length (3× for 3 nodes, ~2× under heavy writes).
- Optional weaker reads: eventual, or bounded staleness by time or version count.
- Test-and-set on version number at the head; single-chain mini-transactions need only
  the head; multi-chain via optimistic 2PC among heads.
- Membership via ZooKeeper ephemeral nodes + watches; join anywhere needs
  back-propagation; writes stall for the failure-detection interval when a member dies.
- **For the spec:**
  - The "version query" is the cheap way to make replica reads strongly consistent:
    pgfs's freshness check compares LSNs per 500 ms; per-read, a standby could serve an
    entry if `entries.data_ver` (v0.2) matches the primary's — a one-integer query.
    Better: wait-for-LSN (read your own commit LSN on the standby). ReplicaFreshnessCacheReq
    explicitly allows reads up to 500 ms stale, including after this daemon's own write —
    a read-your-writes violation the spec documents but should fix.
  - Bounded-staleness reads as an explicit option (`--replica-max-lag`) is better than an
    implicit 500 ms.

## r07 TableFS (ATC13; Ren, Gibson)
- FUSE file system that stores ALL metadata and small files (<4 KB) in one LevelDB table;
  large files go to an underlying ext4 as objects named by inode. Row key =
  (parent inode, name) — sorted so a directory is one prefix range; value = struct stat
  (+ inline data). Global inode counter; hard links get an extra row keyed (ino, "").
- Rename = atomic batch (insert new key, delete old); chmod/utime need read-modify-write
  under a light lock layer. WAL synced every 5 s ("ordered-mode-like").
- 2–10× faster than ext4/XFS/Btrfs on metadata workloads even through FUSE; FUSE costs
  ~3× vs a library build and doubles caching (FUSE's inode cache competes with the
  underlying fs). readdir+stat and readdir+read win because attrs/data are co-located.
- **For the spec:**
  - The (parent_ino, name) key is the layout InoBasedRekeyingReq specifies; TableFS shows
    one table keyed that way can carry attrs + inline small data. For pgfs: inline small
    files in `entries` (avoid a `blocks` row for files < N bytes) is a measurable win for
    git-like workloads (most objects are small). Not in the spec.
  - Readdir returning full attributes (READDIRPLUS) is where co-location pays off.
  - Their "FUSE double caching" warning applies: pgfs + Postgres shared_buffers + kernel
    page cache = three caches of the same block.

## r08 Raft (ATC14; Ongaro, Ousterhout)
- Replicated log with a strong leader; terms as a logical clock; randomized election
  timeouts; AppendEntries consistency check (prevLogIndex/term) makes follower logs
  converge; election restriction (vote only for an at-least-as-up-to-date log) gives
  Leader Completeness; only current-term entries are committed by counting.
- Safety never depends on timing; availability needs broadcastTime ≪ electionTimeout ≪ MTBF.
- Membership change via joint consensus (C_old,new needs both majorities); new members
  join as non-voting learners first; removed servers must not disrupt with higher terms.
- Clients (§7/tech report): commands carry unique client serial numbers so a retried
  command is not applied twice; linearizable reads need the leader to commit a no-op
  in its term and confirm leadership by heartbeat (or a lease).
- **For the spec:**
  - pgfs delegates consensus to Postgres + an external failover manager (pg_auto_failover
    keeps its own Raft-like state machine; Patroni uses etcd/Consul, i.e. Raft). The
    spec should say so explicitly rather than re-invent quorum (shard.py does).
  - Client serial numbers: pgfs mutations retried after a dropped connection (Bug 5
    fix) need an idempotency key — e.g. a `client_op_id` recorded in the same
    transaction, or "check then act" on retry. Add to the reconnection requirement.
  - Learner-first joining = v0.2 `set add` should attach a new data-set member as an
    asynchronous standby, then promote it to synchronous once caught up.

## r09 IndexFS (SC14; Ren, Zheng, Patil, Gibson)
- Metadata middleware over PVFS/HDFS/Lustre/PanFS: TableFS-style LSM table per server,
  key = (parent ino, hash(name)), small files (<64 KB) inline. Directories placed by
  "power of two choices"; big directories split by GIGA+ hash-range halving; clients
  cache partition→server maps INCONSISTENTLY (wrong server redirects).
- Stateless consistent client caching of path components (name + permission only):
  server hands out short leases and only remembers the max expiry per entry; a mutation
  waits for leases to expire. Lease length adaptive: 3 s/depth, or by read/write ratio.
  Rate-based leases gave the best throughput and tail.
- Bulk insertion: `mkdir LOCALIZE` gives a client a write lease on a new subtree; it
  creates entries locally into an SSTable and submits it in one step (113× creates).
- Fault tolerance pushed into the underlying DFS; standby servers replay WAL; WAL async
  by default (5 s / 16 KB); rename limited to files and leaf dirs.
- **For the spec:**
  - Path-component leases with server-side "max expiry" are a cheap middle between
    `--attr-ttl` (no invalidation) and per-client callbacks. FUSE lets pgfs choose a TTL
    per reply: directories near the root could get long entry TTLs, leaf files short
    ones — a refinement of KernelAttrCacheTtlReq worth specifying.
  - Batch-create (untar, git checkout) is pgfs's worst case (one transaction per create
    with today's code). A per-handle or per-directory batching window would be the
    pgfs analogue of bulk insertion; TransactionBatchingReq covers only single ops.

## r10 CalvinFS (FAST15; Thomson, Abadi)
- Metadata in Calvin, a deterministic distributed database: requests are appended to a
  Paxos-replicated (meta)log first, then every replica executes them in log order with
  deterministic locking — no 2PC, no inter-replica coordination at execution; WAN
  latency paid once at log append.
- Metadata hash-partitioned by FULL PATH; each entry stores type, permissions, the
  permissions of EVERY ANCESTOR (so a permission check reads one entry), and contents
  (directory: child names; file: list of (block id, range)). Recursive ops (dir rename,
  chmod -R) are compound transactions via OLLP (analyze dry-run → run, abort+retry if
  the set grew).
- Blocks: variable size 1 B–10 MB, IMMUTABLE; appends/overwrites write a new block and
  edit the range map; background compaction rewrites fragmented files (only if the file
  didn't change meanwhile). Block→bucket→servers map in a Paxos config file.
- Read modes: linearizable (through the log), recent snapshot (≤400 ms stale), or
  client-specified snapshot/timestamp bound. ~140 B per metadata entry.
- **For the spec:**
  - CalvinFS is the "keep paths as keys" design taken to its conclusion: it accepts
    O(subtree) directory rename and chmod. pgfs's RenamesEntries has the same cost
    profile today. Their answer for scale was a deterministic DB; pgfs's answer should
    be InoBasedRekeyingReq.
  - Immutable blocks + range map = copy-on-write: makes reflink, snapshots and safe
    concurrent readers trivial, and makes "data before metadata" ordering natural
    (write new block, then swing the pointer). Worth an explicit comparison against
    in-place `blocks` upserts in the offload/sharding designs.
  - Explicit read modes (linearizable / bounded-stale / snapshot) are a cleaner way to
    specify ReplicatedFilesystem than an implicit 500 ms cache.

## r11 HopsFS (FAST17; Niazi et al.) — the closest relative of pgfs's metadata design
- HDFS namenode replaced by stateless namenodes over MySQL Cluster (NDB). Fully
  NORMALIZED metadata: inode row keyed (parent_inode_id, name), partitioned by
  parent id so a directory listing is one partition-pruned index scan; per-file block/
  replica tables partitioned by the file's inode id. Top two levels hash-partitioned
  to avoid the root hotspot; the root inode is immutable and cached everywhere.
- Only cheap DB ops on hot paths: primary-key, batched PK, partition-pruned scans;
  never full or global index scans.
- NDB offers only READ COMMITTED, so HopsFS adds row locks in a TOTAL ORDER (root→leaf,
  left-ordered DFS) and takes the strongest lock up front (no lock upgrades) to avoid
  deadlocks. Transaction template: lock phase (batched read of all path components
  via an "inode hint cache" of primary keys — N round trips → 1; lock last
  component) → execute against a per-transaction cache → batched update phase.
- Subtree ops (mv, rm -rf, chmod -R, quota) too big for one transaction: set a
  persisted subtree-lock flag on the root (with the owning namenode id), quiesce the
  subtree by taking/releasing write locks top-down in parallel, then apply in batches;
  move/chmod only touch the subtree ROOT in the final transaction; delete is post-order
  in batches, so a crash leaves a smaller, still-connected tree to resubmit. Locks of
  dead namenodes are cleared lazily (leader election via the DB itself).
- Results: 16× HDFS on Spotify trace, 37× with 20% creates; no failover downtime
  (clients just retry on another namenode); ~1.5× memory vs HDFS; p99 create ~100 ms.
  Metadata in SQL enabled ad hoc analytics, xattrs via FK tables, Elasticsearch export.
- **For the spec:**
  - Direct validation of InoBasedRekeyingReq + v0.2 §3: key `(parent_ino, name)`,
    cluster by parent; blocks keyed by `ino`. Make it the target schema, not a
    "Tier 1 optimisation".
  - Lock ordering: pgfs rename touches two parents and possibly a victim; to avoid
    deadlocks under concurrent dispatch (MultiThreadedConnectionPoolReq), lock rows
    in a fixed order (e.g. ascending ino) and use SELECT … FOR UPDATE up front.
    No spec requirement covers concurrency control at all today.
  - Inode hint cache = pgfs's in-memory ino↔path maps; HopsFS shows they must be
    treated as hints validated by the DB, not as truth (relevant once several
    daemons share a database: a cached path may have been renamed elsewhere).
  - Subtree-op protocol: if pgfs keeps path keys, a big directory rename is one huge
    transaction (WAL, locks, bloat). Even with ino keys, `rm -rf` of a million files
    via FUSE is a million unlinks — fine — but server-side recursive ops in
    CloneSubtreeReq would need batching + a persisted subtree flag.
  - Stateless frontends + DB as the source of truth = pgfs daemons; their
    "no downtime on frontend failure" result applies to multiple pgfs mounts.

## r12 To FUSE or Not to FUSE (FAST17; Vangoor, Tarasov, Zadok)
- FUSE internals: 43 request types; five kernel queues (interrupts > forgets (16 per 8
  others) > pending; processing; background for async READ/writeback WRITEs, capped by
  `max_background` (12) with congestion at 75%). Splice for >1-page payloads. Multi-
  threaded library spawns threads on demand (cap ~10 idle + 12 bg). Without writeback
  cache every write is synchronous and 4 KB; with it, writes are batched up to max_write
  (128 KB) — but only SEQUENTIAL writes are merged.
- Results (passthrough Stackfs vs ext4): from +6% to −83%. Sequential I/O fine
  (readahead 128 KB evens out request size). Worst: file creates (GETATTR, LOOKUP,
  CREATE, WRITE, FLUSH serially per file + FORGET churn) and metadata-heavy macro
  workloads on fast SSDs; single-threaded daemon is catastrophic for parallel apps
  (web-server: 31 of 32 threads wait). Optimizations can hurt (more threads → extra
  sync for same-file reads; writeback cache adds async throttling for random writes).
- Instrumentation: log2 latency histograms per request type per queue, exported via
  fusectl and SIGUSR1 — same approach as pgfs's ExportsPeriodicMetrics.
- **For the spec:**
  - KernelWritebackCacheReq is the single biggest data-path win (4 KB sync → 128 KB
    batched) and pgfs leaves it off; it requires correct handling of kernel-initiated
    writes after close, mtime/size ownership by the kernel, and `O_APPEND`. Specify
    those preconditions.
  - pgfs negotiates only FUSE_BIG_WRITES and runs single-threaded: the paper's worst
    configuration for parallel workloads. MultiThreadedConnectionPoolReq is blocked on
    `&mut self`; this should be a top-priority requirement, with max_background tuned.
  - Creates cost 5 serial round trips; pgfs adds 1–2 SQL round trips to each. Batched
    or pipelined create (or replying from cache: lookup negative-entry TTL) matters.
  - pgfs's latency histogram buckets are decade-wide; FUSE's log2 buckets per op type
    are finer and cheap — upgrade ExportsPeriodicMetrics to per-op log2 histograms.

## r13 ExtFUSE (ATC19; Bijlani, Ramachandran)
- eBPF extensions loaded by the FUSE daemon serve requests in-kernel: lookup/getattr/
  getxattr replies cached in eBPF maps with explicit invalidation hooks (unlink, rmdir,
  rename, setattr run their kernel extension BEFORE forwarding, so invalidation is
  race-free); passthrough read/write to a lower fd for stacked file systems.
- The VFS issues GETXATTR(security.capability) before every write: with xattr support
  this costs 27–44% of write throughput unless cached. Kernel build: lookups −75%,
  getattr −99%.
- Not upstream as such; Linux 6.9 FUSE passthrough is the mainline descendant.
- **For the spec:**
  - pgfs does not implement getxattr; the kernel then caches ENOSYS/ENOTSUP after the
    first call and the write-path tax disappears. If xattrs are ever added (the
    Reference lists them as unsupported), negative-cache `security.capability` or the
    tax returns. Write that constraint into any future xattr requirement.
  - FUSE passthrough is irrelevant to pgfs (no lower file), but the invalidation lesson
    is: invalidate BEFORE applying a mutation that a cache may contradict. pgfs with
    long TTLs must call `notify_inval_entry` for names it renames/unlinks on behalf of
    OTHER daemons (v0.2 §11).

## r14 Tectonic (FAST21; Pan et al., Facebook)
- Exabyte multitenant FS; metadata DISAGGREGATED into Name (dir_id, name), File
  (file_id → blocks) and Block (blk_id → chunks, plus reverse disk→blocks index)
  layers, each HASH-partitioned in ZippyDB (Paxos-replicated RocksDB shards; no
  cross-shard transactions). Lists are "expanded" (one key per child) so a create
  doesn't rewrite the directory.
- Consistency: read-after-write for single-object ops and same-directory renames
  (in-shard transaction); cross-directory moves are NON-atomic two-step with a parent
  backpointer; a rename re-checks in its final in-shard transaction that the file id
  hasn't changed since step 1 (to beat a concurrent create-overwrite).
- Single writer per file enforced by a write TOKEN in file metadata; a new opener
  replaces the token and seals the previous writer's blocks (fencing).
- Sealed objects (files/dirs/blocks) are immutable → cacheable anywhere.
- Partial-block quorum appends: ack after 2 of 3; commit the post-append size +
  checksum to block metadata BEFORE acking; readers only read up to the committed size.
- Ops lessons: hash partitioning beats range partitioning for hotspots; listing APIs
  return file IDs so workers open by id instead of re-hitting the directory shard;
  reconstruction storms (cap reconstructed reads at 10%); memory corruption is common
  at scale → checksums carried across every API boundary and verified through inverse
  transforms; no recursive du (stale periodic stats instead); metadata latency rose vs
  HDFS so clients parallelised renames.
- **For the spec:**
  - "Commit size+checksum to metadata before ack; readers bound by committed size" is
    exactly v0.2's ordered-write rule; good external validation.
  - Write token per file = fencing for v0.2 multi-client writes; pgfs has no notion of
    a writer token. Close-to-open with last-writer-wins on `size` is weaker.
  - End-to-end checksums: pgfs blocks have none beyond Postgres page checksums (off by
    default in initdb!). Spec should require `initdb --data-checksums` in the
    project-local cluster (ProvidesProjectLocalCluster) and consider a per-block
    checksum column.
  - Returning ino with readdir entries (READDIRPLUS) mirrors their "list returns IDs".

## r15 FoundationDB (SIGMOD21; Zhou et al.)
- Unbundled: control plane (coordinators, disk Paxos) recruits a Sequencer; data plane
  = stateless Proxies + Resolvers (OCC conflict check over key ranges) + LogServers
  (synchronous f+1 WAL replication) + StorageServers (pull redo asynchronously, serve
  MVCC reads within a 5 s window). Strict serializability from OCC+MVCC.
- Failure handling by RECONFIGURATION, not quorums: any TS/LS failure → sequencer
  exits → new epoch; recovery only finds the end of the log (PEV/RV), no redo/undo
  on the critical path; median 3 s. Tolerates f failures with f+1 replicas.
- Deterministic simulation testing from day one (Flow actors; network, disk, time,
  RNG simulated; "buggification" injects legal-but-rare behaviour; swarm testing;
  coverage macros to ensure rare conditions are hit). Zero data corruption in 0.5M
  disk-years; ZooKeeper was dropped after simulated faults found two bugs in it.
  Several bugs came from the OS contract being weaker than believed.
- Lessons: whole-cluster restart upgrades (fast recovery makes rolling unnecessary);
  restarts "heal" latent state bugs; 5 s transaction limit forces apps to split work.
- **For the spec:**
  - "All failures go through one well-tested recovery path" is the right model for
    pgfs's Bug 5 (no reconnect): on any connection error, drop all DB state, reconnect,
    revalidate, retry idempotently — one path, exercised by tests that kill Postgres.
  - Deterministic simulation is out of reach for pgfs end to end (kernel + Postgres),
    but its spirit fits the err.py contexts: inject faults at the Db boundary (a
    `FaultyDb` wrapper returning errors/delays at chosen calls) and assert invariants.
    The spec has no fault-injection requirement at all.
  - The 5 s MVCC window lesson: long transactions (path-key directory rename over a
    large tree) are a design smell; split work into bounded transactions.

## r16 InfiniFS (FAST22; Lv, Lu et al., Tsinghua/Alibaba)
- Splits directory metadata into ACCESS (name, id, permissions — stored with the
  PARENT) and CONTENT (entry list, timestamps — stored with the CHILDREN); groups
  hashed by directory id. Create/delete/readdir/stat become single-server; only mkdir,
  rmdir and file rename are two-server (2PC).
- Predictable directory ids = hash(birth parent id, name, version) → clients compute
  every component's id and resolve paths in PARALLEL; renamed dirs keep their id
  (back-pointer + rename-list keep ids unique; mispredictions return the true id).
- Optimistic client cache of access metadata, invalidated LAZILY: directory renames /
  chmods go through a central rename coordinator that detects orphaned loops and
  broadcasts to metadata SERVERS' invalidation lists; servers validate the client's
  cache version on each request. Beats leases (1/5/10 s) because near-root lease
  renewals become the hotspot.
- Alibaba trace: 95.8% file ops; readdir 93% of dir ops; dir rename + dir chmod only
  0.0083% of all ops.
- **For the spec:**
  - The 0.0083% figure is the strongest argument that directory rename may be slow-ish
    but must be CORRECT — and that pgfs paying O(subtree) on every dir rename is not
    the real problem; correctness (LIKE bug, open-file data loss) is.
  - Orphaned-loop detection: pgfs checks "dir into its own descendant" with a path
    prefix test (RejectsInvalidRenames); with ino keys that check needs an ancestor
    walk under a lock, or two concurrent renames can create a loop exactly as in
    InfiniFS Fig. 8. Serialize directory renames (one advisory lock in Postgres:
    `pg_advisory_xact_lock(<const>)`) — cheap given their rarity. Spec it.
  - Versioned server-side invalidation is a model for multi-daemon pgfs: a monotonic
    `rename_seq` in the DB that each daemon compares before trusting its path cache.

## r17 RFUSE (FAST24; Cho, Choi, Kwon, Kim)
- Replaces /dev/fuse read/write with per-core NUMA-local shared-memory ring channels
  (pending/forget/interrupt rings + header and argument buffers, bitmaps), hybrid
  polling (50 µs spin then sleep), ≤2 workers per ring pinned to the core, round-robin
  spill of bursty async requests. Drop-in for libfuse (relink).
- Empty CREATE latency −53%; 72% of a create's time is VFS path lookup (each component a
  LOOKUP round trip); waking the app thread costs ~39 µs. Random read 2.27× FUSE.
  PostgreSQL on FUSE: fsync dominates (writeback of dirty pages then FSYNC).
- Mainline Linux took a related route: FUSE-over-io_uring (6.14) with per-core queues.
- **For the spec:**
  - IoUringBufferRingReq says "Linux 6.9+ fuse-io_uring"; the upstream feature is
    FUSE over io_uring (`FUSE_IO_URING`, merged in 6.14) and fuser 0.14 does not
    support it. Correct the version and mark it dependent on the FUSE crate.
  - Per-core queues only help if the daemon is concurrent; again
    MultiThreadedConnectionPoolReq/concurrent dispatch first.
  - Their path-lookup share (72%) argues for longer ENTRY TTLs on directories (pgfs
    gives every reply the same `--attr-ttl`).

## r18 Fire-Flyer AI-HPC / 3FS (SC24; DeepSeek) — read the 3FS, checkpoint and
## reliability sections; the GPU/allreduce/network sections are out of scope
- 3FS metadata: stateless meta services over a transactional distributed KV store
  (FoundationDB in the open-source release): an INODE table (inode id → size, layout)
  and a DIRECTORY-ENTRY table keyed (parent inode, name) → child inode.
- Data: CRAQ chains of storage targets (write-all, read-any); each file gets a chain
  table offset and stripe width k; each SSD hosts targets of many chains for balance.
  Request-to-send flow control at the client to avoid incast (trades latency for
  sustainable throughput). Checkpoints every 5 min, batched reads load in seconds.
- Weekly hardware "validator" runs; memory ECC, IB flash-cuts and GPU Xid errors are
  routine.
- **For the spec:**
  - A 2024 production system again picks: inode table + (parent, name) dentry table
    in a transactional store, stateless metadata front ends. Same as v0.2 MDB and
    InoBasedRekeyingReq. Splitting `entries` into `inodes` and `dentries` would also
    give hard links for free (FlatFilesystem: link unsupported).
  - CRAQ for data matches the "replica reads" goal; with Postgres, reads from a
    synchronous standby need the remote_apply/LSN wait, not a 500 ms guess.

## r19 Amazon Aurora (SIGMOD17; Verbitski et al.)
- "The log is the database": the MySQL engine ships only redo records to a storage
  service; storage nodes materialise pages in the background. 10 GB segments, 6 copies
  across 3 AZs, write quorum 4/6, read quorum 3/6 (survive AZ+1 for reads, AZ for
  writes); small segments make MTTR ~10 s.
- No 2PC: LSN-tracked completeness. VCL (volume complete LSN), CPLs (mini-transaction
  boundaries), VDL (highest CPL ≤ VCL); commit acks asynchronously once VDL ≥ commit
  LSN; writers never block on commit. LSN allocation limited to VDL+10M (backpressure).
  Recovery truncates above VDL with epoch-versioned truncation ranges.
- Reads normally hit ONE segment known (from SCL tracking) to be complete at the read
  point — quorum reads only during recovery. Replicas share the storage volume and
  apply the redo stream to cached pages (lag ~20 ms).
- Lessons: SaaS tenants with 150k tables; online DDL by per-page schema versions;
  zero-downtime patching.
- **For the spec:**
  - Aurora shows replica lag can be ~ms when replicas consume the same log; pgfs's
    replica check should be an LSN wait ("serve from standby once replay_lsn ≥ my
    last commit LSN") — the exact Aurora read-point rule — instead of a 500 ms cache.
  - Async commit with explicit durability points is what `synchronous_commit = off`
    approximates; pgfs could expose fsync as "wait until commit LSN flushed"
    (`pg_current_wal_flush_lsn()` ≥ my LSN) to give real fsync semantics cheaply.
  - Quorum math for v0.2 data sets: 1 primary + 2 sync standbys with
    `synchronous_standby_names = 'ANY 1 (a,b)'` is 2/3, which Aurora argues is not
    enough under correlated failure; document the failure domains instead.

## r20 λFS (ASPLOS23; Carver et al.)
- HopsFS with the stateless NameNodes replaced by FaaS functions that KEEP a metadata
  cache (trie of all path components) across invocations; namespace partitioned over n
  function deployments by hashing the parent path; HTTP invocations drive autoscaling,
  direct TCP RPCs give 1–2 ms latency; a random ≤1% of TCP calls are sent as HTTP so the
  platform still sees load.
- Coherence: a writer takes exclusive row locks in NDB, sends INV through a coordinator
  (ZooKeeper/NDB) to every live instance of each deployment caching the path, waits for
  all ACKs, then persists. NameNodes cache recent results so a client's retried request
  returns the cached answer instead of re-executing (idempotent retry).
- 10× lower latency than HopsFS (1 ms vs 10.6 ms), handles 7× bursts.
- **For the spec:**
  - The HopsFS lesson in reverse: purely stateless front ends cap at the DB; a coherent
    front-end cache is where latency comes from. For pgfs (one daemon per client) the
    daemon IS the front end; caching attributes in the daemon beyond the kernel TTL is
    only safe with invalidation (LISTEN/NOTIFY), never with time alone.
  - "Cache the reply of a completed mutation keyed by request id" is the idempotent
    retry pattern pgfs needs after reconnect (create/mkdir/rename replay).

## r21 Leases (SOSP89; Gray, Cheriton)
- A lease = time-bounded right to cache; the server must get every leaseholder's
  approval (or wait out the term) before a write. Host/network failures cost delay,
  never correctness (needs bounded clock drift ε). After a server crash, it waits the
  maximum term before allowing writes.
- Model + V traces: most benefit by ~10 s terms (10% of zero-term consistency
  traffic); term 0 beats a very short term for heavily write-shared files; installed
  (read-mostly) files covered by a few broadcast-extended leases; leases must cover
  the name→file binding and permissions too, not just contents.
- **For the spec:**
  - `--attr-ttl` is a lease WITHOUT the write-side half (no one waits for it to
    expire before writing). That is fine for one mount, but for multiple pgfs mounts
    the spec should say cross-mount staleness is bounded by TTL and is NOT consistency.
  - Per-file adaptive terms (0 for write-shared files, long for read-mostly trees) map
    onto per-reply FUSE TTLs; pgfs can choose TTL per entry kind/age at no cost.

## r22 LFS — Log-Structured File System (SOSP91; Rosenblum, Ousterhout)
- All modifications (data, inodes, directories) buffered and written sequentially as
  segments; inode map locates inodes; segment summaries + usage table drive a
  cost-benefit cleaner that separates hot and cold data. Crash recovery from
  checkpoints + roll-forward of the log tail. 70% of disk bandwidth for writes vs 5–10%.
- Explicit premise: accept losing "a few seconds" of work on a crash in exchange for
  asynchronous metadata.
- **For the spec:**
  - pgfs inherits LFS-like behaviour from Postgres (WAL = log, heap pages lazily
    written, VACUUM = cleaner). Block upserts create dead tuples: 64 KiB TOASTed
    values rewritten on every partial-block write → table/TOAST bloat. The spec has
    no requirement about VACUUM/autovacuum tuning or bloat monitoring; a 1-byte write
    rewrites a 64 KiB value (write amplification ~65 000×).
  - LFS's stated durability trade ("lose seconds") is the same one SynchronousCommitTuningReq
    makes; LFS still gave fsync real meaning. pgfs should too.

## r23 Paxos Made Simple (2001; Lamport)
- Derives single-decree Paxos from safety requirements: proposals numbered; phase 1
  prepare/promise (learn highest accepted value), phase 2 accept; majorities
  intersect; acceptors persist promises before replying; liveness needs a
  distinguished proposer (leader) chosen with timeouts/randomness (FLP).
  Multi-Paxos = state machine of consecutive instances with a stable leader.
- **For the spec:** none directly; pgfs should never implement consensus. shard.py's
  quorum-write + compensating rollback design (RequiresWriteQuorum) is NOT a
  consensus protocol (no ordering, rollback can itself fail) — the v0.2 decision to
  rely on Postgres replication + an external failover manager is the right one.

## r24 ZooKeeper (ATC10; Hunt, Konar, Junqueira, Reed)
- Wait-free znode tree (no locks in the API): regular / ephemeral / sequential znodes,
  per-znode version for conditional set/delete, one-shot WATCHES (notify that something
  changed, not what), sessions with timeouts that move between servers. Guarantees:
  linearizable writes (Zab atomic broadcast) + FIFO client order; reads served LOCALLY
  by any server (may be stale) with `sync` to catch up. Recipes for locks, barriers,
  leader election built client-side.
- **For the spec:**
  - Watches = Postgres LISTEN/NOTIFY: one-shot "something changed under X", fetch the
    current state yourself. v0.2 §11 coherence design should follow this shape
    (notification carries ino/parent only; receiver invalidates and re-reads).
  - "Reads served locally may be stale; `sync` to catch up" is exactly the replica-read
    contract pgfs should state instead of "fresh within 500 ms" — and offer a
    read-your-writes path (wait for own commit LSN) as ZooKeeper's `sync`.
  - Conditional updates on a version number = v0.2 `data_ver` compare-and-swap.

## r25 HDFS (MSST10; Shvachko et al., Yahoo)
- Single NameNode holding the whole namespace (inodes + block lists) in RAM, journal
  (WAL) flushed and synced before ack with GROUP COMMIT across client threads,
  checkpoints by a CheckpointNode; block→DataNode locations NOT persisted, rebuilt
  from block reports. 128 MB blocks, 3 replicas, pipelined writes, generation stamps
  per block replica, checksums stored next to data. Namespace ID handshake stops
  DataNodes from a different instance joining; software-version check on join.
- Relaxed POSIX: single writer with leases, append-only, no random writes.
- **For the spec:**
  - Group commit is the honest alternative to `synchronous_commit = off`: Postgres has
    it (`commit_delay`/`commit_siblings`) and gets it naturally with concurrent
    connections. Once pgfs dispatches concurrently, synchronous commit costs far less.
  - Namespace-ID handshake ↔ v0.2 `set_identity`: a data set must prove it belongs to
    this file system (system identifier + pgfs cluster uuid) before use. Also useful
    today: refuse to mount a database created by an incompatible pgfs schema version —
    the spec has no schema-version table or migration requirement.

## r26 Haystack (OSDI10; Beaver et al., Facebook) — read intro/design; skimmed eval
- NFS/NAS photo store needed ≥3 disk I/Os per photo (directory, inode, data), and
  >10 with big directories; caching file handles in memcache only moved the problem.
  Haystack packs photos as needles in large append-only volume files; the in-memory
  index (key → offset, size) makes reads one disk I/O; deletes are flags; compaction
  reclaims space; per-photo metadata cut to the minimum.
- **For the spec:** for many small files, per-file metadata round trips dominate — the
  same lesson as TableFS/IndexFS. pgfs's create path is 2–3 SQL statements per file
  (existence check, insert, block upsert at close). Inline small-file data in the
  entries row and a single INSERT … ON CONFLICT for create would halve round trips.

## r27 Windows Azure Storage (SOSP11; Calder et al.) — read intro/architecture
- Stamps (10–20 racks) with three layers: Front-End, Partition layer (range-partitioned
  object tables; a Partition Manager assigns RangePartitions to partition servers,
  splits/merges them under load) and Stream layer (append-only "extents", sealed when
  full, replicated synchronously within the stamp). Geo-replication between stamps is
  asynchronous. Strong consistency + conditional ops (optimistic concurrency via ETags)
  on objects in the same partition.
- **For the spec:** Azure's partition map (a table assigning key ranges to servers,
  moved by a manager) is the same indirection v0.2 uses (`entries.set_id`,
  `data_sets`). Its "seal extent then replicate" is the immutable-block idea again.
  Conditional writes with ETags = pgfs could expose `data_ver` as an xattr/ETag for
  tools (rsync-like) that want cheap change detection.

## r28 Viewstamped Replication Revisited (MIT TR 2012; Liskov, Cowling) — read §1–4
- Primary-backup state machine replication with 2f+1 replicas, views with a
  deterministic primary per view number, view-change protocol, recovery of a crashed
  replica without disk (state comes from the group), reconfiguration via epochs.
- Client proxy keeps (client-id, request-number); replicas keep a CLIENT TABLE with the
  latest request number and reply per client, so retransmitted requests are answered
  from the table, not re-executed (at-most-once semantics).
- **For the spec:** the client table is the canonical answer for pgfs mutation retry
  after a lost connection: a small `client_ops(client_id, op_seq, result)` row written
  in the same transaction as each mutation lets a reconnected daemon ask "did op N
  commit?" instead of guessing. Add to the reconnect requirement (Bug 5).

## r29 Erasure Coding in Windows Azure Storage — LRC (ATC12; Huang et al.) — read §1–3
- Extents replicated 3× while open; once SEALED (~1 GB) they are erasure-coded lazily in
  the background and the replicas deleted. Local Reconstruction Codes (k, l, r) add a
  local parity per group so a single lost fragment needs k/l reads instead of k;
  reconstruction latency is dominated by stragglers, so degraded reads race a
  reconstruction against the slow node.
- **For the spec:** only relevant if pgfs ever does its own redundancy (v0.2 says no:
  Postgres replication). The "replicate hot/mutable, encode sealed/cold" lifecycle is
  a reasonable future tier for blocks of files untouched for a long time.

## r30 Spanner (OSDI12; Corbett et al.) — read §1–2, skimmed §3–5
- Data sharded into tablets, each a Paxos group with leader leases; 2PC across groups
  with Paxos-replicated participants; TrueTime exposes clock uncertainty and commit
  waits it out, giving external consistency, lock-free snapshot reads at a timestamp,
  atomic schema changes. Directories (key prefixes) are the unit of placement/movement.
- **For the spec:** Spanner is what shard.py would need to be correct (replicated
  groups + 2PC + global timestamps). Postgres cannot give cross-database atomicity
  without 2PC (`PREPARE TRANSACTION`), which pgfs should avoid; v0.2 rightly keeps all
  metadata in one database and makes data sets slaves of metadata ordering.

## r31 f4 (OSDI14; Muralidhar et al., Facebook) — read §1–4
- Warm BLOBs (older, low request and delete rate) moved from Haystack (3.6× effective
  replication) to f4: RS(10,4) within a cell across racks + XOR across datacenters
  (2.1–2.8×). Age is a good proxy for temperature; deletes are rare so space is
  reclaimed lazily; a CDN + caching stack shields storage from hot reads.
- **For the spec:** temperature-by-age suggests a per-file "last written" column is
  worth keeping (pgfs ignores atime/mtime for directories). Lazy space reclamation
  matches v0.2 lazy GC.

## r32 PolarFS (VLDB18; Cao et al., Alibaba) — read §1–4, §6
- Shared-storage FS for POLARDB: one primary + read-only DB nodes mount the same
  volume via a user-space library (libpfs) over RDMA/SPDK; chunks replicated by
  ParallelRaft (Raft allowing out-of-order ack/commit/apply when I/O ranges don't
  overlap, since databases tolerate out-of-order completion).
- File-system metadata lives IN the volume; changes are serialised with a disk-Paxos
  lock per volume and journaled so RO nodes replay them; a node that lost the lease
  (network partition) is fenced so only the real primary writes.
- **For the spec:** two takeaways for pgfs's multi-mount future: (1) metadata changes
  must be published to other mounts as an ordered journal (≈ NOTIFY with a sequence
  number, or logical decoding), (2) a stale writer must be fenced (v0.2 epochs).
  ParallelRaft's "out-of-order is fine if ranges don't overlap" also justifies
  concurrent block writes to different block_no of one file under concurrent dispatch.

## r33 BlueStore (SOSP19; Aghayev et al.) — read §1–3, skimmed §4–6
- Ceph abandoned building its OSD backend on local file systems after a decade:
  efficient transactions are hard on POSIX (WAL-on-FS doubles writes), directory
  enumeration of millions of entries was slow and unordered, and new hardware (SMR,
  ZNS) can't wait for file systems to adopt it. BlueStore: data on the raw device,
  ALL metadata (including allocation bitmaps) in RocksDB (on BlueFS), full data
  checksums, inline compression, copy-on-write clones with extent ref counts,
  deferred small writes through the KV WAL.
- **For the spec:**
  - pgfs is the inverse experiment (file system ON a database). BlueStore's warning
    applies symmetrically: storing DATA through a WAL means every byte is written at
    least twice (WAL + heap), more with `full_page_writes` after each checkpoint and
    TOAST chunking. The spec's block storage requirement never states the write
    amplification; it should be measured (pg_stat_wal bytes per byte written) and a
    target set. Large files may belong outside the WAL (v0.2 data sets help).
  - Their small-write deferral (small overwrites go through the KV WAL, large ones go
    direct + metadata) is a model for a two-path write: small writes as row updates,
    big sequential writes as whole new immutable blocks.
  - Clone with ref-counted extents = ReflinkBlockSharingReq; BlueStore found the
    ref-counting overhead manageable only with careful interface design (shared
    blob records). Spec it with a separate `block_refs` table, not a column on blocks.

## r34 TiDB (VLDB20; Huang et al., PingCAP) — read §1–3
- Multi-Raft row store (TiKV; regions of key ranges, each a Raft group) + Raft LEARNERS
  that asynchronously receive the log and build a columnar replica (TiFlash) for
  analytics; reads on learners stay consistent by waiting for the leader's commit
  index (read index) before serving.
- **For the spec:** learner + read-index is the principled version of pgfs's replica
  reads: ask the primary for its current LSN (cheap), then wait on the standby until
  replay ≥ that LSN — reads are linearizable, not "fresh within 500 ms". A learner-
  style analytics replica is also how pgfs could serve ContentSearchOffloadReq
  (trigram indexes) without loading the primary.

## r35 XFUSE (ATC21; Huai, Hsu et al., Alibaba) — read §1–3
- Production-oriented FUSE: per-core channels and busy-event waiting (4 µs requests,
  >8 GB/s), plus RAS features: ONLINE UPGRADE and CRASH RESTART of the user-space file
  system without disrupting applications — the kernel side tracks in-flight requests
  and replays them to the new daemon instance.
- **For the spec:** pgfs's ReliableMount makes the opposite choice: daemon death ⇒
  AutoUnmount ⇒ every process with a cwd or open file inside gets errors. That is
  right for a prototype; for production, Linux FUSE can keep a mount alive across a
  daemon restart by handing /dev/fuse to a successor (fd passing / systemd FD store),
  and pgfs's state (inode map, handles) would have to be rebuildable from the DB.
  Persistent inode numbers (InoBasedRekeyingReq) are the prerequisite. Record this as
  a future requirement and as the reason ino must not stay in-memory.

## r36 SingularFS (ATC23; Guo, Lu et al., Tsinghua) — read §1–3
- One metadata server (RDMA + persistent memory) handles billions of files: 8.4M
  creates/s, 18.8M stats/s — beats InfiniFS on 32 servers. Techniques: log-free
  metadata ops (rely on single-object atomic KV updates + parent/child ordering to
  detect partial states after a crash instead of journaling), hierarchical concurrency
  control (per-inode rwlock + lock-free timestamp updates so creates in a SHARED
  directory don't serialise on the parent), and separating a directory's timestamps
  from its inode, colocated with the children per NUMA node.
- **For the spec:**
  - Strong support for v0.2's single metadata database: scale up before scaling out.
  - Shared-directory contention: if pgfs starts tracking directory mtime/nlink (the
    HierarchicalDirectories explanation says it doesn't, to avoid a parent update per
    create), every create in a hot directory would row-lock the parent. SingularFS's
    answer — separate the directory timestamps, update lazily/monotonically
    (`GREATEST`) — is the way to add correct directory mtimes without contention.

## r37 Petal (ASPLOS96; Lee, Thekkath) — read §1–2
- Distributed virtual disks: servers hold all integrity state; clients hold only
  routing HINTS (wrong server → error → client refreshes hint). Global state (membership,
  vdisk directory) replicated with Paxos; chained declustering for redundancy; copy-on-
  write snapshots via epoch numbers in the translation maps; online reconfiguration.
  Frangipani (12) was built on it.
- **For the spec:** "clients keep only hints" is the rule pgfs's in-memory ino↔path map
  must follow once several daemons share a DB (validate on use, refresh on miss).
  Epoch-tagged copy-on-write snapshots are the cheapest route to a `pgfs snapshot`
  feature if blocks become immutable with a version/epoch column.

## r38 Size Matters: small files in HopsFS (Middleware18; Niazi et al.) — read §1–3
- 20% of files at Yahoo/Spotify are <4 KB (68% at Logical Clocks); ~42% of Spotify ops
  touch files <16 KB. HopsFS++ "inode stuffing": files ≤1 KB stored in-memory in NDB
  tables, ≤64 KB on NVMe-backed NDB disk tables, larger in datanodes; transparent to
  clients; 61× write throughput for 4 KB files, 3–7× lower latency.
- **For the spec:** the same tiering inside pgfs: store files ≤ N bytes inline in the
  entries/inodes row (no blocks row, one round trip for stat+read), blocks table only
  for larger files. Specify N and measure; git objects, configs, source files fall in
  this class.

## r39 Citus (SIGMOD21; Cubukcu et al., Microsoft) — read §1–2
- Distributed PostgreSQL as an EXTENSION (planner/executor hooks, custom scan nodes,
  2PC with recovery, reference tables, co-located distributed tables by a distribution
  column, connection scaling); four workload patterns (multi-tenant, real-time
  analytics, high-performance CRUD, data warehouse).
- **For the spec:** v0.2 open question 5 (metadata scale-out): Citus could distribute
  `entries`/`blocks` co-located by ino (hash) with `data_sets` handled by Citus shards —
  but directory listings by parent_ino and renames across shards become distributed
  transactions (Citus 2PC). Keep it as the documented escape hatch, not the plan.
  Being an extension (not a fork) is also the right model for LocalPgrxEngineReq.

## r40 Caching in the Sprite Network File System (TOCS88; Nelson, Welch, Ousterhout) — read §1–5
- Large write-back client caches (30 s delayed write), consistency by the server: on
  open, the server tracks readers/writers per file; sequential write-sharing handled by
  version numbers + flush of the last writer's dirty blocks; CONCURRENT write-sharing
  disables client caching for that file (all I/O goes to the server). File cache and VM
  negotiate memory. Exact single-machine semantics.
- **For the spec:** Sprite is the model for pgfs's multi-mount coherence: (1) close-to-
  open with version check on open (v0.2 `data_ver`, FOPEN_KEEP_CACHE only when the
  version is unchanged), (2) when two mounts have the same file open for write,
  switch that file to direct I/O (FOPEN_DIRECT_IO) and no buffering — a precise,
  implementable rule the spec currently lacks.

## r41 Lockify (FAST26; Park et al.) — read §1–4
- Shared-disk FS (GFS2, OCFS2) creates collapse with more mounted clients even when
  only ONE client is active: the kernel DLM must ask a hash-chosen remote "directory
  node" who owns a new lock (47% of create latency is DLM round trips). Lockify lets the
  creator declare itself owner of a lock for a NEW object (self-owner notification,
  confirmed asynchronously with a resend wait-list) → ~6.4×.
- **For the spec:** the analogue in pgfs: a create checks existence on the primary
  (`SELECT … FOR UPDATE`/existence query) then INSERTs — two round trips. Use the
  database's uniqueness as the lock: one `INSERT … ON CONFLICT DO NOTHING RETURNING`
  decides EEXIST and creates in a single statement ("the creator owns the new name").
  CreatesFiles/CreatesDirectories should require single-statement create.

## r42 CetoFS (FAST26; Jia, Jiang, Xiong) — read §1–2
- File system for disaggregated NVMe-oF SSDs: user-space data plane, permission checks
  offloaded to the storage server (reverse permission table), concurrency control
  offloaded (hosts submit conflicting writes in parallel; the server serialises request
  groups), and REDO logging offloaded so data crosses the network once. Kernel stack is
  66% of remote write latency; inode rwlocks + network RTT serialise concurrent writers.
- **For the spec:** pgfs already "offloads" concurrency control and atomicity to the
  server (Postgres). The paper's point that a host-side per-inode lock held across a
  network round trip kills scalability argues against a daemon-side per-file mutex when
  dispatch goes multi-threaded: let row-level locks in Postgres serialise writers to the
  same block, and allow concurrent writes to different blocks.

## r43 Sharpen the Spec, Cut the Code — SysSpec (FAST26; Liu, Zou, … Haibo Chen) — read in full
- Ext4 history: 82.4% of 3,157 commits are bug fixes/maintenance; features are 5.1% of
  commits but trigger long tails of fixes (fast-commit: 9 feature commits → ~80 fixes;
  65% semantic bugs such as misordered updates and missed cleanup on early returns).
- SysSpec: LLM-generated FUSE file system from a multi-part spec —
  (1) FUNCTIONALITY: Hoare-style pre/post-conditions + system-wide invariants, written in
      "mathematically disciplined natural language" (e.g. "size = max(old_size,
      offset + len)", not "updates the size if necessary"), plus an optional system
      algorithm/intent for performance-critical choices;
  (2) MODULARITY: rely/guarantee interface contracts per module (≤500 LoC modules);
  (3) CONCURRENCY: a SEPARATE locking spec (pre/post lock states), generated in a
      second pass over a validated sequential version.
  Evolution by DAG-structured spec patches (leaf → intermediate → root with unchanged
  guarantees). Generator + independent evaluator agent with retry-on-feedback; xfstests
  as the regression oracle. 100% module accuracy with strong models vs 81.8% for
  code-in-context prompting; 3–5.4× developer productivity.
- **For the spec (the most important paper for HOW pgfs's spec is written):**
  - pgfs's Requirements are prose docstrings with no postconditions. Bug 1 (buffered
    writes zero the untouched bytes of a partially written block) is exactly the class
    SysSpec's discipline prevents: WritesFiles should state the postcondition
    "for every byte b outside [offset, offset+len): content'(b) = content(b);
    size' = max(size, offset+len)". Bug 2 (LIKE wildcards) is a violated invariant
    "rename affects exactly the subtree of the source path". Bug 3 (rename loses
    buffered writes) violates "after rename, reads of the new name return all bytes
    written through any open handle". Bug 4 (root identity collision) violates
    "ino values are unique". Every one of the known bugs is a missing postcondition
    or invariant.
  - Recommend a uniform template for every Req: Pre, Post, Invariants touched,
    Errors, Concurrency (locks/isolation), Durability point. libspec Req docstrings
    can carry this as structured sections, and tests can be derived per clause.
  - err.py/observe.py are "contexts" (style rules), not behaviour; the behavioural
    contract is what is missing. A separate concurrency spec matters now that
    MultiThreadedConnectionPoolReq is planned.
  - libspec diff ≈ SysSpec spec patches; ordering by dependency (`implementation_order`)
    ≈ their DAG. pgfs already has the right workflow, but needs sharper clauses.

## r44 DisCoGC — discard-based GC (FAST26; Bian et al., ByteDance) — read §1–2
- Append-only ByteStore under a block service: compaction-only GC traded write
  amplification against space amplification (millions $/month). Many workloads write
  long sequential ranges and overwrite them within seconds, leaving long contiguous
  stale ranges; DisCoGC DISCARDS those ranges in place (aligned to allocation units,
  batched, rate-limited by SSD trim IOPS) and compacts only periodically → −25% write
  amp at −10% space amp, ~20% TCO.
- **For the spec:** pgfs's analogue is VACUUM vs block rewrite: a file overwritten
  sequentially leaves dead 64 KiB tuples (TOAST chunks) until VACUUM. Truncate/unlink
  of large files should DELETE block rows in one statement (it does), and the spec
  should state that space is reclaimed by autovacuum (not immediately) and expose dead
  tuple counts in metrics. `statfs` reporting zero today hides this entirely.

## r45 CoFS (FAST26) — read §1–2
- Container-image FS on extended FUSE: a minimal perfect hash over the read-only image
  tree lets lookups be served from kernel space in <1 I/O, plus a full-path MPHF for
  parallel path resolution, plus sparse local files for cached data served by the
  kernel. Each path component LOOKUP through FUSE is the main cost of cold starts.
- **For the spec:** immutability enables kernel-side caching; for pgfs the equivalent
  is long entry TTLs for read-mostly trees. Nothing to adopt directly.

## r46 Ananke (FAST25; Liu, Dai, Arpaci-Dusseau ×2) — read §1–3
- File system as a user-space server (uFS) that recovers TRANSPARENTLY from its own
  process crash: an in-memory "p-log" of in-flight syscalls plus the on-disk journal
  closes the STATE GAP between what applications were told and what is durable; OS-
  coordinated restart transfers client connections; checksums on key in-memory
  structures; 30,000 fault-injection runs, ≤400 ms recovery, <2% overhead.
- **For the spec:** pgfs has a p-crash state gap by design: write() returns success
  while data sits in the per-handle buffer; if the daemon dies before flush, data is
  lost though no error was reported to the writer (FlatFilesystem "Durability" says so).
  At minimum the spec must guarantee that close()/fsync() never report success unless
  the data committed (flush errors → EIO, with tests), and should document that a
  daemon crash loses unflushed writes exactly like a kernel crash loses page cache.
  Longer term, persistent ino + FUSE fd passing (see XFUSE) are what make transparent
  restart possible.

## r47 Combining Buffered and Direct I/O in DFS (FAST24; Qian et al., DDN/Lustre) — read §1–2
- Buffered I/O hits a page-cache "wall" (~20% time copying, >40% page management) and
  causes distributed range-lock contention on shared files; direct I/O wins for large
  I/O and interleaved multi-client writes but loses for small I/O. Lustre now switches
  per request by size, memory pressure and lock contention (up to 3×).
- **For the spec:** FUSE offers the same switch per open file (FOPEN_DIRECT_IO).
  pgfs could open large sequential-write files direct (avoid double caching in kernel
  page cache + daemon buffer + shared_buffers) and keep small files buffered; and use
  direct I/O whenever another mount holds the file open for write (see Sprite r40).

## r48 Metis (FAST24; Liu, Adkar, Holzmann, Kuenning, … Zadok) — read §1–2
- Implementation-level model checking by DIFFERENTIAL execution: issue the same
  weighted-random syscalls (with chosen argument distributions) to the FS under test
  and a reference FS (RefFS, in-memory, with checkpoint/restore), compare ABSTRACT
  states (file data, tree, essential metadata), DFS over states with backtracking,
  replay any discrepancy. Found 11 RefFS bugs, 12 bugs in 5 file systems (one 16 years
  old, fixed upstream). An Ext4 bug needed a write to a 530-byte file — state matters.
- **For the spec:** pgfs's integration tests are hand-written scripts. A Metis-style
  differential test (pgfs vs tmpfs, random create/write/truncate/rename/unlink with
  odd offsets and sizes, compare full tree+content after each op) would have found
  Bugs 1–3 immediately (partial-block writes, rename with `_`/`%`, rename with open
  buffered handle). Add a requirement: "pgfs is differentially tested against a
  reference POSIX FS over random operation sequences in CI". This is the single
  highest-value testing addition.

## r49 Ionia (FAST24; Xu, Zhu, Pandey, Conway, Johnson, Ganesan, Alagappan) — read §1–2
- Replication for SSD write-optimised KV stores: writes are nil-externalising (no result
  returned) so durability on a quorum in 1 RTT suffices, ordering/execution deferred;
  reads go to ANY follower while a parallel "meta query" to the leader (served from a
  compact in-memory history of recently modified keys) lets the CLIENT decide whether
  the follower's answer is fresh → scalable 1-RTT linearizable reads.
- **For the spec:** a concrete design for pgfs replica reads: issue the attribute read
  to the standby and, in parallel, ask the primary for `data_ver`/`mtime_seq` of that
  ino (or the primary's current LSN); accept the standby answer if it matches. Primary
  load is a single-index lookup, standby does the heavy part (readdir, block reads).
  Strictly better than ReplicaFreshnessCacheReq's 500 ms window.

## r50 Fisc (FAST23; Li et al., Alibaba) — read §1–2
- Cloud-native FS: lightweight per-container clients (protocol/network functions moved
  to the DPU and to backend storage nodes), a storage-AWARE distributed gateway that
  routes per file (not per connection), locality-aware reads and file-granular failover
  (second-level → ~100 ms recovery).
- **For the spec:** little direct impact; reinforces "keep the client thin, push
  semantics to the server" (pgfs pushes to Postgres) and per-file routing (v0.2
  per-file placement).

## r51 More Than Capacity: Pangu 2.0 (FAST23; Li et al., Alibaba) — read §1–2
- Evolution from volume-oriented (ext4 + kernel TCP) to performance-oriented (100 µs I/O):
  append-only persistence layer, self-contained chunk layout, user-space storage OS with
  run-to-completion threads; metadata split into a namespace service and a stream-meta
  service, each partitioned by directory then hashed; later traffic-amplification
  reduction (EC instead of 3 replicas), remote direct cache access, CPU offload.
- **For the spec:** the namespace/stream-meta split is the same as v0.2's MDB vs data
  sets. Their "traffic amplification" metric (bytes on the wire per byte written) is a
  metric pgfs should export (WAL bytes + replication bytes per byte written).

## r52 Unsafe at Any Copy (FAST23; Basu, Sampson, Qian, Jaeger) — read §1–3
- Case-insensitive (and encoding-normalising) directories cause NAME COLLISIONS when
  content is copied from case-sensitive trees: silent data loss, symlink traversal,
  unexpected hardlinks, directory merging; CVE-2021-21300 (git). Utilities (cp, rsync,
  tar, dpkg) mostly handle collisions unsafely.
- **For the spec:** pgfs names live in a Postgres `text` column under a UNIQUE
  constraint and are ORDERED by the database collation. Requirements to add:
  (1) names are compared BYTE-EXACTLY — specify `COLLATE "C"` on `name` (a deterministic
  ICU or libc collation still byte-tiebreaks for uniqueness, but ordering and any
  future nondeterministic collation would change semantics); (2) no Unicode
  normalisation; (3) Linux names are arbitrary bytes except '/' and NUL — pgfs rejects
  non-UTF-8 names with ENOENT (HierarchicalDirectories Reference). That is a POSIX
  deviation: creating such a name should fail with EILSEQ (or names should be bytea).
  State it as a decision, not an accident.

## r53 Bento (FAST21; Miller et al., UW) — read §1–2
- Rust file systems loaded into the Linux kernel behind thin safe interfaces; the same
  code compiles to FUSE for user-level debugging; live upgrade with ~15 ms pause.
  The same FS is 7× slower on `git clone` through FUSE than in-kernel (90× on some
  Filebench loads); passes CrashMonkey depth-2.
- **For the spec:** quantifies the FUSE ceiling for pgfs's flagship workload (git): even
  with zero database cost, FUSE alone costs several × on metadata-heavy work. The spec's
  performance expectations should be stated relative to a FUSE baseline (e.g. a
  passthrough FUSE FS), not to ext4.

## r54 pFSCK (FAST21; Domingo, Kannan) — read §1–2
- Parallel e2fsck: data parallelism within passes, pipeline parallelism across passes,
  "out-of-order execution, in-order commit" for shared structures, resource-aware
  scheduling; 2.6× over e2fsck, 1.8× over xfs_repair. C/R tools remain the first remedy
  after crashes, upgrades and corruption.
- **For the spec:** pgfs has no checker. Its invariants are SQL-checkable: every
  entries.parent exists and is a dir; no blocks without an entry; no block_no beyond
  ceil(size/BLOCK_SIZE); unique ino; no cycles in parent_ino. Add a `pgfs check`
  requirement (read-only SQL queries; `--repair` later) and, where possible, enforce
  invariants with FOREIGN KEY / CHECK constraints so the database prevents corruption
  instead of a checker finding it.

## r55 How to Copy Files — nimble clones in BetrFS (FAST20; Zhan et al.) — read §1–2
- BetrFS keys METADATA by full path and DATA by (full path, block#) in a Bε-tree — the
  same layout as pgfs today. Clones via a Bε-DAG with GOTO messages and translation
  prefixes; "copy-on-abundant-write" breaks sharing only after enough changes accrue,
  avoiding the locality decay of block-granular CoW (XFS/ZFS reflinks degrade 3–4×
  after 16 copy+edit rounds).
- **For the spec:** (1) BetrFS is the one serious full-path-keyed FS; it needed range-
  rename machinery (earlier BetrFS papers) to make directory rename cheap — pgfs would
  need the equivalent or ino keys; (2) for ReflinkBlockSharingReq, CoW at 64 KiB block
  granularity in a B-tree table is locality-neutral (rows are fetched by index anyway),
  so the BetrFS locality problem mostly doesn't apply; the real cost is ref-count
  maintenance and VACUUM of shared blocks.

## r56 Consistency-Aware Durability — CAD / ORCA (FAST20; Ganesan, Alagappan, Arpaci-Dusseau ×2) — read §1–3
- Asynchronous durability (ack before fsync/replication) cannot give strong consistency:
  after a crash a client can see an OLDER state than one already read (non-monotonic
  reads), even when only one node fails. Synchronous durability is ~10× slower (Redis).
  CAD moves the durability point from WRITE to READ: data is made durable/replicated
  before it is first read by anyone; writes stay fast; only unread recent writes can be
  lost. ORCA (ZooKeeper) gets cross-client monotonic reads at near-async speed.
- **For the spec (directly about SynchronousCommitTuningReq):** with
  `synchronous_commit = off`, a commit is VISIBLE to other sessions (other processes on
  the mount, other mounts, SQL users) before it is flushed. After a Postgres crash,
  those readers may observe the data disappearing — exactly the non-monotonic anomaly.
  The FlatFilesystem Durability section mentions loss but not this visibility anomaly.
  CAD gives a cheap fix: keep async commits, but before serving a read (or before
  replying to fsync) of an object whose last-commit LSN is not yet flushed, wait for
  `pg_current_wal_flush_lsn() >= commit_lsn` (usually already true; walwriter flushes
  within `wal_writer_delay`). Track per-inode last commit LSN in the daemon. This turns
  "lose < 1 s silently" into "lose only writes no one has seen", and makes fsync real.

## r57 ScaleCheck (FAST19; Stuardo et al., Gunawi) — read §1–2
- Scalability bugs (55 studied in Cassandra/HDFS/HBase/…): 52 caused by scale-dependent
  LOOPS over growing structures (CPU, disk I/O, or locks inside loops), mostly in rarely
  run OPERATIONAL protocols (bootstrap, rebalance, decommission, block report, fsck,
  recovery), surfacing only at 100+ nodes. Static analysis finds such loops; colocation
  tricks test 512 nodes on one PC.
- **For the spec:** pgfs's scale-dependent structures: (1) the in-memory ino↔path maps
  grow with every path ever looked up and are only re-keyed, never shrunk — FUSE
  FORGET must evict entries (check `forget`/`batch_forget` handling; the spec never
  mentions FORGET); (2) rename re-keys the whole subtree in memory, O(subtree); (3)
  readdir fetches all children per call with synthetic offsets, O(n²) for huge
  directories read in pieces. Add requirements bounding memory by the kernel's live
  inode count and making readdir resume by name (keyset pagination), not by index.
  - Checked in `src/fs.rs`/`src/db.rs` (2026-10-27): there is no `forget` handler, so
    the ino↔path maps only grow; `readdir` re-runs `SELECT … WHERE parent = $1 ORDER BY
    name` (all children) on every call and skips to the synthetic offset — O(n²) for a
    directory read in k pieces. Both are real scale-dependent loops.

## r58 T2C — Deriving Semantic Checkers from Tests (OSDI25; Lou et al., Peng Huang) — read §1–2
- Silent semantic violations are 39% of failures in mature systems. Tests already
  encode semantics (assertions + a workload that exercises them) but only for fixed
  inputs; T2C statically extracts the checking logic, dynamically infers preconditions,
  and turns 672 tests (ZooKeeper, Cassandra, HDFS, HBase) into runtime checkers that
  caught 15/20 reproduced production silent failures (median 0.19 s).
- **For the spec:** pgfs's tests (e.g. test_rename.sh "after mv a b, b has a's bytes")
  could be generalised into runtime/background checkers: periodic SQL assertions of
  the invariants listed under r54 plus "size == bytes reachable from blocks + holes",
  logged at ERROR and counted in metrics. Pairs naturally with the per-Req
  postconditions proposed under r43: each postcondition should have a test and, where
  cheap, a production checker.

## r59 Skybridge (OSDI25; Lyerly et al., Meta) — read §1–2
- Asynchronous replication gives unbounded staleness in practice (one developer polled
  for 40 minutes). Skybridge is a lightweight out-of-band stream of write METADATA
  (keys + HLC versions) with "replication with gap detection": out of order and lossy
  is fine as long as gaps are detected; caches consult it to decide whether an item
  may be stale → 2-second bound for 99.99998% of writes (vs 99.993%) at 0.54% of TAO's
  footprint. TTL caching is rejected (stale too long or spurious misses).
- **For the spec:** exactly the shape of v0.2's LISTEN/NOTIFY coherence channel: NOTIFY
  is lossy across disconnects and bounded in payload; pgfs must detect gaps (a
  per-database monotonically increasing `change_seq` carried in each notification;
  a daemon that sees a gap or reconnects invalidates everything it caches). Without gap
  detection, a missed NOTIFY means a cache that is stale forever (until TTL). Add
  "gap detection" to the coherence requirement.

## r60 PoWER Never Corrupts (OSDI25; LeBlanc, Lorch, Hawblitzel, … Chidambaram) — read §1–2
- Crash consistency verified with plain Hoare logic: every durable write API carries a
  PRECONDITION that all crash states it can produce are legal (PoWER); most proofs only
  need the write's LOCATION ("writes to unreachable bytes can't break anything").
  A CRC model that allows data and checksum to be stored apart and updated non-
  atomically; "corruption-detecting Boolean" primitive. Verified PM KV store (Verus)
  competitive with unverified ones; used for an Azure storage prototype.
- **For the spec:** the PoWER rule maps onto pgfs's ordered-write design (v0.2 §7):
  every statement sequence that is not one transaction must state which intermediate
  states are legal ("blocks written before the entries row points at them are
  unreachable, therefore harmless"). Put that as an explicit "crash states" clause in
  any Req whose effect spans more than one transaction or database (offload aligned
  path: "two statements are not wrapped in one transaction" — KernelOffloading
  Reference says so; that is precisely a missing PoWER obligation).

## r61 WOFS / Wolves — metadata write-once FS (OSDI25; Pan, Xia et al.) — read §1–2
- On persistent memory, journaling/LFS/soft-updates spend >70% of I/O time on small
  ordered metadata writes. WOFS writes each operation's metadata ONCE as a checksummed
  package with a single ordering point, then serves inodes/dentries through a package
  translation layer; recovery replays packages. 97–99% of PM bandwidth; RocksDB 1.2–6.7×.
- **For the spec:** analogous to "one operation = one transaction = one commit" in
  pgfs: each FUSE op should map to exactly one transaction (one ordering point). The
  spec already lists the exceptions (unlink, rmdir not wrapped; offload aligned path
  two statements) — they should be fixed rather than documented.

## r62 Okapi (OSDI25; Athlur, Kim, Kadekodi, … Rashmi, Ganger) — read §1
- Decouples data STRIPING (I/O efficiency) from REDUNDANCY GROUPING (erasure-code width)
  so either can change without re-striping; +80% read throughput, −70% seeks, −50–70%
  I/O for EC transitions; metadata +<1%.
- **For the spec:** the general lesson — keep placement (where bytes go) independent of
  redundancy (how they're protected) — is what v0.2 does by letting Postgres handle
  redundancy per data set and pgfs handle placement per file. Keep those decoupled
  in the spec too (no "replication factor" in pgfs flags; shard.py still has one).

## r63 SpecLog / Belfast (OSDI25; Bhat, … Ganesan, Alagappan) — read §1
- Shared logs order records across shards in batches (Scalog-style), adding delivery
  latency; SpecLog delivers records in a SPECULATED global order and later confirms or
  rolls back; "fix-ante ordering" (predetermined per-shard quotas per cut, padded with
  no-ops) makes speculation almost always right.
- **For the spec:** tangential. If pgfs ever streams changes to other mounts
  (coherence), speculative delivery with later confirmation is overkill; ordered NOTIFY
  from one metadata DB already has a total order (commit order). Note only.

## r64 SquirrelFS (OSDI24; LeBlanc, Taylor, Bornholt, Chidambaram) — read §1–2
- Crash consistency checked by the RUST COMPILER: Synchronous Soft Updates reduce crash
  safety to ordering rules (e.g. a dentry may only point to a durable, initialised
  inode), encoded with the TYPESTATE pattern (an uninitialised inode has a different
  type than a durable one; functions demand the right state). Compiles in seconds;
  model-checked with Alloy; crash-atomic rename; NOVA/WineFS-level performance.
- **For the spec (pgfs is Rust):** typestate can enforce pgfs's ordering obligations at
  compile time instead of in prose:
  - v0.2 ordered writes: `publish_size(meta_tx, DataCommitted<Ino, Lsn>)` — the metadata
    update can only be called with a token produced by the data-set commit.
  - Rename with open handles (Bug 3): rename takes `&mut HandleTable` and returns a
    `Renamed` proof that buffers were re-keyed/flushed; or handles hold ino (not path),
    which removes the bug class entirely.
  - Reconnect (Bug 5): a `Db<Connected>` vs `Db<Broken>` state with the only transition
    being the reconnect-and-revalidate path.
  Add a spec context (alongside err.py's Err/Robustness) "OrderingByType": ordering
  invariants that can be expressed as typestate MUST be.

## r65 RefFS / DLDR liveness verification (OSDI24; Zou, Du, Dong, Haibo Chen) — read §1
- First concurrent FS with mechanised proofs of safety AND liveness (no deadlock,
  livelock, infinite loop). "Dynamically layered definite releases": every acquired
  lock is definitely released; a release may wait only on higher-layer releases;
  layers follow the directory hierarchy (parent may wait for child) plus a temporary
  layer for rename's two parents. Applying it to Linux VFS's documented directory
  locking found a real deadlock (confirmed and fixed).
- **For the spec:** once dispatch is concurrent, pgfs's deadlock surface is in Postgres
  row locks: rename locks two parents (+ victim), unlink locks parent+child, buffered
  flush locks block rows. Two requirements follow: (1) a lock-order rule (lock rows by
  ascending ino, or parent before child, with rename taking both parents in ino order)
  in a concurrency spec; (2) treat SQLSTATE 40P01 (deadlock_detected) and 40001
  (serialization_failure) as RETRYABLE with bounded backoff, not as EIO.
  DistinguishesExpectedErrors currently maps every database error to EIO.

## r66 Chardonnay (OSDI23; Eldeeb, Xie, Bernstein, Cidon, Yang) — read §1
- With fast 2PC (~150 µs on Azure), contention shifts to reading cold data from SSD
  WHILE HOLDING LOCKS. Chardonnay dry-runs each transaction with a lock-free strictly
  serializable snapshot read (epoch-based) to prefetch its read set and compute
  read/write sets, then acquires locks in a deadlock-free order. Only 15% throughput
  loss from low to extreme contention (vs −85% for a System R* baseline).
- **For the spec:** in pgfs's buffered flush, read the existing block contents (needed
  for read-modify-write of partial blocks — the Bug 1 fix) BEFORE opening the write
  transaction / taking row locks, then upsert under lock with a version check. Same
  for rename: resolve and read outside, lock in order inside.

## r67 Metastable Failures in the Wild (OSDI22; Huang, Magnusson, … Charapko) — read §1–2
- 22 metastable failures from 11 organisations (≥4 of 15 major AWS outages): a trigger
  (load spike or capacity drop) plus a SUSTAINING amplification (retries, cache-miss
  storms, GC, failover thrash) keeps the system in low-goodput overload after the
  trigger is gone. Systems run in "vulnerable" states for efficiency (a 90%-hit cache
  in front of a 300 RPS database serving 3,000 RPS).
- **For the spec:**
  - The kernel attribute cache + pgfs is such a vulnerable configuration: `--attr-ttl
    30` lets a database serve 10× more clients; a remount/cold cache (or TTL 0 for
    coherence) can overload Postgres. Document it.
  - Reconnection (Bug 5 fix) must use exponential backoff with jitter and a cap, and
    fail fast with EIO while disconnected rather than queueing every FUSE request behind
    reconnect attempts (queued requests + retries = classic sustaining effect).
  - Replica fallback: when the standby is slow, all reads move to the primary (by
    design) — exactly the moment the primary may be busiest. Consider shedding
    (serving stale-but-bounded reads) rather than always failing over.

## r68 DaisyNFS (OSDI22; Chajed, Tassarotti, Theng, Kaashoek, Zeldovich) — read §1
- Verified NFS server: concurrency and crash safety isolated in a transaction system
  (GoTxn = GoJournal + two-phase locking + allocator); every NFS operation is ONE
  transaction, so its logic is verified with SEQUENTIAL reasoning (Dafny, 2× proof/code
  vs 20× for the transaction layer). Large deletes are split: one transaction logically
  deletes, later bounded transactions reclaim space with no other visible effect.
  Within 90% of Linux NFS+ext4 data=journal.
- **For the spec:** this is pgfs's architecture in miniature — Postgres is the verified-
  ish transaction system, so every FUSE operation implemented as exactly one
  transaction can be specified and tested sequentially. Spec rule: "each FUSE
  operation is one transaction" (exceptions require a crash-states clause, see r60).
  The split-delete pattern (logical unlink now, bounded background block reclamation
  later) is the right shape for unlink of huge files and for v0.2 GC.

## r69 GoJournal / Perennial 2.0 (OSDI21; Chajed et al.) — read §1
- Verified concurrent journaling: callers "check out" a private disk fragment, modify it
  as if sequentially, and "check in" at commit; crash framing means no crash reasoning
  until commit. Verification found a serious concurrency bug that unit tests missed.
  GoNFS reaches ≥90% of Linux NFS/ext4 on NVMe; making the journal sequential capped
  20 clients at 2× one client — concurrency matters for throughput.
- **For the spec:** same lesson as r68, plus a quantitative one for pgfs: serial
  execution of FS operations (pgfs's single-threaded dispatch) caps multi-client
  throughput near single-client throughput regardless of backend speed.

## r70 Storage Systems are Distributed Systems (So Verify Them That Way!) — VeriBetrKV (OSDI20; Hance et al.) — read §1–2
- Treat a storage system as a program interacting with an asynchronous, failure-prone
  environment (the disk) and verify it IronFleet-style (TLA state machines + Hoare
  logic), including crash safety and disk corruption; linear types reduce proof burden
  31–37%; 24× BerkeleyDB inserts, 8× slower than RocksDB.
- **For the spec:** pgfs talks to TWO asynchronous environments: the kernel (FUSE) and
  Postgres (network, crashes, failover). The spec models neither as an environment
  that can fail independently; err.py treats DB errors as generic EIO. A small explicit
  state machine for the daemon's view of the database (Connected → Broken →
  Reconnecting → Revalidating → Connected) with allowed operations per state belongs in
  the spec (it is the Bug 5 fix written down).

## r71 Partial Network Partitions — Nifty (OSDI20; Alfatafta, Alkhatib, Alquraan, Al-Kiswany) — read §1
- 51 failures in 12 systems caused by PARTIAL partitions (A↔C and B↔C fine, A↮B):
  catastrophic (data loss), deterministic, triggerable by partitioning ONE node, usually
  in leader election/replication; most findable with simple 5-node tests. Existing
  defenses patch one mechanism or shut down the cluster; ZooKeeper deployments add a
  vulnerability (clients see ZK but not each other). Nifty reroutes around partitions.
- **For the spec:** v0.2 has exactly this topology: daemon ↔ MDB, daemon ↔ data set,
  MDB ↔ data set (replication), failover manager ↔ all. A daemon that can reach the MDB
  but not a data-set primary (or reaches an OLD primary that the manager can't see)
  is the dangerous case. Requirements: data writes must carry the set epoch and be
  fenced by the set itself (not by what the daemon believes); the test plan must
  include partial partitions (iptables on one link) in the Phase 3–4 scripts.

## r72 Copilot replication (OSDI20; Ngo, Sen, Lloyd) — read §1
- First 1-slowdown-tolerant consensus: two pilots process every command in parallel
  (both order, execute, reply); dependencies + dedup reconcile their orders; a fast
  pilot "takes over" a slow pilot's in-flight ordering. Competitive with MultiPaxos/
  EPaxos normally; the only one that keeps latency low with a slow replica.
- **For the spec:** with Postgres synchronous replication, a SLOW synchronous standby
  slows every commit on that data set (fail-slow, see r122). Use quorum commit
  (`synchronous_standby_names = 'ANY 1 (s1, s2)'`) so one slow standby doesn't stall
  the primary; specify it for v0.2 data sets and for the MDB.

## r73 Virtual Consensus in Delos (OSDI20; Balakrishnan et al., Facebook) — read §1
- Virtualise the shared log: a VirtualLog (control plane: reconfiguration via SEALING a
  log segment and chaining a new one) over pluggable Loglets (data plane: ordering
  only, need not be highly available, only sealable). Production in 8 months on a ZK
  shim, then upgraded online to a native Loglet for 10× latency; older segments moved
  to cold storage.
- **For the spec:** "seal then switch" is the clean primitive for v0.2 data-set moves
  and failover: a set epoch is sealed (no more writes accepted under it) before writes
  resume elsewhere. It also argues for keeping pgfs's control plane (placement,
  epochs) separate from the data plane, with a minimal seal operation as the contract
  between them.

## r74 Assise (OSDI20; Anderson, Canini, … Peter, Reda) — read §1
- Distributed FS that treats client-local persistent memory as a linearizable, crash-
  recoverable CACHE (CC-NVM): leases delegated to node/socket/process, chain replication
  to hot/warm replicas, consistency at I/O-operation granularity rather than fixed
  blocks; 22× write latency, 103× faster failover than Ceph/BlueStore.
- **For the spec:** two transferable points: coherence granularity should match the
  operation, not a fixed block (pgfs invalidates whole files/attrs via FUSE — fine);
  and a "hot replica that mirrors the client cache" is why recovery can be instant.
  Neither is near-term for pgfs.

## r75 FlightTracker (OSDI20; Shi et al., Facebook) — read §1–2
- Read-your-writes over asynchronously replicated caches and indexes: a per-session
  TICKET (metadata of the user's recent writes) is fetched once per request and
  attached to every read; any cache entry possibly older than a write in the ticket is
  a "consistency miss" and is refetched. Replaced write-through caching, which tied
  consistency to a fixed topology. 10^15 queries/day.
- **For the spec:** the ticket for a pgfs daemon is tiny: its own last commit LSN (and,
  for multi-mount RYW via the kernel, nothing more is needed). Replica reads must carry
  it: serve from the standby only if `pg_last_wal_replay_lsn() >= ticket`, else wait
  briefly or go to the primary. This is the precise fix for the documented
  "a read can reach the standby up to 500 ms after the primary moved ahead of it,
  including right after this daemon's own write" (ReplicatedFilesystem Reference).

## r76 Pivoted Query Synthesis — SQLancer (OSDI20; Rigger, Su) — read §1–2
- Finds DBMS LOGIC bugs by choosing a random pivot row and synthesising a query that
  must return it; 121 bugs in SQLite, MySQL, PostgreSQL (8 in PostgreSQL), 96 fixed;
  differential testing across DBMSs fails because dialects differ.
- **For the spec:** pgfs's correctness rests on Postgres's; PostgreSQL logic bugs are
  rare but real. ProvidesProjectLocalCluster pins Postgres 17 for development, but the
  spec says nothing about supported server versions for `--conn`. Add a supported-
  version range and run the integration suite against each.

## r77 Crash Consistency in Block-Level Caching — Open CAS (ATC25; Duan, Chen) — read §1–2
- Persistent block caches (Open CAS, NVCache) break the classic assumption that the
  cache is empty after a crash; with a workload-oriented crash tester the authors find
  Open CAS returning bad data after crashes in some modes, and file-system reliability
  features (designed against a volatile page cache) incompatible with it.
- **For the spec:** pgfs adds a cache layer (per-handle dirty buffers + kernel page
  cache + optionally writeback cache) between applications and a durable store. Any new
  cache mode (KernelWritebackCacheReq, FOPEN_KEEP_CACHE) needs crash tests that kill
  the daemon and the database mid-workload and check what applications observe —
  the workload-oriented testing this paper recommends.

## r78 FiDe (ATC25; Rovelli et al., SAP) — read §1
- Reliable crash failure detection in <30 µs using isolated OS pipelines and reserved
  network paths, enabling simpler, faster consensus (up to 2.23× throughput).
- **For the spec:** tangential. The general point — timeouts are the weak link of
  failure detection — applies to pgfs: there are NO database timeouts at all
  (checked: no statement_timeout, connect_timeout, lock_timeout or TCP keepalive in
  src/). A hung connection blocks every FUSE request indefinitely (applications in D
  state). Specify connect/statement/lock timeouts, TCP keepalives, and FUSE INTERRUPT
  handling (cancel the running query with `pg_cancel_backend`/cancel token).

## r79 Fail-Slow Hardware Failure Bugs — Sieve (ATC25; Dong, Hua, Zhang, …) — read §1
- 48 real fail-slow (degraded, not dead) hardware failures: all caused by synchronized
  blocks or timeout handlers around I/O that a slow device stalls (e.g. ZooKeeper froze
  for writes while heartbeats stayed healthy — five months to diagnose). Fine-grained
  slowness (some operations, not all) is what escapes detection. Sieve injects
  slowness at synchronized/timeout-protected I/O points found by static analysis.
- **For the spec:** pgfs is one big synchronized block: single-threaded dispatch around
  one database connection. A slow (not dead) Postgres stalls every operation on the
  mount while the liveness thread (separate thread) stays healthy — DetectsDeadlock will
  not fire, by design. That's acceptable only with statement timeouts (above) and a
  WARN-level "slow query" log with the threshold (observe.py LogLevels already asks for
  "query took 1.2s, threshold 200ms" — not implemented; make it a Req).

## r80 CAFault (ATC25; Chen, Ma, Zhou, Yan, Jiang) — read §1
- Fault-injection testing at the DEFAULT configuration misses fault-handling bugs that
  only appear under other configurations; CAFault learns which config items affect
  fault handling and fuzzes faults per configuration (+30–80% fault-tolerance logic
  coverage, 16 new bugs in HDFS, ZooKeeper, MySQL Cluster, IPFS).
- **For the spec:** pgfs's behaviour under faults depends on flags it treats as
  orthogonal: `--attr-ttl`, `--unlogged`, `--replica`, synchronous_commit, (future)
  writeback cache, pool size. Fault tests (DB restart, standby loss) should run under a
  small matrix of these, at least {ttl 0 / 30} × {replica on/off} × {unlogged on/off}.
  Note `--unlogged` + crash = empty tables; the spec says so, but no test checks that
  pgfs then fails safe rather than serving a half-empty tree.

## r81 Monarch (ATC24; Lyu, … Payer, Kashyap) — read §1–2
- First multi-node fuzzer for POSIX DFSes: VM-based controller/workers, two-step mutator
  (syscall sequences, then syscalls + distributed faults), unified coverage across
  user/kernel contexts, and SymSC, a semantic checker that compares against a POSIX
  model. 48 bugs in 6 DFSes (8 semantic), one present since the initial release.
- **For the spec:** reinforces r48 (Metis): a semantic checker against a POSIX model,
  driven by generated syscall sequences, plus injected faults (Postgres restart,
  standby kill, network delay) is the testing requirement pgfs is missing. Monarch's
  categories of DFS semantic bugs (stale reads after rename, lost writes after
  failover, wrong size after truncate under concurrency) map one-to-one to pgfs's
  known bug list.

## r82 Distributed Transactions at Scale in DynamoDB (ATC23; Idziorek, … Terry, Vig) — read §1–2
- ACID transactions added to DynamoDB as SINGLE-REQUEST transactions (no interactive
  BEGIN … COMMIT; avoids long-held resources in a multi-tenant service), timestamp
  ordering without locks, non-transactional Gets/Puts stay on the fast path but are
  serialised against transactions; single-version store.
- **For the spec:** pgfs uses interactive transactions over a network round trip per
  statement. Packing each FUSE operation's statements into ONE round trip (a single
  multi-statement query, a server-side function, or pipelining) is the DynamoDB lesson
  and the main latency win for remote databases — this is what RemoteAsyncBackendReq
  ("query pipelining") gestures at; specify it per operation (create = 1 RTT, rename =
  1 RTT, unlink = 1 RTT).

## r83 RubbleDB (ATC23; Li, Jiang, Chen, Raina, Zhu, Luo, Cidon) — read §1
- In replicated LSM stores compaction is redundant work on every replica; RubbleDB
  compacts once on the primary and ships compacted files to secondaries over NVMe-oF
  with NIC offload (zero target CPU), using pre-allocated fixed-location files, a file
  map, and ordered version-edit application. Up to 1.9× throughput, −93% tail latency.
- **For the spec:** Postgres physical replication already ships the primary's work
  (standbys replay WAL, they don't re-plan), so pgfs gets this property for free; the
  cost is that standbys also replay VACUUM/TOAST churn. Nothing to change; note for the
  v0.2 cost model (replication traffic = WAL volume, including full-page images).

## r84 Data Placement for Balanced Recovery Load (ATC23; Shan, Chen, Wu) — read §1–2
- Fine-grained data units balance recovery load but raise the probability that some
  placement group loses data (copyset effect) and inflate metadata; optimal placement
  for balanced recovery is NP-hard; a greedy algorithm balances recovery without fine
  units (1.7–2.4× faster recovery), and supports cheap expansion.
- **For the spec:** for v0.2 per-file placement over data sets, "which set receives a
  new file" should consider recovery load per set, not only free space; low priority.

## r85 Speculative Recovery (ATC22; Li, Kalaba, Freedman, Lloyd, Levy) — read §1
- "Recovery from disaggregated storage" (a backup instance remounts the dead primary's
  network disk) is cheap but slow because detection precedes recovery. Speculative
  recovery starts recovery on a CLONE of the disk as soon as the primary looks dead,
  lets both race, and `collapse` keeps exactly one (a dirty bit shows whether the
  primary wrote after `super`; if so the backup is discarded). Evaluated with MySQL,
  PostgreSQL, MariaDB.
- **For the spec:** relevant to how pgfs relies on Postgres HA: the spec should not
  assume failover is instant; v0.2 delegates to pg_auto_failover/Patroni whose
  detection timeouts (tens of seconds) dominate. pgfs must ride through that window
  (reconnect with backoff, keep the mount, return EIO or block with a bound — choose and
  specify).

## r86 Modulo (ATC22; Kim, Kim, Lie) — read §1
- Convergence-failure bugs (replicas never converge after failures) found by model-
  based testing that deliberately drives replicas into DIVERGENCE and back (Divergence
  Resync Models), avoiding full state explosion; 11 bugs in ZooKeeper, MongoDB, Redis.
- **For the spec:** pgfs-level divergence states: daemon in-memory ino map vs DB; kernel
  cache vs DB; standby vs primary; (v0.2) MDB vs data set. Each has a resync path
  (FORGET/invalidation, reconnect revalidation, replica wait, GC/repair). Test each
  resync path by forcing divergence (e.g. rename via SQL behind pgfs's back, then use
  the mount) — today a rename done in SQL leaves pgfs's path map stale with no resync
  except remount.

## r87 DepFast (ATC22; Luo, Shen, Mu, Xu) — read §1
- Coroutine framework with a composable QuorumEvent so quorum protocols (Raft, Copilot)
  are written synchronously instead of callback hell; 25–35% less code, better fail-slow
  tolerance by construction.
- **For the spec:** if pgfs moves to async (RemoteAsyncBackendReq: tokio-postgres +
  fuse3), structure each FUSE operation as one async function with explicit timeouts and
  cancellation (FUSE INTERRUPT), not callbacks; v0.2's "data commit then metadata
  commit" with a timeout is exactly a two-event wait.

## r88 NVMe SSD Failures in the Field (ATC22; Lu, Xu, … Alibaba) — read §1–2
- 1M+ NVMe SSDs at Alibaba: little infant mortality; low-WAF drives fail MORE; intra-
  node/rack failures far more temporally correlated than SATA (14.7×/1.8×); fail-slow
  affects 1.41% of drives in four months (6× HDD), can degrade to HDD speed, does not
  correlate with SMART, and rarely turns into fail-stop.
- **For the spec:** a Postgres server under pgfs will sometimes be slow, not dead, for
  long periods. The observability spec measures DB latency histograms but sets no
  alert thresholds; add a "slow database" signal (p99 above N× baseline for M minutes)
  to the metrics requirements, and make correlated failures part of v0.2 placement
  (members of a data set on different hosts/racks).

## r89 Lodic (ATC21; Park, Hwang, Choi, Min, Won) — read §1
- Per-PROCESS (not per-core) distributed reference counters embedded in PTE bits for
  shared file pages: 65× on shared-block reads, near-ideal scaling for RocksDB/NGINX.
- **For the spec:** kernel-internal; no pgfs implication beyond noting that heavily
  shared read-mostly files benefit from the kernel page cache (FOPEN_KEEP_CACHE), which
  pgfs only gets with correct invalidation.

## r90 Can Applications Recover from fsync Failures? (ATC20; Rebello, Patel, Alagappan, Arpaci-Dusseau ×2) — read §1–2
- ext4/XFS/Btrfs mark pages CLEAN after a failed fsync (retry is useless); reporting
  differs (ext4 data mode sometimes fails the NEXT call). Five applications (PostgreSQL,
  LMDB, LevelDB, SQLite, Redis) all mishandle fsync failures somehow — Redis ignores the
  return code, PostgreSQL could lose old and new data ("fsyncgate"; Postgres now PANICs
  and recovers from WAL). Apps that recover from the page cache instead of disk return
  stale data later. CuttleFS, a FUSE FS, emulates each behaviour for testing.
- **For the spec:** pgfs IS the file system here, so it defines fsync-failure semantics
  for its users. Specify them: (1) a failed flush/fsync (DB error) returns EIO and the
  dirty buffer is RETAINED (not discarded) so a retry can succeed; (2) the error is
  reported on the fsync/close that observes it and not silently on a later unrelated
  call; (3) after a failed commit pgfs never serves the uncommitted bytes to other
  handles as if durable (today reads flush first — a failed flush must make the read
  fail too, not return stale DB contents). None of this is written down now; the spec
  only says flush errors are `EIO`.

## r91 Everyone Loves File — Oracle Cloud FSS (ATC19; Kuszmaul, Frigo, Mazzola Paluska, Sandler) — read §1–2
- Managed NFSv3 service: Paxos-replicated block store (DASD) with limited-size multipage
  transactions → a B-tree with linearizable multi-key limited-size transactions → file
  system METADATA in the B-tree (file data directly in DASD); stateless presentation
  hosts (NFS → FSS protocol) that fail over trivially; snapshots via versioned keys;
  self-validating B-tree nodes; all housekeeping as separate transactions; lock-free
  non-blocking style; per-file encryption keys; latency ~4× a local NVMe NFS server.
- **For the spec:** the strongest production precedent for pgfs: metadata as rows in a
  transactional, replicated store, every FS op a bounded transaction, stateless front
  ends, snapshots from versioned keys. Two specifics to adopt: (1) BOUNDED transaction
  size (FSS splits work into limited-size transactions; pgfs's path-key directory rename
  is unbounded); (2) per-op transactions that include housekeeping only as separate
  transactions (GC, size reconciliation), matching v0.2 lazy GC.

## r92 IASO (ATC19; Panda, Srinivasan, Ke, Gupta, Khot, Gunawi; Nutanix) — read §1
- Peer-based, load-aware fail-slow detection built only from TIMEOUT signals already
  present in the system; deployed on 39,000 nodes, caught 232 incidents in 7 months
  (AFR ≈ 1%), cut full outages from >25 to 2; fail-slow causes are diverse, so detection
  must be at service level, and mitigation (quarantine) must be automatic.
- **For the spec:** pgfs has a natural peer set for replicas (primary vs standby latency
  on the same query) — if the standby is consistently slower, stop routing to it
  (proportional, see r03 CRUSH overload) rather than only on LSN staleness.

## r93 FlexGroup Volumes (ATC19; Kesavan et al., NetApp) — read §1–2
- One namespace over many single-node WAFL volumes: any dentry may point to an inode on
  another member volume; a Remote Access Layer does cross-volume transactional updates;
  heuristics place new files/dirs to balance capacity and load while limiting remote
  links (mkdir is the expensive cross-node case); HA pairs replay NVRAM logs on takeover.
  Field data from 1,000+ deployments.
- **For the spec:** the v0.2 "metadata in one MDB, data on sets" is simpler than
  FlexGroup's cross-volume namespace; FlexGroup is the evidence that per-file placement
  heuristics (balance capacity, keep related files local) work in the field. Their
  lesson: placement heuristics need production telemetry to tune — v0.2 should record
  per-set capacity and load in `data_sets` from day one.

## r94 Protocol-Aware Recovery — CTRL (FAST18; Alagappan, Ganesan, … Arpaci-Dusseau ×2) — read §1–2
- Recovering a corrupted entry from replicas is unsafe without protocol knowledge
  (fixing from a stale replica loses data; discarding uncommitted vs committed entries
  needs global commitment info). CTRL: a storage layer that distinguishes crash-induced
  torn writes from media corruption, plus a recovery protocol that determines global
  commitment and uses leader-initiated identical snapshots. LogCabin and ZooKeeper
  become safe under storage faults with little overhead.
- **For the spec:** pgfs relies on Postgres for this (page checksums, WAL CRCs; a
  corrupted standby page is not repaired automatically). Requirements: enable
  `data_checksums` on clusters pgfs creates; document that block corruption surfaces as
  EIO with the Postgres error in the log; for v0.2, repair = rebuild the standby from
  the primary (pg_basebackup/pg_rewind), never "copy rows from whichever replica".

## r95 Dayu (ATC19; Wang, Zhang, Wang, Yang, Zhu) — read §1
- Recovery scheduling in Pangu (10K nodes): timeslot-based scheduling of a subset of
  re-replication tasks per slot, greedy with convex-hull/bucket approximations,
  prioritising nodes with idle bandwidth; fast and low-interference with foreground I/O.
- **For the spec:** v0.2 `set drain` / rebalancing should be rate-limited and scheduled
  in bounded batches that yield to foreground traffic (a `--max-move-rate` and per-batch
  transactions), not one big copy.

## r96 — not on disk (ACM-only; listed as missing in README.md). Not read.

## r100 Morph (SOSP24; Kim, Athlur, Kadekodi, … Ganger, Rashmi) — read §1
- Files change redundancy over their lifetime (3-way replication at ingest → narrow EC →
  wider LRC); transcoding is 20–33% of 5–13 PB/hour of cluster I/O at Google. Morph:
  hybrid "1 replica + EC stripe" at ingest (free first transition by deleting the
  replica), Convertible Codes for EC→EC with placement that avoids data movement;
  −95% transcode I/O, −50–60% ingest+transcode I/O.
- **For the spec:** only relevant if pgfs ever tiers blocks (hot rows in Postgres, cold
  blocks elsewhere). The design lesson — make lifecycle transitions native and cheap
  by planning the target layout at ingest — would apply to v0.2 data-set moves: write
  new files where they will live, rather than moving them later.

## r118 All File Systems Are Not Created Equal — BOB & ALICE (OSDI14; Pillai, Chidambaram, … Arpaci-Dusseau ×2) — read §1–2
- POSIX does not specify crash behaviour; file systems differ in PERSISTENCE PROPERTIES:
  atomicity of operations (is rename atomic? are appends atomic?) and ORDERING (are
  operations persisted in issue order?). BOB measures them by reordering block traces;
  ALICE replays application syscall traces under weaker orderings and found 60 crash
  vulnerabilities in 11 applications (git, SQLite, LevelDB, PostgreSQL, HDFS,
  ZooKeeper, …) — e.g. git requires appends to be persisted before a later rename.
- **For the spec (important):** pgfs should STATE its persistence properties, because
  they are unusually strong and applications can rely on them:
  - every metadata operation (create, mkdir, unlink, rmdir, rename, truncate) is
    atomic (one transaction) and operations are persisted in COMMIT ORDER (WAL order),
    i.e. pgfs provides "ordered metadata" like ext3 data=ordered — even with
    synchronous_commit off a crash loses a SUFFIX of operations, never a middle one;
  - DATA written through an open handle is buffered and becomes durable (and ordered
    relative to other operations) only at flush/fsync/close; a rename of a file that
    still has buffered writes is NOT ordered after those writes (Bug 3 — and the exact
    git vulnerability ALICE describes). Fix Bug 3 so "appends before rename" holds:
    rename must flush the source's buffered handles first (or handles key by ino).
  Add a "PersistenceProperties" Req listing these as testable clauses, and test them
  with an ALICE-style trace replay against pgfs (kill Postgres at each commit boundary).

## r119 Simple Testing Can Prevent Most Critical Failures — Aspirator (OSDI14; Yuan et al.) — read §1–2
- 198 production failures (Cassandra, HBase, HDFS, MapReduce, Redis): almost all
  reproducible with ≤3 nodes; multiple ordered inputs needed; logs usually contain the
  triggering events. 92% of CATASTROPHIC failures come from incorrect handling of
  non-fatal errors explicitly signalled in code; 35% from trivial patterns — error
  handlers that are empty, that abort on a non-fatal error, or contain "TODO/FIXME".
  Aspirator (static checker for those patterns) would have prevented >30%.
- **For the spec:** err.py already bans silent `let _ =` but explicitly allows
  `fallible().ok(); // intentional: ...` — precisely Aspirator's "ignored exception"
  pattern. Require every such site to be listed and covered by a test that drives the
  error (fault injection at the Db boundary), and add a CI check (clippy lint
  `let_underscore_must_use`, grep for `.ok();`, `unwrap()` outside tests). Error
  handling code is the last line of defense; the spec's error contexts describe
  formatting of errors, not testing of handlers — add the latter.

## r120 Torturing Databases for Fun and Profit (OSDI14; Zheng, Tucek, … Lillibridge) — read §1
- Record block traces under iSCSI, replay prefixes to simulate CLEAN power loss at any
  point; self-checking workloads test ACID. All 8 databases tested (SQLite, Tokyo
  Cabinet, commercial OLTP …) misbehaved; 7 violated ACID; five low-level I/O patterns
  predict vulnerable points (20× faster search).
- **For the spec:** pgfs inherits Postgres's durability, which in turn depends on the
  storage honouring flushes. The spec should require `fsync = on` and
  `full_page_writes = on` on any cluster pgfs initialises (and refuse/ warn when
  connected to a server with `fsync = off`), and never place pgdata on storage known
  to drop cache flushes. `--unlogged` must keep printing what it gives up.

## r121 Redundancy Does Not Imply Fault Tolerance — CORDS (FAST17; Ganesan, Alagappan, Arpaci-Dusseau ×2) — read §1–2
- errfs (a FUSE FS) injects single file-system faults (corruption, read/write errors)
  under 8 distributed stores; a single fault can cause data loss, corruption,
  unavailability, and even SPREAD corruption to intact replicas (via read repair, leader
  election, resync). Systems rarely use redundancy to recover; crash and corruption
  handling are conflated.
- **For the spec:** (1) physical replication copies pages, so primary-side corruption
  that Postgres doesn't detect (checksums off!) is faithfully replicated — the
  ReplicatedFilesystem explanation ("a standby failure never causes data loss") should
  not be read as protection against corruption; enable data checksums (again).
  (2) The errfs approach is directly usable to test pgfs's error paths: run Postgres's
  data directory on an errfs-like FUSE layer in CI and check pgfs replies EIO with a
  logged chain and never returns wrong bytes.

## r122 Fail-Slow at Scale (FAST18; Gunawi et al., 12 institutions) — read §1–3
- 101 fail-slow hardware incidents across disks, SSDs, CPUs, memory, NICs: faults convert
  between forms (error masking → slowness), symptoms vary (permanent, transient,
  partial, transient stop), cause chains are long (a dead fan → vibration → slow disk),
  impacts cascade (a slow NIC locks slots on healthy machines); diagnosis takes hours to
  months; 39% of causes are external (environment, configuration).
- **For the spec:** full-stack visibility is the recommendation. pgfs's spans already
  separate FUSE time from DB time; add the database server's own view (pg_stat_activity
  wait events, `pg_stat_io`) to the SIGUSR1 dump so an operator can tell "Postgres is
  slow" from "the network is slow" from "pgfs is slow".

## r123 CrashMonkey & ACE — Bounded Black-Box Crash Testing (OSDI18; Mohan, Martinez, … Chidambaram) — read §1–2
- Study of 5 years of Linux FS crash-consistency bugs: nearly all reproduce with ≤3
  operations on a fresh FS, with a crash right after a PERSISTENCE POINT (fsync/
  fdatasync/sync). B3 exhaustively generates such small workloads, crashes after each
  persistence point, and checks that everything persisted survives. Found 24/26 known
  bugs and 10 new ones (non-atomic rename, files vanishing after fsync).
- **For the spec:** apply B3 to pgfs directly: generate ≤3-op workloads over the mount,
  call fsync, then `pg_ctl stop -m immediate` (and/or kill the daemon), restart, check
  what fsync promised. TODAY pgfs FAILS this test by design: `synchronous_commit = off`
  means an fsync'd file can vanish after a Postgres crash (FlatFilesystem Reference
  says so). This is the strongest single argument to make fsync durable (commit with
  synchronous_commit=on/local for the fsync's transaction, or wait for flush LSN) while
  keeping async commit for everything else — the B3 contract is exactly "persisted
  before a persistence point ⇒ survives".

## r124 Gray Failure (HotOS17; Huang, Guo, Zhou, Lorch, Dang, Chintalapati, Yao) — read in full
- Most Azure incidents are GRAY failures (slow, flaky, partially broken), defined by
  DIFFERENTIAL OBSERVABILITY: the failure detector sees "healthy" while applications
  suffer (heartbeat module fine, request module stuck). More redundancy can lower
  availability under gray failure (fan-out touches every switch). Advice: detect from
  the application's vantage point; bridge the observability gap.
- **For the spec (checked against src/):** DetectsDeadlock is a textbook differential-
  observability gap. The liveness thread bumps a counter every 10 s and FUSE callbacks
  check it ON ENTRY (`src/fs.rs:100-107`); so it detects a stalled HEARTBEAT thread,
  not a stalled DISPATCH thread. If the single dispatch thread blocks inside a database
  call (hung connection, lock wait), no callback runs, nothing checks, and the mount
  hangs while the heartbeat stays healthy. The detector should measure what users
  feel: the age of the oldest in-flight FUSE request (stamp on entry, clear on reply,
  checked by the watchdog thread), combined with statement timeouts (r78).

## r125 Metastable Failures in Distributed Systems (HotOS21; Bronson, Aghayev, Charapko, Zhu) — read in full
- Framework: stable → vulnerable (high efficiency, load above a hidden threshold) →
  metastable (a trigger plus a SUSTAINING feedback loop such as retries, cache
  refill, or slow-path work amplification keeps goodput near zero after the trigger is
  gone; needs a strong corrective push). Examples: retry storms, look-aside cache loss,
  slow error paths. Root cause is the sustaining effect, not the trigger.
- **For the spec:** see r67. Concretely for pgfs: (1) any retry (reconnect, 40001/
  40P01 retry) must be bounded and backed off; (2) the kernel's own retries and
  applications retrying EIO can sustain overload of the database — the daemon should
  shed (fail fast with EIO while disconnected) rather than queue; (3) document the
  "attr cache in front of Postgres" vulnerability.

## r126 Socrates: The New SQL Server in the Cloud (SIGMOD'19; Antonopoulos et al.): I read §1–2 and §4 (XLOG, Page Servers, RBPEX)
- Socrates separates **durability** from **availability**. Durability comes from the log: the primary writes log blocks synchronously to a durable "landing zone". Availability comes from a fleet of stateless page servers and secondaries. XLOG disseminates the log to them asynchronously.
- The design goal is that no operation should be O(size-of-data). Seeding, backup, restore and scale-up all become O(1) or bounded by sharding pages across page servers and taking snapshots in cheap storage.
- RBPEX (a resilient buffer-pool extension) is a recoverable SSD cache. A restarted compute node comes back warm, which cuts mean time to recovery.
- **For the spec:** this gives v0.2 the cleanest vocabulary for its data-set design. A pgfs data set's durability point is "the synchronous standby set has flushed the WAL". Availability (which node serves reads) is a separate policy. The spec should keep the two apart: a `DurabilityPointReq` that names exactly which WAL flush acknowledges an fsync, and a replica policy that may be weaker. Socrates also argues that the size of a set should not decide the cost of adding a member. Learner-first set add (r08) plus `pg_basebackup` is O(data), so the spec should at least state that cost and rate-limit it (r95).

## r127 Mantle: Hierarchical Metadata for Cloud Object Storage (SOSP'25; Li et al., Baidu): I read §1–2 and the design overview
- The architecture has two layers. A sharded transactional DB (TafDB) holds all metadata. A per-namespace, single-server IndexNode (Raft-replicated) caches only directory metadata, so path resolution is **one RPC**.
- Path lookup accounts for more than 89% of metadata latency in real namespaces, whose depth averages about 11 and reaches 95.
- **Delta records** turn contended in-place updates of a directory's attributes (mtime, nlink, size) into conflict-free appends. Readers fold the deltas.
- Loop detection for cross-directory rename moves to the IndexNode, so a rename locks and validates in one RPC.
- Lookups are offloaded to Raft followers and learners.
- **For the spec:** pgfs is already "one DB plus path keys", which gives single-statement lookup for free. That is the one advantage of path keying. Moving to ino keys (r00/r07) gives it up, so v0.2 needs a lookup plan: a dentry cache with NOTIFY invalidation, or a recursive CTE resolving a path in one round trip. Delta records are a direct answer to directory-attribute contention. If pgfs ever maintains directory mtime/nlink (today these are the epoch), do it as appended deltas compacted later, not as an `UPDATE` of a hot parent row. Rename loop checking needs a single serialisation point (advisory lock, r16). Mantle confirms that this works at scale.

## r128 Using Lightweight Formal Methods to Validate a Key-Value Storage Node in Amazon S3 (SOSP'21; Bornholt et al.): I read §1–4
- ShardStore is 40k lines of Rust. Its specifications are **executable reference models** in the same language: a hash map stands in for the LSM tree. The models are about 1% of the code and double as mocks in unit tests, so engineers keep them current.
- Correctness is decomposed into three properties, each with its own tool:
  - **functional:** property-based tests run random operation sequences against both the model and the implementation, then compare outputs and check invariants;
  - **crash consistency:** the model is extended with the recent mutations that may be lost, and histories are generated with arbitrary crashes;
  - **concurrency:** stateless model checking (loom/shuttle) shows linearizability against the model.
- Argument selection is biased with domain knowledge, and code coverage tracks how effective the harness is. The harness is about 12% of the code. It prevented 16 production bugs, and 18% of the harness is now written by non-experts.
- Crash ordering is declared with a `Dependency` type: an append is not issued until its input dependency is persisted.
- **For the spec:** this paper is the template for how pgfs should state and test its contracts, and it is Rust, as pgfs is.
  - Add a `ReferenceModelReq`: an in-memory `BTreeMap<path, Node>` model of the filesystem in `tests/`, behind the same trait as the `Db` layer. The `PgfsBackendTraitReq` in optimize.py already wants such a trait.
  - Add `ModelConformanceReq`: proptest-generated op sequences through the FUSE-level API against model and real pgfs.
  - Add `CrashConformanceReq`: kill the backend or drop the connection mid-history, and assert the post-recovery state is one the model permits. With `synchronous_commit=off`, "permits" must include losing recently acknowledged ops. Writing that down is exactly what forces the fsync durability decision.
  - Add `LinearizabilityReq` (shuttle) once dispatch becomes multi-threaded.
  This would have caught Bugs 1–3 mechanically.

## r129 Exploiting Nil-Externality for Fast Replicated Storage (Skyros) (SOSP'21; Ganesan, Alagappan et al.): I read §1–3
- A **nil-externalizing** operation changes state but returns nothing except the acknowledgement: a blind put, a delete, a merge. Such an operation must be made durable immediately, but it can be *ordered and executed lazily*, so long as that happens before a later read externalises its effect.
- Skyros makes nilext writes durable in one RTT using a supermajority quorum and a durability log. Reads check whether the target has been ordered yet and force ordering only when it has not. A DAG-based resolution recovers the order during view changes.
- Making durability lazy is unsafe. Deferring ordering is fine.
- **For the spec:** POSIX `write()` is nearly nilext: it returns only a byte count. pgfs already relies on this in the per-handle write buffer, which is a deferred *apply*. Skyros states the rule that makes this safe. **Deferral is fine only if (a) every externalising operation on the same file is ordered after the deferred writes, and (b) durability is not deferred past the point the API promises.** pgfs breaks (a): a `read`, `getattr` (size) or `rename` from another handle does not see the buffer, and rename loses it (Bug 3). It breaks (b) at fsync. So the BufferedWriteFlushReq in optimize.py needs an explicit invariant: *any op that externalises a file's state (read, getattr, lookup size, readdir size, rename, link, setattr, copy_file_range) must first drain or merge pending buffers for that path, from all handles.*

## r130 Scale and Performance in a Filesystem Semi-Microkernel (uFS) (SOSP'21; Liu, Arpaci-Dusseau et al.): I read §1–2 and the design overview
- uFS is a user-level filesystem server built on SPDK. A primary thread handles metadata. Worker threads each *own* a set of inodes and serve all data operations for them without locks. Inodes migrate between workers to balance load, and the number of workers scales dynamically with demand.
- Clients cache file descriptors and data under **leases**. A rename or unlink invalidates the FD lease. A writer waits for two lease terms to expire before reads are allowed again.
- Crash consistency comes from ordered metadata journaling.
- **For the spec:** this is the concurrency model to copy for MultiThreadedConnectionPoolReq: **shard by inode, not by request.** Each worker owns a connection plus the inodes hashed to it. Data operations on one file then serialise on one worker, which keeps per-file ordering and buffer coherence (r129) without locks. Metadata operations that span parents go through a coordinator with an advisory lock (r16). A lease-based client cache is what the kernel attr/entry TTLs already approximate (KernelAttrCacheTtlReq). uFS states the cost explicitly (a wait of two terms on a conflicting write), and pgfs should state its own TTL staleness the same way.

## r131 HMFS: Decoupling Directory Semantics from Metadata Indexing (SoCC'25; Lv, Lu, Shu et al.): I read §1 and the design overview
- 95.3% of metadata operations in production traces are point operations (stat, create, open). Only listing needs order.
- HMFS indexes metadata in an **unordered hash**, with per-directory linked lists for readdir. Single-point operations need one atomic write, and double-point operations use a few ordered atomic writes. Crash inconsistencies are detected and repaired **on demand**, using the relationships between objects, not by logging.
- Directory listing is parallel and pipelined.
- **For the spec:** this applies in the Postgres setting too. Point lookups on `(parent_ino, name)` should use a unique index (a B-tree is fine), and readdir should not be an `ORDER BY name` over the whole directory on every call (src/db.rs:195). Paginate by keyset with a cookie (`name > $last`) that the readdir offset can carry (r57). Nothing in POSIX requires sorted readdir, but a stable order across calls is required for correct offsets, and keyset on `name` gives that. On-demand repair is a cheap version of the `pgfs check` invariant checker (r54): verify invariants lazily on lookup, as well as in batch.

## r132 Fletch: In-Switch File-System Metadata Caching (arXiv'25; Liu, Lee et al.): I read §1–2
- Fletch puts a cache in the programmable switch, which sees all client requests. It cuts invalidation cost from O(clients) to O(1) and absorbs skew: under 3% of files take 34–39% of requests.
- Its path-aware admission and eviction caches a file only when its ancestors are cached. Multi-level read/write locks cover the path, and variable-length paths are hashed to fixed keys with collision handling.
- **For the spec:** the switch itself is irrelevant to pgfs, but two ideas carry over. (1) In a multi-mount v0.2 deployment, invalidation fan-out grows with the number of mounts. LISTEN/NOTIFY on the MDB is pgfs's central point, as the switch is for Fletch, so state "one NOTIFY per mutation, every mount listens" as O(1) work for the writer. (2) A pgfs dentry cache, if added, must be **path-dependent**: when a directory is renamed or removed, every cached descendant must be invalidated, keyed by ancestor. Ino-keyed dentries make this free: a cached `(parent_ino, name)` stays valid across an ancestor rename. That is another argument for ino keys.

## r133 SwitchΔ: Accelerating Ordered Writes via In-Network Visibility (ICDE'26; Lu et al.): I read §1–2
- In a split data/metadata store, an **ordered write** installs the data first and then updates metadata to make it visible. Both steps sit on the critical path.
- SwitchΔ acks once the data is installed and buffers the pending metadata update in the switch, so reads see it. Metadata nodes apply updates asynchronously and in batches. When the switch is out of resources, the write falls back to the ordered two-phase path. Timestamps order accelerated writes against unaccelerated ones.
- **For the spec:** v0.2's ordered write (data on set S, then the placement/size row on the MDB) is exactly this pattern, and this paper gives the two invariants v0.2 must keep. (1) **Data is never referenced before it is durable.** Write blocks on S with a synchronous commit first, then commit the MDB row. (2) If metadata is deferred, the deferred state must be readable by every reader, not only the writer. Without a switch, pgfs's version of (2) is the per-handle buffer: invisible to other readers, so not linearizable. Either state that pgfs gives *close-to-open* consistency only (NFS-style) and document it in the spec, or make buffered state visible. Batching deferred MDB updates (operation combining) is a valid TransactionBatchingReq technique *after* the data commit.

## r134 Disaggregation: A New Architecture for Cloud Databases (VLDB'25 vision; Xiangyao Yu): I read the full paper (short)
- The paper traces a progression: storage disaggregation (Aurora, Snowflake), then further splitting (Socrates' log, page cache and page store), then compute pushdown, intermediate-data caches and lakehouse metadata layers.
- Cornus rethinks 2PC when storage is disaggregated: participants' votes live in shared storage, so a coordinator failure does not block. Disaggregate only where the communication cost is justified.
- **For the spec:** pgfs is itself a disaggregation of a filesystem: FUSE compute over Postgres storage. The "only when justified" rule is the test for v0.2. Splitting the MDB from the data sets costs a cross-database round trip per write. Do this only above a stated size or throughput threshold, and keep a single-set mode (MDB = data set) as the default. Cornus is relevant if v0.2 ever needs atomic cross-set operations, such as a cross-set rename of file data. The better answer is still the v0.2 plan to avoid 2PC (move data lazily, and make the metadata row the single commit point).

## r135 When Amnesia Strikes: Reproducing Data Loss Bugs with Fault Injection (LazyFS) (VLDB'24; Ramos, Kingsbury, Paulo et al.): I read §1–2 and the PostgreSQL/Redis experiments
- LazyFS is a **FUSE** filesystem with its own page cache. It injects lost writes, torn writes (linear and non-linear) and reordered persistence at chosen points, and needs no changes to the system under test. It is integrated with Jepsen.
- It reproduced 12 known bugs and found 8 new ones, across PostgreSQL, etcd, ZooKeeper, Redis, LevelDB, PebblesDB and Lightning.
- PostgreSQL results:
  - The pgstat statistics file is never fsynced.
  - With **full_page_writes disabled**, a non-linear torn page write still lets PG restart, but `amcheck` then reports corruption. With it enabled, PG recovers.
- **For the spec:** two direct uses.
  1. **Run pgfs's backing Postgres on LazyFS in CI** (`ProvidesProjectLocalCluster` could mount the data directory through it). Then inject lost or torn writes under PG and check that pgfs plus PG recovery yields a state the reference model permits (r128). This is the crash-conformance test harness, and it is off the shelf.
  2. The cluster requirement should demand `full_page_writes = on`, `fsync = on` and data checksums (r14/r94/r120), and should run `amcheck` in `pgfs check`.
  Also note: pgfs *is* a FUSE filesystem, so the same tools that test PG (LazyFS, CrashMonkey r123, ALICE r118) can test applications running *on* pgfs. pgfs's fsync contract is what they would measure.

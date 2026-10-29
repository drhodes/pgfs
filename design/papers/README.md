# pgfs reference papers

Research collected to inform `design/sharded_filesystem.md` and pgfs in
general. 149 papers are listed: GFS, its 12 references, 41 hand-picked
papers (`r00`–`r40`) and 95 from a systematic sweep (`r41`–`r135`). 128 are
on disk; the other 21 are ACM papers with no free copy found.

- [`open-papers/`](open-papers/README.md): papers whose terms permit
  redistribution. Committed.
- [`closed-papers/`](closed-papers/README.md): everything else. Kept locally,
  ignored by git; each README lists sources so copies can be re-fetched.
- [`reading-notes.md`](reading-notes.md): notes on the papers actually read
  (GFS and its 12 references).

Numbering: unnumbered GFS file; `01`–`12` the GFS bibliography; `r00`–`r40`
landmark papers chosen by hand; `r41`–`r135` a systematic sweep (below).

## How the sweep was done (October 2026)

Programs of FAST 2019–2026, OSDI 2020–2025, USENIX ATC 2019–2025, SOSP 2021–2025
and EuroSys 2023–2025 were screened for papers on: distributed file systems
and metadata services, file systems built on databases, user-space file
systems, crash consistency and durability testing, replication and failover,
caching and leases, and gray/fail-slow/metastable failures. Classic papers
the earlier lists lacked (crash consistency, failure studies) and a few
VLDB/SIGMOD/SoCC/arXiv papers were added by targeted search. Papers on
persistent memory, SSD internals, deduplication and ML infrastructure were
excluded unless they bore directly on pgfs.

Known gaps: SOSP 2019 and EuroSys 2019–2022 and 2026 programs were not
screened (sites unreachable or lists unavailable); VLDB/SIGMOD were searched
by topic, not swept. MesaFS (EuroSys 2026) and SwitchFS (EuroSys 2026) were
found but no copy was available.

**Status of the reading:** only GFS and its bibliography have been read
(see `reading-notes.md`). The one-line descriptions below for everything
else come from titles, abstracts and general knowledge, not from reading
the papers.

## Core reading list

Ordered by how directly each paper bears on work pgfs has in front of it.
`[id]` is the file prefix.

### 1. Crash consistency and durability (Phase 0 bugs; v0.2 ordered writes)

| id | Paper | Why |
|---|---|---|
| r118 | *All File Systems Are Not Created Equal* (OSDI 2014) | What applications assume about write ordering and atomicity; pgfs must honour those assumptions. |
| r123 | *CrashMonkey* (OSDI 2018) | Systematic crash testing of file systems; a model for testing pgfs's write path. |
| r90 | *Can Applications Recover from fsync Failures?* (ATC 2020) | What `fsync` errors must mean; pgfs reports database failures as `EIO`. |
| r56 | *Consistency-Aware Durability* (FAST 2020) | The trade-off behind pgfs's `synchronous_commit = off` and v0.2's ordered writes. |
| r120 | *Torturing Databases for Fun and Profit* (OSDI 2014) | Databases losing data under power faults; pgfs inherits Postgres's durability settings. |
| r135 | *When Amnesia Strikes* (VLDB 2024) | Reproducing data-loss bugs with fault injection. |
| r96 | *Fawkes* (SOSP 2025) — not obtained | Finding durability bugs in DBMSs by checking recovered state. |
| r64 | *SquirrelFS* (OSDI 2024) | Using Rust's type system to enforce crash-consistent update ordering; pgfs is Rust. |

### 2. Metadata service design (v0.2 MDB; open question 5)

| id | Paper | Why |
|---|---|---|
| r91 | *Everyone Loves File: Oracle FSS* (ATC 2019) | A production file service whose metadata sits in a transactional, replicated store. |
| r36 | *SingularFS* (ATC 2023) | How far a single metadata server can scale; supports v0.2's single MDB. |
| r115 | *CFS* (EuroSys 2023) — not obtained | Shrinking critical sections in a metadata service at Baidu scale. |
| r131 | *Decoupling Directory Semantics from Metadata Indexing* (SoCC 2025) | Separating directory structure from the metadata index. |
| r127 | *Mantle* (SOSP 2025) | Hierarchical metadata for cloud object storage (Baidu). |
| r16 | *InfiniFS* (FAST 2022) | Decoupled access and content metadata; speculative path resolution. |
| r11, r38 | *HopsFS* (FAST 2017) and its small-files paper (Middleware 2018) | Metadata and small files in a relational database. |

### 3. Failure models (v0.2 §9)

| id | Paper | Why |
|---|---|---|
| r124 | *Gray Failure* (HotOS 2017) | Failures that monitors miss but clients feel. |
| r122 | *Fail-Slow at Scale* (FAST 2018) | Evidence that slow components are common. |
| r125, r67 | *Metastable Failures* (HotOS 2021) and *in the Wild* (OSDI 2022) | Retry and failover storms; relevant to reconnection design. |
| r121 | *Redundancy Does Not Imply Fault Tolerance* (FAST 2017) | How replicated stores mishandle single faults. |
| r94 | *Protocol-Aware Recovery* (FAST 2018) | Recovering replicated storage from corruption correctly. |
| r72 | *Copilots* (OSDI 2020) | Keeping replication fast when one replica is slow. |

### 4. User-space file systems (pgfs is a FUSE daemon)

| id | Paper | Why |
|---|---|---|
| r12 | *To FUSE or Not to FUSE* (FAST 2017) | Where FUSE overhead comes from. |
| r17 | *RFUSE* (FAST 2024) | Scalable kernel–user-space request queues. |
| r130 | *uFS: Filesystem Semi-Microkernel* (SOSP 2021) | A user-space file system that scales across cores. |
| r46 | *Ananke* (FAST 2025) | Transparent recovery of a user-space file system process. |
| r35 | *XFUSE* (ATC 2021) | Restarting and upgrading FUSE daemons without unmounting. |
| r53 | *Bento* (FAST 2021) | File systems written in Rust with live upgrade. |

### 5. Testing and verification (the libspec workflow; the bugs found)

| id | Paper | Why |
|---|---|---|
| r128 | *Lightweight Formal Methods for S3 ShardStore* (SOSP 2021) | Executable reference models checked against the implementation: practical for pgfs. |
| r43 | *Sharpen the Spec, Cut the Code: SYSSPEC* (FAST 2026) | Generating a file system from a specification; close to pgfs's libspec approach. |
| r119 | *Simple Testing Can Prevent Most Critical Failures* (OSDI 2014) | Most catastrophic failures come from untested error handling. |
| r81 | *Monarch* (ATC 2024) | Fuzzing distributed file systems. |
| r48 | *Metis* (FAST 2024) | Model checking file systems against a reference. |

### 6. Caching, coherence and copying

| id | Paper | Why |
|---|---|---|
| r59 | *Skybridge: Bounded Staleness for Distributed Caches* (OSDI 2025) | Bounded staleness, the guarantee `--attr-ttl` provides informally. |
| r47 | *Combining Buffered and Direct I/O in Distributed File Systems* (FAST 2024) | Client caching choices in a DFS. |
| r55 | *How to Copy Files* (FAST 2020) | Efficient copies; relevant to pgfs's offload features. |
| r41 | *Lockify* (FAST 2026) | Cost of distributed lock management over shared storage. |

### 7. Databases over shared storage (Postgres as the storage layer)

| id | Paper | Why |
|---|---|---|
| r126 | *Socrates* (SIGMOD 2019) | SQL Server with log and page storage split out. |
| r19 | *Aurora* (SIGMOD 2017) | Shipping redo to replicated storage. |
| r134 | *Disaggregation: A New Architecture for Cloud Databases* (VLDB 2025) | Survey of the design space. |
| r32 | *PolarFS* (VLDB 2018) | A file system built for a database primary and its replicas. |

Everything else is background; see the catalogue below and the
closed-papers README.

## Catalogue of the sweep (r41–r135)

`where`: open (committed), closed (local only), missing (no free copy found).

| id | Paper | Venue | Topic | where | Source |
|---|---|---|---|---|---|
| r41 | Lockify: Understanding Linux Distributed Lock Management Overheads in Shared Storage | FAST26 | locking | closed | <https://www.usenix.org/system/files/fast26-park.pdf> |
| r42 | CETOFS: A High-Performance File System with Host-Server Collaboration for Remote Storage | FAST26 | dfs | closed | <https://www.usenix.org/system/files/fast26-jia.pdf> |
| r43 | Sharpen the Spec, Cut the Code: A Case for Generative File System with SYSSPEC | FAST26 | spec-driven FS | closed | <https://www.usenix.org/system/files/fast26-liu-qingyuan.pdf> |
| r44 | Discard-Based Garbage Collection for Distributed Log-Structured Storage Systems in ByteDance | FAST26 | gc | closed | <https://www.usenix.org/system/files/fast26-bian.pdf> |
| r45 | CoFS: A Filesystem for Fast Container Startup | FAST26 | dfs | closed | <https://www.usenix.org/system/files/fast26-wang-li.pdf> |
| r46 | Fast, Transparent Filesystem Microkernel Recovery with Ananke | FAST25 | userspace-fs recovery | closed | <https://www.usenix.org/system/files/fast25-liu-jing.pdf> |
| r47 | Combining Buffered I/O and Direct I/O in Distributed File Systems | FAST24 | dfs caching | closed | <https://www.usenix.org/system/files/fast24-qian.pdf> |
| r48 | Metis: File System Model Checking via Versatile Input and State Exploration | FAST24 | testing | closed | <https://www.usenix.org/system/files/fast24-liu-yifei.pdf> |
| r49 | IONIA: High-Performance Replication for Modern Disk-based KV Stores | FAST24 | replication | closed | <https://www.usenix.org/system/files/fast24-xu.pdf> |
| r50 | Fisc: A Large-scale Cloud-native-oriented File System | FAST23 | dfs | closed | <https://www.usenix.org/system/files/fast23-li-qiang.pdf> |
| r51 | More Than Capacity: Performance-oriented Evolution of Pangu in Alibaba | FAST23 | dfs | closed | <https://www.usenix.org/system/files/fast23-li-qiang_more.pdf> |
| r52 | Unsafe at Any Copy: Name Collisions from Mixing Case Sensitivities | FAST23 | fs semantics | closed | <https://www.usenix.org/system/files/fast23-basu.pdf> |
| r53 | High Velocity Kernel File Systems with Bento | FAST21 | rust fs | closed | <https://www.usenix.org/system/files/fast21-miller.pdf> |
| r54 | pFSCK: Accelerating File System Checking and Repair for Modern Storage | FAST21 | fsck | closed | <https://www.usenix.org/system/files/fast21-domingo.pdf> |
| r55 | How to Copy Files | FAST20 | copy/offload | closed | <https://www.usenix.org/system/files/fast20-zhan.pdf> |
| r56 | Strong and Efficient Consistency with Consistency-Aware Durability | FAST20 | durability | closed | <https://www.usenix.org/system/files/fast20-ganesan.pdf> |
| r57 | ScaleCheck: A Single-Machine Approach for Discovering Scalability Bugs in Large Distributed Systems | FAST19 | testing | closed | <https://www.usenix.org/system/files/fast19-stuardo.pdf> |
| r58 | Deriving Semantic Checkers from Tests to Detect Silent Failures in Production Distributed Systems | OSDI25 | testing | closed | <https://www.usenix.org/system/files/osdi25-lou.pdf> |
| r59 | Skybridge: Bounded Staleness for Distributed Caches | OSDI25 | caching | closed | <https://www.usenix.org/system/files/osdi25-lyerly.pdf> |
| r60 | PoWER Never Corrupts: Tool-Agnostic Verification of Crash Consistency and Corruption Detection | OSDI25 | crash consistency | closed | <https://www.usenix.org/system/files/osdi25-leblanc.pdf> |
| r61 | Fast and Synchronous Crash Consistency with Metadata Write-Once File System | OSDI25 | crash consistency | closed | <https://www.usenix.org/system/files/osdi25-pan.pdf> |
| r62 | Okapi: Decoupling Data Striping and Redundancy Grouping in Cluster File Systems | OSDI25 | dfs placement | closed | <https://www.usenix.org/system/files/osdi25-athlur.pdf> |
| r63 | Low End-to-End Latency atop a Speculative Shared Log with Fix-Ante Ordering | OSDI25 | replication | closed | <https://www.usenix.org/system/files/osdi25-bhat.pdf> |
| r64 | SquirrelFS: using the Rust compiler to check file-system crash consistency | OSDI24 | crash consistency rust | closed | <https://www.usenix.org/system/files/osdi24-leblanc.pdf> |
| r65 | Using Dynamically Layered Definite Releases for Verifying the RefFS File System | OSDI24 | verification | closed | <https://www.usenix.org/system/files/osdi24-zou.pdf> |
| r66 | Chardonnay: Fast and General Datacenter Transactions for On-Disk Databases | OSDI23 | transactions | closed | <https://www.usenix.org/system/files/osdi23-eldeeb.pdf> |
| r67 | Metastable Failures in the Wild | OSDI22 | failures | closed | <https://www.usenix.org/system/files/osdi22-huang-lexiang.pdf> |
| r68 | Verifying the DaisyNFS concurrent and crash-safe file system with sequential reasoning | OSDI22 | verification | closed | <https://www.usenix.org/system/files/osdi22-chajed.pdf> |
| r69 | GoJournal: a verified, concurrent, crash-safe journaling system | OSDI21 | verification | closed | <https://www.usenix.org/system/files/osdi21-chajed.pdf> |
| r70 | Storage Systems are Distributed Systems (So Verify Them That Way!) | OSDI20 | verification | closed | <https://www.usenix.org/system/files/osdi20-hance.pdf> |
| r71 | Toward a Generic Fault Tolerance Technique for Partial Network Partitioning | OSDI20 | failures | closed | <https://www.usenix.org/system/files/osdi20-alfatafta.pdf> |
| r72 | Tolerating Slowdowns in Replicated State Machines using Copilots | OSDI20 | fail-slow replication | closed | <https://www.usenix.org/system/files/osdi20-ngo.pdf> |
| r73 | Virtual Consensus in Delos | OSDI20 | replication | closed | <https://www.usenix.org/system/files/osdi20-balakrishnan.pdf> |
| r74 | Assise: Performance and Availability via Client-local NVM in a Distributed File System | OSDI20 | dfs caching | closed | <https://www.usenix.org/system/files/osdi20-anderson.pdf> |
| r75 | FlightTracker: Consistency across Read-Optimized Online Stores at Facebook | OSDI20 | consistency/caching | closed | <https://www.usenix.org/system/files/osdi20-shi.pdf> |
| r76 | Testing Database Engines via Pivoted Query Synthesis | OSDI20 | testing | closed | <https://www.usenix.org/system/files/osdi20-rigger.pdf> |
| r77 | Crash Consistency in Block-Level Caching Systems: An Open CAS Case Study | ATC25 | crash consistency | closed | <https://www.usenix.org/system/files/atc25-duan-shaohua.pdf> |
| r78 | FiDe: Reliable and Fast Crash Failure Detection to Boost Datacenter Coordination | ATC25 | failure detection | closed | <https://www.usenix.org/system/files/atc25-rovelli.pdf> |
| r79 | Understanding and Detecting Fail-Slow Hardware Failure Bugs in Cloud Systems | ATC25 | fail-slow | closed | <https://www.usenix.org/system/files/atc25-dong.pdf> |
| r80 | CAFault: Enhance Fault Injection Technique in Practical Distributed Systems via Abundant Fault-Dependent Configurations | ATC25 | testing | closed | <https://www.usenix.org/system/files/atc25-chen-yuanliang.pdf> |
| r81 | Monarch: A Fuzzing Framework for Distributed File Systems | ATC24 | testing dfs | closed | <https://www.usenix.org/system/files/atc24-lyu.pdf> |
| r82 | Distributed Transactions at Scale in Amazon DynamoDB | ATC23 | transactions | closed | <https://www.usenix.org/system/files/atc23-idziorek.pdf> |
| r83 | RubbleDB: CPU-Efficient Replication with NVMe-oF | ATC23 | replication | closed | <https://www.usenix.org/system/files/atc23-li-haoyu.pdf> |
| r84 | Explore Data Placement Algorithm for Balanced Recovery Load Distribution | ATC23 | placement/recovery | closed | <https://www.usenix.org/system/files/atc23-shan.pdf> |
| r85 | Speculative Recovery: Cheap, Highly Available Fault Tolerance with Disaggregated Storage | ATC22 | failover | closed | <https://www.usenix.org/system/files/atc22-li-nanqinqin.pdf> |
| r86 | Modulo: Finding Convergence Failure Bugs in Distributed Systems with Divergence Resync Models | ATC22 | testing replication | closed | <https://www.usenix.org/system/files/atc22-kim-beom-heyn.pdf> |
| r87 | DepFast: Orchestrating Code of Quorum Systems | ATC22 | fail-slow quorum | closed | <https://www.usenix.org/system/files/atc22-luo.pdf> |
| r88 | NVMe SSD Failures in the Field: the Fail-Stop and the Fail-Slow | ATC22 | fail-slow | closed | <https://www.usenix.org/system/files/atc22-lu.pdf> |
| r89 | LODIC: Logical Distributed Counting for Scalable File Access | ATC21 | metadata | closed | <https://www.usenix.org/system/files/atc21-park.pdf> |
| r90 | Can Applications Recover from fsync Failures? | ATC20 | durability | closed | <https://www.usenix.org/system/files/atc20-rebello.pdf> |
| r91 | Everyone Loves File: File Storage Service (FSS) in Oracle Cloud Infrastructure | ATC19 | dfs (transactional metadata) | closed | <https://www.usenix.org/system/files/atc19-kuszmaul.pdf> |
| r92 | IASO: A Fail-Slow Detection and Mitigation Framework for Distributed Storage Services | ATC19 | fail-slow | closed | <https://www.usenix.org/system/files/atc19-panda_0.pdf> |
| r93 | FlexGroup Volumes: A Distributed WAFL File System | ATC19 | dfs | closed | <https://www.usenix.org/system/files/atc19-kesavan.pdf> |
| r94 | Protocol-Aware Recovery for Consensus-Based Storage | FAST18 | replication/corruption | closed | <https://www.usenix.org/system/files/conference/fast18/fast18-alagappan.pdf> |
| r95 | Dayu: Fast and Low-interference Data Recovery in Very-large Storage Systems | ATC19 | re-replication | closed | <https://www.usenix.org/system/files/atc19-wang_0.pdf> |
| r96 | Fawkes: Finding Data Durability Bugs in DBMSs via Recovered Data State Verification | SOSP25 | durability testing | missing | ACM DL only (no free copy found) |
| r97 | cache_ext: Customizing the Page Cache with eBPF | SOSP25 | caching | missing | ACM DL only (no free copy found) |
| r98 | Aeolia: A Fast and Secure Userspace Interrupt-Based Storage Stack | SOSP25 | userspace storage | missing | ACM DL only (no free copy found) |
| r99 | Optimistic Recovery for High-Availability Software via Partial Process State Preservation | SOSP25 | recovery | missing | ACM DL only (no free copy found) |
| r100 | Efficient file-lifetime redundancy management for cluster file systems | SOSP24 | dfs redundancy | closed | <https://www.pdl.cmu.edu/PDL-FTP/Storage/Morph_Final.pdf> |
| r101 | Efficient Reproduction of Fault-Induced Failures in Distributed Systems with Feedback-Driven Fault Injection | SOSP24 | testing | missing | ACM DL only (no free copy found) |
| r102 | LazyLog: A New Shared Log Abstraction for Low-Latency Applications | SOSP24 | replication | missing | ACM DL only (no free copy found) |
| r103 | Enabling High-Performance and Secure Userspace NVM File Systems with the Trio Architecture | SOSP23 | userspace fs | missing | ACM DL only (no free copy found) |
| r104 | Acto: Automatic End-to-End Testing for Operation Correctness of Cloud System Management | SOSP23 | testing | missing | ACM DL only (no free copy found) |
| r105 | QuePaxa: Escaping the Tyranny of Timeouts in Consensus | SOSP23 | consensus | missing | ACM DL only (no free copy found) |
| r106 | LineFS: Efficient SmartNIC Offload of a Distributed File System with Pipeline Parallelism | SOSP21 | dfs | missing | ACM DL only (no free copy found) |
| r107 | Understanding and Detecting Software Upgrade Failures in Distributed Systems | SOSP21 | failures | missing | ACM DL only (no free copy found) |
| r108 | Shard Manager: A Generic Shard Management Framework for Geo-distributed Applications | SOSP21 | sharding | missing | ACM DL only (no free copy found) |
| r109 | Log-structured Protocols in Delos | SOSP21 | replication | missing | ACM DL only (no free copy found) |
| r110 | Cheetah: Metadata Aggregation for Fast Object Storage without Distributed Ordering | EUROSYS25 | metadata | missing | ACM DL only (no free copy found) |
| r111 | Themis: Finding Imbalance Failures in Distributed File Systems via a Load Variance Model | EUROSYS25 | testing dfs | missing | ACM DL only (no free copy found) |
| r112 | Validating Database System Isolation Level Implementations with Version Certificate Recovery | EUROSYS24 | testing db | missing | ACM DL only (no free copy found) |
| r113 | Volley: Accelerating Write-Read Orders in Disaggregated Storage | EUROSYS24 | ordering | missing | ACM DL only (no free copy found) |
| r114 | SplitFT: Fault Tolerance for Disaggregated Datacenters via Remote Memory Logging | EUROSYS24 | fault tolerance | missing | ACM DL only (no free copy found) |
| r115 | CFS: Scaling Metadata Service for Distributed File System via Pruned Scope of Critical Sections | EUROSYS23 | metadata | missing | ACM DL only (no free copy found) |
| r116 | Omni-Paxos: Breaking the Barriers of Partial Connectivity | EUROSYS23 | consensus | missing | ACM DL only (no free copy found) |
| r117 | Fail through the Cracks: Cross-System Interaction Failures in Modern Cloud Systems | EUROSYS23 | failures | missing | ACM DL only (no free copy found) |
| r118 | All File Systems Are Not Created Equal: On the Complexity of Crafting Crash-Consistent Applications | OSDI14 | crash consistency | closed | <https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf> |
| r119 | Simple Testing Can Prevent Most Critical Failures | OSDI14 | testing | closed | <https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-yuan.pdf> |
| r120 | Torturing Databases for Fun and Profit | OSDI14 | durability testing | closed | <https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-zheng_mai.pdf> |
| r121 | Redundancy Does Not Imply Fault Tolerance: Analysis of Distributed Storage Reactions to Single Errors and Corruptions | FAST17 | failures | closed | <https://www.usenix.org/system/files/conference/fast17/fast17-ganesan.pdf> |
| r122 | Fail-Slow at Scale: Evidence of Hardware Performance Faults in Large Production Systems | FAST18 | fail-slow | closed | <https://www.usenix.org/system/files/conference/fast18/fast18-gunawi.pdf> |
| r123 | Finding Crash-Consistency Bugs with Bounded Black-Box Crash Testing (CrashMonkey) | OSDI18 | crash consistency | closed | <https://www.usenix.org/system/files/osdi18-mohan.pdf> |
| r124 | Gray Failure: The Achilles' Heel of Cloud-Scale Systems | HOTOS17 | gray failure | closed | <https://www.microsoft.com/en-us/research/wp-content/uploads/2017/06/paper-1.pdf> |
| r125 | Metastable Failures in Distributed Systems | HOTOS21 | failures | closed | <https://sigops.org/s/conferences/hotos/2021/papers/hotos21-s11-bronson.pdf> |
| r126 | Socrates: The New SQL Server in the Cloud | SIGMOD19 | disagg-db | closed | <https://www.microsoft.com/en-us/research/uploads/prod/2019/05/socrates.pdf> |
| r127 | Mantle: Efficient Hierarchical Metadata Management for Cloud Object Storage Services | SOSP25 | metadata | closed | <https://madsys.cs.tsinghua.edu.cn/publication/mantle-efficient-hierarchical-metadata-management-for-cloud-object-storage-services/SOSP25-Li.pdf> |
| r128 | Using Lightweight Formal Methods to Validate a Key-Value Storage Node in Amazon S3 | SOSP21 | verification | closed | <https://www.cs.utexas.edu/~bornholt/papers/shardstore-sosp21.pdf> |
| r129 | Exploiting Nil-Externality for Fast Replicated Storage | SOSP21 | replication | closed | <https://par.nsf.gov/servlets/purl/10299722> |
| r130 | Scale and Performance in a Filesystem Semi-Microkernel | SOSP21 | userspace fs | closed | <https://research.cs.wisc.edu/adsl/Publications/ufs-sosp21.pdf> |
| r131 | Accelerating Distributed Filesystem Metadata Service via Decoupling Directory Semantics from Metadata Indexing | SOCC25 | metadata | open | <https://raw.githubusercontent.com/kurodes/SoCC25-HMFS/main/Accelerating_Distributed_Filesystem_Metadata_Service_via_Decoupling_Directory_Semantics_from_Metadata_Indexing.pdf> |
| r132 | Fletch: File-System Metadata Caching in Programmable Switches | ARXIV25 | metadata caching | open | <https://arxiv.org/pdf/2510.08351> |
| r133 | SwitchDelta: Asynchronous Metadata Updating for Distributed Storage with In-Network Data Visibility | ICDE26 | metadata | closed | <https://arxiv.org/pdf/2511.19978> |
| r134 | Disaggregation: A New Architecture for Cloud Databases | VLDB25 | disagg-db | open | <https://www.vldb.org/pvldb/vol18/p5527-xiangyao.pdf> |
| r135 | When Amnesia Strikes: Understanding and Reproducing Data Loss Bugs with Fault Injection | VLDB24 | durability testing | open | <https://www.vldb.org/pvldb/vol17/p3017-ramos.pdf> |

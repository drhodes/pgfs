# Closed papers

The PDFs for these papers are kept locally in this directory but are not
committed: their copyright notices do not permit redistribution, or they
carry no license at all. `.gitignore` excludes `*.pdf` here. Each one is
free to read at the source listed; download it into this directory to
restore a local copy.

The numbers match the bibliography of the GFS paper.

| # | Paper | Why closed | Source |
|---|---|---|---|
| — | Ghemawat, Gobioff, Leung. *The Google File System*. SOSP 2003. | ACM copyright: personal or classroom use only | <https://static.googleusercontent.com/media/research.google.com/en//archive/gfs-sosp2003.pdf> |
| 1 | Anderson, Dahlin, Neefe, Patterson, Roselli, Wang. *Serverless Network File Systems* (xFS). SOSP 1995. | ACM copyright: no posting or redistribution | <https://www.cs.columbia.edu/~nieh/teaching/e6118_s00/papers/p109-anderson.pdf> |
| 2 | Arpaci-Dusseau et al. *Cluster I/O with River: Making the Fast Case Common*. IOPADS 1999. | Published by ACM; no license in the PDF | <https://ftp.eecs.berkeley.edu/~yelick/papers/river.pdf> |
| 3 | Cabrera, Long. *Swift: Using Distributed Disk Striping to Provide High I/O Data Rates*. Computing Systems 4(4), 1991 (UCSC-CRL-91-46). | No license in the PDF | <https://tr.soe.ucsc.edu/sites/default/files/technical-reports/UCSC-CRL-91-46.pdf> |
| 4 | Gibson et al. *A Cost-Effective, High-Bandwidth Storage Architecture* (NASD). ASPLOS 1998. | ACM copyright: no posting or redistribution | <https://www.cs.cmu.edu/afs/cs.cmu.edu/academic/class/15712-s12/www/papers/gibson98.pdf> |
| 5 | Howard et al. *Scale and Performance in a Distributed File System* (AFS). ACM TOCS 6(1), 1988. | ACM copyright: republishing needs permission | <https://research.cs.wisc.edu/areas/os/Qual/papers/afs.pdf> |
| 6 | Braam, Nelson. *Removing Bottlenecks in Distributed Filesystems: Coda & InterMezzo as Examples*. Linux Expo 1999. Stands in for the GFS citation of the InterMezzo website. | No license in the PDF | <https://www.cs.cmu.edu/afs/cs/project/coda-www/ResearchWebPages/docdir/linuxexpo99.pdf> |
| 7 | Liskov, Ghemawat, Gruber, Johnson, Shrira, Williams. *Replication in the Harp File System*. SOSP 1991. | Published by ACM; no license in the PDF | <https://pdos.csail.mit.edu/6.824-2004/papers/harp.pdf> |
| 8 | Cluster File Systems, Inc. *Lustre: A Scalable, High-Performance File System*. Whitepaper, 2002. Stands in for the GFS citation of the Lustre website. | Corporate document; no license (GPL covers the software only) | <https://cse.buffalo.edu/faculty/tkosar/cse710/papers/lustre-whitepaper.pdf> |
| 9 | Patterson, Gibson, Katz. *A Case for Redundant Arrays of Inexpensive Disks (RAID)*. SIGMOD 1988. | ACM copyright: republishing needs permission | <https://web.mit.edu/6.033/2015/wwwdocs/papers/Patterson88.pdf> |
| 12 | Thekkath, Mann, Lee. *Frangipani: A Scalable Distributed File System*. SOSP 1997. | ACM copyright: no posting or redistribution | <https://www.cs.bu.edu/~jappavoo/jappavoo.github.com/451/papers/thekkath-frangipani.pdf> |

References 10 (GPFS) and 11 (the Global File System) permit redistribution
and are committed in [`../open-papers/`](../open-papers/).

## Further papers (recent work and foundations)

Papers beyond the GFS bibliography that bear on `design/sharded_filesystem.md`:
later systems, and earlier foundations the design relies on. Files are
prefixed `rNN-`. None carries a license permitting redistribution: USENIX
papers are free to read but carry no redistribution grant, ACM and IEEE
papers are copyrighted by the publisher, and the two arXiv papers use
arXiv's default non-exclusive license, which lets only arXiv distribute
them.

| # | Paper | Why it matters for pgfs | Source |
|---|---|---|---|
| r00 | Olson. *The Design and Implementation of the Inversion File System*. USENIX Winter 1993. | A file system built inside POSTGRES, using its transactions and time travel: the closest ancestor of pgfs. | <https://dsf.berkeley.edu/papers/S2K-93-28.pdf> (fetched via the Wayback Machine) |
| r01 | van Renesse, Schneider. *Chain Replication for Supporting High Throughput and Availability*. OSDI 2004. | Alternative to primary-backup for data sets: strong consistency with reads from the tail. | <https://www.usenix.org/legacy/event/osdi04/tech/full_papers/renesse/renesse.pdf> |
| r02 | Weil et al. *Ceph: A Scalable, High-Performance Distributed File System*. OSDI 2006. | Metadata cluster separate from object storage, as in design v0.2; dynamic subtree partitioning of metadata. | <https://www.usenix.org/legacy/event/osdi06/tech/full_papers/weil/weil.pdf> |
| r03 | Weil et al. *CRUSH: Controlled, Scalable, Decentralized Placement of Replicated Data*. SC 2006. | The strongest case for computed placement, which v0.2 rejects in favour of a table; read before revisiting. | <https://ceph.io/assets/pdfs/weil-crush-sc06.pdf> |
| r04 | Burrows. *The Chubby Lock Service for Loosely-Coupled Distributed Systems*. OSDI 2006. | Leases, sequencers (fencing tokens) and cache invalidation in practice. | <https://static.googleusercontent.com/media/research.google.com/en//archive/chubby-osdi06.pdf> |
| r05 | Weil et al. *RADOS: A Scalable, Reliable Storage Service for Petabyte-scale Storage Clusters*. PDSW 2007. | Placement groups, cluster maps with epochs, and peering for recovery. | <https://ceph.io/assets/pdfs/weil-rados-pdsw07.pdf> |
| r06 | Terrace, Freedman. *Object Storage on CRAQ*. USENIX ATC 2009. | Chain replication that serves reads from every replica: relevant to Phase 4 standby reads. | <https://www.usenix.org/legacy/event/usenix09/tech/full_papers/terrace/terrace.pdf> |
| r07 | Ren, Gibson. *TableFS: Enhancing Metadata Efficiency in the Local File System*. USENIX ATC 2013. | File system metadata stored in a key-value table (LevelDB), with small files inline: informs the inline threshold question. | <https://www.usenix.org/system/files/conference/atc13/atc13-ren.pdf> |
| r08 | Ongaro, Ousterhout. *In Search of an Understandable Consensus Algorithm* (Raft). USENIX ATC 2014. | Consensus behind the failover tooling (Patroni's etcd) that v0.2 delegates to. | <https://www.usenix.org/system/files/conference/atc14/atc14-paper-ongaro.pdf> |
| r09 | Ren et al. *IndexFS: Scaling File System Metadata Performance with Stateless Caching and Bulk Insertion*. SC 2014. | Partitioned metadata with client lease caching: a path for open question 5. | <https://www.pdl.cmu.edu/PDL-FTP/FS/IndexFS-SC14.pdf> |
| r10 | Thomson, Abadi. *CalvinFS: Consistent WAN Replication and Scalable Metadata Management for Distributed File Systems*. FAST 2015. | File system metadata in a distributed transactional database. | <https://www.usenix.org/system/files/conference/fast15/fast15-paper-thomson.pdf> |
| r11 | Niazi et al. *HopsFS: Scaling Hierarchical File System Metadata Using NewSQL Databases*. FAST 2017. | HDFS metadata moved into a relational database (MySQL Cluster): the most direct precedent for pgfs's metadata design, including lock ordering and partitioning by parent. | <https://www.usenix.org/system/files/conference/fast17/fast17-niazi.pdf> |
| r12 | Vangoor, Tarasov, Zadok. *To FUSE or Not to FUSE: Performance of User-Space File Systems*. FAST 2017. | Measured FUSE overheads and which options (writeback cache, splicing, big writes) matter. | <https://www.usenix.org/system/files/conference/fast17/fast17-vangoor.pdf> |
| r13 | Bijlani, Ramachandran. *Extension Framework for File Systems in User Space* (ExtFUSE). USENIX ATC 2019. | Serving metadata requests in the kernel with eBPF to avoid user-space round trips. | <https://www.usenix.org/system/files/atc19-bijlani.pdf> |
| r14 | Pan et al. *Facebook's Tectonic Filesystem: Efficiency from Exascale*. FAST 2021. | Metadata layered over a sharded key-value store; separates name, file and block layers. | <https://www.usenix.org/system/files/fast21-pan.pdf> |
| r15 | Zhou et al. *FoundationDB: A Distributed Unbundled Transactional Key Value Store*. SIGMOD 2021. | The transactional store several modern file systems (3FS, JuiceFS option) put metadata in. | <https://www.foundationdb.org/files/fdb-paper.pdf> |
| r16 | Lv et al. *InfiniFS: An Efficient Metadata Service for Large-Scale Distributed Filesystems*. FAST 2022. | Decouples access and content metadata and speculates path resolution: techniques for a metadata bottleneck. | <https://www.usenix.org/system/files/fast22-lv.pdf> |
| r17 | Cho et al. *RFUSE: Modernizing Userspace Filesystem Framework through Scalable Kernel-Userspace Communication*. FAST 2024. | Per-core FUSE request queues; pgfs is single-threaded today. | <https://www.usenix.org/system/files/fast24-cho.pdf> |
| r18 | An et al. *Fire-Flyer AI-HPC: A Cost-Effective Software-Hardware Co-Design for Deep Learning*. SC 2024 (arXiv:2408.14158). | Describes 3FS: metadata in FoundationDB, data on chain-replicated storage. | <https://arxiv.org/abs/2408.14158> |
| r19 | Verbitski et al. *Amazon Aurora: Design Considerations for High Throughput Cloud-Native Relational Databases*. SIGMOD 2017. | Replicating a relational database by shipping redo to quorum storage: an alternative to streaming replication per data set. | <https://www.cs.purdue.edu/homes/csjgwang/CS592DisaggregatedDB/AuroraSIGMOD17.pdf> |
| r20 | Carver et al. *λFS: A Scalable and Elastic Distributed File System Metadata Service using Serverless Functions*. ASPLOS 2023 (arXiv:2306.11877). | Elastic metadata caching in front of HopsFS's database. | <https://arxiv.org/abs/2306.11877> |

| r21 | Gray, Cheriton. *Leases: An Efficient Fault-Tolerant Mechanism for Distributed File Cache Consistency*. SOSP 1989. | The original leases paper; basis for client leases in design §11.3. | <https://www.eecg.utoronto.ca/~yuan/teaching/gradOS/papers/89-leases.pdf> |
| r22 | Rosenblum, Ousterhout. *The Design and Implementation of a Log-Structured File System*. SOSP 1991. | The log structure xFS and Zebra build on; segment cleaning as garbage collection. | <https://people.eecs.berkeley.edu/~brewer/cs262/LFS.pdf> |
| r23 | Lamport. *Paxos Made Simple*. 2001. | The consensus algorithm behind Frangipani's lock service and Chubby. | <https://lamport.azurewebsites.net/pubs/paxos-simple.pdf> |
| r24 | Hunt et al. *ZooKeeper: Wait-free Coordination for Internet-scale Systems*. USENIX ATC 2010. | Coordination service used by Patroni-style failover; watches as invalidation. | <https://www.usenix.org/legacy/event/atc10/tech/full_papers/Hunt.pdf> |
| r25 | Shvachko et al. *The Hadoop Distributed File System*. MSST 2010. | Open-source GFS design; single NameNode with in-memory metadata, the limit HopsFS removes. | <https://pages.cs.wisc.edu/~akella/CS838/F15/838-CloudPapers/hdfs.pdf> |
| r26 | Beaver et al. *Finding a Needle in Haystack: Facebook's Photo Storage*. OSDI 2010. | Packing many small objects into large files to cut metadata per object. | <https://www.usenix.org/legacy/event/osdi10/tech/full_papers/Beaver.pdf> |
| r27 | Calder et al. *Windows Azure Storage: A Highly Available Cloud Storage Service with Strong Consistency*. SOSP 2011. | Partition map plus stream layer: a production metadata/data split with strong consistency. | <https://sigops.org/s/conferences/sosp/2011/current/2011-Cascais/printable/11-calder.pdf> |
| r28 | Liskov, Cowling. *Viewstamped Replication Revisited*. MIT-CSAIL-TR-2012-021. | Modern statement of the protocol Harp uses, including reconfiguration. | <https://www.cs.princeton.edu/courses/archive/fall19/cos418/papers/vr-revisited.pdf> |
| r29 | Huang et al. *Erasure Coding in Windows Azure Storage*. USENIX ATC 2012. | Local reconstruction codes; reference for the erasure-coding non-goal. | <https://www.usenix.org/system/files/conference/atc12/atc12-final181_0.pdf> |
| r30 | Corbett et al. *Spanner: Google's Globally-Distributed Database*. OSDI 2012. | Distributed SQL with Paxos groups per shard; background for metadata scale-out. | <https://www.usenix.org/system/files/conference/osdi12/osdi12-final-16.pdf> |
| r31 | Muralidhar et al. *f4: Facebook's Warm BLOB Storage System*. OSDI 2014. | Moving cold data from replication to erasure coding. | <https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-muralidhar.pdf> |
| r33 | Aghayev et al. *File Systems Unfit as Distributed Storage Backends: Lessons from 10 Years of Ceph Evolution*. SOSP 2019. | Why Ceph stopped storing objects on a general-purpose file system; a caution for storing blocks in a general-purpose database. | <https://www.pdl.cmu.edu/PDL-FTP/Storage/ceph-exp-sosp19.pdf> |
| r35 | Huai et al. *XFUSE: An Infrastructure for Running Filesystem Services in User Space*. USENIX ATC 2021. | Making FUSE daemons highly available and upgradable without unmounting. | <https://www.usenix.org/system/files/atc21-huai.pdf> |
| r36 | Guo et al. *SingularFS: A Billion-Scale Distributed File System Using a Single Metadata Server*. USENIX ATC 2023. | How far one metadata server can go: supports design v0.2's single MDB. | <https://www.usenix.org/system/files/atc23-guo.pdf> |
| r37 | Lee, Thekkath. *Petal: Distributed Virtual Disks*. ASPLOS 1996. | The replicated storage layer under Frangipani; chained declustering. | <https://pages.cs.wisc.edu/~remzi/Classes/739/Fall2016/Papers/petal96.pdf> |
| r38 | Niazi et al. *Size Matters: Improving the Performance of Small Files in Hadoop*. Middleware 2018 (submitted version). | Small files stored inline in the metadata database: directly informs design open question 2. | <https://content.hopsworks.ai/hubfs/research/size-matters-improving-the-performance-of-small-files-in-hadoop.pdf> |
| r39 | Cubukcu et al. *Citus: Distributed PostgreSQL for Data-Intensive Applications*. SIGMOD 2021. | Sharding PostgreSQL itself: one route to metadata scale-out without leaving Postgres. | <https://paper-notes.zhjwpku.com/assets/pdfs/citus.pdf> (third-party copy) |
| r40 | Nelson, Welch, Ousterhout. *Caching in the Sprite Network File System*. ACM TOCS 6(1), 1988. | Server-tracked open files and disabling caching on write sharing. | <https://pdos.csail.mit.edu/archive/6.824-2002/papers/sprite-caching.pdf> |

r32 (PolarFS) and r34 (TiDB) are CC BY-NC-ND and committed in
[`../open-papers/`](../open-papers/README.md).

Not found as a free copy: Wang et al., *CFS: Scaling Metadata Service for
Distributed File System via Pruned Scope of Critical Sections*, EuroSys
2023 (ACM Digital Library only).

## Notes on the local copies

- `gfs-sosp2003.pdf` is Google's original file. Its page index is damaged,
  so some viewers show only 10 of its 15 pages. `gfs-sosp2003-full.pdf` is
  the MIT 6.824 copy rewritten with `qpdf` and shows all 15.
- The other PDFs were also rewritten with `qpdf` (`qpdf in.pdf out.pdf`) to
  repair similar index damage. Their content is unchanged.
- The Swift report came from the Wayback Machine because the UCSC server
  was unreachable.
- xFS is a scan with poor text recognition: it reads fine, but searching
  its text gives garbled results.

## Sweep papers (r41–r135)

The 70 closed papers from the systematic sweep are catalogued, with
sources, in [`../README.md`](../README.md#catalogue-of-the-sweep-r41r135).
All are USENIX papers without a redistribution grant or ACM/IEEE papers
under publisher copyright.

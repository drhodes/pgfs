# Open papers

These papers permit redistribution and are committed here. The numbers
match the bibliography of the GFS paper; the rest of that bibliography is
listed in [`../closed-papers/`](../closed-papers/README.md).

| # | Paper | Terms | Source |
|---|---|---|---|
| 10 | Schmuck, Haskin. *GPFS: A Shared-Disk File System for Large Computing Clusters*. FAST 2002. | USENIX: "Permission is granted for noncommercial reproduction of the work for educational or research purposes. This copyright notice must be included in the reproduced paper." The notice is on the PDF's first page. | <https://www.usenix.org/events/fast02/full_papers/schmuck/schmuck.pdf> |
| 11 | Soltis, Ruwart, O'Keefe. *The Global File System*. Fifth NASA Goddard Conference on Mass Storage Systems and Technologies, 1996. | NASA Technical Reports Server record 19960052744: "public use permitted", public distribution. | <https://ntrs.nasa.gov/citations/19960052744> |

Both PDFs were rewritten with `qpdf` to repair a damaged page index; their
content is unchanged.

## Recent papers

Numbering continues the recent-papers list in
[`../closed-papers/README.md`](../closed-papers/README.md). These two are
published by VLDB under Creative Commons BY-NC-ND 4.0, which permits
noncommercial redistribution of the unmodified work with attribution, so
they are the publisher's original files, not repaired copies.

| # | Paper | Terms | Source |
|---|---|---|---|
| r32 | Cao et al. *PolarFS: An Ultra-low Latency and Failure Resilient Distributed File System for Shared Storage Cloud Database*. PVLDB 11(12), 2018. | CC BY-NC-ND 4.0 (notice on first page) | <https://www.vldb.org/pvldb/vol11/p1849-cao.pdf> |
| r34 | Huang et al. *TiDB: A Raft-based HTAP Database*. PVLDB 13(12), 2020. | CC BY-NC-ND 4.0 (notice on first page) | <https://www.vldb.org/pvldb/vol13/p3072-huang.pdf> |

### From the sweep

These four are also the publisher's unmodified files. See
[`../README.md`](../README.md) for the reading guide.

| # | Paper | Terms | Source |
|---|---|---|---|
| r131 | Decoupling Directory Semantics from Metadata Indexing (HMFS). SoCC 2025. | CC BY 4.0 (notice in the paper) | <https://raw.githubusercontent.com/kurodes/SoCC25-HMFS/main/Accelerating_Distributed_Filesystem_Metadata_Service_via_Decoupling_Directory_Semantics_from_Metadata_Indexing.pdf> |
| r132 | *Fletch: File-System Metadata Caching in Programmable Switches*. arXiv:2510.08351, 2025. | CC BY 4.0 (arXiv license) | <https://arxiv.org/abs/2510.08351> |
| r134 | Yu. *Disaggregation: A New Architecture for Cloud Databases*. PVLDB 18(12), 2025. | CC BY-NC-ND 4.0 (notice on first page) | <https://www.vldb.org/pvldb/vol18/p5527-xiangyao.pdf> |
| r135 | Ramos et al. *When Amnesia Strikes: Understanding and Reproducing Data Loss Bugs with Fault Injection*. PVLDB 17, 2024. | CC BY-NC-ND 4.0 (notice on first page) | <https://www.vldb.org/pvldb/vol17/p3017-ramos.pdf> |

PolarFS shares one replicated block store among a database primary and
its read replicas (relevant to the MDB and its standby). TiDB is a
distributed SQL database built on Raft-replicated key ranges: a candidate
for metadata scale-out (design open question 5).

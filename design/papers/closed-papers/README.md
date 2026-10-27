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

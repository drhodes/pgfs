# pgfs — PostgreSQL-Backed FUSE Filesystem

Welcome to the **pgfs** documentation site, structured according to the [Diátaxis](https://diataxis.fr) framework.

`pgfs` is a high-performance, POSIX-compliant FUSE filesystem backed natively by a PostgreSQL database engine.

---

## 🧭 Documentation Structure

The documentation is organized across four distinct quadrants:

<div class="grid cards" markdown>

-   :material-school: **Tutorials**
    ---
    Learning-oriented step-by-step guides to get hands-on experience with `pgfs`.

    [:octicons-arrow-right-24: View Tutorials](tutorials/flat_filesystem.md)

-   :material-wrench: **How-To Guides**
    ---
    Task-oriented instructions for real-world configuration, operations, and troubleshooting.

    [:octicons-arrow-right-24: View How-To Guides](how-to/flat_filesystem.md)

-   :material-book-open-page-variant: **Reference**
    ---
    Complete, authoritative descriptions of parameters, FUSE callbacks, schemas, and metrics.

    [:octicons-arrow-right-24: View Reference](reference/flat_filesystem.md)

-   :material-lightbulb-outline: **Explanation**
    ---
    Deep architectural insights into storage engine designs, kernel offloading, and consistency models.

    [:octicons-arrow-right-24: View Explanation](explanation/flat_filesystem.md)

</div>

---

## ⚡ Specification Features

The following specification features are defined using `libspec.diataxis`:

- **Atomic Renames & Lock-Move Semantics** (`spec.app.AtomicRename`): Users can rename (move) files and directories with standard
- **POSIX Flat Filesystem** (`spec.app.FlatFilesystem`): Users can create, read, write, and delete files through standard
- **Hierarchical Directory Trees** (`spec.app.HierarchicalDirectories`): Users can create nested directories, list their contents, and
- **Reliable Mount & Lifecycle Management** (`spec.app.ReliableMount`): The filesystem mounts and unmounts cleanly. Crashing or killing
- **Database-Native Kernel Acceleration** (`spec.offload.KernelOffloadingAcceleration`): pgfs incorporates Database-Native Kernel Offloading capabilities, utilizing
- **Dual-Interface Storage Architecture** (`spec.optimize.DualInterfaceArchitecture`): pgfs features a Dual-Interface Storage Architecture providing a sub-100µs
- **Performance Optimization Pipeline** (`spec.optimize.OptimizationRoadmap`): pgfs incorporates a structured performance optimization pipeline
- **Replicated Physical Standby Storage** (`spec.replica.ReplicatedFilesystem`): A pgfs mount can be backed by a primary Postgres and a physical

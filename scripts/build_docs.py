#!/usr/bin/env python3
"""
Generates Diátaxis-structured MkDocs documentation from pgfs libspec feature specifications.
"""

import sys
sys.path.insert(0, ".")

from pathlib import Path
from spec.main_spec import MainSpec
from libspec.diataxis import Diataxis
import inspect

DOCS_DIR = Path("docs")

FEATURE_TITLES = {
    "FlatFilesystem": "POSIX Flat Filesystem",
    "HierarchicalDirectories": "Hierarchical Directory Trees",
    "AtomicRename": "Atomic Renames & Lock-Move Semantics",
    "ReliableMount": "Reliable Mount & Lifecycle Management",
    "KernelOffloadingAcceleration": "Database-Native Kernel Acceleration",
    "DualInterfaceArchitecture": "Dual-Interface Storage Architecture",
    "OptimizationRoadmap": "Performance Optimization Pipeline",
    "ReplicatedFilesystem": "Replicated Physical Standby Storage",
}

QUADRANTS = [
    ("tutorials", "Tutorials", "tutorial"),
    ("how-to", "How-To Guides", "how_to"),
    ("reference", "Reference", "reference"),
    ("explanation", "Explanation", "explanation"),
]


def collect_features():
    features = []
    for module in MainSpec().modules():
        for name, cls in inspect.getmembers(module, inspect.isclass):
            if (
                issubclass(cls, Diataxis)
                and cls is not Diataxis
                and not cls.__dict__.get("__is_base_spec__", False)
            ):
                features.append((module.__name__.split(".")[-1], name, cls()))
    return features


def generate_docs():
    features = collect_features()

    for folder, _, _ in QUADRANTS:
        (DOCS_DIR / folder).mkdir(parents=True, exist_ok=True)

    # 1. Index page
    index_content = """# pgfs — PostgreSQL-Backed FUSE Filesystem

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

"""
    for module_name, cls_name, instance in features:
        title = FEATURE_TITLES.get(cls_name, cls_name)
        doc = (instance.__doc__ or "").strip().split("\n")[0]
        index_content += f"- **{title}** (`spec.{module_name}.{cls_name}`): {doc}\n"

    (DOCS_DIR / "index.md").write_text(index_content, encoding="utf-8")

    # 2. Quadrant pages per feature
    for module_name, cls_name, instance in features:
        title = FEATURE_TITLES.get(cls_name, cls_name)
        file_stem = cls_name.lower()
        if file_stem.endswith("filesystem"):
            file_stem = file_stem[:-10] + "_filesystem"

        doc_summary = (instance.__doc__ or "").strip()

        # Tutorial
        tut_content = f"# Tutorial: {title}\n\n**Feature**: `spec.{module_name}.{cls_name}`\n\n> {doc_summary}\n\n---\n\n{instance.tutorial()}\n"
        (DOCS_DIR / "tutorials" / f"{file_stem}.md").write_text(tut_content, encoding="utf-8")

        # How-To
        howto_content = f"# How-To: {title}\n\n**Feature**: `spec.{module_name}.{cls_name}`\n\n> {doc_summary}\n\n---\n\n{instance.how_to()}\n"
        (DOCS_DIR / "how-to" / f"{file_stem}.md").write_text(howto_content, encoding="utf-8")

        # Reference
        ref_content = f"# Reference: {title}\n\n**Feature**: `spec.{module_name}.{cls_name}`\n\n> {doc_summary}\n\n---\n\n{instance.reference()}\n"
        (DOCS_DIR / "reference" / f"{file_stem}.md").write_text(ref_content, encoding="utf-8")

        # Explanation
        exp_content = f"# Explanation: {title}\n\n**Feature**: `spec.{module_name}.{cls_name}`\n\n> {doc_summary}\n\n---\n\n{instance.explanation()}\n"
        (DOCS_DIR / "explanation" / f"{file_stem}.md").write_text(exp_content, encoding="utf-8")

    print(f"Generated Diátaxis documentation for {len(features)} features in {DOCS_DIR}/")


if __name__ == "__main__":
    generate_docs()

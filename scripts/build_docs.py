#!/usr/bin/env python3
"""
Generates Diátaxis-structured MkDocs documentation from pgfs libspec feature specifications.

Every `Feat` subclass in spec/ implements the four Diátaxis quadrants
(`tutorial`, `how_to`, `reference`, `explanation`) plus `title`, `status`
and `status_note`, and lists its Requirement classes in `deps`. This script
renders one page per feature per quadrant, an index, and the `nav:` section
of mkdocs.yml. Everything under docs/{tutorials,how-to,reference,explanation}
and docs/index.md is generated; edit the spec, not the pages.
"""

import inspect
import re
import sys

sys.path.insert(0, ".")

from pathlib import Path

from libspec.diataxis import Diataxis
from spec.err import PARTIAL, PLANNED, STABLE
from spec.main_spec import MainSpec

DOCS_DIR = Path("docs")
MKDOCS_YML = Path("mkdocs.yml")

# Reading order: getting started, core filesystem semantics, operations,
# then optional modes and roadmap features. Features not listed here are
# appended in discovery order.
READING_ORDER = [
    "ProjectLocalEnvironment",
    "FlatFilesystem",
    "HierarchicalDirectories",
    "AtomicRename",
    "ReliableMount",
    "ObservableDaemon",
    "OnDemandProfiling",
    "ReplicatedFilesystem",
    "OptimizationRoadmap",
    "KernelOffloadingAcceleration",
    "DualInterfaceArchitecture",
    "ShardedFilesystem",
]

# (folder, nav label, method, page-title prefix)
QUADRANTS = [
    ("tutorials", "Tutorials", "tutorial", "Tutorial"),
    ("how-to", "How-To Guides", "how_to", "How-To"),
    ("reference", "Reference", "reference", "Reference"),
    ("explanation", "Explanation", "explanation", "Explanation"),
]

STATUS_STYLE = {
    STABLE: ("success", "Stable"),
    PARTIAL: ("warning", "Partially implemented"),
    PLANNED: ("danger", "Planned, not implemented"),
}


def collect_features():
    found = {}
    for module in MainSpec().modules():
        for name, cls in inspect.getmembers(module, inspect.isclass):
            if (
                issubclass(cls, Diataxis)
                and cls is not Diataxis
                and cls.__module__ == module.__name__
                and not cls.__dict__.get("__is_base_spec__", False)
            ):
                found[name] = (module.__name__, cls)
    ordered = [n for n in READING_ORDER if n in found]
    ordered += [n for n in found if n not in READING_ORDER]
    return [(found[n][0], n, found[n][1]()) for n in ordered]


def slug(cls_name):
    # Kept stable for existing URLs: "FlatFilesystem" -> "flat_filesystem".
    stem = cls_name.lower()
    if stem.endswith("filesystem"):
        stem = stem[: -len("filesystem")] + "_filesystem"
    return stem


def summary(instance):
    return " ".join((type(instance).__doc__ or "").split())


def status_block(instance):
    kind, label = STATUS_STYLE[instance.status()]
    note = " ".join(instance.status_note().split())
    return f'!!! {kind} "Status: {label}"\n    {note}\n'


def indent(text, prefix="    "):
    return "\n".join(prefix + line if line.strip() else "" for line in text.splitlines())


def requirement_markdown(cls):
    """Render a Requirement docstring as Markdown, inserting the blank line
    Python-Markdown needs before a list that follows a paragraph line."""
    lines = inspect.cleandoc(cls.__doc__ or "").splitlines()
    out = []
    in_list = False
    for line in lines:
        is_item = line.lstrip().startswith("- ")
        if is_item and not in_list and out and out[-1].strip():
            out.append("")
        if is_item:
            in_list = True
        elif not line.strip():
            in_list = False
        out.append(line)
    return "\n".join(out)


def requirements_section(instance):
    deps = list(getattr(type(instance), "deps", []) or [])
    if not deps:
        return ""
    parts = [
        "## Specification requirements",
        "",
        f"`{type(instance).__name__}` is built from these requirement "
        "classes. Each is declared in `spec/` and listed in the feature's "
        "`deps`.",
        "",
    ]
    for req in deps:
        ref = f"{req.__module__}.{req.__name__}"
        parts.append(f'??? abstract "{req.__name__}"')
        parts.append(f"    `{ref}`")
        parts.append("")
        parts.append(indent(requirement_markdown(req)))
        parts.append("")
    return "\n".join(parts)


def other_views(cls_name, current_folder):
    s = slug(cls_name)
    links = []
    for folder, _, _, prefix in QUADRANTS:
        label = "How-to" if folder == "how-to" else prefix
        if folder == current_folder:
            links.append(f"**{label}**")
        else:
            links.append(f"[{label}](../{folder}/{s}.md)")
    return " · ".join(links)


def render_page(module_name, cls_name, instance, folder, method, prefix):
    title = instance.title()
    body = getattr(instance, method)().strip()
    parts = [
        f"# {prefix}: {title}",
        "",
        status_block(instance),
        f"> {summary(instance)}",
        "",
        f"{other_views(cls_name, folder)}",
        "",
        "---",
        "",
        body,
        "",
    ]
    if method == "reference":
        reqs = requirements_section(instance)
        if reqs:
            parts += ["", reqs]
    parts += [
        "---",
        "",
        f"<small>Generated from `{module_name}.{cls_name}` by "
        "`scripts/build_docs.py`. Edit the specification, not this page.</small>",
        "",
    ]
    return "\n".join(parts)


def render_index(features):
    rows = []
    for _, cls_name, instance in features:
        s = slug(cls_name)
        _, label = STATUS_STYLE[instance.status()]
        rows.append(
            f"| **{instance.title()}** | {label} | "
            f"[Tutorial](tutorials/{s}.md) | [How-to](how-to/{s}.md) | "
            f"[Reference](reference/{s}.md) | [Explanation](explanation/{s}.md) |"
        )
    summaries = "\n".join(
        f"- **{instance.title()}**: {summary(instance)}" for _, _, instance in features
    )
    return f"""# pgfs: a PostgreSQL-backed FUSE filesystem

pgfs mounts a directory tree whose files and directories are rows in a
PostgreSQL database. Ordinary programs (`cp`, `git`, editors, build tools)
read and write it like any local directory; SQL clients see the same data
as tables.

```bash
nix develop
./scripts/pgfs.sh up                          # PostgreSQL + build + mount at testdata/mnt
echo hello > testdata/mnt/hello.txt
psql -h "$PWD/testdata" -d pgfs -c "SELECT parent, name, size FROM entries"
./scripts/pgfs.sh down
```

## How these pages are organised

The documentation follows [Diátaxis](https://diataxis.fr). Every feature is
described four ways; pick the one that matches what you are doing.

| If you want to… | Read the | It is |
|---|---|---|
| learn by doing, step by step | **Tutorial** | a guided lesson with a visible result |
| get a specific task done | **How-to guide** | a recipe for someone who knows the basics |
| look up an exact fact | **Reference** | complete, terse description: options, schema, errors |
| understand why it works this way | **Explanation** | background, design reasoning and trade-offs |

## Features

| Feature | Status | | | | |
|---|---|---|---|---|---|
{chr(10).join(rows)}

**Stable** features are implemented and tested. **Partially implemented**
and **Planned** pages say on every page what works today; for planned
features, the tutorials and how-to guides describe the intended interface.

## Where to start

1. [Your first pgfs mount](tutorials/projectlocalenvironment.md) gets a mount running in five minutes.
2. [Regular Files](tutorials/flat_filesystem.md), [Directory Trees](tutorials/hierarchicaldirectories.md)
   and [Atomic Rename](tutorials/atomicrename.md) cover everyday use.
3. [Reliable Mount](how-to/reliablemount.md) and [Observability](how-to/observabledaemon.md)
   are for running pgfs and diagnosing it.
4. [Regular Files: Explanation](explanation/flat_filesystem.md) explains the storage model
   everything else builds on.

## Feature summaries

{summaries}

---

<small>Generated from the `libspec.diataxis` Feature classes in `spec/` by
`scripts/build_docs.py`. Edit the specification, not these pages.</small>
"""


def render_nav(features):
    lines = ["nav:", "  - Overview: index.md"]
    for folder, label, _, _ in QUADRANTS:
        lines.append(f"  - {label}:")
        for _, cls_name, instance in features:
            lines.append(f'      - "{instance.title()}": {folder}/{slug(cls_name)}.md')
    return "\n".join(lines) + "\n"


def write_nav(features):
    text = MKDOCS_YML.read_text(encoding="utf-8")
    match = re.search(r"^nav:\s*$", text, flags=re.MULTILINE)
    if not match:
        raise SystemExit("mkdocs.yml has no top-level `nav:` section to replace")
    # nav: is the last top-level key; everything from it to EOF is generated.
    MKDOCS_YML.write_text(text[: match.start()] + render_nav(features), encoding="utf-8")


def generate_docs():
    features = collect_features()
    written = set()

    for folder, _, method, prefix in QUADRANTS:
        out_dir = DOCS_DIR / folder
        out_dir.mkdir(parents=True, exist_ok=True)
        for module_name, cls_name, instance in features:
            path = out_dir / f"{slug(cls_name)}.md"
            path.write_text(
                render_page(module_name, cls_name, instance, folder, method, prefix),
                encoding="utf-8",
            )
            written.add(path)
        for stale in out_dir.glob("*.md"):
            if stale not in written:
                stale.unlink()
                print(f"removed stale page {stale}")

    (DOCS_DIR / "index.md").write_text(render_index(features), encoding="utf-8")
    write_nav(features)
    print(f"Generated Diátaxis documentation for {len(features)} features in {DOCS_DIR}/")


if __name__ == "__main__":
    generate_docs()

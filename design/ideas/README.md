# Ideas

Uses of pgfs that go beyond what the specification states. Nothing here is
specified or promised; it records what was worked out, what was tried,
and what would be needed. The specified features have their own
documentation in `docs/`, generated from `spec/`.

Each note follows [Diátaxis](https://diataxis.fr/) and has four parts:

- **Tutorial**: a lesson you can run end to end today, on a throwaway
  database. Where an idea cannot be tried yet, the tutorial says so and
  names what is missing, rather than describing steps that do not work.
- **How-to guides**: recipes for a goal, assuming you know pgfs.
- **Reference**: tables of facts: mappings, requirements, gaps.
- **Explanation**: why it works, what the trade-offs are.

| Idea | Tutorial | How-to | Reference | Explanation |
|---|---|---|---|---|
| [Can it git?](can-it-git.md): versioning a directory like a repository | runnable | yes | yes | yes |
| [OS upgrades with branches](os-upgrades-with-branches.md): branch, upgrade, test, promote | runnable (a simulated OS tree) | yes | yes | yes |
| [An operating system on pgfs](os-on-pgfs.md): containers and root filesystems | not yet: needs root, untried | outline | yes | yes |
| [Booting an OS on bare metal from pgfs](bare-metal-root.md): initramfs, `switch_root`, shutdown | not yet: needs `--no-auto-unmount` and an initramfs module | plan (VM first) | yes | yes |
| [Hosting PostgreSQL itself](hosting-postgres.md): the filesystem underneath | not applicable | yes | yes | yes |
| [pgfs and ZFS](pgfs-vs-zfs.md): what each does that the other doesn't | not applicable | yes | yes | yes |

The runnable tutorials were run against pgfs 0.2.0 on 2026-10-30. They
use their own database, `pgfs_tut`, so they never touch the `pgfs`
database of `./scripts/pgfs.sh`:

```bash
# from the repository root, inside `nix develop`
cargo build
alias pgfs=$PWD/target/debug/pgfs
createdb -h "$PWD/testdata" pgfs_tut
export PGFS_CONN="host=$PWD/testdata dbname=pgfs_tut"
pgfs mkfs
```

When done: `dropdb -h "$PWD/testdata" pgfs_tut`.

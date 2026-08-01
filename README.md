# pgfs

A filesystem that lives in PostgreSQL. You mount a directory, and every
`ls`, `mkdir`, `cat`, `echo >`, and `rm` becomes rows in a Postgres table —
your filesystem is a database, with all the querying and replication that
implies.

Everything runs through [Nix](https://nixos.org): `flake.nix` pins the Rust
toolchain, FUSE3 headers, and PostgreSQL, so `nix develop` gives you the
whole environment, reproducible. The one non-Nix system requirement is a
single line in `/etc/fuse.conf` (see [Setup](#setup)).

It's deliberately small: arbitrary directory nesting, whole files stored as
`bytea` blobs, no chunking, no search. It exists to prove the read/write
path end to end — a playground to poke at, and a foundation to build on.

## Quick start

```bash
# 1. Enter the Nix dev shell (pins Rust 1.9x, Postgres 17, FUSE3)
nix develop

# 2. Bring up Postgres + build + mount (waits until the mount is live)
./scripts/pgfs.sh up

# 3. It's just a directory — mess around
echo "hello world" > testdata/mnt/hello.txt
mkdir -p testdata/mnt/docs
echo "notes" > testdata/mnt/docs/notes.txt
cat testdata/mnt/hello.txt
find testdata/mnt
rm testdata/mnt/docs/notes.txt

# 4. See what Postgres actually stored
psql -h "$(pwd)/testdata" -d pgfs -c "SELECT parent, name, kind, size FROM entries ORDER BY parent, name;"

# 5. Tear it down
./scripts/pgfs.sh down
```

Three commands (`up`, `down`, `status`) are the whole lifecycle. See the
full docs in [MANUAL.md](MANUAL.md).

## Setup

**Nix (recommended).** `nix develop` provides `rustc`/`cargo`, PostgreSQL
server binaries, and FUSE3 — no system packages to install. If you use
direnv, the included `.envrc` enters the shell automatically.

**Non-Nix fallback.** You'll need a current stable Rust (1.85+), `postgres`
binaries, and `libfuse3-dev` + `pkg-config` on your `PATH`.

**The one system line.** The mount uses `auto_unmount` so a dead daemon can
never leave a ghost mount behind. That requires `user_allow_other` in
`/etc/fuse.conf`:

```
sudo sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
```

## How it works, in one paragraph

Two modules meet behind a thin seam: `db.rs` is the only file that speaks
SQL, and `fs.rs` is the only file that speaks FUSE. Postgres stores the tree
in one `entries` table keyed by `(parent, name)`; `fs.rs` keeps an in-memory
map translating FUSE inode numbers (which the kernel uses for everything
after the first lookup) to those keys. Every filesystem syscall becomes a
query over a Unix socket to a Postgres cluster that lives entirely inside
`testdata/` — no TCP, no system-wide Postgres, no data written outside the
project.

## Layout

```
flake.nix          pinned Nix dev environment (Rust + Postgres + FUSE3)
scripts/
  init_db.sh       create/start the project-local Postgres cluster
  pgfs.sh          the up/down/status/run lifecycle control
src/
  db.rs            the only module that runs SQL
  fs.rs            the only module that talks FUSE (fuser::Filesystem)
  main.rs          wires them together, CLI args, mount lifecycle
testdata/          gitignored; the Postgres cluster, logs, pid, mount point
MANUAL.md          the full manual
```

## Limitations (by design, for now)

- **`mv`/rename not implemented** — `cp` works, `mv` doesn't (yet).
- **Whole-blob storage** — every write rewrites the entire file.
- **No permissions** — files are always 0644, dirs 0755.
- **Single-threaded** — fine for the kernel's default mount; the in-memory
  inode map would need a lock before going multi-threaded.
- **No search, indices, or chunked blocks yet.**

Details, rationale, troubleshooting, and the roadmap are in
[MANUAL.md](MANUAL.md).

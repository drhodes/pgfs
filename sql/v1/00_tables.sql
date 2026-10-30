-- pgfs schema version 1: tables, sequences and indexes.
--
-- spec/storage.py StorageLayout. `@UNLOGGED@` is replaced by `UNLOGGED`
-- on a scratch filesystem (spec/durability.py ScratchMode) and by nothing
-- otherwise. Every row that can change carries `born`/`died` epochs
-- (spec/branch.py EpochVersionedRows); `died = 9223372036854775807` marks
-- a live row.

CREATE TABLE pgfs.meta (
    key   text PRIMARY KEY,
    value text NOT NULL
);

-- ── Trees, layers, snapshots (logged even on a scratch filesystem) ──────

CREATE SEQUENCE pgfs.layer_seq;

CREATE TABLE pgfs.layers (
    layer          bigint PRIMARY KEY,
    base_layer     bigint REFERENCES pgfs.layers,
    base_epoch     bigint,
    depth          integer NOT NULL DEFAULT 0,
    epoch          bigint NOT NULL DEFAULT 1,
    state          text NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'deleting')),
    deleting_since timestamptz,
    CHECK ((base_layer IS NULL) = (base_epoch IS NULL))
);

CREATE TABLE pgfs.trees (
    name       text PRIMARY KEY,
    layer      bigint NOT NULL UNIQUE REFERENCES pgfs.layers,
    root_ino   bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE pgfs.snapshots (
    tree          text NOT NULL,
    name          text NOT NULL,
    layer         bigint NOT NULL REFERENCES pgfs.layers,
    epoch         bigint NOT NULL,
    xid           xid8 NOT NULL,
    root_ino      bigint NOT NULL,
    logical_bytes bigint NOT NULL DEFAULT 0,
    inodes        bigint NOT NULL DEFAULT 0,
    created_at    timestamptz NOT NULL DEFAULT now(),
    digest        text,
    PRIMARY KEY (tree, name)
);
CREATE INDEX snapshots_layer ON pgfs.snapshots (layer, epoch);

-- Epochs released by `pgfs snapshot delete` / `pgfs tree delete` stay
-- retained until every reader of them has failed closed
-- (spec/space.py ReclaimsRetiredRows grace period).
CREATE TABLE pgfs.epoch_holds (
    layer bigint NOT NULL,
    epoch bigint NOT NULL,
    until timestamptz NOT NULL,
    PRIMARY KEY (layer, epoch)
);

-- ── Mount sessions and control requests ────────────────────────────────

CREATE SEQUENCE pgfs.fence_seq;

CREATE TABLE pgfs.mounts (
    mount_id    uuid PRIMARY KEY,
    tree        text NOT NULL,
    snapshot    text,
    layer       bigint NOT NULL,
    read_only   boolean NOT NULL,
    fence       bigint NOT NULL,
    host        text NOT NULL DEFAULT '',
    pid         integer NOT NULL DEFAULT 0,
    workers     integer NOT NULL DEFAULT 0,
    acquired_at timestamptz NOT NULL DEFAULT now(),
    lease_until timestamptz NOT NULL
);
CREATE UNIQUE INDEX mounts_one_writer ON pgfs.mounts (tree) WHERE NOT read_only;

CREATE TABLE pgfs.control_requests (
    request_id bigserial PRIMARY KEY,
    tree       text NOT NULL,
    kind       text NOT NULL CHECK (kind IN ('snapshot', 'branch')),
    args       jsonb NOT NULL,
    state      text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'done', 'cancelled', 'failed')),
    result     jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- ── Inodes, names, data, xattrs (unlogged on a scratch filesystem) ─────

CREATE SEQUENCE pgfs.ino_seq START 2;
CREATE SEQUENCE pgfs.blob_seq;

CREATE @UNLOGGED@ TABLE pgfs.inodes (
    layer         bigint NOT NULL,
    ino           bigint NOT NULL,
    gen           bigint NOT NULL,
    born          bigint NOT NULL,
    died          bigint NOT NULL DEFAULT 9223372036854775807,
    kind          "char" NOT NULL CHECK (kind IN ('f', 'd', 'l', 'p', 's', 'c', 'b')),
    mode          integer NOT NULL,
    uid           bigint NOT NULL,
    gid           bigint NOT NULL,
    nlink         integer NOT NULL,
    size          bigint NOT NULL DEFAULT 0,
    atime_ns      bigint NOT NULL,
    mtime_ns      bigint NOT NULL,
    ctime_ns      bigint NOT NULL,
    btime_ns      bigint NOT NULL,
    rdev          bigint NOT NULL DEFAULT 0,
    inline_data   bytea,
    target        bytea,
    inherit_limit bigint NOT NULL DEFAULT 9223372036854775807,
    shadows_base  boolean NOT NULL DEFAULT false,
    version       bigint NOT NULL DEFAULT 1,
    PRIMARY KEY (layer, ino, born),
    CHECK (born < died)
) WITH (fillfactor = 80);
CREATE UNIQUE INDEX inodes_live ON pgfs.inodes (layer, ino) WHERE died = 9223372036854775807;
CREATE INDEX inodes_born ON pgfs.inodes (layer, born);
CREATE INDEX inodes_died ON pgfs.inodes (layer, died) WHERE died <> 9223372036854775807;

CREATE @UNLOGGED@ TABLE pgfs.dentries (
    layer        bigint NOT NULL,
    parent_ino   bigint NOT NULL,
    name         bytea NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    born         bigint NOT NULL,
    died         bigint NOT NULL DEFAULT 9223372036854775807,
    ino          bigint,
    kind         "char",
    shadows_base boolean NOT NULL DEFAULT false,
    PRIMARY KEY (layer, parent_ino, name, born),
    CHECK (born < died)
);
CREATE UNIQUE INDEX dentries_live ON pgfs.dentries (layer, parent_ino, name) WHERE died = 9223372036854775807;
CREATE INDEX dentries_ino ON pgfs.dentries (layer, ino) WHERE ino IS NOT NULL;
CREATE INDEX dentries_born ON pgfs.dentries (layer, born);
CREATE INDEX dentries_died ON pgfs.dentries (layer, died) WHERE died <> 9223372036854775807;

CREATE @UNLOGGED@ TABLE pgfs.blobs (
    blob_id bigint PRIMARY KEY,
    len     integer NOT NULL,
    crc32c  integer NOT NULL,
    shared  boolean NOT NULL DEFAULT false,
    data    bytea COMPRESSION lz4 NOT NULL,
    CHECK (len = octet_length(data))
) WITH (toast_tuple_target = 8160, autovacuum_vacuum_scale_factor = 0.02);

CREATE @UNLOGGED@ TABLE pgfs.file_blocks (
    layer    bigint NOT NULL,
    ino      bigint NOT NULL,
    block_no bigint NOT NULL CHECK (block_no >= 0),
    born     bigint NOT NULL,
    died     bigint NOT NULL DEFAULT 9223372036854775807,
    blob_id  bigint REFERENCES pgfs.blobs,
    PRIMARY KEY (layer, ino, block_no, born),
    CHECK (born < died)
) WITH (autovacuum_vacuum_scale_factor = 0.02);
CREATE UNIQUE INDEX file_blocks_live ON pgfs.file_blocks (layer, ino, block_no) WHERE died = 9223372036854775807;
CREATE INDEX file_blocks_blob ON pgfs.file_blocks (blob_id) WHERE blob_id IS NOT NULL;
CREATE INDEX file_blocks_born ON pgfs.file_blocks (layer, born);
CREATE INDEX file_blocks_died ON pgfs.file_blocks (layer, died) WHERE died <> 9223372036854775807;

CREATE @UNLOGGED@ TABLE pgfs.xattrs (
    layer        bigint NOT NULL,
    ino          bigint NOT NULL,
    name         bytea NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    value        bytea CHECK (octet_length(value) <= 65536),
    born         bigint NOT NULL,
    died         bigint NOT NULL DEFAULT 9223372036854775807,
    shadows_base boolean NOT NULL DEFAULT false,
    PRIMARY KEY (layer, ino, name, born),
    CHECK (born < died)
);
CREATE UNIQUE INDEX xattrs_live ON pgfs.xattrs (layer, ino, name) WHERE died = 9223372036854775807;
CREATE INDEX xattrs_born ON pgfs.xattrs (layer, born);
CREATE INDEX xattrs_died ON pgfs.xattrs (layer, died) WHERE died <> 9223372036854775807;

CREATE @UNLOGGED@ TABLE pgfs.orphans (
    layer bigint NOT NULL,
    ino   bigint NOT NULL,
    since timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (layer, ino)
);

CREATE @UNLOGGED@ TABLE pgfs.op_log (
    mount_id  uuid NOT NULL REFERENCES pgfs.mounts ON DELETE CASCADE,
    worker_id integer NOT NULL,
    op_seq    bigint NOT NULL,
    result    jsonb,
    PRIMARY KEY (mount_id, worker_id)
) WITH (fillfactor = 80);

-- ── Change feed ────────────────────────────────────────────────────────

CREATE SEQUENCE pgfs.change_seq;

CREATE @UNLOGGED@ TABLE pgfs.changes (
    seq            bigint PRIMARY KEY,
    layer          bigint NOT NULL,
    xid            xid8 NOT NULL,
    at             timestamptz NOT NULL DEFAULT now(),
    op             text NOT NULL,
    ino            bigint,
    parent_ino     bigint,
    name           bytea,
    new_parent_ino bigint,
    new_name       bytea,
    path           bytea,
    new_path       bytea,
    kind           "char"
) WITH (autovacuum_vacuum_scale_factor = 0.01);
CREATE INDEX changes_layer_xid ON pgfs.changes (layer, xid);
CREATE INDEX changes_at ON pgfs.changes (at);

CREATE TABLE pgfs.change_consumers (
    name       text PRIMARY KEY,
    tree       text NOT NULL,
    layer      bigint NOT NULL,
    watermark  jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- ── Space accounting and housekeeping queues ───────────────────────────

CREATE @UNLOGGED@ TABLE pgfs.gc_queue (
    blob_id bigint PRIMARY KEY
);

CREATE @UNLOGGED@ TABLE pgfs.usage_deltas (
    seq            bigserial PRIMARY KEY,
    layer          bigint NOT NULL,
    logical_bytes  bigint NOT NULL DEFAULT 0,
    physical_bytes bigint NOT NULL DEFAULT 0,
    inodes         bigint NOT NULL DEFAULT 0
) WITH (autovacuum_vacuum_scale_factor = 0.01);

-- Folded totals: one row per layer for logical bytes and inodes; the
-- filesystem-wide physical total is kept on the row for layer 0.
CREATE @UNLOGGED@ TABLE pgfs.usage_totals (
    layer          bigint PRIMARY KEY,
    logical_bytes  bigint NOT NULL DEFAULT 0,
    physical_bytes bigint NOT NULL DEFAULT 0,
    inodes         bigint NOT NULL DEFAULT 0
);

CREATE TABLE pgfs.scrub_state (
    id             integer PRIMARY KEY CHECK (id = 1),
    position       bigint NOT NULL DEFAULT 0,
    pass_started   timestamptz,
    pass_completed timestamptz
);

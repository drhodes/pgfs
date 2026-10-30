-- pgfs schema version 1: shared helpers.
--
-- Error codes (spec/failure.py MapsDatabaseErrors): PF001 fenced; every
-- other PF0xx code names one errno, mirrored in src/errno.rs:
--   PF002 ENOENT  PF003 EEXIST  PF004 ENOTEMPTY  PF005 EISDIR  PF006 ENOTDIR
--   PF007 EINVAL  PF008 ENAMETOOLONG  PF009 EXDEV  PF010 cursor expired
--   PF011 tree moved  PF012 EMLINK  PF013 EFBIG  PF014 ENOSPC  PF015 EDQUOT
--   PF016 EROFS  PF017 EPERM  PF018 EBUSY  PF019 ESTALE  PF020 ENODATA
--   PF021 E2BIG  PF022 EOPNOTSUPP  PF023 ERANGE  PF024 EACCES
--   PF025 retry: flush the dirty inodes in DETAIL, then call again
--   PF026 ETIMEDOUT

-- The attributes every inode-returning function replies with.
CREATE TYPE pgfs.attr AS (
    ino      bigint,
    gen      bigint,
    kind     "char",
    mode     integer,
    uid      bigint,
    gid      bigint,
    nlink    integer,
    size     bigint,
    blocks   bigint,
    atime_ns bigint,
    mtime_ns bigint,
    ctime_ns bigint,
    btime_ns bigint,
    rdev     bigint,
    version  bigint
);

-- The view chain of (layer, epoch): the layer itself at depth 0, then its
-- base snapshot, and so on (spec/branch.py LayeredLookup).
CREATE FUNCTION pgfs._chain(p_layer bigint, p_epoch bigint)
RETURNS TABLE (layer bigint, epoch bigint, depth integer)
LANGUAGE sql STABLE AS $$
    WITH RECURSIVE c(layer, epoch, depth) AS (
        SELECT p_layer, p_epoch, 0
        UNION ALL
        SELECT l.base_layer, l.base_epoch, c.depth + 1
        FROM c JOIN pgfs.layers l ON l.layer = c.layer
        WHERE l.base_layer IS NOT NULL AND c.depth < 16
    )
    SELECT layer, epoch, depth FROM c ORDER BY depth
$$;

CREATE FUNCTION pgfs._has_base(p_layer bigint) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT base_layer IS NOT NULL FROM pgfs.layers WHERE layer = p_layer
$$;

-- The winning inode row for `ino` in a view, or NULL if absent or whited
-- out (nlink = 0 and not an orphan of that layer).
CREATE FUNCTION pgfs._inode(p_layer bigint, p_epoch bigint, p_ino bigint)
RETURNS pgfs.inodes LANGUAGE plpgsql STABLE AS $$
DECLARE
    c record;
    r pgfs.inodes;
BEGIN
    FOR c IN SELECT * FROM pgfs._chain(p_layer, p_epoch) LOOP
        SELECT * INTO r FROM pgfs.inodes i
        WHERE i.layer = c.layer AND i.ino = p_ino AND i.born <= c.epoch AND c.epoch < i.died;
        IF FOUND THEN
            IF r.nlink = 0 AND NOT EXISTS (
                SELECT 1 FROM pgfs.orphans o WHERE o.layer = c.layer AND o.ino = p_ino) THEN
                RETURN NULL;
            END IF;
            RETURN r;
        END IF;
    END LOOP;
    RETURN NULL;
END $$;

-- The inode row for `ino` as seen from the base of `p_layer` (depth >= 1),
-- used to copy up an inherited inode.
CREATE FUNCTION pgfs._inode_inherited(p_layer bigint, p_ino bigint)
RETURNS pgfs.inodes LANGUAGE plpgsql STABLE AS $$
DECLARE
    l record;
BEGIN
    SELECT * INTO l FROM pgfs.layers WHERE layer = p_layer;
    IF l.base_layer IS NULL THEN
        RETURN NULL;
    END IF;
    RETURN pgfs._inode(l.base_layer, l.base_epoch, p_ino);
END $$;

-- The winning dentry row for (parent, name) in a view, whiteouts included.
CREATE FUNCTION pgfs._dentry(p_layer bigint, p_epoch bigint, p_parent bigint, p_name bytea)
RETURNS pgfs.dentries LANGUAGE plpgsql STABLE AS $$
DECLARE
    c record;
    r pgfs.dentries;
BEGIN
    FOR c IN SELECT * FROM pgfs._chain(p_layer, p_epoch) LOOP
        SELECT * INTO r FROM pgfs.dentries d
        WHERE d.layer = c.layer AND d.parent_ino = p_parent AND d.name = p_name
          AND d.born <= c.epoch AND c.epoch < d.died;
        IF FOUND THEN
            RETURN r;
        END IF;
    END LOOP;
    RETURN NULL;
END $$;

-- The winning xattr row for (ino, name) in a view, whiteouts included.
CREATE FUNCTION pgfs._xattr(p_layer bigint, p_epoch bigint, p_ino bigint, p_name bytea)
RETURNS pgfs.xattrs LANGUAGE plpgsql STABLE AS $$
DECLARE
    c record;
    r pgfs.xattrs;
BEGIN
    FOR c IN SELECT * FROM pgfs._chain(p_layer, p_epoch) LOOP
        SELECT * INTO r FROM pgfs.xattrs x
        WHERE x.layer = c.layer AND x.ino = p_ino AND x.name = p_name
          AND x.born <= c.epoch AND c.epoch < x.died;
        IF FOUND THEN
            RETURN r;
        END IF;
    END LOOP;
    RETURN NULL;
END $$;

-- The visible block map of `ino` in [p_first, p_last] in a view: one row per
-- block number with a row (hole rows included, blob_id NULL), resolved
-- through the chain with each layer's inherit_limit
-- (spec/branch.py LayeredLookup, CopiesUpOnWrite).
CREATE FUNCTION pgfs._blocks(p_layer bigint, p_epoch bigint, p_ino bigint,
                             p_first bigint, p_last bigint)
RETURNS TABLE (block_no bigint, blob_id bigint)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    c record;
    lim bigint := 9223372036854775807;
    i pgfs.inodes;
    v_layers bigint[] := '{}';
    v_epochs bigint[] := '{}';
    v_lims bigint[] := '{}';
    v_depths integer[] := '{}';
BEGIN
    IF NOT pgfs._has_base(p_layer) THEN
        RETURN QUERY
            SELECT b.block_no, b.blob_id FROM pgfs.file_blocks b
            WHERE b.layer = p_layer AND b.ino = p_ino AND b.block_no BETWEEN p_first AND p_last
              AND b.born <= p_epoch AND p_epoch < b.died;
        RETURN;
    END IF;
    FOR c IN SELECT * FROM pgfs._chain(p_layer, p_epoch) LOOP
        EXIT WHEN lim <= p_first;
        v_layers := v_layers || c.layer;
        v_epochs := v_epochs || c.epoch;
        v_lims := v_lims || lim;
        v_depths := v_depths || c.depth;
        SELECT * INTO i FROM pgfs.inodes x
        WHERE x.layer = c.layer AND x.ino = p_ino AND x.born <= c.epoch AND c.epoch < x.died;
        IF FOUND THEN
            lim := LEAST(lim, i.inherit_limit);
        END IF;
    END LOOP;
    RETURN QUERY
        SELECT DISTINCT ON (b.block_no) b.block_no, b.blob_id
        FROM unnest(v_layers, v_epochs, v_lims, v_depths) AS cc(layer, epoch, lim, depth)
        JOIN pgfs.file_blocks b
          ON b.layer = cc.layer AND b.ino = p_ino
         AND b.block_no BETWEEN p_first AND LEAST(p_last, cc.lim - 1)
         AND b.born <= cc.epoch AND cc.epoch < b.died
        ORDER BY b.block_no, cc.depth;
END $$;

-- Stored 512-byte blocks of an inode, holes excluded (spec/app.py
-- ReportsAttributes).
CREATE FUNCTION pgfs._stored_blocks(p_layer bigint, p_epoch bigint, r pgfs.inodes)
RETURNS bigint LANGUAGE plpgsql STABLE AS $$
DECLARE
    bs bigint := current_setting('pgfs.block_size', true)::bigint;
    n bigint;
BEGIN
    IF r.kind <> 'f' THEN
        RETURN 0;
    END IF;
    IF r.inline_data IS NOT NULL THEN
        RETURN (octet_length(r.inline_data) + 511) / 512;
    END IF;
    IF bs IS NULL THEN
        SELECT value::bigint INTO bs FROM pgfs.meta WHERE key = 'block_size';
    END IF;
    IF NOT pgfs._has_base(p_layer) THEN
        SELECT count(*) INTO n FROM pgfs.file_blocks b
        WHERE b.layer = p_layer AND b.ino = r.ino AND b.blob_id IS NOT NULL
          AND b.born <= p_epoch AND p_epoch < b.died;
    ELSE
        SELECT count(*) INTO n FROM pgfs._blocks(p_layer, p_epoch, r.ino, 0, 9223372036854775806) x
        WHERE x.blob_id IS NOT NULL;
    END IF;
    RETURN LEAST(n * (bs / 512), (r.size + 511) / 512);
END $$;

CREATE FUNCTION pgfs._attr(p_layer bigint, p_epoch bigint, r pgfs.inodes)
RETURNS pgfs.attr LANGUAGE sql STABLE AS $$
    SELECT ROW(r.ino, r.gen, r.kind, r.mode, r.uid, r.gid, r.nlink, r.size,
               pgfs._stored_blocks(p_layer, p_epoch, r),
               r.atime_ns, r.mtime_ns, r.ctime_ns, r.btime_ns, r.rdev, r.version)::pgfs.attr
$$;

CREATE FUNCTION pgfs._meta_int(p_key text) RETURNS bigint
LANGUAGE sql STABLE AS $$
    SELECT value::bigint FROM pgfs.meta WHERE key = p_key
$$;

-- ── Mutation prologue and epilogue ─────────────────────────────────────

-- Take the layer gate shared, check the fence and return the layer's
-- current epoch (spec/branch.py EpochVersionedRows,
-- spec/consistency.py FencesStaleWriters). A NULL mount is a CLI caller,
-- refused while the tree has a live read-write mount.
CREATE FUNCTION pgfs._begin(p_layer bigint, p_mount uuid, p_fence bigint)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    v_epoch bigint;
    v_state text;
BEGIN
    PERFORM pg_advisory_xact_lock_shared(1885759091, p_layer::integer);
    SELECT epoch, state INTO v_epoch, v_state FROM pgfs.layers WHERE layer = p_layer;
    IF NOT FOUND OR v_state <> 'active' THEN
        RAISE EXCEPTION 'layer % does not exist', p_layer USING ERRCODE = 'PF002';
    END IF;
    IF p_mount IS NOT NULL THEN
        IF NOT EXISTS (SELECT 1 FROM pgfs.mounts
                       WHERE mount_id = p_mount AND fence = p_fence AND NOT read_only) THEN
            RAISE EXCEPTION 'mount % is fenced (fence %)', p_mount, p_fence USING ERRCODE = 'PF001';
        END IF;
    ELSE
        IF EXISTS (SELECT 1 FROM pgfs.mounts m JOIN pgfs.trees t ON t.name = m.tree
                   WHERE t.layer = p_layer AND NOT m.read_only AND m.lease_until > now()) THEN
            RAISE EXCEPTION 'the tree on layer % has a read-write mount', p_layer USING ERRCODE = 'PF018';
        END IF;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pgfs.trees WHERE layer = p_layer) THEN
        RAISE EXCEPTION 'layer % is not the live layer of a tree', p_layer USING ERRCODE = 'PF016';
    END IF;
    RETURN v_epoch;
END $$;

-- Record the operation's result in the worker's op_log slot, in the same
-- transaction (spec/failure.py IdempotentMutations).
CREATE FUNCTION pgfs._done(p_mount uuid, p_worker integer, p_op_seq bigint, p_result jsonb)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF p_mount IS NULL THEN
        RETURN;
    END IF;
    UPDATE pgfs.op_log SET op_seq = p_op_seq, result = p_result
    WHERE mount_id = p_mount AND worker_id = p_worker;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'op_log slot (%, %) is missing', p_mount, p_worker USING ERRCODE = 'PF001';
    END IF;
END $$;

CREATE FUNCTION pgfs._sync(p_sync boolean) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF p_sync THEN
        PERFORM set_config('synchronous_commit', 'on', true);
    END IF;
END $$;

-- ── Names ──────────────────────────────────────────────────────────────

CREATE FUNCTION pgfs._check_name(p_name bytea) RETURNS void
LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    IF p_name IS NULL OR octet_length(p_name) = 0 OR position('\x2f'::bytea IN p_name) > 0
       OR position('\x00'::bytea IN p_name) > 0 OR p_name = '\x2e'::bytea OR p_name = '\x2e2e'::bytea THEN
        RAISE EXCEPTION 'invalid name' USING ERRCODE = 'PF007';
    END IF;
    IF octet_length(p_name) > 255 THEN
        RAISE EXCEPTION 'name longer than 255 bytes' USING ERRCODE = 'PF008';
    END IF;
END $$;

-- ── Copy-on-write row access for the current epoch ─────────────────────

-- Lock the inode's own live row for update, making sure it was born in
-- the current epoch: a row from an earlier epoch is retired and copied,
-- and an inherited row is copied up with shadows_base = true
-- (spec/branch.py EpochVersionedRows, CopiesUpOnWrite). NULL if the inode
-- is absent or whited out in the view.
CREATE FUNCTION pgfs._inode_lock(p_layer bigint, p_epoch bigint, p_ino bigint)
RETURNS pgfs.inodes LANGUAGE plpgsql AS $$
DECLARE
    r pgfs.inodes;
    attempt integer := 0;
BEGIN
    LOOP
        attempt := attempt + 1;
        SELECT * INTO r FROM pgfs.inodes
        WHERE layer = p_layer AND ino = p_ino AND died = 9223372036854775807
        FOR UPDATE;
        IF FOUND THEN
            IF r.nlink = 0 AND NOT EXISTS (
                SELECT 1 FROM pgfs.orphans o WHERE o.layer = p_layer AND o.ino = p_ino) THEN
                RETURN NULL;
            END IF;
            IF r.born = p_epoch THEN
                RETURN r;
            END IF;
            UPDATE pgfs.inodes SET died = p_epoch
            WHERE layer = p_layer AND ino = p_ino AND born = r.born;
            r.born := p_epoch;
            r.died := 9223372036854775807;
            INSERT INTO pgfs.inodes VALUES (r.*);
            RETURN r;
        END IF;
        r := pgfs._inode_inherited(p_layer, p_ino);
        IF r IS NULL THEN
            RETURN NULL;
        END IF;
        r.layer := p_layer;
        r.born := p_epoch;
        r.died := 9223372036854775807;
        r.shadows_base := true;
        r.inherit_limit := 9223372036854775807;
        INSERT INTO pgfs.inodes VALUES (r.*) ON CONFLICT DO NOTHING;
        IF attempt >= 3 THEN
            RAISE EXCEPTION 'could not lock inode %', p_ino USING ERRCODE = '40001';
        END IF;
    END LOOP;
END $$;

-- Write back a row returned by _inode_lock (born in the current epoch).
CREATE FUNCTION pgfs._inode_put(r pgfs.inodes) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE pgfs.inodes SET
        gen = r.gen, kind = r.kind, mode = r.mode, uid = r.uid, gid = r.gid, nlink = r.nlink,
        size = r.size, atime_ns = r.atime_ns, mtime_ns = r.mtime_ns, ctime_ns = r.ctime_ns,
        btime_ns = r.btime_ns, rdev = r.rdev, inline_data = r.inline_data, target = r.target,
        inherit_limit = r.inherit_limit, shadows_base = r.shadows_base, version = r.version + 1
    WHERE layer = r.layer AND ino = r.ino AND born = r.born;
END $$;

-- Free a blob whose last current-epoch map row went away
-- (spec/space.py FreesUnsharedBlobsInline, CollectsSharedBlobs).
CREATE FUNCTION pgfs._blob_release(p_blob bigint) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    v_shared boolean;
BEGIN
    IF p_blob IS NULL THEN
        RETURN;
    END IF;
    SELECT shared INTO v_shared FROM pgfs.blobs WHERE blob_id = p_blob;
    IF v_shared THEN
        INSERT INTO pgfs.gc_queue VALUES (p_blob) ON CONFLICT DO NOTHING;
    ELSE
        DELETE FROM pgfs.blobs WHERE blob_id = p_blob;
    END IF;
END $$;

-- Remove every own map row of `ino` at or past `p_from` (in batches of at
-- most p_limit; the rest is left for the reclaimer, which removes map rows
-- past an inode's size). Returns the number of rows handled.
CREATE FUNCTION pgfs._blocks_cut(p_layer bigint, p_epoch bigint, p_ino bigint,
                                 p_from bigint, p_limit integer)
RETURNS integer LANGUAGE plpgsql AS $$
DECLARE
    b record;
    n integer := 0;
BEGIN
    FOR b IN SELECT * FROM pgfs.file_blocks
             WHERE layer = p_layer AND ino = p_ino AND block_no >= p_from
               AND died = 9223372036854775807
             ORDER BY block_no LIMIT p_limit FOR UPDATE LOOP
        IF b.born = p_epoch THEN
            DELETE FROM pgfs.file_blocks
            WHERE layer = p_layer AND ino = p_ino AND block_no = b.block_no AND born = b.born;
            PERFORM pgfs._blob_release(b.blob_id);
        ELSE
            UPDATE pgfs.file_blocks SET died = p_epoch
            WHERE layer = p_layer AND ino = p_ino AND block_no = b.block_no AND born = b.born;
        END IF;
        n := n + 1;
    END LOOP;
    RETURN n;
END $$;

-- Point block `p_block` of `ino` at `p_blob` (NULL writes a hole row).
CREATE FUNCTION pgfs._block_put(p_layer bigint, p_epoch bigint, p_ino bigint,
                                p_block bigint, p_blob bigint)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    b pgfs.file_blocks;
BEGIN
    SELECT * INTO b FROM pgfs.file_blocks
    WHERE layer = p_layer AND ino = p_ino AND block_no = p_block AND died = 9223372036854775807
    FOR UPDATE;
    IF FOUND THEN
        IF b.born = p_epoch THEN
            UPDATE pgfs.file_blocks SET blob_id = p_blob
            WHERE layer = p_layer AND ino = p_ino AND block_no = p_block AND born = b.born;
            PERFORM pgfs._blob_release(b.blob_id);
            RETURN;
        END IF;
        UPDATE pgfs.file_blocks SET died = p_epoch
        WHERE layer = p_layer AND ino = p_ino AND block_no = p_block AND born = b.born;
    END IF;
    INSERT INTO pgfs.file_blocks (layer, ino, block_no, born, blob_id)
    VALUES (p_layer, p_ino, p_block, p_epoch, p_blob);
END $$;

-- Make block `p_block` read as zero: remove the own row, and leave a hole
-- row when a base block could otherwise show through.
CREATE FUNCTION pgfs._block_hole(p_layer bigint, p_epoch bigint, r pgfs.inodes, p_block bigint)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    b pgfs.file_blocks;
    v_inherits boolean := r.shadows_base AND p_block < r.inherit_limit AND pgfs._has_base(p_layer);
BEGIN
    SELECT * INTO b FROM pgfs.file_blocks
    WHERE layer = p_layer AND ino = r.ino AND block_no = p_block AND died = 9223372036854775807
    FOR UPDATE;
    IF FOUND THEN
        IF b.blob_id IS NULL THEN
            RETURN;
        END IF;
        IF b.born = p_epoch THEN
            IF v_inherits THEN
                UPDATE pgfs.file_blocks SET blob_id = NULL
                WHERE layer = p_layer AND ino = r.ino AND block_no = p_block AND born = b.born;
            ELSE
                DELETE FROM pgfs.file_blocks
                WHERE layer = p_layer AND ino = r.ino AND block_no = p_block AND born = b.born;
            END IF;
            PERFORM pgfs._blob_release(b.blob_id);
            RETURN;
        END IF;
        UPDATE pgfs.file_blocks SET died = p_epoch
        WHERE layer = p_layer AND ino = r.ino AND block_no = p_block AND born = b.born;
    END IF;
    IF v_inherits THEN
        INSERT INTO pgfs.file_blocks (layer, ino, block_no, born, blob_id)
        VALUES (p_layer, r.ino, p_block, p_epoch, NULL);
    END IF;
END $$;

-- Delete an inode whose last link and last handle are gone: an inode
-- whiteout if a base can see it, otherwise the row goes (or is retired by
-- _inode_lock already). Its map rows, xattrs and orphan row in the layer
-- go with it (spec/branch.py CopiesUpOnWrite, spec/posix.py OpenAfterUnlink).
CREATE FUNCTION pgfs._inode_delete(p_layer bigint, p_epoch bigint, r pgfs.inodes)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    x record;
BEGIN
    PERFORM pgfs._blocks_cut(p_layer, p_epoch, r.ino, 0, 4096);
    FOR x IN SELECT * FROM pgfs.xattrs
             WHERE layer = p_layer AND ino = r.ino AND died = 9223372036854775807 FOR UPDATE LOOP
        IF x.born = p_epoch THEN
            DELETE FROM pgfs.xattrs WHERE layer = p_layer AND ino = r.ino AND name = x.name AND born = x.born;
        ELSE
            UPDATE pgfs.xattrs SET died = p_epoch
            WHERE layer = p_layer AND ino = r.ino AND name = x.name AND born = x.born;
        END IF;
    END LOOP;
    DELETE FROM pgfs.orphans WHERE layer = p_layer AND ino = r.ino;
    IF r.shadows_base THEN
        UPDATE pgfs.inodes SET nlink = 0, size = 0, inherit_limit = 0, inline_data = NULL,
                               target = NULL, version = version + 1
        WHERE layer = p_layer AND ino = r.ino AND born = r.born;
    ELSE
        DELETE FROM pgfs.inodes WHERE layer = p_layer AND ino = r.ino AND born = r.born;
    END IF;
END $$;

-- Insert the name (parent, name) -> ino. A whiteout of the name in this
-- layer is replaced (shadows_base = true); an existing name raises EEXIST,
-- decided by the unique index (spec/posix.py ExclusiveCreate).
CREATE FUNCTION pgfs._dentry_insert(p_layer bigint, p_epoch bigint, p_parent bigint,
                                    p_name bytea, p_ino bigint, p_kind "char")
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    d pgfs.dentries;
    v_inserted bigint;
BEGIN
    SELECT * INTO d FROM pgfs.dentries
    WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND died = 9223372036854775807
    FOR UPDATE;
    IF FOUND THEN
        IF d.ino IS NOT NULL THEN
            RAISE EXCEPTION 'name exists' USING ERRCODE = 'PF003';
        END IF;
        IF d.born = p_epoch THEN
            UPDATE pgfs.dentries SET ino = p_ino, kind = p_kind, shadows_base = true
            WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND born = d.born;
        ELSE
            UPDATE pgfs.dentries SET died = p_epoch
            WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND born = d.born;
            INSERT INTO pgfs.dentries (layer, parent_ino, name, born, ino, kind, shadows_base)
            VALUES (p_layer, p_parent, p_name, p_epoch, p_ino, p_kind, true);
        END IF;
        RETURN;
    END IF;
    IF pgfs._has_base(p_layer) THEN
        d := pgfs._dentry(p_layer, p_epoch, p_parent, p_name);
        IF d.ino IS NOT NULL THEN
            RAISE EXCEPTION 'name exists' USING ERRCODE = 'PF003';
        END IF;
    END IF;
    INSERT INTO pgfs.dentries (layer, parent_ino, name, born, ino, kind, shadows_base)
    VALUES (p_layer, p_parent, p_name, p_epoch, p_ino, p_kind, false)
    ON CONFLICT DO NOTHING
    RETURNING ino INTO v_inserted;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'name exists' USING ERRCODE = 'PF003';
    END IF;
END $$;

-- Point the existing name (parent, name) at `p_ino` (NULL makes it a
-- whiteout when a base can see it, or removes it otherwise).
CREATE FUNCTION pgfs._dentry_set(p_layer bigint, p_epoch bigint, p_parent bigint,
                                 p_name bytea, p_ino bigint, p_kind "char")
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    d pgfs.dentries;
BEGIN
    SELECT * INTO d FROM pgfs.dentries
    WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND died = 9223372036854775807
    FOR UPDATE;
    IF FOUND THEN
        IF p_ino IS NULL AND NOT d.shadows_base THEN
            IF d.born = p_epoch THEN
                DELETE FROM pgfs.dentries
                WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND born = d.born;
            ELSE
                UPDATE pgfs.dentries SET died = p_epoch
                WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND born = d.born;
            END IF;
            RETURN;
        END IF;
        IF d.born = p_epoch THEN
            UPDATE pgfs.dentries SET ino = p_ino, kind = p_kind
            WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND born = d.born;
        ELSE
            UPDATE pgfs.dentries SET died = p_epoch
            WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND born = d.born;
            INSERT INTO pgfs.dentries (layer, parent_ino, name, born, ino, kind, shadows_base)
            VALUES (p_layer, p_parent, p_name, p_epoch, p_ino, p_kind, d.shadows_base);
        END IF;
        RETURN;
    END IF;
    -- Inherited from the base: copy up (or whiteout).
    INSERT INTO pgfs.dentries (layer, parent_ino, name, born, ino, kind, shadows_base)
    VALUES (p_layer, p_parent, p_name, p_epoch, p_ino, p_kind, true);
END $$;

-- Lock the own live dentry row of (parent, name), if any, so that a
-- concurrent change to the name waits (spec/concurrency.py LockOrder).
CREATE FUNCTION pgfs._dentry_lock(p_layer bigint, p_parent bigint, p_name bytea)
RETURNS void LANGUAGE sql AS $$
    SELECT NULL FROM pgfs.dentries
    WHERE layer = p_layer AND parent_ino = p_parent AND name = p_name AND died = 9223372036854775807
    FOR UPDATE
$$;

-- ── Paths for the change feed ──────────────────────────────────────────

-- The live dentry naming directory `p_ino` in a view.
CREATE FUNCTION pgfs._name_of(p_layer bigint, p_epoch bigint, p_ino bigint)
RETURNS pgfs.dentries LANGUAGE plpgsql STABLE AS $$
DECLARE
    c record;
    d pgfs.dentries;
    w pgfs.dentries;
BEGIN
    FOR c IN SELECT * FROM pgfs._chain(p_layer, p_epoch) LOOP
        FOR d IN SELECT * FROM pgfs.dentries x
                 WHERE x.layer = c.layer AND x.ino = p_ino
                   AND x.born <= c.epoch AND c.epoch < x.died LOOP
            w := pgfs._dentry(p_layer, p_epoch, d.parent_ino, d.name);
            IF w.ino = p_ino THEN
                RETURN w;
            END IF;
        END LOOP;
    END LOOP;
    RETURN NULL;
END $$;

-- The path of directory `p_dir` from the view root, as bytes ('/' for the root).
CREATE FUNCTION pgfs._path(p_layer bigint, p_epoch bigint, p_dir bigint, p_root bigint)
RETURNS bytea LANGUAGE plpgsql STABLE AS $$
DECLARE
    v_path bytea := '';
    v_ino bigint := p_dir;
    d pgfs.dentries;
    steps integer := 0;
BEGIN
    WHILE v_ino <> p_root LOOP
        d := pgfs._name_of(p_layer, p_epoch, v_ino);
        IF d IS NULL THEN
            RETURN NULL;
        END IF;
        v_path := '\x2f'::bytea || d.name || v_path;
        v_ino := d.parent_ino;
        steps := steps + 1;
        IF steps > 4096 THEN
            RETURN NULL;
        END IF;
    END LOOP;
    IF v_path = '' THEN
        RETURN '\x2f'::bytea;
    END IF;
    RETURN v_path;
END $$;

CREATE FUNCTION pgfs._join(p_dir bytea, p_name bytea) RETURNS bytea
LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE WHEN p_dir IS NULL THEN NULL
                WHEN p_dir = '\x2f'::bytea THEN p_dir || p_name
                ELSE p_dir || '\x2f'::bytea || p_name END
$$;

CREATE FUNCTION pgfs._root_of(p_layer bigint) RETURNS bigint
LANGUAGE sql STABLE AS $$
    SELECT COALESCE((SELECT root_ino FROM pgfs.trees WHERE layer = p_layer), 1)
$$;

-- Append one change row; seq is taken after every lock is held
-- (spec/changefeed.py RecordsChanges).
CREATE FUNCTION pgfs._change(p_layer bigint, p_op text, p_ino bigint, p_kind "char",
                             p_parent bigint, p_name bytea, p_path bytea,
                             p_new_parent bigint DEFAULT NULL, p_new_name bytea DEFAULT NULL,
                             p_new_path bytea DEFAULT NULL)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO pgfs.changes (seq, layer, xid, op, ino, parent_ino, name, new_parent_ino,
                              new_name, path, new_path, kind)
    VALUES (nextval('pgfs.change_seq'), p_layer, pg_current_xact_id(), p_op, p_ino, p_parent,
            p_name, p_new_parent, p_new_name, p_path, p_new_path, p_kind)
$$;

-- Path of the inode `p_ino` through any one of its live names (for
-- changes on an inode reached by number).
CREATE FUNCTION pgfs._ino_path(p_layer bigint, p_epoch bigint, p_ino bigint)
RETURNS bytea LANGUAGE plpgsql STABLE AS $$
DECLARE
    d pgfs.dentries;
    v_root bigint := pgfs._root_of(p_layer);
BEGIN
    IF p_ino = v_root THEN
        RETURN '\x2f'::bytea;
    END IF;
    d := pgfs._name_of(p_layer, p_epoch, p_ino);
    IF d IS NULL THEN
        RETURN NULL;
    END IF;
    RETURN pgfs._join(pgfs._path(p_layer, p_epoch, d.parent_ino, v_root), d.name);
END $$;

-- 63-bit random generation number from the server's strong random source
-- (spec/storage.py PersistentInodes); never from a sequence.
CREATE FUNCTION pgfs._new_gen() RETURNS bigint
LANGUAGE sql VOLATILE AS $$
    -- The first 16 hex digits of one v4 UUID and the last 16 of another
    -- carry 60 and 62 random bits; their XOR is uniform over 64 bits.
    SELECT (('x' || substr(replace(gen_random_uuid()::text, '-', ''), 1, 16))::bit(64)::bigint
          # ('x' || substr(replace(gen_random_uuid()::text, '-', ''), 17, 16))::bit(64)::bigint)
          & 9223372036854775807
$$;

-- pgfs schema version 1: snapshots, branches, promote, flatten, delete
-- (spec/branch.py).

CREATE FUNCTION pgfs._check_tree_name(p_name text) RETURNS void
LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN
    IF p_name IS NULL OR p_name !~ '^[A-Za-z0-9._-]{1,64}$' OR left(p_name, 1) = '.' THEN
        RAISE EXCEPTION 'invalid name "%": use 1 to 64 characters from [A-Za-z0-9._-], not starting with "."',
            p_name USING ERRCODE = 'PF007';
    END IF;
END $$;

-- Resolve a path (bytes, '/'-separated, relative to p_root) to a directory
-- inode in a view (spec/branch.py ViewsRootedAtDirectory).
CREATE FUNCTION pgfs.resolve_dir(p_layer bigint, p_epoch bigint, p_root bigint, p_path bytea)
RETURNS bigint LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
    v_ino bigint := p_root;
    v_part text;
    d pgfs.dentries;
BEGIN
    FOREACH v_part IN ARRAY string_to_array(encode(COALESCE(p_path, ''), 'escape'), '/') LOOP
        CONTINUE WHEN v_part = '' OR v_part = '.';
        d := pgfs._dentry(p_layer, e, v_ino, decode(v_part, 'escape'));
        IF d.ino IS NULL THEN
            RAISE EXCEPTION 'path does not exist' USING ERRCODE = 'PF002';
        END IF;
        IF d.kind <> 'd' THEN
            RAISE EXCEPTION 'path is not a directory' USING ERRCODE = 'PF006';
        END IF;
        v_ino := d.ino;
    END LOOP;
    RETURN v_ino;
END $$;

CREATE FUNCTION pgfs._fence_check(p_tree text, p_mount uuid, p_fence bigint) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF p_mount IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM pgfs.mounts WHERE mount_id = p_mount AND fence = p_fence
          AND tree = p_tree AND NOT read_only) THEN
        RAISE EXCEPTION 'mount % is fenced', p_mount USING ERRCODE = 'PF001';
    END IF;
END $$;

-- Snapshot a tree in constant time: record the layer's current epoch and
-- advance it, holding the gate exclusively (spec/branch.py CreatesSnapshot).
CREATE FUNCTION pgfs.snapshot(p_tree text, p_name text, p_mount uuid DEFAULT NULL,
                              p_fence bigint DEFAULT NULL)
RETURNS TABLE (layer bigint, epoch bigint)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    t pgfs.trees;
    v_epoch bigint;
    u record;
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    PERFORM pgfs._check_tree_name(p_name);
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'tree % does not exist', p_tree USING ERRCODE = 'PF002';
    END IF;
    PERFORM pgfs._fence_check(p_tree, p_mount, p_fence);
    PERFORM pgfs._gate_exclusive(t.layer);
    IF EXISTS (SELECT 1 FROM pgfs.snapshots s WHERE s.tree = p_tree AND s.name = p_name) THEN
        RAISE EXCEPTION 'snapshot %@% exists', p_tree, p_name USING ERRCODE = 'PF003';
    END IF;
    SELECT l.epoch INTO v_epoch FROM pgfs.layers l WHERE l.layer = t.layer FOR UPDATE;
    u := pgfs.layer_usage(t.layer);
    INSERT INTO pgfs.snapshots (tree, name, layer, epoch, xid, root_ino, logical_bytes, inodes)
    VALUES (p_tree, p_name, t.layer, v_epoch, pg_current_xact_id(), t.root_ino,
            u.logical_bytes, u.inodes);
    UPDATE pgfs.layers l SET epoch = l.epoch + 1 WHERE l.layer = t.layer;
    layer := t.layer;
    epoch := v_epoch;
    RETURN NEXT;
END $$;

-- Branch a tree or snapshot into a new writable tree in constant time,
-- optionally rooted at a directory (spec/branch.py CreatesBranch,
-- ViewsRootedAtDirectory, LimitsBranchDepth).
CREATE FUNCTION pgfs.branch(p_source text, p_snapshot text, p_path bytea, p_new text,
                            p_mount uuid DEFAULT NULL, p_fence bigint DEFAULT NULL,
                            p_max_depth integer DEFAULT 8)
RETURNS TABLE (layer bigint, depth integer)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    t pgfs.trees;
    s pgfs.snapshots;
    v_base_layer bigint;
    v_base_epoch bigint;
    v_root bigint;
    v_depth integer;
    v_layer bigint := nextval('pgfs.layer_seq');
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    PERFORM pgfs._check_tree_name(p_new);
    IF EXISTS (SELECT 1 FROM pgfs.trees x WHERE x.name = p_new) THEN
        RAISE EXCEPTION 'tree % exists', p_new USING ERRCODE = 'PF003';
    END IF;
    SELECT * INTO t FROM pgfs.trees x WHERE x.name = p_source;
    IF p_snapshot IS NOT NULL THEN
        SELECT * INTO s FROM pgfs.snapshots x WHERE x.tree = p_source AND x.name = p_snapshot;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'snapshot %@% does not exist', p_source, p_snapshot USING ERRCODE = 'PF002';
        END IF;
        v_base_layer := s.layer;
        v_base_epoch := s.epoch;
        v_root := s.root_ino;
    ELSE
        IF t.name IS NULL THEN
            RAISE EXCEPTION 'tree % does not exist', p_source USING ERRCODE = 'PF002';
        END IF;
        PERFORM pgfs._fence_check(p_source, p_mount, p_fence);
        PERFORM pgfs._gate_exclusive(t.layer);
        SELECT l.epoch INTO v_base_epoch FROM pgfs.layers l WHERE l.layer = t.layer FOR UPDATE;
        UPDATE pgfs.layers l SET epoch = l.epoch + 1 WHERE l.layer = t.layer;
        v_base_layer := t.layer;
        v_root := t.root_ino;
    END IF;
    SELECT l.depth + 1 INTO v_depth FROM pgfs.layers l WHERE l.layer = v_base_layer;
    IF v_depth > p_max_depth THEN
        RAISE EXCEPTION 'branch depth % would exceed %; run pgfs tree flatten %',
            v_depth, p_max_depth, p_source USING ERRCODE = 'PF012';
    END IF;
    IF p_path IS NOT NULL AND p_path <> '\x2f'::bytea AND p_path <> ''::bytea THEN
        v_root := pgfs.resolve_dir(v_base_layer, v_base_epoch, v_root, p_path);
    END IF;
    INSERT INTO pgfs.layers (layer, base_layer, base_epoch, depth, epoch)
    VALUES (v_layer, v_base_layer, v_base_epoch, v_depth, 1);
    INSERT INTO pgfs.trees (name, layer, root_ino) VALUES (p_new, v_layer, v_root);
    layer := v_layer;
    depth := v_depth;
    RETURN NEXT;
END $$;

-- Move every row of layer `p_from` into layer `p_to` at epoch `p_epoch`
-- (normal promote fold-back, spec/branch.py PromotesBranch).
CREATE FUNCTION pgfs._fold(p_from bigint, p_to bigint, p_epoch bigint) RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE
    inf constant bigint := 9223372036854775807;
    v_has_base boolean := pgfs._has_base(p_to);
    n bigint := 0;
    k bigint;
BEGIN
    CREATE TEMP TABLE _pgfs_dead ON COMMIT DROP AS
        SELECT b.ino FROM pgfs.inodes b
        WHERE b.layer = p_from AND b.died = inf AND b.nlink = 0
          AND NOT EXISTS (SELECT 1 FROM pgfs.orphans o WHERE o.layer = p_from AND o.ino = b.ino);

    -- Inodes.
    UPDATE pgfs.inodes t SET died = p_epoch
    FROM pgfs.inodes b
    WHERE b.layer = p_from AND b.died = inf AND t.layer = p_to AND t.ino = b.ino AND t.died = inf;
    INSERT INTO pgfs.inodes
    SELECT p_to, b.ino, b.gen, p_epoch, inf, b.kind, b.mode, b.uid, b.gid, b.nlink, b.size,
           b.atime_ns, b.mtime_ns, b.ctime_ns, b.btime_ns, b.rdev, b.inline_data, b.target,
           LEAST(COALESCE(t.inherit_limit, inf), b.inherit_limit),
           COALESCE(t.shadows_base, b.shadows_base), b.version
    FROM pgfs.inodes b
    LEFT JOIN pgfs.inodes t ON t.layer = p_to AND t.ino = b.ino AND t.died = p_epoch
    WHERE b.layer = p_from AND b.died = inf
      AND (b.ino NOT IN (SELECT ino FROM _pgfs_dead)
           OR (v_has_base AND COALESCE(t.shadows_base, b.shadows_base)));
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;

    -- Dentries.
    UPDATE pgfs.dentries t SET died = p_epoch
    FROM pgfs.dentries b
    WHERE b.layer = p_from AND b.died = inf AND t.layer = p_to AND t.parent_ino = b.parent_ino
      AND t.name = b.name AND t.died = inf;
    INSERT INTO pgfs.dentries (layer, parent_ino, name, born, died, ino, kind, shadows_base)
    SELECT p_to, b.parent_ino, b.name, p_epoch, inf, b.ino, b.kind, COALESCE(t.shadows_base, b.shadows_base)
    FROM pgfs.dentries b
    LEFT JOIN pgfs.dentries t ON t.layer = p_to AND t.parent_ino = b.parent_ino AND t.name = b.name
                             AND t.died = p_epoch
    WHERE b.layer = p_from AND b.died = inf
      AND (b.ino IS NOT NULL OR (v_has_base AND COALESCE(t.shadows_base, b.shadows_base)));
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    -- Names under directories the branch removed.
    UPDATE pgfs.dentries t SET died = p_epoch
    WHERE t.layer = p_to AND t.died = inf AND t.born < p_epoch
      AND t.parent_ino IN (SELECT ino FROM _pgfs_dead);

    -- Xattrs.
    UPDATE pgfs.xattrs t SET died = p_epoch
    FROM pgfs.xattrs b
    WHERE b.layer = p_from AND b.died = inf AND t.layer = p_to AND t.ino = b.ino
      AND t.name = b.name AND t.died = inf;
    UPDATE pgfs.xattrs t SET died = p_epoch
    WHERE t.layer = p_to AND t.died = inf AND t.born < p_epoch AND t.ino IN (SELECT ino FROM _pgfs_dead);
    INSERT INTO pgfs.xattrs (layer, ino, name, value, born, died, shadows_base)
    SELECT p_to, b.ino, b.name, b.value, p_epoch, inf, COALESCE(t.shadows_base, b.shadows_base)
    FROM pgfs.xattrs b
    LEFT JOIN pgfs.xattrs t ON t.layer = p_to AND t.ino = b.ino AND t.name = b.name AND t.died = p_epoch
    WHERE b.layer = p_from AND b.died = inf
      AND (b.value IS NOT NULL OR (v_has_base AND COALESCE(t.shadows_base, b.shadows_base)));
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;

    -- Block map: the branch's rows replace the tree's; tree rows the branch
    -- no longer inherits (past its inherit_limit, or of a deleted inode) retire.
    UPDATE pgfs.file_blocks t SET died = p_epoch
    FROM pgfs.file_blocks b
    WHERE b.layer = p_from AND b.died = inf AND t.layer = p_to AND t.ino = b.ino
      AND t.block_no = b.block_no AND t.died = inf;
    UPDATE pgfs.file_blocks t SET died = p_epoch
    FROM pgfs.inodes b
    WHERE b.layer = p_from AND b.died = inf AND t.layer = p_to AND t.ino = b.ino
      AND t.died = inf AND t.born < p_epoch AND t.block_no >= b.inherit_limit;
    INSERT INTO pgfs.file_blocks (layer, ino, block_no, born, died, blob_id)
    SELECT p_to, b.ino, b.block_no, p_epoch, inf, b.blob_id
    FROM pgfs.file_blocks b
    WHERE b.layer = p_from AND b.died = inf
      AND (b.blob_id IS NOT NULL OR v_has_base);
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;

    INSERT INTO pgfs.orphans (layer, ino, since)
    SELECT p_to, o.ino, o.since FROM pgfs.orphans o WHERE o.layer = p_from
    ON CONFLICT DO NOTHING;
    RETURN n;
END $$;

-- Delete every row of a layer, freeing blobs no other row references.
-- Bounded: at most p_limit rows per table per call. Returns rows deleted.
CREATE FUNCTION pgfs._purge_layer(p_layer bigint, p_limit integer) RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE
    n bigint := 0;
    k bigint;
    b record;
BEGIN
    FOR b IN DELETE FROM pgfs.file_blocks f
             WHERE (f.layer, f.ino, f.block_no, f.born) IN (
                 SELECT x.layer, x.ino, x.block_no, x.born FROM pgfs.file_blocks x
                 WHERE x.layer = p_layer LIMIT p_limit)
             RETURNING f.blob_id LOOP
        n := n + 1;
        IF b.blob_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM pgfs.file_blocks f WHERE f.blob_id = b.blob_id) THEN
            PERFORM pgfs._blob_release(b.blob_id);
        END IF;
    END LOOP;
    DELETE FROM pgfs.dentries WHERE (layer, parent_ino, name, born) IN (
        SELECT layer, parent_ino, name, born FROM pgfs.dentries WHERE layer = p_layer LIMIT p_limit);
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    DELETE FROM pgfs.xattrs WHERE (layer, ino, name, born) IN (
        SELECT layer, ino, name, born FROM pgfs.xattrs WHERE layer = p_layer LIMIT p_limit);
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    DELETE FROM pgfs.inodes WHERE (layer, ino, born) IN (
        SELECT layer, ino, born FROM pgfs.inodes WHERE layer = p_layer LIMIT p_limit);
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    DELETE FROM pgfs.orphans WHERE layer = p_layer;
    RETURN n;
END $$;

-- Promote a branch to be the new contents of a tree, atomically
-- (spec/branch.py PromotesBranch).
CREATE FUNCTION pgfs.promote(p_branch text, p_tree text, p_force boolean DEFAULT false)
RETURNS TABLE (rows_folded bigint, snapshot text)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    b pgfs.trees;
    t pgfs.trees;
    bl pgfs.layers;
    tl pgfs.layers;
    v_snap text := 'pre-promote-' || to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYYMMDD"T"HH24MISSUS"Z"');
    e bigint;
    n bigint := 0;
    x record;
    u record;
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    SELECT * INTO b FROM pgfs.trees WHERE name = p_branch;
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF b.name IS NULL OR t.name IS NULL THEN
        RAISE EXCEPTION 'tree % or % does not exist', p_branch, p_tree USING ERRCODE = 'PF002';
    END IF;
    IF b.layer < t.layer THEN
        PERFORM pgfs._gate_exclusive(b.layer);
        PERFORM pgfs._gate_exclusive(t.layer);
    ELSE
        PERFORM pgfs._gate_exclusive(t.layer);
        PERFORM pgfs._gate_exclusive(b.layer);
    END IF;
    SELECT * INTO bl FROM pgfs.layers WHERE layer = b.layer FOR UPDATE;
    SELECT * INTO tl FROM pgfs.layers WHERE layer = t.layer FOR UPDATE;
    IF bl.base_layer IS DISTINCT FROM t.layer OR b.root_ino <> t.root_ino THEN
        IF bl.base_layer IS NOT NULL AND EXISTS (
            SELECT 1 FROM pgfs.snapshots s WHERE s.tree = p_tree AND s.layer = bl.base_layer) THEN
            RAISE EXCEPTION 'tree % moved to another layer since % was branched', p_tree, p_branch
                USING ERRCODE = 'PF019';
        END IF;
        RAISE EXCEPTION '% was not branched from %', p_branch, p_tree USING ERRCODE = 'PF007';
    END IF;
    IF EXISTS (SELECT 1 FROM pgfs.mounts m WHERE m.tree IN (p_tree, p_branch) AND NOT m.read_only
               AND m.lease_until > now()) THEN
        RAISE EXCEPTION '% or % has a read-write mount', p_tree, p_branch USING ERRCODE = 'PF018';
    END IF;
    IF EXISTS (SELECT 1 FROM pgfs.snapshots s WHERE s.layer = b.layer) THEN
        RAISE EXCEPTION '% has snapshots: %', p_branch,
            (SELECT string_agg(s.tree || '@' || s.name, ', ') FROM pgfs.snapshots s WHERE s.layer = b.layer)
            USING ERRCODE = 'PF018';
    END IF;
    IF EXISTS (SELECT 1 FROM pgfs.layers l WHERE l.base_layer = b.layer) THEN
        RAISE EXCEPTION 'another branch is based on %', p_branch USING ERRCODE = 'PF018';
    END IF;
    IF NOT p_force AND (tl.epoch <> bl.base_epoch + 1
        OR EXISTS (SELECT 1 FROM pgfs.inodes WHERE layer = t.layer AND (born = tl.epoch OR died = tl.epoch))
        OR EXISTS (SELECT 1 FROM pgfs.dentries WHERE layer = t.layer AND (born = tl.epoch OR died = tl.epoch))
        OR EXISTS (SELECT 1 FROM pgfs.file_blocks WHERE layer = t.layer AND (born = tl.epoch OR died = tl.epoch))
        OR EXISTS (SELECT 1 FROM pgfs.xattrs WHERE layer = t.layer AND (born = tl.epoch OR died = tl.epoch))) THEN
        RAISE EXCEPTION '% changed since % was branched; use --force to replace it', p_tree, p_branch
            USING ERRCODE = 'PF019';
    END IF;

    -- Record the tree's previous contents and advance its epoch.
    e := tl.epoch;
    u := pgfs.layer_usage(t.layer);
    INSERT INTO pgfs.snapshots (tree, name, layer, epoch, xid, root_ino, logical_bytes, inodes)
    VALUES (p_tree, v_snap, t.layer, e, pg_current_xact_id(), t.root_ino, u.logical_bytes, u.inodes);
    UPDATE pgfs.layers SET epoch = e + 1 WHERE layer = t.layer;

    IF p_force THEN
        DELETE FROM pgfs.trees WHERE name = p_branch;
        UPDATE pgfs.trees SET layer = b.layer WHERE name = p_tree;
        rows_folded := 0;
        snapshot := v_snap;
        RETURN NEXT;
        RETURN;
    END IF;

    n := pgfs._fold(b.layer, t.layer, e + 1);
    -- One net change row per touched inode, under the promote's xid.
    FOR x IN SELECT DISTINCT ino FROM (
                 SELECT ino FROM pgfs.inodes WHERE layer = b.layer
                 UNION SELECT ino FROM pgfs.file_blocks WHERE layer = b.layer
                 UNION SELECT ino FROM pgfs.xattrs WHERE layer = b.layer
                 UNION SELECT ino FROM pgfs.dentries WHERE layer = b.layer AND ino IS NOT NULL) i
             ORDER BY ino LOOP
        PERFORM pgfs._change(t.layer, 'modify', x.ino,
            (SELECT kind FROM pgfs.inodes WHERE layer = t.layer AND ino = x.ino AND died = 9223372036854775807),
            NULL, NULL, pgfs._ino_path(t.layer, e + 1, x.ino));
    END LOOP;
    DELETE FROM pgfs.trees WHERE name = p_branch;
    -- The branch's live rows now live in the tree's layer; drop its copies
    -- without freeing their blobs, then its retired rows with theirs.
    DELETE FROM pgfs.file_blocks WHERE layer = b.layer AND died = 9223372036854775807;
    PERFORM pgfs._purge_layer(b.layer, 2147483647);
    DELETE FROM pgfs.change_consumers WHERE layer = b.layer;
    DELETE FROM pgfs.layers WHERE layer = b.layer;
    rows_folded := n;
    snapshot := v_snap;
    RETURN NEXT;
END $$;

-- Delete a snapshot name in constant time; its epoch stays retained for a
-- grace period (spec/branch.py DeletesSnapshotsAndTrees).
CREATE FUNCTION pgfs.snapshot_delete(p_tree text, p_name text, p_grace_secs double precision DEFAULT 90)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    s pgfs.snapshots;
    v_users text;
BEGIN
    SELECT * INTO s FROM pgfs.snapshots WHERE tree = p_tree AND name = p_name FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'snapshot %@% does not exist', p_tree, p_name USING ERRCODE = 'PF002';
    END IF;
    SELECT string_agg(t.name, ', ') INTO v_users FROM pgfs.layers l JOIN pgfs.trees t ON t.layer = l.layer
    WHERE l.base_layer = s.layer AND l.base_epoch = s.epoch;
    IF v_users IS NOT NULL THEN
        RAISE EXCEPTION 'snapshot %@% is the base of %', p_tree, p_name, v_users USING ERRCODE = 'PF018';
    END IF;
    SELECT string_agg(m.host || ' pid ' || m.pid, ', ') INTO v_users FROM pgfs.mounts m
    WHERE m.tree = p_tree AND m.snapshot = p_name AND m.lease_until > now();
    IF v_users IS NOT NULL THEN
        RAISE EXCEPTION 'snapshot %@% is mounted by %', p_tree, p_name, v_users USING ERRCODE = 'PF018';
    END IF;
    INSERT INTO pgfs.epoch_holds (layer, epoch, until)
    VALUES (s.layer, s.epoch, GREATEST(now(), COALESCE((SELECT max(lease_until) FROM pgfs.mounts m
                WHERE m.tree = p_tree AND m.snapshot = p_name), now()))
            + make_interval(secs => p_grace_secs))
    ON CONFLICT (layer, epoch) DO UPDATE SET until = GREATEST(pgfs.epoch_holds.until, EXCLUDED.until);
    DELETE FROM pgfs.mounts WHERE tree = p_tree AND snapshot = p_name;
    DELETE FROM pgfs.snapshots WHERE tree = p_tree AND name = p_name;
END $$;

-- Delete a tree in constant time; the reclaimer recovers its space later.
CREATE FUNCTION pgfs.tree_delete(p_tree text, p_grace_secs double precision DEFAULT 90)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    t pgfs.trees;
    v_users text;
    s record;
BEGIN
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'tree % does not exist', p_tree USING ERRCODE = 'PF002';
    END IF;
    PERFORM pgfs._gate_exclusive(t.layer);
    SELECT string_agg(m.host || ' pid ' || m.pid || CASE WHEN m.read_only THEN ' (read-only)' ELSE '' END, ', ')
    INTO v_users FROM pgfs.mounts m WHERE m.tree = p_tree AND m.lease_until > now();
    IF v_users IS NOT NULL THEN
        RAISE EXCEPTION 'tree % is mounted by %', p_tree, v_users USING ERRCODE = 'PF018';
    END IF;
    SELECT string_agg(x.name, ', ') INTO v_users
    FROM pgfs.layers l JOIN pgfs.trees x ON x.layer = l.layer
    WHERE x.name <> p_tree
      AND l.base_layer IN (SELECT layer FROM pgfs.snapshots WHERE tree = p_tree UNION SELECT t.layer);
    IF v_users IS NOT NULL THEN
        RAISE EXCEPTION 'tree % is the base of %', p_tree, v_users USING ERRCODE = 'PF018';
    END IF;
    FOR s IN SELECT * FROM pgfs.snapshots WHERE tree = p_tree LOOP
        INSERT INTO pgfs.epoch_holds (layer, epoch, until)
        VALUES (s.layer, s.epoch, now() + make_interval(secs => p_grace_secs))
        ON CONFLICT (layer, epoch) DO NOTHING;
    END LOOP;
    DELETE FROM pgfs.snapshots WHERE tree = p_tree;
    DELETE FROM pgfs.mounts WHERE tree = p_tree;
    DELETE FROM pgfs.change_consumers WHERE tree = p_tree;
    DELETE FROM pgfs.trees WHERE name = p_tree;
    UPDATE pgfs.layers SET state = 'deleting', deleting_since = now() + make_interval(secs => p_grace_secs)
    WHERE layer = t.layer
      AND NOT EXISTS (SELECT 1 FROM pgfs.snapshots x WHERE x.layer = t.layer);
END $$;

-- The inodes reachable from `p_root` in a view (a subtree branch).
CREATE FUNCTION pgfs._subtree(p_layer bigint, p_epoch bigint, p_root bigint)
RETURNS TABLE (ino bigint)
LANGUAGE plpgsql STABLE AS $$
#variable_conflict use_column
DECLARE
    v_queue bigint[] := ARRAY[p_root];
    v_dir bigint;
    x record;
BEGIN
    ino := p_root;
    RETURN NEXT;
    WHILE array_length(v_queue, 1) > 0 LOOP
        v_dir := v_queue[1];
        v_queue := v_queue[2:];
        FOR x IN SELECT * FROM pgfs.readdir(p_layer, p_epoch, v_dir, NULL, 2147483647) LOOP
            ino := x.ino;
            RETURN NEXT;
            IF x.kind = 'd' THEN
                v_queue := v_queue || x.ino;
            END IF;
        END LOOP;
    END LOOP;
END $$;

-- One flatten batch: copy up to p_limit visible base rows into the
-- branch's own layer with born = 0, so the branch can be detached
-- (spec/branch.py FlattensBranch). Returns the number of rows copied.
CREATE FUNCTION pgfs.flatten_batch(p_tree text, p_limit integer DEFAULT 10000)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    inf constant bigint := 9223372036854775807;
    t pgfs.trees;
    l pgfs.layers;
    n bigint := 0;
    k bigint;
    v_subtree boolean;
BEGIN
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'tree % does not exist', p_tree USING ERRCODE = 'PF002';
    END IF;
    PERFORM pg_advisory_xact_lock_shared(1885759091, t.layer::integer);
    SELECT * INTO l FROM pgfs.layers WHERE layer = t.layer;
    IF l.base_layer IS NULL THEN
        RETURN 0;
    END IF;
    v_subtree := t.root_ino <> 1;
    CREATE TEMP TABLE IF NOT EXISTS _pgfs_reach (ino bigint PRIMARY KEY) ON COMMIT DROP;
    IF v_subtree THEN
        INSERT INTO _pgfs_reach SELECT DISTINCT s.ino FROM pgfs._subtree(l.base_layer, l.base_epoch, t.root_ino) s
        ON CONFLICT DO NOTHING;
    END IF;

    -- Inodes.
    INSERT INTO pgfs.inodes
    SELECT t.layer, b.ino, b.gen, 0,
           COALESCE((SELECT min(o.born) FROM pgfs.inodes o WHERE o.layer = t.layer AND o.ino = b.ino AND o.born > 0), inf),
           b.kind, b.mode, b.uid, b.gid, b.nlink, b.size, b.atime_ns, b.mtime_ns, b.ctime_ns,
           b.btime_ns, b.rdev, b.inline_data, b.target, inf, false, b.version
    FROM (SELECT DISTINCT ON (i.ino) i.* FROM pgfs._chain(l.base_layer, l.base_epoch) c
          JOIN pgfs.inodes i ON i.layer = c.layer AND i.born <= c.epoch AND c.epoch < i.died
          WHERE NOT EXISTS (SELECT 1 FROM pgfs.inodes z WHERE z.layer = t.layer AND z.ino = i.ino AND z.born = 0)
            AND (NOT v_subtree OR i.ino IN (SELECT ino FROM _pgfs_reach))
          ORDER BY i.ino, c.depth) b
    WHERE (b.nlink > 0 OR EXISTS (SELECT 1 FROM pgfs.orphans o WHERE o.ino = b.ino))
    LIMIT p_limit
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    IF n >= p_limit THEN
        RETURN n;
    END IF;

    -- Dentries.
    INSERT INTO pgfs.dentries (layer, parent_ino, name, born, died, ino, kind, shadows_base)
    SELECT t.layer, b.parent_ino, b.name, 0,
           COALESCE((SELECT min(o.born) FROM pgfs.dentries o WHERE o.layer = t.layer
                       AND o.parent_ino = b.parent_ino AND o.name = b.name AND o.born > 0), inf),
           b.ino, b.kind, false
    FROM (SELECT DISTINCT ON (d.parent_ino, d.name) d.* FROM pgfs._chain(l.base_layer, l.base_epoch) c
          JOIN pgfs.dentries d ON d.layer = c.layer AND d.born <= c.epoch AND c.epoch < d.died
          WHERE NOT EXISTS (SELECT 1 FROM pgfs.dentries z WHERE z.layer = t.layer
                              AND z.parent_ino = d.parent_ino AND z.name = d.name AND z.born = 0)
            AND (NOT v_subtree OR d.parent_ino IN (SELECT ino FROM _pgfs_reach))
          ORDER BY d.parent_ino, d.name, c.depth) b
    WHERE b.ino IS NOT NULL
    LIMIT p_limit - n
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    IF n >= p_limit THEN
        RETURN n;
    END IF;

    -- Xattrs.
    INSERT INTO pgfs.xattrs (layer, ino, name, value, born, died, shadows_base)
    SELECT t.layer, b.ino, b.name, b.value, 0,
           COALESCE((SELECT min(o.born) FROM pgfs.xattrs o WHERE o.layer = t.layer
                       AND o.ino = b.ino AND o.name = b.name AND o.born > 0), inf),
           false
    FROM (SELECT DISTINCT ON (x.ino, x.name) x.* FROM pgfs._chain(l.base_layer, l.base_epoch) c
          JOIN pgfs.xattrs x ON x.layer = c.layer AND x.born <= c.epoch AND c.epoch < x.died
          WHERE NOT EXISTS (SELECT 1 FROM pgfs.xattrs z WHERE z.layer = t.layer
                              AND z.ino = x.ino AND z.name = x.name AND z.born = 0)
            AND (NOT v_subtree OR x.ino IN (SELECT ino FROM _pgfs_reach))
          ORDER BY x.ino, x.name, c.depth) b
    WHERE b.value IS NOT NULL
    LIMIT p_limit - n
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    IF n >= p_limit THEN
        RETURN n;
    END IF;

    -- Block map: an inherited block ends where the branch first wrote it
    -- or first lowered its inode's inherit_limit below it. Data is shared,
    -- never copied.
    CREATE TEMP TABLE IF NOT EXISTS _pgfs_flat_blocks (ino bigint, block_no bigint, blob_id bigint, died bigint)
        ON COMMIT DROP;
    TRUNCATE _pgfs_flat_blocks;
    INSERT INTO _pgfs_flat_blocks
    SELECT b.ino, b.block_no, b.blob_id,
           LEAST(COALESCE((SELECT min(o.born) FROM pgfs.file_blocks o WHERE o.layer = t.layer
                             AND o.ino = b.ino AND o.block_no = b.block_no AND o.born > 0), inf),
                 COALESCE((SELECT min(o.born) FROM pgfs.inodes o WHERE o.layer = t.layer
                             AND o.ino = b.ino AND o.born > 0 AND o.inherit_limit <= b.block_no), inf))
    FROM (SELECT DISTINCT ON (f.ino, f.block_no) f.* FROM pgfs._chain(l.base_layer, l.base_epoch) c
          JOIN pgfs.file_blocks f ON f.layer = c.layer AND f.born <= c.epoch AND c.epoch < f.died
          WHERE NOT EXISTS (SELECT 1 FROM pgfs.file_blocks z WHERE z.layer = t.layer
                              AND z.ino = f.ino AND z.block_no = f.block_no AND z.born = 0)
            AND (NOT v_subtree OR f.ino IN (SELECT ino FROM _pgfs_reach))
          ORDER BY f.ino, f.block_no, c.depth) b
    WHERE b.blob_id IS NOT NULL
    LIMIT p_limit - n;
    UPDATE pgfs.blobs SET shared = true
    WHERE blob_id IN (SELECT blob_id FROM _pgfs_flat_blocks) AND NOT shared;
    INSERT INTO pgfs.file_blocks (layer, ino, block_no, born, died, blob_id)
    SELECT t.layer, ino, block_no, 0, died, blob_id FROM _pgfs_flat_blocks
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS k = ROW_COUNT;
    n := n + k;
    RETURN n;
END $$;

-- Finish a flatten when no rows remain to rebase: detach the branch from
-- its base. Returns false if a batch is still needed.
CREATE FUNCTION pgfs.flatten_finish(p_tree text) RETURNS boolean
LANGUAGE plpgsql AS $$
DECLARE
    t pgfs.trees;
    l pgfs.layers;
BEGIN
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'tree % does not exist', p_tree USING ERRCODE = 'PF002';
    END IF;
    PERFORM pgfs._gate_exclusive(t.layer);
    SELECT * INTO l FROM pgfs.layers WHERE layer = t.layer FOR UPDATE;
    IF l.base_layer IS NULL THEN
        RETURN true;
    END IF;
    IF pgfs.flatten_batch(p_tree, 1) > 0 THEN
        RETURN false;
    END IF;
    UPDATE pgfs.layers SET base_layer = NULL, base_epoch = NULL, depth = 0 WHERE layer = t.layer;
    WITH RECURSIVE kids(layer, depth) AS (
        SELECT x.layer, 1 FROM pgfs.layers x WHERE x.base_layer = t.layer
        UNION ALL
        SELECT x.layer, k.depth + 1 FROM pgfs.layers x JOIN kids k ON x.base_layer = k.layer
    )
    UPDATE pgfs.layers x SET depth = k.depth FROM kids k WHERE x.layer = k.layer;
    DELETE FROM pgfs.dentries WHERE layer = t.layer AND ino IS NULL;
    DELETE FROM pgfs.xattrs WHERE layer = t.layer AND value IS NULL;
    DELETE FROM pgfs.file_blocks WHERE layer = t.layer AND blob_id IS NULL;
    DELETE FROM pgfs.inodes i WHERE i.layer = t.layer AND i.nlink = 0
      AND NOT EXISTS (SELECT 1 FROM pgfs.orphans o WHERE o.layer = t.layer AND o.ino = i.ino);
    UPDATE pgfs.dentries SET shadows_base = false WHERE layer = t.layer AND shadows_base;
    UPDATE pgfs.xattrs SET shadows_base = false WHERE layer = t.layer AND shadows_base;
    UPDATE pgfs.inodes SET shadows_base = false WHERE layer = t.layer AND shadows_base;
    UPDATE pgfs.inodes SET inherit_limit = 9223372036854775807
    WHERE layer = t.layer AND died = 9223372036854775807 AND inherit_limit <> 9223372036854775807;
    RETURN true;
END $$;

-- What changed from view A to view B, computed from epochs rather than
-- from the change feed (spec/branch.py SnapshotDiff). The views are two
-- epochs of one layer, or a branch and the layer it is based on (in
-- either order). Only keys whose rows changed between the views are
-- examined, so the cost is proportional to the change. A non-directory is
-- `modified` when its content (mtime or size), mode, owner or symlink
-- target differs.
CREATE FUNCTION pgfs.diff_views(p_la bigint, p_ea bigint, p_lb bigint, p_eb bigint)
RETURNS TABLE (change text, ino bigint, kind "char", path bytea, old_path bytea)
LANGUAGE plpgsql STABLE AS $$
#variable_conflict use_column
DECLARE
    v_root_a bigint := pgfs._root_of(p_la);
    v_root_b bigint := pgfs._root_of(p_lb);
    v_branch bigint;       -- the branch layer (all of its rows are keys)
    v_base bigint;         -- the layer it is based on
    v_lo bigint;           -- base-layer rows changed in (v_lo, v_hi] are keys
    v_hi bigint;
    l pgfs.layers;
    x record;
    a pgfs.dentries;
    b pgfs.dentries;
    n pgfs.dentries;
    ia pgfs.inodes;
    ib pgfs.inodes;
BEGIN
    IF p_la = p_lb THEN
        v_branch := NULL;
        v_base := p_la;
        v_lo := LEAST(p_ea, p_eb);
        v_hi := GREATEST(p_ea, p_eb);
    ELSE
        SELECT * INTO l FROM pgfs.layers WHERE layer = p_lb;
        IF l.base_layer = p_la THEN
            v_branch := p_lb;
            v_lo := LEAST(p_ea, l.base_epoch);
            v_hi := GREATEST(p_ea, l.base_epoch);
        ELSE
            SELECT * INTO l FROM pgfs.layers WHERE layer = p_la;
            IF l.base_layer = p_lb THEN
                v_branch := p_la;
                v_lo := LEAST(p_eb, l.base_epoch);
                v_hi := GREATEST(p_eb, l.base_epoch);
            ELSE
                RAISE EXCEPTION 'the two views are not related: neither is the other''s branch'
                    USING ERRCODE = 'PF007';
            END IF;
        END IF;
        v_base := l.base_layer;
    END IF;

    -- Names.
    FOR x IN SELECT DISTINCT d.parent_ino, d.name FROM pgfs.dentries d
             WHERE (d.layer = v_base AND ((d.born > v_lo AND d.born <= v_hi) OR (d.died > v_lo AND d.died <= v_hi)))
                OR d.layer = v_branch
             ORDER BY 1, 2 LOOP
        a := pgfs._dentry(p_la, p_ea, x.parent_ino, x.name);
        b := pgfs._dentry(p_lb, p_eb, x.parent_ino, x.name);
        CONTINUE WHEN a.ino IS NOT DISTINCT FROM b.ino;
        IF a.ino IS NOT NULL AND pgfs._name_of(p_lb, p_eb, a.ino) IS NULL THEN
            change := 'removed';
            ino := a.ino;
            kind := a.kind;
            path := pgfs._join(pgfs._path(p_la, p_ea, x.parent_ino, v_root_a), x.name);
            old_path := NULL;
            RETURN NEXT;
        END IF;
        IF b.ino IS NOT NULL THEN
            n := pgfs._name_of(p_la, p_ea, b.ino);
            IF n IS NULL THEN
                change := 'added';
                old_path := NULL;
            ELSE
                change := 'renamed';
                old_path := pgfs._join(pgfs._path(p_la, p_ea, n.parent_ino, v_root_a), n.name);
            END IF;
            ino := b.ino;
            kind := b.kind;
            path := pgfs._join(pgfs._path(p_lb, p_eb, x.parent_ino, v_root_b), x.name);
            RETURN NEXT;
        END IF;
    END LOOP;

    -- Contents and attributes of inodes present in both views.
    FOR x IN SELECT DISTINCT i FROM (
                 SELECT r.ino AS i FROM pgfs.inodes r
                 WHERE (r.layer = v_base AND ((r.born > v_lo AND r.born <= v_hi) OR (r.died > v_lo AND r.died <= v_hi)))
                    OR r.layer = v_branch
                 UNION
                 SELECT f.ino FROM pgfs.file_blocks f
                 WHERE (f.layer = v_base AND ((f.born > v_lo AND f.born <= v_hi) OR (f.died > v_lo AND f.died <= v_hi)))
                    OR f.layer = v_branch
                 UNION
                 SELECT t.ino FROM pgfs.xattrs t
                 WHERE (t.layer = v_base AND ((t.born > v_lo AND t.born <= v_hi) OR (t.died > v_lo AND t.died <= v_hi)))
                    OR t.layer = v_branch) s
             ORDER BY 1 LOOP
        ia := pgfs._inode(p_la, p_ea, x.i);
        ib := pgfs._inode(p_lb, p_eb, x.i);
        -- Directories change through their names, reported above; a rename
        -- alone (ctime only) is not a modification.
        CONTINUE WHEN ia IS NULL OR ib IS NULL OR ib.kind = 'd';
        CONTINUE WHEN ia.mtime_ns = ib.mtime_ns AND ia.size = ib.size AND ia.mode = ib.mode
                  AND ia.uid = ib.uid AND ia.gid = ib.gid
                  AND ia.target IS NOT DISTINCT FROM ib.target;
        change := 'modified';
        ino := x.i;
        kind := ib.kind;
        path := pgfs._ino_path(p_lb, p_eb, x.i);
        old_path := NULL;
        RETURN NEXT;
    END LOOP;
END $$;

-- Two epochs of one layer.
CREATE FUNCTION pgfs.diff(p_layer bigint, p_from bigint, p_to bigint)
RETURNS TABLE (change text, ino bigint, kind "char", path bytea, old_path bytea)
LANGUAGE sql STABLE AS $$
    SELECT * FROM pgfs.diff_views(p_layer, p_from, p_layer, p_to)
$$;

-- Control requests for an owning daemon (spec/branch.py DrainsOwnerBeforeSnapshot).
CREATE FUNCTION pgfs.control_submit(p_tree text, p_kind text, p_args jsonb)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    v_id bigint;
BEGIN
    INSERT INTO pgfs.control_requests (tree, kind, args) VALUES (p_tree, p_kind, p_args)
    RETURNING request_id INTO v_id;
    PERFORM pg_notify('pgfs_control', p_tree);
    RETURN v_id;
END $$;

-- Cancel a pending request; returns true if it was still pending.
CREATE FUNCTION pgfs.control_cancel(p_request bigint) RETURNS boolean
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE pgfs.control_requests SET state = 'cancelled' WHERE request_id = p_request AND state = 'pending';
    RETURN FOUND;
END $$;

CREATE FUNCTION pgfs.control_status(p_request bigint, OUT state text, OUT result jsonb)
LANGUAGE sql STABLE AS $$
    SELECT state, result FROM pgfs.control_requests WHERE request_id = p_request
$$;

-- Executed by the owning daemon after its drain barrier, within its fence.
CREATE FUNCTION pgfs.control_execute(p_request bigint, p_mount uuid, p_fence bigint)
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE
    r pgfs.control_requests;
    v_result jsonb;
BEGIN
    SELECT * INTO r FROM pgfs.control_requests WHERE request_id = p_request FOR UPDATE;
    IF NOT FOUND OR r.state <> 'pending' THEN
        RETURN NULL;
    END IF;
    BEGIN
        IF r.kind = 'snapshot' THEN
            SELECT to_jsonb(x) INTO v_result
            FROM pgfs.snapshot(r.tree, r.args->>'name', p_mount, p_fence) x;
        ELSE
            SELECT to_jsonb(x) INTO v_result
            FROM pgfs.branch(r.tree, NULL, decode(r.args->>'path', 'hex'), r.args->>'new',
                             p_mount, p_fence, COALESCE((r.args->>'max_depth')::integer, 8)) x;
        END IF;
        UPDATE pgfs.control_requests SET state = 'done', result = v_result WHERE request_id = p_request;
    EXCEPTION WHEN OTHERS THEN
        v_result := jsonb_build_object('sqlstate', SQLSTATE, 'message', SQLERRM);
        UPDATE pgfs.control_requests SET state = 'failed', result = v_result WHERE request_id = p_request;
    END;
    RETURN v_result;
END $$;

-- Pending control requests for a tree.
CREATE FUNCTION pgfs.control_pending(p_tree text)
RETURNS SETOF bigint LANGUAGE sql STABLE AS $$
    SELECT request_id FROM pgfs.control_requests WHERE tree = p_tree AND state = 'pending'
    ORDER BY request_id
$$;

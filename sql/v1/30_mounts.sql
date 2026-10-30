-- pgfs schema version 1: mount sessions, leases and fencing
-- (spec/consistency.py ExclusiveTreeLease, FencesStaleWriters,
-- ReadOnlyLiveMounts; spec/branch.py MountsSnapshots).

CREATE FUNCTION pgfs._gate_exclusive(p_layer bigint) RETURNS void
LANGUAGE sql AS $$
    SELECT pg_advisory_xact_lock(1885759091, p_layer::integer)
$$;

-- Register a mount session. A read-write session is the tree's lease:
-- it succeeds only if no unexpired read-write session exists, replaces an
-- expired one (with its op_log slots), takes a new fence and pre-inserts
-- one op_log slot per worker. Commits synchronously.
CREATE FUNCTION pgfs.mount_acquire(p_tree text, p_snapshot text, p_read_only boolean,
                                   p_host text, p_pid integer, p_workers integer,
                                   p_lease_secs double precision)
RETURNS TABLE (mount_id uuid, fence bigint, layer bigint, epoch bigint, root_ino bigint,
               scratch boolean)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    t pgfs.trees;
    s pgfs.snapshots;
    old pgfs.mounts;
    v_id uuid := gen_random_uuid();
    v_fence bigint := nextval('pgfs.fence_seq');
    v_layer bigint;
    v_epoch bigint;
    v_root bigint;
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF p_snapshot IS NOT NULL THEN
        SELECT * INTO s FROM pgfs.snapshots WHERE tree = p_tree AND name = p_snapshot;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'snapshot %@% does not exist', p_tree, p_snapshot USING ERRCODE = 'PF002';
        END IF;
        v_layer := s.layer;
        v_epoch := s.epoch;
        v_root := s.root_ino;
        p_read_only := true;
    ELSE
        IF t.name IS NULL THEN
            RAISE EXCEPTION 'tree % does not exist', p_tree USING ERRCODE = 'PF002';
        END IF;
        v_layer := t.layer;
        v_epoch := NULL;
        v_root := t.root_ino;
    END IF;
    IF NOT p_read_only THEN
        PERFORM pgfs._gate_exclusive(v_layer);
        SELECT * INTO old FROM pgfs.mounts m WHERE m.tree = p_tree AND NOT m.read_only FOR UPDATE;
        IF FOUND THEN
            IF old.lease_until > now() THEN
                RAISE EXCEPTION 'tree % is mounted read-write by % pid % until %',
                    p_tree, old.host, old.pid, old.lease_until USING ERRCODE = 'PF018';
            END IF;
            DELETE FROM pgfs.mounts m WHERE m.mount_id = old.mount_id;
        END IF;
    END IF;
    INSERT INTO pgfs.mounts (mount_id, tree, snapshot, layer, read_only, fence, host, pid,
                             workers, lease_until)
    VALUES (v_id, p_tree, p_snapshot, v_layer, p_read_only, v_fence, p_host, p_pid,
            CASE WHEN p_read_only THEN 0 ELSE p_workers END,
            now() + make_interval(secs => p_lease_secs));
    IF NOT p_read_only THEN
        INSERT INTO pgfs.op_log (mount_id, worker_id, op_seq, result)
        SELECT v_id, w, 0, NULL FROM generate_series(0, p_workers - 1) w;
    END IF;
    mount_id := v_id;
    fence := v_fence;
    layer := v_layer;
    epoch := v_epoch;
    root_ino := v_root;
    scratch := COALESCE((SELECT value = 'true' FROM pgfs.meta WHERE key = 'scratch'), false);
    RETURN NEXT;
END $$;

-- Renew a session; PF001 if the exact (mount_id, fence) row is gone.
-- Returns the schema version so running mounts notice a migration.
CREATE FUNCTION pgfs.mount_renew(p_mount uuid, p_fence bigint, p_lease_secs double precision)
RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE pgfs.mounts SET lease_until = now() + make_interval(secs => p_lease_secs)
    WHERE mount_id = p_mount AND fence = p_fence;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'mount % (fence %) has no session', p_mount, p_fence USING ERRCODE = 'PF001';
    END IF;
    RETURN (SELECT value FROM pgfs.meta WHERE key = 'schema_version');
END $$;

-- Release a session at clean shutdown, synchronously (spec/app.py MountsCleanly).
CREATE FUNCTION pgfs.mount_release(p_mount uuid, p_fence bigint)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    DELETE FROM pgfs.mounts WHERE mount_id = p_mount AND fence = p_fence;
END $$;

-- The mount-wide revalidation after a reconnect, in one round trip
-- (spec/failure.py RevalidatesAfterReconnect).
CREATE FUNCTION pgfs.revalidate(p_mount uuid, p_fence bigint)
RETURNS TABLE (schema_version text, fs_uuid text, session_ok boolean, root_exists boolean,
               snapshot_ok boolean, slots jsonb)
LANGUAGE plpgsql STABLE AS $$
#variable_conflict use_column
DECLARE
    m pgfs.mounts;
BEGIN
    SELECT * INTO m FROM pgfs.mounts WHERE mount_id = p_mount AND fence = p_fence;
    schema_version := (SELECT value FROM pgfs.meta WHERE key = 'schema_version');
    fs_uuid := (SELECT value FROM pgfs.meta WHERE key = 'fs_uuid');
    session_ok := m.mount_id IS NOT NULL;
    root_exists := EXISTS (SELECT 1 FROM pgfs.inodes LIMIT 1);
    snapshot_ok := m.snapshot IS NULL
        OR EXISTS (SELECT 1 FROM pgfs.snapshots s WHERE s.tree = m.tree AND s.name = m.snapshot);
    slots := COALESCE((SELECT jsonb_object_agg(worker_id::text, op_seq)
                       FROM pgfs.op_log WHERE mount_id = p_mount), '{}'::jsonb);
    RETURN NEXT;
END $$;

-- The op_log slot of one worker (spec/failure.py IdempotentMutations).
CREATE FUNCTION pgfs.op_slot(p_mount uuid, p_worker integer, OUT op_seq bigint, OUT result jsonb)
LANGUAGE sql STABLE AS $$
    SELECT op_seq, result FROM pgfs.op_log WHERE mount_id = p_mount AND worker_id = p_worker
$$;

-- Reap expired read-only and snapshot sessions. Read-write sessions are
-- never reaped (spec/consistency.py ExclusiveTreeLease).
CREATE FUNCTION pgfs.reap_sessions() RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
    n integer;
BEGIN
    DELETE FROM pgfs.mounts WHERE read_only AND lease_until < now();
    GET DIAGNOSTICS n = ROW_COUNT;
    RETURN n;
END $$;

-- Re-seed a scratch filesystem whose unlogged tables were emptied by a
-- crash or failover (spec/durability.py ScratchMode). Not DDL.
CREATE FUNCTION pgfs.scratch_reset() RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
    tr record;
    n integer := 0;
    v_now bigint := (extract(epoch FROM clock_timestamp()) * 1000000000)::bigint;
BEGIN
    IF COALESCE((SELECT value FROM pgfs.meta WHERE key = 'scratch'), 'false') <> 'true' THEN
        RAISE EXCEPTION 'not a scratch filesystem' USING ERRCODE = 'PF007';
    END IF;
    IF EXISTS (SELECT 1 FROM pgfs.inodes) THEN
        RETURN 0;
    END IF;
    DELETE FROM pgfs.snapshots;
    DELETE FROM pgfs.epoch_holds;
    DELETE FROM pgfs.control_requests;
    UPDATE pgfs.layers SET base_layer = NULL, base_epoch = NULL;
    DELETE FROM pgfs.layers l WHERE NOT EXISTS (SELECT 1 FROM pgfs.trees t WHERE t.layer = l.layer);
    UPDATE pgfs.layers SET depth = 0, epoch = 1, state = 'active', deleting_since = NULL;
    UPDATE pgfs.trees SET root_ino = 1;
    UPDATE pgfs.change_consumers SET watermark = jsonb_build_object(
        'layer', layer, 'low', pg_snapshot_xmin(pg_current_snapshot())::text,
        'high', pg_snapshot_xmin(pg_current_snapshot())::text, 's_cap', NULL, 'last_seq', 0,
        'held', '[]'::jsonb);
    UPDATE pgfs.scrub_state SET position = 0;
    FOR tr IN SELECT * FROM pgfs.trees LOOP
        INSERT INTO pgfs.inodes (layer, ino, gen, born, kind, mode, uid, gid, nlink,
                                 atime_ns, mtime_ns, ctime_ns, btime_ns)
        SELECT tr.layer, 1, pgfs._new_gen(), 1, 'd', mode, uid, gid, 2, v_now, v_now, v_now, v_now
        FROM (SELECT COALESCE((SELECT value::integer FROM pgfs.meta WHERE key = 'root_mode'), 493) AS mode,
                     COALESCE((SELECT value::bigint FROM pgfs.meta WHERE key = 'root_uid'), 0) AS uid,
                     COALESCE((SELECT value::bigint FROM pgfs.meta WHERE key = 'root_gid'), 0) AS gid) x;
        n := n + 1;
    END LOOP;
    RAISE WARNING 'pgfs: scratch filesystem contents were lost; % tree(s) reset', n;
    RETURN n;
END $$;

-- The synchronous barrier of a CLI command, which has no op_log slot
-- (spec/durability.py FsyncIsDurable).
CREATE FUNCTION pgfs.cli_barrier() RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    UPDATE pgfs.scrub_state SET position = position WHERE id = 1;
END $$;

-- Scrub progress (spec/integrity.py Scrub).
CREATE FUNCTION pgfs.scrub_batch(p_after bigint, p_limit integer)
RETURNS TABLE (blob_id bigint, crc32c integer, data bytea)
LANGUAGE sql STABLE AS $$
    SELECT b.blob_id, b.crc32c, b.data FROM pgfs.blobs b
    WHERE b.blob_id > p_after ORDER BY b.blob_id LIMIT p_limit
$$;

CREATE FUNCTION pgfs.scrub_progress(p_position bigint, p_completed boolean) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE pgfs.scrub_state SET
        position = CASE WHEN p_completed THEN 0 ELSE p_position END,
        pass_started = CASE WHEN position = 0 AND NOT p_completed THEN now() ELSE pass_started END,
        pass_completed = CASE WHEN p_completed THEN now() ELSE pass_completed END
    WHERE id = 1;
END $$;

CREATE FUNCTION pgfs.scrub_position() RETURNS bigint
LANGUAGE sql STABLE AS $$
    SELECT position FROM pgfs.scrub_state WHERE id = 1
$$;

-- Which files reference a blob, for error reports.
CREATE FUNCTION pgfs.blob_users(p_blob bigint)
RETURNS TABLE (layer bigint, ino bigint, block_no bigint, path bytea)
LANGUAGE sql STABLE AS $$
    SELECT f.layer, f.ino, f.block_no,
           pgfs._ino_path(f.layer, (SELECT epoch FROM pgfs.layers l WHERE l.layer = f.layer), f.ino)
    FROM pgfs.file_blocks f WHERE f.blob_id = p_blob AND f.died = 9223372036854775807
$$;

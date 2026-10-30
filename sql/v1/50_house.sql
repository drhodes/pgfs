-- pgfs schema version 1: space accounting and housekeeping
-- (spec/space.py, spec/consistency.py HousekeepingInSeparateTransactions).

-- ── Usage deltas, appended by statement triggers ───────────────────────
-- Logical bytes and inodes count each layer's own live rows; physical
-- bytes count stored blob bytes, inline data and a fixed per-inode-row
-- overhead of 256 bytes across the filesystem (spec/space.py TracksUsage).

CREATE FUNCTION pgfs._usage_inodes() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO pgfs.usage_deltas (layer, logical_bytes, physical_bytes, inodes)
        SELECT layer,
               sum(CASE WHEN died = 9223372036854775807 AND nlink > 0 AND kind = 'f' THEN size ELSE 0 END),
               sum(256 + COALESCE(octet_length(inline_data), 0) + COALESCE(octet_length(target), 0)),
               sum(CASE WHEN died = 9223372036854775807 AND nlink > 0 THEN 1 ELSE 0 END)
        FROM new_rows GROUP BY layer;
    ELSIF TG_OP = 'DELETE' THEN
        INSERT INTO pgfs.usage_deltas (layer, logical_bytes, physical_bytes, inodes)
        SELECT layer,
               -sum(CASE WHEN died = 9223372036854775807 AND nlink > 0 AND kind = 'f' THEN size ELSE 0 END),
               -sum(256 + COALESCE(octet_length(inline_data), 0) + COALESCE(octet_length(target), 0)),
               -sum(CASE WHEN died = 9223372036854775807 AND nlink > 0 THEN 1 ELSE 0 END)
        FROM old_rows GROUP BY layer;
    ELSE
        INSERT INTO pgfs.usage_deltas (layer, logical_bytes, physical_bytes, inodes)
        SELECT layer, sum(lb), sum(pb), sum(n) FROM (
            SELECT layer,
                   CASE WHEN died = 9223372036854775807 AND nlink > 0 AND kind = 'f' THEN size ELSE 0 END AS lb,
                   COALESCE(octet_length(inline_data), 0) + COALESCE(octet_length(target), 0) AS pb,
                   CASE WHEN died = 9223372036854775807 AND nlink > 0 THEN 1 ELSE 0 END AS n
            FROM new_rows
            UNION ALL
            SELECT layer,
                   -CASE WHEN died = 9223372036854775807 AND nlink > 0 AND kind = 'f' THEN size ELSE 0 END,
                   -(COALESCE(octet_length(inline_data), 0) + COALESCE(octet_length(target), 0)),
                   -CASE WHEN died = 9223372036854775807 AND nlink > 0 THEN 1 ELSE 0 END
            FROM old_rows) x
        GROUP BY layer
        HAVING sum(lb) <> 0 OR sum(pb) <> 0 OR sum(n) <> 0;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER inodes_usage_ins AFTER INSERT ON pgfs.inodes
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pgfs._usage_inodes();
CREATE TRIGGER inodes_usage_upd AFTER UPDATE ON pgfs.inodes
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pgfs._usage_inodes();
CREATE TRIGGER inodes_usage_del AFTER DELETE ON pgfs.inodes
    REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION pgfs._usage_inodes();

CREATE FUNCTION pgfs._usage_blobs() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO pgfs.usage_deltas (layer, physical_bytes)
        SELECT 0, sum(len) FROM new_rows HAVING count(*) > 0;
    ELSE
        INSERT INTO pgfs.usage_deltas (layer, physical_bytes)
        SELECT 0, -sum(len) FROM old_rows HAVING count(*) > 0;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER blobs_usage_ins AFTER INSERT ON pgfs.blobs
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pgfs._usage_blobs();
CREATE TRIGGER blobs_usage_del AFTER DELETE ON pgfs.blobs
    REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION pgfs._usage_blobs();

-- Fold the deltas into the totals (every 10 s from housekeeping).
CREATE FUNCTION pgfs.fold_usage() RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
    n integer;
BEGIN
    CREATE TEMP TABLE _pgfs_folded ON COMMIT DROP AS
    WITH d AS (DELETE FROM pgfs.usage_deltas RETURNING *)
    SELECT layer, logical_bytes, physical_bytes, inodes FROM d;
    GET DIAGNOSTICS n = ROW_COUNT;
    INSERT INTO pgfs.usage_totals (layer, logical_bytes, inodes)
    SELECT layer, sum(logical_bytes), sum(inodes) FROM _pgfs_folded WHERE layer <> 0 GROUP BY layer
    ON CONFLICT (layer) DO UPDATE SET logical_bytes = pgfs.usage_totals.logical_bytes + EXCLUDED.logical_bytes,
                                      inodes = pgfs.usage_totals.inodes + EXCLUDED.inodes;
    INSERT INTO pgfs.usage_totals (layer, physical_bytes)
    SELECT 0, COALESCE(sum(physical_bytes), 0) FROM _pgfs_folded
    ON CONFLICT (layer) DO UPDATE SET physical_bytes = pgfs.usage_totals.physical_bytes + EXCLUDED.physical_bytes;
    DROP TABLE _pgfs_folded;
    RETURN n;
END $$;

-- ── Reclaiming space ───────────────────────────────────────────────────

-- The retained epochs of a layer (spec/branch.py RetainedEpochs).
CREATE FUNCTION pgfs.retained_epochs(p_layer bigint) RETURNS bigint[]
LANGUAGE sql STABLE AS $$
    SELECT array_agg(DISTINCT e) FROM (
        SELECT epoch AS e FROM pgfs.snapshots WHERE layer = p_layer
        UNION SELECT base_epoch FROM pgfs.layers WHERE base_layer = p_layer AND state = 'active'
        UNION SELECT epoch FROM pgfs.epoch_holds WHERE layer = p_layer AND until > now()
        UNION SELECT epoch FROM pgfs.layers WHERE layer = p_layer) x
$$;

-- Drop a blob reference: free the blob when nothing references it.
CREATE FUNCTION pgfs._blob_unref(p_blob bigint) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    IF p_blob IS NOT NULL AND NOT EXISTS (SELECT 1 FROM pgfs.file_blocks WHERE blob_id = p_blob) THEN
        PERFORM pgfs._blob_release(p_blob);
    END IF;
END $$;

-- One reclaim batch (spec/space.py ReclaimsRetiredRows): retired rows no
-- retained epoch can see, map rows of deleted inodes or past the end of a
-- file, and whole layers being deleted. Returns rows removed.
CREATE FUNCTION pgfs.reclaim_batch(p_limit integer DEFAULT 10000) RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE
    l record;
    v_ret bigint[];
    n bigint := 0;
    k bigint;
    b record;
    i record;
    r pgfs.inodes;
    e bigint;
    bs bigint := pgfs._meta_int('block_size');
BEGIN
    DELETE FROM pgfs.epoch_holds WHERE until <= now();

    -- Layers nothing names any more become garbage.
    UPDATE pgfs.layers x SET state = 'deleting', deleting_since = now()
    WHERE x.state = 'active'
      AND NOT EXISTS (SELECT 1 FROM pgfs.trees t WHERE t.layer = x.layer)
      AND NOT EXISTS (SELECT 1 FROM pgfs.snapshots s WHERE s.layer = x.layer)
      AND NOT EXISTS (SELECT 1 FROM pgfs.epoch_holds h WHERE h.layer = x.layer)
      AND NOT EXISTS (SELECT 1 FROM pgfs.layers c WHERE c.base_layer = x.layer AND c.state = 'active')
      AND NOT EXISTS (SELECT 1 FROM pgfs.mounts m WHERE m.layer = x.layer AND m.lease_until > now());

    -- Layers being deleted, once their grace period is over.
    FOR l IN SELECT * FROM pgfs.layers x
             WHERE x.state = 'deleting' AND x.deleting_since <= now()
               AND NOT EXISTS (SELECT 1 FROM pgfs.mounts m WHERE m.layer = x.layer AND m.lease_until > now())
             ORDER BY x.layer LOOP
        PERFORM pg_advisory_xact_lock(1885759091, l.layer::integer);
        k := pgfs._purge_layer(l.layer, p_limit);
        n := n + k;
        IF k = 0 AND NOT EXISTS (SELECT 1 FROM pgfs.layers c WHERE c.base_layer = l.layer) THEN
            DELETE FROM pgfs.snapshots WHERE layer = l.layer;
            DELETE FROM pgfs.epoch_holds WHERE layer = l.layer;
            DELETE FROM pgfs.mounts WHERE layer = l.layer;
            DELETE FROM pgfs.layers WHERE layer = l.layer;
            DELETE FROM pgfs.usage_totals WHERE layer = l.layer;
            INSERT INTO pgfs.usage_deltas (layer, logical_bytes, inodes)
            SELECT l.layer, -sum(logical_bytes), -sum(inodes) FROM pgfs.usage_deltas WHERE layer = l.layer
            HAVING count(*) > 0;
        END IF;
        IF n >= p_limit THEN
            RETURN n;
        END IF;
    END LOOP;

    -- Retired rows of active layers.
    FOR l IN SELECT * FROM pgfs.layers WHERE state = 'active' ORDER BY layer LOOP
        PERFORM pg_advisory_xact_lock_shared(1885759091, l.layer::integer);
        SELECT epoch INTO e FROM pgfs.layers WHERE layer = l.layer;
        v_ret := pgfs.retained_epochs(l.layer);
        FOR b IN DELETE FROM pgfs.file_blocks f
                 WHERE (f.layer, f.ino, f.block_no, f.born) IN (
                     SELECT x.layer, x.ino, x.block_no, x.born FROM pgfs.file_blocks x
                     WHERE x.layer = l.layer AND x.died <> 9223372036854775807
                       AND NOT EXISTS (SELECT 1 FROM unnest(v_ret) re WHERE x.born <= re AND re < x.died)
                     LIMIT p_limit - n)
                 RETURNING f.blob_id LOOP
            n := n + 1;
            PERFORM pgfs._blob_unref(b.blob_id);
        END LOOP;
        DELETE FROM pgfs.dentries WHERE (layer, parent_ino, name, born) IN (
            SELECT x.layer, x.parent_ino, x.name, x.born FROM pgfs.dentries x
            WHERE x.layer = l.layer AND x.died <> 9223372036854775807
              AND NOT EXISTS (SELECT 1 FROM unnest(v_ret) re WHERE x.born <= re AND re < x.died)
            LIMIT GREATEST(p_limit - n, 0));
        GET DIAGNOSTICS k = ROW_COUNT;
        n := n + k;
        DELETE FROM pgfs.xattrs WHERE (layer, ino, name, born) IN (
            SELECT x.layer, x.ino, x.name, x.born FROM pgfs.xattrs x
            WHERE x.layer = l.layer AND x.died <> 9223372036854775807
              AND NOT EXISTS (SELECT 1 FROM unnest(v_ret) re WHERE x.born <= re AND re < x.died)
            LIMIT GREATEST(p_limit - n, 0));
        GET DIAGNOSTICS k = ROW_COUNT;
        n := n + k;
        DELETE FROM pgfs.inodes WHERE (layer, ino, born) IN (
            SELECT x.layer, x.ino, x.born FROM pgfs.inodes x
            WHERE x.layer = l.layer AND x.died <> 9223372036854775807
              AND NOT EXISTS (SELECT 1 FROM unnest(v_ret) re WHERE x.born <= re AND re < x.died)
            LIMIT GREATEST(p_limit - n, 0));
        GET DIAGNOSTICS k = ROW_COUNT;
        n := n + k;

        -- Live map rows of deleted inodes, or past the end of their file
        -- (the remainder of a large unlink or truncate).
        FOR i IN SELECT DISTINCT f.ino FROM pgfs.file_blocks f
                 LEFT JOIN pgfs.inodes x ON x.layer = f.layer AND x.ino = f.ino
                                        AND x.died = 9223372036854775807
                 WHERE f.layer = l.layer AND f.died = 9223372036854775807
                   AND (x.ino IS NULL
                        OR (x.nlink = 0 AND NOT EXISTS (SELECT 1 FROM pgfs.orphans o
                                                        WHERE o.layer = l.layer AND o.ino = f.ino))
                        OR x.inline_data IS NOT NULL
                        OR f.block_no * bs >= x.size)
                 LIMIT 100 LOOP
            SELECT * INTO r FROM pgfs.inodes x
            WHERE x.layer = l.layer AND x.ino = i.ino AND x.died = 9223372036854775807 FOR UPDATE;
            IF r.ino IS NULL OR (r.nlink = 0 AND NOT EXISTS (SELECT 1 FROM pgfs.orphans o
                                 WHERE o.layer = l.layer AND o.ino = i.ino)) OR r.inline_data IS NOT NULL THEN
                n := n + pgfs._blocks_cut(l.layer, e, i.ino, 0, GREATEST(p_limit - n, 1)::integer);
            ELSE
                n := n + pgfs._blocks_cut(l.layer, e, i.ino, (r.size + bs - 1) / bs,
                                          GREATEST(p_limit - n, 1)::integer);
            END IF;
            EXIT WHEN n >= p_limit;
        END LOOP;
        IF n >= p_limit THEN
            RETURN n;
        END IF;
    END LOOP;
    RETURN n;
END $$;

-- One garbage-collection batch of shared blobs (spec/space.py CollectsSharedBlobs).
CREATE FUNCTION pgfs.gc_batch(p_limit integer DEFAULT 1000) RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
    b record;
    n integer := 0;
BEGIN
    FOR b IN SELECT blob_id FROM pgfs.gc_queue ORDER BY blob_id LIMIT p_limit FOR UPDATE SKIP LOCKED LOOP
        IF NOT EXISTS (SELECT 1 FROM pgfs.file_blocks WHERE blob_id = b.blob_id) THEN
            DELETE FROM pgfs.blobs WHERE blob_id = b.blob_id;
            n := n + 1;
        END IF;
        DELETE FROM pgfs.gc_queue WHERE blob_id = b.blob_id;
    END LOOP;
    RETURN n;
END $$;

-- The reclaim and GC backlog (spec/space.py ReportsSpace).
CREATE FUNCTION pgfs.backlog(OUT retired_rows bigint, OUT gc_queue bigint, OUT deleting_layers bigint)
LANGUAGE sql STABLE AS $$
    SELECT (SELECT count(*) FROM pgfs.file_blocks WHERE died <> 9223372036854775807)
         + (SELECT count(*) FROM pgfs.inodes WHERE died <> 9223372036854775807)
         + (SELECT count(*) FROM pgfs.dentries WHERE died <> 9223372036854775807),
           (SELECT count(*) FROM pgfs.gc_queue),
           (SELECT count(*) FROM pgfs.layers WHERE state = 'deleting')
$$;

-- Trees above the auto-flatten depth (spec/branch.py LimitsBranchDepth).
CREATE FUNCTION pgfs.deep_trees(p_depth integer) RETURNS SETOF text
LANGUAGE sql STABLE AS $$
    SELECT t.name FROM pgfs.trees t JOIN pgfs.layers l ON l.layer = t.layer
    WHERE l.depth > p_depth ORDER BY l.depth DESC, t.name
$$;

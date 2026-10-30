-- pgfs schema version 1: invariant checks (spec/integrity.py PgfsCheck).
-- Each returns (class, detail) rows, one per violation.

-- The names reachable from a tree's root in its live view: (parent, name,
-- ino, kind). Directories reached twice are reported by the caller.
CREATE FUNCTION pgfs._walk(p_layer bigint, p_epoch bigint, p_root bigint)
RETURNS TABLE (parent_ino bigint, name bytea, ino bigint, kind "char")
LANGUAGE plpgsql STABLE AS $$
#variable_conflict use_column
DECLARE
    v_queue bigint[] := ARRAY[p_root];
    v_seen bigint[] := ARRAY[p_root];
    v_dir bigint;
    x record;
BEGIN
    WHILE array_length(v_queue, 1) > 0 LOOP
        v_dir := v_queue[1];
        v_queue := v_queue[2:];
        FOR x IN SELECT * FROM pgfs.readdir(p_layer, p_epoch, v_dir, NULL, 2147483647) LOOP
            parent_ino := v_dir;
            name := x.name;
            ino := x.ino;
            kind := x.kind;
            RETURN NEXT;
            IF x.kind = 'd' AND NOT x.ino = ANY (v_seen) THEN
                v_queue := v_queue || x.ino;
                v_seen := v_seen || x.ino;
            END IF;
        END LOOP;
    END LOOP;
END $$;

-- A digest of everything a view shows: names, attributes, block map and xattrs.
CREATE FUNCTION pgfs.view_digest(p_layer bigint, p_epoch bigint, p_root bigint) RETURNS text
LANGUAGE plpgsql STABLE AS $$
DECLARE
    v text;
BEGIN
    SELECT md5(string_agg(line, E'\n' ORDER BY line)) INTO v FROM (
        SELECT 'd ' || w.parent_ino || ' ' || encode(w.name, 'hex') || ' ' || w.ino AS line
        FROM pgfs._walk(p_layer, p_epoch, p_root) w
        UNION ALL
        SELECT 'i ' || i.ino || ' ' || i.gen || ' ' || i.mode || ' ' || i.uid || ' ' || i.gid || ' ' || i.nlink
               || ' ' || i.size || ' ' || i.mtime_ns || ' ' || COALESCE(md5(i.inline_data), '-')
               || ' ' || COALESCE(md5(i.target), '-')
        FROM (SELECT DISTINCT w.ino FROM pgfs._walk(p_layer, p_epoch, p_root) w
              UNION SELECT p_root) s,
             LATERAL (SELECT (pgfs._inode(p_layer, p_epoch, s.ino)).*) i
        UNION ALL
        SELECT 'b ' || s.ino || ' ' || m.block_no || ' ' || COALESCE(m.blob_id::text, 'hole')
        FROM (SELECT DISTINCT w.ino FROM pgfs._walk(p_layer, p_epoch, p_root) w WHERE w.kind = 'f') s,
             LATERAL pgfs._blocks(p_layer, p_epoch, s.ino, 0, 9223372036854775806) m
        UNION ALL
        SELECT 'x ' || s.ino || ' ' || encode(n, 'hex') || ' '
               || md5((pgfs._xattr(p_layer, p_epoch, s.ino, n)).value)
        FROM (SELECT DISTINCT w.ino FROM pgfs._walk(p_layer, p_epoch, p_root) w UNION SELECT p_root) s,
             LATERAL pgfs.listxattr(p_layer, p_epoch, s.ino) n
    ) lines;
    RETURN COALESCE(v, md5(''));
END $$;

-- Compute and store a snapshot's digest if it has none (spec/branch.py
-- SnapshotsAreImmutable). Returns the stored digest.
CREATE FUNCTION pgfs.snapshot_seal(p_tree text, p_name text) RETURNS text
LANGUAGE plpgsql AS $$
DECLARE
    s pgfs.snapshots;
    v text;
BEGIN
    SELECT * INTO s FROM pgfs.snapshots WHERE tree = p_tree AND name = p_name FOR UPDATE;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'snapshot %@% does not exist', p_tree, p_name USING ERRCODE = 'PF002';
    END IF;
    IF s.digest IS NOT NULL THEN
        RETURN s.digest;
    END IF;
    v := pgfs.view_digest(s.layer, s.epoch, s.root_ino);
    UPDATE pgfs.snapshots SET digest = v WHERE tree = p_tree AND name = p_name;
    RETURN v;
END $$;

CREATE FUNCTION pgfs.check(p_tree text DEFAULT NULL)
RETURNS TABLE (class integer, detail text)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    t record;
    l record;
    s record;
    x record;
    e bigint;
    bs bigint := pgfs._meta_int('block_size');
    v_inline_max bigint := pgfs._meta_int('inline_max');
    v_ret bigint[];
    r pgfs.inodes;
    v_digest text;
BEGIN
    CREATE TEMP TABLE IF NOT EXISTS _pgfs_names (parent_ino bigint, name bytea, ino bigint, kind "char") ON COMMIT DROP;
    FOR t IN SELECT * FROM pgfs.trees WHERE p_tree IS NULL OR name = p_tree ORDER BY name LOOP
        e := pgfs._epoch(t.layer, NULL);
        TRUNCATE _pgfs_names;
        INSERT INTO _pgfs_names SELECT * FROM pgfs._walk(t.layer, e, t.root_ino);

        -- 1. Names resolve to live inodes of the same kind; no own name
        --    (whiteouts included) under a whited-out directory.
        FOR x IN SELECT n.* FROM _pgfs_names n LOOP
            r := pgfs._inode(t.layer, e, x.ino);
            IF r IS NULL THEN
                class := 1; detail := format('%s: name %s in directory %s points to missing inode %s',
                    t.name, pgfs.name_text(x.name), x.parent_ino, x.ino);
                RETURN NEXT;
            ELSIF r.kind <> x.kind THEN
                class := 1; detail := format('%s: name %s in directory %s has kind %s but inode %s has kind %s',
                    t.name, pgfs.name_text(x.name), x.parent_ino, x.kind, x.ino, r.kind);
                RETURN NEXT;
            END IF;
        END LOOP;
        FOR x IN SELECT d.parent_ino, d.name FROM pgfs.dentries d
                 JOIN pgfs.inodes i ON i.layer = d.layer AND i.ino = d.parent_ino AND i.died = 9223372036854775807
                 WHERE d.layer = t.layer AND d.died = 9223372036854775807 AND i.nlink = 0
                   AND NOT EXISTS (SELECT 1 FROM pgfs.orphans o WHERE o.layer = i.layer AND o.ino = i.ino) LOOP
            class := 1; detail := format('%s: name %s lives under removed directory %s',
                t.name, pgfs.name_text(x.name), x.parent_ino);
            RETURN NEXT;
        END LOOP;

        -- 2. Every own live inode is reachable; directories have one name; no cycle.
        FOR x IN SELECT i.ino FROM pgfs.inodes i
                 WHERE i.layer = t.layer AND i.died = 9223372036854775807 AND i.nlink > 0
                   AND i.ino <> t.root_ino
                   AND NOT EXISTS (SELECT 1 FROM _pgfs_names n WHERE n.ino = i.ino)
                   AND (t.root_ino = 1 OR i.born > 0) LOOP
            IF t.root_ino = 1 OR pgfs._name_of(t.layer, e, x.ino) IS NOT NULL THEN
                class := 2; detail := format('%s: inode %s is not reachable from the root', t.name, x.ino);
                RETURN NEXT;
            END IF;
        END LOOP;
        FOR x IN SELECT n.ino, count(*) AS c FROM _pgfs_names n WHERE n.kind = 'd' GROUP BY n.ino HAVING count(*) > 1 LOOP
            class := 2; detail := format('%s: directory %s has %s names (or the graph has a cycle)', t.name, x.ino, x.c);
            RETURN NEXT;
        END LOOP;

        -- 3. Link counts.
        FOR x IN SELECT s2.ino, s2.names, s2.subdirs, (pgfs._inode(t.layer, e, s2.ino)).nlink AS nlink,
                        (pgfs._inode(t.layer, e, s2.ino)).kind AS kind
                 FROM (SELECT i AS ino,
                              (SELECT count(*) FROM _pgfs_names n WHERE n.ino = i) AS names,
                              (SELECT count(*) FROM _pgfs_names n WHERE n.parent_ino = i AND n.kind = 'd') AS subdirs
                       FROM (SELECT DISTINCT ino AS i FROM _pgfs_names UNION SELECT t.root_ino) u) s2 LOOP
            IF x.kind = 'd' AND x.nlink <> 2 + x.subdirs THEN
                class := 3; detail := format('%s: directory %s has nlink %s but %s subdirectories', t.name, x.ino, x.nlink, x.subdirs);
                RETURN NEXT;
            ELSIF x.kind IS NOT NULL AND x.kind <> 'd' AND x.nlink <> x.names THEN
                class := 3; detail := format('%s: inode %s has nlink %s but %s names', t.name, x.ino, x.nlink, x.names);
                RETURN NEXT;
            END IF;
        END LOOP;
    END LOOP;

    -- 4. Inline data and block maps.
    FOR x IN SELECT i.layer, i.ino, i.size, octet_length(i.inline_data) AS ilen,
                    (SELECT count(*) FROM pgfs.file_blocks f WHERE f.layer = i.layer AND f.ino = i.ino
                       AND f.died = 9223372036854775807) AS maps,
                    (SELECT count(*) FROM pgfs.file_blocks f WHERE f.layer = i.layer AND f.ino = i.ino
                       AND f.died = 9223372036854775807 AND f.block_no * bs >= i.size) AS past
             FROM pgfs.inodes i
             WHERE i.died = 9223372036854775807 AND i.kind = 'f'
               AND (p_tree IS NULL OR i.layer IN (SELECT layer FROM pgfs.trees WHERE name = p_tree)) LOOP
        IF x.ilen IS NOT NULL AND (x.maps > 0 OR x.size > v_inline_max) THEN
            class := 4; detail := format('layer %s inode %s: inline data with %s map rows and size %s', x.layer, x.ino, x.maps, x.size);
            RETURN NEXT;
        END IF;
        IF x.past > 0 THEN
            class := 4; detail := format('layer %s inode %s: %s map rows past its size %s', x.layer, x.ino, x.past, x.size);
            RETURN NEXT;
        END IF;
    END LOOP;

    IF p_tree IS NULL THEN
        -- 5. Unreferenced unshared blobs.
        FOR x IN SELECT b.blob_id FROM pgfs.blobs b
                 WHERE NOT b.shared AND NOT EXISTS (SELECT 1 FROM pgfs.file_blocks f WHERE f.blob_id = b.blob_id)
                   AND NOT EXISTS (SELECT 1 FROM pgfs.gc_queue q WHERE q.blob_id = b.blob_id) LOOP
            class := 5; detail := format('blob %s is referenced by no map row', x.blob_id);
            RETURN NEXT;
        END LOOP;
    END IF;

    FOR l IN SELECT * FROM pgfs.layers WHERE state = 'active'
               AND (p_tree IS NULL OR layer IN (SELECT layer FROM pgfs.trees WHERE name = p_tree)) LOOP
        -- 6. No retained epoch sees two versions of one key.
        v_ret := pgfs.retained_epochs(l.layer);
        FOR x IN SELECT re, 'inodes' AS tbl, ino::text AS k FROM unnest(v_ret) re, pgfs.inodes i
                 WHERE i.layer = l.layer AND i.born <= re AND re < i.died GROUP BY re, ino HAVING count(*) > 1
                 UNION ALL
                 SELECT re, 'dentries', parent_ino || '/' || encode(name, 'hex') FROM unnest(v_ret) re, pgfs.dentries d
                 WHERE d.layer = l.layer AND d.born <= re AND re < d.died GROUP BY re, parent_ino, name HAVING count(*) > 1
                 UNION ALL
                 SELECT re, 'file_blocks', ino || ':' || block_no FROM unnest(v_ret) re, pgfs.file_blocks f
                 WHERE f.layer = l.layer AND f.born <= re AND re < f.died GROUP BY re, ino, block_no HAVING count(*) > 1
                 UNION ALL
                 SELECT re, 'xattrs', ino || ':' || encode(name, 'hex') FROM unnest(v_ret) re, pgfs.xattrs a
                 WHERE a.layer = l.layer AND a.born <= re AND re < a.died GROUP BY re, ino, name HAVING count(*) > 1 LOOP
            class := 6; detail := format('layer %s epoch %s: %s key %s has two visible versions', l.layer, x.re, x.tbl, x.k);
            RETURN NEXT;
        END LOOP;

        -- 7. Layer chain.
        IF l.base_layer IS NOT NULL THEN
            SELECT * INTO x FROM pgfs.layers b WHERE b.layer = l.base_layer;
            IF x.layer IS NULL THEN
                class := 7; detail := format('layer %s: base layer %s does not exist', l.layer, l.base_layer);
                RETURN NEXT;
            ELSE
                IF l.depth <> x.depth + 1 THEN
                    class := 7; detail := format('layer %s: depth %s but its base has depth %s', l.layer, l.depth, x.depth);
                    RETURN NEXT;
                END IF;
                IF l.base_epoch >= x.epoch THEN
                    class := 7; detail := format('layer %s: base epoch %s is not frozen (base is at %s)', l.layer, l.base_epoch, x.epoch);
                    RETURN NEXT;
                END IF;
            END IF;
        ELSIF l.depth <> 0 THEN
            class := 7; detail := format('layer %s: no base but depth %s', l.layer, l.depth);
            RETURN NEXT;
        END IF;

        -- 9. Usage totals.
        SELECT * INTO x FROM pgfs.layer_usage(l.layer);
        SELECT COALESCE(sum(CASE WHEN kind = 'f' THEN size ELSE 0 END), 0)::bigint AS lb, count(*)::bigint AS n
        INTO s FROM pgfs.inodes WHERE layer = l.layer AND died = 9223372036854775807 AND nlink > 0;
        IF x.logical_bytes <> s.lb OR x.inodes <> s.n THEN
            class := 9; detail := format('layer %s: usage says %s bytes, %s inodes; rows hold %s bytes, %s inodes',
                l.layer, x.logical_bytes, x.inodes, s.lb, s.n);
            RETURN NEXT;
        END IF;
    END LOOP;

    IF p_tree IS NULL THEN
        SELECT * INTO x FROM pgfs.fs_usage();
        SELECT ((SELECT COALESCE(sum(len), 0) FROM pgfs.blobs)
              + (SELECT COALESCE(sum(256 + COALESCE(octet_length(inline_data), 0) + COALESCE(octet_length(target), 0)), 0)
                 FROM pgfs.inodes))::bigint AS pb INTO s;
        IF x.physical_bytes <> s.pb THEN
            class := 9; detail := format('physical usage says %s bytes; rows hold %s', x.physical_bytes, s.pb);
            RETURN NEXT;
        END IF;
    END IF;

    -- 8. Snapshot digests (sealed ones are verified; the CLI seals the rest).
    FOR s IN SELECT * FROM pgfs.snapshots WHERE digest IS NOT NULL AND (p_tree IS NULL OR tree = p_tree) LOOP
        v_digest := pgfs.view_digest(s.layer, s.epoch, s.root_ino);
        IF v_digest <> s.digest THEN
            class := 8; detail := format('snapshot %s@%s changed: digest %s, sealed %s', s.tree, s.name, v_digest, s.digest);
            RETURN NEXT;
        END IF;
    END LOOP;

    -- 10. Table settings (spec/space.py TableTuning).
    FOR x IN SELECT * FROM (VALUES
                 ('inodes', 'fillfactor=80'), ('op_log', 'fillfactor=80'),
                 ('blobs', 'toast_tuple_target=8160'), ('blobs', 'autovacuum_vacuum_scale_factor=0.02'),
                 ('file_blocks', 'autovacuum_vacuum_scale_factor=0.02'),
                 ('changes', 'autovacuum_vacuum_scale_factor=0.01'),
                 ('usage_deltas', 'autovacuum_vacuum_scale_factor=0.01')) v(tbl, opt)
             WHERE NOT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                               WHERE n.nspname = 'pgfs' AND c.relname = v.tbl AND v.opt = ANY (c.reloptions)) LOOP
        class := 10; detail := format('table %s lacks %s', x.tbl, x.opt);
        RETURN NEXT;
    END LOOP;
    IF NOT EXISTS (SELECT 1 FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid
                   JOIN pg_namespace n ON n.oid = c.relnamespace
                   WHERE n.nspname = 'pgfs' AND c.relname = 'blobs' AND a.attname = 'data' AND a.attcompression = 'l') THEN
        class := 10; detail := 'blobs.data is not COMPRESSION lz4';
        RETURN NEXT;
    END IF;
END $$;

-- Repairs `pgfs check --repair` may make: queue unreferenced blobs (5)
-- and recompute the usage totals (9).
CREATE FUNCTION pgfs.check_repair() RETURNS integer
LANGUAGE plpgsql AS $$
DECLARE
    n integer;
BEGIN
    IF EXISTS (SELECT 1 FROM pgfs.mounts WHERE NOT read_only AND lease_until > now()) THEN
        RAISE EXCEPTION 'a tree is mounted read-write; unmount before --repair' USING ERRCODE = 'PF018';
    END IF;
    INSERT INTO pgfs.gc_queue
    SELECT b.blob_id FROM pgfs.blobs b
    WHERE NOT EXISTS (SELECT 1 FROM pgfs.file_blocks f WHERE f.blob_id = b.blob_id)
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS n = ROW_COUNT;
    DELETE FROM pgfs.usage_deltas;
    DELETE FROM pgfs.usage_totals;
    INSERT INTO pgfs.usage_totals (layer, logical_bytes, inodes)
    SELECT layer, COALESCE(sum(CASE WHEN kind = 'f' THEN size ELSE 0 END), 0), count(*)
    FROM pgfs.inodes WHERE died = 9223372036854775807 AND nlink > 0 GROUP BY layer;
    INSERT INTO pgfs.usage_totals (layer, physical_bytes)
    VALUES (0, (SELECT COALESCE(sum(len), 0) FROM pgfs.blobs)
             + (SELECT COALESCE(sum(256 + COALESCE(octet_length(inline_data), 0) + COALESCE(octet_length(target), 0)), 0)
                FROM pgfs.inodes))
    ON CONFLICT (layer) DO UPDATE SET physical_bytes = EXCLUDED.physical_bytes;
    RETURN n;
END $$;

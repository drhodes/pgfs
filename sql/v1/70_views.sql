-- pgfs schema version 1: read-only views for SQL users
-- (spec/storage.py SqlReadViews).

-- A name as text: UTF-8 names unchanged; other bytes as \xNN escapes.
CREATE FUNCTION pgfs.name_text(p bytea) RETURNS text
LANGUAGE plpgsql IMMUTABLE AS $$
DECLARE
    v text := '';
    i integer;
    b integer;
BEGIN
    IF p IS NULL THEN
        RETURN NULL;
    END IF;
    BEGIN
        RETURN convert_from(p, 'UTF8');
    EXCEPTION WHEN OTHERS THEN
        FOR i IN 0 .. octet_length(p) - 1 LOOP
            b := get_byte(p, i);
            IF b BETWEEN 32 AND 126 AND b <> 92 THEN
                v := v || chr(b);
            ELSE
                v := v || '\x' || lpad(to_hex(b), 2, '0');
            END IF;
        END LOOP;
        RETURN v;
    END;
END $$;

-- Every file of a tree's live view, with its path.
CREATE FUNCTION pgfs.tree_files(p_tree text)
RETURNS TABLE (path text, ino bigint, kind "char", size bigint, mode integer, uid bigint,
               gid bigint, mtime timestamptz)
LANGUAGE plpgsql STABLE AS $$
#variable_conflict use_column
DECLARE
    t pgfs.trees;
    e bigint;
    v_queue bigint[];
    v_paths text[];
    v_dir bigint;
    v_dpath text;
    x record;
    r pgfs.inodes;
BEGIN
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF NOT FOUND THEN
        RETURN;
    END IF;
    e := pgfs._epoch(t.layer, NULL);
    r := pgfs._inode(t.layer, e, t.root_ino);
    IF r IS NULL THEN
        RETURN;
    END IF;
    path := '/'; ino := r.ino; kind := r.kind; size := r.size; mode := r.mode; uid := r.uid;
    gid := r.gid; mtime := to_timestamp(r.mtime_ns / 1e9);
    RETURN NEXT;
    v_queue := ARRAY[t.root_ino];
    v_paths := ARRAY[''];
    WHILE array_length(v_queue, 1) > 0 LOOP
        v_dir := v_queue[1];
        v_dpath := v_paths[1];
        v_queue := v_queue[2:];
        v_paths := v_paths[2:];
        FOR x IN SELECT * FROM pgfs.readdir(t.layer, e, v_dir, NULL, 2147483647) LOOP
            r := pgfs._inode(t.layer, e, x.ino);
            CONTINUE WHEN r IS NULL;
            path := v_dpath || '/' || pgfs.name_text(x.name);
            ino := r.ino; kind := r.kind; size := r.size; mode := r.mode; uid := r.uid;
            gid := r.gid; mtime := to_timestamp(r.mtime_ns / 1e9);
            RETURN NEXT;
            IF r.kind = 'd' THEN
                v_queue := v_queue || r.ino;
                v_paths := v_paths || path;
            END IF;
        END LOOP;
    END LOOP;
END $$;

CREATE VIEW pgfs.files AS
    SELECT t.name AS tree, f.path, f.ino, f.kind, f.size, f.mode, f.uid, f.gid, f.mtime
    FROM pgfs.trees t, LATERAL pgfs.tree_files(t.name) f;

CREATE VIEW pgfs.tree_usage AS
    SELECT t.name AS tree, u.inodes AS files, u.logical_bytes AS bytes
    FROM pgfs.trees t, LATERAL pgfs.layer_usage(t.layer) u;

-- Bytes held only by a snapshot: blobs its view references that the
-- live view does not.
CREATE FUNCTION pgfs.snapshot_unique_bytes(p_layer bigint, p_epoch bigint) RETURNS bigint
LANGUAGE sql STABLE AS $$
    SELECT COALESCE(sum(b.len), 0)::bigint FROM pgfs.blobs b
    WHERE b.blob_id IN (
        SELECT f.blob_id FROM pgfs.file_blocks f
        WHERE f.layer = p_layer AND f.born <= p_epoch AND p_epoch < f.died
          AND f.died <> 9223372036854775807 AND f.blob_id IS NOT NULL)
$$;

CREATE VIEW pgfs.snapshot_list AS
    SELECT s.tree, s.name, s.created_at, pgfs.snapshot_unique_bytes(s.layer, s.epoch) AS bytes_unique
    FROM pgfs.snapshots s;

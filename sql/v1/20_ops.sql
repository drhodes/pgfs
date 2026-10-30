-- pgfs schema version 1: filesystem operations
-- (spec/storage.py OperationsAsSqlFunctions).
--
-- Reads take a view (layer, epoch); a NULL epoch is the live view, read at
-- the layer's current epoch. Every mutation takes
--   (p_layer, p_mount, p_fence, p_worker, p_op_seq)
-- first, runs in one transaction (spec/consistency.py
-- OneOperationOneTransaction), locks rows in the order of
-- spec/concurrency.py LockOrder, records its change and fills its op_log
-- slot.

CREATE FUNCTION pgfs._epoch(p_layer bigint, p_epoch bigint) RETURNS bigint
LANGUAGE sql STABLE AS $$
    SELECT COALESCE(p_epoch, (SELECT epoch FROM pgfs.layers WHERE layer = p_layer))
$$;

-- ── Reads ──────────────────────────────────────────────────────────────

-- Look up (parent, name). No row: ENOENT. Names under a missing or
-- whited-out directory never resolve (spec/app.py LooksUpEntries).
CREATE FUNCTION pgfs.lookup(p_layer bigint, p_epoch bigint, p_parent bigint, p_name bytea)
RETURNS SETOF pgfs.attr LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
    d pgfs.dentries;
    r pgfs.inodes;
BEGIN
    IF pgfs._inode(p_layer, e, p_parent) IS NULL THEN
        RETURN;
    END IF;
    d := pgfs._dentry(p_layer, e, p_parent, p_name);
    IF d.ino IS NULL THEN
        RETURN;
    END IF;
    r := pgfs._inode(p_layer, e, d.ino);
    IF r IS NULL THEN
        RETURN;
    END IF;
    RETURN NEXT pgfs._attr(p_layer, e, r);
END $$;

-- Attributes of `ino`; no row when absent or when `p_gen` (if given) does
-- not match (ESTALE, spec/storage.py PersistentInodes).
CREATE FUNCTION pgfs.getattr(p_layer bigint, p_epoch bigint, p_ino bigint, p_gen bigint)
RETURNS SETOF pgfs.attr LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
    r pgfs.inodes := pgfs._inode(p_layer, e, p_ino);
BEGIN
    IF r IS NULL OR (p_gen IS NOT NULL AND r.gen <> p_gen) THEN
        RETURN;
    END IF;
    RETURN NEXT pgfs._attr(p_layer, e, r);
END $$;

-- One page of a directory listing in byte order of name, after `p_after`
-- (keyset pagination, spec/app.py ListsDirectories). On a branch, the
-- layers are merged and whiteouts dropped (spec/branch.py LayeredLookup).
CREATE FUNCTION pgfs.readdir(p_layer bigint, p_epoch bigint, p_dir bigint, p_after bytea,
                             p_limit integer)
RETURNS TABLE (name bytea, ino bigint, kind "char")
LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
BEGIN
    IF NOT pgfs._has_base(p_layer) THEN
        RETURN QUERY
            SELECT d.name, d.ino, d.kind FROM pgfs.dentries d
            WHERE d.layer = p_layer AND d.parent_ino = p_dir
              AND d.name > COALESCE(p_after, ''::bytea)
              AND d.born <= e AND e < d.died AND d.ino IS NOT NULL
            ORDER BY d.name LIMIT p_limit;
        RETURN;
    END IF;
    RETURN QUERY
        SELECT w.name, w.ino, w.kind FROM (
            SELECT DISTINCT ON (d.name) d.name, d.ino, d.kind
            FROM pgfs._chain(p_layer, e) c
            JOIN pgfs.dentries d
              ON d.layer = c.layer AND d.parent_ino = p_dir
             AND d.name > COALESCE(p_after, ''::bytea)
             AND d.born <= c.epoch AND c.epoch < d.died
            ORDER BY d.name, c.depth
        ) w
        WHERE w.ino IS NOT NULL
        ORDER BY w.name LIMIT p_limit;
END $$;

CREATE FUNCTION pgfs.readdirplus(p_layer bigint, p_epoch bigint, p_dir bigint, p_after bytea,
                                 p_limit integer)
RETURNS TABLE (name bytea, a pgfs.attr)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
    x record;
    r pgfs.inodes;
BEGIN
    FOR x IN SELECT * FROM pgfs.readdir(p_layer, e, p_dir, p_after, p_limit) LOOP
        r := pgfs._inode(p_layer, e, x.ino);
        IF r.ino IS NOT NULL THEN
            name := x.name;
            a := pgfs._attr(p_layer, e, r);
            RETURN NEXT;
        END IF;
    END LOOP;
END $$;

-- Blocks p_first..p_last of a regular file with their checksums. An inline
-- file returns one row, block 0, with is_inline = true and a NULL crc32c.
-- ESTALE if the inode is gone or its generation differs.
CREATE FUNCTION pgfs.read_blocks(p_layer bigint, p_epoch bigint, p_ino bigint, p_gen bigint,
                                 p_first bigint, p_last bigint)
RETURNS TABLE (block_no bigint, data bytea, crc32c integer, blob_id bigint, is_inline boolean)
LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
    r pgfs.inodes := pgfs._inode(p_layer, e, p_ino);
BEGIN
    IF r IS NULL OR (p_gen IS NOT NULL AND r.gen <> p_gen) THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    IF r.kind = 'd' THEN
        RAISE EXCEPTION 'inode % is a directory', p_ino USING ERRCODE = 'PF005';
    END IF;
    IF r.inline_data IS NOT NULL THEN
        IF p_first = 0 THEN
            RETURN QUERY SELECT 0::bigint, r.inline_data, NULL::integer, NULL::bigint, true;
        END IF;
        RETURN;
    END IF;
    RETURN QUERY
        SELECT m.block_no, b.data, b.crc32c, b.blob_id, false
        FROM pgfs._blocks(p_layer, e, p_ino, p_first, p_last) m
        JOIN pgfs.blobs b ON b.blob_id = m.blob_id
        ORDER BY m.block_no;
END $$;

CREATE FUNCTION pgfs.readlink(p_layer bigint, p_epoch bigint, p_ino bigint)
RETURNS bytea LANGUAGE plpgsql STABLE AS $$
DECLARE
    r pgfs.inodes := pgfs._inode(p_layer, pgfs._epoch(p_layer, p_epoch), p_ino);
BEGIN
    IF r IS NULL THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    IF r.kind <> 'l' THEN
        RAISE EXCEPTION 'not a symlink' USING ERRCODE = 'PF007';
    END IF;
    RETURN r.target;
END $$;

-- The first block at or after `p_block` that holds data (p_data) or is a
-- hole (NOT p_data), at block granularity (spec/posix.py SparseFiles).
-- NULL: no such block before the end of the file.
CREATE FUNCTION pgfs.seek_block(p_layer bigint, p_epoch bigint, p_ino bigint, p_block bigint,
                                p_data boolean)
RETURNS bigint LANGUAGE plpgsql STABLE AS $$
DECLARE
    e bigint := pgfs._epoch(p_layer, p_epoch);
    r pgfs.inodes := pgfs._inode(p_layer, e, p_ino);
    bs bigint := pgfs._meta_int('block_size');
    v_last bigint;
    v bigint;
BEGIN
    IF r IS NULL THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    v_last := (r.size + bs - 1) / bs - 1;
    IF r.inline_data IS NOT NULL THEN
        IF p_data THEN
            RETURN CASE WHEN p_block <= v_last THEN p_block END;
        END IF;
        RETURN GREATEST(p_block, v_last + 1);
    END IF;
    IF p_data THEN
        SELECT min(m.block_no) INTO v FROM pgfs._blocks(p_layer, e, p_ino, p_block, v_last) m
        WHERE m.blob_id IS NOT NULL;
        RETURN v;
    END IF;
    SELECT min(g) INTO v FROM generate_series(p_block, v_last) g
    WHERE NOT EXISTS (SELECT 1 FROM pgfs._blocks(p_layer, e, p_ino, g, g) m WHERE m.blob_id IS NOT NULL);
    RETURN COALESCE(v, GREATEST(p_block, v_last + 1));
END $$;

CREATE FUNCTION pgfs.getxattr(p_layer bigint, p_epoch bigint, p_ino bigint, p_name bytea)
RETURNS SETOF bytea LANGUAGE plpgsql STABLE AS $$
DECLARE
    x pgfs.xattrs := pgfs._xattr(p_layer, pgfs._epoch(p_layer, p_epoch), p_ino, p_name);
BEGIN
    IF x.value IS NOT NULL THEN
        RETURN NEXT x.value;
    END IF;
END $$;

CREATE FUNCTION pgfs.listxattr(p_layer bigint, p_epoch bigint, p_ino bigint)
RETURNS SETOF bytea LANGUAGE sql STABLE AS $$
    SELECT w.name FROM (
        SELECT DISTINCT ON (x.name) x.name, x.value
        FROM pgfs._chain(p_layer, pgfs._epoch(p_layer, p_epoch)) c
        JOIN pgfs.xattrs x ON x.layer = c.layer AND x.ino = p_ino
                          AND x.born <= c.epoch AND c.epoch < x.died
        ORDER BY x.name, c.depth
    ) w WHERE w.value IS NOT NULL ORDER BY w.name
$$;

-- ── Creating names ─────────────────────────────────────────────────────

-- create, mkdir, mknod and symlink (spec/app.py CreatesFiles,
-- CreatesDirectories; spec/posix.py SpecialFiles, SymbolicLinks;
-- spec/security.py OwnershipFromCaller).
CREATE FUNCTION pgfs._make(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                           p_op_seq bigint, p_parent bigint, p_name bytea, p_kind "char",
                           p_mode integer, p_uid bigint, p_gid bigint, p_rdev bigint,
                           p_target bytea, p_now bigint)
RETURNS pgfs.attr LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    v_ino bigint;
    par pgfs.inodes;
    r pgfs.inodes;
    v_gid bigint := p_gid;
    v_mode integer := p_mode & 4095;
    v_root bigint;
    a pgfs.attr;
BEGIN
    PERFORM pgfs._check_name(p_name);
    IF p_kind = 'l' AND octet_length(p_target) > 4095 THEN
        RAISE EXCEPTION 'symlink target longer than 4095 bytes' USING ERRCODE = 'PF008';
    END IF;
    e := pgfs._begin(p_layer, p_mount, p_fence);
    v_ino := nextval('pgfs.ino_seq');
    par := pgfs._inode_lock(p_layer, e, p_parent);
    IF par IS NULL THEN
        RAISE EXCEPTION 'parent % does not exist', p_parent USING ERRCODE = 'PF002';
    END IF;
    IF par.kind <> 'd' THEN
        RAISE EXCEPTION 'parent % is not a directory', p_parent USING ERRCODE = 'PF006';
    END IF;
    IF par.mode & 1024 <> 0 THEN
        v_gid := par.gid;
        IF p_kind = 'd' THEN
            v_mode := v_mode | 1024;
        END IF;
    END IF;
    IF p_kind = 'l' THEN
        v_mode := 511;
    END IF;
    r.layer := p_layer;
    r.ino := v_ino;
    r.gen := pgfs._new_gen();
    r.born := e;
    r.died := 9223372036854775807;
    r.kind := p_kind;
    r.mode := v_mode;
    r.uid := p_uid;
    r.gid := v_gid;
    r.nlink := CASE WHEN p_kind = 'd' THEN 2 ELSE 1 END;
    r.size := CASE WHEN p_kind = 'l' THEN octet_length(p_target) ELSE 0 END;
    r.atime_ns := p_now;
    r.mtime_ns := p_now;
    r.ctime_ns := p_now;
    r.btime_ns := p_now;
    r.rdev := COALESCE(p_rdev, 0);
    r.inline_data := CASE WHEN p_kind = 'f' AND pgfs._meta_int('inline_max') > 0 THEN ''::bytea END;
    r.target := CASE WHEN p_kind = 'l' THEN p_target END;
    r.inherit_limit := 9223372036854775807;
    r.shadows_base := false;
    r.version := 1;
    INSERT INTO pgfs.inodes VALUES (r.*);
    PERFORM pgfs._dentry_insert(p_layer, e, p_parent, p_name, v_ino, p_kind);
    UPDATE pgfs.inodes SET mtime_ns = p_now, ctime_ns = p_now, version = version + 1,
                           nlink = nlink + CASE WHEN p_kind = 'd' THEN 1 ELSE 0 END
    WHERE layer = p_layer AND ino = p_parent AND born = par.born;
    v_root := pgfs._root_of(p_layer);
    PERFORM pgfs._change(p_layer,
        CASE p_kind WHEN 'f' THEN 'create' WHEN 'd' THEN 'mkdir' WHEN 'l' THEN 'symlink' ELSE 'mknod' END,
        v_ino, p_kind, p_parent, p_name,
        pgfs._join(pgfs._path(p_layer, e, p_parent, v_root), p_name));
    a := pgfs._attr(p_layer, e, r);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, to_jsonb(a));
    RETURN a;
END $$;

CREATE FUNCTION pgfs.create(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                            p_op_seq bigint, p_parent bigint, p_name bytea, p_mode integer,
                            p_uid bigint, p_gid bigint, p_now bigint)
RETURNS pgfs.attr LANGUAGE sql AS $$
    SELECT pgfs._make(p_layer, p_mount, p_fence, p_worker, p_op_seq, p_parent, p_name, 'f',
                      p_mode, p_uid, p_gid, 0, NULL, p_now)
$$;

CREATE FUNCTION pgfs.mkdir(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                           p_op_seq bigint, p_parent bigint, p_name bytea, p_mode integer,
                           p_uid bigint, p_gid bigint, p_now bigint)
RETURNS pgfs.attr LANGUAGE sql AS $$
    SELECT pgfs._make(p_layer, p_mount, p_fence, p_worker, p_op_seq, p_parent, p_name, 'd',
                      p_mode, p_uid, p_gid, 0, NULL, p_now)
$$;

CREATE FUNCTION pgfs.mknod(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                           p_op_seq bigint, p_parent bigint, p_name bytea, p_kind "char",
                           p_mode integer, p_uid bigint, p_gid bigint, p_rdev bigint, p_now bigint)
RETURNS pgfs.attr LANGUAGE sql AS $$
    SELECT pgfs._make(p_layer, p_mount, p_fence, p_worker, p_op_seq, p_parent, p_name, p_kind,
                      p_mode, p_uid, p_gid, p_rdev, NULL, p_now)
$$;

CREATE FUNCTION pgfs.symlink(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                             p_op_seq bigint, p_parent bigint, p_name bytea, p_target bytea,
                             p_uid bigint, p_gid bigint, p_now bigint)
RETURNS pgfs.attr LANGUAGE sql AS $$
    SELECT pgfs._make(p_layer, p_mount, p_fence, p_worker, p_op_seq, p_parent, p_name, 'l',
                      511, p_uid, p_gid, 0, p_target, p_now)
$$;

-- link (spec/posix.py HardLinks).
CREATE FUNCTION pgfs.link(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                          p_op_seq bigint, p_ino bigint, p_gen bigint, p_newparent bigint,
                          p_newname bytea, p_now bigint)
RETURNS pgfs.attr LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    r pgfs.inodes;
    par pgfs.inodes;
    v_root bigint;
    a pgfs.attr;
BEGIN
    PERFORM pgfs._check_name(p_newname);
    e := pgfs._begin(p_layer, p_mount, p_fence);
    IF p_ino < p_newparent THEN
        r := pgfs._inode_lock(p_layer, e, p_ino);
        par := pgfs._inode_lock(p_layer, e, p_newparent);
    ELSE
        par := pgfs._inode_lock(p_layer, e, p_newparent);
        r := pgfs._inode_lock(p_layer, e, p_ino);
    END IF;
    IF r IS NULL OR r.gen <> COALESCE(p_gen, r.gen) THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    IF r.nlink = 0 THEN
        RAISE EXCEPTION 'inode % has been unlinked', p_ino USING ERRCODE = 'PF002';
    END IF;
    IF r.kind = 'd' THEN
        RAISE EXCEPTION 'hard link to a directory' USING ERRCODE = 'PF017';
    END IF;
    IF r.nlink >= 65000 THEN
        RAISE EXCEPTION 'too many links' USING ERRCODE = 'PF012';
    END IF;
    IF par IS NULL THEN
        RAISE EXCEPTION 'parent % does not exist', p_newparent USING ERRCODE = 'PF002';
    END IF;
    IF par.kind <> 'd' THEN
        RAISE EXCEPTION 'parent % is not a directory', p_newparent USING ERRCODE = 'PF006';
    END IF;
    PERFORM pgfs._dentry_insert(p_layer, e, p_newparent, p_newname, p_ino, r.kind);
    UPDATE pgfs.inodes SET nlink = nlink + 1, ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND ino = p_ino AND born = e
    RETURNING * INTO r;
    UPDATE pgfs.inodes SET mtime_ns = p_now, ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND ino = p_newparent AND born = e;
    v_root := pgfs._root_of(p_layer);
    PERFORM pgfs._change(p_layer, 'link', p_ino, r.kind, p_newparent, p_newname,
        pgfs._join(pgfs._path(p_layer, e, p_newparent, v_root), p_newname));
    a := pgfs._attr(p_layer, e, r);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, to_jsonb(a));
    RETURN a;
END $$;

-- ── Removing names ─────────────────────────────────────────────────────

-- Count the live names of `p_ino` reachable from the view root (a subtree
-- branch, spec/branch.py ViewsRootedAtDirectory).
CREATE FUNCTION pgfs._reachable_names(p_layer bigint, p_epoch bigint, p_ino bigint, p_root bigint)
RETURNS integer LANGUAGE plpgsql STABLE AS $$
DECLARE
    c record;
    d pgfs.dentries;
    w pgfs.dentries;
    n integer := 0;
    seen bytea[] := '{}';
BEGIN
    FOR c IN SELECT * FROM pgfs._chain(p_layer, p_epoch) LOOP
        FOR d IN SELECT * FROM pgfs.dentries x
                 WHERE x.layer = c.layer AND x.ino = p_ino
                   AND x.born <= c.epoch AND c.epoch < x.died LOOP
            w := pgfs._dentry(p_layer, p_epoch, d.parent_ino, d.name);
            IF w.ino = p_ino AND NOT (int8send(d.parent_ino) || d.name = ANY (seen))
               AND pgfs._path(p_layer, p_epoch, d.parent_ino, p_root) IS NOT NULL THEN
                seen := seen || (int8send(d.parent_ino) || d.name);
                n := n + 1;
            END IF;
        END LOOP;
    END LOOP;
    RETURN n;
END $$;

-- Drop one link of a non-directory whose name was removed. Returns the new
-- nlink; deletes or orphans the inode at zero (spec/app.py UnlinksFiles,
-- spec/posix.py OpenAfterUnlink).
CREATE FUNCTION pgfs._drop_link(p_layer bigint, p_epoch bigint, p_ino bigint, p_open bigint[],
                                p_now bigint, OUT nlink integer, OUT orphaned boolean)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    r pgfs.inodes;
    v_root bigint := pgfs._root_of(p_layer);
BEGIN
    SELECT * INTO r FROM pgfs.inodes
    WHERE layer = p_layer AND ino = p_ino AND died = 9223372036854775807;
    r.nlink := r.nlink - 1;
    IF r.nlink > 0 AND v_root <> 1
       AND pgfs._reachable_names(p_layer, p_epoch, p_ino, v_root) = 0 THEN
        r.nlink := 0;
    END IF;
    orphaned := false;
    UPDATE pgfs.inodes SET nlink = r.nlink, ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND ino = p_ino AND born = r.born;
    IF r.nlink <= 0 THEN
        r.nlink := 0;
        IF p_ino = ANY (COALESCE(p_open, '{}')) THEN
            INSERT INTO pgfs.orphans (layer, ino) VALUES (p_layer, p_ino) ON CONFLICT DO NOTHING;
            orphaned := true;
        ELSE
            PERFORM pgfs._inode_delete(p_layer, p_epoch, r);
        END IF;
    END IF;
    nlink := r.nlink;
END $$;

-- Raise PF025 when an inode the operation touches still has dirty buffers
-- on the mount (spec/concurrency.py InodeShardedWorkers).
CREATE FUNCTION pgfs._check_dirty(p_inos bigint[], p_dirty bigint[]) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    v bigint[];
BEGIN
    SELECT array_agg(i) INTO v FROM unnest(p_inos) i WHERE i = ANY (COALESCE(p_dirty, '{}'));
    IF v IS NOT NULL THEN
        RAISE EXCEPTION 'retry' USING ERRCODE = 'PF025', DETAIL = array_to_string(v, ',');
    END IF;
END $$;

-- unlink: returns the unlinked inode, its remaining links and whether it
-- became an orphan.
CREATE FUNCTION pgfs.unlink(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                            p_op_seq bigint, p_parent bigint, p_name bytea, p_open bigint[],
                            p_dirty bigint[], p_now bigint)
RETURNS TABLE (ino bigint, nlink integer, orphaned boolean)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    e bigint;
    d pgfs.dentries;
    par pgfs.inodes;
    ch pgfs.inodes;
    v_child bigint;
    v_path bytea;
    v_root bigint;
    x record;
    attempt integer := 0;
BEGIN
    PERFORM pgfs._check_name(p_name);
    e := pgfs._begin(p_layer, p_mount, p_fence);
    v_root := pgfs._root_of(p_layer);
    LOOP
        attempt := attempt + 1;
        -- Discover.
        d := pgfs._dentry(p_layer, e, p_parent, p_name);
        IF d.ino IS NULL THEN
            RAISE EXCEPTION 'no such name' USING ERRCODE = 'PF002';
        END IF;
        IF d.kind = 'd' THEN
            RAISE EXCEPTION 'is a directory' USING ERRCODE = 'PF005';
        END IF;
        v_child := d.ino;
        PERFORM pgfs._check_dirty(ARRAY[v_child], p_dirty);
        -- Lock inodes in ascending order, then the dentry; revalidate.
        IF p_parent < v_child THEN
            par := pgfs._inode_lock(p_layer, e, p_parent);
            ch := pgfs._inode_lock(p_layer, e, v_child);
        ELSE
            ch := pgfs._inode_lock(p_layer, e, v_child);
            par := pgfs._inode_lock(p_layer, e, p_parent);
        END IF;
        IF par IS NULL THEN
            RAISE EXCEPTION 'parent does not exist' USING ERRCODE = 'PF002';
        END IF;
        PERFORM pgfs._dentry_lock(p_layer, p_parent, p_name);
        d := pgfs._dentry(p_layer, e, p_parent, p_name);
        EXIT WHEN d.ino = v_child AND ch.ino IS NOT NULL;
        IF attempt >= 3 THEN
            RAISE EXCEPTION 'unlink raced' USING ERRCODE = '40001';
        END IF;
    END LOOP;
    v_path := pgfs._join(pgfs._path(p_layer, e, p_parent, v_root), p_name);
    UPDATE pgfs.inodes SET mtime_ns = p_now, ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND inodes.ino = p_parent AND born = e;
    PERFORM pgfs._dentry_set(p_layer, e, p_parent, p_name, NULL, NULL);
    x := pgfs._drop_link(p_layer, e, v_child, p_open, p_now);
    PERFORM pgfs._change(p_layer, 'unlink', v_child, ch.kind, p_parent, p_name, v_path);
    ino := v_child;
    nlink := x.nlink;
    orphaned := x.orphaned;
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq,
                       jsonb_build_object('ino', ino, 'nlink', nlink, 'orphaned', orphaned));
    RETURN NEXT;
END $$;

-- True if directory `p_dir` has a visible entry.
CREATE FUNCTION pgfs._dir_has_entries(p_layer bigint, p_epoch bigint, p_dir bigint)
RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM pgfs.readdir(p_layer, p_epoch, p_dir, NULL, 1))
$$;

-- Delete an empty directory inode and the whiteouts under it in the layer.
CREATE FUNCTION pgfs._dir_delete(p_layer bigint, p_epoch bigint, p_dir bigint)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    w record;
    r pgfs.inodes;
BEGIN
    FOR w IN SELECT * FROM pgfs.dentries
             WHERE layer = p_layer AND parent_ino = p_dir AND died = 9223372036854775807
             FOR UPDATE LOOP
        IF w.born = p_epoch THEN
            DELETE FROM pgfs.dentries
            WHERE layer = p_layer AND parent_ino = p_dir AND name = w.name AND born = w.born;
        ELSE
            UPDATE pgfs.dentries SET died = p_epoch
            WHERE layer = p_layer AND parent_ino = p_dir AND name = w.name AND born = w.born;
        END IF;
    END LOOP;
    SELECT * INTO r FROM pgfs.inodes
    WHERE layer = p_layer AND ino = p_dir AND died = 9223372036854775807;
    PERFORM pgfs._inode_delete(p_layer, p_epoch, r);
END $$;

-- rmdir (spec/app.py RemovesDirectories).
CREATE FUNCTION pgfs.rmdir(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                           p_op_seq bigint, p_parent bigint, p_name bytea, p_now bigint)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    d pgfs.dentries;
    par pgfs.inodes;
    ch pgfs.inodes;
    v_child bigint;
    v_root bigint;
    v_path bytea;
    attempt integer := 0;
BEGIN
    PERFORM pgfs._check_name(p_name);
    e := pgfs._begin(p_layer, p_mount, p_fence);
    v_root := pgfs._root_of(p_layer);
    LOOP
        attempt := attempt + 1;
        d := pgfs._dentry(p_layer, e, p_parent, p_name);
        IF d.ino IS NULL THEN
            RAISE EXCEPTION 'no such name' USING ERRCODE = 'PF002';
        END IF;
        IF d.kind <> 'd' THEN
            RAISE EXCEPTION 'not a directory' USING ERRCODE = 'PF006';
        END IF;
        v_child := d.ino;
        IF v_child = v_root THEN
            RAISE EXCEPTION 'root of the view' USING ERRCODE = 'PF018';
        END IF;
        IF p_parent < v_child THEN
            par := pgfs._inode_lock(p_layer, e, p_parent);
            ch := pgfs._inode_lock(p_layer, e, v_child);
        ELSE
            ch := pgfs._inode_lock(p_layer, e, v_child);
            par := pgfs._inode_lock(p_layer, e, p_parent);
        END IF;
        IF par IS NULL THEN
            RAISE EXCEPTION 'parent does not exist' USING ERRCODE = 'PF002';
        END IF;
        PERFORM pgfs._dentry_lock(p_layer, p_parent, p_name);
        d := pgfs._dentry(p_layer, e, p_parent, p_name);
        EXIT WHEN d.ino = v_child AND ch.ino IS NOT NULL;
        IF attempt >= 3 THEN
            RAISE EXCEPTION 'rmdir raced' USING ERRCODE = '40001';
        END IF;
    END LOOP;
    IF pgfs._dir_has_entries(p_layer, e, v_child) THEN
        RAISE EXCEPTION 'directory not empty' USING ERRCODE = 'PF004';
    END IF;
    v_path := pgfs._join(pgfs._path(p_layer, e, p_parent, v_root), p_name);
    UPDATE pgfs.inodes SET nlink = nlink - 1, mtime_ns = p_now, ctime_ns = p_now,
                           version = version + 1
    WHERE layer = p_layer AND ino = p_parent AND born = e;
    PERFORM pgfs._dir_delete(p_layer, e, v_child);
    PERFORM pgfs._dentry_set(p_layer, e, p_parent, p_name, NULL, NULL);
    PERFORM pgfs._change(p_layer, 'rmdir', v_child, 'd', p_parent, p_name, v_path);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, jsonb_build_object('ino', v_child));
    RETURN v_child;
END $$;

-- True if `p_dir` is `p_ancestor` or lies below it in the view.
CREATE FUNCTION pgfs._is_within(p_layer bigint, p_epoch bigint, p_dir bigint, p_ancestor bigint,
                                p_root bigint)
RETURNS boolean LANGUAGE plpgsql STABLE AS $$
DECLARE
    v bigint := p_dir;
    d pgfs.dentries;
    steps integer := 0;
BEGIN
    LOOP
        IF v = p_ancestor THEN
            RETURN true;
        END IF;
        IF v = p_root THEN
            RETURN false;
        END IF;
        d := pgfs._name_of(p_layer, p_epoch, v);
        IF d IS NULL THEN
            RETURN false;
        END IF;
        v := d.parent_ino;
        steps := steps + 1;
        IF steps > 4096 THEN
            RETURN false;
        END IF;
    END LOOP;
END $$;

-- rename, with RENAME_NOREPLACE (1) and RENAME_EXCHANGE (2)
-- (spec/app.py RenamesEntries, RejectsInvalidRenames; spec/posix.py
-- RenameFlags; spec/concurrency.py RenameSerialisationPoint). Returns the
-- moved inode and, when a target was replaced, its remaining links.
CREATE FUNCTION pgfs.rename(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                            p_op_seq bigint, p_parent bigint, p_name bytea, p_newparent bigint,
                            p_newname bytea, p_flags integer, p_open bigint[], p_dirty bigint[],
                            p_now bigint)
RETURNS TABLE (ino bigint, replaced_ino bigint, replaced_nlink integer, replaced_orphaned boolean)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    e bigint;
    s pgfs.dentries;
    t pgfs.dentries;
    v_src bigint;
    v_tgt bigint;
    v_root bigint;
    v_exchange boolean := p_flags & 2 <> 0;
    v_noreplace boolean := p_flags & 1 <> 0;
    v_inos bigint[];
    i bigint;
    v_path bytea;
    v_new_path bytea;
    x record;
    attempt integer := 0;
BEGIN
    IF p_flags & ~3 <> 0 OR (v_exchange AND v_noreplace) THEN
        RAISE EXCEPTION 'unsupported rename flags %', p_flags USING ERRCODE = 'PF007';
    END IF;
    PERFORM pgfs._check_name(p_name);
    PERFORM pgfs._check_name(p_newname);
    e := pgfs._begin(p_layer, p_mount, p_fence);
    v_root := pgfs._root_of(p_layer);
    replaced_ino := NULL;
    replaced_nlink := NULL;
    replaced_orphaned := false;
    LOOP
        attempt := attempt + 1;
        -- 1. Discover.
        s := pgfs._dentry(p_layer, e, p_parent, p_name);
        IF s.ino IS NULL THEN
            RAISE EXCEPTION 'no such name' USING ERRCODE = 'PF002';
        END IF;
        t := pgfs._dentry(p_layer, e, p_newparent, p_newname);
        v_src := s.ino;
        v_tgt := t.ino;
        IF v_src = v_root OR v_tgt = v_root THEN
            RAISE EXCEPTION 'root of the view' USING ERRCODE = 'PF018';
        END IF;
        IF v_tgt = v_src THEN
            -- Same inode under both names: POSIX says succeed and do nothing.
            ino := v_src;
            PERFORM pgfs._done(p_mount, p_worker, p_op_seq, jsonb_build_object('ino', v_src));
            RETURN NEXT;
            RETURN;
        END IF;
        IF v_exchange AND v_tgt IS NULL THEN
            RAISE EXCEPTION 'exchange target does not exist' USING ERRCODE = 'PF002';
        END IF;
        IF v_noreplace AND v_tgt IS NOT NULL THEN
            RAISE EXCEPTION 'target exists' USING ERRCODE = 'PF003';
        END IF;
        IF NOT v_exchange AND v_tgt IS NOT NULL THEN
            IF s.kind = 'd' AND t.kind <> 'd' THEN
                RAISE EXCEPTION 'target is not a directory' USING ERRCODE = 'PF006';
            END IF;
            IF s.kind <> 'd' AND t.kind = 'd' THEN
                RAISE EXCEPTION 'target is a directory' USING ERRCODE = 'PF005';
            END IF;
        END IF;
        PERFORM pgfs._check_dirty(ARRAY[v_src, v_tgt], p_dirty);
        -- Directories changing parent: the rename lock, then the cycle check.
        IF p_parent <> p_newparent AND (s.kind = 'd' OR (v_exchange AND t.kind = 'd')) THEN
            PERFORM pg_advisory_xact_lock(1885759092, p_layer::integer);
            IF s.kind = 'd' AND pgfs._is_within(p_layer, e, p_newparent, v_src, v_root) THEN
                RAISE EXCEPTION 'cannot move a directory into itself' USING ERRCODE = 'PF007';
            END IF;
            IF v_exchange AND t.kind = 'd' AND pgfs._is_within(p_layer, e, p_parent, v_tgt, v_root) THEN
                RAISE EXCEPTION 'cannot move a directory into itself' USING ERRCODE = 'PF007';
            END IF;
        END IF;
        -- 2. Lock inodes in ascending order.
        SELECT array_agg(DISTINCT u ORDER BY u) INTO v_inos
        FROM unnest(ARRAY[p_parent, p_newparent, v_src, v_tgt]) u WHERE u IS NOT NULL;
        FOREACH i IN ARRAY v_inos LOOP
            IF pgfs._inode_lock(p_layer, e, i) IS NULL THEN
                RAISE EXCEPTION 'inode % does not exist', i USING ERRCODE = 'PF002';
            END IF;
        END LOOP;
        -- 3. Lock dentries in ascending (parent, name) order; revalidate.
        IF (p_parent, p_name) < (p_newparent, p_newname) THEN
            PERFORM pgfs._dentry_lock(p_layer, p_parent, p_name);
            PERFORM pgfs._dentry_lock(p_layer, p_newparent, p_newname);
        ELSE
            PERFORM pgfs._dentry_lock(p_layer, p_newparent, p_newname);
            PERFORM pgfs._dentry_lock(p_layer, p_parent, p_name);
        END IF;
        s := pgfs._dentry(p_layer, e, p_parent, p_name);
        t := pgfs._dentry(p_layer, e, p_newparent, p_newname);
        EXIT WHEN s.ino IS NOT DISTINCT FROM v_src AND t.ino IS NOT DISTINCT FROM v_tgt;
        IF attempt >= 3 THEN
            RAISE EXCEPTION 'rename raced' USING ERRCODE = '40001';
        END IF;
    END LOOP;
    IF (SELECT kind FROM pgfs.inodes WHERE layer = p_layer AND inodes.ino = p_newparent
          AND died = 9223372036854775807) <> 'd' THEN
        RAISE EXCEPTION 'target parent is not a directory' USING ERRCODE = 'PF006';
    END IF;
    v_path := pgfs._join(pgfs._path(p_layer, e, p_parent, v_root), p_name);
    v_new_path := pgfs._join(pgfs._path(p_layer, e, p_newparent, v_root), p_newname);

    IF v_exchange THEN
        PERFORM pgfs._dentry_set(p_layer, e, p_parent, p_name, v_tgt, t.kind);
        PERFORM pgfs._dentry_set(p_layer, e, p_newparent, p_newname, v_src, s.kind);
        IF p_parent <> p_newparent AND (s.kind = 'd') <> (t.kind = 'd') THEN
            UPDATE pgfs.inodes SET nlink = nlink + CASE WHEN s.kind = 'd' THEN -1 ELSE 1 END
            WHERE layer = p_layer AND inodes.ino = p_parent AND born = e;
            UPDATE pgfs.inodes SET nlink = nlink + CASE WHEN s.kind = 'd' THEN 1 ELSE -1 END
            WHERE layer = p_layer AND inodes.ino = p_newparent AND born = e;
        END IF;
        UPDATE pgfs.inodes SET ctime_ns = p_now, version = version + 1
        WHERE layer = p_layer AND inodes.ino IN (v_src, v_tgt) AND born = e;
        UPDATE pgfs.inodes SET mtime_ns = p_now, ctime_ns = p_now, version = version + 1
        WHERE layer = p_layer AND inodes.ino IN (p_parent, p_newparent) AND born = e;
        PERFORM pgfs._change(p_layer, 'exchange', v_src, s.kind, p_parent, p_name, v_path,
                             p_newparent, p_newname, v_new_path);
        ino := v_src;
        PERFORM pgfs._done(p_mount, p_worker, p_op_seq, jsonb_build_object('ino', v_src));
        RETURN NEXT;
        RETURN;
    END IF;

    IF v_tgt IS NOT NULL THEN
        replaced_ino := v_tgt;
        IF t.kind = 'd' THEN
            IF pgfs._dir_has_entries(p_layer, e, v_tgt) THEN
                RAISE EXCEPTION 'target directory not empty' USING ERRCODE = 'PF004';
            END IF;
            PERFORM pgfs._dir_delete(p_layer, e, v_tgt);
            UPDATE pgfs.inodes SET nlink = nlink - 1
            WHERE layer = p_layer AND inodes.ino = p_newparent AND born = e;
            replaced_nlink := 0;
        ELSE
            PERFORM pgfs._dentry_set(p_layer, e, p_newparent, p_newname, v_src, s.kind);
            x := pgfs._drop_link(p_layer, e, v_tgt, p_open, p_now);
            replaced_nlink := x.nlink;
            replaced_orphaned := x.orphaned;
        END IF;
        IF t.kind = 'd' THEN
            PERFORM pgfs._dentry_set(p_layer, e, p_newparent, p_newname, v_src, s.kind);
        END IF;
    ELSE
        PERFORM pgfs._dentry_insert(p_layer, e, p_newparent, p_newname, v_src, s.kind);
    END IF;
    PERFORM pgfs._dentry_set(p_layer, e, p_parent, p_name, NULL, NULL);
    IF s.kind = 'd' AND p_parent <> p_newparent THEN
        UPDATE pgfs.inodes SET nlink = nlink - 1
        WHERE layer = p_layer AND inodes.ino = p_parent AND born = e;
        UPDATE pgfs.inodes SET nlink = nlink + 1
        WHERE layer = p_layer AND inodes.ino = p_newparent AND born = e;
    END IF;
    UPDATE pgfs.inodes SET ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND inodes.ino = v_src AND born = e;
    UPDATE pgfs.inodes SET mtime_ns = p_now, ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND inodes.ino IN (p_parent, p_newparent) AND born = e;
    PERFORM pgfs._change(p_layer, 'rename', v_src, s.kind, p_parent, p_name, v_path,
                         p_newparent, p_newname, v_new_path);
    ino := v_src;
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq,
        jsonb_build_object('ino', v_src, 'replaced_ino', replaced_ino,
                           'replaced_nlink', replaced_nlink, 'replaced_orphaned', replaced_orphaned));
    RETURN NEXT;
END $$;

-- ── Attributes and data ────────────────────────────────────────────────

CREATE FUNCTION pgfs._kill_priv(p_mode integer) RETURNS integer
LANGUAGE sql IMMUTABLE AS $$
    -- Clear set-user-id, and set-group-id when group execute is set.
    SELECT CASE WHEN p_mode & 8 <> 0 THEN p_mode & ~3072 ELSE p_mode & ~2048 END
$$;

-- Apply a new size to the locked inode row `r` (born in this epoch),
-- returning the updated row (spec/app.py TruncatesFiles). `p_tail` is the
-- block that now holds the end of the file, when the end falls inside a
-- block that has data (block number `p_tail_block`), computed by the daemon
-- with the tail zeroed or, for an inline file growing past inline_max, the
-- inline content as block 0.
CREATE FUNCTION pgfs._set_size(p_layer bigint, p_epoch bigint, r pgfs.inodes, p_size bigint,
                               p_tail_block bigint, p_tail bytea, p_tail_crc integer)
RETURNS pgfs.inodes LANGUAGE plpgsql AS $$
DECLARE
    bs bigint := pgfs._meta_int('block_size');
    v_inline_max bigint := pgfs._meta_int('inline_max');
    v_blob bigint;
    v_len integer;
BEGIN
    IF r.kind = 'd' THEN
        RAISE EXCEPTION 'is a directory' USING ERRCODE = 'PF005';
    END IF;
    IF r.kind <> 'f' THEN
        RAISE EXCEPTION 'not a regular file' USING ERRCODE = 'PF007';
    END IF;
    IF p_size < 0 THEN
        RAISE EXCEPTION 'negative size' USING ERRCODE = 'PF007';
    END IF;
    IF r.inline_data IS NOT NULL THEN
        v_len := octet_length(r.inline_data);
        IF p_size <= v_inline_max THEN
            IF p_size <= v_len THEN
                r.inline_data := substring(r.inline_data FROM 1 FOR p_size::integer);
            ELSE
                r.inline_data := r.inline_data || decode(repeat('00', (p_size - v_len)::integer), 'hex');
            END IF;
            r.size := p_size;
            RETURN r;
        END IF;
        -- Growing past inline_max: the inline bytes become block 0.
        r.inline_data := NULL;
    ELSIF p_size < r.size THEN
        PERFORM pgfs._blocks_cut(p_layer, p_epoch, r.ino, (p_size + bs - 1) / bs, 4096);
        r.inherit_limit := LEAST(r.inherit_limit, p_size / bs);
    END IF;
    IF p_tail IS NOT NULL THEN
        v_blob := nextval('pgfs.blob_seq');
        INSERT INTO pgfs.blobs (blob_id, len, crc32c, data)
        VALUES (v_blob, octet_length(p_tail), p_tail_crc, p_tail);
        PERFORM pgfs._block_put(p_layer, p_epoch, r.ino, p_tail_block, v_blob);
    ELSIF p_size < r.size AND p_size % bs <> 0 THEN
        PERFORM pgfs._block_hole(p_layer, p_epoch, r, p_size / bs);
    END IF;
    r.size := p_size;
    RETURN r;
END $$;

-- setattr: every attribute given, in one transaction (spec/posix.py
-- SetsAttributes, Timestamps; spec/security.py ClearsSetIdBits).
-- NULL arguments are left unchanged.
CREATE FUNCTION pgfs.setattr(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                             p_op_seq bigint, p_ino bigint, p_gen bigint, p_mode integer,
                             p_uid bigint, p_gid bigint, p_size bigint, p_tail_block bigint,
                             p_tail bytea, p_tail_crc integer, p_atime_ns bigint,
                             p_mtime_ns bigint, p_caller_uid bigint, p_now bigint)
RETURNS pgfs.attr LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    r pgfs.inodes;
    a pgfs.attr;
BEGIN
    e := pgfs._begin(p_layer, p_mount, p_fence);
    r := pgfs._inode_lock(p_layer, e, p_ino);
    IF r IS NULL OR r.gen <> COALESCE(p_gen, r.gen) THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    IF p_mode IS NOT NULL THEN
        r.mode := p_mode & 4095;
    END IF;
    IF (p_uid IS NOT NULL AND p_uid <> r.uid) OR (p_gid IS NOT NULL AND p_gid <> r.gid) THEN
        IF r.kind = 'f' AND p_mode IS NULL THEN
            r.mode := pgfs._kill_priv(r.mode);
        END IF;
        r.uid := COALESCE(p_uid, r.uid);
        r.gid := COALESCE(p_gid, r.gid);
    END IF;
    IF p_size IS NOT NULL THEN
        r := pgfs._set_size(p_layer, e, r, p_size, p_tail_block, p_tail, p_tail_crc);
        r.mtime_ns := p_now;
        IF p_caller_uid IS NOT NULL AND p_caller_uid NOT IN (0, r.uid) AND p_mode IS NULL THEN
            r.mode := pgfs._kill_priv(r.mode);
        END IF;
    END IF;
    IF p_atime_ns IS NOT NULL THEN
        r.atime_ns := p_atime_ns;
    END IF;
    IF p_mtime_ns IS NOT NULL THEN
        r.mtime_ns := p_mtime_ns;
    END IF;
    r.ctime_ns := p_now;
    PERFORM pgfs._inode_put(r);
    r.version := r.version + 1;
    PERFORM pgfs._change(p_layer, CASE WHEN p_size IS NOT NULL THEN 'modify' ELSE 'setattr' END,
                         p_ino, r.kind, NULL, NULL, pgfs._ino_path(p_layer, e, p_ino));
    a := pgfs._attr(p_layer, e, r);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, to_jsonb(a));
    RETURN a;
END $$;

-- Commit data to a regular file: the buffered writes of a flush, a sync
-- write, a hole punch or zero range (spec/optimize.py
-- BufferedWriteFlushReq, spec/offload.py FallocatePunchHoleReq).
--   p_inline       new inline content (the file stays inline), or NULL;
--   p_block_nos …  whole blocks to store as new blobs, each with its CRC32C
--                  computed by the daemon (spec/integrity.py BlobChecksums);
--   p_hole_first … blocks to make read as zero, applied first;
--   p_size         the file's size afterwards.
CREATE FUNCTION pgfs.write_blocks(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                                  p_op_seq bigint, p_ino bigint, p_gen bigint, p_size bigint,
                                  p_inline bytea, p_block_nos bigint[], p_datas bytea[],
                                  p_crcs integer[], p_hole_first bigint, p_hole_last bigint,
                                  p_mtime_ns bigint, p_now bigint, p_writer_uids bigint[],
                                  p_sync boolean)
RETURNS pgfs.attr LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    r pgfs.inodes;
    bs bigint := pgfs._meta_int('block_size');
    i integer;
    v_blob bigint;
    b record;
    a pgfs.attr;
BEGIN
    e := pgfs._begin(p_layer, p_mount, p_fence);
    r := pgfs._inode_lock(p_layer, e, p_ino);
    IF r IS NULL OR r.gen <> COALESCE(p_gen, r.gen) THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    IF r.kind = 'd' THEN
        RAISE EXCEPTION 'is a directory' USING ERRCODE = 'PF005';
    END IF;
    IF r.kind <> 'f' THEN
        RAISE EXCEPTION 'not a regular file' USING ERRCODE = 'PF007';
    END IF;
    IF p_inline IS NOT NULL THEN
        IF r.inline_data IS NULL AND r.size > 0 THEN
            RAISE EXCEPTION 'inode % is not inline', p_ino USING ERRCODE = 'PF007';
        END IF;
        PERFORM pgfs._blocks_cut(p_layer, e, p_ino, 0, 4096);
        r.inline_data := p_inline;
    ELSE
        IF p_hole_first IS NOT NULL THEN
            IF r.shadows_base AND pgfs._has_base(p_layer) THEN
                FOR b IN SELECT m.block_no FROM pgfs._blocks(p_layer, e, p_ino, p_hole_first, p_hole_last) m
                         WHERE m.blob_id IS NOT NULL LOOP
                    PERFORM pgfs._block_hole(p_layer, e, r, b.block_no);
                END LOOP;
            ELSE
                FOR b IN SELECT f.block_no FROM pgfs.file_blocks f
                         WHERE f.layer = p_layer AND f.ino = p_ino AND f.died = 9223372036854775807
                           AND f.block_no BETWEEN p_hole_first AND p_hole_last LOOP
                    PERFORM pgfs._block_hole(p_layer, e, r, b.block_no);
                END LOOP;
            END IF;
        END IF;
        IF p_block_nos IS NOT NULL THEN
            FOR i IN 1 .. COALESCE(array_length(p_block_nos, 1), 0) LOOP
                IF octet_length(p_datas[i]) > bs THEN
                    RAISE EXCEPTION 'block longer than the block size' USING ERRCODE = 'PF007';
                END IF;
                v_blob := nextval('pgfs.blob_seq');
                INSERT INTO pgfs.blobs (blob_id, len, crc32c, data)
                VALUES (v_blob, octet_length(p_datas[i]), p_crcs[i], p_datas[i]);
                PERFORM pgfs._block_put(p_layer, e, p_ino, p_block_nos[i], v_blob);
            END LOOP;
        END IF;
        IF r.inline_data IS NOT NULL AND (p_block_nos IS NOT NULL OR p_size > pgfs._meta_int('inline_max')) THEN
            r.inline_data := NULL;
        END IF;
    END IF;
    r.size := p_size;
    r.mtime_ns := COALESCE(p_mtime_ns, p_now);
    r.ctime_ns := p_now;
    IF EXISTS (SELECT 1 FROM unnest(COALESCE(p_writer_uids, '{}')) u WHERE u NOT IN (0, r.uid))
       AND r.mode & 3072 <> 0 THEN
        r.mode := pgfs._kill_priv(r.mode);
    END IF;
    PERFORM pgfs._inode_put(r);
    r.version := r.version + 1;
    PERFORM pgfs._change(p_layer, 'modify', p_ino, 'f', NULL, NULL, pgfs._ino_path(p_layer, e, p_ino));
    PERFORM pgfs._sync(p_sync);
    a := pgfs._attr(p_layer, e, r);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, to_jsonb(a));
    RETURN a;
END $$;

-- Server-side copy_file_range (spec/offload.py CopyFileRangeReq): the
-- destination blocks p_dst_first .. p_dst_first + p_count - 1 are mapped
-- to the source's blobs from p_src_first on, which become shared
-- (spec/space.py CollectsSharedBlobs); partly covered blocks come from the
-- daemon as new blobs, like write_blocks.
CREATE FUNCTION pgfs.copy_range(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                                p_op_seq bigint, p_in bigint, p_gen_in bigint, p_out bigint,
                                p_gen_out bigint, p_src_first bigint, p_dst_first bigint,
                                p_count bigint, p_block_nos bigint[], p_datas bytea[],
                                p_crcs integer[], p_size bigint, p_now bigint)
RETURNS pgfs.attr LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    ri pgfs.inodes;
    ro pgfs.inodes;
    i integer;
    v_blob bigint;
    b record;
    k bigint;
    a pgfs.attr;
BEGIN
    e := pgfs._begin(p_layer, p_mount, p_fence);
    IF p_in < p_out THEN
        ri := pgfs._inode_lock(p_layer, e, p_in);
        ro := pgfs._inode_lock(p_layer, e, p_out);
    ELSIF p_in > p_out THEN
        ro := pgfs._inode_lock(p_layer, e, p_out);
        ri := pgfs._inode_lock(p_layer, e, p_in);
    ELSE
        ro := pgfs._inode_lock(p_layer, e, p_out);
        ri := ro;
    END IF;
    IF ri IS NULL OR ri.gen <> COALESCE(p_gen_in, ri.gen) OR ro IS NULL
       OR ro.gen <> COALESCE(p_gen_out, ro.gen) THEN
        RAISE EXCEPTION 'inode is stale' USING ERRCODE = 'PF019';
    END IF;
    IF ri.kind <> 'f' OR ro.kind <> 'f' THEN
        RAISE EXCEPTION 'not a regular file' USING ERRCODE = 'PF005';
    END IF;
    IF p_count > 0 THEN
        IF ri.inline_data IS NOT NULL THEN
            RAISE EXCEPTION 'cannot share blocks of an inline file' USING ERRCODE = 'PF007';
        END IF;
        FOR k IN 0 .. p_count - 1 LOOP
            SELECT m.blob_id INTO v_blob
            FROM pgfs._blocks(p_layer, e, p_in, p_src_first + k, p_src_first + k) m;
            IF v_blob IS NULL THEN
                PERFORM pgfs._block_hole(p_layer, e, ro, p_dst_first + k);
            ELSE
                UPDATE pgfs.blobs SET shared = true WHERE blob_id = v_blob AND NOT shared;
                PERFORM pgfs._block_put(p_layer, e, p_out, p_dst_first + k, v_blob);
            END IF;
        END LOOP;
    END IF;
    FOR i IN 1 .. COALESCE(array_length(p_block_nos, 1), 0) LOOP
        v_blob := nextval('pgfs.blob_seq');
        INSERT INTO pgfs.blobs (blob_id, len, crc32c, data)
        VALUES (v_blob, octet_length(p_datas[i]), p_crcs[i], p_datas[i]);
        PERFORM pgfs._block_put(p_layer, e, p_out, p_block_nos[i], v_blob);
    END LOOP;
    SELECT * INTO ro FROM pgfs.inodes WHERE layer = p_layer AND ino = p_out AND born = e;
    ro.inline_data := NULL;
    ro.size := GREATEST(ro.size, p_size);
    ro.mtime_ns := p_now;
    ro.ctime_ns := p_now;
    PERFORM pgfs._inode_put(ro);
    ro.version := ro.version + 1;
    PERFORM pgfs._change(p_layer, 'modify', p_out, 'f', NULL, NULL, pgfs._ino_path(p_layer, e, p_out));
    a := pgfs._attr(p_layer, e, ro);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, to_jsonb(a));
    RETURN a;
END $$;

-- ── Extended attributes (spec/posix.py ExtendedAttributes) ─────────────

-- p_flags: 1 XATTR_CREATE, 2 XATTR_REPLACE.
CREATE FUNCTION pgfs.setxattr(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                              p_op_seq bigint, p_ino bigint, p_name bytea, p_value bytea,
                              p_flags integer, p_now bigint)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    r pgfs.inodes;
    x pgfs.xattrs;
    own pgfs.xattrs;
BEGIN
    IF octet_length(p_value) > 65536 THEN
        RAISE EXCEPTION 'xattr value too large' USING ERRCODE = 'PF021';
    END IF;
    IF octet_length(p_name) > 255 OR octet_length(p_name) = 0 THEN
        RAISE EXCEPTION 'xattr name length' USING ERRCODE = 'PF023';
    END IF;
    e := pgfs._begin(p_layer, p_mount, p_fence);
    r := pgfs._inode_lock(p_layer, e, p_ino);
    IF r IS NULL THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    SELECT * INTO own FROM pgfs.xattrs
    WHERE layer = p_layer AND ino = p_ino AND name = p_name AND died = 9223372036854775807
    FOR UPDATE;
    x := pgfs._xattr(p_layer, e, p_ino, p_name);
    IF p_flags & 1 <> 0 AND x.value IS NOT NULL THEN
        RAISE EXCEPTION 'xattr exists' USING ERRCODE = 'PF003';
    END IF;
    IF p_flags & 2 <> 0 AND x.value IS NULL THEN
        RAISE EXCEPTION 'no such xattr' USING ERRCODE = 'PF020';
    END IF;
    IF own.ino IS NOT NULL THEN
        IF own.born = e THEN
            UPDATE pgfs.xattrs SET value = p_value
            WHERE layer = p_layer AND ino = p_ino AND name = p_name AND born = e;
        ELSE
            UPDATE pgfs.xattrs SET died = e
            WHERE layer = p_layer AND ino = p_ino AND name = p_name AND born = own.born;
            INSERT INTO pgfs.xattrs (layer, ino, name, value, born, shadows_base)
            VALUES (p_layer, p_ino, p_name, p_value, e, own.shadows_base);
        END IF;
    ELSE
        INSERT INTO pgfs.xattrs (layer, ino, name, value, born, shadows_base)
        VALUES (p_layer, p_ino, p_name, p_value, e, x.value IS NOT NULL);
    END IF;
    UPDATE pgfs.inodes SET ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND ino = p_ino AND born = e;
    PERFORM pgfs._change(p_layer, 'setxattr', p_ino, r.kind, NULL, p_name, pgfs._ino_path(p_layer, e, p_ino));
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, '{}'::jsonb);
END $$;

CREATE FUNCTION pgfs.removexattr(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                                 p_op_seq bigint, p_ino bigint, p_name bytea, p_now bigint)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    r pgfs.inodes;
    x pgfs.xattrs;
    own pgfs.xattrs;
BEGIN
    e := pgfs._begin(p_layer, p_mount, p_fence);
    r := pgfs._inode_lock(p_layer, e, p_ino);
    IF r IS NULL THEN
        RAISE EXCEPTION 'inode % is stale', p_ino USING ERRCODE = 'PF019';
    END IF;
    SELECT * INTO own FROM pgfs.xattrs
    WHERE layer = p_layer AND ino = p_ino AND name = p_name AND died = 9223372036854775807
    FOR UPDATE;
    x := pgfs._xattr(p_layer, e, p_ino, p_name);
    IF x.value IS NULL THEN
        RAISE EXCEPTION 'no such xattr' USING ERRCODE = 'PF020';
    END IF;
    IF own.ino IS NOT NULL THEN
        IF own.shadows_base THEN
            IF own.born = e THEN
                UPDATE pgfs.xattrs SET value = NULL
                WHERE layer = p_layer AND ino = p_ino AND name = p_name AND born = e;
            ELSE
                UPDATE pgfs.xattrs SET died = e
                WHERE layer = p_layer AND ino = p_ino AND name = p_name AND born = own.born;
                INSERT INTO pgfs.xattrs (layer, ino, name, value, born, shadows_base)
                VALUES (p_layer, p_ino, p_name, NULL, e, true);
            END IF;
        ELSIF own.born = e THEN
            DELETE FROM pgfs.xattrs WHERE layer = p_layer AND ino = p_ino AND name = p_name AND born = e;
        ELSE
            UPDATE pgfs.xattrs SET died = e
            WHERE layer = p_layer AND ino = p_ino AND name = p_name AND born = own.born;
        END IF;
    ELSE
        INSERT INTO pgfs.xattrs (layer, ino, name, value, born, shadows_base)
        VALUES (p_layer, p_ino, p_name, NULL, e, true);
    END IF;
    UPDATE pgfs.inodes SET ctime_ns = p_now, version = version + 1
    WHERE layer = p_layer AND ino = p_ino AND born = e;
    PERFORM pgfs._change(p_layer, 'removexattr', p_ino, r.kind, NULL, p_name, pgfs._ino_path(p_layer, e, p_ino));
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, '{}'::jsonb);
END $$;

-- ── Orphans and barriers ───────────────────────────────────────────────

-- The last handle of an unlinked file was released (spec/posix.py
-- OpenAfterUnlink). Returns false if the inode is not an orphan.
CREATE FUNCTION pgfs.release_orphan(p_layer bigint, p_mount uuid, p_fence bigint, p_worker integer,
                                    p_op_seq bigint, p_ino bigint)
RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    r pgfs.inodes;
BEGIN
    e := pgfs._begin(p_layer, p_mount, p_fence);
    IF NOT EXISTS (SELECT 1 FROM pgfs.orphans WHERE layer = p_layer AND ino = p_ino) THEN
        RETURN false;
    END IF;
    r := pgfs._inode_lock(p_layer, e, p_ino);
    IF r IS NULL OR r.nlink > 0 THEN
        DELETE FROM pgfs.orphans WHERE layer = p_layer AND ino = p_ino;
        RETURN false;
    END IF;
    PERFORM pgfs._inode_delete(p_layer, e, r);
    PERFORM pgfs._done(p_mount, p_worker, p_op_seq, '{}'::jsonb);
    RETURN true;
END $$;

-- Reclaim one orphan of the layer after the lease was acquired
-- (spec/space.py ReclaimsOrphans). Returns its ino, or NULL when none is left.
CREATE FUNCTION pgfs.reclaim_orphan(p_layer bigint, p_mount uuid, p_fence bigint)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE
    e bigint;
    v_ino bigint;
    r pgfs.inodes;
BEGIN
    e := pgfs._begin(p_layer, p_mount, p_fence);
    SELECT ino INTO v_ino FROM pgfs.orphans WHERE layer = p_layer ORDER BY ino LIMIT 1 FOR UPDATE;
    IF v_ino IS NULL THEN
        RETURN NULL;
    END IF;
    r := pgfs._inode_lock(p_layer, e, v_ino);
    IF r IS NULL OR r.nlink > 0 THEN
        DELETE FROM pgfs.orphans WHERE layer = p_layer AND ino = v_ino;
    ELSE
        PERFORM pgfs._inode_delete(p_layer, e, r);
    END IF;
    RETURN v_ino;
END $$;

-- The synchronous barrier (spec/durability.py FsyncForcesEarlierCommits):
-- a HOT update of the worker's op_log slot committed with
-- synchronous_commit = on proves every earlier commit is flushed and
-- replicated to the synchronous standbys.
CREATE FUNCTION pgfs.sync_barrier(p_mount uuid, p_worker integer, p_op_seq bigint)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('synchronous_commit', 'on', true);
    UPDATE pgfs.op_log SET op_seq = p_op_seq, result = '{}'::jsonb
    WHERE mount_id = p_mount AND worker_id = p_worker;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'op_log slot (%, %) is missing', p_mount, p_worker USING ERRCODE = 'PF001';
    END IF;
END $$;

-- Space in use, folded totals plus unfolded deltas (spec/space.py
-- ReportsStatfs, TracksUsage).
CREATE FUNCTION pgfs.fs_usage(OUT physical_bytes bigint, OUT inodes bigint)
LANGUAGE sql STABLE AS $$
    SELECT (COALESCE((SELECT sum(physical_bytes) FROM pgfs.usage_totals), 0)
          + COALESCE((SELECT sum(physical_bytes) FROM pgfs.usage_deltas), 0))::bigint,
           (COALESCE((SELECT sum(inodes) FROM pgfs.usage_totals WHERE layer <> 0), 0)
          + COALESCE((SELECT sum(inodes) FROM pgfs.usage_deltas), 0))::bigint
$$;

CREATE FUNCTION pgfs.layer_usage(p_layer bigint, OUT logical_bytes bigint, OUT inodes bigint)
LANGUAGE sql STABLE AS $$
    SELECT (COALESCE((SELECT logical_bytes FROM pgfs.usage_totals WHERE layer = p_layer), 0)
          + COALESCE((SELECT sum(logical_bytes) FROM pgfs.usage_deltas WHERE layer = p_layer), 0))::bigint,
           (COALESCE((SELECT inodes FROM pgfs.usage_totals WHERE layer = p_layer), 0)
          + COALESCE((SELECT sum(inodes) FROM pgfs.usage_deltas WHERE layer = p_layer), 0))::bigint
$$;

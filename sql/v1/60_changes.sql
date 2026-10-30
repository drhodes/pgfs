-- pgfs schema version 1: the change feed (spec/changefeed.py).
--
-- A watermark token is jsonb:
--   {layer, low, high, s_cap, last_seq, held}
-- with `low`/`high` transaction ids (xid8 as text) bounding the current
-- window, `s_cap` the sequence cap of that window, `last_seq` the last row
-- delivered in it, and `held` the seqs held back to the next window
-- (spec/changefeed.py HorizonOrderedDelivery).

CREATE TABLE pgfs.change_prune (
    layer     bigint PRIMARY KEY,
    below_xid xid8 NOT NULL
);

CREATE FUNCTION pgfs.changes_token(p_layer bigint, p_xid xid8 DEFAULT NULL) RETURNS jsonb
LANGUAGE sql STABLE AS $$
    SELECT jsonb_build_object('layer', p_layer,
        'low', COALESCE(p_xid, pg_snapshot_xmin(pg_current_snapshot()))::text,
        'high', COALESCE(p_xid, pg_snapshot_xmin(pg_current_snapshot()))::text,
        's_cap', NULL, 'last_seq', 0, 'held', '[]'::jsonb)
$$;

-- One page of changes after `p_token`, and the token that follows it.
CREATE FUNCTION pgfs.changes_page(p_token jsonb, p_limit integer)
RETURNS TABLE (seq bigint, xid xid8, at timestamptz, op text, ino bigint, kind "char",
               parent_ino bigint, name bytea, new_parent_ino bigint, new_name bytea,
               path bytea, new_path bytea, watermark jsonb)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    v_layer bigint := (p_token->>'layer')::bigint;
    v_low xid8 := (p_token->>'low')::xid8;
    v_high xid8 := (p_token->>'high')::xid8;
    v_cap bigint := (p_token->>'s_cap')::bigint;
    v_last bigint := COALESCE((p_token->>'last_seq')::bigint, 0);
    v_held bigint[] := ARRAY(SELECT jsonb_array_elements_text(COALESCE(p_token->'held', '[]'))::bigint);
    v_pruned xid8;
    v_count integer := 0;
    v_tok jsonb;
    c record;
BEGIN
    SELECT below_xid INTO v_pruned FROM pgfs.change_prune WHERE layer = v_layer;
    IF v_pruned IS NOT NULL AND v_low < v_pruned THEN
        RAISE EXCEPTION 'cursor expired: changes before % were pruned; resync from a snapshot', v_pruned
            USING ERRCODE = 'PF010';
    END IF;
    IF v_low = v_high THEN
        -- A new window: horizon and sequence cap from one snapshot.
        SELECT h, cap INTO v_high, v_cap FROM (
            SELECT pg_snapshot_xmin(pg_current_snapshot()) AS h,
                   (SELECT min(x.seq) FROM pgfs.changes x
                    WHERE x.layer = v_layer AND x.xid >= pg_snapshot_xmin(pg_current_snapshot())) AS cap) s;
        v_last := 0;
    END IF;
    v_tok := jsonb_build_object('layer', v_layer, 'low', v_low::text, 'high', v_high::text,
                                's_cap', v_cap, 'held', to_jsonb(v_held));
    FOR c IN SELECT * FROM pgfs.changes x
             WHERE x.layer = v_layer
               AND (x.seq = ANY (v_held) OR (x.xid >= v_low AND x.xid < v_high))
               AND x.seq > v_last AND (v_cap IS NULL OR x.seq < v_cap)
             ORDER BY x.seq LIMIT p_limit LOOP
        v_count := v_count + 1;
        v_last := c.seq;
        seq := c.seq; xid := c.xid; at := c.at; op := c.op; ino := c.ino; kind := c.kind;
        parent_ino := c.parent_ino; name := c.name; new_parent_ino := c.new_parent_ino;
        new_name := c.new_name; path := c.path; new_path := c.new_path;
        watermark := NULL;
        RETURN NEXT;
    END LOOP;
    IF v_count >= p_limit THEN
        v_tok := v_tok || jsonb_build_object('last_seq', v_last);
    ELSE
        -- The window is exhausted: hold back what lies at or past the cap.
        v_tok := jsonb_build_object('layer', v_layer, 'low', v_high::text, 'high', v_high::text,
            's_cap', NULL, 'last_seq', 0,
            'held', COALESCE((SELECT jsonb_agg(x.seq ORDER BY x.seq) FROM pgfs.changes x
                              WHERE x.layer = v_layer AND v_cap IS NOT NULL AND x.seq >= v_cap
                                AND (x.seq = ANY (v_held) OR (x.xid >= v_low AND x.xid < v_high))),
                             '[]'::jsonb));
    END IF;
    -- The token travels on a final row with no change.
    seq := NULL; xid := NULL; at := NULL; op := NULL; ino := NULL; kind := NULL;
    parent_ino := NULL; name := NULL; new_parent_ino := NULL; new_name := NULL;
    path := NULL; new_path := NULL;
    watermark := v_tok;
    RETURN NEXT;
END $$;

-- Register a consumer of a tree's changes (spec/changefeed.py ResumableCursors).
CREATE FUNCTION pgfs.changes_register(p_name text, p_tree text, p_at_tree text DEFAULT NULL,
                                      p_at_snapshot text DEFAULT NULL)
RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE
    t pgfs.trees;
    v_xid xid8;
    v_tok jsonb;
BEGIN
    SELECT * INTO t FROM pgfs.trees WHERE name = p_tree;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'tree % does not exist', p_tree USING ERRCODE = 'PF002';
    END IF;
    IF p_at_snapshot IS NOT NULL THEN
        SELECT xid INTO v_xid FROM pgfs.snapshots WHERE tree = COALESCE(p_at_tree, p_tree) AND name = p_at_snapshot;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'snapshot does not exist' USING ERRCODE = 'PF002';
        END IF;
    END IF;
    v_tok := pgfs.changes_token(t.layer, v_xid);
    INSERT INTO pgfs.change_consumers (name, tree, layer, watermark) VALUES (p_name, p_tree, t.layer, v_tok);
    RETURN v_tok;
EXCEPTION WHEN unique_violation THEN
    RAISE EXCEPTION 'consumer % exists', p_name USING ERRCODE = 'PF003';
END $$;

CREATE FUNCTION pgfs.changes_unregister(p_name text) RETURNS boolean
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM pgfs.change_consumers WHERE name = p_name;
    RETURN FOUND;
END $$;

-- The next page for a registered consumer, from its acknowledged token.
CREATE FUNCTION pgfs.changes_next(p_name text, p_limit integer)
RETURNS TABLE (seq bigint, xid xid8, at timestamptz, op text, ino bigint, kind "char",
               parent_ino bigint, name bytea, new_parent_ino bigint, new_name bytea,
               path bytea, new_path bytea, watermark jsonb)
LANGUAGE plpgsql AS $$
#variable_conflict use_column
DECLARE
    c pgfs.change_consumers;
    t pgfs.trees;
BEGIN
    SELECT * INTO c FROM pgfs.change_consumers x WHERE x.name = p_name;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'consumer % does not exist', p_name USING ERRCODE = 'PF002';
    END IF;
    SELECT * INTO t FROM pgfs.trees x WHERE x.name = c.tree;
    IF t.layer IS DISTINCT FROM c.layer THEN
        RAISE EXCEPTION 'tree moved: resync from %@%', c.tree,
            COALESCE((SELECT s.name FROM pgfs.snapshots s WHERE s.tree = c.tree AND s.layer = c.layer
                        AND s.name LIKE 'pre-promote-%' ORDER BY s.created_at DESC LIMIT 1), 'pre-promote-<ts>')
            USING ERRCODE = 'PF011';
    END IF;
    RETURN QUERY SELECT * FROM pgfs.changes_page(c.watermark, p_limit);
END $$;

CREATE FUNCTION pgfs.changes_ack(p_name text, p_watermark jsonb) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE pgfs.change_consumers SET watermark = p_watermark, updated_at = now() WHERE name = p_name;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'consumer % does not exist', p_name USING ERRCODE = 'PF002';
    END IF;
END $$;

-- Retention (spec/changefeed.py ChangeRetention): unregister idle
-- consumers, then delete rows older than the retention that every consumer
-- of their layer has passed. Returns the names unregistered.
CREATE FUNCTION pgfs.prune_changes(p_retention_secs double precision, p_expiry_secs double precision)
RETURNS SETOF text LANGUAGE plpgsql AS $$
DECLARE
    l record;
    v_min xid8;
    v_cut xid8;
BEGIN
    RETURN QUERY DELETE FROM pgfs.change_consumers
        WHERE updated_at < now() - make_interval(secs => p_expiry_secs) RETURNING name;
    FOR l IN SELECT DISTINCT layer FROM pgfs.changes LOOP
        SELECT min((watermark->>'low')::xid8) INTO v_min FROM pgfs.change_consumers WHERE layer = l.layer;
        SELECT max(xid) INTO v_cut FROM pgfs.changes x
        WHERE x.layer = l.layer AND x.at < now() - make_interval(secs => p_retention_secs)
          AND (v_min IS NULL OR x.xid < v_min)
          AND NOT EXISTS (SELECT 1 FROM pgfs.change_consumers c
                          WHERE c.layer = l.layer AND c.watermark->'held' @> to_jsonb(x.seq));
        CONTINUE WHEN v_cut IS NULL;
        DELETE FROM pgfs.changes x
        WHERE x.layer = l.layer AND x.xid <= v_cut AND x.at < now() - make_interval(secs => p_retention_secs)
          AND NOT EXISTS (SELECT 1 FROM pgfs.change_consumers c
                          WHERE c.layer = l.layer AND c.watermark->'held' @> to_jsonb(x.seq));
        INSERT INTO pgfs.change_prune (layer, below_xid) VALUES (l.layer, v_cut)
        ON CONFLICT (layer) DO UPDATE SET below_xid = GREATEST(pgfs.change_prune.below_xid, EXCLUDED.below_xid);
    END LOOP;
END $$;

-- The newest change seq of a layer, for coalesced notifications
-- (spec/changefeed.py ChangeNotifications).
CREATE FUNCTION pgfs.changes_head() RETURNS TABLE (layer bigint, seq bigint)
LANGUAGE sql STABLE AS $$
    SELECT layer, max(seq) FROM pgfs.changes GROUP BY layer
$$;

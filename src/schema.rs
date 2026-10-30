//! The `pgfs` schema: formatting, versions, migrations and tuning
//! (spec/storage.py SchemaVersion, FilesystemIdentity; spec/ops.py
//! MakesFilesystem, MigratesSchema, TunesFilesystem,
//! RefusesUnformattedDatabase, SupportedPostgresVersions;
//! spec/durability.py ChecksServerDurability; spec/security.py
//! LeastPrivilegeRoles).

use crate::conn::{Conn, DbError};
use anyhow::{anyhow, bail, Context, Result};

/// The schema version this binary uses.
pub const SCHEMA_VERSION: i32 = 1;

/// Oldest supported server, and the newest major version tested.
pub const MIN_SERVER_VERSION: i32 = 170000;
pub const MAX_TESTED_MAJOR: i32 = 18;

/// Migration `n` takes the schema from version n-1 to n. Migration 1
/// creates it.
const MIGRATIONS: &[&[&str]] = &[&[
    include_str!("../sql/v1/00_tables.sql"),
    include_str!("../sql/v1/10_core.sql"),
    include_str!("../sql/v1/20_ops.sql"),
    include_str!("../sql/v1/30_mounts.sql"),
    include_str!("../sql/v1/40_trees.sql"),
    include_str!("../sql/v1/50_house.sql"),
    include_str!("../sql/v1/60_changes.sql"),
    include_str!("../sql/v1/70_views.sql"),
    include_str!("../sql/v1/80_check.sql"),
]];

/// Every table of schema version 1 (spec/storage.py StorageLayout reference).
pub const TABLES: &[&str] = &[
    "meta",
    "layers",
    "trees",
    "snapshots",
    "epoch_holds",
    "mounts",
    "control_requests",
    "inodes",
    "dentries",
    "blobs",
    "file_blocks",
    "xattrs",
    "orphans",
    "op_log",
    "changes",
    "change_consumers",
    "change_prune",
    "gc_queue",
    "usage_deltas",
    "usage_totals",
    "scrub_state",
];

/// Data and queue tables that are UNLOGGED on a scratch filesystem
/// (spec/durability.py ScratchMode).
pub const SCRATCH_UNLOGGED: &[&str] = &[
    "inodes",
    "dentries",
    "file_blocks",
    "blobs",
    "xattrs",
    "orphans",
    "op_log",
    "changes",
    "usage_deltas",
    "usage_totals",
    "gc_queue",
];

/// The server's identity and settings, read in one round trip.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub version_num: i32,
    pub fsync: bool,
    pub full_page_writes: bool,
    pub data_checksums: bool,
    pub wal_level: String,
    pub wal_writer_delay_ms: u64,
    pub synchronous_standby_names: String,
    pub system_identifier: i64,
    pub timeline: i32,
    pub in_recovery: bool,
    pub is_superuser: bool,
    pub current_user: String,
    /// `meta` rows, or None if the database is unformatted.
    pub meta: Option<std::collections::HashMap<String, String>>,
    pub schema_owner: Option<String>,
}

impl ServerInfo {
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta
            .as_ref()
            .and_then(|m| m.get(key))
            .map(String::as_str)
    }

    pub fn schema_version(&self) -> Option<i32> {
        self.meta("schema_version").and_then(|v| v.parse().ok())
    }

    pub fn fs_uuid(&self) -> Option<&str> {
        self.meta("fs_uuid")
    }

    pub fn scratch(&self) -> bool {
        self.meta("scratch") == Some("true")
    }

    pub fn block_size(&self) -> u64 {
        self.meta("block_size")
            .and_then(|v| v.parse().ok())
            .unwrap_or(65536)
    }

    pub fn inline_max(&self) -> u64 {
        self.meta("inline_max")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2048)
    }

    pub fn capacity(&self) -> Option<u64> {
        self.meta("capacity_bytes").and_then(|v| v.parse().ok())
    }

    /// The asynchronous commit window: three times wal_writer_delay
    /// (spec/durability.py AsyncCommitWindow).
    pub fn async_window_ms(&self) -> u64 {
        3 * self.wal_writer_delay_ms
    }
}

const SERVER_SQL: &str = "SELECT current_setting('server_version_num')::int,
       current_setting('fsync') = 'on',
       current_setting('full_page_writes') = 'on',
       current_setting('data_checksums') = 'on',
       current_setting('wal_level'),
       (SELECT setting::bigint FROM pg_settings WHERE name = 'wal_writer_delay'),
       current_setting('synchronous_standby_names'),
       (SELECT system_identifier FROM pg_control_system()),
       (SELECT timeline_id FROM pg_control_checkpoint()),
       pg_is_in_recovery(),
       (SELECT rolsuper FROM pg_roles WHERE rolname = current_user),
       current_user::text,
       (SELECT pg_get_userbyid(nspowner)::text FROM pg_namespace WHERE nspname = 'pgfs'),
       to_regclass('pgfs.meta') IS NOT NULL";

/// Read the server's version, durability settings and the `meta` rows.
pub fn server_info(conn: &mut Conn) -> Result<ServerInfo> {
    let row = conn
        .query_sql(SERVER_SQL, &[])
        .map_err(|e| anyhow!("read server settings: {e}"))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("read server settings: no row"))?;
    let has_meta: bool = row.get(13);
    let meta = if has_meta {
        let rows = conn
            .query_sql("SELECT key, value FROM pgfs.meta", &[])
            .map_err(|e| anyhow!("read pgfs.meta: {e}"))?;
        Some(
            rows.iter()
                .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
                .collect(),
        )
    } else {
        None
    };
    Ok(ServerInfo {
        version_num: row.get(0),
        fsync: row.get(1),
        full_page_writes: row.get(2),
        data_checksums: row.get(3),
        wal_level: row.get(4),
        wal_writer_delay_ms: row.get::<_, Option<i64>>(5).unwrap_or(200) as u64,
        synchronous_standby_names: row.get(6),
        system_identifier: row.get(7),
        timeline: row.get(8),
        in_recovery: row.get(9),
        is_superuser: row.get::<_, Option<bool>>(10).unwrap_or(false),
        current_user: row.get(11),
        schema_owner: row.get(12),
        meta,
    })
}

/// Refuse servers older than PostgreSQL 17; warn about untested majors.
pub fn check_version(info: &ServerInfo) -> Result<()> {
    if info.version_num < MIN_SERVER_VERSION {
        bail!(
            "PostgreSQL {} is not supported; pgfs needs 17 or later",
            info.version_num / 10000
        );
    }
    if info.version_num / 10000 > MAX_TESTED_MAJOR {
        tracing::warn!(
            "PostgreSQL {} has not been tested with pgfs (tested: 17 to {MAX_TESTED_MAJOR})",
            info.version_num / 10000
        );
    }
    Ok(())
}

/// Refuse a server on which durability is impossible
/// (spec/durability.py ChecksServerDurability).
pub fn check_durability(info: &ServerInfo, accept_data_loss: bool, replica: bool) -> Result<()> {
    let mut problems = Vec::new();
    if !info.fsync {
        problems.push("fsync is off");
    }
    if !info.full_page_writes {
        problems.push("full_page_writes is off");
    }
    if replica && info.wal_level == "minimal" {
        problems.push("wal_level is minimal, so --replica cannot work");
    }
    if !info.data_checksums {
        tracing::warn!("data_checksums is off: Postgres cannot detect torn or corrupted pages");
    }
    if problems.is_empty() {
        return Ok(());
    }
    if accept_data_loss {
        tracing::warn!(
            "--i-accept-data-loss: running although {}",
            problems.join(", ")
        );
        return Ok(());
    }
    bail!(
        "the server cannot keep data durable: {}; fix the server or pass --i-accept-data-loss",
        problems.join(", ")
    )
}

/// Refuse an unformatted database or an unknown schema version
/// (spec/storage.py SchemaVersion, spec/ops.py RefusesUnformattedDatabase).
pub fn check_schema(info: &ServerInfo) -> Result<()> {
    let Some(v) = info.schema_version() else {
        bail!("the database has no pgfs filesystem; run `pgfs mkfs` first");
    };
    if v != SCHEMA_VERSION {
        if v < SCHEMA_VERSION {
            bail!("schema version {v} is older than this binary's {SCHEMA_VERSION}; run `pgfs migrate`");
        }
        bail!("schema version {v} is newer than this binary supports ({SCHEMA_VERSION}); upgrade pgfs");
    }
    Ok(())
}

/// Options of `pgfs mkfs`.
#[derive(Debug, Clone)]
pub struct MkfsOptions {
    pub block_size: u64,
    pub inline_max: u64,
    pub capacity: Option<u64>,
    pub scratch: bool,
    pub runtime_role: Option<String>,
    pub force: bool,
    pub root_uid: u32,
    pub root_gid: u32,
}

impl Default for MkfsOptions {
    fn default() -> Self {
        MkfsOptions {
            block_size: 65536,
            inline_max: 2048,
            capacity: None,
            scratch: false,
            runtime_role: None,
            force: false,
            // SAFETY: getuid/getgid cannot fail.
            root_uid: unsafe { libc::getuid() },
            root_gid: unsafe { libc::getgid() },
        }
    }
}

fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The SQL text that creates the schema: every migration, with UNLOGGED
/// substituted for a scratch filesystem.
pub fn schema_sql(scratch: bool) -> String {
    let mut out = String::new();
    for m in MIGRATIONS {
        for part in m.iter() {
            out.push_str(&part.replace("@UNLOGGED@", if scratch { "UNLOGGED" } else { "" }));
            out.push('\n');
        }
    }
    out
}

/// Statements that harden the functions (SECURITY DEFINER with a fixed
/// search_path for the public ones) and grant the runtime role.
pub fn privileges_sql(runtime_role: Option<&str>) -> String {
    let mut s = String::from(
        "DO $$ DECLARE f record; BEGIN
           FOR f IN SELECT p.oid::regprocedure AS sig FROM pg_proc p
                    JOIN pg_namespace n ON n.oid = p.pronamespace
                    WHERE n.nspname = 'pgfs' AND p.prokind = 'f' AND p.proname NOT LIKE '\\_%'
                      AND p.prorettype <> 'trigger'::regtype LOOP
             EXECUTE format('ALTER FUNCTION %s SECURITY DEFINER SET search_path = pgfs, pg_catalog, pg_temp', f.sig);
           END LOOP;
         END $$;
         REVOKE ALL ON SCHEMA pgfs FROM PUBLIC;
         REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA pgfs FROM PUBLIC;\n",
    );
    if let Some(role) = runtime_role {
        s.push_str(&grant_sql(role));
    }
    s
}

/// Grant the runtime privileges to a role: execute the public functions
/// and read the tables and views, nothing else.
pub fn grant_sql(role: &str) -> String {
    let r = ident(role);
    format!(
        "GRANT USAGE ON SCHEMA pgfs TO {r};
         GRANT SELECT ON ALL TABLES IN SCHEMA pgfs TO {r};
         DO $$ DECLARE f record; BEGIN
           FOR f IN SELECT p.oid::regprocedure AS sig FROM pg_proc p
                    JOIN pg_namespace n ON n.oid = p.pronamespace
                    WHERE n.nspname = 'pgfs' AND p.prokind = 'f' AND p.proname NOT LIKE '\\_%'
                      AND p.prorettype <> 'trigger'::regtype LOOP
             EXECUTE format('GRANT EXECUTE ON FUNCTION %s TO %s', f.sig, {lit});
           END LOOP;
         END $$;\n",
        lit = literal(&ident(role)),
    )
}

/// Format a database in one transaction (spec/ops.py MakesFilesystem).
pub fn mkfs(conn: &mut Conn, opts: &MkfsOptions) -> Result<String> {
    if !opts.block_size.is_power_of_two() || !(4096..=1_048_576).contains(&opts.block_size) {
        bail!("--block-size must be a power of two from 4096 to 1048576");
    }
    if opts.inline_max > 2048 || opts.inline_max > opts.block_size {
        bail!("--inline-max must be at most 2048 bytes");
    }
    let info = server_info(conn)?;
    check_version(&info)?;
    check_durability(&info, false, false).or_else(|e| {
        if opts.scratch {
            tracing::warn!("{e:#}");
            Ok(())
        } else {
            Err(e)
        }
    })?;
    let exists = conn
        .query_sql("SELECT to_regnamespace('pgfs') IS NOT NULL", &[])
        .map_err(|e| anyhow!("check for a pgfs schema: {e}"))?[0]
        .get::<_, bool>(0);
    if exists && !opts.force {
        bail!("the database already has a pgfs schema; pass --force to replace it (this destroys the filesystem)");
    }
    let fs_uuid = uuid::Uuid::new_v4().to_string();
    let now_ns = crate::fs::now_ns();
    let mut sql = String::from("BEGIN;\n");
    if exists {
        sql.push_str("DROP SCHEMA pgfs CASCADE;\n");
    }
    sql.push_str("CREATE SCHEMA pgfs;\n");
    sql.push_str(&schema_sql(opts.scratch));
    let capacity = opts.capacity.map(|c| c.to_string());
    let mut meta = vec![
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("fs_uuid", fs_uuid.clone()),
        ("block_size", opts.block_size.to_string()),
        ("inline_max", opts.inline_max.to_string()),
        ("scratch", opts.scratch.to_string()),
        ("created_at", "now".to_string()),
        ("root_uid", opts.root_uid.to_string()),
        ("root_gid", opts.root_gid.to_string()),
        ("root_mode", "493".to_string()),
    ];
    if let Some(c) = capacity {
        meta.push(("capacity_bytes", c));
    }
    for (k, v) in &meta {
        if *k == "created_at" {
            sql.push_str("INSERT INTO pgfs.meta VALUES ('created_at', to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'));\n");
        } else {
            sql.push_str(&format!(
                "INSERT INTO pgfs.meta VALUES ({}, {});\n",
                literal(k),
                literal(v)
            ));
        }
    }
    sql.push_str(&format!(
        "INSERT INTO pgfs.layers (layer) VALUES (nextval('pgfs.layer_seq'));
         INSERT INTO pgfs.trees (name, layer) VALUES ('main', currval('pgfs.layer_seq'));
         INSERT INTO pgfs.inodes (layer, ino, gen, born, kind, mode, uid, gid, nlink, atime_ns, mtime_ns, ctime_ns, btime_ns)
           VALUES (currval('pgfs.layer_seq'), 1, pgfs._new_gen(), 1, 'd', 493, {uid}, {gid}, 2, {now_ns}, {now_ns}, {now_ns}, {now_ns});
         INSERT INTO pgfs.scrub_state (id) VALUES (1);
         INSERT INTO pgfs.usage_totals (layer) VALUES (0);\n",
        uid = opts.root_uid,
        gid = opts.root_gid,
    ));
    sql.push_str(&privileges_sql(opts.runtime_role.as_deref()));
    sql.push_str("COMMIT;\n");
    if let Err(e) = conn.batch(&sql) {
        let _ = conn.batch("ROLLBACK");
        return Err(anyhow!("format the database: {e}"));
    }
    Ok(fs_uuid)
}

/// Refuse when any unexpired mount session exists.
fn check_no_sessions(conn: &mut Conn, what: &str) -> Result<()> {
    let rows = conn
        .query_sql(
            "SELECT tree || COALESCE('@' || snapshot, '') || ' by ' || host || ' pid ' || pid
               || CASE WHEN read_only THEN ' (read-only)' ELSE '' END
             FROM pgfs.mounts WHERE lease_until > now()",
            &[],
        )
        .map_err(|e| anyhow!("list mount sessions: {e}"))?;
    if !rows.is_empty() {
        let who: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
        bail!(
            "{what} refused: the filesystem is mounted: {}",
            who.join(", ")
        );
    }
    Ok(())
}

/// Upgrade the schema one numbered migration at a time
/// (spec/ops.py MigratesSchema). Returns the steps (run or, with
/// `dry_run`, planned).
pub fn migrate(conn: &mut Conn, dry_run: bool) -> Result<Vec<String>> {
    let info = server_info(conn)?;
    let Some(v) = info.schema_version() else {
        bail!("the database has no pgfs filesystem; run `pgfs mkfs` first");
    };
    if v > SCHEMA_VERSION {
        bail!("schema version {v} is newer than this binary supports ({SCHEMA_VERSION})");
    }
    let mut steps = Vec::new();
    if v == SCHEMA_VERSION {
        return Ok(steps);
    }
    if !dry_run {
        check_no_sessions(conn, "migrate")?;
    }
    for n in (v + 1)..=SCHEMA_VERSION {
        steps.push(format!("migration {n}: schema version {} -> {n}", n - 1));
        if dry_run {
            continue;
        }
        let mut sql = String::from("BEGIN;\n");
        for part in MIGRATIONS[(n - 1) as usize] {
            sql.push_str(&part.replace("@UNLOGGED@", if info.scratch() { "UNLOGGED" } else { "" }));
        }
        sql.push_str(&format!(
            "UPDATE pgfs.meta SET value = '{n}' WHERE key = 'schema_version';\nCOMMIT;\n"
        ));
        conn.batch(&sql)
            .with_context(|| format!("run migration {n}"))?;
    }
    Ok(steps)
}

/// Options of `pgfs tune` (spec/ops.py TunesFilesystem).
#[derive(Debug, Clone, Default)]
pub struct TuneOptions {
    /// Some(Some(n)) sets capacity, Some(None) removes it.
    pub capacity: Option<Option<u64>>,
    pub new_uuid: bool,
    pub runtime_role: Option<String>,
}

pub fn tune(conn: &mut Conn, opts: &TuneOptions) -> Result<Vec<String>> {
    let info = server_info(conn)?;
    check_schema(&info)?;
    let mut done = Vec::new();
    if let Some(cap) = opts.capacity {
        match cap {
            Some(c) => {
                conn.query_sql(
                    "INSERT INTO pgfs.meta VALUES ('capacity_bytes', $1)
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                    &[&c.to_string()],
                )
                .map_err(|e| anyhow!("set capacity: {e}"))?;
                done.push(format!("capacity set to {c} bytes"));
            }
            None => {
                conn.query_sql("DELETE FROM pgfs.meta WHERE key = 'capacity_bytes'", &[])
                    .map_err(|e| anyhow!("remove capacity: {e}"))?;
                done.push("capacity removed".to_string());
            }
        }
    }
    if opts.new_uuid {
        check_no_sessions(conn, "tune --new-uuid")?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.query_sql(
            "UPDATE pgfs.meta SET value = $1 WHERE key = 'fs_uuid'",
            &[&id],
        )
        .map_err(|e| anyhow!("set fs_uuid: {e}"))?;
        done.push(format!("fs_uuid set to {id}"));
    }
    if let Some(role) = &opts.runtime_role {
        conn.batch(&grant_sql(role))
            .map_err(|e| anyhow!("grant runtime role {role}: {e}"))?;
        done.push(format!("runtime privileges granted to {role}"));
    }
    Ok(done)
}

/// Map a DbError from a CLI command to an error with its errno named.
pub fn cli_err(what: &str, e: DbError) -> anyhow::Error {
    match e.class() {
        crate::errno::DbClass::Function(errno) => {
            anyhow!(
                "{what}: {} ({})",
                e.message,
                crate::errno::errno_name(errno)
            )
        }
        _ => anyhow!("{what}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_schema_is_unlogged_for_data_tables_only() {
        let s = schema_sql(true);
        for t in SCRATCH_UNLOGGED {
            assert!(
                s.contains(&format!("CREATE UNLOGGED TABLE pgfs.{t} ")),
                "{t} should be unlogged"
            );
        }
        for t in ["meta", "trees", "layers", "snapshots", "mounts"] {
            assert!(
                !s.contains(&format!("UNLOGGED TABLE pgfs.{t} ")),
                "{t} must stay logged"
            );
        }
        assert!(!schema_sql(false).contains("UNLOGGED TABLE"));
        assert!(!s.contains("@UNLOGGED@"));
    }

    #[test]
    fn every_table_is_created() {
        let s = schema_sql(false);
        for t in TABLES {
            assert!(s.contains(&format!("TABLE pgfs.{t} (")), "missing {t}");
        }
    }

    #[test]
    fn durability_refusals() {
        let mut info = ServerInfo {
            version_num: 170000,
            fsync: true,
            full_page_writes: true,
            data_checksums: true,
            wal_level: "replica".into(),
            wal_writer_delay_ms: 200,
            synchronous_standby_names: String::new(),
            system_identifier: 1,
            timeline: 1,
            in_recovery: false,
            is_superuser: false,
            current_user: "u".into(),
            meta: None,
            schema_owner: None,
        };
        assert!(check_durability(&info, false, true).is_ok());
        assert_eq!(info.async_window_ms(), 600);
        info.fsync = false;
        assert!(check_durability(&info, false, false).is_err());
        assert!(check_durability(&info, true, false).is_ok());
        info.fsync = true;
        info.wal_level = "minimal".into();
        assert!(check_durability(&info, false, true).is_err());
        assert!(check_durability(&info, false, false).is_ok());
        info.version_num = 160004;
        assert!(check_version(&info).is_err());
        assert!(check_schema(&info).is_err());
    }
}

//! `pgfs check` and `pgfs scrub` (spec/integrity.py PgfsCheck, Scrub).

use crate::conn::Conn;
use crate::metrics;
use crate::schema::cli_err;
use anyhow::{anyhow, Result};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// A violation found by `pgfs check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub class: i32,
    pub detail: String,
}

/// Run the checks in one REPEATABLE READ, read-only transaction, so they
/// see one consistent state while mounts keep working; then amcheck.
pub fn violations(c: &mut Conn, tree: Option<&str>) -> Result<Vec<Violation>> {
    c.batch("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .map_err(|e| anyhow!("begin check: {e}"))?;
    let rows = c.query_sql("SELECT class, detail FROM pgfs.check($1)", &[&tree]);
    let _ = c.batch("COMMIT");
    let rows = rows.map_err(|e| cli_err("check", e))?;
    let mut out: Vec<Violation> = rows
        .iter()
        .map(|r| Violation {
            class: r.get(0),
            detail: r.get(1),
        })
        .collect();
    // amcheck on every pgfs index.
    match c.query_sql(
        "SELECT c.relname::text FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_am a ON a.oid = c.relam
         WHERE n.nspname = 'pgfs' AND a.amname = 'btree'",
        &[],
    ) {
        Ok(idx) => {
            let have: bool = c
                .query_sql(
                    "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'amcheck')",
                    &[],
                )
                .map(|r| r[0].get(0))
                .unwrap_or(false);
            if !have && c.batch("CREATE EXTENSION IF NOT EXISTS amcheck").is_err() {
                tracing::warn!(
                    "amcheck is not installed and could not be created; index checks skipped"
                );
            } else {
                for r in idx {
                    let name: String = r.get(0);
                    if let Err(e) = c.query_sql(
                        "SELECT bt_index_check(('pgfs.' || quote_ident($1))::regclass)",
                        &[&name],
                    ) {
                        if e.sqlstate.as_deref() == Some("42501") {
                            tracing::warn!("amcheck needs more privileges; index checks skipped");
                            break;
                        }
                        out.push(Violation {
                            class: 11,
                            detail: format!("index {name}: {e}"),
                        });
                    }
                }
            }
        }
        Err(e) => tracing::warn!("list indexes: {e}"),
    }
    Ok(out)
}

/// Compute and store a snapshot's digest.
pub fn seal(c: &mut Conn, tree: &str, name: &str) -> Result<String> {
    let rows = c
        .query_sql("SELECT pgfs.snapshot_seal($1, $2)", &[&tree, &name])
        .map_err(|e| cli_err("seal snapshot", e))?;
    Ok(rows[0].get(0))
}

pub fn run(c: &mut Conn, tree: Option<&str>, repair: bool, json: bool) -> Result<i32> {
    let t0 = Instant::now();
    if repair {
        let n: i32 = c
            .query_sql("SELECT pgfs.check_repair()", &[])
            .map_err(|e| cli_err("repair", e))?[0]
            .get(0);
        println!("repair: queued {n} unreferenced blob(s) and recomputed the usage totals");
    }
    let v = violations(c, tree)?;
    // Seal snapshots that have no digest yet.
    let unsealed = c
        .query_sql(
            "SELECT tree, name FROM pgfs.snapshots WHERE digest IS NULL AND ($1::text IS NULL OR tree = $1)",
            &[&tree],
        )
        .map_err(|e| cli_err("list snapshots", e))?;
    for r in unsealed {
        let (t, n): (String, String) = (r.get(0), r.get(1));
        match seal(c, &t, &n) {
            Ok(d) => tracing::info!("sealed {t}@{n}: {d}"),
            Err(e) => tracing::warn!("could not seal {t}@{n}: {e:#}"),
        }
    }
    for x in &v {
        if json {
            println!(
                "{}",
                serde_json::json!({"class": x.class, "detail": x.detail})
            );
        } else {
            println!("class {}: {}", x.class, x.detail);
        }
    }
    if !json {
        println!("check: {} violation(s) in {:.1?}", v.len(), t0.elapsed());
    }
    Ok(if v.is_empty() { 0 } else { 1 })
}

/// Read every blob and verify its checksum at a limited rate, resuming
/// from the stored position (spec/integrity.py Scrub).
pub fn scrub(c: &mut Conn, rate_mib: u64) -> Result<i32> {
    let mut pos: i64 = c
        .query_sql("SELECT pgfs.scrub_position()", &[])
        .map_err(|e| cli_err("scrub position", e))?[0]
        .get::<_, Option<i64>>(0)
        .unwrap_or(0);
    if pos > 0 {
        println!("scrub: resuming after blob {pos}");
    }
    let rate = (rate_mib.max(1) << 20) as f64;
    let t0 = Instant::now();
    let mut bytes = 0u64;
    let mut blobs = 0u64;
    let mut failures = 0u64;
    loop {
        let rows = c
            .query_sql(
                "SELECT blob_id, crc32c, data FROM pgfs.scrub_batch($1, 256)",
                &[&pos, &256i32],
            )
            .map_err(|e| cli_err("scrub batch", e))?;
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            let id: i64 = r.get(0);
            let stored: i32 = r.get(1);
            let data: Vec<u8> = r.get(2);
            let actual = crate::pg::crc(&data);
            if actual != stored {
                failures += 1;
                metrics::CHECKSUM_FAILURES.fetch_add(1, Ordering::Relaxed);
                let users = c
                    .query_sql(
                        "SELECT layer, ino, block_no, path FROM pgfs.blob_users($1)",
                        &[&id],
                    )
                    .map(|u| {
                        u.iter()
                            .map(|x| {
                                let p: Option<Vec<u8>> = x.get(3);
                                format!(
                                    "layer {} inode {} block {} ({})",
                                    x.get::<_, i64>(0),
                                    x.get::<_, i64>(1),
                                    x.get::<_, i64>(2),
                                    p.map(|p| crate::names::display_bytes(&p))
                                        .unwrap_or_else(|| "?".into())
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default();
                tracing::error!(
                    "checksum mismatch: blob {id}: expected {:08x}, found {:08x}; used by {users}",
                    stored as u32,
                    actual as u32
                );
                println!(
                    "blob {id}: checksum mismatch (expected {:08x}, found {:08x}): {users}",
                    stored as u32, actual as u32
                );
            }
            bytes += data.len() as u64;
            blobs += 1;
            pos = id;
        }
        c.query_sql("SELECT pgfs.scrub_progress($1, false)", &[&pos])
            .map_err(|e| cli_err("scrub progress", e))?;
        // Rate limit.
        let due = Duration::from_secs_f64(bytes as f64 / rate);
        let el = t0.elapsed();
        if due > el {
            std::thread::sleep(due - el);
        }
    }
    c.query_sql("SELECT pgfs.scrub_progress(0, true)", &[])
        .map_err(|e| cli_err("scrub progress", e))?;
    println!(
        "scrub: {blobs} blob(s), {bytes} byte(s), {failures} checksum failure(s) in {:.1?}",
        t0.elapsed()
    );
    Ok(if failures == 0 { 0 } else { 1 })
}

//! `pgfs changes` (spec/changefeed.py ChangesCommand, ResumableCursors).

use crate::cli::{ChangesArgs, Global};
use crate::conn::Conn;
use crate::names::{display_bytes, ViewSpec};
use crate::schema::cli_err;
use anyhow::{anyhow, bail, Result};
use std::time::Duration;
use tokio_postgres::Row;

fn row_json(r: &Row, watermark: &serde_json::Value) -> serde_json::Value {
    let b = |i: usize| r.get::<_, Option<Vec<u8>>>(i).map(|v| display_bytes(&v));
    serde_json::json!({
        "seq": r.get::<_, Option<i64>>(0),
        "xid": r.get::<_, Option<String>>(1),
        "at": r.get::<_, Option<String>>(2),
        "op": r.get::<_, Option<String>>(3),
        "ino": r.get::<_, Option<i64>>(4),
        "kind": r.get::<_, Option<i8>>(5).map(|k| ((k as u8) as char).to_string()),
        "parent_ino": r.get::<_, Option<i64>>(6),
        "name": b(7),
        "new_parent_ino": r.get::<_, Option<i64>>(8),
        "new_name": b(9),
        "path": b(10),
        "new_path": b(11),
        "watermark": watermark.to_string(),
    })
}

const COLS: &str = "seq, xid::text, at::text, op, ino, kind, parent_ino, name, new_parent_ino, new_name, path, new_path, watermark";

pub fn run(_g: &Global, mut c: Conn, a: ChangesArgs) -> Result<i32> {
    match a.args.first().map(String::as_str) {
        Some("register") => {
            let (name, tree) = match (a.args.get(1), a.args.get(2)) {
                (Some(n), Some(t)) => (n, t),
                _ => bail!("changes register [--at-snapshot TREE@NAME] NAME TREE"),
            };
            let (at_tree, at_snap) = match &a.at_snapshot {
                Some(s) => {
                    let v = ViewSpec::parse(s).map_err(|e| anyhow!(e))?;
                    (Some(v.tree), v.snapshot)
                }
                None => (None, None),
            };
            let rows = c
                .query_sql(
                    "SELECT pgfs.changes_register($1, $2, $3, $4)",
                    &[name, tree, &at_tree, &at_snap],
                )
                .map_err(|e| cli_err("register consumer", e))?;
            let tok: serde_json::Value = rows[0].get(0);
            println!(
                "{}",
                serde_json::json!({"consumer": name, "tree": tree, "watermark": tok.to_string()})
            );
            Ok(0)
        }
        Some("unregister") => {
            let name = a
                .args
                .get(1)
                .ok_or_else(|| anyhow!("changes unregister NAME"))?;
            let rows = c
                .query_sql("SELECT pgfs.changes_unregister($1)", &[name])
                .map_err(|e| cli_err("unregister consumer", e))?;
            if !rows[0].get::<_, bool>(0) {
                bail!("consumer {name} does not exist (ENOENT)");
            }
            Ok(0)
        }
        Some(tree) => {
            let tree = tree.to_string();
            let mut token: Option<serde_json::Value> = match &a.since {
                Some(s) => Some(
                    serde_json::from_str(s)
                        .map_err(|e| anyhow!("--since is not a watermark token: {e}"))?,
                ),
                None => None,
            };
            if token.is_none() && a.consumer.is_none() {
                let rows = c
                    .query_sql(
                        "SELECT pgfs.changes_token(t.layer, COALESCE((SELECT below_xid FROM pgfs.change_prune p
                           WHERE p.layer = t.layer), '0'::xid8)) FROM pgfs.trees t WHERE t.name = $1",
                        &[&tree],
                    )
                    .map_err(|e| cli_err("start token", e))?;
                let r = rows
                    .first()
                    .ok_or_else(|| anyhow!("tree {tree} does not exist (ENOENT)"))?;
                // Without a token, start from the oldest retained change.
                token = Some(r.get(0));
            }
            if a.follow {
                let _ = c.batch("LISTEN pgfs_changes");
            }
            loop {
                let rows = match (&a.consumer, &token) {
                    (Some(name), _) => c.query_sql(
                        &format!("SELECT {COLS} FROM pgfs.changes_next($1, $2)"),
                        &[name, &a.limit],
                    ),
                    (None, Some(t)) => c.query_sql(
                        &format!("SELECT {COLS} FROM pgfs.changes_page($1, $2)"),
                        &[t, &a.limit],
                    ),
                    (None, None) => unreachable!(),
                }
                .map_err(|e| cli_err("read changes", e))?;
                let wm: serde_json::Value = rows
                    .last()
                    .and_then(|r| r.get::<_, Option<serde_json::Value>>(12))
                    .unwrap_or_default();
                let n = rows.len().saturating_sub(1);
                for r in &rows[..n] {
                    println!("{}", row_json(r, &wm));
                }
                if let Some(name) = &a.consumer {
                    c.query_sql("SELECT pgfs.changes_ack($1, $2)", &[name, &wm])
                        .map_err(|e| cli_err("acknowledge changes", e))?;
                }
                token = Some(wm.clone());
                if n >= a.limit as usize {
                    continue;
                }
                let exhausted_window = wm.get("low") == wm.get("high");
                if !a.follow {
                    if exhausted_window && n == 0 {
                        // One more page: a new window may hold rows.
                        let rows = match (&a.consumer, &token) {
                            (Some(name), _) => c.query_sql(
                                &format!("SELECT {COLS} FROM pgfs.changes_next($1, $2)"),
                                &[name, &a.limit],
                            ),
                            (None, Some(t)) => c.query_sql(
                                &format!("SELECT {COLS} FROM pgfs.changes_page($1, $2)"),
                                &[t, &a.limit],
                            ),
                            _ => unreachable!(),
                        }
                        .map_err(|e| cli_err("read changes", e))?;
                        let wm2: serde_json::Value = rows
                            .last()
                            .and_then(|r| r.get::<_, Option<serde_json::Value>>(12))
                            .unwrap_or_default();
                        let n2 = rows.len().saturating_sub(1);
                        for r in &rows[..n2] {
                            println!("{}", row_json(r, &wm2));
                        }
                        if let Some(name) = &a.consumer {
                            let _ = c.query_sql("SELECT pgfs.changes_ack($1, $2)", &[name, &wm2]);
                        }
                    }
                    return Ok(0);
                }
                if n == 0 {
                    let _ = c.wait_notification(Duration::from_secs(a.poll));
                }
            }
        }
        None => bail!("changes TREE | register NAME TREE | unregister NAME"),
    }
}

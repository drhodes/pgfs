//! `pgfs export` and `pgfs import` (spec/integrity.py ExportsSnapshots,
//! ImportsArchives): portable pax tar streams of a snapshot, with names as
//! bytes, modes, owners, nanosecond times, hard links, symlinks, special
//! files and xattrs.

use crate::backend::*;
use crate::conn::{Conn, ConnSettings};
use crate::errno::FsError;
use crate::health::Health;
use crate::pg::PgBackend;
use crate::schema::cli_err;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;

fn fs_err(what: &str, e: FsError) -> anyhow::Error {
    anyhow!("{what}: {e}")
}

fn ts(ns: i64) -> String {
    let s = ns.div_euclid(1_000_000_000);
    let n = ns.rem_euclid(1_000_000_000);
    format!("{s}.{n:09}")
}

fn backend(s: &ConnSettings) -> Result<PgBackend> {
    let mut b = PgBackend::new(s.clone(), Arc::new(Health::new(1)), None);
    b.open()?;
    Ok(b)
}

fn pax_header(
    kind: tar::EntryType,
    path: &[u8],
    a: &Attr,
    size: u64,
) -> (tar::Header, Vec<(String, Vec<u8>)>) {
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(kind);
    h.set_mode(a.mode & 0o7777);
    h.set_uid(a.uid as u64);
    h.set_gid(a.gid as u64);
    h.set_size(size);
    h.set_mtime(a.mtime_ns.max(0) as u64 / 1_000_000_000);
    let short = &path[..path.len().min(99)];
    if let Some(old) = h.as_old_mut().name.get_mut(..short.len()) {
        old.copy_from_slice(short);
    }
    let pax = vec![
        ("path".to_string(), path.to_vec()),
        ("mtime".to_string(), ts(a.mtime_ns).into_bytes()),
        ("atime".to_string(), ts(a.atime_ns).into_bytes()),
        ("ctime".to_string(), ts(a.ctime_ns).into_bytes()),
        ("uid".to_string(), a.uid.to_string().into_bytes()),
        ("gid".to_string(), a.gid.to_string().into_bytes()),
    ];
    (h, pax)
}

/// Write a snapshot as a pax tar stream.
pub fn export(
    s: &ConnSettings,
    tree: &str,
    snapshot: &str,
    path: Option<&str>,
    out: impl Write,
) -> Result<()> {
    let mut c = Conn::open(s)?;
    let rows = c
        .query_sql(
            "SELECT layer, epoch, root_ino FROM pgfs.snapshots WHERE tree = $1 AND name = $2",
            &[&tree, &snapshot],
        )
        .map_err(|e| cli_err("find snapshot", e))?;
    let r = rows
        .first()
        .ok_or_else(|| anyhow!("snapshot {tree}@{snapshot} does not exist (ENOENT)"))?;
    let (layer, epoch, mut root): (i64, i64, i64) = (r.get(0), r.get(1), r.get(2));
    if let Some(p) = path {
        root = c
            .query_sql(
                "SELECT pgfs.resolve_dir($1, $2, $3, $4)",
                &[&layer, &Some(epoch), &root, &p.as_bytes()],
            )
            .map_err(|e| cli_err(&format!("resolve {p}"), e))?[0]
            .get(0);
    }
    let view = View {
        layer,
        epoch: Some(epoch),
        root: root as u64,
    };
    let bs: i64 = c
        .query_sql(
            "SELECT value::bigint FROM pgfs.meta WHERE key = 'block_size'",
            &[],
        )
        .map_err(|e| cli_err("block size", e))?[0]
        .get(0);
    let mut be = backend(s)?;
    let mut tar = tar::Builder::new(out);
    let mut first_path: HashMap<u64, Vec<u8>> = HashMap::new();
    let mut queue: Vec<(u64, Vec<u8>)> = vec![(root as u64, b".".to_vec())];
    while let Some((dir, dpath)) = queue.pop() {
        let mut after: Option<Vec<u8>> = None;
        loop {
            let page = be
                .readdirplus(&view, dir, after.as_deref(), 512)
                .map_err(|e| fs_err("list", e))?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|(n, _)| n.clone());
            for (name, a) in page {
                let mut p = dpath.clone();
                p.push(b'/');
                p.extend_from_slice(&name);
                let mut extra: Vec<(String, Vec<u8>)> = Vec::new();
                for x in be
                    .listxattr(&view, a.ino)
                    .map_err(|e| fs_err("listxattr", e))?
                {
                    if let Some(v) = be
                        .getxattr(&view, a.ino, &x)
                        .map_err(|e| fs_err("getxattr", e))?
                    {
                        extra.push((format!("SCHILY.xattr.{}", String::from_utf8_lossy(&x)), v));
                    }
                }
                if a.kind != Kind::Dir && a.nlink > 1 {
                    if let Some(target) = first_path.get(&a.ino) {
                        let (mut h, mut pax) = pax_header(tar::EntryType::Link, &p, &a, 0);
                        pax.push(("linkpath".into(), target.clone()));
                        write_entry(&mut tar, &mut h, pax, &[])?;
                        continue;
                    }
                    first_path.insert(a.ino, p.clone());
                }
                match a.kind {
                    Kind::Dir => {
                        let (mut h, mut pax) = pax_header(tar::EntryType::Directory, &p, &a, 0);
                        pax.extend(extra);
                        write_entry(&mut tar, &mut h, pax, &[])?;
                        queue.push((a.ino, p));
                    }
                    Kind::Symlink => {
                        let t = be
                            .readlink(&view, a.ino)
                            .map_err(|e| fs_err("readlink", e))?;
                        let (mut h, mut pax) = pax_header(tar::EntryType::Symlink, &p, &a, 0);
                        pax.push(("linkpath".into(), t));
                        pax.extend(extra);
                        write_entry(&mut tar, &mut h, pax, &[])?;
                    }
                    Kind::File => {
                        let mut data = vec![0u8; a.size as usize];
                        if a.size > 0 {
                            let last = (a.size - 1) / bs as u64;
                            let mut first = 0u64;
                            while first <= last {
                                let to = (first + 255).min(last);
                                let r = be
                                    .read_blocks(&view, a.ino, Some(a.gen), first, to)
                                    .map_err(|e| fs_err("read", e))?;
                                for (b, d) in r.blocks {
                                    let off = (b * bs as u64) as usize;
                                    let n = d.len().min(data.len().saturating_sub(off));
                                    data[off..off + n].copy_from_slice(&d[..n]);
                                }
                                first = to + 1;
                            }
                        }
                        let (mut h, mut pax) = pax_header(tar::EntryType::Regular, &p, &a, a.size);
                        pax.extend(extra);
                        write_entry(&mut tar, &mut h, pax, &data)?;
                    }
                    k => {
                        let et = match k {
                            Kind::Fifo => tar::EntryType::Fifo,
                            Kind::CharDev => tar::EntryType::Char,
                            Kind::BlockDev => tar::EntryType::Block,
                            _ => continue, // sockets have no tar form
                        };
                        let (mut h, mut pax) = pax_header(et, &p, &a, 0);
                        h.set_device_major(libc::major(a.rdev as libc::dev_t))?;
                        h.set_device_minor(libc::minor(a.rdev as libc::dev_t))?;
                        pax.extend(extra);
                        write_entry(&mut tar, &mut h, pax, &[])?;
                    }
                }
            }
        }
    }
    tar.finish()?;
    Ok(())
}

fn write_entry<W: Write>(
    tar: &mut tar::Builder<W>,
    h: &mut tar::Header,
    pax: Vec<(String, Vec<u8>)>,
    data: &[u8],
) -> Result<()> {
    tar.append_pax_extensions(pax.iter().map(|(k, v)| (k.as_str(), v.as_slice())))?;
    h.set_cksum();
    tar.append(h, data)?;
    Ok(())
}

fn now() -> i64 {
    crate::fs::now_ns()
}

fn parse_ts(v: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(v).ok()?;
    let (sec, frac) = s.split_once('.').unwrap_or((s, "0"));
    let sec: i64 = sec.parse().ok()?;
    let mut f = frac.to_string();
    f.truncate(9);
    while f.len() < 9 {
        f.push('0');
    }
    Some(sec * 1_000_000_000 + f.parse::<i64>().ok()?)
}

/// Read a tar stream into TREE[:/DIR] through a hidden staging directory
/// that is renamed into place at the end. Returns the number of entries.
pub fn import(s: &ConnSettings, tree: &str, dir: Option<&str>, input: impl Read) -> Result<u64> {
    let mut c = Conn::open(s)?;
    let rows = c
        .query_sql(
            "SELECT t.layer, t.root_ino, (SELECT value::bigint FROM pgfs.meta WHERE key = 'block_size'),
                    (SELECT value::bigint FROM pgfs.meta WHERE key = 'inline_max')
             FROM pgfs.trees t WHERE t.name = $1",
            &[&tree],
        )
        .map_err(|e| cli_err("find tree", e))?;
    let r = rows
        .first()
        .ok_or_else(|| anyhow!("tree {tree} does not exist (ENOENT)"))?;
    let (layer, root, bs, inline_max): (i64, i64, i64, i64) =
        (r.get(0), r.get(1), r.get(2), r.get(3));
    let (bs, inline_max) = (bs as u64, inline_max as u64);
    let view = View {
        layer,
        epoch: None,
        root: root as u64,
    };
    let mut be = backend(s)?;
    // SAFETY: getuid/getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };

    // Where the result goes.
    let (target_parent, target_name) = match dir
        .map(|d| d.trim_end_matches('/'))
        .filter(|d| !d.is_empty())
    {
        Some(d) => {
            let (parent, name) = d.rsplit_once('/').unwrap_or(("", d));
            let p = c
                .query_sql(
                    "SELECT pgfs.resolve_dir($1, NULL, $2, $3)",
                    &[&layer, &root, &parent.as_bytes()],
                )
                .map_err(|e| cli_err(&format!("resolve {parent}"), e))?[0]
                .get::<_, i64>(0) as u64;
            if be
                .lookup(&view, p, name.as_bytes())
                .map_err(|e| fs_err("lookup", e))?
                .is_some()
            {
                bail!("{d} exists (EEXIST); import into a new directory");
            }
            (p, Some(name.as_bytes().to_vec()))
        }
        None => (root as u64, None),
    };
    let staging = format!(".pgfs-import-{}", uuid::Uuid::new_v4());
    let mk = |be: &mut PgBackend,
              parent: u64,
              name: &[u8],
              kind: Kind,
              mode: u32,
              rdev: u64,
              target: Option<Vec<u8>>| {
        be.make(
            layer,
            &MakeArgs {
                parent,
                name: name.to_vec(),
                kind,
                mode,
                uid,
                gid,
                rdev,
                target,
                now: now(),
            },
        )
    };
    let stage = mk(
        &mut be,
        target_parent,
        staging.as_bytes(),
        Kind::Dir,
        0o700,
        0,
        None,
    )
    .map_err(|e| fs_err("create staging directory", e))?;
    let mut dirs: HashMap<Vec<u8>, u64> = HashMap::new();
    dirs.insert(Vec::new(), stage.ino);
    let mut by_path: HashMap<Vec<u8>, u64> = HashMap::new();
    let mut count = 0u64;
    let mut archive = tar::Archive::new(input);
    let mut dir_times: Vec<(u64, i64, i64, u32, u32, u32)> = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let mut pax: HashMap<String, Vec<u8>> = HashMap::new();
        if let Ok(Some(exts)) = entry.pax_extensions() {
            for e in exts.flatten() {
                pax.insert(e.key().unwrap_or("").to_string(), e.value_bytes().to_vec());
            }
        }
        let raw = pax
            .get("path")
            .cloned()
            .unwrap_or_else(|| entry.path_bytes().into_owned());
        let comps: Vec<&[u8]> = raw
            .split(|&b| b == b'/')
            .filter(|c| !c.is_empty() && *c != b".")
            .collect();
        if comps.iter().any(|c| *c == b"..") {
            bail!(
                "refusing path with ..: {}",
                crate::names::display_bytes(&raw)
            );
        }
        let Some((&name, parents)) = comps.split_last() else {
            continue;
        };
        let parent_key: Vec<u8> = parents.join(&b'/');
        let key: Vec<u8> = comps.join(&b'/');
        // Create missing parents.
        let mut acc: Vec<u8> = Vec::new();
        let mut parent = stage.ino;
        for p in parents {
            if !acc.is_empty() {
                acc.push(b'/');
            }
            acc.extend_from_slice(p);
            parent = match dirs.get(&acc) {
                Some(&i) => i,
                None => {
                    let a = mk(&mut be, parent, p, Kind::Dir, 0o755, 0, None)
                        .map_err(|e| fs_err("mkdir", e))?;
                    dirs.insert(acc.clone(), a.ino);
                    a.ino
                }
            };
        }
        let _ = parent_key;
        let h = entry.header().clone();
        let mode = h.mode().unwrap_or(0o644) & 0o7777;
        let euid = pax
            .get("uid")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse().ok())
            .unwrap_or(h.uid().unwrap_or(uid as u64) as u32);
        let egid = pax
            .get("gid")
            .and_then(|v| std::str::from_utf8(v).ok()?.parse().ok())
            .unwrap_or(h.gid().unwrap_or(gid as u64) as u32);
        let mtime = pax
            .get("mtime")
            .and_then(|v| parse_ts(v))
            .unwrap_or(h.mtime().unwrap_or(0) as i64 * 1_000_000_000);
        let atime = pax.get("atime").and_then(|v| parse_ts(v)).unwrap_or(mtime);
        let ino = match h.entry_type() {
            tar::EntryType::Directory => {
                let a = match dirs.get(&key) {
                    Some(&i) => i,
                    None => {
                        let a = mk(&mut be, parent, name, Kind::Dir, mode, 0, None)
                            .map_err(|e| fs_err("mkdir", e))?;
                        dirs.insert(key.clone(), a.ino);
                        a.ino
                    }
                };
                dir_times.push((a, atime, mtime, mode, euid, egid));
                a
            }
            tar::EntryType::Symlink => {
                let t = pax
                    .get("linkpath")
                    .cloned()
                    .or_else(|| entry.link_name_bytes().map(|b| b.into_owned()))
                    .unwrap_or_default();
                mk(&mut be, parent, name, Kind::Symlink, 0o777, 0, Some(t))
                    .map_err(|e| fs_err("symlink", e))?
                    .ino
            }
            tar::EntryType::Link => {
                let t = pax
                    .get("linkpath")
                    .cloned()
                    .or_else(|| entry.link_name_bytes().map(|b| b.into_owned()))
                    .unwrap_or_default();
                let tk: Vec<u8> = t
                    .split(|&b| b == b'/')
                    .filter(|c| !c.is_empty() && *c != b".")
                    .collect::<Vec<_>>()
                    .join(&b'/');
                let target = *by_path.get(&tk).ok_or_else(|| {
                    anyhow!("hard link to unknown {}", crate::names::display_bytes(&t))
                })?;
                be.link(layer, target, None, parent, name, now())
                    .map_err(|e| fs_err("link", e))?;
                count += 1;
                continue;
            }
            tar::EntryType::Fifo | tar::EntryType::Char | tar::EntryType::Block => {
                let kind = match h.entry_type() {
                    tar::EntryType::Fifo => Kind::Fifo,
                    tar::EntryType::Char => Kind::CharDev,
                    _ => Kind::BlockDev,
                };
                let rdev = libc::makedev(
                    h.device_major()?.unwrap_or(0),
                    h.device_minor()?.unwrap_or(0),
                ) as u64;
                mk(&mut be, parent, name, kind, mode, rdev, None)
                    .map_err(|e| fs_err("mknod", e))?
                    .ino
            }
            _ => {
                let a = mk(&mut be, parent, name, Kind::File, mode, 0, None)
                    .map_err(|e| fs_err("create", e))?;
                let mut data = Vec::new();
                entry.read_to_end(&mut data).context("read archive entry")?;
                let size = data.len() as u64;
                if size > 0 {
                    if size <= inline_max {
                        be.write_blocks(
                            layer,
                            &WriteArgs {
                                ino: a.ino,
                                gen: Some(a.gen),
                                size,
                                inline: Some(data),
                                now: now(),
                                ..Default::default()
                            },
                        )
                        .map_err(|e| fs_err("write", e))?;
                    } else {
                        // Bounded batches of 64 blocks; all-zero blocks stay holes.
                        let chunks: Vec<(u64, Vec<u8>)> = data
                            .chunks(bs as usize)
                            .enumerate()
                            .filter(|(_, d)| d.iter().any(|&b| b != 0))
                            .map(|(i, d)| (i as u64, d.to_vec()))
                            .collect();
                        let mut written = 0u64;
                        for batch in chunks.chunks(64) {
                            let end = batch
                                .last()
                                .map(|(b, d)| b * bs + d.len() as u64)
                                .unwrap_or(0);
                            written = written.max(end);
                            be.write_blocks(
                                layer,
                                &WriteArgs {
                                    ino: a.ino,
                                    gen: Some(a.gen),
                                    size: written,
                                    blocks: batch.to_vec(),
                                    now: now(),
                                    ..Default::default()
                                },
                            )
                            .map_err(|e| fs_err("write", e))?;
                        }
                        if written < size {
                            be.setattr(
                                layer,
                                &SetAttrArgs {
                                    ino: a.ino,
                                    gen: Some(a.gen),
                                    size: Some(size),
                                    now: now(),
                                    ..Default::default()
                                },
                            )
                            .map_err(|e| fs_err("extend", e))?;
                        }
                    }
                }
                a.ino
            }
        };
        by_path.insert(key, ino);
        for (k, v) in &pax {
            if let Some(x) = k.strip_prefix("SCHILY.xattr.") {
                be.setxattr(layer, ino, x.as_bytes(), v, 0, now())
                    .map_err(|e| fs_err("setxattr", e))?;
            }
        }
        if h.entry_type() != tar::EntryType::Symlink && h.entry_type() != tar::EntryType::Directory
        {
            be.setattr(
                layer,
                &SetAttrArgs {
                    ino,
                    mode: Some(mode),
                    uid: Some(euid),
                    gid: Some(egid),
                    atime_ns: Some(atime),
                    mtime_ns: Some(mtime),
                    now: now(),
                    ..Default::default()
                },
            )
            .map_err(|e| fs_err("set attributes", e))?;
        }
        count += 1;
    }
    // Directory times last, after their contents changed them.
    for (ino, atime, mtime, mode, u, g) in dir_times.into_iter().rev() {
        be.setattr(
            layer,
            &SetAttrArgs {
                ino,
                mode: Some(mode),
                uid: Some(u),
                gid: Some(g),
                atime_ns: Some(atime),
                mtime_ns: Some(mtime),
                now: now(),
                ..Default::default()
            },
        )
        .map_err(|e| fs_err("set directory attributes", e))?;
    }
    // Make the result visible.
    let rename = |be: &mut PgBackend, from_parent: u64, from: &[u8], to_parent: u64, to: &[u8]| {
        be.rename(
            layer,
            &RenameArgs {
                parent: from_parent,
                name: from.to_vec(),
                newparent: to_parent,
                newname: to.to_vec(),
                flags: RENAME_NOREPLACE,
                open: vec![],
                dirty: vec![],
                now: now(),
            },
        )
    };
    match target_name {
        Some(name) => {
            rename(
                &mut be,
                target_parent,
                staging.as_bytes(),
                target_parent,
                &name,
            )
            .map_err(|e| fs_err("rename staging into place", e))?;
            be.setattr(
                layer,
                &SetAttrArgs {
                    ino: stage.ino,
                    mode: Some(0o755),
                    now: now(),
                    ..Default::default()
                },
            )
            .map_err(|e| fs_err("set mode", e))?;
        }
        None => {
            let top = be
                .readdir(&view, stage.ino, None, u32::MAX >> 1)
                .map_err(|e| fs_err("list staging", e))?;
            for e in top {
                rename(&mut be, stage.ino, &e.name, target_parent, &e.name)
                    .map_err(|x| fs_err("move into place", x))?;
            }
            be.rmdir(layer, target_parent, staging.as_bytes(), now())
                .map_err(|e| fs_err("remove staging", e))?;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pax_times_round_trip() {
        for ns in [0i64, 1_700_000_000_123_456_789, 5] {
            assert_eq!(parse_ts(ts(ns).as_bytes()), Some(ns));
        }
        assert_eq!(parse_ts(b"12.5"), Some(12_500_000_000));
    }
}

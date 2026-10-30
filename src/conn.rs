//! One database connection, owned by one thread for its whole life
//! (spec/concurrency.py WorkerOwnsConnection).
//!
//! Every wait is bounded (spec/failure.py BoundsEveryWait); remote hosts
//! need verified TLS (spec/security.py RequiresTlsForRemote); passwords
//! come from files or the environment, never argv, and never reach a log
//! line (spec/security.py RedactsSecrets). Notices are watched for the
//! `SyncRepWaitForLSN` warning that marks a synchronous commit whose
//! standby wait was cancelled (spec/durability.py FsyncIsDurable).

use crate::errno::{classify, DbClass};
use crate::metrics;
use crate::redact::redact;
use anyhow::{anyhow, Context};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_postgres::types::ToSql;
use tokio_postgres::{AsyncMessage, Client, Notification, Row, Statement};

/// Connection settings shared by every connection of a process.
#[derive(Clone, Debug)]
pub struct ConnSettings {
    pub conn: String,
    pub password_file: Option<PathBuf>,
    pub connect_timeout: Duration,
    pub statement_timeout: Duration,
    pub lock_timeout: Duration,
    pub idle_in_transaction_timeout: Duration,
    pub insecure_transport: bool,
    pub application_name: String,
    /// Run with `synchronous_commit = off` (every daemon connection;
    /// durable operations turn it on per transaction).
    pub async_commit: bool,
}

impl ConnSettings {
    pub fn new(conn: &str) -> Self {
        ConnSettings {
            conn: conn.to_string(),
            password_file: None,
            connect_timeout: Duration::from_secs(5),
            statement_timeout: Duration::from_secs(30),
            lock_timeout: Duration::from_secs(5),
            idle_in_transaction_timeout: Duration::from_secs(60),
            insecure_transport: false,
            application_name: "pgfs".to_string(),
            async_commit: false,
        }
    }

    pub fn redacted(&self) -> String {
        redact(&self.conn)
    }
}

/// A database error, with its SQLSTATE when the server sent one.
#[derive(Debug)]
pub struct DbError {
    pub sqlstate: Option<String>,
    pub message: String,
    pub detail: Option<String>,
    pub source: Option<tokio_postgres::Error>,
}

impl DbError {
    pub fn class(&self) -> DbClass {
        classify(self.sqlstate.as_deref())
    }

    pub fn synthetic(sqlstate: &str, message: &str) -> Self {
        DbError {
            sqlstate: Some(sqlstate.to_string()),
            message: message.to_string(),
            detail: None,
            source: None,
        }
    }

    fn from_pg(e: tokio_postgres::Error) -> Self {
        let (sqlstate, message, detail) = match e.as_db_error() {
            Some(db) => (
                Some(db.code().code().to_string()),
                db.message().to_string(),
                db.detail().map(str::to_string),
            ),
            None => (e.code().map(|c| c.code().to_string()), e.to_string(), None),
        };
        DbError {
            sqlstate,
            message,
            detail,
            source: Some(e),
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.sqlstate {
            Some(s) => write!(f, "{} (SQLSTATE {s})", self.message)?,
            None => write!(f, "{}", self.message)?,
        }
        if let Some(d) = &self.detail {
            write!(f, ": {d}")?;
        }
        if let Some(src) = &self.source {
            if src.as_db_error().is_none() {
                if let Some(inner) = std::error::Error::source(src) {
                    write!(f, ": {inner}")?;
                }
            }
        }
        Ok(())
    }
}

impl std::error::Error for DbError {}

/// How the connection reaches its server, for TLS policy and cancels.
#[derive(Clone)]
enum Tls {
    None,
    Rustls(tokio_postgres_rustls::MakeRustlsConnect),
}

/// What a cancel needs, usable from another thread
/// (spec/failure.py InFlightAgeWatchdog, HonoursInterrupt).
#[derive(Clone)]
pub struct Canceller {
    token: tokio_postgres::CancelToken,
    tls: Tls,
    sent: Arc<AtomicBool>,
}

impl Canceller {
    /// Cancel whatever query is running; records that a cancel was sent so
    /// a synchronous commit that still succeeds is treated as unreplicated.
    pub fn cancel(&self) -> anyhow::Result<()> {
        self.sent.store(true, Ordering::Release);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let r = match &self.tls {
            Tls::None => rt.block_on(self.token.cancel_query(tokio_postgres::NoTls)),
            Tls::Rustls(t) => rt.block_on(self.token.cancel_query(t.clone())),
        };
        r.map_err(|e| anyhow!("cancel query: {e}"))
    }
}

/// One open connection with its prepared statements.
pub struct Conn {
    rt: tokio::runtime::Runtime,
    client: Client,
    pub backend_pid: i32,
    pub server_version_num: i32,
    sync_rep_warning: Arc<AtomicBool>,
    cancel_sent: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    notifications: tokio::sync::mpsc::UnboundedReceiver<Notification>,
    stmts: HashMap<&'static str, Statement>,
    tls: Tls,
    pub redacted: String,
}

/// Split a libpq key=value string into pairs (values unquoted).
fn kv_pairs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        if key.is_empty() {
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == '=') {
            chars.next();
        }
        let mut val = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            val.push(n);
                        }
                    }
                    '\'' => break,
                    c => val.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        val.push(n);
                    }
                } else {
                    val.push(c);
                }
            }
        }
        out.push((key, val));
    }
    out
}

fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Connection parameters that libpq knows but tokio-postgres does not.
#[derive(Default, Debug)]
pub struct Extra {
    pub sslmode: Option<String>,
    pub sslrootcert: Option<String>,
    pub passfile: Option<String>,
}

/// Strip the libpq-only parameters from a connection string.
pub fn split_extra(conn: &str) -> (String, Extra) {
    let mut extra = Extra::default();
    let t = conn.trim();
    if t.starts_with("postgres://") || t.starts_with("postgresql://") {
        let (base, query) = match t.split_once('?') {
            Some((b, q)) => (b, q),
            None => (t, ""),
        };
        let mut kept = Vec::new();
        for kv in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match k {
                "sslmode" if v.starts_with("verify") => extra.sslmode = Some(v.to_string()),
                "sslmode" => {
                    extra.sslmode = Some(v.to_string());
                    kept.push(kv.to_string());
                }
                "sslrootcert" => extra.sslrootcert = Some(v.to_string()),
                "passfile" => extra.passfile = Some(v.to_string()),
                _ => kept.push(kv.to_string()),
            }
        }
        if extra
            .sslmode
            .as_deref()
            .is_some_and(|m| m.starts_with("verify"))
        {
            kept.push("sslmode=require".into());
        }
        let s = if kept.is_empty() {
            base.to_string()
        } else {
            format!("{base}?{}", kept.join("&"))
        };
        return (s, extra);
    }
    let mut kept = Vec::new();
    for (k, v) in kv_pairs(t) {
        match k.as_str() {
            "sslmode" => {
                extra.sslmode = Some(v.clone());
                let m = if v.starts_with("verify") {
                    "require".to_string()
                } else {
                    v
                };
                kept.push(format!("sslmode={}", quote(&m)));
            }
            "sslrootcert" => extra.sslrootcert = Some(v),
            "passfile" => extra.passfile = Some(v),
            _ => kept.push(format!("{k}={}", quote(&v))),
        }
    }
    (kept.join(" "), extra)
}

fn is_local(host: &tokio_postgres::config::Host) -> bool {
    match host {
        tokio_postgres::config::Host::Unix(_) => true,
        tokio_postgres::config::Host::Tcp(h) => {
            h == "localhost"
                || h == "::1"
                || h.parse::<std::net::Ipv4Addr>()
                    .is_ok_and(|a| a.is_loopback())
        }
    }
}

/// Look a password up in a pgpass file: `host:port:db:user:password`.
fn pgpass_lookup(
    path: &std::path::Path,
    host: &str,
    port: u16,
    db: &str,
    user: &str,
) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                }
                ':' if fields.len() < 4 => fields.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
        }
        fields.push(cur);
        if fields.len() != 5 {
            continue;
        }
        let m = |pat: &str, v: &str| pat == "*" || pat == v;
        if m(&fields[0], host)
            && m(&fields[1], &port.to_string())
            && m(&fields[2], db)
            && m(&fields[3], user)
        {
            return Some(fields[4].clone());
        }
    }
    None
}

impl Conn {
    /// Open a connection with the bounded waits of spec/failure.py
    /// BoundsEveryWait and the TLS policy of spec/security.py
    /// RequiresTlsForRemote.
    pub fn open(settings: &ConnSettings) -> anyhow::Result<Conn> {
        let redacted = settings.redacted();
        let (stripped, extra) = split_extra(&settings.conn);
        let mut cfg: tokio_postgres::Config = stripped
            .parse()
            .map_err(|e| anyhow!("parse connection string {redacted}: {e}"))?;
        if cfg.get_hosts().is_empty() {
            let host = std::env::var("PGHOST").unwrap_or_else(|_| "/run/postgresql".to_string());
            cfg.host(&host);
        }
        if cfg.get_user().is_none() {
            let user = std::env::var("PGUSER").unwrap_or_else(|_| current_user());
            cfg.user(&user);
        }
        if cfg.get_dbname().is_none() {
            if let Ok(db) = std::env::var("PGDATABASE") {
                cfg.dbname(&db);
            }
        }
        cfg.connect_timeout(settings.connect_timeout);
        cfg.keepalives(true);
        cfg.keepalives_idle(Duration::from_secs(10));
        cfg.keepalives_interval(Duration::from_secs(5));
        cfg.keepalives_retries(3);
        cfg.tcp_user_timeout(Duration::from_secs(30));
        cfg.application_name(&settings.application_name);
        let mut opts = format!(
            "-c statement_timeout={} -c lock_timeout={} -c idle_in_transaction_session_timeout={}",
            settings.statement_timeout.as_millis(),
            settings.lock_timeout.as_millis(),
            settings.idle_in_transaction_timeout.as_millis()
        );
        if settings.async_commit {
            opts.push_str(" -c synchronous_commit=off");
        }
        if let Some(o) = cfg.get_options() {
            opts = format!("{o} {opts}");
        }
        cfg.options(&opts);

        // Passwords: the string itself, --password-file, PGPASSWORD, then
        // a pgpass file.
        if cfg.get_password().is_none() {
            if let Some(pw) = password_from_sources(settings, &extra, &cfg) {
                cfg.password(pw);
            }
        }

        // TLS policy.
        let remote = cfg.get_hosts().iter().any(|h| !is_local(h));
        let verify = extra.sslmode.as_deref().is_some_and(|m| m == "verify-full");
        if remote && !verify && !settings.insecure_transport {
            anyhow::bail!(
                "connection to a remote host requires sslmode=verify-full ({redacted}); \
                 pass --insecure-transport to override"
            );
        }
        let tls = if extra
            .sslmode
            .as_deref()
            .is_some_and(|m| m != "disable" && m != "prefer")
            || verify
        {
            Tls::Rustls(rustls_connector(extra.sslrootcert.as_deref())?)
        } else {
            Tls::None
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build connection runtime")?;
        let sync_rep_warning = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let (ntx, nrx) = tokio::sync::mpsc::unbounded_channel();
        let client = match &tls {
            Tls::None => {
                let (client, connection) = rt
                    .block_on(cfg.connect(tokio_postgres::NoTls))
                    .map_err(|e| anyhow!("connect to {redacted}: {}", DbError::from_pg(e)))?;
                spawn_connection(
                    &rt,
                    connection,
                    sync_rep_warning.clone(),
                    closed.clone(),
                    ntx,
                );
                client
            }
            Tls::Rustls(t) => {
                let (client, connection) = rt
                    .block_on(cfg.connect(t.clone()))
                    .map_err(|e| anyhow!("connect to {redacted}: {}", DbError::from_pg(e)))?;
                spawn_connection(
                    &rt,
                    connection,
                    sync_rep_warning.clone(),
                    closed.clone(),
                    ntx,
                );
                client
            }
        };
        let row = rt
            .block_on(client.query_one(
                "SELECT pg_backend_pid(), current_setting('server_version_num')::int",
                &[],
            ))
            .map_err(|e| anyhow!("query backend pid on {redacted}: {}", DbError::from_pg(e)))?;
        Ok(Conn {
            rt,
            client,
            backend_pid: row.get(0),
            server_version_num: row.get(1),
            sync_rep_warning,
            cancel_sent: Arc::new(AtomicBool::new(false)),
            closed,
            notifications: nrx,
            stmts: HashMap::new(),
            tls,
            redacted,
        })
    }

    /// Prepare a named statement once, when the connection opens
    /// (spec/optimize.py PreparedStatementReq).
    pub fn prepare(&mut self, name: &'static str, sql: &str) -> Result<(), DbError> {
        let st = self
            .rt
            .block_on(self.client.prepare(sql))
            .map_err(DbError::from_pg)?;
        self.stmts.insert(name, st);
        Ok(())
    }

    pub fn has_statement(&self, name: &str) -> bool {
        self.stmts.contains_key(name)
    }

    /// Run a prepared statement, inside a `db` span recording the statement
    /// name, the inode, the backend pid and the round-trip time
    /// (spec/app.py InstrumentsDbOperations).
    pub fn query(
        &mut self,
        name: &'static str,
        ino: Option<u64>,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, DbError> {
        let span = tracing::debug_span!(
            "db",
            stmt = name,
            ino = ino,
            pid = self.backend_pid,
            rtt_us = tracing::field::Empty
        );
        let _g = span.enter();
        #[cfg(feature = "fault-injection")]
        if let Some(e) = crate::fault::check(name) {
            return Err(e);
        }
        let st = self.stmts.get(name).cloned().ok_or_else(|| {
            DbError::synthetic("XX000", &format!("statement {name} is not prepared"))
        })?;
        let t0 = Instant::now();
        let r = self.rt.block_on(self.client.query(&st, params));
        let rtt = t0.elapsed();
        metrics::DB_LATENCY.record(rtt);
        metrics::record_statement_latency(name, rtt);
        span.record("rtt_us", rtt.as_micros() as u64);
        #[cfg(feature = "fault-injection")]
        if r.is_ok() {
            if let Some(e) = crate::fault::check_after(name) {
                return Err(e);
            }
        }
        r.map_err(DbError::from_pg)
    }

    /// Run a statement and, in the same network round trip, a second
    /// autocommit statement (spec/replica.py LsnTicketReads).
    pub fn query_pipelined(
        &mut self,
        name: &'static str,
        ino: Option<u64>,
        params: &[&(dyn ToSql + Sync)],
        second: &'static str,
    ) -> Result<(Vec<Row>, Option<Row>), DbError> {
        let span = tracing::debug_span!("db", stmt = name, ino = ino, pid = self.backend_pid);
        let _g = span.enter();
        #[cfg(feature = "fault-injection")]
        if let Some(e) = crate::fault::check(name) {
            return Err(e);
        }
        let st = self.stmts.get(name).cloned().ok_or_else(|| {
            DbError::synthetic("XX000", &format!("statement {name} is not prepared"))
        })?;
        let st2 = self.stmts.get(second).cloned().ok_or_else(|| {
            DbError::synthetic("XX000", &format!("statement {second} is not prepared"))
        })?;
        let t0 = Instant::now();
        let client = &self.client;
        let (a, b) = self.rt.block_on(async {
            futures_util::join!(client.query(&st, params), client.query_opt(&st2, &[]))
        });
        let rtt = t0.elapsed();
        metrics::DB_LATENCY.record(rtt);
        metrics::record_statement_latency(name, rtt);
        let rows = a.map_err(DbError::from_pg)?;
        Ok((rows, b.ok().flatten()))
    }

    /// Run SQL text that is not on the hot path (CLI commands, startup checks).
    pub fn query_sql(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, DbError> {
        let t0 = Instant::now();
        let r = self
            .rt
            .block_on(self.client.query(sql, params))
            .map_err(DbError::from_pg);
        metrics::DB_LATENCY.record(t0.elapsed());
        r
    }

    /// Run several statements in one simple-query round trip.
    pub fn batch(&mut self, sql: &str) -> Result<(), DbError> {
        self.rt
            .block_on(self.client.batch_execute(sql))
            .map_err(DbError::from_pg)
    }

    pub fn canceller(&self) -> Canceller {
        Canceller {
            token: self.client.cancel_token(),
            tls: self.tls.clone(),
            sent: self.cancel_sent.clone(),
        }
    }

    /// True (once) if a `SyncRepWaitForLSN` warning arrived, or a cancel was
    /// sent, since the last call: the last synchronous commit is not known
    /// to be replicated (spec/durability.py FsyncIsDurable).
    pub fn take_unconfirmed_sync(&self) -> bool {
        let w = self.sync_rep_warning.swap(false, Ordering::AcqRel);
        let c = self.cancel_sent.swap(false, Ordering::AcqRel);
        w || c
    }

    pub fn clear_cancel(&self) {
        self.cancel_sent.store(false, Ordering::Release);
    }

    pub fn cancel_was_sent(&self) -> bool {
        self.cancel_sent.load(Ordering::Acquire)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.client.is_closed()
    }

    /// Wait up to `timeout` for a NOTIFY on this connection.
    pub fn wait_notification(&mut self, timeout: Duration) -> Option<Notification> {
        if let Ok(n) = self.notifications.try_recv() {
            return Some(n);
        }
        let rx = &mut self.notifications;
        self.rt
            .block_on(async { tokio::time::timeout(timeout, rx.recv()).await })
            .ok()
            .flatten()
    }
}

fn spawn_connection<S, T>(
    rt: &tokio::runtime::Runtime,
    connection: tokio_postgres::Connection<S, T>,
    warn: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    ntx: tokio::sync::mpsc::UnboundedSender<Notification>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    rt.spawn(async move {
        let mut connection = connection;
        let mut stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
        use futures_util::StreamExt;
        while let Some(m) = stream.next().await {
            match m {
                Ok(AsyncMessage::Notice(n)) => {
                    if n.routine() == Some("SyncRepWaitForLSN") {
                        warn.store(true, Ordering::Release);
                        tracing::warn!(
                            "postgres: {} (synchronous replication not confirmed)",
                            n.message()
                        );
                    } else if n.severity() == "WARNING" {
                        tracing::warn!("postgres: {}", n.message());
                    } else {
                        tracing::debug!("postgres: {}", n.message());
                    }
                }
                Ok(AsyncMessage::Notification(n)) => {
                    let _ = ntx.send(n);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("connection ended: {e}");
                    break;
                }
            }
        }
        closed.store(true, Ordering::Release);
    });
}

fn password_from_sources(
    settings: &ConnSettings,
    extra: &Extra,
    cfg: &tokio_postgres::Config,
) -> Option<String> {
    if let Some(p) = &settings.password_file {
        if let Ok(s) = std::fs::read_to_string(p) {
            return Some(s.trim_end_matches(['\n', '\r']).to_string());
        }
        tracing::warn!("could not read password file {}", p.display());
    }
    if let Ok(pw) = std::env::var("PGPASSWORD") {
        return Some(pw);
    }
    let passfile = extra
        .passfile
        .clone()
        .or_else(|| std::env::var("PGPASSFILE").ok())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".pgpass"))
        })?;
    let host = match cfg.get_hosts().first()? {
        tokio_postgres::config::Host::Tcp(h) => h.clone(),
        tokio_postgres::config::Host::Unix(p) => {
            let _ = p;
            "localhost".to_string()
        }
    };
    let port = cfg.get_ports().first().copied().unwrap_or(5432);
    let user = cfg.get_user().unwrap_or_default().to_string();
    let db = cfg.get_dbname().unwrap_or(&user).to_string();
    pgpass_lookup(&passfile, &host, port, &db, &user)
}

fn rustls_connector(
    rootcert: Option<&str>,
) -> anyhow::Result<tokio_postgres_rustls::MakeRustlsConnect> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    if let Some(path) = rootcert {
        let pem = std::fs::read(path).with_context(|| format!("read sslrootcert {path}"))?;
        for cert in rustls_pemfile_certs(&pem)? {
            roots
                .add(cert)
                .map_err(|e| anyhow!("add root certificate from {path}: {e}"))?;
        }
    } else {
        let loaded = rustls_native_certs::load_native_certs();
        for cert in loaded.certs {
            let _ = roots.add(cert);
        }
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

/// Parse PEM certificates without another dependency.
fn rustls_pemfile_certs(
    pem: &[u8],
) -> anyhow::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    use rustls::pki_types::pem::PemObject;
    rustls::pki_types::CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow!("parse PEM certificates: {e:?}"))
}

pub fn current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| {
        // SAFETY: getpwuid returns a pointer into static storage or null.
        unsafe {
            let pw = libc::getpwuid(libc::geteuid());
            if pw.is_null() {
                return "postgres".to_string();
            }
            std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned()
        }
    })
}

/// The libpq string for a project-local cluster under `testdata/`.
pub fn default_conn() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!("host={cwd}/testdata dbname=pgfs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libpq_only_parameters_are_stripped() {
        let (s, e) =
            split_extra("host=db.example sslmode=verify-full sslrootcert=/ca.pem dbname=pgfs");
        assert_eq!(e.sslmode.as_deref(), Some("verify-full"));
        assert_eq!(e.sslrootcert.as_deref(), Some("/ca.pem"));
        assert!(s.contains("sslmode='require'"));
        assert!(!s.contains("sslrootcert"));
        let (s, e) = split_extra("postgresql://db/pgfs?sslmode=verify-full&application_name=x");
        assert_eq!(e.sslmode.as_deref(), Some("verify-full"));
        assert!(s.contains("sslmode=require") && s.contains("application_name=x"));
    }

    #[test]
    fn keyvalue_parsing_handles_quotes() {
        let p = kv_pairs("host='/a b' dbname = pgfs password='x\\'y'");
        assert_eq!(p[0], ("host".into(), "/a b".into()));
        assert_eq!(p[1], ("dbname".into(), "pgfs".into()));
        assert_eq!(p[2], ("password".into(), "x'y".into()));
    }

    #[test]
    fn pgpass_matches_wildcards() {
        let dir = std::env::temp_dir().join(format!("pgfs-pgpass-{}", std::process::id()));
        std::fs::write(&dir, "# c\nother:5432:*:*:no\ndb:*:pgfs:u:se\\:cret\n").unwrap();
        assert_eq!(
            pgpass_lookup(&dir, "db", 5433, "pgfs", "u").as_deref(),
            Some("se:cret")
        );
        assert_eq!(pgpass_lookup(&dir, "x", 5432, "pgfs", "u"), None);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn remote_hosts_without_verified_tls_are_refused() {
        let mut s = ConnSettings::new("host=192.0.2.1 dbname=pgfs");
        s.connect_timeout = Duration::from_millis(100);
        let err = Conn::open(&s).err().unwrap();
        assert!(format!("{err:#}").contains("verify-full"), "{err:#}");
    }
}

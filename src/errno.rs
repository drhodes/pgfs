//! Errors the filesystem replies, and how database errors map to them.
//!
//! spec/posix.py ErrnoCatalogue lists every errno pgfs can reply;
//! spec/failure.py MapsDatabaseErrors maps database errors by SQLSTATE in
//! one table, never by matching message text. `PF0xx` codes are raised by
//! the `pgfs.*` SQL functions (sql/v1/10_core.sql lists them).

use std::fmt;

/// One entry of the errno catalogue: the errno and its cause.
pub struct Catalogued {
    pub errno: i32,
    pub name: &'static str,
    pub cause: &'static str,
}

/// Every errno pgfs can reply (spec/posix.py ErrnoCatalogue).
pub const CATALOGUE: &[Catalogued] = &[
    Catalogued {
        errno: libc::ENOENT,
        name: "ENOENT",
        cause: "the name or inode does not exist",
    },
    Catalogued {
        errno: libc::EEXIST,
        name: "EEXIST",
        cause: "the name exists",
    },
    Catalogued {
        errno: libc::ENOTEMPTY,
        name: "ENOTEMPTY",
        cause: "rmdir, or rename over a non-empty directory",
    },
    Catalogued {
        errno: libc::EISDIR,
        name: "EISDIR",
        cause: "wrong kind for the operation",
    },
    Catalogued {
        errno: libc::ENOTDIR,
        name: "ENOTDIR",
        cause: "wrong kind for the operation",
    },
    Catalogued {
        errno: libc::EINVAL,
        name: "EINVAL",
        cause: "invalid name or flags; rename into its own subtree",
    },
    Catalogued {
        errno: libc::ENAMETOOLONG,
        name: "ENAMETOOLONG",
        cause: "a name over 255 bytes or a symlink target over 4 095 bytes",
    },
    Catalogued {
        errno: libc::EXDEV,
        name: "EXDEV",
        cause: "link or rename across trees",
    },
    Catalogued {
        errno: libc::EMLINK,
        name: "EMLINK",
        cause: "too many links or too deep a branch",
    },
    Catalogued {
        errno: libc::EFBIG,
        name: "EFBIG",
        cause: "past the maximum file size",
    },
    Catalogued {
        errno: libc::ENOSPC,
        name: "ENOSPC",
        cause: "capacity reached, or Postgres disk_full",
    },
    Catalogued {
        errno: libc::EDQUOT,
        name: "EDQUOT",
        cause: "per-tree quota reached",
    },
    Catalogued {
        errno: libc::EROFS,
        name: "EROFS",
        cause: "a write to a snapshot or read-only mount",
    },
    Catalogued {
        errno: libc::EPERM,
        name: "EPERM",
        cause: "a hard link to a directory",
    },
    Catalogued {
        errno: libc::EACCES,
        name: "EACCES",
        cause: "permission denied",
    },
    Catalogued {
        errno: libc::EBUSY,
        name: "EBUSY",
        cause: "root of a view, or a tree, branch or snapshot in use",
    },
    Catalogued {
        errno: libc::ESTALE,
        name: "ESTALE",
        cause: "stale inode generation, deleted inode, or changed tree",
    },
    Catalogued {
        errno: libc::EBADF,
        name: "EBADF",
        cause: "a handle not open for the operation",
    },
    Catalogued {
        errno: libc::ENODATA,
        name: "ENODATA",
        cause: "missing xattr",
    },
    Catalogued {
        errno: libc::ERANGE,
        name: "ERANGE",
        cause: "xattr buffer too small",
    },
    Catalogued {
        errno: libc::E2BIG,
        name: "E2BIG",
        cause: "xattr value too large",
    },
    Catalogued {
        errno: libc::EOPNOTSUPP,
        name: "EOPNOTSUPP",
        cause: "unsupported operation",
    },
    Catalogued {
        errno: libc::ENOTTY,
        name: "ENOTTY",
        cause: "ioctl",
    },
    Catalogued {
        errno: libc::ENOSYS,
        name: "ENOSYS",
        cause: "bmap",
    },
    Catalogued {
        errno: libc::EINTR,
        name: "EINTR",
        cause: "the request was interrupted",
    },
    Catalogued {
        errno: libc::ETIMEDOUT,
        name: "ETIMEDOUT",
        cause: "a statement exceeded its timeout",
    },
    Catalogued {
        errno: libc::EIO,
        name: "EIO",
        cause: "database unreachable, mount fenced, checksum failure, or invariant violated",
    },
];

pub fn errno_name(errno: i32) -> &'static str {
    CATALOGUE
        .iter()
        .find(|c| c.errno == errno)
        .map(|c| c.name)
        .unwrap_or("E?")
}

/// The errno named by a `PF0xx` SQLSTATE raised by a pgfs function.
pub fn pf_errno(code: &str) -> Option<i32> {
    Some(match code {
        "PF002" => libc::ENOENT,
        "PF003" => libc::EEXIST,
        "PF004" => libc::ENOTEMPTY,
        "PF005" => libc::EISDIR,
        "PF006" => libc::ENOTDIR,
        "PF007" => libc::EINVAL,
        "PF008" => libc::ENAMETOOLONG,
        "PF009" => libc::EXDEV,
        "PF010" => libc::ESTALE,
        "PF011" => libc::ESTALE,
        "PF012" => libc::EMLINK,
        "PF013" => libc::EFBIG,
        "PF014" => libc::ENOSPC,
        "PF015" => libc::EDQUOT,
        "PF016" => libc::EROFS,
        "PF017" => libc::EPERM,
        "PF018" => libc::EBUSY,
        "PF019" => libc::ESTALE,
        "PF020" => libc::ENODATA,
        "PF021" => libc::E2BIG,
        "PF022" => libc::EOPNOTSUPP,
        "PF023" => libc::ERANGE,
        "PF024" => libc::EACCES,
        "PF026" => libc::ETIMEDOUT,
        _ => return None,
    })
}

/// How a database error class is handled (spec/failure.py MapsDatabaseErrors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbClass {
    /// 23505: EEXIST.
    UniqueViolation,
    /// PF001: EIO, and the mount moves to Fenced.
    Fenced,
    /// PF002..PF0FF: the errno the function names.
    Function(i32),
    /// PF025: flush the named inodes and call again.
    RetryDirty,
    /// 40001, 40P01: retried, then EIO.
    Serialization,
    /// 55P03: ETIMEDOUT, logged at WARN.
    LockTimeout,
    /// 57014: EINTR if interrupted, else ETIMEDOUT.
    Cancelled,
    /// 53100: ENOSPC, logged at ERROR.
    DiskFull,
    /// 53200, 53300: EIO, WARN, back off.
    Resources,
    /// 25006: the server is a standby; EIO and the connection is Broken.
    ReadOnlyServer,
    /// 08*, 57P01-57P03, socket errors: EIO for new requests; the
    /// in-flight mutation awaits reconnect.
    Connection,
    /// Anything else: EIO, ERROR with the full chain.
    Other,
}

/// Classify a SQLSTATE (None: no SQLSTATE, i.e. an I/O or protocol error,
/// which is a connection-class failure).
pub fn classify(sqlstate: Option<&str>) -> DbClass {
    let Some(code) = sqlstate else {
        return DbClass::Connection;
    };
    match code {
        "23505" => DbClass::UniqueViolation,
        "PF001" => DbClass::Fenced,
        "PF025" => DbClass::RetryDirty,
        "40001" | "40P01" => DbClass::Serialization,
        "55P03" => DbClass::LockTimeout,
        "57014" => DbClass::Cancelled,
        "53100" => DbClass::DiskFull,
        "53200" | "53300" => DbClass::Resources,
        "25006" => DbClass::ReadOnlyServer,
        "57P01" | "57P02" | "57P03" => DbClass::Connection,
        c if c.starts_with("08") => DbClass::Connection,
        c if c.starts_with("PF") => match pf_errno(c) {
            Some(e) => DbClass::Function(e),
            None => DbClass::Other,
        },
        _ => DbClass::Other,
    }
}

/// A failed filesystem operation.
#[derive(Debug)]
pub enum FsError {
    /// An expected condition: reply the errno, log at DEBUG at most
    /// (spec/app.py DistinguishesExpectedErrors).
    Errno(i32),
    /// The SQL function asked the caller to flush these inodes' buffers
    /// and call again (PF025).
    RetryDirty(Vec<u64>),
    /// The mount lost its lease or session (PF001).
    Fenced(String),
    /// The connection is not usable; new requests fail fast with EIO.
    Unavailable(String),
    /// An unexpected failure: reply `errno` (EIO unless stated) and log
    /// the full chain.
    Unexpected { errno: i32, error: anyhow::Error },
}

impl FsError {
    pub fn errno(&self) -> i32 {
        match self {
            FsError::Errno(e) => *e,
            FsError::RetryDirty(_) => libc::EIO,
            FsError::Fenced(_) => libc::EIO,
            FsError::Unavailable(_) => libc::EIO,
            FsError::Unexpected { errno, .. } => *errno,
        }
    }

    pub fn is_expected(&self) -> bool {
        matches!(self, FsError::Errno(e) if *e != libc::EIO)
    }

    pub fn unexpected(error: anyhow::Error) -> Self {
        FsError::Unexpected {
            errno: libc::EIO,
            error,
        }
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::Errno(e) => write!(f, "{}", errno_name(*e)),
            FsError::RetryDirty(inos) => write!(f, "retry after flushing inodes {inos:?}"),
            FsError::Fenced(why) => write!(f, "fenced: {why}"),
            FsError::Unavailable(why) => write!(f, "database unavailable: {why}"),
            FsError::Unexpected { errno, error } => write!(f, "{}: {error:#}", errno_name(*errno)),
        }
    }
}

impl From<i32> for FsError {
    fn from(e: i32) -> Self {
        FsError::Errno(e)
    }
}

pub type FsResult<T> = std::result::Result<T, FsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sqlstate_row_maps_as_specified() {
        assert_eq!(classify(Some("23505")), DbClass::UniqueViolation);
        assert_eq!(classify(Some("PF001")), DbClass::Fenced);
        assert_eq!(classify(Some("PF002")), DbClass::Function(libc::ENOENT));
        assert_eq!(classify(Some("PF018")), DbClass::Function(libc::EBUSY));
        assert_eq!(classify(Some("PF025")), DbClass::RetryDirty);
        assert_eq!(classify(Some("40001")), DbClass::Serialization);
        assert_eq!(classify(Some("40P01")), DbClass::Serialization);
        assert_eq!(classify(Some("55P03")), DbClass::LockTimeout);
        assert_eq!(classify(Some("57014")), DbClass::Cancelled);
        assert_eq!(classify(Some("53100")), DbClass::DiskFull);
        assert_eq!(classify(Some("53200")), DbClass::Resources);
        assert_eq!(classify(Some("53300")), DbClass::Resources);
        assert_eq!(classify(Some("25006")), DbClass::ReadOnlyServer);
        assert_eq!(classify(Some("08006")), DbClass::Connection);
        assert_eq!(classify(Some("57P01")), DbClass::Connection);
        assert_eq!(classify(None), DbClass::Connection);
        assert_eq!(classify(Some("XX000")), DbClass::Other);
    }

    #[test]
    fn catalogue_names_are_unique_and_include_eio() {
        let mut names: Vec<_> = CATALOGUE.iter().map(|c| c.name).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(len, names.len());
        assert_eq!(errno_name(libc::EIO), "EIO");
    }

    #[test]
    fn expected_errors_exclude_eio() {
        assert!(FsError::Errno(libc::ENOENT).is_expected());
        assert!(!FsError::Errno(libc::EIO).is_expected());
        assert!(!FsError::Fenced("x".into()).is_expected());
    }
}

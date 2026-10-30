//! Names: file names as exact bytes (spec/storage.py ByteExactNames), tree
//! and snapshot names (spec/branch.py TreesTable), and view specifications
//! `TREE[@SNAPSHOT][:/PATH]` (spec/branch.py ViewsRootedAtDirectory).

/// Check a file name before any query: 1 to 255 bytes, no `/`, no NUL,
/// not `.` or `..`. Returns the errno to reply.
pub fn check_name(name: &[u8]) -> Result<(), i32> {
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') || name.contains(&0)
    {
        return Err(libc::EINVAL);
    }
    if name.len() > 255 {
        return Err(libc::ENAMETOOLONG);
    }
    Ok(())
}

/// Check a tree or snapshot name: 1 to 64 characters from `[A-Za-z0-9._-]`,
/// not starting with `.`.
pub fn check_tree_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid name {name:?}: use 1 to 64 characters from [A-Za-z0-9._-], not starting with \".\""
        ))
    }
}

/// A view of a filesystem: a tree, optionally a snapshot of it, optionally
/// rooted at a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewSpec {
    pub tree: String,
    pub snapshot: Option<String>,
    pub path: Option<String>,
}

impl ViewSpec {
    pub fn parse(s: &str) -> Result<ViewSpec, String> {
        let (head, path) = match s.split_once(':') {
            Some((h, p)) => (h, Some(p.to_string())),
            None => (s, None),
        };
        let (tree, snapshot) = match head.split_once('@') {
            Some((t, n)) => (t.to_string(), Some(n.to_string())),
            None => (head.to_string(), None),
        };
        check_tree_name(&tree)?;
        if let Some(n) = &snapshot {
            check_tree_name(n)?;
        }
        if let Some(p) = &path {
            if !p.starts_with('/') {
                return Err(format!("view path {p:?} must start with /"));
            }
        }
        let path = path.filter(|p| p != "/");
        Ok(ViewSpec {
            tree,
            snapshot,
            path,
        })
    }
}

impl std::fmt::Display for ViewSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.tree)?;
        if let Some(s) = &self.snapshot {
            write!(f, "@{s}")?;
        }
        if let Some(p) = &self.path {
            write!(f, ":{p}")?;
        }
        Ok(())
    }
}

/// Render bytes as text: UTF-8 unchanged, other bytes as `\xNN`.
pub fn display_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b
            .iter()
            .map(|&c| {
                if (32..127).contains(&c) && c != b'\\' {
                    (c as char).to_string()
                } else {
                    format!("\\x{c:02x}")
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_byte_rules() {
        assert_eq!(check_name(b"a"), Ok(()));
        assert_eq!(check_name(b"%_\\"), Ok(()));
        assert_eq!(check_name(&[0xff, 0xfe]), Ok(()));
        assert_eq!(check_name(&[b'x'; 255]), Ok(()));
        assert_eq!(check_name(&[b'x'; 256]), Err(libc::ENAMETOOLONG));
        assert_eq!(check_name(b""), Err(libc::EINVAL));
        assert_eq!(check_name(b"a/b"), Err(libc::EINVAL));
        assert_eq!(check_name(b"a\0b"), Err(libc::EINVAL));
        assert_eq!(check_name(b"."), Err(libc::EINVAL));
        assert_eq!(check_name(b".."), Err(libc::EINVAL));
    }

    #[test]
    fn tree_names() {
        assert!(check_tree_name("main").is_ok());
        assert!(check_tree_name("job-17.v2_x").is_ok());
        assert!(check_tree_name(".hidden").is_err());
        assert!(check_tree_name("").is_err());
        assert!(check_tree_name("a/b").is_err());
        assert!(check_tree_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn view_specs_parse() {
        assert_eq!(
            ViewSpec::parse("main").unwrap(),
            ViewSpec {
                tree: "main".into(),
                snapshot: None,
                path: None
            }
        );
        assert_eq!(
            ViewSpec::parse("main@nightly:/projects/web").unwrap(),
            ViewSpec {
                tree: "main".into(),
                snapshot: Some("nightly".into()),
                path: Some("/projects/web".into())
            }
        );
        assert_eq!(ViewSpec::parse("main:/").unwrap().path, None);
        assert!(ViewSpec::parse("main:rel").is_err());
        assert!(ViewSpec::parse("bad name").is_err());
    }

    #[test]
    fn non_utf8_bytes_are_escaped() {
        assert_eq!(display_bytes(b"plain"), "plain");
        assert_eq!(display_bytes(&[b'a', 0xff]), "a\\xff");
    }
}

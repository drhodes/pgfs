//! Secrets never reach a log line (spec/security.py RedactsSecrets).

/// Replace the `password` parameter and any URI userinfo of a connection
/// string with `***`.
pub fn redact(conn: &str) -> String {
    let trimmed = conn.trim();
    if let Some(rest) = trimmed
        .strip_prefix("postgresql://")
        .map(|r| ("postgresql://", r))
        .or_else(|| {
            trimmed
                .strip_prefix("postgres://")
                .map(|r| ("postgres://", r))
        })
    {
        let (scheme, rest) = rest;
        let (authority, tail) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let authority = match authority.rfind('@') {
            Some(i) => format!("***@{}", &authority[i + 1..]),
            None => authority.to_string(),
        };
        let tail = redact_query(tail);
        return format!("{scheme}{authority}{tail}");
    }
    redact_keyvalue(trimmed)
}

fn redact_query(tail: &str) -> String {
    let Some(q) = tail.find('?') else {
        return tail.to_string();
    };
    let (path, query) = tail.split_at(q + 1);
    let parts: Vec<String> = query
        .split('&')
        .map(|kv| match kv.split_once('=') {
            Some((k, _)) if k.eq_ignore_ascii_case("password") => format!("{k}=***"),
            _ => kv.to_string(),
        })
        .collect();
    format!("{path}{}", parts.join("&"))
}

/// Redact `password=...` in a libpq key=value string, honouring single
/// quotes and backslash escapes in the value.
fn redact_keyvalue(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        // Read a key.
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &s[key_start..i];
        out.push_str(key);
        // Whitespace and '='.
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'=') {
            out.push(bytes[i] as char);
            i += 1;
        }
        // Read a value.
        let value_start = i;
        if i < bytes.len() && bytes[i] == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\'' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
        } else {
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        }
        let value = &s[value_start..i.min(bytes.len())];
        if key.eq_ignore_ascii_case("password") {
            out.push_str("***");
        } else {
            out.push_str(value);
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// True if the connection string carries a password.
pub fn has_password(conn: &str) -> bool {
    let t = conn.trim();
    if let Some(rest) = t
        .strip_prefix("postgresql://")
        .or_else(|| t.strip_prefix("postgres://"))
    {
        let authority = &rest[..rest.find(['/', '?']).unwrap_or(rest.len())];
        let userinfo_pw = authority
            .rfind('@')
            .is_some_and(|i| authority[..i].contains(':'));
        return userinfo_pw || redact_query(rest) != rest;
    }
    redact_keyvalue(t) != t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyvalue_passwords_are_redacted() {
        assert_eq!(
            redact("host=db user=u password=s3cret dbname=pgfs"),
            "host=db user=u password=*** dbname=pgfs"
        );
        assert_eq!(redact("password='a b\\' c' host=x"), "password=*** host=x");
        assert_eq!(redact("host=/tmp dbname=pgfs"), "host=/tmp dbname=pgfs");
    }

    #[test]
    fn uri_userinfo_and_query_passwords_are_redacted() {
        assert_eq!(
            redact("postgresql://u:pw@db:5432/pgfs?sslmode=verify-full"),
            "postgresql://***@db:5432/pgfs?sslmode=verify-full"
        );
        assert_eq!(
            redact("postgres://db/pgfs?password=pw&user=u"),
            "postgres://db/pgfs?password=***&user=u"
        );
    }

    #[test]
    fn has_password_detects_secrets() {
        assert!(has_password("host=x password=y"));
        assert!(!has_password("host=x"));
    }
}

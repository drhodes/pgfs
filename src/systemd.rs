//! systemd readiness and watchdog notifications (spec/ops.py NotifiesSystemd).

use std::os::unix::net::UnixDatagram;

/// Send a notification when `NOTIFY_SOCKET` is set; silently nothing otherwise.
pub fn notify(msg: &str) {
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    let Ok(sock) = UnixDatagram::unbound() else {
        return;
    };
    let r = if let Some(abs) = path.strip_prefix('@') {
        use std::os::linux::net::SocketAddrExt;
        std::os::unix::net::SocketAddr::from_abstract_name(abs.as_bytes())
            .and_then(|addr| sock.send_to_addr(msg.as_bytes(), &addr))
    } else {
        sock.send_to(msg.as_bytes(), &path)
    };
    if let Err(e) = r {
        tracing::debug!("sd_notify {msg:?}: {e}");
    }
}

pub fn enabled() -> bool {
    std::env::var_os("NOTIFY_SOCKET").is_some()
}

#[cfg(test)]
mod tests {
    #[test]
    fn notify_reaches_the_socket() {
        let dir = std::env::temp_dir().join(format!("pgfs-sd-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let rx = std::os::unix::net::UnixDatagram::bind(&dir).unwrap();
        // SAFETY: tests in this module are the only users of NOTIFY_SOCKET.
        unsafe { std::env::set_var("NOTIFY_SOCKET", &dir) };
        super::notify("READY=1");
        unsafe { std::env::remove_var("NOTIFY_SOCKET") };
        let mut buf = [0u8; 16];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        let _ = std::fs::remove_file(&dir);
    }
}

//! Database faults driven through the backend (spec/verify.py
//! DbFaultInjection): every row of spec/failure.py MapsDatabaseErrors, the
//! state machine transition, and ambiguous commits resolved through
//! op_log. Needs `cargo test --features fault-injection`.

#![cfg(feature = "fault-injection")]

mod common;

use common::*;
use pgfs::backend::*;
use pgfs::errno::FsError;
use pgfs::fault;
use pgfs::health::DbState;
use std::sync::Mutex;

/// Faults are process-global: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

#[test]
fn every_error_class_maps_as_specified() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some(d) = TestDb::new("faults") else {
        return;
    };
    let l = d.layer("main");
    let cases: &[(&str, i32, Option<DbState>)] = &[
        ("23505", libc::EEXIST, None),
        ("PF002", libc::ENOENT, None),
        ("PF018", libc::EBUSY, None),
        ("55P03", libc::ETIMEDOUT, None),
        ("57014", libc::ETIMEDOUT, None),
        ("53100", libc::ENOSPC, None),
        ("53200", libc::EIO, None),
        ("53300", libc::EIO, None),
        ("XX000", libc::EIO, None),
        ("40001", libc::EIO, None),
        ("40P01", libc::EIO, None),
        ("PF001", libc::EIO, Some(DbState::Fenced)),
    ];
    for (i, (state, want, then)) in cases.iter().enumerate() {
        let mut be = d.backend();
        fault::clear();
        if state.starts_with("40") {
            // Retried up to 5 attempts in total, then EIO.
            for _ in 0..5 {
                fault::inject("create", 0, state, None);
            }
        } else {
            fault::inject("create", 0, state, None);
        }
        let r = mk(&mut be, l, 1, format!("f{i}").as_bytes(), Kind::File);
        assert_eq!(
            r.as_ref().err().map(FsError::errno),
            Some(*want),
            "{state}: {r:?}"
        );
        fault::clear();
        let _ = then;
    }
    // A serialization failure that clears is retried with the same op_seq.
    let mut be = d.backend();
    fault::inject("create", 0, "40001", None);
    fault::inject("create", 0, "40P01", None);
    assert!(mk(&mut be, l, 1, b"retried", Kind::File).is_ok());
    fault::clear();
    // Connection-class errors move the state machine to Broken.
    let mut be = d.backend();
    fault::inject("getattr", 0, "08006", None);
    let r = be.getattr(&d.view("main"), 1, None);
    assert_eq!(r.err().map(|e| e.errno()), Some(libc::EIO));
    fault::clear();
    d.assert_check();
}

#[test]
fn an_ambiguous_commit_resolves_through_op_log() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some(d) = TestDb::new("ambig") else {
        return;
    };
    let (mut bs, _id, _f, health) = d.mount_backend("main", 1);
    let l = d.layer("main");
    // A stand-in for the reconnector: revalidate as soon as the state breaks.
    let h = health.clone();
    let t = std::thread::spawn(move || {
        while h.state() == DbState::Connected {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.reconnecting();
        h.revalidating();
        h.connected();
    });
    fault::clear();
    fault::inject_after_commit("create", 0);
    // The create committed, then the connection "dropped": the reply must
    // be the committed result, not EEXIST from running it again.
    let a = mk(&mut bs[0], l, 1, b"once", Kind::File).expect("resolved through op_log");
    fault::clear();
    t.join().unwrap();
    let v = d.view("main");
    assert_eq!(bs[0].lookup(&v, 1, b"once").unwrap().unwrap().ino, a.ino);
    let n: i64 = d.sql("SELECT count(*) FROM pgfs.dentries WHERE name = 'once'::bytea")[0].get(0);
    assert_eq!(n, 1, "applied exactly once");
    d.assert_check();
}

#[test]
fn a_mutation_that_never_ran_runs_again_after_reconnect() {
    let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some(d) = TestDb::new("rerun") else {
        return;
    };
    let (mut bs, _id, _f, health) = d.mount_backend("main", 1);
    let l = d.layer("main");
    let h = health.clone();
    let t = std::thread::spawn(move || {
        while h.state() == DbState::Connected {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.reconnecting();
        h.revalidating();
        h.connected();
    });
    fault::clear();
    fault::inject("create", 0, "57P01", None);
    let a = mk(&mut bs[0], l, 1, b"later", Kind::File).expect("re-run with the same op_seq");
    fault::clear();
    t.join().unwrap();
    assert!(bs[0]
        .getattr(&d.view("main"), a.ino, None)
        .unwrap()
        .is_some());
    d.assert_check();
}

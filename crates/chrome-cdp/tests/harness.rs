//! The E2E harness itself (plan 10, milestone 10.1): a [`Session`] closes
//! its session when dropped, and a [`Sandbox`] finds what a session leaves
//! behind.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::process::Command;

use common::{Sandbox, chrome_path, sockets_in, wait_until};

#[test]
fn a_dropped_session_is_closed_and_leaves_nothing() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let (first, second) = (sandbox.session(), sandbox.session());
    assert_ne!(first.name, second.name);
    first.ok(&["open"]);
    second.ok(&["open"]);
    assert_eq!(sandbox.daemons().len(), 2);
    drop(first);
    assert_eq!(sandbox.daemons().len(), 1);
    drop(second);
    assert_eq!(sandbox.leaks(), Vec::<String>::new());
}

#[test]
fn a_killed_daemon_is_a_leak() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    session.ok(&["open"]);
    let [pid] = sandbox.daemons()[..] else {
        panic!("one daemon")
    };
    // SIGKILL: the daemon cannot remove its socket.
    let killed = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    wait_until("the daemon to die", || sandbox.daemons().is_empty());
    let leaks = sandbox.leaks();
    assert!(
        leaks.iter().any(|l| l.starts_with("socket ")),
        "{leaks:?}"
    );

    // What the next command of the session does with a dead one: it
    // cleans up, so the sandbox ends clean.
    session.ok(&["close"]);
    wait_until("nothing to be left", || sandbox.leaks().is_empty());
    assert!(sockets_in(&sandbox.runtime_dir()).is_empty());
}

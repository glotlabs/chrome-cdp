//! Chrome's processes (plan 02, milestone 2.3): Chrome leads its own
//! process group, and that group is killed however chrome-cdp exits.
//!
//! - A clean close kills the group once the browser has exited, so no
//!   helper outlives it.
//! - On Linux, `PR_SET_PDEATHSIG` kills the browser the moment its parent
//!   dies, even from `kill -9`.
//! - On every OS, a [`Watchdog`] (`sh`) reads a pipe from us. If we die,
//!   the pipe closes; the watchdog kills the group and deletes the
//!   launch's run dir (and with it a temporary profile). This stands in
//!   for plan 02's macOS kqueue watchdog: it needs no `unsafe` and is
//!   tested on Linux too.

use std::io::{self, Write as _};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use crate::launch::sys;

/// The watchdog: waits for a line on stdin. `done` means we cleaned up;
/// anything else (EOF, because we died) kills the group and deletes the
/// run dir. It ignores terminal signals so Ctrl-C reaches only
/// chrome-cdp, which then cleans up itself.
const WATCHDOG: &str = "\
trap '' INT HUP TERM QUIT
IFS= read -r line
[ \"$line\" = done ] && exit 0
kill -s KILL -- \"-$1\" 2>/dev/null
[ -n \"$2\" ] && rm -rf -- \"$2\"
exit 0";

/// Sets up `cmd` to start Chrome as the leader of a new process group that
/// dies with us (Linux).
pub(crate) fn configure(cmd: &mut tokio::process::Command) {
    cmd.process_group(0).kill_on_drop(true);
    #[cfg(target_os = "linux")]
    sys::die_with_parent(cmd);
}

/// Kills Chrome's process group; logs a failure.
pub(crate) fn kill_group(pgid: u32) {
    if let Err(e) = sys::kill_group(pgid) {
        tracing::warn!(pgid, "cannot kill Chrome's process group: {e}");
    }
}

/// Whether anything in Chrome's process group is still running.
#[cfg(test)]
pub(crate) fn group_alive(pgid: u32) -> bool {
    sys::group_alive(pgid)
}

/// Kills a process group (Chrome's, or an E2E web server's) and deletes
/// `remove` if chrome-cdp dies without calling [`Watchdog::disarm`]: a
/// small `sh` in a group of its own waits on a pipe from us, whose end
/// closes when we exit, however that happens.
#[derive(Debug)]
pub struct Watchdog {
    stdin: Option<ChildStdin>,
    child: Child,
}

impl Watchdog {
    pub fn start(pgid: u32, remove: Option<&Path>) -> io::Result<Self> {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(WATCHDOG)
            .arg("chrome-cdp-watchdog")
            .arg(pgid.to_string())
            .arg(remove.map(Path::as_os_str).unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own group: not killed with Chrome's, and no terminal
            // signals meant for chrome-cdp.
            .process_group(0)
            .spawn()?;
        let stdin = child.stdin.take();
        Ok(Self { stdin, child })
    }

    /// Tells the watchdog that we cleaned up, and reaps it.
    pub fn disarm(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"done\n");
        }
        // It exits as soon as it reads the line.
        let _ = self.child.wait();
    }

    #[cfg(test)]
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.disarm();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Watchdog, configure, group_alive, kill_group};

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A process group of a shell and its own child.
    fn group() -> tokio::process::Child {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", "sleep 60 & sleep 60; wait"]);
        configure(&mut cmd);
        cmd.spawn().unwrap()
    }

    #[tokio::test]
    async fn kill_group_kills_every_member() {
        let mut child = group();
        let pgid = child.id().unwrap();
        assert!(group_alive(pgid));
        kill_group(pgid);
        child.wait().await.unwrap();
        wait_until("the group to die", || !group_alive(pgid));
    }

    #[tokio::test]
    async fn the_watchdog_kills_the_group_and_deletes_the_profile_when_its_pipe_closes() {
        let profile = tempfile::tempdir().unwrap().keep();
        std::fs::write(profile.join("Local State"), "{}").unwrap();
        let mut child = group();
        let pgid = child.id().unwrap();
        let mut dog = Watchdog::start(pgid, Some(&profile)).unwrap();
        // What a crash does: the write end goes away without `done`.
        drop(dog.stdin.take());
        child.wait().await.unwrap();
        wait_until("the group to die", || !group_alive(pgid));
        wait_until("the profile to go", || !profile.exists());
        let _ = dog.child.wait();
    }

    #[tokio::test]
    async fn a_disarmed_watchdog_does_nothing() {
        let profile = tempfile::tempdir().unwrap();
        let mut child = group();
        let pgid = child.id().unwrap();
        let mut dog = Watchdog::start(pgid, Some(profile.path())).unwrap();
        let dog_pid = dog.pid();
        dog.disarm();
        assert!(!super::sys::process_alive(dog_pid));
        assert!(group_alive(pgid));
        assert!(profile.path().exists());
        kill_group(pgid);
        child.wait().await.unwrap();
    }
}

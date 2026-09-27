//! The few raw system calls launch needs: the effective uid, signalling a
//! process group, and (Linux) `PR_SET_PDEATHSIG` in the child.
//!
//! Like the fd module in `cdp`, this is one of the two modules allowed to
//! use `unsafe`. Each call is a plain libc call on values we own.
#![allow(unsafe_code)]

use std::io;

/// The effective user id.
pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Sends SIGKILL to every process in the group `pgid`. A group that no
/// longer exists is not an error.
pub(crate) fn kill_group(pgid: u32) -> io::Result<()> {
    let pgid =
        libc::pid_t::try_from(pgid).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    if pgid <= 1 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: kill has no memory preconditions; a negative pid names the group.
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

/// Whether any process in the group `pgid` is still alive (zombies count
/// until they are reaped).
#[cfg(test)]
pub(crate) fn group_alive(pgid: u32) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the group exists.
    unsafe { libc::kill(-pgid, 0) == 0 || *errno() == libc::EPERM }
}

/// Whether a process with this pid exists.
pub(crate) fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 || *errno() == libc::EPERM }
}

/// This machine's host name.
pub(crate) fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname writes at most
    // that many bytes.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).ok()
}

#[cfg(target_os = "linux")]
fn errno() -> *mut libc::c_int {
    // SAFETY: returns a pointer to this thread's errno.
    unsafe { libc::__errno_location() }
}

#[cfg(target_os = "macos")]
fn errno() -> *mut libc::c_int {
    // SAFETY: returns a pointer to this thread's errno.
    unsafe { libc::__error() }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn errno() -> *mut libc::c_int {
    static mut UNKNOWN: libc::c_int = 0;
    &raw mut UNKNOWN
}

/// Makes the child get SIGKILL when the thread that spawns it exits (Linux
/// `PR_SET_PDEATHSIG`). The spawn fails if we are already gone.
///
/// The signal follows the spawning *thread*: spawn from a thread that lives
/// as long as the browser should (a runtime worker or the main thread),
/// never from `spawn_blocking`.
#[cfg(target_os = "linux")]
pub(crate) fn die_with_parent(cmd: &mut tokio::process::Command) {
    let parent = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
    // SAFETY: the closure runs between fork and exec and only makes
    // async-signal-safe calls (prctl, getppid).
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            // The parent died before prctl: we were reparented already.
            if libc::getppid() != parent {
                return Err(io::Error::other("chrome-cdp exited during launch"));
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process_exists_and_has_a_host_name() {
        assert!(super::process_alive(std::process::id()));
        assert!(!super::process_alive(0));
        assert!(super::hostname().is_some_and(|h| !h.is_empty()));
        assert!(super::kill_group(1).is_err());
    }
}

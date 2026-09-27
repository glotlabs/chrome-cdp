//! Maps pipe ends onto fds 3 and 4 in the child process.
//!
//! This is the only module in the workspace allowed to use `unsafe`
//! (`pre_exec` and raw `libc` calls).
#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use tokio::process::Command;

/// The fd Chrome reads commands from.
const CHILD_READ_FD: RawFd = 3;
/// The fd Chrome writes responses and events to.
const CHILD_WRITE_FD: RawFd = 4;
/// Lowest fd the child's pipe ends are moved to before the fork, so that
/// neither can be one of the targets (a `dup2` onto 3 must not clobber the
/// source of the `dup2` onto 4).
const MIN_SOURCE_FD: RawFd = 5;

/// The child's two pipe ends, held open until the child has been spawned.
/// Both are close-on-exec; only their copies on fds 3 and 4 survive `exec`.
#[derive(Debug)]
pub(super) struct ChildFds {
    read: OwnedFd,
    write: OwnedFd,
}

impl ChildFds {
    pub(super) fn new(read: &OwnedFd, write: &OwnedFd) -> io::Result<Self> {
        Ok(Self {
            read: dup_above(read, MIN_SOURCE_FD)?,
            write: dup_above(write, MIN_SOURCE_FD)?,
        })
    }
}

/// Duplicates `fd` to the lowest free fd `>= min`, close-on-exec.
fn dup_above(fd: &OwnedFd, min: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: `fd` is a valid open fd for the duration of the call.
    let new = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, min) };
    if new < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fcntl` returned a new fd that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(new) })
}

/// Arranges for the child to get `fds.read` as fd 3 and `fds.write` as fd 4.
///
/// `fds` must stay open until `cmd` has been spawned.
pub(super) fn install(cmd: &mut Command, fds: &ChildFds) {
    let read = fds.read.as_raw_fd();
    let write = fds.write.as_raw_fd();
    // SAFETY: the closure runs in the child between fork and exec, where only
    // async-signal-safe functions are allowed; it calls only `dup2`. Both
    // sources are >= MIN_SOURCE_FD, so neither is a dup2 target. `dup2` to a
    // different fd clears close-on-exec on the target, so fds 3 and 4
    // survive exec while the sources do not.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(read, CHILD_READ_FD) < 0 || libc::dup2(write, CHILD_WRITE_FD) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

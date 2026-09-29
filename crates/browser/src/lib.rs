//! Chrome discovery, Chrome for Testing install, launch flags and profiles
//! (plan 02).
//!
//! - [`discover`] finds the Chrome to launch (milestone 2.1).
//! - [`Host`] is what discovery reads from the machine: OS, architecture and
//!   a few environment variables. Tests build a fake one.
//! - [`cache_dir`] and [`installed`] know the `chrome-cdp install` cache
//!   layout, which [`install`] (milestone 2.2) fills from the Chrome for
//!   Testing index ([`parse_channels`], [`version_release`]).
//! - [`DepsFix`] and [`missing_libraries`] explain a Linux Chrome that
//!   cannot start for lack of shared libraries.
//! - [`launch()`] starts Chrome over `--remote-debugging-pipe` with the
//!   flags, profile and sandbox rules of milestone 2.3.

mod cft;
mod deps;
mod discover;
mod host;
mod install;
mod installed;
pub mod launch;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod unzip;
mod version;

pub use cft::{
    Channel, ChannelVersion, DEFAULT_INDEX_URL, IndexError, Product, Release, channel_release,
    parse_channels, version_release,
};
pub use deps::{CannotRun, DepsFix, Distro, check_runs, chromium_package_line, missing_libraries};
pub use discover::{Chrome, DiscoverError, Problem, Remedy, Source, discover, locate};
pub use host::{Arch, CACHE_ENV, CHROME_ENV, Host, INDEX_ENV, NO_USERNS_CHECK_ENV, Os};
pub use install::{
    InstallError, Installation, PINNED, Progress, Request, channels, install, quarantined,
    uninstall,
};
pub use installed::{COMPLETE_MARKER, Installed, cache_dir, installed, platform_key};
pub use launch::{LaunchOptions, Launched, ProfileChoice, launch, sandbox_blocked_error};
pub use version::{ChromeVersion, MIN_CHROME_MAJOR};

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Creates an empty executable file (and its parent dirs).
    pub(crate) fn executable(path: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_owned()
    }

    /// A complete install `id` under `cache` with its binary at `binary`.
    pub(crate) fn install(cache: &Path, id: &str, binary: &str) {
        let dir = cache.join("browsers").join(id);
        executable(&dir.join(binary));
        std::fs::write(dir.join(crate::COMPLETE_MARKER), "").unwrap();
    }
}

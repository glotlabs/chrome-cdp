//! Chrome discovery, Chrome for Testing install, launch flags and profiles
//! (plan 02).
//!
//! - [`discover`] finds the Chrome to launch (milestone 2.1).
//! - [`Host`] is what discovery reads from the machine: OS, architecture and
//!   a few environment variables. Tests build a fake one.
//! - [`cache_dir`] and [`installed`] know the `chrome-cdp install` cache
//!   layout, which milestone 2.2 writes.

mod discover;
mod host;
mod installed;
mod version;

pub use discover::{Chrome, DiscoverError, Problem, Source, discover, locate};
pub use host::{Arch, CACHE_ENV, CHROME_ENV, Host, Os};
pub use installed::{COMPLETE_MARKER, Installed, cache_dir, installed, platform_key};
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

//! What discovery reads from the machine, captured once so tests can fake it.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The env var naming the Chrome binary (the `--chrome` flag wins over it).
pub const CHROME_ENV: &str = "CHROME_CDP_CHROME";
/// The env var overriding the cache dir that `chrome-cdp install` fills.
pub const CACHE_ENV: &str = "CHROME_CDP_CACHE";

/// The environment variables discovery reads.
const VARS: &[&str] = &[CHROME_ENV, CACHE_ENV, "XDG_CACHE_HOME", "HOME", "PATH"];

/// An operating system chrome-cdp knows how to find Chrome on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    /// Anything else: only `--chrome`, the env var and `PATH` are searched.
    Other,
}

/// A CPU architecture, as far as Chrome for Testing builds go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

/// The machine as discovery sees it: OS, architecture, a few env vars, and
/// the root that fixed system paths (`/opt/google/chrome/chrome`) live
/// under, which is `/` except in tests.
#[derive(Debug, Clone)]
pub struct Host {
    pub os: Os,
    pub arch: Arch,
    vars: BTreeMap<&'static str, OsString>,
    root: PathBuf,
}

impl Host {
    /// This machine.
    pub fn current() -> Self {
        let os = if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        };
        let arch = if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::Other
        };
        let vars = VARS
            .iter()
            .filter_map(|&name| std::env::var_os(name).map(|v| (name, v)))
            .collect();
        Self {
            os,
            arch,
            vars,
            root: PathBuf::from("/"),
        }
    }

    /// A made-up machine with no env vars whose system paths live under
    /// `root`. For tests.
    pub fn fake(os: Os, arch: Arch, root: &Path) -> Self {
        Self {
            os,
            arch,
            vars: BTreeMap::new(),
            root: root.to_owned(),
        }
    }

    /// Sets one of the env vars discovery reads (`HOME`, `PATH`,
    /// `CHROME_CDP_CHROME`, …). For tests.
    #[must_use]
    pub fn with_var(mut self, name: &str, value: impl Into<OsString>) -> Self {
        let name = VARS
            .iter()
            .find(|&&v| v == name)
            .unwrap_or_else(|| panic!("discovery does not read {name}"));
        self.vars.insert(name, value.into());
        self
    }

    /// An env var; an empty one counts as unset.
    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars
            .get(name)
            .map(OsString::as_os_str)
            .filter(|v| !v.is_empty())
    }

    /// `$HOME`, if it is set and absolute.
    pub fn home(&self) -> Option<PathBuf> {
        self.var("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    }

    /// A fixed system path such as `/opt/google/chrome/chrome`, under the
    /// host's root.
    pub(crate) fn system_path(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// Looks `name` up on `PATH`, like a shell would.
    pub(crate) fn which(&self, name: &str) -> Option<PathBuf> {
        let path = self.var("PATH")?;
        std::env::split_paths(path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|p| is_executable(p))
    }
}

/// A regular file (after symlinks) that someone may execute.
pub(crate) fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

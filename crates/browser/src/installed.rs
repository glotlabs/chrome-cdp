//! The cache that `chrome-cdp install` fills (plan 02, milestone 2.2):
//! `<cache dir>/browsers/<channel>-<version>-<platform>/`, complete once its
//! `INSTALLATION_COMPLETE` marker exists.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::host::{Arch, CACHE_ENV, Host, Os, is_executable};

/// Written last by `install`, so a partial install is never used.
pub const COMPLETE_MARKER: &str = "INSTALLATION_COMPLETE";

/// The cache dir: `$CHROME_CDP_CACHE`, else `$XDG_CACHE_HOME/chrome-cdp` or
/// `~/.cache/chrome-cdp` on Linux and `~/Library/Caches/chrome-cdp` on
/// macOS. `None` without a usable `HOME`.
pub fn cache_dir(host: &Host) -> Option<PathBuf> {
    if let Some(dir) = host.var(CACHE_ENV) {
        return Some(PathBuf::from(dir));
    }
    let home = host.home();
    let base = match host.os {
        Os::MacOs => home?.join("Library/Caches"),
        Os::Linux | Os::Other => host
            .var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".cache")))?,
    };
    Some(base.join("chrome-cdp"))
}

/// The Chrome for Testing platform key for this host, or `None` where there
/// is no build (Linux arm64).
pub(crate) fn platform_key(host: &Host) -> Option<&'static str> {
    match (host.os, host.arch) {
        (Os::Linux, Arch::X86_64) => Some("linux64"),
        (Os::MacOs, Arch::Aarch64) => Some("mac-arm64"),
        (Os::MacOs, Arch::X86_64) => Some("mac-x64"),
        _ => None,
    }
}

/// One complete browser in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The directory name, `<channel>-<version>-<platform>`.
    pub id: String,
    pub channel: String,
    pub version: String,
    pub platform: String,
    /// The binary to launch.
    pub executable: PathBuf,
    /// Whether it is `chrome-headless-shell` (no `--headed`).
    pub headless_shell: bool,
}

/// The complete installs for this host's platform, best first: full Chrome
/// before headless shell, then stable, beta, dev, canary, then the newest
/// version.
pub fn installed(host: &Host) -> Vec<Installed> {
    let (Some(cache), Some(platform)) = (cache_dir(host), platform_key(host)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(cache.join("browsers")) else {
        return Vec::new();
    };
    let mut found: Vec<Installed> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            parse_install(&e.path(), &id, platform)
        })
        .collect();
    found.sort_by(better);
    found
}

/// `stable-154.0.8037.57-linux64` → an [`Installed`], if it is complete,
/// for `platform`, and holds a binary.
fn parse_install(dir: &Path, id: &str, platform: &str) -> Option<Installed> {
    let (channel, rest) = id.split_once('-')?;
    let (version, dir_platform) = rest.split_once('-')?;
    if dir_platform != platform || !dir.join(COMPLETE_MARKER).is_file() {
        return None;
    }
    let (executable, headless_shell) = binaries(platform)
        .into_iter()
        .map(|(rel, shell)| (dir.join(rel), shell))
        .find(|(path, _)| is_executable(path))?;
    Some(Installed {
        id: id.to_owned(),
        channel: channel.to_owned(),
        version: version.to_owned(),
        platform: platform.to_owned(),
        executable,
        headless_shell,
    })
}

/// Where the binary sits inside an install dir (the layout of the Chrome
/// for Testing zips), full Chrome first; `true` marks headless shell.
fn binaries(platform: &str) -> [(String, bool); 2] {
    let chrome = if platform.starts_with("mac") {
        format!(
            "chrome-{platform}/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        )
    } else {
        format!("chrome-{platform}/chrome")
    };
    [
        (chrome, false),
        (
            format!("chrome-headless-shell-{platform}/chrome-headless-shell"),
            true,
        ),
    ]
}

fn better(a: &Installed, b: &Installed) -> Ordering {
    fn channel_rank(channel: &str) -> usize {
        ["stable", "beta", "dev", "canary"]
            .iter()
            .position(|c| *c == channel)
            .unwrap_or(4)
    }
    fn numbers(version: &str) -> Vec<u64> {
        version.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    a.headless_shell
        .cmp(&b.headless_shell)
        .then_with(|| channel_rank(&a.channel).cmp(&channel_rank(&b.channel)))
        .then_with(|| numbers(&b.version).cmp(&numbers(&a.version)))
        .then_with(|| a.id.cmp(&b.id))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{cache_dir, installed, platform_key};
    use crate::host::{Arch, Host, Os};
    use crate::tests::{executable, install};

    #[test]
    fn cache_dir_per_os() {
        let root = Path::new("/");
        let linux = Host::fake(Os::Linux, Arch::X86_64, root).with_var("HOME", "/home/a");
        assert_eq!(
            cache_dir(&linux).unwrap(),
            Path::new("/home/a/.cache/chrome-cdp")
        );
        let xdg = linux.clone().with_var("XDG_CACHE_HOME", "/xdg");
        assert_eq!(cache_dir(&xdg).unwrap(), Path::new("/xdg/chrome-cdp"));
        // A relative XDG_CACHE_HOME is invalid and ignored (XDG spec).
        let relative = linux.clone().with_var("XDG_CACHE_HOME", "xdg");
        assert_eq!(
            cache_dir(&relative).unwrap(),
            Path::new("/home/a/.cache/chrome-cdp")
        );
        let over = xdg.with_var("CHROME_CDP_CACHE", "/c");
        assert_eq!(cache_dir(&over).unwrap(), Path::new("/c"));

        let mac = Host::fake(Os::MacOs, Arch::Aarch64, root)
            .with_var("HOME", "/Users/a")
            .with_var("XDG_CACHE_HOME", "/xdg");
        assert_eq!(
            cache_dir(&mac).unwrap(),
            Path::new("/Users/a/Library/Caches/chrome-cdp")
        );
        assert_eq!(cache_dir(&Host::fake(Os::Linux, Arch::X86_64, root)), None);
    }

    #[test]
    fn platform_keys() {
        let key = |os, arch| platform_key(&Host::fake(os, arch, Path::new("/")));
        assert_eq!(key(Os::Linux, Arch::X86_64), Some("linux64"));
        assert_eq!(key(Os::Linux, Arch::Aarch64), None);
        assert_eq!(key(Os::MacOs, Arch::Aarch64), Some("mac-arm64"));
        assert_eq!(key(Os::MacOs, Arch::X86_64), Some("mac-x64"));
        assert_eq!(key(Os::Other, Arch::X86_64), None);
    }

    #[test]
    fn complete_installs_best_first() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let host =
            Host::fake(Os::Linux, Arch::X86_64, dir.path()).with_var("CHROME_CDP_CACHE", &cache);
        assert!(installed(&host).is_empty());

        install(&cache, "beta-155.0.1.0-linux64", "chrome-linux64/chrome");
        install(&cache, "stable-153.0.9.0-linux64", "chrome-linux64/chrome");
        install(&cache, "stable-154.0.10.0-linux64", "chrome-linux64/chrome");
        install(&cache, "stable-154.0.9.0-linux64", "chrome-linux64/chrome");
        install(
            &cache,
            "stable-160.0.0.0-linux64",
            "chrome-headless-shell-linux64/chrome-headless-shell",
        );
        // Wrong platform, no binary, or not complete: skipped.
        install(
            &cache,
            "stable-160.0.0.0-mac-arm64",
            "chrome-linux64/chrome",
        );
        install(&cache, "stable-161.0.0.0-linux64", "somewhere/else");
        executable(&cache.join("browsers/stable-162.0.0.0-linux64/chrome-linux64/chrome"));
        std::fs::write(cache.join("browsers/stray-file"), "").unwrap();

        let ids: Vec<_> = installed(&host).into_iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            [
                "stable-154.0.10.0-linux64",
                "stable-154.0.9.0-linux64",
                "stable-153.0.9.0-linux64",
                "beta-155.0.1.0-linux64",
                "stable-160.0.0.0-linux64",
            ]
        );
        let best = &installed(&host)[0];
        assert_eq!(best.channel, "stable");
        assert_eq!(best.version, "154.0.10.0");
        assert_eq!(best.platform, "linux64");
        assert!(!best.headless_shell);
        assert!(installed(&host)[4].headless_shell);
    }

    #[test]
    fn mac_installs_are_app_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::fake(Os::MacOs, Arch::Aarch64, dir.path())
            .with_var("CHROME_CDP_CACHE", dir.path());
        install(
            dir.path(),
            "stable-154.0.1.0-mac-arm64",
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        );
        let found = installed(&host);
        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .executable
                .ends_with("MacOS/Google Chrome for Testing")
        );
    }
}

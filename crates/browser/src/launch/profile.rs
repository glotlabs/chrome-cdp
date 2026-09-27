//! Where Chrome keeps its profile (plan 02, milestone 2.3).
//!
//! The default is a temp dir deleted on close; `--profile <name>` is kept
//! under the data dir; `--user-data-dir <path>` is used as given. The
//! user's everyday Chrome profile is refused, and so is a profile another
//! Chrome has open. A snap Chromium gets its profiles under
//! `~/snap/<snap>/common/chrome-cdp`, the only place its confinement lets it
//! write.

use std::path::{Component, Path, PathBuf};

use protocol::{ErrorInfo, ErrorKind, Hint};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::TempDir;

use crate::discover::Chrome;
use crate::host::{Host, Os};
use crate::launch::sys;

/// Which profile to launch with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProfileChoice {
    /// A new temp dir, deleted on close.
    #[default]
    Ephemeral,
    /// `--profile <name>`: kept between runs in the data dir.
    Named(String),
    /// `--user-data-dir <path>`.
    Dir(PathBuf),
}

/// A profile dir ready for launch, and the launch's run dir.
///
/// Every launch gets a private run dir, deleted on close (and by the
/// watchdog if chrome-cdp dies). It holds Chrome's `TMPDIR`, where Chrome
/// keeps its process-singleton socket dir (`org.chromium.Chromium.*`),
/// which would leak into `/tmp` whenever Chrome is killed, and, for a
/// temporary profile, the profile itself.
#[derive(Debug)]
pub(crate) struct Profile {
    pub(crate) dir: PathBuf,
    /// Whether `dir` is inside `run` and goes with it.
    pub(crate) ephemeral: bool,
    /// Deletes the run dir when dropped.
    pub(crate) run: Option<TempDir>,
    pub(crate) warnings: Vec<String>,
}

impl Profile {
    /// Chrome's `TMPDIR`.
    pub(crate) fn tmp_dir(&self) -> Option<PathBuf> {
        self.run.as_ref().map(|r| r.path().join("tmp"))
    }
}

/// chrome-cdp's data dir: `$XDG_DATA_HOME/chrome-cdp` or
/// `~/.local/share/chrome-cdp` on Linux, `~/Library/Application
/// Support/chrome-cdp` on macOS. `None` without a usable `HOME`.
pub fn data_dir(host: &Host) -> Option<PathBuf> {
    let home = host.home();
    let base = match host.os {
        Os::MacOs => home?.join("Library/Application Support"),
        Os::Linux | Os::Other => host
            .var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".local/share")))?,
    };
    Some(base.join("chrome-cdp"))
}

/// The dir for chrome-cdp's own files when Chrome is a snap:
/// `~/snap/<snap>/common/chrome-cdp`.
fn snap_dir(host: &Host, chrome: &Chrome) -> Option<PathBuf> {
    let snap = chrome
        .path
        .strip_prefix(host.system_path("/snap/bin"))
        .ok()
        .and_then(|p| p.to_str())
        .map_or("chromium", |name| name.split('.').next().unwrap_or(name));
    Some(
        host.home()?
            .join("snap")
            .join(snap)
            .join("common/chrome-cdp"),
    )
}

/// Whether `name` is a usable `--profile` name.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Checks a `--profile` name, so a CLI can refuse it before launching.
pub fn check_name(name: &str) -> Result<(), ErrorInfo> {
    if valid_name(name) {
        return Ok(());
    }
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("`{name}` is not a profile name"),
    )
    .hint("use letters, digits, `.`, `_` and `-` (at most 64), e.g. --profile work"))
}

/// The directory of the named profile.
pub fn named_profile_dir(host: &Host, chrome: &Chrome, name: &str) -> Option<PathBuf> {
    let base = if chrome.snap {
        snap_dir(host, chrome)?
    } else {
        data_dir(host)?
    };
    Some(base.join("profiles").join(name))
}

/// The user data dirs of everyday Chrome and Chromium on this OS.
fn real_profiles(host: &Host) -> Vec<PathBuf> {
    let Some(home) = host.home() else {
        return Vec::new();
    };
    let dirs: &[&str] = match host.os {
        Os::MacOs => &[
            "Library/Application Support/Google/Chrome",
            "Library/Application Support/Google/Chrome Beta",
            "Library/Application Support/Google/Chrome Canary",
            "Library/Application Support/Chromium",
        ],
        Os::Linux | Os::Other => &[
            ".config/google-chrome",
            ".config/google-chrome-beta",
            ".config/google-chrome-unstable",
            ".config/chromium",
            "snap/chromium/common/chromium",
        ],
    };
    dirs.iter().map(|d| home.join(d)).collect()
}

/// Makes `path` absolute (against the current dir) and resolves what
/// exists of it, so different spellings compare equal.
fn normalize(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn launch_failed(message: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::LaunchFailed, message)
}

/// Creates (or checks) the profile dir for `choice`.
pub(crate) fn prepare(
    choice: &ProfileChoice,
    host: &Host,
    chrome: &Chrome,
) -> Result<Profile, ErrorInfo> {
    let mut warnings = Vec::new();
    let mut builder = tempfile::Builder::new();
    builder.prefix("chrome-cdp-");
    let run = match chrome.snap.then(|| snap_dir(host, chrome)).flatten() {
        Some(parent) => {
            create_private(&parent)?;
            builder.tempdir_in(&parent)
        }
        None => builder.tempdir(),
    }
    .map_err(|e| launch_failed(format!("cannot create a temporary directory: {e}")))?;
    create_private(&run.path().join("tmp"))?;
    let dir = match choice {
        ProfileChoice::Ephemeral => {
            let dir = run.path().join("profile");
            create_private(&dir)?;
            dir
        }
        ProfileChoice::Named(name) => {
            check_name(name)?;
            let dir = named_profile_dir(host, chrome, name).ok_or_else(|| {
                launch_failed("cannot tell where to keep profiles: HOME is not set")
                    .hint("set HOME, or pass --user-data-dir <path>")
            })?;
            create_private(&dir)?;
            dir
        }
        ProfileChoice::Dir(path) => {
            let dir = normalize(path);
            if let Some(real) = real_profiles(host)
                .iter()
                .find(|real| dir.starts_with(normalize(real)))
            {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("{} is your everyday Chrome profile", dir.display()),
                )
                .note("why", "driving it would expose your logins to every command, and Chrome 136+ refuses remote debugging on it")
                .hint(
                    Hint::text("log in once in a kept profile instead: --profile <name>")
                        .or("pass --user-data-dir with another directory"),
                )
                .details(json!({ "user_data_dir": dir, "real_profile": real })));
            }
            if chrome.snap && !snap_can_write(host, &dir) {
                warnings.push(format!(
                    "{} is a snap, which cannot write {}; use a directory under ~/snap/ or \
                     --profile <name>",
                    chrome.path.display(),
                    dir.display()
                ));
            }
            std::fs::create_dir_all(&dir)
                .map_err(|e| launch_failed(format!("cannot create {}: {e}", dir.display())))?;
            dir
        }
    };
    let ephemeral = *choice == ProfileChoice::Ephemeral;
    if !ephemeral {
        check_not_in_use(&dir, choice)?;
    }
    Ok(Profile {
        dir,
        ephemeral,
        run: Some(run),
        warnings,
    })
}

/// Creates `dir` (and its parents), user-only.
fn create_private(dir: &Path) -> Result<(), ErrorInfo> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| launch_failed(format!("cannot create {}: {e}", dir.display())))
}

/// Whether a snap can write `dir`: not under `/tmp` and no hidden
/// directory under `HOME`.
fn snap_can_write(host: &Host, dir: &Path) -> bool {
    if let Some(rel) = host
        .home()
        .and_then(|h| dir.strip_prefix(h).ok().map(Path::to_owned))
    {
        return !rel
            .components()
            .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')));
    }
    !(dir.starts_with("/tmp") || dir.starts_with("/var/tmp"))
}

/// Fails if another Chrome has the profile open. Chrome keeps a
/// `SingletonLock` symlink to `<host>-<pid>` there while it runs; a second
/// Chrome on the same dir hands its launch to the first and exits.
fn check_not_in_use(dir: &Path, choice: &ProfileChoice) -> Result<(), ErrorInfo> {
    let Ok(target) = std::fs::read_link(dir.join("SingletonLock")) else {
        return Ok(());
    };
    let target = target.to_string_lossy();
    let Some((lock_host, pid)) = target.rsplit_once('-') else {
        return Ok(());
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return Ok(());
    };
    let same_host = sys::hostname().is_none_or(|h| h == lock_host);
    if !same_host || !sys::process_alive(pid) {
        return Ok(());
    }
    let other = match choice {
        ProfileChoice::Named(_) => "use another --profile",
        _ => "use another --user-data-dir",
    };
    Err(launch_failed(format!(
        "the profile {} is in use by another Chrome (pid {pid})",
        dir.display()
    ))
    .hint(Hint::text(format!("close that Chrome (kill {pid})")).or(other))
    .details(json!({ "profile": dir, "pid": pid })))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use protocol::ErrorKind;

    use super::{ProfileChoice, data_dir, named_profile_dir, prepare, valid_name};
    use crate::discover::{Chrome, Source};
    use crate::host::{Arch, Host, Os};

    /// Sessions send the choice and the Chrome to the daemon as JSON.
    #[test]
    fn choice_and_chrome_round_trip_through_json() {
        for choice in [
            ProfileChoice::Ephemeral,
            ProfileChoice::Named("work".into()),
            ProfileChoice::Dir("/p".into()),
        ] {
            let json = serde_json::to_value(&choice).unwrap();
            assert_eq!(
                serde_json::from_value::<ProfileChoice>(json).unwrap(),
                choice
            );
        }
        assert_eq!(
            serde_json::to_value(ProfileChoice::Named("w".into())).unwrap(),
            serde_json::json!({"kind": "named", "value": "w"})
        );
        let mut found = chrome("/c", true);
        found.source = Source::Installed {
            id: "stable-1".into(),
        };
        found.version = crate::ChromeVersion::new("Chromium", "154.0.1.2");
        found.warnings = vec!["dropped".into()];
        let back: Chrome = serde_json::from_value(serde_json::to_value(&found).unwrap()).unwrap();
        found.warnings.clear();
        assert_eq!(back, found);
    }

    fn chrome(path: &str, snap: bool) -> Chrome {
        Chrome {
            path: PathBuf::from(path),
            source: Source::Flag,
            version: None,
            snap,
            headless_shell: false,
            warnings: Vec::new(),
        }
    }

    fn host(root: &Path) -> Host {
        Host::fake(Os::Linux, Arch::X86_64, root).with_var("HOME", root.join("home"))
    }

    #[test]
    fn data_dirs() {
        let root = Path::new("/r");
        assert_eq!(
            data_dir(&host(root)),
            Some(PathBuf::from("/r/home/.local/share/chrome-cdp"))
        );
        let xdg = host(root).with_var("XDG_DATA_HOME", "/x");
        assert_eq!(data_dir(&xdg), Some(PathBuf::from("/x/chrome-cdp")));
        let mac = Host::fake(Os::MacOs, Arch::Aarch64, root).with_var("HOME", "/Users/u");
        assert_eq!(
            data_dir(&mac),
            Some(PathBuf::from(
                "/Users/u/Library/Application Support/chrome-cdp"
            ))
        );
        assert_eq!(data_dir(&Host::fake(Os::Linux, Arch::X86_64, root)), None);
    }

    #[test]
    fn profile_names() {
        for good in ["work", "a", "user-1.test_2"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "-x",
            "a b",
            &"x".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn named_profiles_live_in_the_data_dir_or_the_snap_dir() {
        let root = Path::new("/r");
        let h = host(root);
        assert_eq!(
            named_profile_dir(&h, &chrome("/usr/bin/chromium", false), "work"),
            Some(PathBuf::from(
                "/r/home/.local/share/chrome-cdp/profiles/work"
            ))
        );
        assert_eq!(
            named_profile_dir(&h, &chrome("/r/snap/bin/chromium", true), "work"),
            Some(PathBuf::from(
                "/r/home/snap/chromium/common/chrome-cdp/profiles/work"
            ))
        );
    }

    #[test]
    fn ephemeral_profiles_are_deleted_and_snaps_get_theirs_at_home() {
        let root = tempfile::tempdir().unwrap();
        let h = host(root.path());
        let p = prepare(&ProfileChoice::Ephemeral, &h, &chrome("/c", false)).unwrap();
        let dir = p.dir.clone();
        assert!(dir.is_dir());
        assert!(p.ephemeral);
        let tmp = p.tmp_dir().unwrap();
        assert!(tmp.is_dir());
        assert_eq!(tmp.parent(), dir.parent());
        drop(p);
        assert!(!dir.exists());
        assert!(!tmp.exists());

        let snap = chrome(
            &root.path().join("snap/bin/chromium").to_string_lossy(),
            true,
        );
        let p = prepare(&ProfileChoice::Ephemeral, &h, &snap).unwrap();
        assert!(
            p.dir
                .starts_with(root.path().join("home/snap/chromium/common/chrome-cdp")),
            "{}",
            p.dir.display()
        );
    }

    #[test]
    fn named_profiles_are_created_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let h = host(root.path());
        let p = prepare(
            &ProfileChoice::Named("work".into()),
            &h,
            &chrome("/c", false),
        )
        .unwrap();
        assert!(!p.ephemeral);
        let run = p.tmp_dir().unwrap();
        drop(p);
        assert!(!run.exists());
        let kept = root
            .path()
            .join("home/.local/share/chrome-cdp/profiles/work");
        assert_eq!(kept.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        let err = prepare(
            &ProfileChoice::Named("../x".into()),
            &h,
            &chrome("/c", false),
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
    }

    #[test]
    fn the_real_chrome_profile_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let h = host(root.path());
        let real = root.path().join("home/.config/google-chrome");
        std::fs::create_dir_all(real.join("Default")).unwrap();
        for dir in [
            real.clone(),
            real.join("Default"),
            real.join("../google-chrome"),
        ] {
            let err = prepare(&ProfileChoice::Dir(dir), &h, &chrome("/c", false)).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs);
            assert!(
                err.message.ends_with("is your everyday Chrome profile"),
                "{err}"
            );
        }
        let other = root.path().join("home/profiles/test");
        let p = prepare(&ProfileChoice::Dir(other.clone()), &h, &chrome("/c", false)).unwrap();
        assert_eq!(p.dir, other);
        assert!(other.is_dir());
    }

    #[test]
    fn a_profile_in_use_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let h = host(root.path());
        let dir = root.path().join("p");
        std::fs::create_dir_all(&dir).unwrap();
        let host_name = super::sys::hostname().unwrap();
        std::os::unix::fs::symlink(
            format!("{host_name}-{}", std::process::id()),
            dir.join("SingletonLock"),
        )
        .unwrap();
        let err = prepare(&ProfileChoice::Dir(dir.clone()), &h, &chrome("/c", false)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::LaunchFailed);
        assert!(
            err.message.contains("is in use by another Chrome (pid "),
            "{err}"
        );

        // A stale lock (no such process, or another host) is fine.
        std::fs::remove_file(dir.join("SingletonLock")).unwrap();
        std::os::unix::fs::symlink(format!("{host_name}-999999999"), dir.join("SingletonLock"))
            .unwrap();
        assert!(prepare(&ProfileChoice::Dir(dir.clone()), &h, &chrome("/c", false)).is_ok());
        std::fs::remove_file(dir.join("SingletonLock")).unwrap();
        std::os::unix::fs::symlink(
            format!("elsewhere-{}", std::process::id()),
            dir.join("SingletonLock"),
        )
        .unwrap();
        assert!(prepare(&ProfileChoice::Dir(dir), &h, &chrome("/c", false)).is_ok());
    }

    #[test]
    fn snaps_warn_about_profiles_they_cannot_write() {
        let root = tempfile::tempdir().unwrap();
        let h = host(root.path());
        let snap = chrome("/snap/bin/chromium", true);
        let hidden = root.path().join("home/.hidden/p");
        let p = prepare(&ProfileChoice::Dir(hidden), &h, &snap).unwrap();
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        let fine = root.path().join("home/snap/chromium/common/p");
        let p = prepare(&ProfileChoice::Dir(fine), &h, &snap).unwrap();
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }
}

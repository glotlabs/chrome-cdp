//! Finding the Chrome to launch (plan 02, milestone 2.1).
//!
//! Resolution order, first hit wins:
//!
//! 1. `--chrome <path>`, then `$CHROME_CDP_CHROME`. A bad path here is an
//!    error, never a silent fallback.
//! 2. Browsers installed by `chrome-cdp install` (see [`crate::installed`]).
//! 3. System installs: on Linux `google-chrome-stable`, `google-chrome`,
//!    `chromium`, `chromium-browser` on `PATH`, then
//!    `/opt/google/chrome/chrome` and `/snap/bin/chromium`; on macOS the
//!    Chrome, Chrome Canary and Chromium app bundles in `/Applications` and
//!    `~/Applications`.
//!
//! [`locate`] only looks at the filesystem; [`discover`] also reads the
//! version and collects warnings (too old, version unknown).

use std::path::{Path, PathBuf};

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, format};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::deps::{Distro, chromium_package_line};
use crate::host::{CHROME_ENV, Host, Os, is_executable};
use crate::installed::{cache_dir, installed, platform_key};
use crate::version::{self, ChromeVersion, MIN_CHROME_MAJOR};

/// Linux browser names searched on `PATH`, in order.
const LINUX_PATH_NAMES: &[&str] = &[
    "google-chrome-stable",
    "google-chrome",
    "chromium",
    "chromium-browser",
];

/// Linux locations outside `PATH`, in order.
const LINUX_PATHS: &[&str] = &["/opt/google/chrome/chrome", "/snap/bin/chromium"];

/// macOS app bundle binaries, relative to `/Applications` or
/// `~/Applications`, in order.
const MAC_APPS: &[&str] = &[
    "Google Chrome.app/Contents/MacOS/Google Chrome",
    "Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
    "Chromium.app/Contents/MacOS/Chromium",
];

/// Where the Chrome to launch came from; `status` shows it (milestone 2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// `--chrome`.
    Flag,
    /// `$CHROME_CDP_CHROME`.
    Env,
    /// `chrome-cdp install`'s cache; `id` is the install dir's name.
    Installed { id: String },
    /// Found by name on `PATH`.
    Path { name: String },
    /// A standard system location.
    System,
}

impl Source {
    /// Why this Chrome was picked, e.g. `found google-chrome on PATH`.
    pub fn describe(&self) -> String {
        match self {
            Self::Flag => "given by --chrome".into(),
            Self::Env => format!("given by {CHROME_ENV}"),
            Self::Installed { id } => format!("installed by chrome-cdp install ({id})"),
            Self::Path { name } => format!("found {name} on PATH"),
            Self::System => "found in a standard install location".into(),
        }
    }
}

/// The Chrome to launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chrome {
    /// Absolute path of the binary.
    pub path: PathBuf,
    pub source: Source,
    /// `None` when it could not be read (a warning says why).
    pub version: Option<ChromeVersion>,
    /// A snap package: its confinement blocks profiles under `/tmp` and in
    /// hidden directories, so launch puts them under `~/snap/<snap>/common`.
    pub snap: bool,
    /// `chrome-headless-shell`, which cannot run headed.
    pub headless_shell: bool,
    /// Non-fatal problems for the command's `warnings`.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

impl Chrome {
    fn new(path: PathBuf, source: Source, host: &Host) -> Self {
        let snap = is_snap(&path, host);
        let headless_shell = path
            .file_name()
            .is_some_and(|n| n == "chrome-headless-shell");
        Self {
            path,
            source,
            version: None,
            snap,
            headless_shell,
            warnings: Vec::new(),
        }
    }
}

/// Why `--chrome` or `$CHROME_CDP_CHROME` cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Missing,
    Directory,
    NotExecutable,
}

/// Discovery failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoverError {
    /// The path the user named is not a usable binary.
    #[error("{}", unusable_message(path, from, *problem))]
    Unusable {
        path: PathBuf,
        /// `Flag` or `Env`.
        from: Source,
        problem: Problem,
    },
    /// No Chrome anywhere; `searched` lists where we looked.
    #[error("no Chrome or Chromium found")]
    NotFound {
        searched: Vec<String>,
        remedy: Remedy,
    },
}

/// How to get a Chrome when none is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remedy {
    /// `chrome-cdp install` (Chrome for Testing has a build for this
    /// machine).
    Install,
    /// The distro's Chromium package: its install command, if known.
    Package(Option<&'static str>),
}

fn unusable_message(path: &Path, from: &Source, problem: Problem) -> String {
    let path = path.display();
    let mut message = match problem {
        Problem::Missing => format!("no Chrome at {path}"),
        Problem::Directory => format!("{path} is a directory, not a Chrome binary"),
        Problem::NotExecutable => format!("{path} is not executable"),
    };
    if *from == Source::Env {
        message.push_str(" (from ");
        message.push_str(CHROME_ENV);
        message.push(')');
    }
    message
}

impl DiscoverError {
    /// The user-facing error (plan 08 §3): `ChromeNotFound`, exit code 4.
    pub fn to_error_info(&self) -> ErrorInfo {
        let info = ErrorInfo::new(ErrorKind::ChromeNotFound, self.to_string());
        match self {
            Self::Unusable {
                path,
                problem: Problem::NotExecutable,
                ..
            } => info.hint(format!(
                "make it executable: chmod +x {}",
                format::shell_quote(&path.to_string_lossy())
            )),
            Self::Unusable {
                from: Source::Env, ..
            } => info.hint(format!(
                "point {CHROME_ENV} at a Chrome or Chromium binary, or unset it"
            )),
            Self::Unusable { .. } => {
                info.hint("pass --chrome with the path of a Chrome or Chromium binary")
            }
            Self::NotFound { searched, remedy } => {
                let hint = match remedy {
                    Remedy::Install => Hint::run(Cmd::new("install")),
                    Remedy::Package(Some(line)) => Hint::text(format!("install Chromium: {line}")),
                    Remedy::Package(None) => {
                        Hint::text("install Chromium with the system package manager")
                    }
                }
                .or("pass --chrome /path/to/chrome")
                .or(format!("set {CHROME_ENV}"));
                info.note("searched", searched.join(", "))
                    .hint(hint)
                    .details(json!({ "searched": searched }))
            }
        }
    }
}

/// Finds the Chrome to launch and reads its version. `explicit` is the
/// `--chrome` flag.
pub fn discover(explicit: Option<&Path>, host: &Host) -> Result<Chrome, DiscoverError> {
    let mut chrome = locate(explicit, host)?;
    if chrome.version.is_none() {
        match version::probe(&chrome.path) {
            Ok(v) => chrome.version = Some(v),
            Err(e) => chrome.warnings.push(format!(
                "cannot tell the version of {}: {e}",
                chrome.path.display()
            )),
        }
    }
    if let Some(v) = chrome.version.as_ref().filter(|v| v.is_too_old()) {
        chrome.warnings.push(format!(
            "{v} at {} is older than Chrome {MIN_CHROME_MAJOR}, the oldest chrome-cdp supports; \
             some commands may fail",
            chrome.path.display()
        ));
    }
    Ok(chrome)
}

/// Finds the Chrome to launch without running it. The version is filled in
/// only when the path tells it (installed browsers).
pub fn locate(explicit: Option<&Path>, host: &Host) -> Result<Chrome, DiscoverError> {
    if let Some(path) = explicit {
        return resolve_explicit(path, Source::Flag, host);
    }
    if let Some(path) = host.var(CHROME_ENV) {
        return resolve_explicit(Path::new(path), Source::Env, host);
    }

    if let Some(best) = installed(host).into_iter().next() {
        let mut chrome = Chrome::new(best.executable, Source::Installed { id: best.id }, host);
        chrome.headless_shell = best.headless_shell;
        chrome.version = ChromeVersion::new("Google Chrome for Testing", &best.version);
        return Ok(chrome);
    }

    let mut searched = Vec::new();
    match cache_dir(host) {
        Some(cache) => searched.push(cache.join("browsers").display().to_string()),
        None => searched.push("the chrome-cdp install cache (HOME is not set)".into()),
    }
    match host.os {
        Os::Linux => {
            if let Some(found) = search_path(LINUX_PATH_NAMES, host) {
                return Ok(found);
            }
            searched.push(format!("PATH ({})", LINUX_PATH_NAMES.join(", ")));
            for path in LINUX_PATHS {
                let path = host.system_path(path);
                if is_executable(&path) {
                    return Ok(Chrome::new(path, Source::System, host));
                }
                searched.push(path.display().to_string());
            }
        }
        Os::MacOs => {
            let mut dirs = vec![host.system_path("/Applications")];
            dirs.extend(host.home().map(|h| h.join("Applications")));
            for dir in dirs {
                for app in MAC_APPS {
                    let path = dir.join(app);
                    if is_executable(&path) {
                        return Ok(Chrome::new(path, Source::System, host));
                    }
                }
                searched.push(dir.display().to_string());
            }
        }
        Os::Other => {
            let names = ["chrome", "chromium"];
            if let Some(found) = search_path(&names, host) {
                return Ok(found);
            }
            searched.push(format!("PATH ({})", names.join(", ")));
        }
    }
    let remedy = match platform_key(host) {
        Some(_) => Remedy::Install,
        None => Remedy::Package(chromium_package_line(Distro::detect(host).as_ref())),
    };
    Err(DiscoverError::NotFound { searched, remedy })
}

fn search_path(names: &[&str], host: &Host) -> Option<Chrome> {
    names.iter().find_map(|name| {
        host.which(name).map(|path| {
            Chrome::new(
                path,
                Source::Path {
                    name: (*name).to_owned(),
                },
                host,
            )
        })
    })
}

/// Checks a path the user gave. A bare name that is not a file in the
/// current directory is looked up on `PATH` (`--chrome chromium`), and a
/// macOS `.app` bundle means the binary inside it.
fn resolve_explicit(path: &Path, source: Source, host: &Host) -> Result<Chrome, DiscoverError> {
    let bare = path.components().count() == 1 && !path.exists();
    let path = match path.to_str().filter(|_| bare).and_then(|n| host.which(n)) {
        Some(found) => found,
        None => std::path::absolute(path).unwrap_or_else(|_| path.to_owned()),
    };
    let path = app_bundle_binary(&path).unwrap_or(path);
    let problem = if is_executable(&path) {
        return Ok(Chrome::new(path, source, host));
    } else if path.is_dir() {
        Problem::Directory
    } else if path.is_file() {
        Problem::NotExecutable
    } else {
        Problem::Missing
    };
    Err(DiscoverError::Unusable {
        path,
        from: source,
        problem,
    })
}

/// `/Applications/Google Chrome.app` → `…/Contents/MacOS/Google Chrome`.
fn app_bundle_binary(path: &Path) -> Option<PathBuf> {
    if path.extension()? != "app" || !path.is_dir() {
        return None;
    }
    let binary = path.join("Contents/MacOS").join(path.file_stem()?);
    is_executable(&binary).then_some(binary)
}

/// Whether `path` runs a snap: it lives under `/snap/`, links to
/// `/usr/bin/snap`, or is a wrapper script that execs `/snap/bin/…` (like
/// Ubuntu's `chromium-browser`).
fn is_snap(path: &Path, host: &Host) -> bool {
    if path.starts_with(host.system_path("/snap")) {
        return true;
    }
    if std::fs::canonicalize(path).is_ok_and(|p| p.file_name().is_some_and(|n| n == "snap")) {
        return true;
    }
    // Only small scripts are read; a Chrome binary is ~200 MB.
    let small = path.metadata().is_ok_and(|m| m.len() <= 16 * 1024);
    small
        && std::fs::read(path).is_ok_and(|bytes| {
            bytes.starts_with(b"#!") && String::from_utf8_lossy(&bytes).contains("/snap/bin/")
        })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use protocol::ErrorKind;

    use super::{DiscoverError, Problem, Remedy, Source, discover, locate};
    use crate::host::{Arch, Host, Os};
    use crate::tests::{executable, install};

    /// A Linux host rooted in a temp dir, with HOME and an empty PATH dir.
    fn linux(root: &Path) -> Host {
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        Host::fake(Os::Linux, Arch::X86_64, root)
            .with_var("HOME", root.join("home"))
            .with_var("PATH", root.join("bin"))
    }

    fn source(host: &Host) -> Source {
        locate(None, host).unwrap().source
    }

    #[test]
    fn linux_resolution_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let host = linux(root);
        let err = locate(None, &host).unwrap_err();
        let DiscoverError::NotFound { searched, remedy } = &err else {
            panic!("{err:?}")
        };
        let cache = root.join("home/.cache/chrome-cdp/browsers");
        assert_eq!(
            searched,
            &[
                cache.display().to_string(),
                "PATH (google-chrome-stable, google-chrome, chromium, chromium-browser)".into(),
                root.join("opt/google/chrome/chrome").display().to_string(),
                root.join("snap/bin/chromium").display().to_string(),
            ]
        );
        assert_eq!(remedy, &Remedy::Install);

        // Last resort first, then each better source hides the one before.
        executable(&root.join("snap/bin/chromium"));
        assert_eq!(source(&host), Source::System);
        assert!(locate(None, &host).unwrap().snap);

        executable(&root.join("opt/google/chrome/chrome"));
        let chrome = locate(None, &host).unwrap();
        assert_eq!(chrome.path, root.join("opt/google/chrome/chrome"));
        assert!(!chrome.snap);

        for name in [
            "chromium-browser",
            "chromium",
            "google-chrome",
            "google-chrome-stable",
        ] {
            executable(&root.join("bin").join(name));
            let chrome = locate(None, &host).unwrap();
            assert_eq!(chrome.source, Source::Path { name: name.into() });
            assert_eq!(chrome.path, root.join("bin").join(name));
        }

        install(
            &root.join("home/.cache/chrome-cdp"),
            "stable-154.0.8037.57-linux64",
            "chrome-linux64/chrome",
        );
        let chrome = locate(None, &host).unwrap();
        assert_eq!(
            chrome.source,
            Source::Installed {
                id: "stable-154.0.8037.57-linux64".into()
            }
        );
        assert_eq!(chrome.version.unwrap().major, 154);

        let from_env = executable(&root.join("env-chrome"));
        let host = host.with_var("CHROME_CDP_CHROME", &from_env);
        assert_eq!(source(&host), Source::Env);

        let flag = executable(&root.join("flag-chrome"));
        let chrome = locate(Some(&flag), &host).unwrap();
        assert_eq!((chrome.source, chrome.path), (Source::Flag, flag));
    }

    #[test]
    fn a_bad_explicit_path_is_an_error_not_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        executable(&root.join("bin/chromium"));
        let host = linux(root);
        let missing = root.join("nope/chrome");

        let err = locate(Some(&missing), &host).unwrap_err();
        assert_eq!(
            err,
            DiscoverError::Unusable {
                path: missing.clone(),
                from: Source::Flag,
                problem: Problem::Missing,
            }
        );
        let info = err.to_error_info();
        assert_eq!(info.kind, ErrorKind::ChromeNotFound);
        assert_eq!(
            info.to_text(),
            format!(
                "error: no Chrome at {}\n  hint: pass --chrome with the path of a Chrome or \
                 Chromium binary\n",
                missing.display()
            )
        );

        let host = host.with_var("CHROME_CDP_CHROME", &missing);
        assert_eq!(
            locate(None, &host).unwrap_err().to_error_info().to_text(),
            format!(
                "error: no Chrome at {} (from CHROME_CDP_CHROME)\n  hint: point \
                 CHROME_CDP_CHROME at a Chrome or Chromium binary, or unset it\n",
                missing.display()
            )
        );

        let err = locate(Some(root), &host).unwrap_err();
        assert!(matches!(
            err,
            DiscoverError::Unusable {
                problem: Problem::Directory,
                ..
            }
        ));

        let plain = root.join("plain");
        std::fs::write(&plain, "").unwrap();
        assert_eq!(
            locate(Some(&plain), &host)
                .unwrap_err()
                .to_error_info()
                .to_text(),
            format!(
                "error: {0} is not executable\n  hint: make it executable: chmod +x {0}\n",
                plain.display()
            )
        );
    }

    #[test]
    fn explicit_bare_names_use_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let chromium = executable(&root.join("bin/chromium"));
        let host = linux(root);
        let chrome = locate(Some(Path::new("chromium")), &host).unwrap();
        assert_eq!((chrome.source, chrome.path), (Source::Flag, chromium));
    }

    #[test]
    fn explicit_app_bundles_mean_their_binary() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Chromium.app");
        let binary = executable(&app.join("Contents/MacOS/Chromium"));
        let host = Host::fake(Os::MacOs, Arch::Aarch64, dir.path());
        assert_eq!(locate(Some(&app), &host).unwrap().path, binary);
    }

    #[test]
    fn macos_resolution_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let home = root.join("Users/a");
        let host = Host::fake(Os::MacOs, Arch::Aarch64, root).with_var("HOME", &home);
        let DiscoverError::NotFound { searched, .. } = locate(None, &host).unwrap_err() else {
            panic!()
        };
        assert_eq!(searched.len(), 3, "{searched:?}");

        let user = executable(&home.join("Applications/Chromium.app/Contents/MacOS/Chromium"));
        assert_eq!(locate(None, &host).unwrap().path, user);
        let canary = executable(
            &root.join("Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"),
        );
        assert_eq!(locate(None, &host).unwrap().path, canary);
        let chrome =
            executable(&root.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
        assert_eq!(locate(None, &host).unwrap().path, chrome);
        assert_eq!(source(&host), Source::System);

        install(
            &home.join("Library/Caches/chrome-cdp"),
            "stable-154.0.1.0-mac-arm64",
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        );
        assert!(matches!(source(&host), Source::Installed { .. }));
    }

    #[test]
    fn not_found_error_lists_where_it_looked() {
        let err = DiscoverError::NotFound {
            searched: vec!["/c/browsers".into(), "PATH (chromium)".into()],
            remedy: Remedy::Install,
        };
        assert_eq!(
            err.to_error_info().to_text(),
            "error: no Chrome or Chromium found\n  \
             searched: /c/browsers, PATH (chromium)\n  \
             hint: chrome-cdp install, or pass --chrome /path/to/chrome, or set \
             CHROME_CDP_CHROME\n"
        );
    }

    #[test]
    fn without_a_chrome_for_testing_build_the_hint_is_the_distro_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::write(root.join("etc/os-release"), "ID=debian\nVERSION_ID=12\n").unwrap();
        let host = Host::fake(Os::Linux, Arch::Other, root).with_var("HOME", root);
        let err = locate(None, &host).unwrap_err();
        assert!(
            matches!(
                &err,
                DiscoverError::NotFound {
                    remedy: Remedy::Package(Some("sudo apt-get install -y chromium")),
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(err.to_error_info().to_text().ends_with(
            "hint: install Chromium: sudo apt-get install -y chromium, or pass --chrome \
                 /path/to/chrome, or set CHROME_CDP_CHROME\n"
        ));
    }

    #[test]
    fn snap_wrappers_are_snaps() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let wrapper = root.join("bin/chromium-browser");
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, "#!/bin/sh\nexec /snap/bin/chromium \"$@\"\n").unwrap();
        make_executable(&wrapper);
        let chrome = locate(None, &linux(root)).unwrap();
        assert!(chrome.snap);
    }

    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn script(path: &Path, body: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        make_executable(path);
        path.to_owned()
    }

    #[test]
    fn discover_reads_the_version_and_warns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let host = linux(root);

        let current = script(&root.join("current"), "echo 'Chromium 154.0.1.2 '");
        let chrome = discover(Some(&current), &host).unwrap();
        assert_eq!(chrome.version.unwrap().to_string(), "Chromium 154.0.1.2");
        assert!(chrome.warnings.is_empty(), "{:?}", chrome.warnings);

        let old = script(&root.join("old"), "echo 'Google Chrome 120.0.6099.109'");
        let chrome = discover(Some(&old), &host).unwrap();
        assert_eq!(
            chrome.warnings,
            [format!(
                "Google Chrome 120.0.6099.109 at {} is older than Chrome 150, the oldest \
                 chrome-cdp supports; some commands may fail",
                old.display()
            )]
        );

        let odd = script(&root.join("odd"), "echo hello");
        let chrome = discover(Some(&odd), &host).unwrap();
        assert_eq!(chrome.version, None);
        assert_eq!(
            chrome.warnings,
            [format!(
                "cannot tell the version of {}: `--version` printed `hello`",
                odd.display()
            )]
        );
    }

    #[test]
    fn describes_each_source() {
        assert_eq!(Source::Flag.describe(), "given by --chrome");
        assert_eq!(Source::Env.describe(), "given by CHROME_CDP_CHROME");
        assert_eq!(
            Source::Installed {
                id: "stable-1.0.0.0-linux64".into()
            }
            .describe(),
            "installed by chrome-cdp install (stable-1.0.0.0-linux64)"
        );
        assert_eq!(
            Source::Path {
                name: "chromium".into()
            }
            .describe(),
            "found chromium on PATH"
        );
        assert_eq!(
            Source::System.describe(),
            "found in a standard install location"
        );
    }
}

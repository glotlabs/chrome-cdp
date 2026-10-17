//! Chrome's sandbox (plan 02, milestone 2.3).
//!
//! The sandbox stays on. [`decide`] turns it off only as root or in a
//! container without user namespaces, with a warning. On Linux,
//! [`userns_check`] predicts Ubuntu 23.10+'s AppArmor rule that blocks the
//! sandbox for binaries without a profile granting `userns`, and
//! [`apparmor_profile`] is the profile `install --with-deps` writes for
//! chrome-cdp's own installs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::host::{Host, NO_USERNS_CHECK_ENV, Os};
use crate::installed::cache_dir;
use crate::launch::{apparmor, sys};

/// Whether chrome-cdp runs as root.
pub fn running_as_root() -> bool {
    sys::euid() == 0
}

/// The sysctl behind Ubuntu's rule.
const RESTRICT_SYSCTL: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";

/// Where AppArmor policy lives.
const APPARMOR_DIR: &str = "/etc/apparmor.d";

/// The binaries in a Chrome for Testing install, relative to its dir.
const CFT_BINARIES: &[&str] = &[
    "chrome-linux64/chrome",
    "chrome-linux-arm64/chrome",
    "chrome-headless-shell-linux64/chrome-headless-shell",
    "chrome-headless-shell-linux-arm64/chrome-headless-shell",
];

/// What the machine tells us about sandboxing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SandboxEnv {
    pub(crate) root: bool,
    /// What gave the container away, if we are in one.
    pub(crate) container: Option<&'static str>,
    /// Whether an unprivileged process can create a user namespace;
    /// `None` when we cannot tell.
    pub(crate) userns: Option<bool>,
}

impl SandboxEnv {
    /// This machine. Only probes user namespaces inside a container.
    pub(crate) fn detect(host: &Host) -> Self {
        let root = running_as_root();
        let container = container_marker(host);
        let userns = if container.is_some() && !root && host.os == Os::Linux {
            userns_available(host)
        } else {
            None
        };
        Self {
            root,
            container,
            userns,
        }
    }
}

fn container_marker(host: &Host) -> Option<&'static str> {
    if host.var("container").is_some() {
        Some("$container is set")
    } else if host.system_path("/.dockerenv").exists() {
        Some("/.dockerenv exists")
    } else if host.system_path("/run/.containerenv").exists() {
        Some("/run/.containerenv exists")
    } else {
        None
    }
}

/// Whether user namespaces work: the sysctls allow them and `unshare
/// --user` succeeds (a container's seccomp filter may block it).
fn userns_available(host: &Host) -> Option<bool> {
    let read = |p: &str| {
        std::fs::read_to_string(host.system_path(p))
            .ok()
            .map(|s| s.trim().to_owned())
    };
    if read("/proc/sys/user/max_user_namespaces").as_deref() == Some("0")
        || read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() == Some("0")
    {
        return Some(false);
    }
    let status = Command::new("unshare")
        .args(["--user", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    Some(status.success())
}

/// Why Chrome must run without its sandbox, or `None` to keep it.
pub(crate) fn decide(env: &SandboxEnv) -> Option<String> {
    if env.root {
        return Some("chrome-cdp runs as root, where Chrome's sandbox cannot start".into());
    }
    match (env.container, env.userns) {
        (Some(marker), Some(false)) => Some(format!(
            "this looks like a container ({marker}) that cannot create user namespaces"
        )),
        _ => None,
    }
}

/// Whether Ubuntu's AppArmor rule lets `binary` use the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsernsCheck {
    /// Not Linux, or the rule is off.
    NotRestricted,
    /// The AppArmor profile `name` grants it `userns`.
    Covered { profile: String },
    /// A setuid-root `chrome-sandbox` helper does the job instead.
    SetuidSandbox,
    /// No profile covers it: the sandbox will fail.
    Blocked,
    /// The rule is on but the profiles cannot be read.
    Unknown,
    /// `CHROME_CDP_NO_USERNS_CHECK=1` turned the check off.
    Skipped,
}

/// Checks `binary` against Ubuntu 23.10+'s rule
/// (`kernel.apparmor_restrict_unprivileged_userns=1`).
pub fn userns_check(host: &Host, binary: &Path) -> UsernsCheck {
    if host.os != Os::Linux {
        return UsernsCheck::NotRestricted;
    }
    if host.var(NO_USERNS_CHECK_ENV).is_some_and(|v| v != "0") {
        return UsernsCheck::Skipped;
    }
    let restricted =
        std::fs::read_to_string(host.system_path(RESTRICT_SYSCTL)).is_ok_and(|v| v.trim() == "1");
    if !restricted {
        return UsernsCheck::NotRestricted;
    }
    let binary = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_owned());
    if setuid_sandbox(host, &binary) {
        return UsernsCheck::SetuidSandbox;
    }
    let Ok(entries) = std::fs::read_dir(host.system_path(APPARMOR_DIR)) else {
        return UsernsCheck::Unknown;
    };
    // Canonical, as the binary is: `@{HOME}` matches a home behind a
    // symlink too.
    let home = host.home().map(|h| std::fs::canonicalize(&h).unwrap_or(h));
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(p) = apparmor::parse(&text)
            .into_iter()
            .find(|p| p.userns && p.attaches_to(&binary, home.as_deref()))
        {
            return UsernsCheck::Covered { profile: p.name };
        }
    }
    UsernsCheck::Blocked
}

/// A setuid-root `chrome-sandbox` next to the binary, or at
/// `$CHROME_DEVEL_SANDBOX`.
fn setuid_sandbox(host: &Host, binary: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let candidates = [
        host.var("CHROME_DEVEL_SANDBOX").map(PathBuf::from),
        binary.parent().map(|d| d.join("chrome-sandbox")),
    ];
    candidates.into_iter().flatten().any(|p| {
        p.metadata()
            .is_ok_and(|m| m.uid() == 0 && m.mode() & 0o4000 != 0)
    })
}

/// The AppArmor profile that lets chrome-cdp's installs use the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppArmorProfile {
    /// The profile name, also its file name.
    pub name: String,
    /// `/etc/apparmor.d/<name>`.
    pub path: PathBuf,
    pub text: String,
}

/// The profile for this host's install cache. For the default Linux cache
/// (`~/.cache/chrome-cdp`) it is `chrome-cdp` and covers every user's
/// cache through `@{HOME}`; another cache gets its own
/// `chrome-cdp-<hash>` for its absolute path. A path with a control
/// character gets none: a newline in it would add rules to a profile
/// that is loaded with sudo.
pub fn apparmor_profile(host: &Host) -> Option<AppArmorProfile> {
    let cache = cache_dir(host)?;
    let default = host.home().map(|h| h.join(".cache/chrome-cdp"));
    let (name, base) = if default.as_ref() == Some(&cache) {
        (
            "chrome-cdp".to_owned(),
            "@{HOME}/.cache/chrome-cdp".to_owned(),
        )
    } else {
        let resolved = std::fs::canonicalize(&cache).unwrap_or(cache);
        let base = escape_glob(&resolved.to_string_lossy());
        if base.contains(char::is_control) {
            return None;
        }
        (format!("chrome-cdp-{:08x}", fnv(base.as_bytes())), base)
    };
    let attach = format!("{base}/browsers/*/{{{}}}", CFT_BINARIES.join(","));
    let attach = if attach.contains(char::is_whitespace) {
        format!("\"{attach}\"")
    } else {
        attach
    };
    let mut text = String::new();
    let _ = write!(
        text,
        "# Written by `chrome-cdp install --with-deps`. Lets the browsers that\n\
         # chrome-cdp installs use Chrome's sandbox, which needs user namespaces\n\
         # (Ubuntu 23.10+ allows them only to binaries with a profile like this).\n\
         abi <abi/4.0>,\n\
         include <tunables/global>\n\
         \n\
         profile {name} {attach} flags=(unconfined) {{\n  \
           userns,\n\
         \n  \
           include if exists <local/{name}>\n\
         }}\n"
    );
    Some(AppArmorProfile {
        path: PathBuf::from(APPARMOR_DIR).join(&name),
        name,
        text,
    })
}

/// Escapes AppArmor glob characters in a literal path.
fn escape_glob(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(
            c,
            '*' | '?' | '[' | ']' | '{' | '}' | '\\' | '"' | '@' | '^'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// FNV-1a, for a short stable name.
fn fnv(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |h: u32, &b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Whether `binary` is inside chrome-cdp's install cache.
pub(crate) fn in_cache(host: &Host, binary: &Path) -> bool {
    let Some(browsers) = cache_dir(host).map(|c| c.join("browsers")) else {
        return false;
    };
    let resolved = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    binary.starts_with(&browsers) || resolved(binary).starts_with(resolved(&browsers))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{SandboxEnv, UsernsCheck, apparmor, apparmor_profile, decide, userns_check};
    use crate::host::{Arch, Host, Os};

    #[test]
    fn the_sandbox_is_off_only_as_root_or_in_a_container_without_userns() {
        assert_eq!(decide(&SandboxEnv::default()), None);
        assert!(
            decide(&SandboxEnv {
                root: true,
                ..SandboxEnv::default()
            })
            .is_some()
        );
        let container = |userns| SandboxEnv {
            container: Some("/.dockerenv exists"),
            userns,
            ..SandboxEnv::default()
        };
        assert_eq!(decide(&container(Some(true))), None);
        assert_eq!(decide(&container(None)), None);
        assert_eq!(
            decide(&container(Some(false))).unwrap(),
            "this looks like a container (/.dockerenv exists) that cannot create user namespaces"
        );
    }

    /// A fake root with the sysctl, an apparmor dir and an installed chrome.
    fn machine(restricted: &str, profiles: &[(&str, &str)]) -> (tempfile::TempDir, Host, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let sysctl = root.path().join("proc/sys/kernel");
        std::fs::create_dir_all(&sysctl).unwrap();
        std::fs::write(
            sysctl.join("apparmor_restrict_unprivileged_userns"),
            restricted,
        )
        .unwrap();
        let aa = root.path().join("etc/apparmor.d");
        std::fs::create_dir_all(aa.join("abstractions")).unwrap();
        for (name, text) in profiles {
            std::fs::write(aa.join(name), text).unwrap();
        }
        let home = root.path().join("home/u");
        let binary = home.join(".cache/chrome-cdp/browsers/stable-1-linux64/chrome-linux64/chrome");
        crate::tests::executable(&binary);
        let host = Host::fake(Os::Linux, Arch::X86_64, root.path()).with_var("HOME", &home);
        (root, host, binary)
    }

    #[test]
    fn userns_rule() {
        let (_root, host, binary) = machine("0\n", &[]);
        assert_eq!(userns_check(&host, &binary), UsernsCheck::NotRestricted);

        let (_root, host, binary) = machine("1\n", &[]);
        assert_eq!(userns_check(&host, &binary), UsernsCheck::Blocked);
        let skip = host.clone().with_var("CHROME_CDP_NO_USERNS_CHECK", "1");
        assert_eq!(userns_check(&skip, &binary), UsernsCheck::Skipped);
        let mac = Host::fake(Os::MacOs, Arch::Aarch64, Path::new("/"));
        assert_eq!(userns_check(&mac, &binary), UsernsCheck::NotRestricted);

        // The profile `install --with-deps` writes covers it.
        let (_root, host, binary) = machine("1\n", &[]);
        let profile = apparmor_profile(&host).unwrap();
        assert_eq!(profile.name, "chrome-cdp");
        assert_eq!(profile.path, PathBuf::from("/etc/apparmor.d/chrome-cdp"));
        let aa = host.system_path("/etc/apparmor.d/chrome-cdp");
        std::fs::write(&aa, &profile.text).unwrap();
        assert_eq!(
            userns_check(&host, &binary),
            UsernsCheck::Covered {
                profile: "chrome-cdp".into()
            }
        );

        // Not by a profile for another binary.
        let (_root, host, binary) = machine(
            "1\n",
            &[(
                "chrome",
                "profile chrome /opt/google/chrome/chrome {\n userns,\n}\n",
            )],
        );
        assert_eq!(userns_check(&host, &binary), UsernsCheck::Blocked);
    }

    #[test]
    fn the_profile_covers_every_build_and_custom_caches() {
        let home = Path::new("/home/u");
        let host = Host::fake(Os::Linux, Arch::X86_64, Path::new("/")).with_var("HOME", home);
        let profile = apparmor_profile(&host).unwrap();
        assert!(profile.text.contains(
            "profile chrome-cdp @{HOME}/.cache/chrome-cdp/browsers/*/{chrome-linux64/chrome,\
             chrome-linux-arm64/chrome,chrome-headless-shell-linux64/chrome-headless-shell,\
             chrome-headless-shell-linux-arm64/chrome-headless-shell} flags=(unconfined) {\n  userns,\n"
        ), "{}", profile.text);
        let parsed = apparmor::parse(&profile.text);
        for build in super::CFT_BINARIES {
            let bin = home
                .join(".cache/chrome-cdp/browsers/beta-155.0.1.2-linux-arm64")
                .join(build);
            assert!(parsed[0].attaches_to(&bin, Some(home)), "{build}");
        }

        // A newline in the path would add rules: no profile at all.
        let injected = host
            .clone()
            .with_var("XDG_CACHE_HOME", "/data/x {}\n  capability sys_admin,\n#");
        assert!(apparmor_profile(&injected).is_none());

        // XDG_CACHE_HOME elsewhere: its own profile for that path.
        let custom = host.clone().with_var("XDG_CACHE_HOME", "/data/my cache");
        let profile = apparmor_profile(&custom).unwrap();
        assert!(profile.name.starts_with("chrome-cdp-"), "{}", profile.name);
        assert_eq!(
            profile.path,
            Path::new("/etc/apparmor.d").join(&profile.name)
        );
        assert!(
            profile
                .text
                .contains(" \"/data/my cache/chrome-cdp/browsers/*/{"),
            "{}",
            profile.text
        );
        let parsed = apparmor::parse(&profile.text);
        assert!(parsed[0].attaches_to(
            Path::new("/data/my cache/chrome-cdp/browsers/stable-1-linux64/chrome-linux64/chrome"),
            Some(home)
        ));
        assert!(!parsed[0].attaches_to(
            Path::new("/home/u/.cache/chrome-cdp/browsers/stable-1-linux64/chrome-linux64/chrome"),
            Some(home)
        ));
    }
}

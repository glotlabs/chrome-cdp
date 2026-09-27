//! Linux system libraries Chrome needs (plan 02, milestone 2.2): which ones
//! are missing (`ldd`), and the package manager line that installs them.
//! Also the distro's own Chromium package, for machines that Chrome for
//! Testing has no build for.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::host::{Host, Os};
use crate::version;

/// The distribution, from `/etc/os-release`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Distro {
    /// `ID`, e.g. `ubuntu`.
    pub id: String,
    /// `ID_LIKE`, e.g. `["debian"]`.
    pub like: Vec<String>,
    /// `VERSION_ID`, e.g. `24.04`.
    pub version: Option<String>,
    /// `UBUNTU_CODENAME` (also set by Ubuntu derivatives such as Mint).
    pub ubuntu_codename: Option<String>,
}

/// A package manager family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Debian,
    Fedora,
    Arch,
    Alpine,
    Suse,
}

impl Distro {
    /// Reads `/etc/os-release` (under the host's root).
    pub fn detect(host: &Host) -> Option<Self> {
        let text = std::fs::read_to_string(host.system_path("/etc/os-release"))
            .or_else(|_| std::fs::read_to_string(host.system_path("/usr/lib/os-release")))
            .ok()?;
        Some(Self::parse(&text))
    }

    /// Parses os-release's `KEY=value` lines (values may be quoted).
    pub fn parse(text: &str) -> Self {
        let mut distro = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches(['"', '\'']).to_owned();
            match key {
                "ID" => distro.id = value.to_ascii_lowercase(),
                "ID_LIKE" => {
                    distro.like = value
                        .split_whitespace()
                        .map(str::to_ascii_lowercase)
                        .collect();
                }
                "VERSION_ID" => distro.version = Some(value),
                "UBUNTU_CODENAME" => distro.ubuntu_codename = Some(value),
                _ => {}
            }
        }
        distro
    }

    fn is(&self, name: &str) -> bool {
        self.id == name || self.like.iter().any(|l| l == name)
    }

    fn family(&self) -> Option<Family> {
        if self.is("debian") || self.is("ubuntu") {
            Some(Family::Debian)
        } else if self.is("fedora") || self.is("rhel") || self.is("centos") {
            Some(Family::Fedora)
        } else if self.is("arch") {
            Some(Family::Arch)
        } else if self.is("alpine") {
            Some(Family::Alpine)
        } else if self.is("suse") || self.id.starts_with("opensuse") {
            Some(Family::Suse)
        } else {
            None
        }
    }

    /// Whether apt names some libraries with a `t64` suffix (the 64-bit
    /// `time_t` transition): Ubuntu 24.04 and Debian 13 onwards.
    fn uses_t64(&self) -> bool {
        const OLD_UBUNTU: &[&str] = &["bionic", "focal", "jammy", "kinetic", "lunar", "mantic"];
        let major = |v: &Option<String>| {
            v.as_deref()
                .and_then(|v| v.split('.').next())
                .and_then(|m| m.parse::<u32>().ok())
        };
        if let Some(codename) = &self.ubuntu_codename {
            return !OLD_UBUNTU.contains(&codename.as_str());
        }
        match self.id.as_str() {
            "ubuntu" => major(&self.version).is_some_and(|m| m >= 24),
            "debian" => major(&self.version).is_none_or(|m| m >= 13),
            _ => false,
        }
    }
}

/// One shared library Chrome links: its soname, and the package providing
/// it on Debian/Ubuntu (before and after the t64 rename) and on Fedora.
struct Library {
    soname: &'static str,
    apt: &'static str,
    apt_t64: Option<&'static str>,
    dnf: &'static str,
}

const fn lib(soname: &'static str, apt: &'static str, dnf: &'static str) -> Library {
    Library {
        soname,
        apt,
        apt_t64: None,
        dnf,
    }
}

const fn lib_t64(
    soname: &'static str,
    apt: &'static str,
    t64: &'static str,
    dnf: &'static str,
) -> Library {
    Library {
        soname,
        apt,
        apt_t64: Some(t64),
        dnf,
    }
}

/// The libraries Chrome and chrome-headless-shell link that minimal
/// installs (containers, cloud images) often lack.
const LIBRARIES: &[Library] = &[
    lib("libnss3.so", "libnss3", "nss"),
    lib("libnssutil3.so", "libnss3", "nss-util"),
    lib("libsmime3.so", "libnss3", "nss"),
    lib("libnspr4.so", "libnspr4", "nspr"),
    lib_t64("libatk-1.0.so.0", "libatk1.0-0", "libatk1.0-0t64", "atk"),
    lib_t64(
        "libatk-bridge-2.0.so.0",
        "libatk-bridge2.0-0",
        "libatk-bridge2.0-0t64",
        "at-spi2-atk",
    ),
    lib_t64(
        "libatspi.so.0",
        "libatspi2.0-0",
        "libatspi2.0-0t64",
        "at-spi2-core",
    ),
    lib_t64("libcups.so.2", "libcups2", "libcups2t64", "cups-libs"),
    lib_t64("libasound.so.2", "libasound2", "libasound2t64", "alsa-lib"),
    lib_t64(
        "libglib-2.0.so.0",
        "libglib2.0-0",
        "libglib2.0-0t64",
        "glib2",
    ),
    lib_t64(
        "libgobject-2.0.so.0",
        "libglib2.0-0",
        "libglib2.0-0t64",
        "glib2",
    ),
    lib_t64(
        "libgio-2.0.so.0",
        "libglib2.0-0",
        "libglib2.0-0t64",
        "glib2",
    ),
    lib_t64("libgtk-3.so.0", "libgtk-3-0", "libgtk-3-0t64", "gtk3"),
    lib("libdbus-1.so.3", "libdbus-1-3", "dbus-libs"),
    lib("libdrm.so.2", "libdrm2", "libdrm"),
    lib("libgbm.so.1", "libgbm1", "mesa-libgbm"),
    lib("libexpat.so.1", "libexpat1", "expat"),
    lib("libudev.so.1", "libudev1", "systemd-libs"),
    lib("libxkbcommon.so.0", "libxkbcommon0", "libxkbcommon"),
    lib("libpango-1.0.so.0", "libpango-1.0-0", "pango"),
    lib("libcairo.so.2", "libcairo2", "cairo"),
    lib("libX11.so.6", "libx11-6", "libX11"),
    lib("libXext.so.6", "libxext6", "libXext"),
    lib("libxcb.so.1", "libxcb1", "libxcb"),
    lib("libXcomposite.so.1", "libxcomposite1", "libXcomposite"),
    lib("libXdamage.so.1", "libxdamage1", "libXdamage"),
    lib("libXfixes.so.3", "libxfixes3", "libXfixes"),
    lib("libXrandr.so.2", "libxrandr2", "libXrandr"),
    lib("libxshmfence.so.1", "libxshmfence1", "libxshmfence"),
];

/// What installs a set of missing libraries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepsFix {
    /// The missing sonames, as `ldd` printed them.
    pub missing: Vec<String>,
    /// The packages that provide them, when the distro is known.
    pub packages: Vec<String>,
    /// Missing libraries with no known package.
    pub unknown: Vec<String>,
    /// The install command without `sudo`, e.g. `apt-get install -y …`;
    /// `None` for an unknown distro or when no package is known.
    pub command: Option<Vec<String>>,
}

impl DepsFix {
    /// Maps missing sonames to the distro's packages.
    pub fn new(distro: Option<&Distro>, missing: Vec<String>) -> Self {
        let family = distro.and_then(Distro::family);
        let t64 = distro.is_some_and(Distro::uses_t64);
        let mut packages = BTreeSet::new();
        let mut unknown = Vec::new();
        for soname in &missing {
            let package =
                LIBRARIES
                    .iter()
                    .find(|l| l.soname == soname)
                    .and_then(|l| match family {
                        Some(Family::Debian) => Some(l.apt_t64.filter(|_| t64).unwrap_or(l.apt)),
                        Some(Family::Fedora) => Some(l.dnf),
                        _ => None,
                    });
            match package {
                Some(p) => {
                    packages.insert(p.to_owned());
                }
                None => unknown.push(soname.clone()),
            }
        }
        let packages: Vec<String> = packages.into_iter().collect();
        let command = match family {
            _ if packages.is_empty() => None,
            Some(Family::Debian) => Some(words(&["apt-get", "install", "-y"], &packages)),
            Some(Family::Fedora) => Some(words(&["dnf", "install", "-y"], &packages)),
            _ => None,
        };
        Self {
            missing,
            packages,
            unknown,
            command,
        }
    }

    /// The command as one shell line with `sudo`, for hints.
    pub fn sudo_line(&self) -> Option<String> {
        self.command
            .as_ref()
            .map(|c| format!("sudo {}", c.join(" ")))
    }
}

fn words(head: &[&str], tail: &[String]) -> Vec<String> {
    head.iter()
        .map(|w| (*w).to_owned())
        .chain(tail.iter().cloned())
        .collect()
}

/// Why a Chrome binary cannot start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CannotRun {
    /// What `--version` said went wrong.
    pub reason: String,
    /// The missing libraries and how to install them, if `ldd` found any.
    pub fix: Option<DepsFix>,
}

/// Checks on Linux that `binary` starts (`--version`), and if not, which
/// shared libraries it lacks. Other systems are not checked.
pub fn check_runs(host: &Host, binary: &Path) -> Result<(), CannotRun> {
    if host.os != Os::Linux {
        return Ok(());
    }
    let Err(reason) = version::probe(binary) else {
        return Ok(());
    };
    let missing = missing_libraries(binary).unwrap_or_else(|e| {
        tracing::debug!("{e}");
        Vec::new()
    });
    let fix = (!missing.is_empty()).then(|| DepsFix::new(Distro::detect(host).as_ref(), missing));
    Err(CannotRun { reason, fix })
}

/// The shared libraries of `binary` that the dynamic loader cannot find,
/// according to `ldd`.
pub fn missing_libraries(binary: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("ldd")
        .arg(binary)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run ldd: {e}"))?;
    Ok(parse_ldd(&String::from_utf8_lossy(&out.stdout)))
}

/// `\tlibnss3.so => not found` lines → `libnss3.so`.
pub(crate) fn parse_ldd(output: &str) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for name in output
        .lines()
        .filter_map(|l| l.trim().strip_suffix("=> not found"))
        .map(str::trim)
    {
        if !missing.iter().any(|m| m == name) {
            missing.push(name.to_owned());
        }
    }
    missing
}

/// The command that installs the distro's own Chromium, for machines with
/// no Chrome for Testing build.
pub fn chromium_package_line(distro: Option<&Distro>) -> Option<&'static str> {
    let distro = distro?;
    // Ubuntu's chromium package is a snap.
    if distro.id == "ubuntu" || (distro.ubuntu_codename.is_some() && distro.id != "debian") {
        return Some("sudo snap install chromium");
    }
    Some(match distro.family()? {
        Family::Debian => "sudo apt-get install -y chromium",
        Family::Fedora => "sudo dnf install -y chromium",
        Family::Arch => "sudo pacman -S --noconfirm chromium",
        Family::Alpine => "sudo apk add chromium",
        Family::Suse => "sudo zypper install -y chromium",
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DepsFix, Distro, check_runs, chromium_package_line, parse_ldd};
    use crate::host::{Arch, Host, Os};

    const UBUNTU_2404: &str = r#"PRETTY_NAME="Ubuntu 24.04.1 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
ID=ubuntu
ID_LIKE=debian
UBUNTU_CODENAME=noble
"#;

    #[test]
    fn parses_os_release() {
        let d = Distro::parse(UBUNTU_2404);
        assert_eq!(d.id, "ubuntu");
        assert_eq!(d.like, ["debian"]);
        assert_eq!(d.version.as_deref(), Some("24.04"));
        assert_eq!(d.ubuntu_codename.as_deref(), Some("noble"));
        let fedora = Distro::parse("ID=fedora\nVERSION_ID=41\n");
        assert_eq!(fedora.id, "fedora");
        assert!(fedora.like.is_empty());
    }

    #[test]
    fn parses_ldd_output() {
        let out = "\tlinux-vdso.so.1 (0x00007ffd)\n\
                   \tlibnss3.so => not found\n\
                   \tlibdl.so.2 => /lib/x86_64-linux-gnu/libdl.so.2 (0x00007f)\n\
                   \tlibasound.so.2 => not found\n\
                   \tlibnss3.so => not found\n";
        assert_eq!(parse_ldd(out), ["libnss3.so", "libasound.so.2"]);
        assert!(parse_ldd("\tstatically linked\n").is_empty());
    }

    fn missing() -> Vec<String> {
        [
            "libnss3.so",
            "libnssutil3.so",
            "libasound.so.2",
            "libweird.so.9",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn apt_lines_follow_the_t64_rename() {
        let noble = Distro::parse(UBUNTU_2404);
        let fix = DepsFix::new(Some(&noble), missing());
        assert_eq!(fix.packages, ["libasound2t64", "libnss3"]);
        assert_eq!(fix.unknown, ["libweird.so.9"]);
        assert_eq!(
            fix.sudo_line().unwrap(),
            "sudo apt-get install -y libasound2t64 libnss3"
        );

        let jammy = Distro::parse(
            "ID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"22.04\"\nUBUNTU_CODENAME=jammy\n",
        );
        let fix = DepsFix::new(Some(&jammy), missing());
        assert_eq!(
            fix.sudo_line().unwrap(),
            "sudo apt-get install -y libasound2 libnss3"
        );
        let bookworm = Distro::parse("ID=debian\nVERSION_ID=\"12\"\n");
        assert_eq!(
            DepsFix::new(Some(&bookworm), missing()).packages,
            ["libasound2", "libnss3"]
        );
        let trixie = Distro::parse("ID=debian\nVERSION_ID=\"13\"\n");
        assert_eq!(
            DepsFix::new(Some(&trixie), missing()).packages,
            ["libasound2t64", "libnss3"]
        );
        // Mint and Pop!_OS carry Ubuntu's codename.
        let mint = Distro::parse(
            "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_ID=\"22\"\nUBUNTU_CODENAME=noble\n",
        );
        assert_eq!(
            DepsFix::new(Some(&mint), missing()).packages,
            ["libasound2t64", "libnss3"]
        );
    }

    #[test]
    fn dnf_and_unknown_distros() {
        let fedora = Distro::parse("ID=fedora\nVERSION_ID=41\n");
        let fix = DepsFix::new(Some(&fedora), missing());
        assert_eq!(
            fix.sudo_line().unwrap(),
            "sudo dnf install -y alsa-lib nss nss-util"
        );
        let rocky = Distro::parse("ID=rocky\nID_LIKE=\"rhel centos fedora\"\n");
        assert!(DepsFix::new(Some(&rocky), missing()).command.is_some());

        let arch = Distro::parse("ID=arch\n");
        let fix = DepsFix::new(Some(&arch), missing());
        assert_eq!(fix.command, None);
        assert_eq!(fix.unknown.len(), 4);
        assert_eq!(DepsFix::new(None, missing()).command, None);
        assert_eq!(
            DepsFix::new(Some(&fedora), vec!["libweird.so.9".into()]).command,
            None
        );
    }

    #[test]
    fn chromium_packages() {
        let line = |text: &str| chromium_package_line(Some(&Distro::parse(text)));
        assert_eq!(line(UBUNTU_2404), Some("sudo snap install chromium"));
        assert_eq!(
            line("ID=debian\nVERSION_ID=12\n"),
            Some("sudo apt-get install -y chromium")
        );
        assert_eq!(line("ID=fedora\n"), Some("sudo dnf install -y chromium"));
        assert_eq!(line("ID=alpine\n"), Some("sudo apk add chromium"));
        assert_eq!(
            line("ID=manjaro\nID_LIKE=arch\n"),
            Some("sudo pacman -S --noconfirm chromium")
        );
        assert_eq!(
            line("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Some("sudo zypper install -y chromium")
        );
        assert_eq!(line("ID=gentoo\n"), None);
        assert_eq!(chromium_package_line(None), None);
    }

    #[test]
    fn checks_that_the_binary_starts() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::fake(Os::Linux, Arch::X86_64, dir.path());
        let good = dir.path().join("good");
        std::fs::write(&good, "#!/bin/sh\necho 'Chromium 154.0.1.2'\n").unwrap();
        let bad = dir.path().join("bad");
        std::fs::write(
            &bad,
            "#!/bin/sh\necho 'error while loading shared libraries' >&2\n",
        )
        .unwrap();
        for path in [&good, &bad] {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(check_runs(&host, &good), Ok(()));
        let err = check_runs(&host, &bad).unwrap_err();
        assert_eq!(err.reason, "`--version` printed nothing");
        // A script has no ELF dependencies to report.
        assert_eq!(err.fix, None);
        let mac = Host::fake(Os::MacOs, Arch::Aarch64, dir.path());
        assert_eq!(check_runs(&mac, Path::new("/nonexistent")), Ok(()));
    }
}

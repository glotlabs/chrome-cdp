//! `chrome-cdp upgrade` (plan 10, milestone 10.5): replaces this binary
//! with a release from GitHub.
//!
//! A release has a `manifest.json` (`cargo xtask release manifest`) naming
//! each target's archive and its SHA-256. `upgrade` reads the newest one
//! (`releases/latest/download/manifest.json`, which needs no API call and
//! so no rate limit) or `--version`'s, downloads this machine's archive,
//! checks its hash, unpacks `chrome-cdp-<target>/chrome-cdp` next to this
//! binary, checks that it runs and reports the expected version, and
//! renames it over this binary. Nothing is replaced unless every step
//! succeeded.
//!
//! Binaries a package manager owns (Homebrew, npm, cargo) are left to it:
//! replacing them would confuse its records.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use protocol::schema::UpgradeData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::json;

use crate::cli::UpgradeArgs;
use crate::version::VERSION;

/// The env var replacing [`DEFAULT_RELEASES`] (a mirror with the same
/// layout, or a test server).
pub(crate) const RELEASES_ENV: &str = "CHROME_CDP_RELEASES_URL";
/// Where releases are published.
pub(crate) const DEFAULT_RELEASES: &str = "https://github.com/glotlabs/chrome-cdp/releases";
/// The one-line installer, for hints.
pub(crate) const INSTALL_SH: &str =
    "curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh | sh";

/// A release's `manifest.json`.
#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    targets: BTreeMap<String, Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    archive: String,
    sha256: String,
}

/// Runs `upgrade`.
pub(crate) fn run(args: &UpgradeArgs, timeout: Duration) -> Result<Output, ErrorInfo> {
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| failed(format!("cannot tell where this binary is: {e}")))?;
    let target = release_target().ok_or_else(|| {
        failed(format!(
            "chrome-cdp has no release build for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let owner = package_manager(&exe, &home_vars());
    if let (Some(owner), false) = (&owner, args.check) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "{} was installed by {}; upgrade it with {} instead",
                exe.display(),
                owner.name,
                owner.name
            ),
        )
        .hint(Hint::text(owner.command))
        .details(json!({ "path": exe.display().to_string(), "installed_by": owner.name })));
    }

    let base = std::env::var(RELEASES_ENV).unwrap_or_else(|_| DEFAULT_RELEASES.to_owned());
    let base = base.trim_end_matches('/');
    let wanted = args
        .version
        .as_deref()
        .map(|v| v.trim_start_matches('v').to_owned());
    if let Some(v) = &wanted
        && parse_version(v).is_none()
    {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("`{v}` is not a version: use X.Y.Z, e.g. 1.2.0"),
        )
        .hint(Hint::run(Cmd::new("upgrade"))));
    }
    let manifest_url = match &wanted {
        Some(v) => format!("{base}/download/v{v}/manifest.json"),
        None => format!("{base}/latest/download/manifest.json"),
    };
    let http = Http::new(timeout)?;
    let runtime = runtime()?;
    let manifest: Manifest = runtime
        .block_on(http.get(&manifest_url))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|e| failed(format!("{manifest_url} is not a release manifest: {e}")))
        })?;
    let entry = manifest.targets.get(target).ok_or_else(|| {
        failed(format!(
            "release {} has no build for {target}",
            manifest.version
        ))
    })?;
    let archive_url = format!(
        "{base}/download/v{}/{}",
        manifest.version.trim_start_matches('v'),
        entry.archive
    );
    let newer = compare_versions(&manifest.version, VERSION) == Ordering::Greater;
    let data = |upgraded| UpgradeData {
        from: VERSION.into(),
        to: manifest.version.clone(),
        newer,
        upgraded,
        path: exe.display().to_string(),
        archive: archive_url.clone(),
    };

    let same = manifest.version == VERSION;
    if args.check || (same && !args.force) || (wanted.is_none() && !newer && !args.force) {
        let text = if newer {
            let run = owner.as_ref().map_or("chrome-cdp upgrade", |o| o.command);
            format!(
                "chrome-cdp {} is out; this is {VERSION} ({})\n  run: {run}",
                manifest.version,
                exe.display()
            )
        } else if same {
            format!("chrome-cdp {VERSION} is the newest release; nothing to do")
        } else {
            format!(
                "chrome-cdp {VERSION} is newer than the newest release, {}; nothing to do",
                manifest.version
            )
        };
        return Ok(Output::typed(&data(false), text).quiet(manifest.version.clone()));
    }

    let archive = runtime.block_on(http.get(&archive_url))?;
    let actual = sha256_hex(&archive);
    if !actual.eq_ignore_ascii_case(&entry.sha256) {
        return Err(failed(format!(
            "the checksum of {} does not match its manifest: expected {}, got {actual}",
            entry.archive, entry.sha256
        ))
        .hint(Hint::run(Cmd::new("upgrade")).why("to download it again"))
        .details(json!({ "archive": archive_url, "expected": entry.sha256, "actual": actual })));
    }
    let binary = unpack(&archive, &format!("chrome-cdp-{target}/chrome-cdp"))
        .map_err(|e| failed(format!("cannot unpack {}: {e}", entry.archive)))?;
    replace(&exe, &binary, &manifest.version)?;

    let mut output = Output::typed(
        &data(true),
        format!(
            "upgraded chrome-cdp {VERSION} to {} ({})",
            manifest.version,
            exe.display()
        ),
    )
    .quiet(manifest.version.clone());
    let sessions: Vec<String> = daemon::client::live_sessions(&daemon::paths::Env::current())
        .into_iter()
        .map(|p| p.name)
        .collect();
    if !sessions.is_empty() {
        output = output.warning(format!(
            "session{} {} still run{} {VERSION}; the next command restarts {} with {}",
            if sessions.len() == 1 { "" } else { "s" },
            sessions.join(", "),
            if sessions.len() == 1 { "s" } else { "" },
            if sessions.len() == 1 { "it" } else { "them" },
            manifest.version
        ));
    }
    Ok(output)
}

fn failed(message: String) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::InstallFailed, message)
}

fn runtime() -> Result<tokio::runtime::Runtime, ErrorInfo> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failed(format!("cannot start the runtime: {e}")))
}

/// This binary's release target, as the archives name it. Linux builds
/// take the static musl archive, which runs on any distribution.
pub(crate) fn release_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// A package manager that owns a binary, and how to upgrade with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PackageManager {
    pub(crate) name: &'static str,
    pub(crate) command: &'static str,
}

/// `HOME` and `CARGO_HOME`, for [`package_manager`].
#[derive(Debug, Default)]
pub(crate) struct HomeVars {
    pub(crate) home: Option<PathBuf>,
    pub(crate) cargo_home: Option<PathBuf>,
}

pub(crate) fn home_vars() -> HomeVars {
    HomeVars {
        home: std::env::var_os("HOME").map(PathBuf::from),
        cargo_home: std::env::var_os("CARGO_HOME").map(PathBuf::from),
    }
}

/// Who installed the binary at `exe` (a canonical path), if not
/// install.sh or a hand-copied archive.
pub(crate) fn package_manager(exe: &Path, vars: &HomeVars) -> Option<PackageManager> {
    let text = exe.to_string_lossy();
    if text.contains("/Cellar/") || text.contains("/homebrew/") || text.contains("/linuxbrew/") {
        return Some(PackageManager {
            name: "Homebrew",
            command: "brew upgrade chrome-cdp",
        });
    }
    if exe.components().any(|c| c.as_os_str() == "node_modules") {
        return Some(PackageManager {
            name: "npm",
            command: "npm install chrome-cdp@latest   (add -g for a global install)",
        });
    }
    let cargo_bin = vars
        .cargo_home
        .clone()
        .or_else(|| vars.home.as_ref().map(|h| h.join(".cargo")))
        .map(|c| c.join("bin"));
    if cargo_bin.is_some_and(|bin| exe.parent() == Some(bin.as_path())) {
        return Some(PackageManager {
            name: "cargo",
            command: "cargo binstall chrome-cdp   (or cargo install chrome-cdp)",
        });
    }
    None
}

/// `1.2.3` or `1.2.3-beta.1` → (major, minor, patch, pre-release).
fn parse_version(v: &str) -> Option<(u64, u64, u64, Option<&str>)> {
    let v = v.trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    let mut parts = core.split('.').map(str::parse::<u64>);
    let version = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        pre,
    );
    parts.next().is_none().then_some(version)
}

/// Semver order, roughly: a pre-release sorts before its release, and
/// pre-releases compare as text. Unparsable versions are equal.
pub(crate) fn compare_versions(a: &str, b: &str) -> Ordering {
    let (Some(a), Some(b)) = (parse_version(a), parse_version(b)) else {
        return Ordering::Equal;
    };
    (a.0, a.1, a.2)
        .cmp(&(b.0, b.1, b.2))
        .then(match (a.3, b.3) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => x.cmp(y),
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The file at `path` in a `.tar.gz`.
fn unpack(archive: &[u8], path: &str) -> Result<Vec<u8>, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let name = entry.path().map_err(|e| e.to_string())?;
        if name.to_str().map(|n| n.trim_start_matches("./")) == Some(path) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            return Ok(bytes);
        }
    }
    Err(format!("it holds no {path}"))
}

/// Writes `binary` next to `exe`, checks that it runs and says it is
/// `version`, then renames it over `exe`.
fn replace(exe: &Path, binary: &[u8], version: &str) -> Result<(), ErrorInfo> {
    let dir = exe.parent().unwrap_or(Path::new("/"));
    let staged = dir.join(format!(".chrome-cdp.upgrade-{}", std::process::id()));
    let cannot_write = |e: std::io::Error| {
        failed(format!("cannot write to {}: {e}", dir.display())).hint(
            Hint::text(format!(
                "run it as a user who can write to {}",
                dir.display()
            ))
            .or(format!("reinstall: {INSTALL_SH}")),
        )
    };
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&staged)
        .and_then(|mut f| f.write_all(binary).and_then(|()| f.sync_all()));
    if let Err(e) = written {
        let _ = fs::remove_file(&staged);
        return Err(cannot_write(e));
    }
    let ran = Command::new(&staged).args(["--version", "-q"]).output();
    let reported = ran
        .as_ref()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    if reported.as_deref() != Some(version) {
        let _ = fs::remove_file(&staged);
        let why = match (&ran, reported) {
            (Err(e), _) => e.to_string(),
            (Ok(_), Some(other)) => format!("it says it is {other}"),
            (Ok(o), None) => format!(
                "{}: {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        };
        return Err(failed(format!(
            "the downloaded chrome-cdp {version} does not run here ({why}); this binary is unchanged"
        ))
        .hint(Hint::text(format!("reinstall: {INSTALL_SH}"))));
    }
    fs::rename(&staged, exe).map_err(|e| {
        let _ = fs::remove_file(&staged);
        cannot_write(e)
    })
}

/// A small HTTPS client (rustls, as `install` uses).
struct Http {
    client: reqwest::Client,
    timeout: Duration,
}

impl Http {
    fn new(timeout: Duration) -> Result<Self, ErrorInfo> {
        // reqwest is built without a default provider (no aws-lc).
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .user_agent(concat!("chrome-cdp/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| failed(format!("cannot start the HTTP client: {e}")))?;
        Ok(Self { client, timeout })
    }

    async fn get(&self, url: &str) -> Result<Vec<u8>, ErrorInfo> {
        let error = |e: &reqwest::Error| {
            if e.is_timeout() {
                failed(format!(
                    "{url} sent nothing for {}",
                    protocol::format::duration(self.timeout)
                ))
                .hint(Hint::run(Cmd::new("upgrade").flag("timeout", "2m")))
            } else {
                failed(format!("cannot download {url}: {}", chain(e)))
                    .hint(Hint::text("check the network, then run it again"))
            }
        };
        let response = self.client.get(url).send().await.map_err(|e| error(&e))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(failed(format!("{url} does not exist (HTTP 404)")).hint(
                Hint::text(format!("see the releases at {DEFAULT_RELEASES}"))
                    .or("chrome-cdp upgrade --version X.Y.Z with a version listed there"),
            ));
        }
        if !status.is_success() {
            return Err(failed(format!("{url} answered HTTP {}", status.as_u16())));
        }
        Ok(response.bytes().await.map_err(|e| error(&e))?.to_vec())
    }
}

/// reqwest's error and its causes, without its "for url (…)".
fn chain(e: &dyn std::error::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        source = s.source();
    }
    parts
        .iter()
        .map(|p| p.find(" for url (").map_or(p.as_str(), |at| &p[..at]))
        .collect::<Vec<_>>()
        .join(": ")
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::path::{Path, PathBuf};

    use super::{HomeVars, compare_versions, package_manager, parse_version, sha256_hex, unpack};

    #[test]
    fn compares_versions() {
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("v2.0.0", "1.99.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-beta.1", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0-rc.1"), Ordering::Greater);
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-beta.1"),
            Ordering::Greater
        );
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn leaves_package_manager_installs_alone() {
        let vars = HomeVars {
            home: Some(PathBuf::from("/home/ada")),
            cargo_home: None,
        };
        let who = |p: &str| package_manager(Path::new(p), &vars).map(|m| m.name);
        assert_eq!(
            who("/opt/homebrew/Cellar/chrome-cdp/1.0.0/bin/chrome-cdp"),
            Some("Homebrew")
        );
        assert_eq!(
            who("/home/linuxbrew/.linuxbrew/Cellar/chrome-cdp/1.0.0/bin/chrome-cdp"),
            Some("Homebrew")
        );
        assert_eq!(
            who("/home/ada/app/node_modules/chrome-cdp-linux-x64/bin/chrome-cdp"),
            Some("npm")
        );
        assert_eq!(who("/home/ada/.cargo/bin/chrome-cdp"), Some("cargo"));
        assert_eq!(who("/home/ada/.local/bin/chrome-cdp"), None);
        assert_eq!(who("/usr/local/bin/chrome-cdp"), None);
        let custom = HomeVars {
            home: Some(PathBuf::from("/home/ada")),
            cargo_home: Some(PathBuf::from("/opt/cargo")),
        };
        assert_eq!(
            package_manager(Path::new("/opt/cargo/bin/chrome-cdp"), &custom).map(|m| m.name),
            Some("cargo")
        );
    }

    #[test]
    fn hashes_with_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn unpacks_one_file_from_a_tar_gz() {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, body) in [
            ("chrome-cdp-t/README.md", &b"readme"[..]),
            ("chrome-cdp-t/chrome-cdp", &b"binary"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, body).unwrap();
        }
        let archive = builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(
            unpack(&archive, "chrome-cdp-t/chrome-cdp").unwrap(),
            b"binary"
        );
        assert_eq!(
            unpack(&archive, "chrome-cdp-x/chrome-cdp").unwrap_err(),
            "it holds no chrome-cdp-x/chrome-cdp"
        );
        assert!(unpack(b"not gzip", "x").is_err());
    }
}

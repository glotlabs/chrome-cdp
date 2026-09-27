//! Reading a Chrome binary's version without launching a browser.

use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The oldest Chrome major version chrome-cdp supports: current stable − 4
/// (overview §3). The vendored CDP protocol is Chrome 154's (r1687809);
/// raise this when the protocol pin moves.
pub const MIN_CHROME_MAJOR: u32 = 150;

/// How long `chrome --version` may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A Chrome build's name and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromeVersion {
    /// E.g. `Google Chrome for Testing` or `Chromium`.
    pub product: String,
    /// E.g. `154.0.8037.57`.
    pub version: String,
    /// E.g. `154`.
    pub major: u32,
}

impl ChromeVersion {
    /// Builds one from a dotted version string such as `154.0.8037.57`.
    pub fn new(product: impl Into<String>, version: &str) -> Option<Self> {
        let major = version.split('.').next()?.parse().ok()?;
        let dotted = version.contains('.')
            && version
                .split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        dotted.then(|| Self {
            product: product.into(),
            version: version.to_owned(),
            major,
        })
    }

    /// Parses `chrome --version` output: `Google Chrome 154.0.8037.57`,
    /// `Chromium 150.0.7000.1 snap`, `Chromium 151.0.1.2 built on Debian …`.
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().find(|l| !l.trim().is_empty())?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|w| Self::new("", w).is_some())?;
        let product = if at == 0 {
            "Chrome".to_owned()
        } else {
            words[..at].join(" ")
        };
        Self::new(product, words[at])
    }

    /// Whether this is older than [`MIN_CHROME_MAJOR`].
    pub fn is_too_old(&self) -> bool {
        self.major < MIN_CHROME_MAJOR
    }
}

impl std::fmt::Display for ChromeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.product, self.version)
    }
}

/// Reads the version of the Chrome at `path`: from the app bundle's
/// `Info.plist` on macOS (no process needed), else from `--version`.
pub(crate) fn probe(path: &Path) -> Result<ChromeVersion, String> {
    if let Some(version) = app_bundle_version(path) {
        return Ok(version);
    }
    let output = run_version(path)?;
    ChromeVersion::parse(&output).ok_or_else(|| {
        let first = output.lines().next().unwrap_or_default().trim();
        if first.is_empty() {
            "`--version` printed nothing".to_owned()
        } else {
            format!("`--version` printed `{first}`")
        }
    })
}

/// `…/Foo.app/Contents/MacOS/Foo` → the version in
/// `…/Foo.app/Contents/Info.plist`.
fn app_bundle_version(path: &Path) -> Option<ChromeVersion> {
    let contents = path
        .parent()
        .filter(|p| p.ends_with("Contents/MacOS"))?
        .parent()?;
    let plist = std::fs::read_to_string(contents.join("Info.plist")).ok()?;
    let product = plist_string(&plist, "CFBundleName").unwrap_or("Chrome");
    ChromeVersion::new(product, plist_string(&plist, "CFBundleShortVersionString")?)
}

/// The `<string>` after `<key>{key}</key>` in an XML property list.
fn plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let rest = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let len = rest[start..].find("</string>")?;
    Some(rest[start..start + len].trim())
}

/// Runs `path --version` and returns its stdout, giving up after
/// [`PROBE_TIMEOUT`].
fn run_version(path: &Path) -> Result<String, String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run it: {e}"))?;
    // Read on a thread: a helper process may keep stdout open after the
    // binary itself exits.
    let (tx, rx) = mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        thread::spawn(move || {
            let mut out = String::new();
            let _ = stdout.read_to_string(&mut out);
            let _ = tx.send(out);
        });
    }
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`--version` did not finish within {}s",
                    PROBE_TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(format!("cannot run it: {e}")),
        }
    }
    Ok(rx
        .recv_timeout(Duration::from_millis(500))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{ChromeVersion, MIN_CHROME_MAJOR, app_bundle_version};

    #[test]
    fn parses_version_output() {
        let v = ChromeVersion::parse("Google Chrome for Testing 154.0.8037.57 \n").unwrap();
        assert_eq!(v.product, "Google Chrome for Testing");
        assert_eq!(v.version, "154.0.8037.57");
        assert_eq!(v.major, 154);
        assert_eq!(v.to_string(), "Google Chrome for Testing 154.0.8037.57");

        let v = ChromeVersion::parse("Chromium 150.0.7000.1 snap").unwrap();
        assert_eq!((v.product.as_str(), v.major), ("Chromium", 150));
        let v = ChromeVersion::parse(
            "Chromium 119.0.6045.159 built on Debian 12.2, running on Debian 12.2",
        )
        .unwrap();
        assert_eq!(v.version, "119.0.6045.159");
        assert!(v.is_too_old());
        let v = ChromeVersion::parse("\n120.0.1.2").unwrap();
        assert_eq!(v.product, "Chrome");

        for bad in ["", "Chromium", "Chromium 1.x.2", "Chromium 154", "sh: nope"] {
            assert_eq!(ChromeVersion::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn minimum_is_four_majors_behind_the_protocol_pin() {
        // The vendored protocol is Chrome 154's (see MIN_CHROME_MAJOR).
        assert_eq!(cdp::domains::PROTOCOL_REVISION, 1_687_809);
        assert_eq!(MIN_CHROME_MAJOR, 154 - 4);
        assert!(
            !ChromeVersion::new("Chromium", "150.0.0.0")
                .unwrap()
                .is_too_old()
        );
        assert!(
            ChromeVersion::new("Chromium", "149.9.9.9")
                .unwrap()
                .is_too_old()
        );
    }

    #[test]
    fn reads_the_app_bundle_plist() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Google Chrome.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            "<plist><dict>\n<key>CFBundleName</key>\n<string>Google Chrome</string>\n\
             <key>CFBundleShortVersionString</key>\n\t<string>153.0.7999.3</string>\n</dict></plist>",
        )
        .unwrap();
        let v = app_bundle_version(&contents.join("MacOS/Google Chrome")).unwrap();
        assert_eq!(v.to_string(), "Google Chrome 153.0.7999.3");
        assert_eq!(app_bundle_version(&dir.path().join("chrome")), None);
    }
}

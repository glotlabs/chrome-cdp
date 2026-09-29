//! `chrome-cdp upgrade` (plan 10.5) against releases served by a local
//! HTTP server. Each test runs a hard link of the binary in its own dir,
//! so replacing it leaves the build alone; the "new" binaries in the
//! archives are scripts that answer `--version -q`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use browser::testing::{FakeCft, Response};
use common::{cli, code, json_out, run, stderr, stdout};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        (_, "aarch64") => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    }
}

fn archive_name() -> String {
    format!("chrome-cdp-{}.tar.gz", target())
}

/// A `.tar.gz` holding `chrome-cdp-<target>/chrome-cdp` with `body`.
fn archive(body: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(
            &mut header,
            format!("chrome-cdp-{}/chrome-cdp", target()),
            body.as_bytes(),
        )
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// A "binary" that says it is `version`.
fn script(version: &str) -> String {
    format!("#!/bin/sh\necho {version}\n")
}

/// Publishes release `version` at `path` (`latest/download` or
/// `download/v<version>`), its archive holding `body`, and its manifest
/// claiming `sha` (the archive's own hash if `None`).
fn publish(server: &FakeCft, dir: &str, version: &str, body: &str, sha: Option<&str>) {
    let bytes = archive(body);
    let sum = sha.map_or_else(|| sha256(&bytes), str::to_owned);
    let manifest = json!({
        "version": version,
        "targets": { target(): { "archive": archive_name(), "sha256": sum } }
    });
    server.route(
        &format!("/{dir}/manifest.json"),
        Response::ok(manifest.to_string()),
    );
    server.route(
        &format!("/download/v{version}/{}", archive_name()),
        Response::ok(bytes),
    );
}

/// A hard link of the binary in a fresh dir under `target/tmp` (the same
/// file system as the build).
struct Installed {
    dir: tempfile::TempDir,
}

impl Installed {
    fn new(sub: &str) -> Self {
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let bin = dir.path().join(sub);
        fs::create_dir_all(&bin).unwrap();
        fs::hard_link(env!("CARGO_BIN_EXE_chrome-cdp"), bin.join("chrome-cdp")).unwrap();
        let installed = Self { dir };
        fs::create_dir_all(installed.dir.path().join("run")).unwrap();
        installed
    }

    fn exe(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub).join("chrome-cdp")
    }

    /// `cli()`'s environment and working dir, but this binary.
    fn upgrade(&self, sub: &str, server: &FakeCft, args: &[&str]) -> Output {
        let template = cli();
        let mut cmd = std::process::Command::new(self.exe(sub));
        for (key, value) in template.get_envs() {
            match value {
                Some(v) => cmd.env(key, v),
                None => cmd.env_remove(key),
            };
        }
        cmd.current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::null())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("CHROME_CDP_RELEASES_URL", &server.url)
            .arg("upgrade")
            .args(args);
        run(&mut cmd)
    }

    fn files(&self, sub: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir.path().join(sub))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn is_the_build(path: &Path) -> bool {
    fs::metadata(path).unwrap().len()
        == fs::metadata(env!("CARGO_BIN_EXE_chrome-cdp"))
            .unwrap()
            .len()
}

#[test]
fn check_reports_a_newer_release_without_installing_it() {
    let server = FakeCft::start("linux64");
    publish(
        &server,
        "latest/download",
        "99.0.0",
        &script("99.0.0"),
        None,
    );
    let installed = Installed::new("bin");

    let out = installed.upgrade("bin", &server, &["--check"]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        format!(
            "chrome-cdp 99.0.0 is out; this is {VERSION} ({})\n  run: chrome-cdp upgrade\n",
            fs::canonicalize(installed.exe("bin")).unwrap().display()
        )
    );
    let out = installed.upgrade("bin", &server, &["--check", "--json"]);
    let data = &json_out(&out)["data"];
    assert_eq!(data["from"], VERSION);
    assert_eq!(data["to"], "99.0.0");
    assert_eq!(data["newer"], true);
    assert_eq!(data["upgraded"], false);
    assert_eq!(
        data["archive"],
        format!("{}/download/v99.0.0/{}", server.url, archive_name())
    );
    assert!(is_the_build(&installed.exe("bin")));
    assert!(
        !server.requests().iter().any(|r| r.ends_with(".tar.gz")),
        "--check downloads no archive"
    );
}

#[test]
fn nothing_to_do_on_the_newest_release() {
    let server = FakeCft::start("linux64");
    publish(&server, "latest/download", VERSION, &script(VERSION), None);
    let installed = Installed::new("bin");
    let out = installed.upgrade("bin", &server, &[]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        format!("chrome-cdp {VERSION} is the newest release; nothing to do\n")
    );
    assert!(is_the_build(&installed.exe("bin")));
}

#[test]
fn replaces_the_binary_with_a_newer_release() {
    let server = FakeCft::start("linux64");
    publish(
        &server,
        "latest/download",
        "99.0.0",
        &script("99.0.0"),
        None,
    );
    let installed = Installed::new("bin");
    let out = installed.upgrade("bin", &server, &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "upgraded chrome-cdp {VERSION} to 99.0.0 ({})\n",
            fs::canonicalize(installed.exe("bin")).unwrap().display()
        )
    );
    assert_eq!(
        fs::read_to_string(installed.exe("bin")).unwrap(),
        script("99.0.0")
    );
    assert_eq!(
        installed.files("bin"),
        ["chrome-cdp"],
        "no staged file left"
    );
    assert!(
        is_the_build(Path::new(env!("CARGO_BIN_EXE_chrome-cdp"))),
        "the build itself is untouched"
    );
}

#[test]
fn installs_a_named_version_even_an_older_one() {
    let server = FakeCft::start("linux64");
    publish(&server, "download/v0.0.1", "0.0.1", &script("0.0.1"), None);
    let installed = Installed::new("bin");
    let out = installed.upgrade("bin", &server, &["--version", "latest"]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "error: `latest` is not a version: use X.Y.Z, e.g. 1.2.0\n  hint: chrome-cdp upgrade\n"
    );
    let out = installed.upgrade("bin", &server, &["--version", "v0.0.1", "--json"]);
    assert_eq!(code(&out), 0);
    let data = &json_out(&out)["data"];
    assert_eq!(data["to"], "0.0.1");
    assert_eq!(data["newer"], false);
    assert_eq!(data["upgraded"], true);
    assert_eq!(
        fs::read_to_string(installed.exe("bin")).unwrap(),
        script("0.0.1")
    );
}

#[test]
fn keeps_the_binary_when_the_checksum_differs() {
    let server = FakeCft::start("linux64");
    let wrong = "0".repeat(64);
    publish(
        &server,
        "latest/download",
        "99.0.0",
        &script("99.0.0"),
        Some(&wrong),
    );
    let installed = Installed::new("bin");
    let out = installed.upgrade("bin", &server, &["--json"]);
    assert_eq!(code(&out), 4);
    let error = &json_out(&out)["error"];
    assert_eq!(error["kind"], "InstallFailed");
    assert!(
        error["message"].as_str().unwrap().starts_with(&format!(
            "the checksum of {} does not match its manifest: expected {wrong}, got ",
            archive_name()
        )),
        "{error}"
    );
    assert_eq!(error["hint"], "chrome-cdp upgrade   (to download it again)");
    assert!(is_the_build(&installed.exe("bin")));
    assert_eq!(installed.files("bin"), ["chrome-cdp"]);
}

#[test]
fn keeps_the_binary_when_the_new_one_does_not_run() {
    let server = FakeCft::start("linux64");
    publish(
        &server,
        "latest/download",
        "99.0.0",
        &script("98.0.0"),
        None,
    );
    let installed = Installed::new("bin");
    let out = installed.upgrade("bin", &server, &[]);
    assert_eq!(code(&out), 4);
    assert!(
        stderr(&out).starts_with(
            "error: the downloaded chrome-cdp 99.0.0 does not run here (it says it is 98.0.0); \
             this binary is unchanged\n"
        ),
        "{}",
        stderr(&out)
    );
    assert!(is_the_build(&installed.exe("bin")));
    assert_eq!(
        installed.files("bin"),
        ["chrome-cdp"],
        "no staged file left"
    );
}

#[test]
fn a_missing_release_names_the_releases_page() {
    let server = FakeCft::start("linux64");
    let installed = Installed::new("bin");
    let out = installed.upgrade("bin", &server, &["--version", "1.2.3", "--json"]);
    assert_eq!(code(&out), 4);
    let error = &json_out(&out)["error"];
    assert_eq!(
        error["message"],
        format!(
            "{}/download/v1.2.3/manifest.json does not exist (HTTP 404)",
            server.url
        )
    );
    assert_eq!(
        error["hint"],
        "see the releases at https://github.com/glotlabs/chrome-cdp/releases, or chrome-cdp \
         upgrade --version X.Y.Z with a version listed there"
    );
}

#[test]
fn leaves_an_npm_install_to_npm() {
    let server = FakeCft::start("linux64");
    publish(
        &server,
        "latest/download",
        "99.0.0",
        &script("99.0.0"),
        None,
    );
    let sub = "node_modules/chrome-cdp-linux-x64/bin";
    let installed = Installed::new(sub);
    let out = installed.upgrade(sub, &server, &["--json"]);
    assert_eq!(code(&out), 2);
    let error = &json_out(&out)["error"];
    assert_eq!(error["kind"], "BadArgs");
    assert_eq!(
        error["hint"],
        "npm install chrome-cdp@latest   (add -g for a global install)"
    );
    assert!(is_the_build(&installed.exe(sub)));

    // --check still says what is out, with npm's command.
    let out = installed.upgrade(sub, &server, &["--check"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out)
            .ends_with("\n  run: npm install chrome-cdp@latest   (add -g for a global install)\n"),
        "{}",
        stdout(&out)
    );
}

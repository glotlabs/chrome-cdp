//! The install channels of plan 10.5 that live in the repo: `install.sh`
//! (the curl installer) and the npm launcher (`npm/chrome-cdp`). Both run
//! against a release laid out on disk, with this build's binary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// This machine's release target, as install.sh names it.
fn target() -> String {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    if cfg!(target_os = "macos") {
        format!("{arch}-apple-darwin")
    } else {
        format!("{arch}-unknown-linux-musl")
    }
}

fn run(cmd: &mut Command) -> Output {
    let out = cmd.output().expect("run");
    eprintln!(
        "{:?}: {}\nstdout: {}\nstderr: {}",
        cmd,
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn sha256(path: &Path) -> String {
    let out = if cfg!(target_os = "macos") {
        run(Command::new("shasum").args(["-a", "256"]).arg(path))
    } else {
        run(Command::new("sha256sum").arg(path))
    };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

/// Writes `chrome-cdp-<target>.tar.gz` and its `.sha256` into `dir`, as
/// `cargo xtask release archive` does, with this build's binary.
fn release_into(dir: &Path) {
    let target = target();
    let staging = tempfile::tempdir().unwrap();
    let top = staging.path().join(format!("chrome-cdp-{target}"));
    fs::create_dir_all(&top).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_chrome-cdp"), top.join("chrome-cdp")).unwrap();
    fs::create_dir_all(dir).unwrap();
    let archive = format!("chrome-cdp-{target}.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(dir.join(&archive))
        .arg("-C")
        .arg(staging.path())
        .arg(format!("chrome-cdp-{target}"))
        .status()
        .unwrap();
    assert!(status.success());
    let sum = sha256(&dir.join(&archive));
    fs::write(
        dir.join(format!("{archive}.sha256")),
        format!("{sum}  {archive}\n"),
    )
    .unwrap();
}

fn install_sh(releases: &Path, args: &[&str]) -> Output {
    run(Command::new("sh")
        .arg(repo().join("install.sh"))
        .args(args)
        .env(
            "CHROME_CDP_RELEASES_URL",
            format!("file://{}", releases.display()),
        )
        .env_remove("CHROME_CDP_VERSION")
        .env_remove("CHROME_CDP_INSTALL_DIR"))
}

#[test]
fn install_sh_installs_the_newest_or_a_named_release() {
    let tmp = tempfile::tempdir().unwrap();
    let releases = tmp.path().join("releases");
    release_into(&releases.join("latest/download"));
    release_into(&releases.join("download/v0.1.0"));

    let bin = tmp.path().join("bin");
    let out = install_sh(&releases, &["--dir", bin.to_str().unwrap()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/latest/download/chrome-cdp-"), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "installed chrome-cdp {} (git",
            env!("CARGO_PKG_VERSION")
        )),
        "{stdout}"
    );
    assert!(stdout.contains("is not on your PATH"), "{stdout}");
    let version = run(Command::new(bin.join("chrome-cdp")).args(["--version", "-q"]));
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        env!("CARGO_PKG_VERSION")
    );

    let pinned = tmp.path().join("pinned");
    let out = install_sh(
        &releases,
        &["--version", "v0.1.0", "--dir", pinned.to_str().unwrap()],
    );
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("/download/v0.1.0/chrome-cdp-"));
    assert!(pinned.join("chrome-cdp").is_file());
    assert_eq!(
        fs::read_dir(&pinned).unwrap().count(),
        1,
        "no temporary file is left"
    );
}

#[test]
fn install_sh_refuses_a_download_whose_checksum_differs() {
    let tmp = tempfile::tempdir().unwrap();
    let releases = tmp.path().join("releases");
    let latest = releases.join("latest/download");
    release_into(&latest);
    let archive = latest.join(format!("chrome-cdp-{}.tar.gz", target()));
    let mut bytes = fs::read(&archive).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&archive, bytes).unwrap();

    let bin = tmp.path().join("bin");
    let out = install_sh(&releases, &["--dir", bin.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!(
            "error: the checksum of chrome-cdp-{}.tar.gz does not match: expected ",
            target()
        )),
        "{stderr}"
    );
    assert!(!bin.exists(), "nothing is installed");

    let out = install_sh(&releases, &["--version"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "error: --version needs a value\n"
    );
    let missing = install_sh(
        &releases,
        &["--version", "9.9.9", "--dir", bin.to_str().unwrap()],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("error: cannot download "));
}

/// Node's name for this platform's package: `chrome-cdp-<os>-<cpu>`.
fn npm_platform_package() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let cpu = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    format!("chrome-cdp-{os}-{cpu}")
}

fn has_node() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn npm_launcher_runs_the_platform_binary() {
    if !has_node() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let modules = tmp.path().join("node_modules");
    let launcher = modules.join("chrome-cdp/bin/chrome-cdp.js");
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    fs::copy(repo().join("npm/chrome-cdp/bin/chrome-cdp.js"), &launcher).unwrap();

    // Without the platform package: a hint, and exit code 4.
    let out = run(Command::new("node").arg(&launcher).arg("--version"));
    assert_eq!(out.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr,
        format!(
            "error: {} is not installed, so chrome-cdp has no binary\n  hint: reinstall without \
             --no-optional / --omit=optional, e.g. npm install chrome-cdp\n",
            npm_platform_package()
        )
    );

    let bin = modules.join(npm_platform_package()).join("bin/chrome-cdp");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_chrome-cdp"), &bin).unwrap();
    let out = run(Command::new("node")
        .arg(&launcher)
        .args(["--version", "-q"]));
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        env!("CARGO_PKG_VERSION")
    );
    // Exit codes pass through (2: a usage error).
    let out = run(Command::new("node").arg(&launcher).arg("no-such-command"));
    assert_eq!(out.status.code(), Some(2));
}

//! Embeds the git commit in the binary for `chrome-cdp --version`.
//!
//! `CHROME_CDP_GIT_SHA` at build time overrides it (for builds from a source
//! tarball). A crates.io build (`cargo install chrome-cdp`) reads the commit
//! `cargo package` recorded in `.cargo_vcs_info.json`; outside a git checkout
//! it is `unknown`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-env-changed=CHROME_CDP_GIT_SHA");
    let sha = std::env::var("CHROME_CDP_GIT_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(packaged_sha)
        .or_else(git_sha)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo::rustc-env=CHROME_CDP_GIT_SHA={sha}");
}

/// The commit in `.cargo_vcs_info.json`, which only a packaged crate has.
fn packaged_sha() -> Option<String> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let info = std::fs::read_to_string(PathBuf::from(dir).join(".cargo_vcs_info.json")).ok()?;
    let rest = &info[info.find("\"sha1\"")? + 6..];
    let start = rest.find('"')? + 1;
    let sha: String = rest[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    (sha.len() >= 12).then(|| sha[..12].to_owned())
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim().to_owned()).filter(|s| !s.is_empty())
}

fn git_sha() -> Option<String> {
    let sha = git(&["rev-parse", "--short=12", "HEAD"])?;
    // Rebuild when HEAD moves: HEAD itself changes on checkout, the branch ref
    // (loose or packed, in the common dir for worktrees) on commit. Only
    // existing paths are watched; a missing one would force a rerun per build.
    let git_dir = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from);
    let common_dir = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .map(PathBuf::from)
        .or_else(|| git_dir.clone());
    let mut watch = Vec::new();
    if let Some(dir) = &git_dir {
        watch.push(dir.join("HEAD"));
    }
    if let Some(dir) = &common_dir {
        watch.push(dir.join("packed-refs"));
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch.push(dir.join(branch));
        }
    }
    for path in watch.into_iter().filter(|p| p.exists()) {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    Some(sha)
}

//! `cargo xtask release …`: the release artifacts of plan 10, milestone
//! 10.5, made from the binaries `cargo xtask dist --target T` leaves in
//! `dist/`.
//!
//! - `archive --target T`: `dist/chrome-cdp-T.tar.gz` (a `chrome-cdp-T/`
//!   dir with the binary, README, CHANGELOG, NOTICE and any LICENSE file)
//!   and its `.sha256`;
//! - `manifest`: `dist/SHA256SUMS` and `dist/manifest.json`, which
//!   `chrome-cdp upgrade` reads, from the archives there;
//! - `notes [--tag vX.Y.Z]`: the GitHub release notes: the CHANGELOG
//!   section, then the commits since the previous tag grouped by their
//!   conventional-commit type;
//! - `formula`: the Homebrew formula for the tap;
//! - `npm [--out DIR]`: the npm wrapper package and one package per
//!   platform (`npm/README.md`);
//! - `check [--tag vX.Y.Z]`: the versions agree (workspace, internal
//!   dependencies, the tag) and the CHANGELOG has the tag's section.
//!
//! Archive names carry no version, so `releases/latest/download/…` URLs
//! (install.sh) always name the newest build.
//!
//! Like the rest of xtask this is std-only: `tar` and `sha256sum` (or
//! macOS's `shasum`) do the packing and hashing.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{Result, output, warn, workspace_root};

/// The release targets: (Rust triple, npm `os`, npm `cpu`, Homebrew
/// `on_*` blocks).
pub(crate) const TARGETS: [Target; 4] = [
    Target {
        triple: "x86_64-unknown-linux-musl",
        os: "linux",
        cpu: "x64",
    },
    Target {
        triple: "aarch64-unknown-linux-musl",
        os: "linux",
        cpu: "arm64",
    },
    Target {
        triple: "x86_64-apple-darwin",
        os: "darwin",
        cpu: "x64",
    },
    Target {
        triple: "aarch64-apple-darwin",
        os: "darwin",
        cpu: "arm64",
    },
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Target {
    pub(crate) triple: &'static str,
    /// Node's `process.platform`.
    pub(crate) os: &'static str,
    /// Node's `process.arch`.
    pub(crate) cpu: &'static str,
}

impl Target {
    fn find(triple: &str) -> Result<Self> {
        TARGETS
            .into_iter()
            .find(|t| t.triple == triple)
            .ok_or_else(|| {
                let known: Vec<&str> = TARGETS.iter().map(|t| t.triple).collect();
                format!("{triple} is not a release target ({})", known.join(", "))
            })
    }

    fn archive(self) -> String {
        format!("chrome-cdp-{}.tar.gz", self.triple)
    }

    /// The npm package holding this target's binary.
    fn npm_package(self) -> String {
        format!("chrome-cdp-{}-{}", self.os, self.cpu)
    }
}

/// The GitHub repository releases are published in.
pub(crate) const REPO: &str = "glotlabs/chrome-cdp";

/// Files next to the binary in each archive, when they exist.
const EXTRA_FILES: [&str; 6] = [
    "README.md",
    "CHANGELOG.md",
    "NOTICE",
    "LICENSE",
    "LICENSE-MIT",
    "LICENSE-APACHE",
];

pub(crate) const USAGE: &str = "\
usage: cargo xtask release <step> [args]

steps:
  archive --target T   pack dist/chrome-cdp-T into dist/chrome-cdp-T.tar.gz + .sha256
  manifest             write dist/SHA256SUMS and dist/manifest.json from the archives
  notes [--tag vX.Y.Z] print the GitHub release notes: the CHANGELOG section and
                       the commits since the previous tag, grouped by type
  formula              print the Homebrew formula for the archives in dist/
  npm [--out DIR]      write the npm packages (default target/npm) from dist/
  check [--tag vX.Y.Z] check that the versions agree and CHANGELOG.md has the
                       tag's section";

pub(crate) fn run(args: &[String]) -> Result {
    let (step, rest) = args.split_first().ok_or_else(|| USAGE.to_owned())?;
    let root = workspace_root();
    let dist = root.join("dist");
    match (step.as_str(), rest) {
        ("archive", [flag, triple]) if flag == "--target" => {
            archive(&root, &dist, Target::find(triple)?)
        }
        ("manifest", []) => manifest(&root, &dist),
        ("notes", []) => {
            print!("{}", notes(&root, None)?);
            Ok(())
        }
        ("notes", [flag, tag]) if flag == "--tag" => {
            print!("{}", notes(&root, Some(tag))?);
            Ok(())
        }
        ("formula", []) => {
            print!("{}", formula(&version(&root)?, &read_sums(&dist)?)?);
            Ok(())
        }
        ("npm", []) => npm(&root, &dist, &root.join("target").join("npm")),
        ("npm", [flag, out]) if flag == "--out" => npm(&root, &dist, &root.join(out)),
        ("check", []) => check(&root, None),
        ("check", [flag, tag]) if flag == "--tag" => check(&root, Some(tag)),
        _ => Err(USAGE.to_owned()),
    }
}

/// The workspace version, from `[workspace.package]`.
pub(crate) fn version(root: &Path) -> Result<String> {
    let manifest = read(&root.join("Cargo.toml"))?;
    workspace_version(&manifest).ok_or_else(|| "no version in [workspace.package]".into())
}

fn workspace_version(manifest: &str) -> Option<String> {
    let section = &manifest[manifest.find("[workspace.package]")?..];
    section
        .lines()
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .find_map(|l| quoted_value(l, "version"))
}

/// `key = "value"` → `value`.
fn quoted_value(line: &str, key: &str) -> Option<String> {
    let (k, v) = line.split_once('=')?;
    (k.trim() == key).then_some(())?;
    let v = v.trim();
    Some(v.strip_prefix('"')?.split('"').next()?.to_owned())
}

fn archive(root: &Path, dist: &Path, target: Target) -> Result {
    let binary = dist.join(format!("chrome-cdp-{}", target.triple));
    if !binary.is_file() {
        return Err(format!(
            "{} is missing: run `cargo xtask dist --target {}` first",
            binary.display(),
            target.triple
        ));
    }
    let name = format!("chrome-cdp-{}", target.triple);
    let staging = dist.join(".staging").join(&name);
    let _ = fs::remove_dir_all(dist.join(".staging"));
    fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    copy(&binary, &staging.join("chrome-cdp"))?;
    for file in EXTRA_FILES {
        let from = root.join(file);
        if from.is_file() {
            copy(&from, &staging.join(file))?;
        }
    }
    let tarball = dist.join(target.archive());
    crate::run(
        Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(dist.join(".staging"))
            .arg(&name),
    )?;
    let _ = fs::remove_dir_all(dist.join(".staging"));
    let sum = sha256(&tarball)?;
    let line = format!("{sum}  {}\n", target.archive());
    let sum_file = dist.join(format!("{}.sha256", target.archive()));
    fs::write(&sum_file, &line).map_err(|e| format!("write {}: {e}", sum_file.display()))?;
    println!("{}", tarball.display());
    print!("{line}");
    Ok(())
}

fn copy(from: &Path, to: &Path) -> Result {
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("copy {} to {}: {e}", from.display(), to.display()))
}

/// The SHA-256 of `path`, in hex.
fn sha256(path: &Path) -> Result<String> {
    let out = if cfg!(target_os = "macos") {
        output(Command::new("shasum").args(["-a", "256"]).arg(path))?
    } else {
        output(Command::new("sha256sum").arg(path))?
    };
    parse_sum(&out)
        .map(|(hex, _)| hex.to_owned())
        .ok_or_else(|| format!("no hash in `{}`", out.trim()))
}

/// `<hex>  <file>` (sha256sum's format) → (hex, file).
fn parse_sum(line: &str) -> Option<(&str, &str)> {
    let (hex, file) = line.trim().split_once(char::is_whitespace)?;
    let file = file.trim_start().trim_start_matches('*');
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some((hex, file))
}

/// Each archive's checksum, from the `.sha256` files `archive` wrote.
fn read_sums(dist: &Path) -> Result<BTreeMap<&'static str, String>> {
    let mut sums = BTreeMap::new();
    for target in TARGETS {
        let path = dist.join(format!("{}.sha256", target.archive()));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let (hex, file) = parse_sum(&text).ok_or_else(|| format!("{}: no hash", path.display()))?;
        if file != target.archive() {
            return Err(format!("{}: names {file}", path.display()));
        }
        sums.insert(target.triple, hex.to_owned());
    }
    if sums.is_empty() {
        return Err(format!(
            "no archives in {}: run `cargo xtask release archive --target T`",
            dist.display()
        ));
    }
    Ok(sums)
}

fn manifest(root: &Path, dist: &Path) -> Result {
    let version = version(root)?;
    let sums = read_sums(dist)?;
    let mut lines = String::new();
    for target in TARGETS {
        if let Some(sum) = sums.get(target.triple) {
            let _ = writeln!(lines, "{sum}  {}", target.archive());
        }
    }
    write(&dist.join("SHA256SUMS"), &lines)?;
    write(&dist.join("manifest.json"), &manifest_json(&version, &sums))?;
    for target in TARGETS.iter().filter(|t| !sums.contains_key(t.triple)) {
        warn(&format!("no archive for {}", target.triple));
    }
    print!("{lines}");
    Ok(())
}

/// What `chrome-cdp upgrade` reads: the version and each target's archive
/// and checksum.
fn manifest_json(version: &str, sums: &BTreeMap<&str, String>) -> String {
    let mut json = format!("{{\n  \"version\": \"{version}\",\n  \"targets\": {{\n");
    let entries: Vec<String> = TARGETS
        .iter()
        .filter_map(|t| {
            let sum = sums.get(t.triple)?;
            Some(format!(
                "    \"{}\": {{ \"archive\": \"{}\", \"sha256\": \"{sum}\" }}",
                t.triple,
                t.archive()
            ))
        })
        .collect();
    json.push_str(&entries.join(",\n"));
    json.push_str("\n  }\n}\n");
    json
}

fn write(path: &Path, text: &str) -> Result {
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))
}

/// The Homebrew formula: one archive per OS and CPU.
fn formula(version: &str, sums: &BTreeMap<&str, String>) -> Result<String> {
    let url = |t: Target| {
        format!(
            "https://github.com/{REPO}/releases/download/v{version}/{}",
            t.archive()
        )
    };
    let mut blocks = String::new();
    for (os, block) in [("darwin", "on_macos"), ("linux", "on_linux")] {
        let _ = writeln!(blocks, "  {block} do");
        for (cpu, arch) in [("arm64", "on_arm"), ("x64", "on_intel")] {
            let target = TARGETS
                .into_iter()
                .find(|t| t.os == os && t.cpu == cpu)
                .ok_or("no target")?;
            let sum = sums
                .get(target.triple)
                .ok_or_else(|| format!("no archive for {} in dist/", target.triple))?;
            let _ = writeln!(blocks, "    {arch} do");
            let _ = writeln!(blocks, "      url \"{}\"", url(target));
            let _ = writeln!(blocks, "      sha256 \"{sum}\"");
            let _ = writeln!(blocks, "    end");
        }
        let _ = writeln!(blocks, "  end");
    }
    Ok(format!(
        "# Generated by `cargo xtask release formula` in {REPO}; do not edit.
class ChromeCdp < Formula
  desc \"Drive Chrome over the DevTools Protocol from a shell; built for AI agents\"
  homepage \"https://github.com/{REPO}\"
  version \"{version}\"

{blocks}
  def install
    bin.install \"chrome-cdp\"
    generate_completions_from_executable(bin/\"chrome-cdp\", \"completions\")
  end

  def caveats
    <<~EOS
      chrome-cdp uses the Chrome or Chromium it finds, or run:
        chrome-cdp install
      Check the setup with:
        chrome-cdp doctor
    EOS
  end

  test do
    assert_match version.to_s, shell_output(\"#{{bin}}/chrome-cdp --version\")
  end
end
"
    ))
}

/// The npm packages: `chrome-cdp` (the launcher in `npm/chrome-cdp`, with
/// the version filled in) and a `chrome-cdp-<os>-<cpu>` package per
/// archive in `dist/`, which the launcher lists as optional dependencies.
fn npm(root: &Path, dist: &Path, out: &Path) -> Result {
    let version = version(root)?;
    let sums = read_sums(dist)?;
    let _ = fs::remove_dir_all(out);
    let template = root.join("npm").join("chrome-cdp");
    let main = out.join("chrome-cdp");
    fs::create_dir_all(main.join("bin")).map_err(|e| format!("create {}: {e}", main.display()))?;
    let package = read(&template.join("package.json"))?;
    write(
        &main.join("package.json"),
        &main_package_json(&package, &version)?,
    )?;
    copy(
        &template.join("bin").join("chrome-cdp.js"),
        &main.join("bin").join("chrome-cdp.js"),
    )?;
    copy(&template.join("README.md"), &main.join("README.md"))?;
    for target in TARGETS.iter().filter(|t| sums.contains_key(t.triple)) {
        let dir = out.join(target.npm_package());
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).map_err(|e| format!("create {}: {e}", bin.display()))?;
        crate::run(
            Command::new("tar")
                .arg("-xzf")
                .arg(dist.join(target.archive()))
                .arg("-C")
                .arg(&bin)
                .arg("--strip-components=1")
                .arg(format!("chrome-cdp-{}/chrome-cdp", target.triple)),
        )?;
        write(
            &dir.join("package.json"),
            &platform_package_json(*target, &version),
        )?;
        write(
            &dir.join("README.md"),
            &format!(
                "# {}\n\nThe `chrome-cdp` binary for {} {}. Install `chrome-cdp` instead: \
                 it depends on the package for your platform.\n",
                target.npm_package(),
                target.os,
                target.cpu
            ),
        )?;
        println!("{}", dir.display());
    }
    println!("{}", main.display());
    Ok(())
}

/// The launcher's package.json with `version` in place of `0.0.0-dev`,
/// the version of every optional dependency included.
fn main_package_json(template: &str, version: &str) -> Result<String> {
    if !template.contains("\"0.0.0-dev\"") {
        return Err("npm/chrome-cdp/package.json has no \"0.0.0-dev\" to replace".into());
    }
    for target in TARGETS {
        if !template.contains(&format!("\"{}\"", target.npm_package())) {
            return Err(format!(
                "npm/chrome-cdp/package.json lacks the optional dependency {}",
                target.npm_package()
            ));
        }
    }
    Ok(template.replace("\"0.0.0-dev\"", &format!("\"{version}\"")))
}

fn platform_package_json(target: Target, version: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "version": "{version}",
  "description": "The chrome-cdp binary for {os} {cpu}",
  "repository": {{ "type": "git", "url": "git+https://github.com/{REPO}.git" }},
  "homepage": "https://github.com/{REPO}",
  "os": ["{os}"],
  "cpu": ["{cpu}"],
  "files": ["bin/chrome-cdp"],
  "preferUnplugged": true
}}
"#,
        name = target.npm_package(),
        os = target.os,
        cpu = target.cpu,
    )
}

/// A commit, as `notes` groups it.
#[derive(Debug, PartialEq, Eq)]
struct Commit {
    sha: String,
    subject: String,
    body: String,
}

/// The release notes for `tag` (default: `v<workspace version>`).
fn notes(root: &Path, tag: Option<&String>) -> Result<String> {
    let tag = match tag {
        Some(tag) => tag.clone(),
        None => format!("v{}", version(root)?),
    };
    let version = tag.trim_start_matches('v');
    let changelog = read(&root.join("CHANGELOG.md")).unwrap_or_default();
    let head = if git_ok(root, &["rev-parse", "--verify", "--quiet", &tag]) {
        tag.clone()
    } else {
        "HEAD".to_owned()
    };
    let previous = Command::new("git")
        .current_dir(root)
        .args(["describe", "--tags", "--abbrev=0", "--match", "v*"])
        .arg(format!("{head}^"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    // The first release lists no commits: that would be the whole history.
    let commits = match &previous {
        Some(previous) => parse_log(&output(
            Command::new("git")
                .current_dir(root)
                .args(["log", "--no-merges", "--format=%h%x1f%s%x1f%b%x1e"])
                .arg(format!("{previous}..{head}")),
        )?),
        None => Vec::new(),
    };
    let mut text = String::new();
    if let Some(section) = changelog_section(&changelog, version) {
        text.push_str(section.trim());
        text.push_str("\n\n");
    } else {
        warn(&format!("CHANGELOG.md has no section for {version}"));
    }
    if previous.is_some() {
        text.push_str(&group_commits(&commits));
    }
    if let Some(previous) = previous {
        let _ = writeln!(
            text,
            "\n**Full diff:** https://github.com/{REPO}/compare/{previous}...{tag}"
        );
    }
    Ok(text)
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `git log --format=%h%x1f%s%x1f%b%x1e` → commits.
fn parse_log(log: &str) -> Vec<Commit> {
    log.split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(3, '\x1f');
            let sha = fields.next()?.trim();
            let subject = fields.next()?.trim();
            (!sha.is_empty()).then(|| Commit {
                sha: sha.to_owned(),
                subject: subject.to_owned(),
                body: fields.next().unwrap_or_default().trim().to_owned(),
            })
        })
        .collect()
}

/// The section of a Keep a Changelog file for `version`: from its
/// `## [version]` (or `## version`) heading to the next `## `.
fn changelog_section<'a>(changelog: &'a str, version: &str) -> Option<&'a str> {
    let mut start = None;
    let mut offset = 0;
    for line in changelog.split_inclusive('\n') {
        if let Some(heading) = line.strip_prefix("## ") {
            if start.is_some() {
                return start.map(|s| &changelog[s..offset]);
            }
            let name = heading
                .trim()
                .trim_start_matches('[')
                .split([']', ' '])
                .next()
                .unwrap_or_default();
            if name.trim_start_matches('v') == version {
                start = Some(offset + line.len());
            }
        }
        offset += line.len();
    }
    start.map(|s| &changelog[s..])
}

/// The heading a commit goes under: `type(scope)!: subject` of the
/// conventional-commit types, with a `!` or a `BREAKING CHANGE:` footer
/// first. Commits whose prefix is a component (`mock: …`, `xtask: …`)
/// or that have none are "Changes".
fn group(commit: &Commit) -> &'static str {
    let prefix = commit.subject.split_once(':').map(|(p, _)| p.trim());
    let breaking = prefix.is_some_and(|p| p.ends_with('!'))
        || commit.body.contains("BREAKING CHANGE:")
        || commit.body.contains("BREAKING-CHANGE:");
    if breaking {
        return "Breaking changes";
    }
    let kind = prefix.filter(|p| !p.contains(' ')).map(|p| {
        p.split('(')
            .next()
            .unwrap_or(p)
            .split('/')
            .next()
            .unwrap_or(p)
    });
    match kind {
        Some("feat") => "Features",
        Some("fix") => "Fixes",
        Some("perf") => "Performance",
        Some("docs") => "Documentation",
        Some("test" | "tests") => "Tests",
        Some("ci" | "build") => "Build and CI",
        Some("chore" | "refactor" | "style" | "revert") => "Maintenance",
        _ => "Changes",
    }
}

const GROUP_ORDER: [&str; 9] = [
    "Breaking changes",
    "Features",
    "Fixes",
    "Performance",
    "Changes",
    "Documentation",
    "Tests",
    "Build and CI",
    "Maintenance",
];

fn group_commits(commits: &[Commit]) -> String {
    let mut text = String::new();
    for heading in GROUP_ORDER {
        let in_group: Vec<&Commit> = commits.iter().filter(|c| group(c) == heading).collect();
        if in_group.is_empty() {
            continue;
        }
        let _ = writeln!(text, "### {heading}\n");
        for c in in_group {
            let _ = writeln!(text, "- {} ({})", c.subject, c.sha);
        }
        text.push('\n');
    }
    if commits.is_empty() {
        text.push_str("No commits since the previous release.\n");
    }
    text
}

/// Checks what a release needs: every internal dependency's `version`
/// equals the workspace version, and with `tag`, the tag is `v<version>`
/// and CHANGELOG.md has its section. A missing license only warns: the
/// GitHub release does not need one, crates.io and npm refuse without.
fn check(root: &Path, tag: Option<&String>) -> Result {
    let version = version(root)?;
    let manifest = read(&root.join("Cargo.toml"))?;
    let mut problems = internal_version_problems(&manifest, &version);
    if let Some(tag) = tag {
        if tag.strip_prefix('v') != Some(version.as_str()) {
            problems.push(format!(
                "the tag {tag} does not match the workspace version {version}: \
                 set version = \"{}\" in Cargo.toml",
                tag.trim_start_matches('v')
            ));
        }
        let changelog = read(&root.join("CHANGELOG.md")).unwrap_or_default();
        if changelog_section(&changelog, &version).is_none_or(|s| s.trim().is_empty()) {
            problems.push(format!(
                "CHANGELOG.md has no `## [{version}]` section (or it is empty)"
            ));
        }
    }
    if !manifest.contains("\nlicense") {
        warn("no `license` in [workspace.package]: crates.io and npm need one before publishing");
    }
    if crates_publish_off(&manifest) {
        warn("`publish = false` in [workspace.package]: the crates.io job will skip publishing");
    }
    if problems.is_empty() {
        eprintln!("release check: {version} ok");
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Whether `[workspace.package]` turns publishing off (until a license is
/// chosen).
fn crates_publish_off(manifest: &str) -> bool {
    manifest
        .find("[workspace.package]")
        .map(|at| &manifest[at..])
        .is_some_and(|section| {
            section
                .lines()
                .skip(1)
                .take_while(|l| !l.starts_with('['))
                .any(|l| l.replace(' ', "") == "publish=false")
        })
}

/// Internal dependencies (`package = "chrome-cdp-…"`) whose `version` is
/// not the workspace's.
fn internal_version_problems(manifest: &str, version: &str) -> Vec<String> {
    manifest
        .lines()
        .filter(|l| l.contains("package = \"chrome-cdp-"))
        .filter_map(|l| {
            let found = l
                .split_once("version = \"")
                .and_then(|(_, v)| v.split('"').next());
            (found != Some(version)).then(|| {
                format!(
                    "{}: its version must be \"{version}\", the workspace version",
                    l.trim()
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(subject: &str, body: &str) -> Commit {
        Commit {
            sha: "abc1234".into(),
            subject: subject.into(),
            body: body.into(),
        }
    }

    #[test]
    fn reads_the_workspace_version() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[x]\nversion = \"9\"\n";
        assert_eq!(workspace_version(manifest).as_deref(), Some("1.2.3"));
        assert!(version(&workspace_root()).is_ok());
    }

    #[test]
    fn parses_sha256sum_lines() {
        let hex = "a".repeat(64);
        assert_eq!(
            parse_sum(&format!("{hex}  chrome-cdp-x.tar.gz\n")),
            Some((hex.as_str(), "chrome-cdp-x.tar.gz"))
        );
        assert_eq!(
            parse_sum(&format!("{hex} *f.tar.gz")),
            Some((hex.as_str(), "f.tar.gz"))
        );
        assert_eq!(parse_sum("abc  f"), None);
    }

    #[test]
    fn groups_commits_by_conventional_type() {
        assert_eq!(group(&commit("feat(cli): doctor", "")), "Features");
        assert_eq!(group(&commit("fix: a crash", "")), "Fixes");
        assert_eq!(group(&commit("feat!: new output", "")), "Breaking changes");
        assert_eq!(
            group(&commit("mock: x", "BREAKING CHANGE: the file format")),
            "Breaking changes"
        );
        assert_eq!(group(&commit("docs/e2e: recipes", "")), "Documentation");
        assert_eq!(group(&commit("tests: two flakes", "")), "Tests");
        assert_eq!(group(&commit("ci: weekly beta", "")), "Build and CI");
        assert_eq!(group(&commit("mock: a formatter", "")), "Changes");
        assert_eq!(group(&commit("plan 10.3: parity", "")), "Changes");
        assert_eq!(group(&commit("Publishable crates", "")), "Changes");
        let text = group_commits(&[
            commit("docs: a guide", ""),
            commit("feat: upgrade", ""),
            commit("fix: b", ""),
        ]);
        assert_eq!(
            text,
            "### Features\n\n- feat: upgrade (abc1234)\n\n### Fixes\n\n- fix: b (abc1234)\n\n\
             ### Documentation\n\n- docs: a guide (abc1234)\n\n"
        );
    }

    #[test]
    fn parses_git_log_records() {
        let log = "abc\x1ffeat: x\x1fbody line\n\x1e\ndef\x1ffix: y\x1f\x1e\n";
        assert_eq!(
            parse_log(log),
            vec![
                Commit {
                    sha: "abc".into(),
                    subject: "feat: x".into(),
                    body: "body line".into()
                },
                Commit {
                    sha: "def".into(),
                    subject: "fix: y".into(),
                    body: String::new()
                },
            ]
        );
    }

    #[test]
    fn finds_a_changelog_section() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n- next\n\n## [1.1.0] - 2026-10-01\n\n- one\n- two\n\n## [1.0.0]\n\n- first\n";
        assert_eq!(
            changelog_section(changelog, "1.1.0"),
            Some("\n- one\n- two\n\n")
        );
        assert_eq!(changelog_section(changelog, "1.0.0"), Some("\n- first\n"));
        assert_eq!(changelog_section(changelog, "0.9.0"), None);
        assert!(
            changelog_section(
                &read(&workspace_root().join("CHANGELOG.md")).unwrap(),
                "Unreleased"
            )
            .is_some()
        );
    }

    #[test]
    fn internal_dependencies_follow_the_workspace_version() {
        let manifest = "cdp = { path = \"crates/cdp\", package = \"chrome-cdp-cdp\", version = \"0.1.0\" }\n\
                        mock = { path = \"crates/mock\", package = \"chrome-cdp-mock\", version = \"0.0.9\" }\n";
        let problems = internal_version_problems(manifest, "0.1.0");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("mock = "), "{problems:?}");
        let root = workspace_root();
        let real = read(&root.join("Cargo.toml")).unwrap();
        assert_eq!(
            internal_version_problems(&real, &version(&root).unwrap()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn sees_whether_publishing_is_off() {
        assert!(crates_publish_off(
            "[workspace.package]\npublish = false\n[x]\n"
        ));
        assert!(!crates_publish_off(
            "[workspace.package]\nversion = \"1\"\n[x]\npublish = false\n"
        ));
    }

    #[test]
    fn manifest_json_lists_each_archive() {
        let mut sums = BTreeMap::new();
        sums.insert("aarch64-apple-darwin", "b".repeat(64));
        sums.insert("x86_64-unknown-linux-musl", "a".repeat(64));
        let json = manifest_json("1.2.3", &sums);
        assert_eq!(
            json,
            format!(
                "{{\n  \"version\": \"1.2.3\",\n  \"targets\": {{\n    \"x86_64-unknown-linux-musl\": {{ \"archive\": \"chrome-cdp-x86_64-unknown-linux-musl.tar.gz\", \"sha256\": \"{}\" }},\n    \"aarch64-apple-darwin\": {{ \"archive\": \"chrome-cdp-aarch64-apple-darwin.tar.gz\", \"sha256\": \"{}\" }}\n  }}\n}}\n",
                "a".repeat(64),
                "b".repeat(64)
            )
        );
    }

    #[test]
    fn formula_needs_all_four_archives() {
        let mut sums = BTreeMap::new();
        for (i, t) in TARGETS.iter().enumerate() {
            sums.insert(t.triple, format!("{i}").repeat(64));
        }
        let rb = formula("1.2.3", &sums).unwrap();
        assert!(rb.contains("class ChromeCdp < Formula"), "{rb}");
        assert!(
            rb.contains(
                "    on_arm do\n      url \"https://github.com/glotlabs/chrome-cdp/releases/download/v1.2.3/chrome-cdp-aarch64-apple-darwin.tar.gz\"\n      sha256 \"3333"
            ),
            "{rb}"
        );
        sums.remove("x86_64-apple-darwin");
        assert!(formula("1.2.3", &sums).is_err());
    }

    #[test]
    fn npm_package_json_gets_the_version() {
        let template = read(&workspace_root().join("npm/chrome-cdp/package.json")).unwrap();
        let json = main_package_json(&template, "1.2.3").unwrap();
        assert!(!json.contains("0.0.0-dev"), "{json}");
        assert_eq!(json.matches("\"1.2.3\"").count(), 5, "{json}");
        let platform = platform_package_json(TARGETS[1], "1.2.3");
        assert!(platform.contains("\"name\": \"chrome-cdp-linux-arm64\""));
        assert!(platform.contains("\"os\": [\"linux\"]"));
        assert!(platform.contains("\"cpu\": [\"arm64\"]"));
    }
}

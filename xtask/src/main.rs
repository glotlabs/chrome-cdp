//! Development tasks, run as `cargo xtask <task>`.
//!
//! Std-only on purpose: `cargo xtask ci` should not compile a dependency tree
//! before it can start checking the workspace.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The release binary must stay under this size (plan 01, milestone 1.1).
const MAX_BINARY_BYTES: u64 = 15 * 1024 * 1024;

const USAGE: &str = "\
usage: cargo xtask <task> [args]

tasks:
  ci                  run cargo fmt --check, clippy -D warnings and tests
  dist [--target T]   build the release binary into dist/ and check its size
  gen-cdp [--check]   regenerate CDP types from the vendored protocol JSON
  fixtures            manage the fixture site in testdata/site
";

type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some((task, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match task.as_str() {
        "ci" => ci(rest),
        "dist" => dist(rest),
        "gen-cdp" => gen_cdp(rest),
        "fixtures" => fixtures(rest),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown task `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn ci(args: &[String]) -> Result {
    no_args("ci", args)?;
    cargo(["fmt", "--all", "--check"])?;
    cargo([
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(["test", "--workspace", "--locked"])?;
    // Milestone 1.2 adds: gen-cdp --check (generated code must not drift).
    Ok(())
}

fn dist(args: &[String]) -> Result {
    let target = match args {
        [] => None,
        [flag, triple] if flag == "--target" => Some(triple.as_str()),
        _ => return Err("usage: cargo xtask dist [--target <triple>]".into()),
    };

    let mut build = vec!["build", "--release", "--locked", "--package", "chrome-cdp"];
    if let Some(triple) = target {
        build.extend(["--target", triple]);
    }
    cargo(build)?;

    let root = workspace_root();
    let exe = format!("chrome-cdp{}", env::consts::EXE_SUFFIX);
    let built = match target {
        Some(triple) => root.join("target").join(triple).join("release").join(&exe),
        None => root.join("target").join("release").join(&exe),
    };
    let dist_dir = root.join("dist");
    fs::create_dir_all(&dist_dir).map_err(|e| format!("create {}: {e}", dist_dir.display()))?;
    let out = match target {
        Some(triple) => dist_dir.join(format!("chrome-cdp-{triple}{}", env::consts::EXE_SUFFIX)),
        None => dist_dir.join(&exe),
    };
    fs::copy(&built, &out).map_err(|e| format!("copy {}: {e}", built.display()))?;

    let size = fs::metadata(&out)
        .map_err(|e| format!("stat {}: {e}", out.display()))?
        .len();
    println!("{} ({})", out.display(), human_size(size));
    if size > MAX_BINARY_BYTES {
        return Err(format!(
            "binary is {}, over the {} budget",
            human_size(size),
            human_size(MAX_BINARY_BYTES)
        ));
    }
    Ok(())
}

fn gen_cdp(args: &[String]) -> Result {
    match args {
        [] => {}
        [flag] if flag == "--check" => {}
        _ => return Err("usage: cargo xtask gen-cdp [--check]".into()),
    }
    Err("gen-cdp is not implemented yet (plan 01, milestone 1.2)".into())
}

fn fixtures(args: &[String]) -> Result {
    no_args("fixtures", args)?;
    Err(
        "fixtures is not implemented yet (it will serve testdata/site once the test server exists)"
            .into(),
    )
}

fn no_args(task: &str, args: &[String]) -> Result {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "`{task}` takes no arguments, got: {}",
            args.join(" ")
        ))
    }
}

/// Runs `cargo <args>` in the workspace root and fails if it does not succeed.
fn cargo<I, S>(args: I) -> Result
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(args).current_dir(workspace_root());
    let shown = format!(
        "cargo {}",
        cmd.get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    eprintln!("$ {shown}");
    let status = cmd.status().map_err(|e| format!("{shown}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} failed ({status})"))
    }
}

fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
}

#[allow(clippy::cast_precision_loss)]
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{}KB", bytes / KB)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_uses_short_units() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(12 * 1024), "12KB");
        assert_eq!(human_size(3 * 1024 * 1024 + 512 * 1024), "3.5MB");
    }

    #[test]
    fn workspace_root_contains_workspace_manifest() {
        let manifest = fs::read_to_string(workspace_root().join("Cargo.toml")).unwrap();
        assert!(manifest.contains("[workspace]"));
    }
}

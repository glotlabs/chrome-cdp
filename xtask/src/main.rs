//! Development tasks, run as `cargo xtask <task>`.
//!
//! Std-only on purpose: `cargo xtask ci` should not compile a dependency tree
//! before it can start checking the workspace.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The release binary must stay under this size (plan 01, milestone 1.1).
const MAX_BINARY_BYTES: u64 = 15 * 1024 * 1024;

const USAGE: &str = "\
usage: cargo xtask <task> [args]

tasks:
  ci                  run cargo fmt --check, gen-cdp --check, clippy -D warnings
                      and tests (with cargo-nextest when it is installed)
  e2e [--repeat N]    run the whole suite with cargo-nextest (profile `ci`, no
                      retries) N times (default 1), checking for leaks after each
                      run; any failing run fails the task (the flakiness gate)
  leaks               fail if a Chrome, watchdog or daemon of chrome-cdp still runs,
                      or a session dir still holds a socket file
  soak                run the ignored soak tests (release build; needs CHROME_PATH),
                      then check for leaks
  dist [--target T]   build the release binary into dist/ and check its size
  gen-cdp [--check]   regenerate CDP types from the vendored protocol JSON
  build-js [--check]  typecheck and bundle the injected helper (needs Node's npx)
                      into crates/engine/src/injected/ccdp.js
  fixtures            serve testdata/site on two local origins (--port, --port-b)
                      and an HTTPS one with a self-signed certificate
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
        "e2e" => e2e(rest),
        "leaks" => leaks(rest),
        "soak" => soak(rest),
        "dist" => dist(rest),
        "gen-cdp" => gen_cdp(rest),
        "build-js" => build_js(rest),
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
    gen_cdp(&["--check".to_owned()])?;
    cargo([
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    if has_nextest() {
        cargo([
            "nextest",
            "run",
            "--workspace",
            "--locked",
            "--profile",
            "ci",
        ])?;
        // nextest does not run doctests.
        cargo(["test", "--workspace", "--locked", "--doc"])?;
    } else {
        cargo(["test", "--workspace", "--locked"])?;
    }
    Ok(())
}

fn has_nextest() -> bool {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .args(["nextest", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Where `e2e` writes the summary of its runs (the nightly job files it as
/// an issue when a run failed).
const E2E_SUMMARY: &str = "target/nextest/e2e-summary.md";

/// Runs the whole suite `--repeat N` times with nextest's `ci` profile
/// (retries off, so a flaky test fails a run) and checks for leaks after
/// each run. Every run happens even after a failure, so the summary says
/// how often each run failed. Arguments after `--` go to nextest.
fn e2e(args: &[String]) -> Result {
    let (own, extra) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let repeat: usize = match own {
        [] => 1,
        [flag, n] if flag == "--repeat" => n
            .parse()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("--repeat takes a count, got `{n}`"))?,
        _ => return Err("usage: cargo xtask e2e [--repeat N] [-- <nextest args>]".into()),
    };
    if !has_nextest() {
        return Err("cargo-nextest is not installed: cargo install --locked cargo-nextest".into());
    }
    let mut failed = Vec::new();
    for n in 1..=repeat {
        eprintln!("=== e2e run {n} of {repeat}");
        let mut run = vec![
            "nextest",
            "run",
            "--workspace",
            "--locked",
            "--profile",
            "ci",
        ];
        run.extend(extra.iter().map(String::as_str));
        let tests = cargo(run).err();
        let leaked = leaks(&[]).err();
        if tests.is_some() || leaked.is_some() {
            let why: Vec<String> = tests.into_iter().chain(leaked).collect();
            failed.push((n, why.join("; ")));
        }
    }
    let mut summary = format!("# E2E suite: {} of {repeat} runs failed\n\n", failed.len());
    for (n, why) in &failed {
        let _ = writeln!(summary, "- run {n}: {why}");
    }
    if !failed.is_empty() {
        summary.push_str(
            "\nRetries are off, so a test that failed once and passed in the other \
             runs is flaky. The job's log has each run's failures.\n",
        );
    }
    let path = workspace_root().join(E2E_SUMMARY);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    fs::write(&path, &summary).map_err(|e| format!("write {}: {e}", path.display()))?;
    eprint!("{summary}");
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} of {repeat} runs failed", failed.len()))
    }
}

/// What a run of chrome-cdp may leave behind. Only chrome-cdp starts Chrome
/// with `--remote-debugging-pipe`.
const LEAKED_PROCESSES: [&str; 3] = [
    "--remote-debugging-pipe",
    "chrome-cdp-watchdog",
    "chrome-cdp __daemon",
];

/// Fails if a process of [`LEAKED_PROCESSES`] still runs (after a grace
/// period of 5 s, as processes exit shortly after `close`), or a socket
/// file is left in a session dir or under `target/tmp` (the tests' fixed
/// dirs). Meant for CI, after the tests: on a machine with sessions of its
/// own, those count too.
fn leaks(args: &[String]) -> Result {
    no_args("leaks", args)?;
    let mut processes = leaked_processes()?;
    for _ in 0..50 {
        if processes.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        processes = leaked_processes()?;
    }
    let mut found: Vec<String> = processes
        .into_iter()
        .map(|p| format!("process {p}"))
        .collect();
    for dir in session_dirs()? {
        found.extend(
            sockets_in(&dir)
                .into_iter()
                .map(|s| format!("socket {}", s.display())),
        );
    }
    if found.is_empty() {
        eprintln!("no leaks: no chrome-cdp processes or socket files are left");
        return Ok(());
    }
    Err(format!(
        "chrome-cdp left these behind:\n  {}",
        found.join("\n  ")
    ))
}

fn leaked_processes() -> Result<Vec<String>> {
    let ps = output(Command::new("ps").args(["-eo", "pid=,args="]))?;
    Ok(ps
        .lines()
        .filter(|l| LEAKED_PROCESSES.iter().any(|p| l.contains(p)))
        .map(|l| l.trim().to_owned())
        .collect())
}

/// The dirs sessions live in (see `crates/daemon/src/paths.rs`), plus the
/// tests' fixed dirs under `target/tmp`.
fn session_dirs() -> Result<Vec<PathBuf>> {
    let uid = output(Command::new("id").arg("-u"))?.trim().to_owned();
    let mut dirs = vec![
        PathBuf::from(format!("/tmp/chrome-cdp-{uid}")),
        PathBuf::from(format!("/tmp/ccdp-{uid}")),
        workspace_root().join("target/tmp"),
    ];
    for var in ["XDG_RUNTIME_DIR", "TMPDIR"] {
        if let Some(dir) = env::var_os(var).filter(|d| !d.is_empty()) {
            dirs.push(PathBuf::from(dir).join("chrome-cdp"));
        }
    }
    Ok(dirs)
}

/// The socket files under `dir`, at any depth.
fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt as _;
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        #[cfg(unix)]
        if kind.is_socket() {
            found.push(entry.path());
            continue;
        }
        if kind.is_dir() {
            found.extend(sockets_in(&entry.path()));
        }
    }
    found
}

/// Runs the ignored soak tests in a release build (they need `CHROME_PATH`;
/// see `crates/chrome-cdp/tests/soak.rs`), then checks for leaks.
fn soak(args: &[String]) -> Result {
    no_args("soak", args)?;
    if env::var_os("CHROME_PATH").is_none_or(|p| p.is_empty()) {
        return Err("the soak tests need a Chrome: set CHROME_PATH".into());
    }
    cargo([
        "test",
        "--release",
        "--locked",
        "--package",
        "chrome-cdp",
        "--test",
        "soak",
        "--",
        "--ignored",
        "--nocapture",
        "--test-threads",
        "1",
    ])?;
    leaks(&[])
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

/// Runs the `cdp-gen` binary. It needs serde, so it lives in its own crate
/// and xtask stays std-only.
fn gen_cdp(args: &[String]) -> Result {
    let mut run = vec!["run", "--quiet", "--locked", "--package", "cdp-gen", "--"];
    match args {
        [] => {}
        [flag] if flag == "--check" => run.push("--check"),
        _ => return Err("usage: cargo xtask gen-cdp [--check]".into()),
    }
    cargo(run)
}

/// The injected helper bundle's TypeScript sources and built file.
const INJECTED_DIR: &str = "crates/engine/src/injected";
const INJECTED_OUT: &str = "ccdp.js";
/// Pinned so the committed bundle builds the same everywhere.
const ESBUILD: &str = "esbuild@0.28.2";
const TYPESCRIPT: &str = "typescript@7.0.2";

/// Typechecks the injected TypeScript with `tsc` and bundles it with
/// `esbuild` (both through `npx`, pinned), into a file that is committed,
/// so a plain `cargo build` needs no Node. The first line records a hash of
/// the sources; an engine test fails when it does not match them.
/// `--check` fails if the committed file is not what the sources build.
fn build_js(args: &[String]) -> Result {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => return Err("usage: cargo xtask build-js [--check]".into()),
    };
    let dir = workspace_root().join(INJECTED_DIR);
    run(Command::new("npx")
        .args(["--yes", "-p", TYPESCRIPT, "tsc", "-p", "tsconfig.json"])
        .current_dir(&dir))?;
    let bundled = output(
        Command::new("npx")
            .args([
                "--yes",
                ESBUILD,
                "src/index.ts",
                "--bundle",
                "--format=iife",
                "--target=chrome120",
                "--charset=utf8",
                "--legal-comments=none",
                "--log-level=warning",
            ])
            .current_dir(&dir),
    )?;
    let hash = injected_hash(&dir.join("src"))?;
    let built = format!(
        "// Built by `cargo xtask build-js` from src/*.ts; do not edit. sources: {hash:016x}\n{bundled}"
    );
    let out = dir.join(INJECTED_OUT);
    if check {
        let committed = fs::read_to_string(&out).unwrap_or_default();
        if committed != built {
            return Err(format!(
                "{} is not what its sources build; run `cargo xtask build-js`",
                out.display()
            ));
        }
        return Ok(());
    }
    fs::write(&out, built).map_err(|e| format!("write {}: {e}", out.display()))?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

/// FNV-1a over the `.ts` files of `dir`, by name: each file's name, a NUL,
/// its bytes and a NUL. The engine's `injected` test computes the same.
fn injected_hash(dir: &Path) -> Result<u64> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|p| p.extension() == Some(OsStr::new("ts")))
        .collect();
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in files {
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes = fs::read(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
        for byte in name.bytes().chain([0]).chain(bytes).chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Ok(hash)
}

/// Runs `cmd` and fails if it does not succeed.
fn run(cmd: &mut Command) -> Result {
    let shown = format!("{cmd:?}");
    eprintln!("$ {shown}");
    let status = cmd.status().map_err(|e| format!("{shown}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shown} failed ({status})"))
    }
}

/// Runs `cmd` and returns its stdout.
fn output(cmd: &mut Command) -> Result<String> {
    let shown = format!("{cmd:?}");
    eprintln!("$ {shown}");
    let out = cmd
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("{shown}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{shown} failed ({})", out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{shown}: {e}"))
}

/// Serves the fixture site (`crates/fixtures`, the same server the
/// integration tests use) until interrupted. Takes `--port` and `--port-b`.
fn fixtures(args: &[String]) -> Result {
    let mut run = vec!["run", "--quiet", "--locked", "--package", "fixtures", "--"];
    run.extend(args.iter().map(String::as_str));
    cargo(run)
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

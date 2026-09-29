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

mod release;

/// Each target's release binary size, in bytes: `dist` fails when a build
/// is more than 20% over it (plan 10, milestone 10.2; it replaced plan 01's
/// fixed 15MB budget).
const SIZE_BASELINES: &str = "xtask/binary-sizes.txt";

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
  bench <micro|tokens|agent> [--no-tests] [-- <args>]
                      build the release binaries and run a benchmark suite
                      (crates/bench; needs CHROME_PATH, and bench/playwright's
                      node_modules for the Playwright side); see docs/benchmarks.md.
                      micro also runs the plans' ignored timing tests (--no-tests
                      skips them); `-- --help` lists the suite's own options
  dist [--target T] [--update-size]
                      build the release binary into dist/ and check its size: at most
                      20% over the target's baseline in xtask/binary-sizes.txt
                      (--update-size records the new size there)
  release <step>      the release artifacts from dist/ (plan 10.5): archive --target T,
                      manifest, notes [--tag vX.Y.Z], formula, npm [--out DIR],
                      check [--tag vX.Y.Z]; `cargo xtask release` lists them
  chrome <which> [--with-deps]
                      get the Chrome the tests run against and print its path; in
                      GitHub Actions, also set CHROME_PATH and CHROME_CDP_REQUIRE_CHROME.
                      <which>: stable, beta, min (the oldest supported major), a
                      version (154, 154.0.8037.57) or system (Google Chrome's own
                      install); all but system run `chrome-cdp install`
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
        "bench" => bench(rest),
        "dist" => dist(rest),
        "release" => release::run(rest),
        "chrome" => chrome(rest),
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

/// The ignored tests that time a plan's budget inside a crate, run by
/// `bench micro` in a release build: (package, test target, test name).
const TIMING_TESTS: [(&str, &str, &str); 4] = [
    ("chrome-cdp-cdp", "chrome", "sequential_evaluate_latency"),
    (
        "chrome-cdp-engine",
        "snapshot",
        "snapshot_latency_benchmark",
    ),
    ("chrome-cdp-engine", "route", "overhead_with_20_rules"),
    (
        "chrome-cdp",
        "bench",
        "e2e_runner_overhead_and_the_shop_suite",
    ),
];

/// Builds the release binaries the benchmarks time, then runs a suite of
/// the `bench` harness (plan 10.4; see docs/benchmarks.md). `micro` also
/// runs [`TIMING_TESTS`], after the suite, and fails if one does.
fn bench(args: &[String]) -> Result {
    let usage =
        || "usage: cargo xtask bench <micro|tokens|agent> [--no-tests] [-- <args>]".to_owned();
    let (own, extra) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let (suite, no_tests) = match own {
        [suite] => (suite.as_str(), false),
        [suite, flag] if flag == "--no-tests" => (suite.as_str(), true),
        _ => return Err(usage()),
    };
    if !matches!(suite, "micro" | "tokens" | "agent") {
        return Err(usage());
    }
    cargo([
        "build",
        "--release",
        "--locked",
        "--package",
        "chrome-cdp",
        "--package",
        "fixtures",
    ])?;
    let mut run = vec![
        "run",
        "--quiet",
        "--locked",
        "--package",
        "bench",
        "--",
        suite,
        "--bin-dir",
        "target/release",
    ];
    run.extend(extra.iter().map(String::as_str));
    let suite_result = cargo(run);
    if suite != "micro" || no_tests || extra.iter().any(|a| a == "--help" || a == "-h") {
        return suite_result;
    }
    let mut failed: Vec<String> = suite_result.err().into_iter().collect();
    for (package, target, test) in TIMING_TESTS {
        if let Err(e) = cargo([
            "test",
            "--release",
            "--locked",
            "--package",
            package,
            "--test",
            target,
            "--",
            "--ignored",
            "--exact",
            test,
            "--nocapture",
        ]) {
            failed.push(e);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("\n"))
    }
}

fn dist(args: &[String]) -> Result {
    let usage = || "usage: cargo xtask dist [--target <triple>] [--update-size]".to_owned();
    let mut target = None;
    let mut update = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--target" => target = Some(rest.next().ok_or_else(usage)?.as_str()),
            "--update-size" => update = true,
            _ => return Err(usage()),
        }
    }

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

    let triple = match target {
        Some(triple) => triple.to_owned(),
        None => host_triple()?,
    };
    let path = root.join(SIZE_BASELINES);
    let baselines =
        fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if update {
        let updated = with_baseline(&baselines, &triple, size);
        fs::write(&path, updated).map_err(|e| format!("write {}: {e}", path.display()))?;
        eprintln!("recorded {triple} {size} in {SIZE_BASELINES}");
        return Ok(());
    }
    let check = check_size(&triple, size, baseline(&baselines, &triple));
    summarize(&format!(
        "| `{triple}` | {} | {} |",
        human_size(size),
        check.note
    ));
    match check.verdict {
        Verdict::Fine => {
            eprintln!("{triple}: {}", check.note);
            Ok(())
        }
        Verdict::Warn => {
            warn(&format!("{triple}: {}", check.note));
            Ok(())
        }
        Verdict::Fail => Err(format!("{triple}: {}", check.note)),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Fine,
    Warn,
    Fail,
}

#[derive(Debug)]
struct SizeCheck {
    verdict: Verdict,
    note: String,
}

/// Compares a binary's size with its target's baseline: more than 20% over
/// it fails; no baseline, or more than 20% under it, warns, because the
/// baseline wants recording.
fn check_size(triple: &str, size: u64, baseline: Option<u64>) -> SizeCheck {
    let update = format!("cargo xtask dist --target {triple} --update-size");
    let Some(baseline) = baseline else {
        return SizeCheck {
            verdict: Verdict::Warn,
            note: format!("no size baseline in {SIZE_BASELINES}; record it with `{update}`"),
        };
    };
    let change = (i128::from(size) - i128::from(baseline)) * 100 / i128::from(baseline.max(1));
    let versus = format!("{change:+}% vs its {} baseline", human_size(baseline));
    if size * 5 > baseline * 6 {
        SizeCheck {
            verdict: Verdict::Fail,
            note: format!(
                "{versus}, over the 20% limit; if the growth is wanted, run `{update}` \
                 and commit {SIZE_BASELINES}"
            ),
        }
    } else if size * 5 < baseline * 4 {
        SizeCheck {
            verdict: Verdict::Warn,
            note: format!("{versus}; lower the baseline with `{update}`"),
        }
    } else {
        SizeCheck {
            verdict: Verdict::Fine,
            note: versus,
        }
    }
}

/// The baseline for `triple` in the baselines file: lines of
/// `<triple> <bytes>`, `#` comments.
fn baseline(file: &str, triple: &str) -> Option<u64> {
    file.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == triple).then_some(())?;
        words.next()?.parse().ok()
    })
}

/// `file` with `triple`'s baseline set to `size`, replacing its line or
/// appending one.
fn with_baseline(file: &str, triple: &str, size: u64) -> String {
    let entry = format!("{triple} {size}");
    let mut found = false;
    let mut out: Vec<String> = file
        .lines()
        .map(|line| {
            if line.split_whitespace().next() == Some(triple) {
                found = true;
                entry.clone()
            } else {
                line.to_owned()
            }
        })
        .collect();
    if !found {
        out.push(entry);
    }
    out.join("\n") + "\n"
}

/// The target triple `rustc` builds for by default.
fn host_triple() -> Result<String> {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let info = output(Command::new(rustc).arg("-vV"))?;
    info.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV printed no host".into())
}

/// Prints a warning, as an annotation in GitHub Actions.
fn warn(message: &str) {
    if env::var_os("GITHUB_ACTIONS").is_some() {
        println!("::warning::{message}");
    } else {
        eprintln!("warning: {message}");
    }
}

/// Adds a line to the GitHub Actions job summary, if there is one.
fn summarize(line: &str) {
    if let Some(path) = env::var_os("GITHUB_STEP_SUMMARY") {
        let _ = append(Path::new(&path), &format!("{line}\n"));
    }
}

fn append(path: &Path, text: &str) -> Result {
    use std::io::Write as _;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Where Google Chrome installs itself; `chrome system` takes the first
/// that exists.
const SYSTEM_CHROME: [&str; 2] = [
    "/opt/google/chrome/chrome",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
];

/// The file that defines the oldest supported Chrome major.
const MIN_CHROME_SOURCE: &str = "crates/browser/src/version.rs";

/// Gets the Chrome a CI job tests against (plan 10, milestone 10.2) and
/// prints its path. Every Chrome but `system` comes from Chrome for
/// Testing through `chrome-cdp install` (a debug build), so CI also covers
/// the installer. In GitHub Actions it exports `CHROME_PATH` for the tests
/// and `CHROME_CDP_REQUIRE_CHROME=1`, so they fail instead of skipping.
fn chrome(args: &[String]) -> Result {
    let usage =
        || "usage: cargo xtask chrome <stable|beta|min|VERSION|system> [--with-deps]".to_owned();
    let (which, with_deps) = match args {
        [which] => (which.as_str(), false),
        [which, flag] if flag == "--with-deps" => (which.as_str(), true),
        _ => return Err(usage()),
    };
    let path = if which == "system" {
        SYSTEM_CHROME
            .iter()
            .find(|p| Path::new(p).is_file())
            .map(|p| (*p).to_owned())
            .ok_or_else(|| {
                format!(
                    "no Google Chrome here; looked at {}",
                    SYSTEM_CHROME.join(", ")
                )
            })?
    } else {
        let mut install = vec!["install".to_owned(), "--quiet".to_owned()];
        match which {
            "stable" | "beta" => install.extend(["--channel".to_owned(), which.to_owned()]),
            "min" => install.extend(["--version".to_owned(), min_chrome_major()?.to_string()]),
            version if version.starts_with(|c: char| c.is_ascii_digit()) => {
                install.extend(["--version".to_owned(), version.to_owned()]);
            }
            _ => return Err(usage()),
        }
        if with_deps {
            install.push("--with-deps".to_owned());
        }
        cargo(["build", "--locked", "--package", "chrome-cdp"])?;
        let exe = workspace_root()
            .join("target/debug")
            .join(format!("chrome-cdp{}", env::consts::EXE_SUFFIX));
        output(Command::new(exe).args(&install))?.trim().to_owned()
    };
    let version = output(Command::new(&path).arg("--version"))?;
    let version = version.trim();
    eprintln!("{version}");
    summarize(&format!("Chrome under test: {version} (`{path}`)"));
    if let Some(env_file) = env::var_os("GITHUB_ENV") {
        append(
            Path::new(&env_file),
            &format!("CHROME_PATH={path}\nCHROME_CDP_REQUIRE_CHROME=1\n"),
        )?;
    }
    println!("{path}");
    Ok(())
}

/// `MIN_CHROME_MAJOR` from the browser crate (xtask is std-only, so it
/// reads the source).
fn min_chrome_major() -> Result<u32> {
    let path = workspace_root().join(MIN_CHROME_SOURCE);
    let source = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_min_chrome_major(&source).ok_or_else(|| {
        format!(
            "no `pub const MIN_CHROME_MAJOR: u32 = N;` in {}",
            path.display()
        )
    })
}

fn parse_min_chrome_major(source: &str) -> Option<u32> {
    source.lines().find_map(|line| {
        line.trim()
            .strip_prefix("pub const MIN_CHROME_MAJOR: u32 = ")?
            .strip_suffix(';')?
            .parse()
            .ok()
    })
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
    fn size_check_fails_over_twenty_percent() {
        let mb = 1024 * 1024;
        assert_eq!(
            check_size("t", 10 * mb, Some(10 * mb)).verdict,
            Verdict::Fine
        );
        assert_eq!(
            check_size("t", 12 * mb, Some(10 * mb)).verdict,
            Verdict::Fine
        );
        let over = check_size("t", 12 * mb + 1, Some(10 * mb));
        assert_eq!(over.verdict, Verdict::Fail);
        assert!(
            over.note.starts_with("+20% vs its 10.0MB baseline"),
            "{}",
            over.note
        );
        assert!(
            over.note
                .contains("cargo xtask dist --target t --update-size")
        );
        assert_eq!(
            check_size("t", 8 * mb, Some(10 * mb)).verdict,
            Verdict::Fine
        );
        assert_eq!(
            check_size("t", 8 * mb - 1, Some(10 * mb)).verdict,
            Verdict::Warn
        );
        assert_eq!(check_size("t", mb, None).verdict, Verdict::Warn);
    }

    #[test]
    fn baselines_are_read_and_replaced_by_target() {
        let file = "# comment\nx86_64-unknown-linux-musl 100\naarch64-apple-darwin 200\n";
        assert_eq!(baseline(file, "aarch64-apple-darwin"), Some(200));
        assert_eq!(baseline(file, "x86_64-apple-darwin"), None);
        assert_eq!(
            with_baseline(file, "x86_64-unknown-linux-musl", 150),
            "# comment\nx86_64-unknown-linux-musl 150\naarch64-apple-darwin 200\n"
        );
        assert_eq!(
            with_baseline(file, "x86_64-apple-darwin", 300),
            format!("{file}x86_64-apple-darwin 300\n")
        );
    }

    #[test]
    fn min_chrome_major_is_read_from_the_browser_crate() {
        let major = min_chrome_major().unwrap();
        assert!((100..1000).contains(&major), "{major}");
        assert_eq!(
            parse_min_chrome_major("/// x\npub const MIN_CHROME_MAJOR: u32 = 150;\n"),
            Some(150)
        );
    }

    #[test]
    fn workspace_root_contains_workspace_manifest() {
        let manifest = fs::read_to_string(workspace_root().join("Cargo.toml")).unwrap();
        assert!(manifest.contains("[workspace]"));
    }
}

//! `chrome-cdp lint` and `chrome-cdp fmt` (plan 11, milestone 11.12):
//! no browser needed. `testdata/lint` has one problem per lint rule.
//!
//! To accept new golden output: `UPDATE_GOLDEN=1 cargo test -p
//! chrome-cdp --test lint`.

// clippy.toml's test allowances cover `#[test]` fns, not their helpers.
#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::path::{Path, PathBuf};

use common::{cli, code, json_out, run, stderr, stdout};
use serde_json::Value;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/lint")
}

fn lint_in(dir: &Path, args: &[&str]) -> std::process::Output {
    run(cli().current_dir(dir).arg("lint").args(args))
}

/// Compares `actual` with golden file `name`, or writes it with
/// UPDATE_GOLDEN=1.
fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        expected, actual,
        "{name} differs (UPDATE_GOLDEN=1 accepts it)"
    );
}

#[test]
fn lint_reports_each_rule_where_it_is() {
    let out = lint_in(&fixture(), &["--json"]);
    assert_eq!(code(&out), 1);
    let v = json_out(&out);
    assert_eq!(v["ok"], Value::Bool(false));
    assert_eq!(v["error"]["kind"], "ExpectFailed");
    let problems = v["error"]["details"]["problems"].as_array().unwrap();
    let found = |file: &str, line: u64, col: u64, rule: &str| {
        problems.iter().find(|p| {
            p["file"].as_str().unwrap().ends_with(file)
                && p["line"] == line
                && p["col"] == col
                && p["rule"] == rule
        })
    };
    // (file, line, col, rule, severity, part of the hint)
    let expected = [
        (
            "e2e/scenarios/syntax.cdp",
            2,
            9,
            "syntax",
            "error",
            "quotes never span lines",
        ),
        (
            "e2e/scenarios/load.cdp",
            1,
            1,
            "unknown-directive",
            "error",
            "did you mean @tags?",
        ),
        (
            "e2e/scenarios/load.cdp",
            2,
            1,
            "syntax",
            "error",
            "@timeout 60s",
        ),
        // The directive errors do not hide the steps' problems.
        (
            "e2e/scenarios/load.cdp",
            6,
            3,
            "ref",
            "error",
            "stable locator",
        ),
        (
            "e2e/scenarios/args.cdp",
            4,
            3,
            "bad-args",
            "error",
            "chrome-cdp fill --help",
        ),
        (
            "e2e/scenarios/args.cdp",
            5,
            3,
            "bad-args",
            "error",
            "chrome-cdp expect --perf 'lcp<2500 cls<0.1'   (did you mean this?)",
        ),
        (
            "e2e/scenarios/includes.cdp",
            2,
            12,
            "include-missing",
            "error",
            "relative to the file",
        ),
        (
            "e2e/steps/cycle-b.cdp",
            1,
            10,
            "include-cycle",
            "error",
            "cannot include itself",
        ),
        (
            "e2e/steps/login.cdp",
            1,
            18,
            "include-param",
            "error",
            "pass it on the @include line",
        ),
        (
            "e2e/scenarios/steps.cdp",
            4,
            3,
            "unknown-command",
            "error",
            "chrome-cdp click text=Add",
        ),
        (
            "e2e/scenarios/steps.cdp",
            5,
            3,
            "unknown-flag",
            "error",
            "--force",
        ),
        (
            "e2e/scenarios/steps.cdp",
            6,
            3,
            "locator",
            "error",
            "role=button",
        ),
        (
            "e2e/scenarios/steps.cdp",
            7,
            3,
            "ref",
            "error",
            "history --as-scenario",
        ),
        (
            "e2e/scenarios/steps.cdp",
            8,
            3,
            "sleep",
            "warning",
            "wait <locator>",
        ),
        (
            "e2e/scenarios/steps.cdp",
            12,
            20,
            "undefined-var",
            "error",
            "only_local = ",
        ),
        (
            "e2e/scenarios/steps.cdp",
            13,
            19,
            "undefined-var",
            "error",
            "@include parameters",
        ),
        (
            "e2e/scenarios/steps.cdp",
            18,
            3,
            "mock-literal",
            "warning",
            "${var:name}",
        ),
        (
            "e2e/scenarios/steps.cdp",
            19,
            3,
            "mock-only",
            "warning",
            "@backend mock",
        ),
        (
            "e2e/scenarios/steps.cdp",
            21,
            1,
            "duplicate-test",
            "error",
            "rename one",
        ),
        (
            "e2e/scenarios/steps.cdp",
            21,
            1,
            "no-expect",
            "warning",
            "expect text=",
        ),
        (
            "e2e/scenarios/steps.cdp",
            24,
            1,
            "unknown-backend",
            "error",
            "@backend mock",
        ),
        (
            "e2e/scenarios/steps.cdp",
            24,
            1,
            "unknown-device",
            "error",
            "emulate",
        ),
        (
            "e2e/mocks/base.toml",
            9,
            1,
            "unreachable",
            "warning",
            "move \"never answers\"",
        ),
    ];
    let mut missing = Vec::new();
    for (file, line, col, rule, severity, hint) in expected {
        match found(file, line, col, rule) {
            Some(p) => {
                assert_eq!(p["severity"], severity, "{p}");
                let h = p["hint"].as_str().unwrap_or("");
                assert!(h.contains(hint), "{rule}: hint {h:?} lacks {hint:?}");
            }
            None => missing.push(format!("{file}:{line}:{col} {rule}")),
        }
    }
    assert!(
        missing.is_empty(),
        "missing: {missing:#?}\nall: {problems:#?}"
    );
    // Nothing is reported that the fixture does not plant.
    assert_eq!(problems.len(), expected.len(), "{problems:#?}");
    // A test marked @backend mock may use mock-only commands.
    assert!(
        !problems
            .iter()
            .any(|p| p["file"].as_str().unwrap().ends_with("clean.cdp"))
    );
}

#[test]
fn lint_prints_github_annotations() {
    let out = lint_in(&fixture(), &["--format", "github"]);
    assert_eq!(code(&out), 1);
    let root = fixture().canonicalize().unwrap().display().to_string();
    golden("lint/github.txt", &stdout(&out).replace(&root, "<fixture>"));
    assert!(
        stderr(&out).contains("10 files: 18 errors, 5 warnings"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn lint_text_lists_errors_first() {
    let out = lint_in(&fixture(), &[]);
    assert_eq!(code(&out), 1);
    let err = stderr(&out);
    assert!(
        err.starts_with("error: e2e/scenarios/args.cdp:4:3: error [bad-args]"),
        "{err}"
    );
    assert!(stdout(&out).is_empty());
}

#[test]
fn lint_of_a_clean_file_passes() {
    let out = lint_in(&fixture(), &["e2e/scenarios/clean.cdp"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out).trim(), "1 file: 0 errors, 0 warnings");
    let out = lint_in(&fixture(), &["e2e/scenarios/clean.cdp", "-q"]);
    assert_eq!(stdout(&out).trim(), "0 0");
}

#[test]
fn lint_warnings_alone_pass() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "e2e/scenarios/a.cdp",
        "@test \"waits\"\n  goto /\n  wait 2s\n",
    );
    let out = lint_in(dir.path(), &[]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("a.cdp:3:3: warning [sleep]"), "{text}");
    assert!(text.contains("a.cdp:1:1: warning [no-expect]"), "{text}");
    assert!(text.ends_with("1 file: 0 errors, 2 warnings\n"), "{text}");
}

#[test]
fn test_refuses_to_start_on_lint_errors() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "chrome-cdp.toml",
        "[e2e]\nbase_url = \"http://127.0.0.1:9\"\n",
    );
    write(
        dir.path(),
        "e2e/scenarios/a.cdp",
        "@test \"a ref\"\n  click e12\n  expect text=Done visible\n",
    );
    // No browser starts: this fails before (CHROME_CDP_CHROME is a file
    // that does not exist).
    let out = run(cli()
        .current_dir(dir.path())
        .env("CHROME_CDP_CHROME", dir.path().join("no-chrome"))
        .arg("test"));
    assert_eq!(code(&out), 1);
    let err = stderr(&out);
    assert!(err.contains("a.cdp:2:3: error [ref]"), "{err}");
    assert!(err.contains("chrome-cdp lint"), "{err}");
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

const MESSY_CDP: &str = "@tags smoke\n@mocks base\n@test 'adds'\n  screenshot -o a.png\n  expect \"text=Added\" visible\n@test \"b\"\n  goto /\n  expect text=B visible\n";
const TIDY_CDP: &str = "@mocks base\n@tags smoke\n\n@test \"adds\"\n  screenshot --output a.png\n  expect 'text=Added' visible\n\n@test \"b\"\n  goto /\n  expect text=B visible\n";
const MESSY_TOML: &str = "[[mock]]\nrespond={status=204}\nmatch = { path = \"/api/a\" }\n";

#[test]
fn fmt_check_prints_a_diff_and_fmt_writes() {
    let dir = tempfile::tempdir().unwrap();
    let cdp = write(dir.path(), "e2e/scenarios/a.cdp", MESSY_CDP);
    let toml = write(dir.path(), "e2e/mocks/base.toml", MESSY_TOML);
    let tidy = write(dir.path(), "e2e/scenarios/b.cdp", TIDY_CDP);
    let fmt = |args: &[&str]| run(cli().current_dir(dir.path()).arg("fmt").args(args));

    let out = fmt(&["--check"]);
    assert_eq!(code(&out), 1);
    let err = stderr(&out);
    assert!(
        err.starts_with(
            "error: 2 files are not formatted: e2e/scenarios/a.cdp, e2e/mocks/base.toml"
        ),
        "{err}"
    );
    assert!(err.contains("-  screenshot -o a.png"), "{err}");
    assert!(err.contains("+  screenshot --output a.png"), "{err}");
    assert!(err.contains("+respond = { status = 204 }"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&cdp).unwrap(),
        MESSY_CDP,
        "--check wrote"
    );

    let out = fmt(&["--json", "--check"]);
    let v = json_out(&out);
    assert_eq!(v["error"]["details"]["check"], true);
    assert_eq!(v["error"]["details"]["files"], 3);
    assert!(
        v["error"]["details"]["diff"]
            .as_str()
            .unwrap()
            .contains("+++ b/e2e/scenarios/a.cdp")
    );

    let out = fmt(&[]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "formatted e2e/scenarios/a.cdp\nformatted e2e/mocks/base.toml\n3 files, 2 formatted\n"
    );
    assert_eq!(std::fs::read_to_string(&cdp).unwrap(), TIDY_CDP);
    assert_eq!(std::fs::read_to_string(&tidy).unwrap(), TIDY_CDP);
    assert_eq!(
        std::fs::read_to_string(&toml).unwrap(),
        "[[mock]]\nmatch = { path = \"/api/a\" }\nrespond = { status = 204 }\n"
    );

    let out = fmt(&["--check"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "3 files, all formatted\n");
    let out = fmt(&["-q"]);
    assert_eq!(stdout(&out), "");
}

#[test]
fn fmt_leaves_a_file_with_syntax_errors_alone() {
    let dir = tempfile::tempdir().unwrap();
    let bad = write(dir.path(), "a.cdp", "@test \"x\"\n  click 'open\n");
    let good = write(dir.path(), "b.cdp", "goto   /\n");
    let out = run(cli()
        .current_dir(dir.path())
        .args(["fmt", "a.cdp", "b.cdp"]));
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(
        err.contains("cannot format a.cdp:2:9: a single quote without its closing quote"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&bad).unwrap(),
        "@test \"x\"\n  click 'open\n"
    );
    // The other files are formatted.
    assert_eq!(std::fs::read_to_string(&good).unwrap(), "goto /\n");
}

#[test]
fn fmt_says_what_it_takes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "notes.txt", "x");
    let out = run(cli().current_dir(dir.path()).args(["fmt", "notes.txt"]));
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("is not a .cdp, mock (.toml) or audit baseline (.json) file"));
    let out = run(cli().current_dir(dir.path()).args(["fmt"]));
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("no .cdp or mock files in"),
        "{}",
        stderr(&out)
    );
}

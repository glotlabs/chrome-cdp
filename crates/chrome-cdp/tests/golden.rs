//! Golden tests of every command's text, `--json` and `-q` output, errors
//! and hints included (plan 08, milestone 8.1). The wording is part of the
//! UX, so a change shows up in review as a diff of `tests/golden/`.
//!
//! Each case runs the binary once per output mode and compares the result
//! with `tests/golden/<case>.txt`, after replacing machine-specific parts
//! (version, git sha, Chrome path, temp dirs) with placeholders. Every
//! registered JSON Schema is also kept in `tests/golden/schema/`.
//!
//! To accept new output: `UPDATE_GOLDEN=1 cargo test -p chrome-cdp --test
//! golden` (with `CHROME_PATH` set, or the Chrome cases are not written).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{chrome_path, cli, fake_chrome};
use serde_json::Value;

/// What a case needs before it runs.
#[derive(Clone, Copy, PartialEq)]
enum Fixture {
    None,
    /// A real Chrome; `{chrome}` in the args is its path.
    Chrome,
    /// `{chrome}` is a stand-in that dies like a Chrome without a sandbox.
    SandboxlessChrome,
    /// `{chrome}` is a stand-in that never answers.
    SilentChrome,
    /// Runs in a directory whose `.chrome-cdp.toml` has a typo.
    BadConfig,
}

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    /// Extra flags per run: "" is the text form.
    modes: &'static [&'static str],
    fixture: Fixture,
}

const ALL: &[&str] = &["", "--json", "-q"];
const ERR: &[&str] = &["", "--json"];

const CASES: &[Case] = &[
    Case {
        name: "version",
        args: &["-V"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "no-command",
        args: &[],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "unknown-flag",
        args: &["--bogus"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mistyped-command",
        args: &["scheme"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "missing-subcommand",
        args: &["__direct"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "mistyped-flag",
        args: &["__direct", "cdp", "Browser.getVersion", "--jsn"],
        modes: &[""],
        fixture: Fixture::None,
    },
    Case {
        name: "duration-without-unit",
        args: &["--timeout", "5000", "-V"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "config-unknown-key",
        args: &["-V"],
        modes: ERR,
        fixture: Fixture::BadConfig,
    },
    Case {
        name: "schema-list",
        args: &["schema"],
        modes: ALL,
        fixture: Fixture::None,
    },
    Case {
        name: "schema-unknown",
        args: &["schema", "nope"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "direct-bad-method",
        args: &[
            "__direct",
            "cdp",
            "getVersion",
            "--chrome",
            "/nonexistent/chrome",
        ],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "direct-bad-params",
        args: &[
            "__direct",
            "cdp",
            "Runtime.evaluate",
            "{expression:1}",
            "--chrome",
            "/nonexistent/chrome",
        ],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "direct-params-not-object",
        args: &[
            "__direct",
            "cdp",
            "Runtime.evaluate",
            "[1]",
            "--chrome",
            "/nonexistent/chrome",
        ],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "direct-no-chrome-given",
        args: &["__direct", "cdp", "Browser.getVersion"],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "direct-chrome-missing",
        args: &[
            "__direct",
            "cdp",
            "Browser.getVersion",
            "--chrome",
            "/nonexistent/chrome",
        ],
        modes: ERR,
        fixture: Fixture::None,
    },
    Case {
        name: "direct-no-sandbox",
        args: &[
            "__direct",
            "cdp",
            "Browser.getVersion",
            "--chrome",
            "{chrome}",
        ],
        modes: ERR,
        fixture: Fixture::SandboxlessChrome,
    },
    Case {
        name: "direct-startup-timeout",
        args: &[
            "__direct",
            "cdp",
            "Browser.getVersion",
            "--chrome",
            "{chrome}",
            "--timeout",
            "300ms",
        ],
        modes: ERR,
        fixture: Fixture::SilentChrome,
    },
    Case {
        name: "direct-evaluate",
        args: &[
            "__direct",
            "cdp",
            "--chrome",
            "{chrome}",
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"6*7","returnByValue":true}"#,
        ],
        modes: ALL,
        fixture: Fixture::Chrome,
    },
    Case {
        name: "direct-truncated",
        args: &[
            "__direct",
            "cdp",
            "--chrome",
            "{chrome}",
            "--target",
            "page",
            "Runtime.evaluate",
            r#"{"expression":"'ab'.repeat(100)","returnByValue":true}"#,
            "--max-chars",
            "100",
        ],
        modes: ALL,
        fixture: Fixture::Chrome,
    },
    Case {
        name: "direct-page-method-on-browser",
        args: &[
            "__direct",
            "cdp",
            "--chrome",
            "{chrome}",
            "Runtime.evaluate",
            r#"{"expression":"1"}"#,
        ],
        modes: ERR,
        fixture: Fixture::Chrome,
    },
];

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Compares `actual` with the golden file, or rewrites it when updating.
fn check(path: &Path, actual: &str, failures: &mut Vec<String>) {
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, actual).unwrap();
        return;
    }
    match std::fs::read_to_string(path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => {
            eprintln!(
                "--- {} (expected)\n{expected}\n--- (actual)\n{actual}",
                path.display()
            );
            failures.push(path.display().to_string());
        }
        Err(e) => failures.push(format!("{}: {e}", path.display())),
    }
}

fn finish(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "golden output differs (UPDATE_GOLDEN=1 accepts it): {failures:#?}"
    );
}

/// Machine-specific strings and their placeholders, longest first so a
/// path is replaced before a prefix of it.
struct Normalizer(Vec<(String, &'static str)>);

impl Normalizer {
    fn new() -> Self {
        let out = cli().args(["-V", "--json"]).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let sha = v["data"]["git_sha"].as_str().unwrap().to_owned();
        Self(vec![
            (sha, "<sha>"),
            (env!("CARGO_PKG_VERSION").to_owned(), "<version>"),
        ])
    }

    fn with(&self, from: &Path, to: &'static str) -> Self {
        let mut pairs = self.0.clone();
        pairs.push((from.display().to_string(), to));
        pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        Self(pairs)
    }

    fn apply(&self, text: &str) -> String {
        self.0
            .iter()
            .fold(text.to_owned(), |text, (from, to)| text.replace(from, to))
    }
}

/// Runs every mode of `case`; `None` if it needs a Chrome we do not have.
fn run_case(case: &Case, base: &Normalizer) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    let mut norm = base.with(dir.path(), "<tmp>");
    let mut cwd = None;
    let chrome = match case.fixture {
        Fixture::None => None,
        Fixture::Chrome => {
            let chrome = chrome_path()?;
            norm = norm.with(&chrome, "<chrome>");
            Some(chrome)
        }
        Fixture::SandboxlessChrome => Some(fake_chrome(
            &dir,
            "echo '[1:1:0927/101010.1:FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox! \
             If you are running on Ubuntu 23.10+ ...' >&2\nexit 1",
        )),
        Fixture::SilentChrome => Some(fake_chrome(&dir, "exec sleep 30")),
        Fixture::BadConfig => {
            std::fs::write(dir.path().join(".chrome-cdp.toml"), "sesion = \"shop\"\n").unwrap();
            cwd = Some(dir.path().to_owned());
            None
        }
    };

    let mut out = String::new();
    for mode in case.modes {
        let mut args: Vec<String> = case
            .args
            .iter()
            .map(|a| match &chrome {
                Some(chrome) => a.replace("{chrome}", &chrome.display().to_string()),
                None => (*a).to_owned(),
            })
            .collect();
        if !mode.is_empty() {
            args.push((*mode).to_owned());
        }
        let mut cmd: Command = cli();
        cmd.args(&args);
        if let Some(cwd) = &cwd {
            cmd.current_dir(cwd);
        }
        let result = cmd.output().unwrap();
        let shown: Vec<_> = args.iter().map(|a| shell_words(a)).collect();
        let _ = writeln!(out, "$ chrome-cdp {}", shown.join(" ").trim_end());
        let _ = writeln!(out, "exit: {}", result.status.code().unwrap());
        for (name, bytes) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
            if !bytes.is_empty() {
                let _ = write!(out, "--- {name}\n{}", String::from_utf8_lossy(bytes));
                if !bytes.ends_with(b"\n") {
                    out.push_str("\n(no newline at end)\n");
                }
            }
        }
        out.push('\n');
    }
    Some(norm.apply(&out))
}

/// Quotes an argument for display the way a shell user would type it.
fn shell_words(arg: &str) -> String {
    protocol::format::shell_quote(arg).into_owned()
}

#[test]
fn command_output_matches_golden_files() {
    let base = Normalizer::new();
    let mut failures = Vec::new();
    for case in CASES {
        let Some(actual) = run_case(case, &base) else {
            continue;
        };
        check(
            &golden_dir().join(format!("{}.txt", case.name)),
            &actual,
            &mut failures,
        );
    }
    finish(&failures);
}

#[test]
fn schemas_match_golden_files() {
    let mut failures = Vec::new();
    for entry in protocol::schema::COMMANDS {
        let out = cli()
            .args(["schema"])
            .args(entry.name.split(' '))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", entry.name);
        let file = format!("{}.json", entry.name.replace(' ', "_"));
        check(
            &golden_dir().join("schema").join(file),
            &String::from_utf8(out.stdout).unwrap(),
            &mut failures,
        );
    }
    finish(&failures);
}

/// Every golden file belongs to a case or schema, so none go stale.
#[test]
fn no_stale_golden_files() {
    let mut expected: Vec<PathBuf> = CASES
        .iter()
        .map(|c| golden_dir().join(format!("{}.txt", c.name)))
        .collect();
    expected.extend(protocol::schema::COMMANDS.iter().map(|c| {
        golden_dir()
            .join("schema")
            .join(format!("{}.json", c.name.replace(' ', "_")))
    }));
    for dir in [golden_dir(), golden_dir().join("schema")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_file() {
                assert!(
                    expected.contains(&path),
                    "stale golden file {}",
                    path.display()
                );
            }
        }
    }
    let mut names: Vec<_> = CASES.iter().map(|c| c.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CASES.len(), "case names must be unique");
}

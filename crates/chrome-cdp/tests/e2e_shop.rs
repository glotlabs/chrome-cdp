//! `examples/e2e-shop` (plan 11, milestone 11.10), the acceptance suite
//! of plan 11: its scenarios pass against the mocks with `--jobs 4` and
//! against the real API (`fixtures::Shop`, `--backend local`), the mocks
//! match that API (`--verify-mocks`), its deliberately failing fixture
//! gets the unmatched-request diagnostics, and `fmt --check` and `lint`
//! pass on it.
//!
//! Needs a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set). Each test copies the example to a
//! temp dir and moves its ports (4173 for the mocks' static server, 4174
//! for the real API) to free ones.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::{Sandbox, chrome_path, stderr, stdout};
use fixtures::Shop;
use serde_json::Value;

/// A copy of the example, with its real API running.
struct Example {
    sandbox: Sandbox,
    root: PathBuf,
}

impl Example {
    fn new(sandbox: Sandbox) -> Self {
        let root = sandbox.dir.path().join("e2e-shop");
        copy_dir(&example(), &root);
        // The real backend: already running, so the run reuses it rather
        // than building the fixture server with cargo.
        let real = Shop::start(root.join("dist"), 0).unwrap();
        let real_port = real.url().rsplit_once(':').unwrap().1.to_owned();
        let config = root.join("chrome-cdp.toml");
        let text = std::fs::read_to_string(&config)
            .unwrap()
            .replace("4173", &free_port().to_string())
            .replace("4174", &real_port);
        std::fs::write(&config, text).unwrap();
        Self { sandbox, root }
    }

    fn cli(&self, args: &[&str]) -> Output {
        common::run(self.sandbox.cli().current_dir(&self.root).args(args))
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.cli(&[args, &["--json"]].concat());
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}{}", stdout(&out), stderr(&out)))
    }
}

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/e2e-shop")
}

/// Copies the example, less what a checkout does not have.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "node_modules" || name == ".results" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `(passed, failed, skipped)` of a `--json` run.
fn counts(value: &Value) -> (u64, u64, u64) {
    let data = if value["ok"] == Value::Bool(true) {
        &value["data"]
    } else {
        &value["error"]["details"]
    };
    let n = |k: &str| data[k].as_u64().unwrap_or_default();
    (n("passed"), n("failed"), n("skipped"))
}

#[test]
fn the_shop_passes_against_the_mocks() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    let value = example.json(&["test", "--jobs", "4"]);
    assert_eq!(counts(&value), (17, 0, 0), "{value:#}");
    assert!(example.sandbox.daemons().is_empty());
}

#[test]
fn the_same_scenarios_pass_against_the_real_api_and_the_mocks_match_it() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    let out = example.cli(&["test", "--backend", "local", "--verify-mocks"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    // Only the @backend mock tests are skipped.
    assert!(text.contains("17 tests: 12 passed, 5 skipped ("), "{text}");
    let skipped: Vec<&str> = text.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(skipped.len(), 5, "{text}");
    assert!(
        skipped
            .iter()
            .all(|l| l.ends_with("(skipped: @backend mock; this run's backend is local)")),
        "{skipped:?}"
    );
    // The page's requests (the WebSocket too) and the http arrange steps
    // meet the mocks, and the ids the real API gave carry over.
    assert!(text.contains(" mocks; no drift\n"), "{text}");
}

#[test]
fn an_unmatched_request_names_the_closest_mock() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    let out = example.cli(&["test", "e2e/fixtures/unmatched.cdp"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("    error: unmatched request: GET http://localhost:"),
        "{text}"
    );
    assert!(
        text.contains(
            "      closest mock: base.toml:16 \"list products\": path differs: expects /api/products, got /api/prodcts\n"
        ),
        "{text}"
    );
    assert!(text.contains("/api/prodcts --mocks base\n"), "{text}");
    let report = example
        .root
        .join("e2e/.results/e2e/fixtures/unmatched.cdp/asks-for-a-path-no-mock-answers/report.md");
    let report = std::fs::read_to_string(report).unwrap();
    assert!(
        report.contains("← unmatched in mock set base (fail); closest mock: base.toml:16"),
        "{report}"
    );
}

#[test]
fn the_shop_is_formatted_and_lints_clean() {
    let example = Example::new(Sandbox::new());
    let out = example.cli(&["fmt", "--check"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = example.cli(&["lint"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).ends_with(" files: 0 errors, 0 warnings\n"),
        "{}",
        stdout(&out)
    );
}

//! `examples/e2e-shop` (plan 11, milestone 11.10), the acceptance suite
//! of plan 11: its scenarios pass against the mocks with `--jobs 4`,
//! against the real API and payment provider (`fixtures::Shop`,
//! `--backend local`) and against the real API with the provider mocked
//! (`--backend hybrid`, plan 19.3), the mocks match that API
//! (`--verify-mocks`), its deliberately failing fixture
//! gets the unmatched-request diagnostics, its mocks fit its OpenAPI
//! spec and a run names the operations they miss (plan 19.4), and `fmt
//! --check` and `lint` pass on it. Its committed fingerprints are what a run writes, and a
//! redesign of it (`testdata/heal/redesign.toml`) is healed (plan 18.2).
//!
//! Needs a real Chrome (`CHROME_PATH`; skipped without it unless
//! `CHROME_CDP_REQUIRE_CHROME` is set). Each test copies the example to a
//! temp dir and moves its ports (4173 for the mocks' static server, 4174
//! for the real API, 4175 for the payment provider) to free ones.

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
        let port = |url: &str| url.rsplit_once(':').unwrap().1.to_owned();
        let (real_port, payments_port) = (port(real.url()), port(real.payments()));
        for file in ["chrome-cdp.toml", "dist/config.js"] {
            let path = root.join(file);
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("4173", &free_port().to_string())
                .replace("4174", &real_port)
                .replace("4175", &payments_port);
            std::fs::write(&path, text).unwrap();
        }
        Self { sandbox, root }
    }

    fn cli(&self, args: &[&str]) -> Output {
        // Fingerprints are written unless CI is set.
        common::run(
            self.sandbox
                .cli()
                .current_dir(&self.root)
                .env_remove("CI")
                .args(args),
        )
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
    assert_eq!(counts(&value), (18, 0, 0), "{value:#}");
    assert!(example.sandbox.daemons().is_empty());
    // The run writes the fingerprints the example has, byte for byte.
    let committed = crate::example().join("e2e/fingerprints");
    let written = example.root.join("e2e/fingerprints");
    let names = files(&committed, &committed);
    assert_eq!(names.len(), 7, "{names:?}");
    assert_eq!(files(&written, &written).len(), names.len());
    for name in names {
        assert_eq!(
            std::fs::read_to_string(written.join(&name)).unwrap(),
            std::fs::read_to_string(committed.join(&name)).unwrap(),
            "e2e/fingerprints/{name} differs from what a run writes: run `chrome-cdp test` in examples/e2e-shop"
        );
    }
}

/// Applies `testdata/heal/redesign.toml` to the example's `dist/` at
/// `root`.
fn redesign(root: &Path) {
    #[derive(serde::Deserialize)]
    struct Substitution {
        from: String,
        to: String,
        count: Option<usize>,
    }
    #[derive(serde::Deserialize)]
    struct Redesign {
        js: Vec<Substitution>,
        css: Vec<Substitution>,
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/heal/redesign.toml");
    let redesign: Redesign = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (extension, substitutions) in [("js", redesign.js), ("css", redesign.css)] {
        let assets = root.join("dist/assets");
        let file = std::fs::read_dir(&assets)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == extension))
            .unwrap();
        let mut text = std::fs::read_to_string(&file).unwrap();
        for s in substitutions {
            let found = text.matches(&s.from).count();
            assert_eq!(
                found,
                s.count.unwrap_or(1),
                "testdata/heal/redesign.toml: `{}` is in the shop's {extension} {found} times; \
                 update it for the rebuilt dist/",
                s.from
            );
            text = text.replace(&s.from, &s.to);
        }
        std::fs::write(&file, text).unwrap();
    }
}

/// The results of a `--json` run, by test name.
fn tests_by_name(value: &Value) -> std::collections::BTreeMap<String, Value> {
    let data = if value["ok"] == Value::Bool(true) {
        &value["data"]
    } else {
        &value["error"]["details"]
    };
    data["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_owned(), t.clone()))
        .collect()
}

/// The test whose element the redesign removed.
const REMOVED: &str = "an unknown product is not found";

#[test]
fn a_redesign_is_healed() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    redesign(&example.root);
    let run = ["test", "--jobs", "4", "--timeout", "3s"];

    // Without --heal: the tests fail, and each names the element its
    // broken locator's fingerprint matches now.
    let value = example.json(&run);
    assert_eq!(counts(&value), (6, 12, 0), "{value:#}");
    for (name, test) in tests_by_name(&value) {
        if test["status"] != "failed" {
            continue;
        }
        let likely = &test["error"]["likely"];
        let level = if name == REMOVED { "low" } else { "high" };
        assert_eq!(likely["level"], level, "{name}: {test:#}");
        let label = if name == REMOVED {
            "closest element now"
        } else {
            "likely the same element now"
        };
        assert!(
            test["error"]["error"]["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["label"] == label),
            "{name}: {test:#}"
        );
        let report = std::fs::read_to_string(test["error"]["report"].as_str().unwrap()).unwrap();
        assert!(
            report.contains("\n## Likely the same element\n"),
            "{report}"
        );
    }

    // --heal: every test whose element is still there passes.
    let out = example.cli(&run.iter().copied().chain(["--heal"]).collect::<Vec<_>>());
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("18 tests: 17 passed, 1 failed ("), "{text}");
    assert!(
        text.contains(
            "    healed e2e/scenarios/cart.cdp:5: role=button[name=\"Add to cart\"] -> role=button[name=\"Add to basket\"] (0.84: same testid, role, container and tag; similar name)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("      closest element now: link \""),
        "{text}"
    );
    assert!(text.contains("too low to heal"), "{text}");
    let patch_path = example.root.join("e2e/.results/heal.patch");
    assert!(
        text.contains(&format!(
            "heal.patch: {} (8 locators; --heal --write applies it)\n",
            patch_path.display()
        )),
        "{text}"
    );
    let patch = std::fs::read_to_string(&patch_path).unwrap();
    for line in [
        "+++ b/e2e/scenarios/cart.cdp\n",
        "+  click 'role=button[name=\"Add to basket\"]'\n",
        "+++ b/e2e/steps/sign-in.cdp\n",
        "+click 'role=button[name=\"Sign in now\"]'\n",
        "+  click 'role=button[name=\"Sign in now\"]'\n",
        "+  expect testid=product-price text $89.00\n",
        "+  expect testid=product-price text $79.00\n",
        "+  expect testid=product-price text ${var:keyboard_price}\n",
    ] {
        assert!(patch.contains(line), "{line}: {patch}");
    }
    assert!(!patch.contains("Back to products"), "{patch}");
    assert!(!patch.contains("Place order"), "its locator works: {patch}");
    let removed = std::fs::read_to_string(
        example
            .root
            .join("e2e/.results/products.cdp/an-unknown-product-is-not-found/report.md"),
    )
    .unwrap();
    assert!(removed.contains("Too low to heal"), "{removed}");

    // --heal --write fixes the files; then the suite passes without it.
    let value = example.json(
        &run.iter()
            .copied()
            .chain(["--heal", "--write"])
            .collect::<Vec<_>>(),
    );
    let written = value["error"]["details"]["heal"]["written"]
        .as_array()
        .unwrap();
    assert_eq!(written.len(), 6, "{value:#}");
    let value = example.json(&run);
    assert_eq!(counts(&value), (17, 1, 0), "{value:#}");
    assert_eq!(
        value["error"]["details"]["tests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["status"] == "failed")
            .unwrap()["name"],
        REMOVED
    );
    // The fingerprints follow the new locators; no password anywhere.
    let cart = std::fs::read_to_string(example.root.join("e2e/fingerprints/cart.json")).unwrap();
    assert!(
        cart.contains("\"5:role=button[name=\\\"Add to basket\\\"]\": {"),
        "{cart}"
    );
    assert!(!cart.contains("Add to cart\\\"]\""), "{cart}");
    for file in files(&example.root, &example.root.join("e2e/fingerprints")) {
        let text = std::fs::read_to_string(example.root.join(&file)).unwrap();
        assert!(!text.contains("\"secret\""), "{file}: {text}");
    }
    assert!(!patch.contains("secret"), "{patch}");
}

/// Plan 18.4: `--ui-coverage` lists every page the suite visits, the
/// controls no step acted on and the mocks no request matched; two
/// pages' numbers are counted by hand.
#[test]
fn ui_coverage_names_what_the_suite_never_touched() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    // A mock no page asks for, and a listed page no test visits.
    let base = example.root.join("e2e/mocks/base.toml");
    let mut mocks = std::fs::read_to_string(&base).unwrap();
    mocks.push_str(
        "\n[[mock]]\nname = \"gift cards\"\nmatch = { method = \"GET\", path = \"/api/gift-cards\" }\nrespond = { json = [] }\n",
    );
    std::fs::write(&base, mocks).unwrap();
    let config = example.root.join("chrome-cdp.toml");
    let text = std::fs::read_to_string(&config).unwrap().replacen(
        "[e2e]\n",
        "[e2e]\npages = [\"/products/:id\", \"/account\"]\n",
        1,
    );
    std::fs::write(&config, text).unwrap();

    let value = example.json(&["test", "--jobs", "4", "--ui-coverage"]);
    assert_eq!(counts(&value), (18, 0, 0), "{value:#}");
    let coverage = &value["data"]["ui_coverage"];
    let pages: Vec<&str> = coverage["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["page"].as_str().unwrap())
        .collect();
    assert_eq!(
        pages,
        [
            "/products/:id",
            "/cart",
            "/login",
            "/",
            "/checkout",
            "/orders/:id",
            "/search"
        ]
    );
    assert_eq!(coverage["unvisited"], serde_json::json!(["/account"]));
    let page = |name: &str| {
        let page = coverage["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["page"] == name)
            .unwrap();
        let keys = |acted: bool| -> Vec<&str> {
            page["controls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| (c["acted"].as_u64().unwrap() > 0) == acted)
                .map(|c| c["key"].as_str().unwrap())
                .collect()
        };
        (keys(true), keys(false))
    };
    // By hand: the banner's link "Shop", cart link, search box and button
    // and "Sign out" (or, signed out, "Sign in"), and the page's own.
    let (acted, never) = page("/checkout");
    assert_eq!(
        acted,
        [
            "button \"Sign out\"",
            "textbox \"Card number\"",
            "button \"Place order\"",
            "button \"Try again\""
        ]
    );
    assert_eq!(
        never,
        [
            "link \"Shop\"",
            "link \"Cart (1)\"",
            "searchbox \"Search\"",
            "button \"Search\"",
            "link \"Sign in\" in banner",
            "link \"Sign in\" in main"
        ]
    );
    let (acted, never) = page("/products/:id");
    assert_eq!(
        acted,
        [
            "button \"Add to cart\"",
            "link \"Back to products\"",
            "link \"Cart (1)\""
        ]
    );
    assert_eq!(
        never,
        [
            "link \"Shop\"",
            "link \"Cart (0)\"",
            "searchbox \"Search\"",
            "button \"Search\"",
            "link \"Sign in\"",
            "button \"Sign out\""
        ]
    );
    let unused: Vec<&str> = coverage["mocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["hits"] == 0)
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(unused, ["gift cards"]);
    let file = example.root.join("e2e/.results/ui-coverage.json");
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["pages"], coverage["pages"]);

    let out = example.cli(&["test", "--jobs", "4", "--ui-coverage"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    let from = text.find("ui coverage:").unwrap();
    let to = text.find("18 tests:").unwrap();
    let summary = text[from..to].replace(&example.root.display().to_string(), "<root>");
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/ui-coverage/shop.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| !v.is_empty() && v != "0") {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &summary).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&golden).unwrap_or_default(),
        summary,
        "golden/ui-coverage/shop.txt differs (UPDATE_GOLDEN=1 accepts it)"
    );
}

#[test]
fn the_same_scenarios_pass_against_the_real_api_and_the_mocks_match_it() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    let out = example.cli(&[
        "test",
        "--backend",
        "local",
        "--verify-mocks",
        "--ui-coverage",
    ]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    // Only the tests that need the mocks are skipped.
    assert!(text.contains("18 tests: 12 passed, 6 skipped ("), "{text}");
    let skipped: Vec<&str> = text.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(skipped.len(), 6, "{text}");
    assert!(
        skipped.iter().all(|l| {
            l.ends_with("(skipped: @backend mock; this run's backend is local)")
                || l.ends_with("(skipped: @backend mock hybrid; this run's backend is local)")
        }),
        "{skipped:?}"
    );
    // The page's requests (the WebSocket too) and the http arrange steps
    // meet the mocks, and the ids the real API gave carry over.
    assert!(text.contains(" mocks; no drift\n"), "{text}");
    // --ui-coverage: a real backend's API requests, by path template.
    assert!(text.contains("\napi requests: "), "{text}");
    assert!(text.contains("\napi requests: GET /api/cart ×"), "{text}");
    assert!(text.contains(", DELETE /api/cart/:id ×"), "{text}");
}

/// Plan 19.3: the real API with the payment provider mocked, so a
/// declined card (a mock) can be tested against the real shop.
#[test]
fn the_scenarios_pass_against_the_real_api_with_the_provider_mocked() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    let out = example.cli(&["test", "--backend", "hybrid"]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    assert!(text.contains("18 tests: 13 passed, 5 skipped ("), "{text}");
    assert!(
        text.contains("✓ e2e/scenarios/checkout.cdp:27 a declined card is not charged ("),
        "{text}"
    );
    assert!(
        text.lines()
            .filter(|l| l.starts_with("- "))
            .all(|l| l.ends_with("(skipped: @backend mock; this run's backend is hybrid)")),
        "{text}"
    );
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
    assert!(
        text.contains("/api/prodcts --mocks base,third-party/payments\n"),
        "{text}"
    );
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
fn a_run_says_which_spec_operations_have_no_mock() {
    let Some(chrome) = chrome_path() else { return };
    let example = Example::new(Sandbox::with_chrome(&chrome));
    // The mocks fit api/openapi.yaml and answer all of it.
    let check = example.json(&["mock", "check"]);
    assert_eq!(check["data"]["errors"], 0, "{check:#}");
    assert_eq!(check["data"]["warnings"], 0, "{check:#}");
    // Without "sign out", POST /logout has no mock: one line in a run
    // (errors/api-down's `/api/**` answers it, and stands for nothing).
    let base = example.root.join("e2e/mocks/base.toml");
    let text = std::fs::read_to_string(&base).unwrap();
    let start = text.find("[[mock]]\nname = \"sign out\"").unwrap();
    let end = start + text[start + 1..].find("[[mock]]").unwrap() + 1;
    std::fs::write(&base, format!("{}{}", &text[..start], &text[end..])).unwrap();
    let out = example.cli(&["test", "e2e/scenarios/products.cdp"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        err.contains("warning: 1 spec operation has no mock: POST /logout (logout)\n"),
        "{err}"
    );
    let check = example.json(&["mock", "check"]);
    assert_eq!(
        check["data"]["uncovered"][0], "POST /logout (logout)",
        "{check:#}"
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

/// The paths below `path` (itself if it is a file), relative to `root`
/// with `/` separators.
fn files(root: &Path, path: &Path) -> Vec<String> {
    if path.is_file() {
        let relative = path.strip_prefix(root).unwrap();
        let parts: Vec<_> = relative.iter().map(|p| p.to_string_lossy()).collect();
        return vec![parts.join("/")];
    }
    std::fs::read_dir(path)
        .unwrap()
        .flat_map(|entry| files(root, &entry.unwrap().path()))
        .collect()
}

#[test]
fn the_committed_dist_is_built_from_the_current_sources() {
    // What vite.config.js's `sourcesHash` computes: FNV-1a over each
    // source's path, a NUL, its bytes and a NUL, in path order.
    let root = example();
    let mut paths: Vec<String> = [
        "index.html",
        "package-lock.json",
        "public",
        "src",
        "vite.config.js",
    ]
    .iter()
    .flat_map(|p| files(&root, &root.join(p)))
    .collect();
    paths.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for path in paths {
        let bytes = std::fs::read(root.join(&path)).unwrap();
        for byte in path.bytes().chain([0]).chain(bytes).chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    let recorded = std::fs::read_to_string(root.join("dist/sources.txt")).unwrap();
    assert_eq!(
        recorded.trim(),
        format!("{hash:016x}"),
        "examples/e2e-shop/dist is out of date: run `npm ci && npm run build` there"
    );
}

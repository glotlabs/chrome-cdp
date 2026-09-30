//! `expect --a11y`'s rules against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site's `a11y/` pages (plan 17, milestone 17.3).
//!
//! Each rule's page has passing elements and failing ones marked
//! `data-fail`: the rule must flag exactly the failing ones, and no other
//! rule may flag anything. `a11y/clean.html` passes every rule. The rules
//! of plan 09 are tested in `expect.rs`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    A11yLevel, Engine, EvalRequest, EvalWorld, ExpectRequest, Expectation, NavigateRequest,
    Navigation, OpenRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::{ErrorInfo, ErrorKind};
use serde_json::Value;

const TIMEOUT_MS: u64 = 30_000;

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

async fn goto(engine: &Engine, url: &str) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait: WaitUntil::Load,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// Runs `script` in the page, with `el` bound to `locator`'s element.
async fn eval(engine: &Engine, script: &str, locator: Option<&str>) -> Value {
    let out = engine
        .eval(EvalRequest {
            script: script.to_owned(),
            locator: locator.map(str::to_owned),
            frame: None,
            world: EvalWorld::Main,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    out.data["value"].clone()
}

/// `expect --a11y` at `level`: `Ok` when it passes.
async fn a11y(engine: &Engine, level: A11yLevel) -> Result<(), ErrorInfo> {
    Box::pin(engine.expect(ExpectRequest {
        expectation: Expectation::A11y { scope: None, level },
        not: false,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }))
    .await
    .map(|_| ())
}

/// The rules of plan 17's milestone 17.3 (and `color-contrast`'s
/// opacity and text shadows), their level, and how many elements their
/// page has that fail.
const RULES: &[(&str, &str, usize)] = &[
    ("aria-valid-role", "A", 3),
    ("aria-valid-attr", "A", 2),
    ("aria-valid-attr-value", "A", 5),
    ("aria-required-attr", "A", 4),
    ("aria-required-children", "A", 2),
    ("aria-required-parent", "A", 4),
    ("duplicate-id-referenced", "A", 2),
    ("list", "A", 2),
    ("table-headers", "A", 2),
    ("heading-order", "A", 2),
    ("empty-heading", "A", 3),
    ("color-contrast", "AA", 4),
    ("autocomplete-valid", "AA", 4),
    ("target-size", "AA", 3),
    ("focus-visible", "AA", 3),
];

#[tokio::test]
async fn each_rule_flags_exactly_its_failing_elements() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "a11y-rules").await;
    for &(rule, level, failing) in RULES {
        goto(&engine, &site.url(&format!("/a11y/{rule}.html"))).await;
        let marked = eval(
            &engine,
            "document.querySelectorAll('[data-fail]').length",
            None,
        )
        .await;
        assert_eq!(marked, failing, "{rule}: the page's data-fail elements");

        let err = a11y(&engine, A11yLevel::AA).await.unwrap_err();
        let text = err.to_text();
        assert_eq!(err.kind, ErrorKind::ExpectFailed, "{rule}: {text}");
        let details = err.details.unwrap();
        let violations = details["violations"].as_array().unwrap();
        let rules: Vec<&str> = violations
            .iter()
            .map(|v| v["rule"].as_str().unwrap())
            .collect();
        assert_eq!(rules, [rule], "{rule}: only its rule fails: {text}");
        let v = &violations[0];
        assert_eq!(v["level"], level, "{rule}");
        assert_eq!(v["count"], failing, "{rule}: {text}");
        let elements = v["elements"].as_array().unwrap();
        assert_eq!(elements.len(), failing, "{rule}: {text}");
        for element in elements {
            let reference = element["ref"].as_str().unwrap();
            let fails = eval(&engine, "el.hasAttribute('data-fail')", Some(reference)).await;
            assert_eq!(fails, true, "{rule}: {element} is not marked data-fail");
        }

        // Level A leaves the AA rules out.
        let at_a = a11y(&engine, A11yLevel::A).await;
        assert_eq!(at_a.is_err(), level == "A", "{rule} at level A");
    }
    engine.close().await;
}

#[tokio::test]
async fn every_failing_rule_gets_a_line() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "a11y-many").await;
    goto(&engine, &site.url("/a11y/many.html")).await;
    let err = a11y(&engine, A11yLevel::AA).await.unwrap_err();
    let text = err.to_text();
    let rules = [
        "aria-valid-role",
        "aria-valid-attr",
        "aria-required-attr",
        "list",
        "empty-heading",
        "autocomplete-valid",
        "target-size",
    ];
    for rule in rules {
        assert!(text.contains(&format!("  {rule}: ")), "{rule}: {text}");
    }
    // 10 lines of elements: one per rule, and the first rules' next ones.
    let elements = err
        .notes
        .iter()
        .filter(|n| rules.contains(&n.label.as_str()) && !n.text.starts_with('…'))
        .count();
    assert_eq!(elements, 10, "{text}");
    assert!(text.contains("  aria-valid-attr: … and 1 more\n"), "{text}");
    assert!(text.contains("  list: … and 2 more\n"), "{text}");
    assert!(
        text.contains("  more: the rest are in --json's details\n"),
        "{text}"
    );
    engine.close().await;
}

#[tokio::test]
async fn one_main_landmark() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "a11y-main").await;
    for (page, why) in [
        ("none", "the page has no main landmark"),
        ("two", "the page has 2 visible main landmarks"),
    ] {
        goto(
            &engine,
            &site.url(&format!("/a11y/landmark-one-main-{page}.html")),
        )
        .await;
        let err = a11y(&engine, A11yLevel::A).await.unwrap_err();
        let details = err.details.unwrap();
        let violations = details["violations"].as_array().unwrap();
        assert_eq!(violations.len(), 1, "{page}: {violations:?}");
        assert_eq!(violations[0]["rule"], "landmark-one-main");
        assert_eq!(violations[0]["page"], why);
    }
    engine.close().await;
}

#[tokio::test]
async fn a_clean_page_passes_every_rule() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "a11y-clean").await;
    goto(&engine, &site.url("/a11y/clean.html")).await;
    a11y(&engine, A11yLevel::AA)
        .await
        .unwrap_or_else(|e| panic!("{}", e.to_text()));
    engine.close().await;
}

#[tokio::test]
async fn focus_goes_back_where_it_was() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "a11y-focus").await;
    goto(&engine, &site.url("/a11y/focus-visible.html")).await;
    eval(&engine, "document.querySelector('input').focus()", None).await;
    a11y(&engine, A11yLevel::AA).await.unwrap_err();
    let focused = eval(&engine, "document.activeElement.tagName", None).await;
    assert_eq!(focused, "INPUT");

    // Checking an iframe's elements moves the page's focus into it; it
    // comes back.
    goto(&engine, &site.url("/expect/a11y-frames.html")).await;
    a11y(&engine, A11yLevel::AA).await.unwrap_err();
    let focused = eval(&engine, "document.activeElement.tagName", None).await;
    assert_eq!(focused, "BODY");
    engine.close().await;
}

/// `docs/accessibility.md`'s table has every rule of `a11y.ts`, with its
/// WCAG criterion and level, and no other (the docs are hand-written:
/// the rules are TypeScript).
#[test]
fn the_docs_list_every_rule() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source =
        std::fs::read_to_string(root.join("crates/engine/src/injected/src/a11y.ts")).unwrap();
    let docs = std::fs::read_to_string(root.join("docs/accessibility.md")).unwrap();
    let rule = regex::Regex::new(r"id: '([a-z-]+)', wcag: '([\d.]+)', level: '(A{1,2})'").unwrap();
    let rules: Vec<String> = rule
        .captures_iter(&source)
        .map(|c| format!("| `{}` | {} | {} |", &c[1], &c[2], &c[3]))
        .collect();
    assert_eq!(rules.len(), 26, "the rules in a11y.ts");
    let row = regex::Regex::new(r"(?m)^(\| `[a-z-]+` \| [\d.]+ \| A{1,2} \|)").unwrap();
    let documented: Vec<&str> = row
        .captures_iter(&docs)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    let mut expected: Vec<&str> = rules.iter().map(String::as_str).collect();
    let mut found = documented.clone();
    expected.sort_unstable();
    found.sort_unstable();
    assert_eq!(found, expected, "docs/accessibility.md's rule table");
}

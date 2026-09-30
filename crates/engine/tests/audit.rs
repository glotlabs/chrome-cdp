//! `audit` and `expect --audit` against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture site's `audit/` pages (plan 17, milestone 17.4).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    AuditBaseline, AuditRequest, Engine, NavigateRequest, Navigation, OpenRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{
    AuditBaselineFile, AuditBaselineResult, AuditData, ExpectData, Finding, Severity,
};
use protocol::{ErrorKind, Output};

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
    start_with(path, name, OpenRequest::default()).await
}

async fn start_with(path: &Path, name: &str, open: OpenRequest) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..open
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
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// Every category, every severity.
fn request() -> AuditRequest {
    AuditRequest {
        tab: None,
        all_tabs: false,
        locator: None,
        categories: Vec::new(),
        min_severity: Severity::Info,
        baseline: None,
        show_accepted: false,
        expect: false,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn audit(engine: &Engine, req: AuditRequest) -> (Output, AuditData) {
    let out = engine
        .audit(req)
        .await
        .unwrap_or_else(|e| panic!("audit: {}", e.to_text()));
    let data: AuditData = serde_json::from_value(out.data.clone()).unwrap();
    (out, data)
}

/// Audits until `done` holds for the findings (issues and console
/// entries arrive while and after a page loads), or fails after a while
/// with what it saw.
async fn until(
    engine: &Engine,
    req: &AuditRequest,
    what: &str,
    done: impl Fn(&[Finding]) -> bool,
) -> (Output, AuditData) {
    let start = std::time::Instant::now();
    loop {
        let (out, data) = audit(engine, req.clone()).await;
        if done(&data.findings) {
            return (out, data);
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{what}: never got there; saw:\n{}",
            out.text
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn keys(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.key.as_str()).collect()
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("chrome-cdp-audit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn baseline(path: &Path, update: bool, create: bool) -> AuditBaseline {
    AuditBaseline {
        name: path.display().to_string(),
        path: path.to_path_buf(),
        update,
        create,
        dir: None,
    }
}

/// The clean page has nothing in any category, at any severity; each
/// other page has its one known finding, with its severity.
#[tokio::test]
async fn one_known_finding_per_category_and_none_on_a_clean_page() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "audit-categories").await;
    goto(&engine, &site.url("/audit/clean.html")).await;
    let (out, data) = audit(&engine, request()).await;
    assert!(data.findings.is_empty(), "{}", out.text);
    assert_eq!(data.categories.len(), 7);
    assert_eq!(data.hidden, 0);
    assert!(out.text.contains("— no findings"), "{}", out.text);
    let cases = [
        (
            "issues",
            "issues/deprecation-unload-handler/self/audit/unload.js:2",
            Severity::Warning,
        ),
        ("a11y", "a11y/image-alt/img", Severity::Error),
        (
            "layout",
            "layout/clipped/text=\"A label far too long for its box\"",
            Severity::Warning,
        ),
        (
            "console",
            "console/console-error/self/audit/console.js:3 Cart failed to load",
            Severity::Warning,
        ),
        (
            "network",
            "network/http-error/GET self/audit/missing.json",
            Severity::Error,
        ),
        (
            "security",
            "security/no-csp//audit/security.html",
            Severity::Warning,
        ),
        (
            "meta",
            "meta/no-viewport//audit/meta.html",
            Severity::Warning,
        ),
    ];
    for (page, key, severity) in cases {
        goto(&engine, &site.url(&format!("/audit/{page}.html"))).await;
        let (out, data) = until(&engine, &request(), page, |f| !f.is_empty()).await;
        assert_eq!(keys(&data.findings), [key], "{page}:\n{}", out.text);
        let found = &data.findings[0];
        assert_eq!(found.category, page);
        assert_eq!(found.severity, severity, "{page}");
        assert!(
            out.text.contains(&format!("{page}/{}", found.rule)),
            "{}",
            out.text
        );
    }
    // The a11y finding names its element with a ref and its WCAG rule.
    goto(&engine, &site.url("/audit/a11y.html")).await;
    let (out, data) = audit(&engine, request()).await;
    let image = &data.findings[0];
    assert_eq!(image.wcag.as_deref(), Some("1.1.1 A"));
    // No locator finds it alone (it has no name): its role is its key.
    assert_eq!(image.target.locator, None);
    assert!(image.target.reference.is_some());
    assert!(out.text.contains("(WCAG 1.1.1 A)"), "{}", out.text);
}

/// Keys hold across origins (so ports) and reloads; `--min-severity`
/// hides the less severe, and `--category` picks.
#[tokio::test]
async fn keys_are_stable_across_ports_and_reloads() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "audit-keys").await;
    let all_there = |f: &[Finding]| {
        ["issues", "console", "network"]
            .iter()
            .all(|c| f.iter().any(|x| x.category == *c))
    };
    goto(&engine, &site.url("/audit/golden.html")).await;
    let (out, first) = until(&engine, &request(), "golden", all_there).await;
    let mut first_keys = keys(&first.findings);
    first_keys.sort_unstable();
    for c in ["a11y", "layout", "security", "meta"] {
        assert!(
            first.findings.iter().any(|f| f.category == c),
            "{c}:\n{}",
            out.text
        );
    }
    goto(&engine, &site.url_b("/audit/golden.html")).await;
    let (_, other) = until(&engine, &request(), "golden on b", all_there).await;
    let mut other_keys = keys(&other.findings);
    other_keys.sort_unstable();
    assert_eq!(first_keys, other_keys);
    goto(&engine, &site.url_b("/audit/golden.html")).await;
    let (_, again) = until(&engine, &request(), "reloaded", all_there).await;
    let mut again_keys = keys(&again.findings);
    again_keys.sort_unstable();
    assert_eq!(first_keys, again_keys);
    assert!(first.findings.iter().all(|f| !f.key.contains("127.0.0.1")
        && !f.key.contains("localhost")
        && !f.key.contains(" e")));
    // Errors only: the rest is counted as hidden.
    let (out, errors) = audit(
        &engine,
        AuditRequest {
            min_severity: Severity::Error,
            ..request()
        },
    )
    .await;
    assert!(
        errors
            .findings
            .iter()
            .all(|f| f.severity == Severity::Error)
    );
    assert_eq!(errors.hidden + errors.findings.len(), again.findings.len());
    assert!(out.text.contains("hidden)"), "{}", out.text);
    let (_, picked) = audit(
        &engine,
        AuditRequest {
            categories: vec!["meta,security".into()],
            ..request()
        },
    )
    .await;
    assert_eq!(picked.categories, ["security", "meta"]);
    assert!(
        picked
            .findings
            .iter()
            .all(|f| f.category == "security" || f.category == "meta")
    );
}

/// A LOCATOR scopes a11y and layout (and security's forms); the
/// page-wide categories stay out unless asked for.
#[tokio::test]
async fn a_locator_scopes_what_can_be_scoped() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "audit-scope").await;
    goto(&engine, &site.url("/audit/golden.html")).await;
    let (out, data) = audit(
        &engine,
        AuditRequest {
            locator: Some("#label".into()),
            ..request()
        },
    )
    .await;
    assert_eq!(data.categories, ["a11y", "layout", "security"]);
    assert_eq!(
        keys(&data.findings),
        ["layout/clipped/text=\"A label far too long for its box\""],
        "{}",
        out.text
    );
    assert!(out.text.contains("inside #label"), "{}", out.text);
}

/// On a host that is not this machine: an http: page with a password
/// field, a form that posts to http:, and a session cookie without
/// HttpOnly or SameSite. On https: no HSTS and a cookie without Secure.
/// A CSP header counts like a meta tag.
#[tokio::test]
async fn security_of_the_scheme_forms_cookies_and_headers() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start_with(
        &path,
        "audit-security",
        OpenRequest {
            ignore_https_errors: Some(true),
            chrome_args: vec![
                "--host-resolver-rules=MAP *.audit.test 127.0.0.1, MAP audit.test 127.0.0.1"
                    .to_owned(),
            ],
            ..OpenRequest::default()
        },
    )
    .await;
    let port = |origin: &str| origin.rsplit(':').next().unwrap().to_owned();
    let http = format!("http://audit.test:{}", port(site.a()));
    goto(&engine, &format!("{http}/audit/login.html")).await;
    let security = AuditRequest {
        categories: vec!["security".into()],
        ..request()
    };
    let (out, data) = audit(&engine, security.clone()).await;
    assert_eq!(
        keys(&data.findings),
        [
            "security/form-http-action/role=form[name=\"Log in\"]",
            "security/http-password//audit/login.html",
        ],
        "{}",
        out.text
    );
    assert!(data.findings.iter().all(|f| f.severity == Severity::Error));
    let form = &data.findings[0];
    assert!(form.target.reference.is_some());
    assert_eq!(
        form.target.url.as_deref(),
        Some("http://api.audit.test/login")
    );
    // A session cookie scripts can read, without SameSite.
    let cookie = "sid%3Dabc%3B%20Path%3D%2F";
    goto(
        &engine,
        &format!("{http}/audit/clean.html?set-cookie={cookie}"),
    )
    .await;
    let (out, data) = audit(&engine, security.clone()).await;
    assert_eq!(
        keys(&data.findings),
        [
            "security/cookie-no-httponly/cookie sid@audit.test",
            "security/cookie-no-samesite/cookie sid@audit.test",
            "security/http-page//audit/clean.html",
        ],
        "{}",
        out.text
    );
    assert!(data.findings.iter().all(|f| f.severity == Severity::Warning));
    // https: no HSTS (info), the cookie there without Secure.
    let https = format!("https://audit.test:{}", port(site.https()));
    goto(
        &engine,
        &format!("{https}/audit/clean.html?set-cookie=token%3D1%3B%20HttpOnly%3B%20SameSite%3DLax"),
    )
    .await;
    let (out, data) = audit(&engine, security.clone()).await;
    let found = keys(&data.findings);
    assert!(
        found.contains(&"security/no-hsts//audit/clean.html"),
        "{}",
        out.text
    );
    assert!(
        found.contains(&"security/cookie-no-secure/cookie token@audit.test"),
        "{}",
        out.text
    );
    assert!(
        !found
            .iter()
            .any(|k| k.contains("cookie-no-samesite/cookie token"))
    );
    // The CSP as a header, and HSTS: neither is missing.
    let header = |h: &str| {
        h.replace(' ', "%20")
            .replace(';', "%3B")
            .replace('=', "%3D")
    };
    goto(
        &engine,
        &format!(
            "{https}/audit/security.html?header={}&header={}",
            header("Content-Security-Policy: default-src 'self'"),
            header("Strict-Transport-Security: max-age=60")
        ),
    )
    .await;
    let (out, data) = audit(&engine, security).await;
    let found = keys(&data.findings);
    assert!(
        !found.iter().any(|k| k.starts_with("security/no-csp")),
        "{}",
        out.text
    );
    assert!(
        !found.iter().any(|k| k.starts_with("security/no-hsts")),
        "{}",
        out.text
    );
}

/// A baseline accepts findings by key, with count limits; entries that
/// match nothing are stale; `--update` keeps notes and drops stale ones.
#[tokio::test]
async fn baselines_accept_limit_and_go_stale() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "audit-baselines").await;
    let dir = temp_dir("baselines");
    let file = dir.join("golden.json");
    goto(&engine, &site.url("/audit/golden.html")).await;
    let warnings = AuditRequest {
        min_severity: Severity::Warning,
        ..request()
    };
    let (_, before) = until(&engine, &warnings, "golden", |f| {
        f.iter().any(|x| x.category == "console") && f.iter().any(|x| x.category == "issues")
    })
    .await;
    // No baseline yet: every finding is new.
    let (out, data) = audit(
        &engine,
        AuditRequest {
            baseline: Some(baseline(&file, false, false)),
            ..warnings.clone()
        },
    )
    .await;
    assert_eq!(data.baseline.unwrap().result, AuditBaselineResult::Missing);
    assert_eq!(data.findings.len(), before.findings.len());
    assert!(out.text.contains("--update"), "{}", out.text);
    // --update writes them.
    let (_, data) = audit(
        &engine,
        AuditRequest {
            baseline: Some(baseline(&file, true, false)),
            ..warnings.clone()
        },
    )
    .await;
    assert_eq!(data.baseline.unwrap().result, AuditBaselineResult::Created);
    let mut written: AuditBaselineFile =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written.accepted.len(), before.findings.len());
    // All accepted now: none listed, unless --show-accepted.
    let with = |show_accepted| AuditRequest {
        baseline: Some(baseline(&file, false, false)),
        show_accepted,
        ..warnings.clone()
    };
    let (out, data) = audit(&engine, with(false)).await;
    assert!(data.findings.is_empty(), "{}", out.text);
    assert_eq!(data.accepted, before.findings.len());
    assert!(out.text.contains("accepted)"), "{}", out.text);
    let (out, data) = audit(&engine, with(true)).await;
    assert!(data.findings.iter().all(|f| f.accepted));
    assert!(out.text.contains("(accepted)"), "{}", out.text);
    // A note, a count limit the page goes over, a stale entry.
    for a in &mut written.accepted {
        if a.key.starts_with("meta/no-viewport") {
            a.note = Some("the app is desktop only".into());
        }
        if a.key.starts_with("console/") {
            a.count = Some(1);
        }
    }
    written.accepted.push(protocol::schema::AcceptedFinding {
        key: "security/no-hsts//audit/golden.html".into(),
        count: None,
        note: None,
    });
    std::fs::write(&file, serde_json::to_string_pretty(&written).unwrap()).unwrap();
    // The console error again, in the same page: 2 where 1 is accepted.
    engine
        .eval(engine::EvalRequest {
            script: "retryCart()".into(),
            ..eval_request()
        })
        .await
        .unwrap();
    let (out, data) = until(&engine, &with(false), "the second error", |f| {
        f.iter().any(|x| x.category == "console")
    })
    .await;
    assert_eq!(data.findings.len(), 1, "{}", out.text);
    assert_eq!(data.findings[0].count, 2);
    let state = data.baseline.unwrap();
    assert_eq!(state.stale, ["security/no-hsts//audit/golden.html"]);
    assert!(out.text.contains("stale: 1 finding"), "{}", out.text);
    // --update: the note stays, the count goes up, the stale one goes.
    audit(
        &engine,
        AuditRequest {
            baseline: Some(baseline(&file, true, false)),
            ..warnings.clone()
        },
    )
    .await;
    let updated: AuditBaselineFile =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let viewport = updated
        .accepted
        .iter()
        .find(|a| a.key.starts_with("meta/no-viewport"))
        .unwrap();
    assert_eq!(viewport.note.as_deref(), Some("the app is desktop only"));
    let console = updated
        .accepted
        .iter()
        .find(|a| a.key.starts_with("console/"))
        .unwrap();
    assert_eq!(console.count, Some(2));
    assert!(!updated.accepted.iter().any(|a| a.key.contains("no-hsts")));
    let _ = std::fs::remove_dir_all(&dir);
}

fn eval_request() -> engine::EvalRequest {
    serde_json::from_value(serde_json::json!({
        "script": "",
        "timeout_ms": TIMEOUT_MS,
        "max_chars": 0,
    }))
    .unwrap()
}

/// `expect --audit` fails on a finding the baseline does not accept,
/// passes once it does, writes a missing baseline when allowed and fails
/// without one otherwise (CI).
#[tokio::test]
async fn expect_audit_fails_on_new_findings_only() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "audit-expect").await;
    let dir = temp_dir("expect");
    let file = dir.join("a11y.json");
    goto(&engine, &site.url("/audit/clean.html")).await;
    let expect = AuditRequest {
        expect: true,
        min_severity: Severity::Warning,
        ..request()
    };
    let out = engine.audit(expect.clone()).await.unwrap();
    let data: ExpectData = serde_json::from_value(out.data.clone()).unwrap();
    assert_eq!(data.assertion, "--audit");
    assert!(
        out.text.starts_with("passed: expect --audit: no findings"),
        "{}",
        out.text
    );
    goto(&engine, &site.url("/audit/a11y.html")).await;
    // No baseline: any finding fails, with the report.
    let e = engine.audit(expect.clone()).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ExpectFailed);
    let text = e.to_text();
    assert!(text.contains("1 finding (no baseline)"), "{text}");
    assert!(text.contains("a11y/image-alt"), "{text}");
    assert!(text.contains("--baseline a11y --update"), "{text}");
    // A missing baseline under CI fails; on a local run it is written.
    let missing = engine
        .audit(AuditRequest {
            baseline: Some(baseline(&file, false, false)),
            ..expect.clone()
        })
        .await
        .unwrap_err();
    assert!(
        missing.to_text().contains("there is no baseline yet"),
        "{}",
        missing.to_text()
    );
    assert!(!file.exists());
    let created = engine
        .audit(AuditRequest {
            baseline: Some(baseline(&file, false, true)),
            ..expect.clone()
        })
        .await
        .unwrap();
    assert!(
        created.text.contains("created baseline"),
        "{}",
        created.text
    );
    assert!(file.exists());
    // Accepted now: it passes, and says so.
    let out = engine
        .audit(AuditRequest {
            baseline: Some(baseline(&file, false, false)),
            ..expect.clone()
        })
        .await
        .unwrap();
    assert!(out.text.contains("1 finding, all accepted"), "{}", out.text);
    let data: ExpectData = serde_json::from_value(out.data.clone()).unwrap();
    let audit = data.audit.unwrap();
    assert_eq!(audit.findings, ["a11y/image-alt/img"]);
    assert_eq!(audit.accepted, 1);
    // Fixed: the entry is stale, which a warning names.
    goto(&engine, &site.url("/audit/clean.html")).await;
    let out = engine
        .audit(AuditRequest {
            baseline: Some(baseline(&file, false, false)),
            ..expect.clone()
        })
        .await
        .unwrap();
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("a11y/image-alt/img")),
        "{:?}",
        out.warnings
    );
    // A new finding fails with only the new one listed.
    goto(&engine, &site.url("/audit/meta.html")).await;
    let e = engine
        .audit(AuditRequest {
            baseline: Some(baseline(&file, false, false)),
            ..expect
        })
        .await
        .unwrap_err();
    let text = e.to_text();
    assert!(text.contains("meta/no-viewport"), "{text}");
    assert!(!text.contains("image-alt  "), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

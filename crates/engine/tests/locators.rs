//! The locator engine against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site (plan 04, milestone 4.4): a table-driven test with a row per
//! grammar form, strictness and its `Ambiguous` error, and `wait` with
//! refs, roles and frames.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    ElementState, Engine, LocateOptions, NavigateRequest, Navigation, OpenRequest, SnapshotRequest,
    WaitFor, WaitRequest, WaitUntil, World,
};
use fixtures::Fixtures;
use protocol::schema::{SnapshotData, SnapshotNode, WaitData};
use protocol::{ErrorInfo, ErrorKind};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS);

/// What the tests read of each match.
const IDS: &str = "function () { return this.map((e) => e.id || e.localName); }";

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
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

async fn all(engine: &Engine, locator: &str, options: &LocateOptions) -> Result<Value, ErrorInfo> {
    engine
        .call_on_all(None, locator, options, IDS, TIMEOUT)
        .await
        .map(|c| c.value)
}

async fn one(engine: &Engine, locator: &str) -> Result<Value, ErrorInfo> {
    engine
        .call_on_locator(
            None,
            locator,
            &LocateOptions::default(),
            "function () { return this.id || this.localName; }",
            TIMEOUT,
        )
        .await
        .map(|c| c.value)
}

async fn snapshot(engine: &Engine) -> SnapshotData {
    let output = engine
        .snapshot(SnapshotRequest {
            tab: None,
            interactive: false,
            scope: None,
            depth: None,
            diff: false,
            all_refs: false,
            all: false,
            text_limit: 200,
            no_text: false,
            engine: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    serde_json::from_value(output.data).unwrap()
}

/// The ref of the first node with this role and name.
fn find_ref(nodes: &[SnapshotNode], role: &str, name: &str) -> Option<String> {
    nodes.iter().find_map(|n| {
        if n.role == role && n.name == name {
            n.reference.clone()
        } else {
            find_ref(&n.children, role, name)
        }
    })
}

/// Every grammar form, with what it matches on `locators/index.html`.
#[tokio::test]
async fn every_grammar_form_finds_its_elements() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-locators-forms").await;
    goto(&engine, &site.url("/locators/index.html")).await;
    let table: &[(&str, &[&str])] = &[
        // CSS, bare and prefixed; it pierces open shadow roots.
        ("#email", &["email"]),
        (".item", &["item-1", "item-2", "item-3", "item-4"]),
        ("[placeholder]", &["email", "password"]),
        (
            "css=form#login button",
            &["submit", "submit-disabled", "toggle", "menu"],
        ),
        ("css=#host .card button", &["shadow-button"]),
        ("css=button.deep", &["deep-button"]),
        ("#outer-host .panel .deep", &["deep-button"]),
        ("css=#host button, #h1", &["h1", "shadow-button"]),
        // XPath.
        ("//h1", &["h1"]),
        (
            "xpath=//table//button",
            &["delete-12", "delete-13", "delete-14"],
        ),
        ("(//li)[2]", &["item-2"]),
        // Text: visible elements only, the innermost that matches.
        ("text=Sign in", &["h1", "submit", "submit-disabled"]),
        ("text=\"Sign in\"", &["h1", "submit"]),
        ("\"Sign in\"", &["h1", "submit"]),
        ("text=/^sign in$/i", &["h1", "submit"]),
        ("text=sign up now", &["signup"]),
        ("text=Hello world", &["split"]),
        ("text=Don't have", &["note"]),
        ("text=Ghost text", &[]),
        ("text=Cherries", &[]),
        ("text=Inside the shadow", &["shadow-text"]),
        // Roles, names and states.
        ("role=button[name=\"Sign in\"]", &["submit"]),
        (
            "role=button[name=\"Sign in\"][include-hidden]",
            &["submit", "hidden-button"],
        ),
        ("role=button[name=sign in]", &["submit", "submit-disabled"]),
        ("role=button[disabled]", &["submit-disabled"]),
        ("role=button[pressed]", &["toggle"]),
        ("role=button[expanded=false]", &["menu"]),
        ("role=checkbox[checked]", &["remember"]),
        ("role=checkbox[checked=false]", &["terms"]),
        ("role=heading[level=2]", &["h2"]),
        ("role=link[name=/pric/i]", &["pricing"]),
        ("role=textbox[name=\"Email address\"]", &["email"]),
        ("role=button[name=Shadow save]", &["shadow-button"]),
        ("role=navigation", &["nav"]),
        // Playwright's other getters.
        ("label=Email", &["email"]),
        ("label=Password", &["password"]),
        ("label=\"Phone\"", &["phone"]),
        ("label=Search the site", &["search"]),
        ("label=remember", &["remember"]),
        ("placeholder=you@", &["email"]),
        ("alt=logo", &["logo"]),
        ("title=\"Search\"", &["search"]),
        ("testid=login-form", &["login"]),
        // Chains and filters.
        ("#items >> .item >> nth=0", &["item-1"]),
        (".item >> nth=-1", &["item-4"]),
        (".item >> nth=9", &[]),
        (".item >> visible=true", &["item-1", "item-2", "item-4"]),
        (".item >> visible=false", &["item-3"]),
        (".item >> has-text=an", &["item-2"]),
        ("role=row >> has-text=Invoice 13", &["row-13"]),
        (
            "role=row >> has-text=Invoice 14 >> text=Delete",
            &["delete-14"],
        ),
        (
            "#invoices >> //button",
            &["delete-12", "delete-13", "delete-14"],
        ),
        ("#login >> role=checkbox >> nth=1", &["terms"]),
        // Frames: by name, by a part of the URL, cross-origin (OOPIF).
        ("frame=payment >> #card", &["card"]),
        ("frame=remote >> label=Card number", &["card"]),
        ("frame=where=other >> role=button", &["pay-button"]),
        ("frame=payment", &["pay"]),
        ("frame=remote", &["remote"]),
    ];
    let options = LocateOptions::default();
    let mut failures = Vec::new();
    for (locator, want) in table {
        let got = all(&engine, locator, &options).await;
        match got {
            Ok(got) if got == json!(want) => {}
            Ok(got) => failures.push(format!("{locator}: got {got}, want {want:?}")),
            Err(e) => failures.push(format!("{locator}: {}", e.to_text())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // The test id attribute is configurable.
    let options = LocateOptions {
        testid_attribute: Some("data-qa".into()),
        ..LocateOptions::default()
    };
    assert_eq!(
        all(&engine, "testid=login", &options).await.unwrap(),
        json!(["login"])
    );
    engine.close().await;
}

#[tokio::test]
async fn refs_start_chains_and_enter_their_iframes() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-locators-refs").await;
    goto(&engine, &site.url("/locators/index.html")).await;
    let snap = snapshot(&engine).await;
    let form = find_ref(&snap.nodes, "form", "").unwrap_or_default();
    let pay = find_ref(&snap.nodes, "iframe", "Payment").unwrap();
    let remote = find_ref(&snap.nodes, "iframe", "Remote").unwrap();
    let submit = find_ref(&snap.nodes, "button", "Sign in").unwrap();
    let options = LocateOptions::default();

    assert_eq!(one(&engine, &submit).await.unwrap(), json!("submit"));
    assert_eq!(
        one(&engine, &format!("@{submit}")).await.unwrap(),
        json!("submit")
    );
    for iframe in [&pay, &remote] {
        assert_eq!(
            all(&engine, &format!("{iframe} >> role=textbox"), &options)
                .await
                .unwrap(),
            json!(["card"]),
            "{iframe}"
        );
        assert_eq!(
            one(&engine, &format!("frame={iframe} >> text=Pay"))
                .await
                .unwrap(),
            json!("pay-button")
        );
    }
    if !form.is_empty() {
        assert_eq!(
            all(&engine, &format!("{form} >> role=checkbox"), &options)
                .await
                .unwrap(),
            json!(["remember", "terms"])
        );
    }
    // frame= needs an iframe's ref.
    let err = one(&engine, &format!("frame={submit} >> #card"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLocator, "{}", err.to_text());
    assert_eq!(
        err.message,
        format!("frame={submit}: {submit} is not an iframe")
    );
    // A ref from an earlier page is stale.
    goto(&engine, &site.url("/locators/index.html")).await;
    let err = one(&engine, &format!("{submit} >> nth=0"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::RefStale, "{}", err.to_text());
    engine.close().await;
}

#[tokio::test]
async fn actions_need_exactly_one_match() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-locators-strict").await;
    goto(&engine, &site.url("/locators/index.html")).await;

    assert_eq!(
        one(&engine, "role=button[name=\"Sign in\"]").await.unwrap(),
        json!("submit")
    );
    assert_eq!(one(&engine, "frame=payment").await.unwrap(), json!("pay"));

    let err = one(&engine, "#nope").await.unwrap_err();
    eprintln!("{}", err.to_text());
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "no element matches #nope");
    let err = one(&engine, "text=Ghost text").await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.notes[0].text, "text= matches visible elements only");

    // Three rows each have a Delete button: the error lists them with
    // new refs and the row each is in.
    let err = one(&engine, "text=Delete").await.unwrap_err();
    eprintln!("{}", err.to_text());
    assert_eq!(err.kind, ErrorKind::Ambiguous);
    assert_eq!(err.exit_code(), 1);
    let refs: Vec<String> = err.details.as_ref().unwrap()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["ref"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(refs.len(), 3);
    assert_eq!(
        err.message,
        format!("3 elements match text=Delete: {}", refs.join(", "))
    );
    let notes: Vec<String> = err
        .notes
        .iter()
        .map(|n| format!("{}: {}", n.label, n.text))
        .collect();
    assert_eq!(
        notes,
        [12, 13, 14]
            .iter()
            .zip(&refs)
            .map(|(n, r)| format!("{r}: button \"Delete\" (in row \"Invoice {n} ${n}0 Delete\")"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        err.hint.as_deref(),
        Some("use one of the refs, or add >> nth=<index> to text=Delete")
    );
    // The new refs name those buttons.
    for (r, id) in refs.iter().zip(["delete-12", "delete-13", "delete-14"]) {
        assert_eq!(one(&engine, r).await.unwrap(), json!(id));
    }
    assert_eq!(
        one(&engine, "text=Delete >> nth=1").await.unwrap(),
        json!("delete-13")
    );

    // Past five, the rest is counted; hidden candidates say so.
    goto(&engine, &site.url("/locators/ambiguous.html")).await;
    let err = one(&engine, "text=Delete").await.unwrap_err();
    assert!(
        err.message.starts_with("6 elements match text=Delete: e"),
        "{}",
        err.message
    );
    assert!(err.message.ends_with(", …"), "{}", err.message);
    let last = err.notes.last().unwrap();
    assert_eq!((last.label.as_str(), last.text.as_str()), ("…", "1 more"));
    let err = one(&engine, "css=button >> nth=-1 >> visible=false").await;
    assert_eq!(err.unwrap(), json!("button"));
    let err = one(&engine, "css=li:nth-child(n+5) button")
        .await
        .unwrap_err();
    let notes: Vec<&str> = err.notes.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(
        notes[2], "button \"Delete\" (in listitem \"Invoice 7\", hidden)",
        "{notes:?}"
    );
    engine.close().await;
}

#[tokio::test]
async fn mistakes_in_locators_are_bad_locator_errors() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-locators-bad").await;
    goto(&engine, &site.url("/locators/index.html")).await;
    let options = LocateOptions::default();
    for (locator, message) in [
        ("css=a[", "css=a[: a[ is not a valid CSS selector"),
        (
            "text=/(/",
            "text=/(/: /(/ is not a valid regex: Invalid regular expression: /(/: Unterminated group",
        ),
        (
            "xpath=//a[",
            "xpath=//a[: //a[ is not a valid XPath expression",
        ),
    ] {
        let err = all(&engine, locator, &options).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadLocator, "{}", err.to_text());
        assert_eq!(err.message, message);
    }
    let err = all(&engine, "button", &options).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLocator);
    let err = all(&engine, "frame=nope >> #card", &options)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound, "{}", err.to_text());
    assert!(
        err.message.starts_with("no frame in tab t1 matches"),
        "{}",
        err.message
    );
    engine.close().await;
}

async fn wait(engine: &Engine, condition: WaitFor, timeout_ms: u64) -> Result<WaitData, ErrorInfo> {
    let output = engine
        .wait(WaitRequest {
            condition,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms,
            max_chars: 20_000,
        })
        .await?;
    Ok(serde_json::from_value(output.data).unwrap())
}

fn element(locator: &str, state: ElementState) -> WaitFor {
    WaitFor::Element {
        locator: locator.to_owned(),
        state,
    }
}

#[tokio::test]
async fn wait_takes_every_locator() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-locators-wait").await;
    goto(&engine, &site.url("/locators/index.html")).await;
    let eval = |js: &'static str| {
        let engine = engine.clone();
        async move {
            engine
                .evaluate(None, None, World::Main, js, TIMEOUT)
                .await
                .unwrap()
        }
    };

    // An element that a role locator finds later.
    eval(
        "setTimeout(() => { const b = document.createElement('button'); \
         b.textContent = 'Later'; document.body.append(b); }, 300)",
    )
    .await;
    let data = wait(
        &engine,
        element("role=button[name=\"Later\"]", ElementState::Visible),
        10_000,
    )
    .await
    .unwrap();
    assert_eq!(data.count, Some(1));
    assert_eq!(data.met, "role=button[name=\"Later\"] is visible");

    // In frames, and text in a frame.
    let data = wait(
        &engine,
        element("frame=remote >> #card", ElementState::Visible),
        10_000,
    )
    .await
    .unwrap();
    assert_eq!(data.count, Some(1));
    for within in ["frame=payment", "frame=remote >> #pay-button"] {
        let data = wait(
            &engine,
            WaitFor::Text {
                text: "Pay".into(),
                within: Some(within.into()),
                gone: false,
            },
            10_000,
        )
        .await
        .unwrap_or_else(|e| panic!("{within}: {}", e.to_text()));
        assert_eq!(data.met, "text \"Pay\" is visible");
    }
    // A frame that comes later.
    eval(
        "setTimeout(() => { const f = document.createElement('iframe'); f.name = 'late'; \
         f.src = '/locators/frame.html'; document.body.append(f); }, 300)",
    )
    .await;
    wait(
        &engine,
        element("frame=late >> #card", ElementState::Visible),
        10_000,
    )
    .await
    .unwrap();
    // No frame yet: detached holds; visible times out.
    wait(
        &engine,
        element("frame=never >> #card", ElementState::Detached),
        2_000,
    )
    .await
    .unwrap();
    let err = wait(
        &engine,
        element("frame=never >> #card", ElementState::Visible),
        500,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout, "{}", err.to_text());

    // A ref, until its element goes.
    let snap = snapshot(&engine).await;
    let later = find_ref(&snap.nodes, "button", "Later").unwrap();
    // An iframe's ref as --in reads its document.
    let pay = find_ref(&snap.nodes, "iframe", "Payment").unwrap();
    let data = wait(
        &engine,
        WaitFor::Text {
            text: "Pay".into(),
            within: Some(pay),
            gone: false,
        },
        5_000,
    )
    .await
    .unwrap();
    assert_eq!(data.met, "text \"Pay\" is visible");
    wait(&engine, element(&later, ElementState::Visible), 5_000)
        .await
        .unwrap();
    eval(
        "setTimeout(() => [...document.querySelectorAll('button')]\
         .find((b) => b.textContent === 'Later').remove(), 300)",
    )
    .await;
    let data = wait(&engine, element(&later, ElementState::Detached), 10_000)
        .await
        .unwrap();
    assert_eq!(data.count, Some(0));
    let err = wait(&engine, element(&later, ElementState::Visible), 2_000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::RefDetached, "{}", err.to_text());

    // A bad selector fails at once.
    let err = wait(&engine, element("css=a[", ElementState::Visible), 10_000)
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadLocator, "{}", err.to_text());
    engine.close().await;
}

//! The ref registry against a real Chrome (`CHROME_PATH`; skipped without
//! it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site
//! (plan 04, milestone 4.2): refs survive re-renders that keep their node,
//! re-resolve by role and name when a re-render replaced it, and fail
//! with `RefStale` or `RefDetached`, never hitting another element, when
//! they cannot. The fuzz test mutates a page at random and checks every
//! ref after each round.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    Engine, NavigateRequest, Navigation, OpenRequest, RefCall, SnapshotRequest, WaitUntil, World,
};
use fixtures::Fixtures;
use protocol::schema::{SnapshotData, SnapshotNode};
use protocol::{ErrorInfo, ErrorKind};
use serde_json::Value;

const TIMEOUT_MS: u64 = 30_000;

/// Reads the element a ref hits.
const TEXT: &str = "function () { return this.textContent; }";

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

async fn start(path: &Path) -> Arc<Engine> {
    let engine = Engine::new("engine-refs-test", None, None);
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

fn request() -> SnapshotRequest {
    SnapshotRequest {
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
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn navigate(engine: &Engine, navigation: Navigation) {
    engine
        .navigate(NavigateRequest {
            navigation: navigation.clone(),
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("{navigation:?}: {}", e.to_text()));
}

async fn goto(engine: &Engine, url: &str) {
    let navigation = Navigation::Goto {
        url: url.to_owned(),
        base_url: None,
    };
    navigate(engine, navigation).await;
}

async fn eval(engine: &Engine, js: &str) -> Value {
    engine
        .evaluate(None, None, World::Main, js, Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("{js}: {}", e.to_text()))
}

/// A reffed node of a snapshot: its ref, role and name, and the iframe it
/// is in ("" for the main frame).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reffed {
    reference: String,
    role: String,
    name: String,
    frame: String,
}

/// The reffed nodes of a full snapshot of the active tab, in order.
async fn refs_of(engine: &Engine) -> Vec<Reffed> {
    let output = engine
        .snapshot(request())
        .await
        .unwrap_or_else(|e| panic!("snapshot: {}", e.to_text()));
    let data: SnapshotData = serde_json::from_value(output.data).unwrap();
    let mut found = Vec::new();
    collect(&data.nodes, "", &mut found);
    found
}

fn collect(nodes: &[SnapshotNode], frame: &str, found: &mut Vec<Reffed>) {
    for node in nodes {
        if let Some(reference) = &node.reference {
            found.push(Reffed {
                reference: reference.clone(),
                role: node.role.clone(),
                name: node.name.clone(),
                frame: frame.to_owned(),
            });
        }
        let inner = if node.role == "iframe" {
            node.name.as_str()
        } else {
            frame
        };
        collect(&node.children, inner, found);
    }
}

/// The ref of the first node with `role` and `name`.
fn ref_of(refs: &[Reffed], role: &str, name: &str) -> String {
    refs.iter()
        .find(|r| r.role == role && r.name == name)
        .unwrap_or_else(|| panic!("no {role} \"{name}\" in {refs:#?}"))
        .reference
        .clone()
}

/// The number of a ref (`12` for `e12`).
fn number(reference: &str) -> u32 {
    reference.trim_start_matches('e').parse().unwrap()
}

async fn call(engine: &Engine, reference: &str, strict: bool) -> Result<RefCall, ErrorInfo> {
    call_fn(engine, reference, strict, TEXT).await
}

async fn call_fn(
    engine: &Engine,
    reference: &str,
    strict: bool,
    function: &str,
) -> Result<RefCall, ErrorInfo> {
    engine
        .call_on_ref(
            None,
            reference,
            strict,
            function,
            Duration::from_millis(TIMEOUT_MS),
        )
        .await
}

/// The element's text, without a note.
async fn hits(engine: &Engine, reference: &str) -> String {
    let call = call(engine, reference, true)
        .await
        .unwrap_or_else(|e| panic!("{reference}: {}", e.to_text()));
    assert_eq!(call.note, None);
    call.value.as_str().unwrap().to_owned()
}

fn expect_error(result: Result<RefCall, ErrorInfo>, kind: ErrorKind) -> ErrorInfo {
    match result {
        Err(e) if e.kind == kind => e,
        Err(e) => panic!("expected {kind}, got {}", e.to_text()),
        Ok(call) => panic!("expected {kind}, got {call:?}"),
    }
}

/// Waits until the main frame's URL ends with `suffix` (the engine saw a
/// same-document navigation).
async fn wait_for_url(engine: &Engine, suffix: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let frames = engine.frames(None, Duration::from_secs(5)).await.unwrap();
        if frames[0].url.ends_with(suffix) {
            return;
        }
        assert!(Instant::now() < deadline, "URL is {}", frames[0].url);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rerender_that_keeps_nodes_keeps_their_refs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/refs/rerender.html")).await;
    let before = refs_of(&engine).await;
    let milk = ref_of(&before, "button", "Buy milk");
    let save = ref_of(&before, "button", "Save");
    let last = before.iter().map(|r| number(&r.reference)).max().unwrap();

    // Same keys, new order, one new item.
    eval(
        &engine,
        "render([{key: 'e', label: 'Feed cat'}, ...TODOS.slice().reverse()]); true",
    )
    .await;
    let after = refs_of(&engine).await;
    assert_eq!(ref_of(&after, "button", "Buy milk"), milk);
    assert_eq!(ref_of(&after, "button", "Save"), save);
    assert!(number(&ref_of(&after, "button", "Feed cat")) > last);
    assert_eq!(hits(&engine, &milk).await, "Buy milk");

    // A new label on a kept node: the same ref, the new name.
    eval(&engine, "render([{key: 'a', label: 'Buy oat milk'}]); true").await;
    assert_eq!(hits(&engine, &milk).await, "Buy oat milk");
    let renamed = refs_of(&engine).await;
    assert_eq!(ref_of(&renamed, "button", "Buy oat milk"), milk);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remount_re_resolves_by_role_and_name() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/refs/rerender.html")).await;
    let before = refs_of(&engine).await;
    let milk = ref_of(&before, "button", "Buy milk");
    let walk = ref_of(&before, "button", "Walk dog");
    let delete = ref_of(&before, "button", "Delete");
    eval(&engine, "remount(TODOS.filter(t => t.key !== 'b')); true").await;

    // Strict: the node is gone, so is the ref.
    let strict = expect_error(call(&engine, &milk, true).await, ErrorKind::RefDetached);
    assert_eq!(
        strict.message,
        format!("element {milk} is no longer in the page")
    );
    assert_eq!(strict.notes[0].text, "button \"Buy milk\"");
    assert!(strict.hint.as_deref().unwrap().contains("snapshot"));

    // Otherwise the one "Buy milk" button stands in, and keeps the ref.
    let found = call(&engine, &milk, false).await.unwrap();
    assert_eq!(found.value, "Buy milk");
    assert_eq!(found.note, Some(format!("{milk} re-resolved by role+name")));
    assert_eq!(hits(&engine, &milk).await, "Buy milk");
    let after = refs_of(&engine).await;
    assert_eq!(ref_of(&after, "button", "Buy milk"), milk);

    // Two "Delete" buttons: no guessing.
    let ambiguous = expect_error(call(&engine, &delete, false).await, ErrorKind::RefDetached);
    assert_eq!(
        ambiguous.notes[1].text,
        "2 elements have its role and name now; not guessing which"
    );
    assert_eq!(ambiguous.details.as_ref().unwrap()["matches"], 2);
    // None left.
    let gone = expect_error(call(&engine, &walk, false).await, ErrorKind::RefDetached);
    assert_eq!(gone.notes.len(), 1, "{}", gone.to_text());

    // snapshot --scope takes the same way, and says so.
    eval(&engine, "remount(TODOS); true").await;
    let scoped = engine
        .snapshot(SnapshotRequest {
            scope: Some(milk.clone()),
            ..request()
        })
        .await
        .unwrap();
    assert!(
        scoped
            .text
            .ends_with(&format!("- button \"Buy milk\" [{milk}]")),
        "{}",
        scoped.text
    );
    assert_eq!(
        scoped.warnings,
        [format!("{milk} re-resolved by role+name")]
    );
    eval(&engine, "remount(TODOS); true").await;
    let strict = engine
        .snapshot(SnapshotRequest {
            scope: Some(milk.clone()),
            strict_refs: true,
            ..request()
        })
        .await
        .unwrap_err();
    assert_eq!(strict.kind, ErrorKind::RefDetached);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_page_makes_refs_stale() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/refs/rerender.html")).await;
    let before = refs_of(&engine).await;
    let save = ref_of(&before, "button", "Save");
    let last = before.iter().map(|r| number(&r.reference)).max().unwrap();

    goto(&engine, &site.url("/snapshot/simple.html")).await;
    let stale = expect_error(call(&engine, &save, false).await, ErrorKind::RefStale);
    assert_eq!(
        stale.message,
        format!("ref {save} is from an earlier page of tab t1")
    );
    assert!(stale.hint.as_deref().unwrap().starts_with("chrome-cdp"));
    let scoped = engine
        .snapshot(SnapshotRequest {
            scope: Some(save.clone()),
            ..request()
        })
        .await
        .unwrap_err();
    assert_eq!(scoped.kind, ErrorKind::RefStale);

    // Numbers go on: an old ref never names a new element.
    let after = refs_of(&engine).await;
    assert!(number(&ref_of(&after, "link", "a link")) > last);
    let unknown = expect_error(call(&engine, "e999", false).await, ErrorKind::NotFound);
    assert_eq!(unknown.message, "tab t1 has no ref e999");
    expect_error(call(&engine, "x1", false).await, ErrorKind::BadLocator);

    // A reload is a new document too.
    let link = ref_of(&after, "link", "a link");
    navigate(&engine, Navigation::Reload { hard: false }).await;
    expect_error(call(&engine, &link, false).await, ErrorKind::RefStale);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_change_keeps_live_refs_but_stops_the_fallback() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/refs/rerender.html")).await;
    let before = refs_of(&engine).await;
    let milk = ref_of(&before, "button", "Buy milk");
    let save = ref_of(&before, "button", "Save");

    eval(
        &engine,
        "history.pushState({}, '', '/refs/other'); remount(TODOS); true",
    )
    .await;
    wait_for_url(&engine, "/refs/other").await;
    assert_eq!(hits(&engine, &save).await, "Save");
    let detached = expect_error(call(&engine, &milk, false).await, ErrorKind::RefDetached);
    assert!(
        detached.notes[1]
            .text
            .starts_with("the page changed its URL"),
        "{}",
        detached.to_text()
    );

    // Refs shown after the route change fall back again.
    let after = refs_of(&engine).await;
    let milk = ref_of(&after, "button", "Buy milk");
    eval(&engine, "remount(TODOS); true").await;
    let found = call(&engine, &milk, false).await.unwrap();
    assert_eq!(found.note, Some(format!("{milk} re-resolved by role+name")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_that_navigates_loses_only_its_refs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/snapshot/frames.html")).await;
    let before = refs_of(&engine).await;
    let in_frame = |frame: &str| -> Vec<Reffed> {
        before
            .iter()
            .filter(|r| r.frame == frame)
            .cloned()
            .collect()
    };
    let same = in_frame("Same site");
    let other = in_frame("Other site");
    let iframes = in_frame("");
    assert_eq!(same.len(), 2, "{before:#?}");
    assert_eq!(other.len(), 2, "{before:#?}");
    assert_eq!(iframes.len(), 2, "{before:#?}");
    assert_eq!(hits(&engine, &same[0].reference).await, "Frame button");
    assert_eq!(hits(&engine, &other[0].reference).await, "Frame button");

    for title in ["Same site", "Other site"] {
        eval(
            &engine,
            &format!(
                "new Promise(done => {{
                   const frame = document.querySelector('iframe[title=\"{title}\"]');
                   frame.onload = () => done(true);
                   frame.src = frame.src.replace('frame-inner.html', 'simple.html');
                 }})"
            ),
        )
        .await;
    }
    for reffed in same.iter().chain(&other) {
        let stale = expect_error(
            call(&engine, &reffed.reference, false).await,
            ErrorKind::RefStale,
        );
        assert_eq!(
            stale.message,
            format!(
                "ref {} is from an earlier document of its frame",
                reffed.reference
            )
        );
    }
    // The iframe elements themselves are the same.
    let iframe = "function () { return this.title; }";
    for (reffed, title) in iframes.iter().zip(["Same site", "Other site"]) {
        let call = call_fn(&engine, &reffed.reference, true, iframe)
            .await
            .unwrap();
        assert_eq!(call.value, title);
    }
}

/// Rounds of the fuzz test.
const FUZZ_ROUNDS: u64 = 40;

/// What the fuzz probe reads from the element a ref hits: its `data-id`,
/// tag, name, and how many shown elements have that tag and name.
const PROBE: &str = "function () {
  const same = [...document.querySelectorAll(this.tagName)].filter(
    e => e.textContent === this.textContent && !e.closest('[hidden]')).length;
  return {id: this.dataset.id || null, tag: this.tagName.toLowerCase(),
          name: this.textContent, same};
}";

/// The fuzz test: after random DOM changes (remove, replace with a look-
/// alike, rename, move, add, hide) and now and then a reload or a route
/// change, every ref of the last snapshot either hits its own element,
/// hits the only shown element with its role and name after its own is
/// gone (with a note; never with `--strict-refs` or after a route change),
/// or fails with `RefStale`/`RefDetached`. A ref whose element is still in
/// the page always hits it.
#[tokio::test(flavor = "multi_thread")]
async fn fuzz_refs_hit_their_element_or_fail() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let base: u64 = std::env::var("REFS_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    goto(&engine, &site.url("/refs/fuzz.html")).await;
    let mut outcomes: BTreeMap<&str, usize> = BTreeMap::new();

    for round in 0..FUZZ_ROUNDS {
        let seed = base * 1_000 + round;
        let refs: Vec<Reffed> = refs_of(&engine)
            .await
            .into_iter()
            .filter(|r| r.role == "button" || r.role == "link")
            .collect();
        let mut ids = Vec::new();
        for reffed in &refs {
            let hit = call_fn(&engine, &reffed.reference, true, PROBE)
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: {}", e.to_text()));
            ids.push(hit.value["id"].as_str().unwrap().to_owned());
        }

        let reload = round % 10 == 9;
        let route = round % 10 == 4;
        let context = if reload {
            navigate(&engine, Navigation::Reload { hard: false }).await;
            "reload".to_owned()
        } else {
            if route {
                eval(
                    &engine,
                    &format!("history.pushState({{}}, '', '#round{round}'); true"),
                )
                .await;
                wait_for_url(&engine, &format!("#round{round}")).await;
            }
            let changes = eval(&engine, &format!("mutate({seed}, 4)")).await;
            format!("{changes}")
        };
        let present = eval(
            &engine,
            "[...document.querySelectorAll('[data-id]')].map(e => e.dataset.id)",
        )
        .await;
        let present: Vec<&str> = present
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();

        for (reffed, id) in refs.iter().zip(&ids) {
            let what = format!(
                "seed {seed} ({context}): {} {} \"{}\" (data-id {id})",
                reffed.reference, reffed.role, reffed.name
            );
            let kept = !reload && present.contains(&id.as_str());
            let tag = if reffed.role == "button" {
                "button"
            } else {
                "a"
            };

            // Strict: its own element, or an error.
            match call_fn(&engine, &reffed.reference, true, PROBE).await {
                Ok(hit) => {
                    assert!(kept, "{what}: hit {} but it is gone", hit.value);
                    assert_eq!(hit.value["id"], id.as_str(), "{what}");
                    assert_eq!(hit.note, None, "{what}");
                }
                Err(e) => {
                    assert!(!kept, "{what}: still in the page, but {}", e.to_text());
                    let expected = if reload {
                        ErrorKind::RefStale
                    } else {
                        ErrorKind::RefDetached
                    };
                    assert_eq!(e.kind, expected, "{what}: {}", e.to_text());
                }
            }

            // With the fallback.
            let outcome = match call_fn(&engine, &reffed.reference, false, PROBE).await {
                Ok(hit) if hit.note.is_none() => {
                    assert!(kept, "{what}: hit {} but it is gone", hit.value);
                    assert_eq!(hit.value["id"], id.as_str(), "{what}");
                    "own element"
                }
                Ok(hit) => {
                    assert!(!kept, "{what}: re-resolved though still in the page");
                    assert!(!route, "{what}: re-resolved after a route change");
                    assert_eq!(hit.value["tag"], tag, "{what}: {}", hit.value);
                    assert_eq!(hit.value["name"], reffed.name.as_str(), "{what}");
                    assert_eq!(hit.value["same"], 1, "{what}: {}", hit.value);
                    assert_ne!(hit.value["id"], id.as_str(), "{what}");
                    "re-resolved"
                }
                Err(e) if e.kind == ErrorKind::RefStale => {
                    assert!(reload, "{what}: {}", e.to_text());
                    "stale"
                }
                Err(e) => {
                    assert_eq!(e.kind, ErrorKind::RefDetached, "{what}: {}", e.to_text());
                    assert!(!kept, "{what}: still in the page, but {}", e.to_text());
                    "detached"
                }
            };
            *outcomes.entry(outcome).or_default() += 1;
        }
    }
    eprintln!("ref fuzz outcomes (seed base {base}): {outcomes:?}");
    for outcome in ["own element", "re-resolved", "stale", "detached"] {
        assert!(
            outcomes.contains_key(outcome),
            "no ref ended {outcome}: {outcomes:?}"
        );
    }
}

//! The keyboard and form actions against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture site's `actions/` pages (plan 05, milestone 5.3).
//!
//! The keyboard fixture logs every key and input event it gets (key,
//! code, keyCode, location, modifiers, target, trusted); the form fixture
//! logs its fields' input and change events; the frameworks fixture
//! echoes the state of real React, Vue and Svelte inputs.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, Engine, LocateOptions, Modifier, NavigateRequest, Navigation,
    OpenRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::Value;

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS);

/// How long a check that never passes is given.
const SHORT: Duration = Duration::from_millis(1200);

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

async fn act_within(
    engine: &Engine,
    action: Action,
    timeout: Duration,
) -> Result<Output, ErrorInfo> {
    engine
        .act(ActionRequest {
            action,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap(),
            no_wait_after: false,
            snap: false,
            max_chars: 0,
        })
        .await
}

async fn act(engine: &Engine, action: Action) -> Output {
    let what = format!("{action:?}");
    act_within(engine, action, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()))
}

async fn fails(engine: &Engine, action: Action) -> ErrorInfo {
    let what = format!("{action:?}");
    match act_within(engine, action, SHORT).await {
        Ok(out) => panic!("{what} passed: {}", out.text),
        Err(err) => err,
    }
}

/// Calls `function` on the one element `locator` matches.
async fn call(engine: &Engine, locator: &str, function: &str) -> Value {
    engine
        .call_on_locator(None, locator, &LocateOptions::default(), function, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{locator}: {}", e.to_text()))
        .value
}

async fn text(engine: &Engine, locator: &str) -> String {
    call(engine, locator, "function () { return this.textContent; }")
        .await
        .as_str()
        .unwrap()
        .to_owned()
}

async fn value(engine: &Engine, locator: &str) -> String {
    call(engine, locator, "function () { return this.value; }")
        .await
        .as_str()
        .unwrap()
        .to_owned()
}

async fn checked(engine: &Engine, locator: &str) -> bool {
    call(engine, locator, "function () { return this.checked; }")
        .await
        .as_bool()
        .unwrap()
}

/// The log lines of `#log` (or `#keys`) after its first `before` bytes.
async fn lines(engine: &Engine, log: &str, before: usize) -> Vec<String> {
    text(engine, log).await[before..]
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn log_len(engine: &Engine, log: &str) -> usize {
    text(engine, log).await.len()
}

fn press(keys: &str, locator: Option<&str>) -> Action {
    Action::Press {
        keys: keys.to_owned(),
        locator: locator.map(str::to_owned),
    }
}

fn type_in(text: &str, locator: Option<&str>) -> Action {
    Action::Type {
        text: text.to_owned(),
        locator: locator.map(str::to_owned),
        delay_ms: 0,
        force: false,
    }
}

fn fill(locator: &str, text: &str) -> Action {
    Action::Fill {
        locator: locator.to_owned(),
        text: text.to_owned(),
        force: false,
    }
}

fn select(locator: &str, values: &[&str]) -> Action {
    Action::Select {
        locator: locator.to_owned(),
        values: values.iter().map(|v| (*v).to_owned()).collect(),
        force: false,
    }
}

fn check(locator: &str, checked: bool) -> Action {
    Action::Check {
        locator: locator.to_owned(),
        checked,
        force: false,
    }
}

#[tokio::test]
async fn presses_keys_and_chords() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-press").await;
    goto(&engine, &site.url("/actions/keyboard.html")).await;

    let out = act(&engine, press("Enter", Some("#field"))).await;
    assert_eq!(out.text, "pressed Enter in e1 textbox \"Field\"");
    assert_eq!(out.data["keys"], serde_json::json!(["Enter"]));
    assert_eq!(
        lines(&engine, "#keys", 0).await,
        [
            "keydown \"Enter\" Enter 13 L0 - field trusted",
            "keypress \"Enter\" Enter 13 L0 - field trusted",
            "keyup \"Enter\" Enter 13 L0 - field trusted",
        ]
    );

    // Modifiers are held around the key, with their own events and bits.
    act(&engine, type_in("abc", Some("#field"))).await;
    let before = log_len(&engine, "#keys").await;
    let out = act(&engine, press("Control+a Backspace", None)).await;
    assert_eq!(
        out.text,
        "pressed Control+a Backspace in e1 textbox \"Field\""
    );
    assert_eq!(
        lines(&engine, "#keys", before).await,
        [
            "keydown \"Control\" ControlLeft 17 L1 ctrl field trusted",
            "keydown \"a\" KeyA 65 L0 ctrl field trusted",
            "keyup \"a\" KeyA 65 L0 ctrl field trusted",
            "keyup \"Control\" ControlLeft 17 L1 - field trusted",
            "keydown \"Backspace\" Backspace 8 L0 - field trusted",
            "input deleteContentBackward null field trusted",
            "keyup \"Backspace\" Backspace 8 L0 - field trusted",
        ]
    );
    assert_eq!(value(&engine, "#field").await, "");

    // Shift makes a key its shifted self; the numpad has its location.
    let before = log_len(&engine, "#keys").await;
    act(&engine, press("Shift+1 Numpad5", None)).await;
    let keys = lines(&engine, "#keys", before).await;
    assert!(
        keys.contains(&"keydown \"!\" Digit1 49 L0 shift field trusted".to_owned()),
        "{keys:#?}"
    );
    assert!(
        keys.contains(&"keydown \"Clear\" Numpad5 12 L3 - field trusted".to_owned()),
        "{keys:#?}"
    );

    // Shift+Tab moves focus back, as a user's does.
    act(&engine, press("Shift+Tab", Some("#area"))).await;
    let out = act(&engine, press("Escape", None)).await;
    assert_eq!(out.text, "pressed Escape in e1 textbox \"Field\"");

    // Mod is Control on this (Linux) browser: the page's shortcut runs.
    let out = act(&engine, press("Mod+k", Some("#btn"))).await;
    assert_eq!(out.data["keys"], serde_json::json!(["Control+k"]));
    assert_eq!(text(&engine, "#palette").await, "Command palette");
    let hidden = call(&engine, "#palette", "function () { return this.hidden; }").await;
    assert_eq!(hidden, Value::Bool(false));

    // Mod in click --modifiers too.
    let out = act(
        &engine,
        Action::Click {
            locator: "#btn".to_owned(),
            button: engine::Button::Left,
            count: 1,
            modifiers: vec![Modifier::Mod],
            position: None,
            force: false,
            dblclick: false,
        },
    )
    .await;
    assert_eq!(out.text, "clicked e3 button \"Button\" with Control");

    // Unknown keys fail before the browser; an element that cannot take
    // focus fails instead of sending keys elsewhere.
    let err = fails(&engine, press("Control+Nope", None)).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert!(
        err.message.starts_with("unknown key `Nope`"),
        "{}",
        err.message
    );
    let err = fails(&engine, press("Enter", Some("#plain"))).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert!(
        err.message
            .ends_with("div \"Plain text\" cannot take focus, so keys would not go to it"),
        "{}",
        err.message
    );

    engine.close().await;
}

#[tokio::test]
async fn types_keys_and_inserts_the_rest() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-type").await;
    goto(&engine, &site.url("/actions/keyboard.html")).await;

    let out = act(&engine, type_in("Hi!", Some("#field"))).await;
    assert_eq!(out.text, "typed 3 characters into e1 textbox \"Field\"");
    assert_eq!(
        lines(&engine, "#keys", 0).await,
        [
            "keydown \"H\" KeyH 72 L0 - field trusted",
            "keypress \"H\" KeyH 72 L0 - field trusted",
            "input insertText \"H\" field trusted",
            "keyup \"H\" KeyH 72 L0 - field trusted",
            "keydown \"i\" KeyI 73 L0 - field trusted",
            "keypress \"i\" KeyI 105 L0 - field trusted",
            "input insertText \"i\" field trusted",
            "keyup \"i\" KeyI 73 L0 - field trusted",
            "keydown \"!\" Digit1 49 L0 - field trusted",
            "keypress \"!\" Digit1 33 L0 - field trusted",
            "input insertText \"!\" field trusted",
            "keyup \"!\" Digit1 49 L0 - field trusted",
        ]
    );

    // What the layout cannot type arrives whole, as an IME commits it.
    let before = log_len(&engine, "#keys").await;
    act(&engine, type_in(" 👍日本", None)).await;
    assert_eq!(value(&engine, "#field").await, "Hi! 👍日本");
    let keys = lines(&engine, "#keys", before).await;
    assert_eq!(
        keys.last().unwrap(),
        "input insertText \"👍日本\" field trusted"
    );

    // A field it focuses gets the text after what it holds.
    act(&engine, type_in("+more", Some("#filled"))).await;
    assert_eq!(value(&engine, "#filled").await, "start+more");
    // New lines are Enter in a textarea; --delay waits between keys.
    act(
        &engine,
        Action::Type {
            text: "a\nb".to_owned(),
            locator: Some("#area".to_owned()),
            delay_ms: 20,
            force: false,
        },
    )
    .await;
    assert_eq!(value(&engine, "#area").await, "a\nb");
    act(&engine, type_in(" typed", Some("#editor"))).await;
    assert_eq!(text(&engine, "#editor").await, "Hello world typed");

    // It waits for an editable element, and says why not.
    let err = fails(&engine, type_in("x", Some("#locked"))).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert_eq!(
        err.message,
        "#locked is read-only: it has the readonly attribute"
    );
    let err = fails(&engine, type_in("x", Some("#plain"))).await;
    assert_eq!(
        err.message,
        "#plain cannot take text: it is not an input, textarea or contenteditable element"
    );
    let err = fails(&engine, type_in("x", Some("#off"))).await;
    assert_eq!(
        err.message,
        "#off is disabled: it has the disabled attribute"
    );
    // --force skips the checks: a read-only field just takes no text.
    act(
        &engine,
        Action::Type {
            text: "x".to_owned(),
            locator: Some("#locked".to_owned()),
            delay_ms: 0,
            force: true,
        },
    )
    .await;
    assert_eq!(value(&engine, "#locked").await, "fixed");
    // A --delay longer than the timeout fails at once.
    let err = fails(
        &engine,
        Action::Type {
            text: "abcdef".to_owned(),
            locator: None,
            delay_ms: 1000,
            force: false,
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::BadArgs);

    engine.close().await;
}

#[tokio::test]
async fn fills_fields_of_every_kind() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-fill").await;
    goto(&engine, &site.url("/actions/form.html")).await;

    let out = act(&engine, fill("#email", "ada@example.com")).await;
    assert_eq!(out.text, "filled e1 textbox \"Email\"");
    assert_eq!(out.data["value"], "ada@example.com");
    assert_eq!(
        lines(&engine, "#log", 0).await,
        ["input email \"ada@example.com\" trusted"]
    );
    // Its change comes when it loses focus, from the browser.
    let before = log_len(&engine, "#log").await;
    let out = act(
        &engine,
        Action::Blur {
            locator: "#email".to_owned(),
        },
    )
    .await;
    assert_eq!(out.text, "blurred e1 textbox \"Email\"");
    assert_eq!(
        lines(&engine, "#log", before).await,
        ["change email \"ada@example.com\" trusted"]
    );
    let out = act(
        &engine,
        Action::Blur {
            locator: "#email".to_owned(),
        },
    )
    .await;
    assert_eq!(out.text, "e1 textbox \"Email\" did not have focus");

    // What it holds is replaced; "" clears.
    act(&engine, fill("#notes", "line 1\nline 2")).await;
    assert_eq!(value(&engine, "#notes").await, "line 1\nline 2");
    act(&engine, fill("#name", "")).await;
    assert_eq!(value(&engine, "#name").await, "");

    // Passwords are not echoed.
    let out = act(&engine, fill("#pw", "hunter22")).await;
    assert_eq!(out.data.get("value"), None);
    assert_eq!(value(&engine, "#pw").await, "hunter22");

    // A field that keeps something else says so.
    let out = act(&engine, fill("#code", "123456")).await;
    assert_eq!(
        out.warnings,
        [
            "the field holds \"1234\", not the text filled (a mask, a length limit, or the page changed it)"
        ]
    );

    // Types that take no typing get their value set, with input and change.
    for (id, v) in [
        ("birthday", "1990-05-17"),
        ("alarm", "13:45"),
        ("meeting", "2026-09-27T13:45"),
        ("month", "2026-09"),
        ("week", "2026-W39"),
        ("colour", "#ff8800"),
        ("volume", "7"),
    ] {
        let before = log_len(&engine, "#log").await;
        act(&engine, fill(&format!("#{id}"), v)).await;
        assert_eq!(value(&engine, &format!("#{id}")).await, v);
        // (After the change of the field focus left.)
        let log = lines(&engine, "#log", before).await;
        assert_eq!(
            log[log.len() - 2..],
            [
                format!("input {id} \"{v}\" synthetic"),
                format!("change {id} \"{v}\" synthetic"),
            ]
        );
    }
    let err = fails(&engine, fill("#birthday", "tomorrow")).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert_eq!(
        err.message,
        "e6 textbox \"Birthday\" cannot hold that text: it is an <input type=date>, which takes a value like 2026-09-27, not \"tomorrow\""
    );
    assert_eq!(
        err.hint.as_deref(),
        Some("chrome-cdp fill '#birthday' 2026-09-27 --session keys-fill")
    );
    let err = fails(&engine, fill("#volume", "70")).await;
    assert!(
        err.message
            .ends_with("which takes a number from 0 to 10, not \"70\""),
        "{}",
        err.message
    );
    let err = fails(&engine, fill("#age", "abc")).await;
    assert!(
        err.message
            .ends_with("it is an <input type=number>, and \"abc\" is not a number"),
        "{}",
        err.message
    );
    act(&engine, fill("#age", "42")).await;
    assert_eq!(value(&engine, "#age").await, "42");

    // Not a text field: at once, with a hint.
    let err = fails(&engine, fill("#remember", "x")).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert_eq!(
        err.message,
        "#remember cannot take text: it is an <input type=checkbox>, which takes no text"
    );
    let err = fails(&engine, fill("#choice", "Red")).await;
    assert_eq!(
        err.message,
        "#choice cannot take text: it is a <select>: choose its options with select"
    );
    let err = fails(
        &engine,
        Action::Fill {
            locator: "#choice".to_owned(),
            text: "Red".to_owned(),
            force: true,
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert_eq!(
        err.hint.as_deref(),
        Some("chrome-cdp select '#choice' Red --session keys-fill")
    );

    // A contenteditable element.
    goto(&engine, &site.url("/actions/keyboard.html")).await;
    act(&engine, fill("#editor", "Rich text")).await;
    assert_eq!(text(&engine, "#editor").await, "Rich text");

    engine.close().await;
}

#[tokio::test]
async fn fill_updates_framework_state() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-frameworks").await;
    goto(&engine, &site.url("/actions/frameworks.html")).await;

    for (framework, name) in [("React", "Ada"), ("Vue", "Grace"), ("Svelte", "Linus")] {
        act(&engine, fill(&format!("label={framework} name"), name)).await;
        let echo = format!("#{}-echo", framework.to_lowercase());
        assert_eq!(text(&engine, &echo).await, name, "{framework}");
        act(&engine, fill(&format!("label={framework} name"), "")).await;
        assert_eq!(text(&engine, &echo).await, "", "{framework}");
    }
    let out = act(&engine, check("#react-agree", true)).await;
    assert_eq!(out.data["changed"], true);
    assert_eq!(text(&engine, "#react-agreed").await, "agreed");
    act(&engine, select("label=React size", &["Large"])).await;
    assert_eq!(text(&engine, "#react-sized").await, "l");

    engine.close().await;
}

#[tokio::test]
async fn selects_options_by_value_or_label() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-select").await;
    goto(&engine, &site.url("/actions/form.html")).await;

    let out = act(&engine, select("#choice", &["g"])).await;
    assert_eq!(
        out.text,
        "selected \"Green\" in e1 combobox \"Colour choice\""
    );
    assert_eq!(out.quiet.as_deref(), Some("g"));
    assert_eq!(
        lines(&engine, "#log", 0).await,
        [
            "input choice \"g\" synthetic",
            "change choice \"g\" synthetic"
        ]
    );
    act(&engine, select("#choice", &["Red"])).await;
    assert_eq!(value(&engine, "#choice").await, "r");
    act(&engine, select("#choice", &["green"])).await;
    assert_eq!(value(&engine, "#choice").await, "g");
    let out = act(&engine, select("#toppings", &["cheese", "Ham"])).await;
    assert_eq!(out.data["selected"][1]["value"], "ham");
    let selected = call(
        &engine,
        "#toppings",
        "function () { return Array.from(this.selectedOptions, (o) => o.value).join(); }",
    )
    .await;
    assert_eq!(selected, "cheese,ham");
    // Options that come later are waited for.
    act(&engine, select("#late", &["Soon"])).await;
    assert_eq!(value(&engine, "#late").await, "soon");

    let err = fails(&engine, select("#choice", &["purple"])).await;
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(
        err.message,
        "no option of e1 combobox \"Colour choice\" has the value or label \"purple\""
    );
    assert_eq!(
        err.notes[0].text,
        "\"Pick one\", \"Red\" (r), \"Green\" (g), \"Blue\" (b) disabled"
    );
    assert_eq!(
        err.hint.as_deref(),
        Some("chrome-cdp select '#choice' Red --session keys-select")
    );
    let err = fails(&engine, select("#choice", &["Blue"])).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    let err = fails(&engine, select("#choice", &["r", "g"])).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    let err = fails(&engine, select("#combo", &["Apple"])).await;
    assert_eq!(err.message, "e4 combobox \"Fruit\" is not a <select>");
    assert_eq!(
        err.hint.as_deref(),
        Some(
            "chrome-cdp click e4 --session keys-select   (opens the list; then chrome-cdp click 'role=option[name=\"Apple\"]' --session keys-select)"
        )
    );

    engine.close().await;
}

#[tokio::test]
async fn checks_and_unchecks_what_needs_it() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-check").await;
    goto(&engine, &site.url("/actions/form.html")).await;

    let out = act(&engine, check("#remember", true)).await;
    assert_eq!(out.text, "checked e1 checkbox \"Remember me\"");
    assert!(checked(&engine, "#remember").await);
    assert_eq!(
        lines(&engine, "#log", 0).await,
        [
            "input remember true trusted",
            "change remember true trusted"
        ]
    );
    // Already so: nothing is clicked.
    let before = log_len(&engine, "#log").await;
    let out = act(&engine, check("e1", true)).await;
    assert_eq!(out.text, "e1 checkbox \"Remember me\" was already checked");
    assert_eq!(out.data["changed"], false);
    assert_eq!(log_len(&engine, "#log").await, before);
    act(&engine, check("#remember", false)).await;
    assert!(!checked(&engine, "#remember").await);

    // Inputs styled away behind their label are checked by clicking it.
    let out = act(&engine, check("#offers", true)).await;
    assert_eq!(
        out.text,
        "checked e2 checkbox \"Send offers\" by its label e3 label \"Send offers\""
    );
    assert!(checked(&engine, "#offers").await);
    act(&engine, check("#terms", true)).await;
    assert!(checked(&engine, "#terms").await);
    // A label locator checks the control it labels.
    let out = act(&engine, check("text=Accept the terms", false)).await;
    assert!(out.text.starts_with("unchecked e"), "{}", out.text);
    assert!(!checked(&engine, "#terms").await);

    // ARIA checkboxes and switches, which update a little later.
    act(&engine, check("#custom", true)).await;
    let state = call(
        &engine,
        "#custom",
        "function () { return this.getAttribute('aria-checked'); }",
    )
    .await;
    assert_eq!(state, "true");
    let out = act(&engine, check("role=switch", false)).await;
    assert_eq!(out.text, "unchecked e7 switch \"Dark mode\"");

    // Radios check, but do not uncheck.
    act(&engine, check("#express", true)).await;
    assert!(!checked(&engine, "#standard").await);
    let err = fails(&engine, check("#express", false)).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert!(
        err.message.contains("cannot be unchecked"),
        "{}",
        err.message
    );

    // A click the page ignores, a disabled box, and what is no checkbox.
    let err = fails(&engine, check("#stubborn", true)).await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    assert!(
        err.message
            .ends_with("checkbox \"Stubborn\" did not check it"),
        "{}",
        err.message
    );
    let err = fails(&engine, check("#locked-box", true)).await;
    assert_eq!(
        err.message,
        "#locked-box is disabled: it has the disabled attribute"
    );
    let err = fails(&engine, check("#email", true)).await;
    assert!(
        err.message
            .ends_with("is not a checkbox, radio button or switch"),
        "{}",
        err.message
    );

    engine.close().await;
}

#[tokio::test]
async fn focuses_and_blurs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-focus").await;
    goto(&engine, &site.url("/actions/keyboard.html")).await;

    let out = act(
        &engine,
        Action::Focus {
            locator: "#btn".to_owned(),
        },
    )
    .await;
    assert_eq!(out.text, "focused e1 button \"Button\"");
    let active = call(
        &engine,
        "#btn",
        "function () { return document.activeElement === this; }",
    )
    .await;
    assert_eq!(active, true);
    let err = fails(
        &engine,
        Action::Focus {
            locator: "#plain".to_owned(),
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::NotEnabled);
    let out = act(
        &engine,
        Action::Blur {
            locator: "#btn".to_owned(),
        },
    )
    .await;
    assert_eq!(out.text, "blurred e1 button \"Button\"");

    engine.close().await;
}

#[tokio::test]
async fn types_into_cross_origin_iframes() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "keys-frames").await;
    goto(&engine, &site.url("/actions/frames.html")).await;

    act(&engine, fill("frame=other >> #inner-field", "cross")).await;
    act(
        &engine,
        type_in(" origin", Some("frame=other >> #inner-field")),
    )
    .await;
    assert_eq!(
        value(&engine, "frame=other >> #inner-field").await,
        "cross origin"
    );
    // Keys go where focus is, in the iframe.
    act(&engine, press("Control+a", None)).await;
    act(&engine, type_in("X", None)).await;
    assert_eq!(value(&engine, "frame=other >> #inner-field").await, "X");

    engine.close().await;
}

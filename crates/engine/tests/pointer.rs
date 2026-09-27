//! The mouse and pointer actions against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the
//! fixture site's `actions/` pages (plan 05, milestone 5.2).
//!
//! Each test acts with [`Engine::act`] and checks what the page saw: the
//! mouse fixture logs every event it gets (type, button, buttons, click
//! count, modifiers, trusted), the others log drags, drops, scrolls and
//! touches.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, CdpRequest, Engine, LocateOptions, Modifier, MouseOp,
    NavigateRequest, Navigation, OpenRequest, ScrollTo, Target, WaitUntil,
};
use fixtures::Fixtures;
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;
const TIMEOUT: Duration = Duration::from_millis(TIMEOUT_MS);

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

async fn cdp(engine: &Engine, method: &str, params: Value) -> Value {
    let Value::Object(params) = params else {
        panic!("params must be an object")
    };
    engine
        .cdp(CdpRequest {
            method: method.to_owned(),
            params,
            target: Target::Page,
            tab: None,
            frame: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{method}: {}", e.to_text()))
        .data
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
        })
        .await
}

async fn act(engine: &Engine, action: Action) -> Output {
    let what = format!("{action:?}");
    act_within(engine, action, TIMEOUT)
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()))
}

/// The text of the one element `locator` matches.
async fn text(engine: &Engine, locator: &str) -> String {
    engine
        .call_on_locator(
            None,
            locator,
            &LocateOptions::default(),
            "function () { return this.textContent; }",
            TIMEOUT,
        )
        .await
        .unwrap_or_else(|e| panic!("{locator}: {}", e.to_text()))
        .value
        .as_str()
        .unwrap()
        .to_owned()
}

/// Evaluates `expression` in the page.
async fn eval(engine: &Engine, expression: &str) -> Value {
    cdp(
        engine,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    )
    .await["result"]["value"]
        .clone()
}

/// A click's options besides its locator.
#[derive(Default)]
struct Click {
    button: Button,
    count: u32,
    modifiers: Vec<Modifier>,
    position: Option<(f64, f64)>,
    force: bool,
    dblclick: bool,
}

impl Click {
    fn on(self, locator: &str) -> Action {
        Action::Click {
            locator: locator.to_owned(),
            button: self.button,
            count: self.count.max(1),
            modifiers: self.modifiers,
            position: self.position,
            force: self.force,
            dblclick: self.dblclick,
        }
    }
}

fn click(locator: &str) -> Action {
    Click::default().on(locator)
}

/// The mouse fixture's event log lines after its first `before` bytes.
async fn events(engine: &Engine, before: usize) -> Vec<String> {
    text(engine, "#events").await[before..]
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn log_len(engine: &Engine) -> usize {
    text(engine, "#events").await.len()
}

#[tokio::test]
async fn clicks_send_what_a_user_sends() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-click").await;
    goto(&engine, &site.url("/actions/mouse.html")).await;

    let out = act(&engine, click("#go")).await;
    assert_eq!(out.text, "clicked e1 button \"Go\"");
    assert_eq!(out.data["element"]["ref"], "e1");
    assert_eq!(
        events(&engine, 0).await,
        [
            "mouseenter go b0 bs0 d0 - trusted",
            "mousedown go b0 bs1 d1 - trusted",
            "mouseup go b0 bs0 d1 - trusted",
            "click go b0 bs0 d1 - trusted",
        ]
    );

    // A ref from the output acts on the same element.
    let before = log_len(&engine).await;
    let out = act(
        &engine,
        Click {
            count: 2,
            dblclick: true,
            ..Click::default()
        }
        .on("e1"),
    )
    .await;
    assert_eq!(out.text, "double-clicked e1 button \"Go\"");
    assert_eq!(
        events(&engine, before).await,
        [
            "mousedown go b0 bs1 d1 - trusted",
            "mouseup go b0 bs0 d1 - trusted",
            "click go b0 bs0 d1 - trusted",
            "mousedown go b0 bs1 d2 - trusted",
            "mouseup go b0 bs0 d2 - trusted",
            "click go b0 bs0 d2 - trusted",
            "dblclick go b0 bs0 d2 - trusted",
        ]
    );

    let before = log_len(&engine).await;
    let out = act(
        &engine,
        Click {
            button: Button::Right,
            modifiers: vec![Modifier::Shift, Modifier::Control],
            ..Click::default()
        }
        .on("#go"),
    )
    .await;
    assert_eq!(
        out.text,
        "right-clicked e1 button \"Go\" with Shift+Control"
    );
    assert_eq!(
        events(&engine, before).await,
        [
            "mousedown go b2 bs2 d1 ctrl+shift trusted",
            "contextmenu go b2 bs2 d0 ctrl+shift trusted",
            "mouseup go b2 bs0 d1 ctrl+shift trusted",
            "auxclick go b2 bs0 d1 ctrl+shift trusted",
        ]
    );

    let before = log_len(&engine).await;
    act(
        &engine,
        Click {
            button: Button::Middle,
            count: 3,
            ..Click::default()
        }
        .on("#go"),
    )
    .await;
    let middle = events(&engine, before).await;
    assert_eq!(middle.len(), 9, "{middle:#?}");
    assert_eq!(middle[6], "mousedown go b1 bs4 d3 - trusted");
    assert_eq!(middle[8], "auxclick go b1 bs0 d3 - trusted");

    engine.close().await;
}

#[tokio::test]
async fn positions_hover_and_raw_mouse_events() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-mouse").await;
    goto(&engine, &site.url("/actions/mouse.html")).await;

    // --position counts from the element's top-left corner.
    let before = log_len(&engine).await;
    let out = act(
        &engine,
        Click {
            position: Some((10.0, 20.0)),
            ..Click::default()
        }
        .on("#pad"),
    )
    .await;
    assert_eq!(out.data["point"], json!({ "x": 10.0, "y": 220.0 }));
    let pad = events(&engine, before).await;
    assert!(
        pad.contains(&"mousedown pad b0 bs1 d1 - trusted 10,20".to_owned()),
        "{pad:#?}"
    );

    // Hover applies :hover (the tooltip shows), even on a disabled button.
    let hover = |locator: &str| Action::Hover {
        locator: locator.to_owned(),
        modifiers: Vec::new(),
        position: None,
        force: false,
    };
    let out = act(&engine, hover("#menu")).await;
    assert_eq!(out.text, "hovered e2 span \"Menu\"");
    assert_eq!(
        eval(
            &engine,
            "getComputedStyle(document.getElementById('tip')).display"
        )
        .await,
        "block"
    );
    act(&engine, hover("#off")).await;

    // A press held across commands: moves carry the button.
    let mouse = |op| Action::Mouse(op);
    let before = log_len(&engine).await;
    act(
        &engine,
        mouse(MouseOp::Move {
            x: 10.0,
            y: 210.0,
            steps: 1,
        }),
    )
    .await;
    let out = act(
        &engine,
        mouse(MouseOp::Down {
            at: None,
            button: Button::Right,
            count: 1,
        }),
    )
    .await;
    assert_eq!(out.text, "pressed the right button at 10,210");
    assert_eq!(out.data["buttons"], 2);
    act(
        &engine,
        mouse(MouseOp::Move {
            x: 50.0,
            y: 250.0,
            steps: 2,
        }),
    )
    .await;
    let out = act(
        &engine,
        mouse(MouseOp::Up {
            at: None,
            button: None,
            count: 1,
        }),
    )
    .await;
    assert_eq!(out.text, "released the right button at 50,250");
    act(&engine, mouse(MouseOp::Wheel { dx: 0.0, dy: 120.0 })).await;
    assert_eq!(
        events(&engine, before).await,
        [
            "mousemove pad b0 bs0 d0 - trusted 10,10",
            "mousedown pad b2 bs2 d1 - trusted 10,10",
            "mousemove pad b0 bs2 d0 - trusted 30,30",
            "mousemove pad b0 bs2 d0 - trusted 50,50",
            "mouseup pad b2 bs0 d1 - trusted 50,50",
            "wheel pad b0 bs0 d0 - trusted 50,50",
        ]
    );

    engine.close().await;
}

#[tokio::test]
async fn force_skips_the_checks() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-force").await;
    goto(&engine, &site.url("/actions/states.html")).await;

    // Transparent: not visible to the checks, but it takes clicks.
    let err = act_within(&engine, click("#clear"), Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout, "{}", err.to_text());
    assert!(err.hint.unwrap().contains("--force"));
    act(
        &engine,
        Click {
            force: true,
            ..Click::default()
        }
        .on("#clear"),
    )
    .await;
    assert_eq!(text(&engine, "#log").await, "clear");

    goto(&engine, &site.url("/actions/obscured.html")).await;
    let err = act_within(&engine, click("#under"), Duration::from_millis(500))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Obscured);
    // With --force the click goes where it aims: to the banner on top.
    act(
        &engine,
        Click {
            force: true,
            ..Click::default()
        }
        .on("#under"),
    )
    .await;
    assert_eq!(text(&engine, "#log").await, "");

    engine.close().await;
}

#[tokio::test]
async fn a_click_that_opens_a_dialog_says_so() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-dialog").await;
    goto(&engine, &site.url("/actions/mouse.html")).await;

    let err = act_within(&engine, click("#ask"), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::DialogOpen, "{}", err.to_text());
    assert_eq!(err.exit_code(), 5);

    engine.close().await;
}

#[tokio::test]
async fn drags_with_html5_events_or_the_mouse() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-drag").await;
    let drag = |from: &str, to: &str| Action::Drag {
        from: from.to_owned(),
        to: to.to_owned(),
        steps: 10,
        force: false,
    };

    goto(&engine, &site.url("/actions/dnd.html")).await;
    let out = act(&engine, drag("#card1", "#done")).await;
    assert_eq!(out.data["method"], "html5");
    assert_eq!(
        text(&engine, "#log").await,
        "dragstart:card1 dragenter drop:card1 dragend:card1"
    );
    assert_eq!(
        eval(&engine, "document.querySelectorAll('#done .card').length").await,
        1
    );
    // The mouse is up again: a click works as usual.
    let out = act(&engine, drag("#card2", "#done")).await;
    assert_eq!(out.data["method"], "html5");
    assert_eq!(
        eval(&engine, "document.querySelectorAll('#done .card').length").await,
        2
    );

    goto(&engine, &site.url("/actions/sortable.html")).await;
    let out = act(&engine, drag("#d", "#a")).await;
    assert_eq!(out.data["method"], "pointer");
    assert_eq!(
        out.text,
        "dragged e4 listitem \"Delta\" to e5 listitem \"Alpha\" (mouse events)"
    );
    assert_eq!(text(&engine, "#order").await, "d a b c");
    assert_eq!(
        eval(&engine, "document.getElementById('order').dataset.moves").await,
        "10"
    );
    act(
        &engine,
        Action::Drag {
            from: "#a".to_owned(),
            to: "#c".to_owned(),
            steps: 3,
            force: false,
        },
    )
    .await;
    assert_eq!(text(&engine, "#order").await, "d b a c");

    engine.close().await;
}

#[tokio::test]
async fn scrolls_with_the_wheel() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-scroll").await;
    goto(&engine, &site.url("/actions/feed.html")).await;
    let scroll =
        |locator: Option<&str>, by: Option<(f64, f64)>, to: Option<ScrollTo>| Action::Scroll {
            locator: locator.map(str::to_owned),
            by,
            to,
            force: false,
        };

    let out = act(&engine, scroll(None, Some((0.0, 300.0)), None)).await;
    assert_eq!(out.data["y"], 300.0);
    assert_eq!(eval(&engine, "scrollY").await, 300);
    assert_ne!(text(&engine, "#wheels").await, "0", "the page saw no wheel");

    // To the bottom: the feed loads more, and the report says where the
    // end is now.
    let out = act(&engine, scroll(None, None, Some(ScrollTo::Bottom))).await;
    assert_eq!(out.data["at_bottom"], true, "{}", out.text);
    assert_ne!(text(&engine, "#loaded").await, "0", "nothing loaded");
    assert_eq!(
        out.data["y"].as_f64(),
        eval(&engine, "scrollY").await.as_f64()
    );

    let out = act(&engine, scroll(None, None, Some(ScrollTo::Top))).await;
    assert_eq!(out.data["y"], 0.0);
    assert_eq!(out.data["at_top"], true);

    // In a scroll container: it scrolls, not the page.
    let out = act(&engine, scroll(Some("#inbox"), Some((0.0, 100.0)), None)).await;
    assert_eq!(out.data["container"]["name"], "Inbox");
    assert_eq!(out.data["y"], 100.0);
    assert_eq!(eval(&engine, "scrollY").await, 0);
    let out = act(
        &engine,
        scroll(Some("#inbox"), None, Some(ScrollTo::Bottom)),
    )
    .await;
    assert_eq!(out.data["at_bottom"], true);
    assert_eq!(
        eval(&engine, "document.getElementById('inbox').scrollTop")
            .await
            .as_f64(),
        out.data["y"].as_f64()
    );

    // A locator alone scrolls it into view.
    let out = act(&engine, scroll(Some("#end"), None, None)).await;
    assert!(
        out.text.contains("button \"End\" is in view"),
        "{}",
        out.text
    );
    assert_eq!(
        eval(
            &engine,
            "document.getElementById('end').getBoundingClientRect().bottom <= innerHeight"
        )
        .await,
        true
    );

    engine.close().await;
}

#[tokio::test]
async fn taps_need_touch_emulation() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-tap").await;
    goto(&engine, &site.url("/actions/touch.html")).await;
    let tap = |locator: &str| Action::Tap {
        locator: locator.to_owned(),
        modifiers: Vec::new(),
        position: None,
        force: false,
    };

    let err = act_within(&engine, tap("#tap"), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert!(err.message.contains("touch emulation"), "{}", err.message);

    cdp(
        &engine,
        "Emulation.setTouchEmulationEnabled",
        json!({ "enabled": true }),
    )
    .await;
    let out = act(&engine, tap("#tap")).await;
    assert_eq!(out.text, "tapped e1 button \"Tap me\"");
    assert_eq!(
        text(&engine, "#log").await,
        "touchstart:tap touchend:tap click:tap"
    );
    // A touchend the page cancels stops the click.
    act(&engine, tap("#cancels")).await;
    assert_eq!(
        text(&engine, "#log").await,
        "touchstart:tap touchend:tap click:tap touchstart:cancels touchend:cancels"
    );

    engine.close().await;
}

#[tokio::test]
async fn clicks_reach_cross_origin_iframes() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "pointer-frames").await;
    goto(&engine, &site.url("/actions/frames.html")).await;

    let out = act(&engine, click("frame=other >> #inner")).await;
    assert!(out.text.starts_with("clicked e"), "{}", out.text);
    assert_eq!(text(&engine, "frame=other >> #log").await, "inner");

    engine.close().await;
}

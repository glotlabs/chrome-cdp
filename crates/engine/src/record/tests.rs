use std::collections::{HashMap, HashSet};

use super::*;

fn recording(events: Vec<Raw>) -> Recording {
    Recording {
        name: "t".into(),
        scope: None,
        started_ms: 0,
        tab: 1,
        existing: HashSet::from([1]),
        opened: HashMap::new(),
        asked: HashSet::new(),
        answered: HashMap::new(),
        events,
    }
}

fn page(tab: u32, json: &str) -> Raw {
    framed(tab, Frame::Main, json)
}

fn framed(tab: u32, frame: Frame, json: &str) -> Raw {
    Raw {
        tab,
        what: What::Page {
            frame,
            event: serde_json::from_str(json).unwrap(),
        },
    }
}

fn nav(tab: u32, how: Nav, url: &str) -> Raw {
    Raw {
        tab,
        what: What::Navigate {
            how,
            url: url.into(),
        },
    }
}

/// The lines, comments first, as the CLI writes them (without quoting).
fn lines(r: &Recording) -> Vec<String> {
    let (steps, _) = steps(r);
    steps
        .into_iter()
        .flat_map(|s| {
            let mut out = s.comments;
            if !s.words.is_empty() {
                out.push(s.words.join(" "));
            }
            out
        })
        .collect()
}

#[test]
fn typing_is_one_fill_when_another_event_comes() {
    let r = recording(vec![
        page(1, r#"{"k":"click","loc":"label=Name","how":"label"}"#),
        page(1, r#"{"k":"input","loc":"label=Name","value":"A"}"#),
        page(1, r#"{"k":"input","loc":"label=Name","value":"Ad"}"#),
        page(1, r#"{"k":"input","loc":"label=Name","value":"Ada"}"#),
        page(1, r#"{"k":"press","key":"Tab"}"#),
        page(1, r#"{"k":"input","loc":"label=City","value":"Oslo"}"#),
        page(1, r#"{"k":"input","loc":"label=Zip","value":"0150"}"#),
    ]);
    // The click that put the caret in Name adds nothing; the last
    // typing is written at the end.
    assert_eq!(
        lines(&r),
        [
            "fill label=Name Ada",
            "press Tab",
            "fill label=City Oslo",
            "fill label=Zip 0150",
        ]
    );
}

#[test]
fn a_page_report_does_not_end_typing() {
    let r = recording(vec![
        page(1, r#"{"k":"input","loc":"label=Q","value":"a"}"#),
        page(1, r#"{"k":"page","url":"http://x/#a","index":1}"#),
        page(1, r#"{"k":"input","loc":"label=Q","value":"ab"}"#),
    ]);
    assert_eq!(lines(&r), ["fill label=Q ab"]);
}

#[test]
fn passwords_are_a_secret_never_a_value() {
    let r = recording(vec![
        page(1, r#"{"k":"input","loc":"label=Password","secret":true}"#),
        page(1, r#"{"k":"press","key":"Enter"}"#),
    ]);
    let (steps, _) = steps(&r);
    assert_eq!(
        steps[0].words,
        ["fill", "label=Password", "${secret:PASSWORD}"]
    );
    assert!(steps[0].comments[0].contains("PASSWORD secret"));
    assert_eq!(steps[1].words, ["press", "Enter"]);
}

#[test]
fn clicks_checks_choices_and_files() {
    let r = recording(vec![
        page(1, r#"{"k":"click","loc":"role=button[name=\"Go\"]"}"#),
        page(1, r#"{"k":"click","loc":"text=Row"}"#),
        page(1, r#"{"k":"click","loc":"text=Row"}"#),
        page(1, r#"{"k":"dblclick","loc":"text=Row"}"#),
        page(1, r#"{"k":"check","loc":"label=Terms","checked":true}"#),
        page(1, r#"{"k":"check","loc":"label=News","checked":false}"#),
        page(1, r#"{"k":"select","loc":"label=Size","values":["m","l"]}"#),
        page(
            1,
            r#"{"k":"upload","loc":"label=Photo","names":["me.png"]}"#,
        ),
        page(1, r#"{"k":"select","loc":"label=Size","values":[]}"#),
    ]);
    assert_eq!(
        lines(&r),
        [
            "click role=button[name=\"Go\"]",
            "dblclick text=Row",
            "check label=Terms",
            "uncheck label=News",
            "select label=Size m l",
            "# TODO: path: the file chosen was me.png; give its path",
            "upload label=Photo me.png",
        ]
    );
}

#[test]
fn an_element_without_a_locator_is_a_comment_and_a_warning() {
    let r = recording(vec![
        page(1, r#"{"k":"click","reason":"it has no role and name"}"#),
        page(1, r#"{"k":"click","loc":"text=A >> nth=2","how":"nth"}"#),
    ]);
    let (steps, warnings) = steps(&r);
    assert!(steps[0].words.is_empty());
    assert!(
        steps[0].comments[0].contains("it has no role and name"),
        "{steps:?}"
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[1].contains("by position"));
}

#[test]
fn iframes_get_their_frame_step() {
    let r = recording(vec![
        framed(
            1,
            Frame::Step("frame=pay".into()),
            r#"{"k":"input","loc":"label=Card","value":"4242"}"#,
        ),
        framed(
            1,
            Frame::Step("frame=pay".into()),
            r#"{"k":"click","loc":"role=button[name=Pay]"}"#,
        ),
        framed(1, Frame::Unnamed, r#"{"k":"click","loc":"text=Ad"}"#),
    ]);
    let l = lines(&r);
    assert_eq!(l[0], "fill frame=pay >> label=Card 4242");
    assert_eq!(l[1], "click frame=pay >> role=button[name=Pay]");
    assert!(l[2].starts_with("# in an iframe"), "{l:?}");
    assert_eq!(l[3], "click text=Ad");
}

#[test]
fn typing_in_two_frames_with_one_locator_is_two_fills() {
    let r = recording(vec![
        framed(
            1,
            Frame::Main,
            r#"{"k":"input","loc":"label=Q","value":"a"}"#,
        ),
        framed(
            1,
            Frame::Step("frame=f".into()),
            r#"{"k":"input","loc":"label=Q","value":"b"}"#,
        ),
    ]);
    assert_eq!(lines(&r), ["fill label=Q a", "fill frame=f >> label=Q b"]);
}

#[test]
fn navigations_the_browser_started() {
    let r = recording(vec![
        page(1, r#"{"k":"page","url":"http://x/a","index":0}"#),
        nav(1, Nav::Goto, "http://x/b"),
        page(1, r#"{"k":"page","url":"http://x/b","index":1}"#),
        nav(1, Nav::History, "http://x/a"),
        page(1, r#"{"k":"page","url":"http://x/a","index":0}"#),
        nav(1, Nav::History, "http://x/b"),
        page(1, r#"{"k":"page","url":"http://x/b","index":1}"#),
        nav(1, Nav::Reload, "http://x/b"),
        nav(1, Nav::Goto, "about:blank"),
    ]);
    assert_eq!(lines(&r), ["goto http://x/b", "back", "forward", "reload"]);
}

#[test]
fn dialogs_answered() {
    let r = recording(vec![
        page(1, r#"{"k":"click","loc":"text=Delete"}"#),
        Raw {
            tab: 1,
            what: What::Dialog {
                accept: true,
                text: None,
            },
        },
        page(1, r#"{"k":"click","loc":"text=Name"}"#),
        Raw {
            tab: 1,
            what: What::Dialog {
                accept: true,
                text: Some("Ada".into()),
            },
        },
        Raw {
            tab: 1,
            what: What::Dialog {
                accept: false,
                text: Some("x".into()),
            },
        },
    ]);
    assert_eq!(
        lines(&r),
        [
            "click text=Delete",
            "dialog accept",
            "click text=Name",
            "dialog accept Ada",
            "dialog dismiss",
        ]
    );
}

#[test]
fn tabs_new_popups_and_switches() {
    let mut r = recording(Vec::new());
    r.existing.insert(2);
    let mut push = |tab: u32, opener: bool, raw: Raw| r.push(tab, opener, raw.what);
    // A tab opened by hand that loads a URL, then a step in it.
    push(3, false, nav(3, Nav::Goto, "http://x/new"));
    push(3, false, page(3, r#"{"k":"click","loc":"text=A"}"#));
    // A popup the page opened: its first load is no step.
    push(4, true, nav(4, Nav::Goto, "http://x/popup"));
    push(4, true, page(4, r#"{"k":"click","loc":"text=B"}"#));
    // Back to the first tab, and to one there was at the start.
    push(1, false, page(1, r#"{"k":"click","loc":"text=C"}"#));
    push(2, false, page(2, r#"{"k":"click","loc":"text=D"}"#));
    assert_eq!(
        lines(&r),
        [
            "tab new http://x/new",
            "click text=A",
            "tab t4",
            "click text=B",
            "tab t1",
            "click text=C",
            "tab t2",
            "click text=D",
        ]
    );
}

#[test]
fn a_new_tab_without_a_url() {
    let mut r = recording(Vec::new());
    r.push(2, false, page(2, r#"{"k":"press","key":"Control+l"}"#).what);
    assert_eq!(lines(&r), ["tab new", "press Control+l"]);
}

#[test]
fn a_key_names_its_element_when_focus_is_elsewhere() {
    let r = recording(vec![
        page(1, r#"{"k":"input","loc":"label=Name","value":"Ada"}"#),
        page(1, r#"{"k":"press","key":"Enter","loc":"label=Name"}"#),
        framed(
            1,
            Frame::Step("frame=card".into()),
            r#"{"k":"click","loc":"text=Pay"}"#,
        ),
        page(1, r#"{"k":"press","key":"Enter","loc":"label=Name"}"#),
        page(1, r#"{"k":"press","key":"Tab","loc":"label=Name"}"#),
        page(1, r#"{"k":"press","key":"Enter","loc":"label=Email"}"#),
        page(1, r#"{"k":"press","key":"Escape"}"#),
    ]);
    assert_eq!(
        lines(&r),
        [
            "fill label=Name Ada",
            "press Enter",
            "click frame=card >> text=Pay",
            "press Enter --in label=Name",
            "press Tab",
            "press Enter",
            "press Escape",
        ]
    );
}

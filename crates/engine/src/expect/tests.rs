use protocol::ErrorKind;

use super::{
    Assertion, absent, as_regex, assertion_text, check_expect, expected, parse_matcher, pattern,
};
use crate::locator::LocateOptions;
use crate::request::{A11yLevel, ExpectRequest, Expectation, Matcher};

fn request(expectation: Expectation, not: bool) -> ExpectRequest {
    ExpectRequest {
        expectation,
        not,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: 5000,
        max_chars: 20_000,
    }
}

fn element(locator: &str, matcher: Matcher) -> Expectation {
    Expectation::Element {
        locator: locator.into(),
        matcher,
    }
}

fn matcher(name: &str, args: &[&str]) -> Result<Matcher, protocol::ErrorInfo> {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    parse_matcher("e1", name, &args, false, None, None)
}

#[test]
fn matchers_take_their_values() {
    assert_eq!(matcher("visible", &[]).unwrap(), Matcher::Visible);
    assert_eq!(matcher("in-viewport", &[]).unwrap(), Matcher::InViewport);
    assert_eq!(
        matcher("attr", &["href"]).unwrap(),
        Matcher::Attr {
            name: "href".into(),
            value: None
        }
    );
    assert_eq!(
        matcher("values", &["a", "b"]).unwrap(),
        Matcher::Values {
            values: vec!["a".into(), "b".into()]
        }
    );
    assert_eq!(
        matcher("count", &["3"]).unwrap(),
        Matcher::Count {
            count: Some(3),
            min: None,
            max: None
        }
    );
    for (name, args) in [
        ("visible", &["x"][..]),
        ("text", &[]),
        ("css", &["color"]),
        ("count", &["-1"]),
        ("count", &[]),
        ("values", &[]),
        ("nope", &[]),
    ] {
        let err = matcher(name, args).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs, "{name} {args:?}");
        assert!(err.hint.is_some(), "{name} {args:?}");
    }
}

#[test]
fn regexes_are_slashes_with_flags() {
    assert_eq!(as_regex("/^Saved$/i"), Some(("^Saved$", "i")));
    assert_eq!(as_regex("/a/b/"), Some(("a/b", "")));
    assert_eq!(as_regex("/x/q"), None);
    assert_eq!(as_regex("//"), None);
    assert_eq!(as_regex("Saved"), None);
    assert_eq!(pattern("Saved", false)["mode"], "substring");
    assert_eq!(pattern("Saved", true)["mode"], "exact");
    assert_eq!(pattern("/Sav/", false)["mode"], "regex");
}

#[test]
fn what_is_expected_in_words() {
    let words = |e: Expectation, not: bool| expected(&request(e, not), None);
    assert_eq!(
        words(
            element(
                "role=alert",
                Matcher::Text {
                    text: "Saved".into(),
                    exact: false
                }
            ),
            false
        ),
        "text containing \"Saved\""
    );
    assert_eq!(words(element("e1", Matcher::Visible), true), "not visible");
    assert_eq!(
        words(
            element(
                ".row",
                Matcher::Count {
                    count: None,
                    min: Some(2),
                    max: Some(5)
                }
            ),
            false
        ),
        "2 to 5 elements"
    );
    assert_eq!(
        words(
            element(
                "a",
                Matcher::Attr {
                    name: "href".into(),
                    value: Some("/^https:/".into())
                }
            ),
            false
        ),
        "attribute href matching /^https:/"
    );
    assert_eq!(
        expected(
            &request(
                Expectation::A11y {
                    scope: None,
                    level: A11yLevel::A
                },
                false
            ),
            Some((9, A11yLevel::A))
        ),
        "no violations of 9 accessibility rules (WCAG level A)"
    );
}

#[test]
fn the_assertion_reads_as_typed() {
    let mut req = request(
        element(
            "role=button[name=\"Save\"]",
            Matcher::Text {
                text: "Saved!".into(),
                exact: true,
            },
        ),
        true,
    );
    req.tab = Some("t2".into());
    assert_eq!(
        assertion_text(&req),
        "'role=button[name=\"Save\"]' text 'Saved!' --exact --not --tab t2"
    );
    let req = request(
        Expectation::NoFailedRequests {
            since: Some("before".into()),
            origin: Some("self".into()),
            all_tabs: false,
        },
        false,
    );
    assert_eq!(
        assertion_text(&req),
        "--no-failed-requests --origin self --since before"
    );
}

#[test]
fn absent_elements_pass_only_what_needs_none() {
    assert_eq!(absent(&Matcher::Hidden), Some(true));
    assert_eq!(absent(&Matcher::Visible), Some(false));
    assert_eq!(
        absent(&Matcher::Count {
            count: None,
            min: None,
            max: Some(2)
        }),
        Some(true)
    );
    assert_eq!(
        absent(&Matcher::Count {
            count: Some(1),
            min: None,
            max: None
        }),
        Some(false)
    );
    assert_eq!(absent(&Matcher::Checked), None);
}

#[test]
fn bad_arguments_fail_before_the_browser() {
    let bad = [
        request(element("role=", Matcher::Visible), false),
        request(
            Expectation::Url {
                pattern: "/(/".into(),
            },
            false,
        ),
        request(
            Expectation::NoErrors {
                since: None,
                all_tabs: false,
            },
            true,
        ),
        request(
            Expectation::NoFailedRequests {
                since: None,
                origin: Some("nowhere".into()),
                all_tabs: false,
            },
            false,
        ),
        request(
            element(
                ".row",
                Matcher::Count {
                    count: None,
                    min: Some(5),
                    max: Some(2),
                },
            ),
            false,
        ),
    ];
    for req in bad {
        let err = check_expect(&req).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{req:?}: {}", err.to_text());
    }
    let ok = request(element("#main", Matcher::Visible), false);
    assert!(Assertion::parse(&ok, &LocateOptions::default()).is_ok());
}

#[test]
fn mock_state_assertions_read_back() {
    use crate::request::MockStateCheck;
    use serde_json::json;
    let state = |name: &str, check| Expectation::MockState {
        name: name.into(),
        check,
    };
    let count = request(
        state(
            "todos",
            MockStateCheck::Count {
                count: Some(4),
                min: None,
                max: Some(9),
            },
        ),
        false,
    );
    assert_eq!(assertion_text(&count), "mock-state todos count 4 --max 9");
    assert_eq!(
        expected(&count, None),
        "todos with 4 items, at most 9 items"
    );
    let value = request(
        state(
            "user",
            MockStateCheck::Value {
                value: json!("ada@example.com"),
            },
        ),
        true,
    );
    assert_eq!(
        assertion_text(&value),
        "mock-state user value ada@example.com --not"
    );
    assert_eq!(expected(&value, None), "not user = \"ada@example.com\"");
    let contains = request(
        state(
            "todos",
            MockStateCheck::Contains {
                value: json!({"title": "Milk"}),
            },
        ),
        false,
    );
    assert_eq!(
        assertion_text(&contains),
        r#"mock-state todos contains '{"title":"Milk"}'"#
    );
    check_expect(&contains).unwrap();
    let bad = request(
        state(
            "todos",
            MockStateCheck::Count {
                count: None,
                min: Some(3),
                max: Some(1),
            },
        ),
        false,
    );
    assert_eq!(check_expect(&bad).unwrap_err().kind, ErrorKind::BadArgs);
}

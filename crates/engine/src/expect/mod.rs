//! `expect` and `mark` (plan 09, milestone 9.1).
//!
//! `expect` asserts something about the page and retries until it holds
//! or its timeout (5s by default) passes, like Playwright's web-first
//! assertions. Element matchers and `--title` are the wait engine's probe
//! (see [`crate::wait`]) running the checks of the injected `expect.ts` on
//! every animation frame; `--url` watches the frame tree. `--no-errors`,
//! `--no-failed-requests` and `--a11y` look once (see [`page`]): what
//! they find does not go away by waiting.
//!
//! A failure is `ExpectFailed` (exit 1) with what was expected, what was
//! seen (after how long and how many tries), the element, and a hint.

mod baseline;
mod mock_state;
mod page;
mod pixels;
mod requests;

pub use baseline::{check_expect_screenshot, check_expect_snapshot};

use std::fmt::Write as _;
use std::time::Duration;

use cdp::domains::runtime;
use protocol::schema::{ExpectData, MarkData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format, suggest};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use crate::context::Scoped as _;
use crate::injected::{self, GUARD};
use crate::locator::{FrameOnly, LocateOptions, Locator, read_listed};
use crate::pattern::UrlPattern;
use crate::request::{
    A11yLevel, ExpectRequest, Expectation, MarkRequest, Matcher, MockStateCheck, RequestCheck,
    SnapshotEngine, millis,
};
use crate::tabs::{Page, tab_name};
use crate::wait::Condition;
use crate::{Engine, Running};

/// The matchers `expect <locator>` takes, as typed.
pub const MATCHERS: &[&str] = &[
    "visible",
    "hidden",
    "attached",
    "detached",
    "enabled",
    "disabled",
    "editable",
    "checked",
    "unchecked",
    "focused",
    "text",
    "value",
    "values",
    "attr",
    "class",
    "css",
    "count",
    "in-viewport",
];

/// How many matches a failure names.
const NAMED: usize = 3;

/// The object group of the elements a failure names, until read.
const GROUP: &str = "chrome-cdp-expect";

/// Longer values are cut in messages.
const SHOWN: usize = 200;

/// Builds the matcher of `expect <locator> <name> [args]`: the words
/// after the locator, `--exact` (for `text`) and `--min`/`--max` (for
/// `count`).
pub fn parse_matcher(
    locator: &str,
    name: &str,
    args: &[String],
    exact: bool,
    min: Option<u64>,
    max: Option<u64>,
) -> Result<Matcher, ErrorInfo> {
    let example = |rest: &str| Cmd::new("expect").arg(locator).raw(rest);
    let Some(&known) = MATCHERS.iter().find(|m| **m == name) else {
        let mut info = ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{name:?} is not a matcher of expect"),
        )
        .note("matchers", MATCHERS.join(", "));
        info = match suggest::similar(name, MATCHERS.iter().copied(), 1).first() {
            Some(close) => info.hint(Hint::run(example(close)).why("did you mean this?")),
            None => info.hint(Cmd::new("expect").switch("help").to_string()),
        };
        return Err(info);
    };
    let usage = |shape: &str, sample: &str| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "expect … {known} takes {shape}, got {}",
                count_words(args.len())
            ),
        )
        .hint(Hint::run(example(sample)))
    };
    if exact && known != "text" {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--exact is for the text matcher (and --title), not {known}"),
        )
        .hint(Hint::run(example("text Saved --exact"))));
    }
    if (min.is_some() || max.is_some()) && known != "count" {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--min and --max are for the count matcher, not {known}"),
        )
        .hint(Hint::run(example("count --min 1"))));
    }
    let arg = |i: usize| args[i].clone();
    let matcher = match (known, args.len()) {
        ("text", 1) => Matcher::Text {
            text: arg(0),
            exact,
        },
        ("text", _) => return Err(usage("one text (or /regex/)", "text Saved")),
        ("value", 1) => Matcher::Value { value: arg(0) },
        ("value", _) => return Err(usage("one value (or /regex/)", "value alice@example.com")),
        ("values", 0) => return Err(usage("the values selected", "values red green")),
        ("values", _) => Matcher::Values {
            values: args.to_vec(),
        },
        ("attr", 1 | 2) => Matcher::Attr {
            name: arg(0),
            value: args.get(1).cloned(),
        },
        ("attr", _) => return Err(usage("a name and maybe a value", "attr aria-expanded true")),
        ("class", 1) => Matcher::Class { name: arg(0) },
        ("class", _) => return Err(usage("one class (or /regex/)", "class active")),
        ("css", 2) => Matcher::Css {
            property: arg(0),
            value: arg(1),
        },
        ("css", _) => return Err(usage("a property and a value", "css color red")),
        ("count", 0 | 1) => {
            let count = args
                .first()
                .map(|n| {
                    n.parse::<u64>().map_err(|_| {
                        ErrorInfo::new(
                            ErrorKind::BadArgs,
                            format!("expect … count takes a number, got {n:?}"),
                        )
                        .hint(Hint::run(example("count 3")))
                    })
                })
                .transpose()?;
            if count.is_none() && min.is_none() && max.is_none() {
                return Err(usage("a number, or --min/--max", "count 3"));
            }
            Matcher::Count { count, min, max }
        }
        ("count", _) => return Err(usage("one number", "count 3")),
        (_, n) if n > 0 => {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!(
                    "expect … {known} takes no value, got {}: {}",
                    count_words(n),
                    args.join(" ")
                ),
            )
            .hint(Hint::run(example(known))));
        }
        ("visible", _) => Matcher::Visible,
        ("hidden", _) => Matcher::Hidden,
        ("attached", _) => Matcher::Attached,
        ("detached", _) => Matcher::Detached,
        ("enabled", _) => Matcher::Enabled,
        ("disabled", _) => Matcher::Disabled,
        ("editable", _) => Matcher::Editable,
        ("checked", _) => Matcher::Checked,
        ("unchecked", _) => Matcher::Unchecked,
        ("focused", _) => Matcher::Focused,
        _ => Matcher::InViewport,
    };
    Ok(matcher)
}

fn count_words(n: usize) -> String {
    match n {
        0 => "none".into(),
        1 => "1 value".into(),
        n => format!("{n} values"),
    }
}

/// Checks an `expect` request's arguments (locators, patterns, flags that
/// do not go together) without a browser.
pub fn check_expect(req: &ExpectRequest) -> Result<(), ErrorInfo> {
    Assertion::parse(req, &LocateOptions::default()).map(drop)
}

/// The name of a matcher, as typed.
fn matcher_name(matcher: &Matcher) -> &'static str {
    match matcher {
        Matcher::Visible => "visible",
        Matcher::Hidden => "hidden",
        Matcher::Attached => "attached",
        Matcher::Detached => "detached",
        Matcher::Enabled => "enabled",
        Matcher::Disabled => "disabled",
        Matcher::Editable => "editable",
        Matcher::Checked => "checked",
        Matcher::Unchecked => "unchecked",
        Matcher::Focused => "focused",
        Matcher::InViewport => "in-viewport",
        Matcher::Text { .. } => "text",
        Matcher::Value { .. } => "value",
        Matcher::Values { .. } => "values",
        Matcher::Attr { .. } => "attr",
        Matcher::Class { .. } => "class",
        Matcher::Css { .. } => "css",
        Matcher::Count { .. } => "count",
    }
}

/// `/body/flags` as a regex's body and flags.
fn as_regex(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('/')?;
    let end = rest.rfind('/')?;
    let (body, flags) = (&rest[..end], &rest[end + 1..]);
    (!body.is_empty() && flags.chars().all(|c| "dgimsuvy".contains(c))).then_some((body, flags))
}

/// A text to compare with, as `expect.ts` takes it: a regex for
/// `/re/flags`, else all of it (`exact`) or a part.
fn pattern(text: &str, exact: bool) -> Value {
    match as_regex(text) {
        Some((body, flags)) => json!({ "mode": "regex", "value": body, "flags": flags }),
        None if exact => json!({ "mode": "exact", "value": text }),
        None => json!({ "mode": "substring", "value": text }),
    }
}

/// How the output shows a text to compare with: `containing "Saved"`,
/// `"Saved"` or `matching /Sav(ed|ing)/`.
fn shown_pattern(text: &str, exact: bool) -> String {
    if as_regex(text).is_some() {
        format!("matching {text}")
    } else if exact {
        format!("{:?}", cut(text))
    } else {
        format!("containing {:?}", cut(text))
    }
}

fn cut(text: &str) -> String {
    if text.chars().count() > SHOWN {
        let mut out: String = text.chars().take(SHOWN - 1).collect();
        out.push('…');
        out
    } else {
        text.to_owned()
    }
}

/// An assertion, checked and compiled.
#[derive(Debug)]
enum Assertion {
    /// Polled in the page: an element matcher or `--title`.
    Polled {
        condition: Condition,
        matcher: Option<Matcher>,
    },
    Url(UrlPattern),
    NoErrors {
        since: Option<String>,
    },
    NoFailedRequests {
        since: Option<String>,
        origin: Option<String>,
    },
    A11y {
        scope: Option<Locator>,
        level: A11yLevel,
    },
    /// Looked at in the session's mocks, not in a page.
    MockState,
    /// Looked at in the network logs of the session's (or context's)
    /// tabs.
    Request,
}

impl Assertion {
    fn parse(req: &ExpectRequest, options: &LocateOptions) -> Result<Self, ErrorInfo> {
        let no_not = |what: &str| {
            if req.not {
                Err(
                    ErrorInfo::new(ErrorKind::BadArgs, format!("--not does not go with {what}"))
                        .hint(Hint::text(
                            "--not is for element matchers, --url and --title",
                        )),
                )
            } else {
                Ok(())
            }
        };
        Ok(match &req.expectation {
            Expectation::Element { locator, matcher } => {
                let locator = options.parse(locator)?;
                check_matcher(matcher)?;
                Self::Polled {
                    condition: Condition::Expect {
                        locator: Some(locator),
                        check: check(matcher),
                        not: req.not,
                        absent: absent(matcher),
                    },
                    matcher: Some(matcher.clone()),
                }
            }
            Expectation::Url { pattern } => Self::Url(UrlPattern::parse(pattern)?),
            Expectation::Title { text, exact } => Self::Polled {
                condition: Condition::Expect {
                    locator: None,
                    check: json!({ "matcher": "title", "pattern": pattern(text, *exact) }),
                    not: req.not,
                    absent: None,
                },
                matcher: None,
            },
            Expectation::NoErrors { since, all_tabs } => {
                no_not("--no-errors")?;
                if *all_tabs && (since.is_some() || req.tab.is_some()) {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        "--all-tabs looks at every tab since it opened; it takes no --since or --tab",
                    )
                    .hint(Hint::run(
                        Cmd::new("expect").switch("no-errors").switch("all-tabs"),
                    )));
                }
                Self::NoErrors {
                    since: since.clone(),
                }
            }
            Expectation::NoFailedRequests { since, origin } => {
                no_not("--no-failed-requests")?;
                if let Some(origin) = origin
                    && origin != "self"
                    && crate::input::origin(origin).is_none()
                {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("--origin {origin:?} is not an origin"),
                    )
                    .hint(Hint::text(
                        "--origin takes self (the page's origin) or an origin such as https://api.example.com",
                    )));
                }
                Self::NoFailedRequests {
                    since: since.clone(),
                    origin: origin.clone(),
                }
            }
            Expectation::A11y { scope, level } => {
                no_not("--a11y")?;
                Self::A11y {
                    scope: scope.as_deref().map(|s| options.parse(s)).transpose()?,
                    level: *level,
                }
            }
            Expectation::MockState { name, check } => {
                if name.trim().is_empty() {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        "expect mock-state needs the name of a named value or collection",
                    )
                    .hint(Hint::run(
                        Cmd::new("expect")
                            .arg("mock-state")
                            .arg("todos")
                            .raw("count 3"),
                    )));
                }
                if let MockStateCheck::Count {
                    min: Some(min),
                    max: Some(max),
                    ..
                } = check
                    && min > max
                {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("--min {min} is more than --max {max}"),
                    ));
                }
                Self::MockState
            }
            Expectation::Request(check) => {
                if req.not {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        "--not does not go with expect request",
                    )
                    .hint(Hint::run(requests::request_command(
                        Cmd::new("expect"),
                        &RequestCheck {
                            none: !check.none,
                            count: None,
                            min: None,
                            max: None,
                            ..check.clone()
                        },
                    ))));
                }
                requests::filter(check)?;
                Self::Request
            }
        })
    }
}

/// Arguments of a matcher that can never hold.
fn check_matcher(matcher: &Matcher) -> Result<(), ErrorInfo> {
    let bad = |message: String| Err(ErrorInfo::new(ErrorKind::BadArgs, message));
    match matcher {
        Matcher::Attr { name, .. } if name.trim().is_empty() => {
            bad("expect … attr needs an attribute name".into())
        }
        Matcher::Css { property, .. } if property.trim().is_empty() => {
            bad("expect … css needs a property name".into())
        }
        Matcher::Class { name } if name.trim().is_empty() => {
            bad("expect … class needs a class name".into())
        }
        Matcher::Values { values } if values.is_empty() => {
            bad("expect … values needs the values selected".into())
        }
        Matcher::Count {
            min: Some(min),
            max: Some(max),
            ..
        } if min > max => bad(format!("--min {min} is more than --max {max}")),
        _ => Ok(()),
    }
}

/// The check `expect.ts` runs for `matcher`.
fn check(matcher: &Matcher) -> Value {
    let name = matcher_name(matcher);
    match matcher {
        Matcher::Text { text, exact } => {
            json!({ "matcher": name, "pattern": pattern(text, *exact) })
        }
        Matcher::Value { value } => json!({ "matcher": name, "pattern": pattern(value, true) }),
        Matcher::Values { values } => json!({ "matcher": name, "values": values }),
        Matcher::Attr { name: attr, value } => json!({
            "matcher": name,
            "name": attr,
            "pattern": value.as_deref().map(|v| pattern(v, true)),
        }),
        Matcher::Class { name: class } => match as_regex(class) {
            Some(_) => json!({ "matcher": name, "pattern": pattern(class, true) }),
            None => json!({ "matcher": name, "name": class }),
        },
        Matcher::Css { property, value } => json!({
            "matcher": name,
            "name": property.trim(),
            "pattern": pattern(value, true),
        }),
        Matcher::Count { count, min, max } => json!({
            "matcher": name,
            "count": { "eq": count, "min": min, "max": max },
        }),
        _ => json!({ "matcher": name }),
    }
}

/// Whether `matcher` holds while its frame or ref element is gone (no
/// element); `None` when it can hold neither way.
fn absent(matcher: &Matcher) -> Option<bool> {
    match matcher {
        Matcher::Visible | Matcher::Attached => Some(false),
        Matcher::Hidden | Matcher::Detached => Some(true),
        Matcher::Count { count, min, .. } => {
            Some(count.is_none_or(|n| n == 0) && min.is_none_or(|n| n == 0))
        }
        _ => None,
    }
}

/// What the assertion expects, in words: `text containing "Saved"`.
fn expected(req: &ExpectRequest, level_rules: Option<(usize, A11yLevel)>) -> String {
    let not = if req.not { "not " } else { "" };
    let text = match &req.expectation {
        Expectation::Element { matcher, .. } => match matcher {
            Matcher::Attached => "attached (at least one match)".into(),
            Matcher::Detached => "detached (no match)".into(),
            Matcher::InViewport => "in the viewport".into(),
            Matcher::Text { text, exact } => format!("text {}", shown_pattern(text, *exact)),
            Matcher::Value { value } => format!("value {}", shown_pattern(value, true)),
            Matcher::Values { values } => {
                let listed: Vec<String> = values.iter().map(|v| format!("{v:?}")).collect();
                format!("values {} (in any order)", listed.join(", "))
            }
            Matcher::Attr { name, value: None } => format!("attribute {name}"),
            Matcher::Attr {
                name,
                value: Some(value),
            } => format!("attribute {name} {}", shown_pattern(value, true)),
            Matcher::Class { name } if as_regex(name).is_some() => format!("class matching {name}"),
            Matcher::Class { name } => format!("class {name:?}"),
            Matcher::Css { property, value } => {
                format!("css {} {}", property.trim(), shown_pattern(value, true))
            }
            Matcher::Count { count, min, max } => {
                let n = |n: u64| {
                    if n == 1 {
                        "1 element".to_owned()
                    } else {
                        format!("{n} elements")
                    }
                };
                match (count, min, max) {
                    (Some(c), None, None) => n(*c),
                    (None, Some(lo), Some(hi)) => format!("{lo} to {}", n(*hi)),
                    (None, Some(lo), None) => format!("at least {}", n(*lo)),
                    (None, None, Some(hi)) => format!("at most {}", n(*hi)),
                    _ => {
                        let parts = [
                            count.map(n),
                            min.map(|m| format!("at least {m}")),
                            max.map(|m| format!("at most {m}")),
                        ];
                        parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
                    }
                }
            }
            other => matcher_name(other).to_owned(),
        },
        Expectation::Url { pattern } => format!("a URL matching {pattern:?}"),
        Expectation::Title { text, exact } => format!("title {}", shown_pattern(text, *exact)),
        Expectation::NoErrors { .. } => "no uncaught exceptions or console errors".into(),
        Expectation::NoFailedRequests { origin, .. } => match origin.as_deref() {
            Some("self") => "no failed requests to the page's origin".into(),
            Some(origin) => format!("no failed requests to {origin}"),
            None => "no failed requests".into(),
        },
        Expectation::A11y { .. } => match level_rules {
            Some((rules, level)) => format!(
                "no violations of {rules} accessibility rules (WCAG level {})",
                level.as_str()
            ),
            None => "no accessibility violations".into(),
        },
        Expectation::MockState { name, check } => match check {
            MockStateCheck::Count { count, min, max } => {
                let n = |n: u64| {
                    if n == 1 {
                        "1 item".to_owned()
                    } else {
                        format!("{n} items")
                    }
                };
                let parts = [
                    count.map(n),
                    min.map(|m| format!("at least {}", n(m))),
                    max.map(|m| format!("at most {}", n(m))),
                ];
                format!(
                    "{name} with {}",
                    parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
                )
            }
            MockStateCheck::Value { value } => {
                format!("{name} = {}", mock::text::compact(value))
            }
            MockStateCheck::Contains { value } => {
                format!("{name} containing {}", mock::text::compact(value))
            }
        },
        Expectation::Request(check) => requests::expected_requests(check),
    };
    format!("{not}{text}")
}

/// The `expect` command line of `req` (for messages and hints).
fn command(req: &ExpectRequest) -> Cmd {
    let mut cmd = Cmd::new("expect");
    match &req.expectation {
        Expectation::Element { locator, matcher } => {
            cmd = cmd.arg(locator).raw(matcher_name(matcher));
            cmd = match matcher {
                Matcher::Text { text, exact } => {
                    let cmd = cmd.arg(text);
                    if *exact { cmd.switch("exact") } else { cmd }
                }
                Matcher::Value { value } => cmd.arg(value),
                Matcher::Values { values } => values.iter().fold(cmd, Cmd::arg),
                Matcher::Attr { name, value } => {
                    let cmd = cmd.arg(name);
                    match value {
                        Some(value) => cmd.arg(value),
                        None => cmd,
                    }
                }
                Matcher::Class { name } => cmd.arg(name),
                Matcher::Css { property, value } => cmd.arg(property).arg(value),
                Matcher::Count { count, min, max } => {
                    let mut cmd = cmd;
                    if let Some(count) = count {
                        cmd = cmd.arg(count.to_string());
                    }
                    if let Some(min) = min {
                        cmd = cmd.flag("min", min.to_string());
                    }
                    if let Some(max) = max {
                        cmd = cmd.flag("max", max.to_string());
                    }
                    cmd
                }
                _ => cmd,
            };
        }
        Expectation::Url { pattern } => cmd = cmd.flag("url", pattern),
        Expectation::Title { text, exact } => {
            cmd = cmd.flag("title", text);
            if *exact {
                cmd = cmd.switch("exact");
            }
        }
        Expectation::NoErrors { since, all_tabs } => {
            cmd = cmd.switch("no-errors");
            if *all_tabs {
                cmd = cmd.switch("all-tabs");
            }
            if let Some(since) = since {
                cmd = cmd.flag("since", since);
            }
        }
        Expectation::NoFailedRequests { since, origin } => {
            cmd = cmd.switch("no-failed-requests");
            if let Some(origin) = origin {
                cmd = cmd.flag("origin", origin);
            }
            if let Some(since) = since {
                cmd = cmd.flag("since", since);
            }
        }
        Expectation::A11y { scope, level } => {
            if let Some(scope) = scope {
                cmd = cmd.arg(scope);
            }
            cmd = cmd.switch("a11y");
            if *level != A11yLevel::default() {
                cmd = cmd.flag("level", level.as_str());
            }
        }
        Expectation::MockState { name, check } => {
            cmd = cmd.arg("mock-state").arg(name);
            cmd = match check {
                MockStateCheck::Count { count, min, max } => {
                    let mut cmd = cmd.raw("count");
                    if let Some(count) = count {
                        cmd = cmd.arg(count.to_string());
                    }
                    if let Some(min) = min {
                        cmd = cmd.flag("min", min.to_string());
                    }
                    if let Some(max) = max {
                        cmd = cmd.flag("max", max.to_string());
                    }
                    cmd
                }
                MockStateCheck::Value { value } => cmd.raw("value").arg(state_arg(value)),
                MockStateCheck::Contains { value } => cmd.raw("contains").arg(state_arg(value)),
            };
        }
        Expectation::Request(check) => cmd = requests::request_command(cmd, check),
    }
    if req.not {
        cmd = cmd.switch("not");
    }
    if let Some(tab) = &req.tab {
        cmd = cmd.flag("tab", tab);
    }
    cmd
}

/// The assertion as typed after `chrome-cdp expect`.
/// A value as `expect mock-state` takes it: text as it is, anything else
/// as JSON.
fn state_arg(value: &Value) -> String {
    match value {
        Value::String(s) if serde_json::from_str::<Value>(s).is_err() => s.clone(),
        other => other.to_string(),
    }
}

fn assertion_text(req: &ExpectRequest) -> String {
    let full = command(req).to_string();
    full.strip_prefix(&format!("{} expect ", protocol::PROGRAM))
        .unwrap_or(&full)
        .to_owned()
}

/// An element a failure names, as the bundle describes it, with a ref.
#[derive(Debug, Clone, Default, Deserialize)]
struct Named {
    role: String,
    name: String,
    tag: String,
    #[serde(skip)]
    reference: Option<String>,
}

impl Named {
    /// `alert "Error: email is required" [e31]`.
    fn line(&self) -> String {
        let mut text = if self.role == "generic" || self.role.is_empty() {
            self.tag.clone()
        } else {
            self.role.clone()
        };
        if !self.name.is_empty() {
            let _ = write!(text, " \"{}\"", self.name.replace('"', "\\\""));
        }
        if let Some(reference) = &self.reference {
            let _ = write!(text, " [{reference}]");
        }
        text
    }

    fn json(&self) -> Value {
        json!({ "ref": self.reference, "role": self.role, "name": self.name, "tag": self.tag })
    }
}

/// What a failed assertion reports.
#[derive(Debug, Default)]
struct Failed {
    expected: String,
    /// What was seen, in words.
    actual: String,
    /// Labelled lines after `actual`: the elements, the errors found.
    notes: Vec<(String, String)>,
    /// `details`, an object.
    details: Map<String, Value>,
    /// The hint, unless there are unread console errors.
    hint: Option<Hint>,
}

/// What a passed assertion reports.
struct Passed {
    actual: Value,
    /// `actual` for `-q`.
    quiet: String,
    count: Option<u64>,
    retries: u64,
}

impl Engine {
    /// `mark <name>`: names the current end of the tab's console and
    /// network logs, for `expect --no-errors --since <name>`. Not a page
    /// command: it needs no script and moves no "previous command" mark.
    pub async fn mark(&self, req: MarkRequest) -> Result<Output, ErrorInfo> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "the mark's name is empty")
                    .hint(Hint::run(Cmd::new("mark").arg("before-save"))),
            );
        }
        let running = self.running()?;
        let id = req
            .tab
            .as_deref()
            .map(crate::tabs::parse_tab_id)
            .transpose()?;
        // The context's tab with --context, as other commands take it.
        let timeout = millis(req.timeout_ms);
        let page = match self.current_context()? {
            Some(scope) => running.tabs.page_in(&scope.browser_id, id, timeout).await,
            None => running.tabs.page(id, timeout).await,
        }
        .map_err(|e| self.tab_error(&running, e))?;
        let replaced = running.tabs.set_mark(page.id, name).is_some();
        let (_, console) = running.tabs.log_marks(page.id);
        let tab = tab_name(page.id);
        let text = format!(
            "{} mark {name:?} in {tab}; check what happened after it: {}",
            if replaced { "moved" } else { "set" },
            Cmd::new("expect")
                .switch("no-errors")
                .flag("since", name)
                .scoped(&self.session)
        );
        let data = MarkData {
            tab,
            name: name.to_owned(),
            console,
            replaced,
        };
        Ok(Output::typed(&data, text).quiet(name.to_owned()))
    }

    /// `expect`: one assertion about the active tab (or `--tab`),
    /// retried until it holds or the timeout passes.
    pub async fn expect(&self, req: ExpectRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let assertion = Assertion::parse(&req, &options)?;
        if let Expectation::MockState { name, check } = &req.expectation {
            return self.expect_mock_state(&req, name, check).await;
        }
        if let Expectation::Request(check) = &req.expectation {
            return self.expect_request(&req, check).await;
        }
        if let Expectation::NoErrors { all_tabs: true, .. } = &req.expectation {
            return self.no_errors_in_all_tabs(&req);
        }
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        if matches!(assertion, Assertion::Polled { .. } | Assertion::A11y { .. }) {
            self.check_not_blocked(&running, &page)?;
        }
        let mark = running.tabs.mark();
        let start = Instant::now();
        let passed = {
            let _tab = self.tabs.lock(&page.target_id).await;
            let check = self.assert(&running, &page, &assertion, &req, &options, timeout);
            self.unless_blocked(&running, &page, mark, check).await?
        };
        let elapsed = start.elapsed();
        let assertion_text = assertion_text(&req);
        let data = ExpectData {
            tab: tab_name(page.id),
            assertion: assertion_text.clone(),
            expected: expected(&req, None),
            actual: passed.actual,
            count: passed.count,
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            retries: passed.retries,
        };
        let text = format!("passed: {assertion_text} ({})", format::duration(elapsed));
        let output = Output::typed(&data, text)
            .quiet(passed.quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it");
        Ok(self.report_events(&running, mark, output, None).await)
    }

    async fn assert(
        &self,
        running: &Running,
        page: &Page,
        assertion: &Assertion,
        req: &ExpectRequest,
        options: &LocateOptions,
        timeout: Duration,
    ) -> Result<Passed, ErrorInfo> {
        match assertion {
            Assertion::Polled { condition, matcher } => {
                let deadline = Instant::now() + timeout;
                let outcome = self
                    .probe_until(running, page, condition, options, deadline)
                    .await?;
                match outcome {
                    Ok(met) => {
                        let observed = met.value.unwrap_or(Value::Null);
                        Ok(Passed {
                            actual: shown_value(&observed),
                            quiet: quiet_value(&observed),
                            count: met.count.filter(|_| matcher.is_some()),
                            retries: retries(&observed),
                        })
                    }
                    Err(unmet) => {
                        let observed = Value::Object(unmet.observed);
                        Err(self
                            .polled_failure(
                                running,
                                page,
                                condition,
                                matcher.as_ref(),
                                &observed,
                                req,
                                options,
                                timeout,
                            )
                            .await)
                    }
                }
            }
            Assertion::Url(pattern) => {
                let deadline = Instant::now() + timeout;
                let not = req.not;
                match running
                    .tabs
                    .main_url(page.id, |url| pattern.matches(url) != not, deadline)
                    .await
                {
                    Ok(url) => Ok(Passed {
                        actual: json!(url),
                        quiet: url,
                        count: None,
                        retries: 0,
                    }),
                    Err(url) => {
                        let actual = format!("{:?} (after {})", url, format::duration(timeout));
                        let mut failed = Failed {
                            expected: expected(req, None),
                            actual,
                            ..Failed::default()
                        };
                        failed.details.insert("actual".into(), json!(url));
                        Err(self.failure(page, req, failed, Some(timeout)))
                    }
                }
            }
            Assertion::NoErrors { since } => self.no_errors(running, page, req, since.as_deref()),
            Assertion::NoFailedRequests { since, origin } => {
                self.no_failed_requests(running, page, req, since.as_deref(), origin.as_deref())
            }
            Assertion::A11y { scope, level } => {
                self.a11y(running, page, req, scope.as_ref(), *level, options, timeout)
                    .await
            }
            Assertion::MockState | Assertion::Request => {
                unreachable!("expect answers mock-state and request before any page")
            }
        }
    }

    /// The failure of an element matcher or `--title`: what it saw last,
    /// and the elements it matched, with refs.
    #[allow(clippy::too_many_arguments)]
    async fn polled_failure(
        &self,
        running: &Running,
        page: &Page,
        condition: &Condition,
        matcher: Option<&Matcher>,
        observed: &Value,
        req: &ExpectRequest,
        options: &LocateOptions,
        timeout: Duration,
    ) -> ErrorInfo {
        let locator = match condition {
            Condition::Expect { locator, .. } => locator.as_ref(),
            _ => None,
        };
        let count = observed["count"].as_u64().unwrap_or(0);
        let named = match locator {
            Some(locator) if count > 0 => {
                self.named(running, page, locator, options, Duration::from_secs(2))
                    .await
            }
            _ => Vec::new(),
        };
        let mut actual = actual_text(matcher, observed);
        let _ = write!(actual, " (after {}", format::duration(timeout));
        if let Some(tries) = observed["tries"].as_u64() {
            let retries = tries.saturating_sub(1);
            let _ = write!(
                actual,
                ", {retries} {}",
                if retries == 1 { "retry" } else { "retries" }
            );
        }
        actual.push(')');
        let mut failed = Failed {
            expected: expected(req, None),
            actual,
            hint: matcher.map(|m| self.element_hint(req, m, observed, &named, locator)),
            ..Failed::default()
        };
        if !named.is_empty() {
            let lines: Vec<String> = named.iter().map(Named::line).collect();
            let label = if named.len() == 1 {
                "element"
            } else {
                "elements"
            };
            failed.notes.push((label.into(), lines.join(", ")));
        }
        failed.details.extend([
            ("actual".into(), shown_value(observed)),
            (
                "count".into(),
                observed.get("count").cloned().unwrap_or(Value::Null),
            ),
            ("retries".into(), json!(retries(observed))),
            ("observed".into(), observed.clone()),
            (
                "elements".into(),
                json!(named.iter().map(Named::json).collect::<Vec<_>>()),
            ),
        ]);
        self.failure(page, req, failed, Some(timeout))
    }

    /// `ExpectFailed`: expected, actual, what else was found, and a
    /// hint. After a `timeout` (a retried assertion), unread console
    /// errors come first in the hint: the page may have failed.
    fn failure(
        &self,
        page: &Page,
        req: &ExpectRequest,
        failed: Failed,
        timeout: Option<Duration>,
    ) -> ErrorInfo {
        let mut info = ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!("expect failed: {}", assertion_text(req)),
        )
        .note("expected", &failed.expected)
        .note("actual", failed.actual);
        for (label, text) in failed.notes {
            info = info.note(label, text);
        }
        let mut details = failed.details;
        details.insert("assertion".into(), json!(assertion_text(req)));
        details.insert("expected".into(), json!(failed.expected));
        if let Some(timeout) = timeout {
            details.insert(
                "timeout_ms".into(),
                json!(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
            );
        }
        let unread = timeout.and_then(|_| self.unread_errors(page.id));
        let hint = match unread {
            Some((unread, cmd)) => {
                details.insert("console_errors".into(), json!(unread));
                Hint::run(cmd).why(format!(
                    "{unread} console {} not shown yet; the page may have failed",
                    if unread == 1 { "error" } else { "errors" }
                ))
            }
            None => failed
                .hint
                .unwrap_or_else(|| self.retry_hint(req, timeout.unwrap_or(millis(req.timeout_ms)))),
        };
        info.hint(hint).details(Value::Object(details))
    }

    /// Try again with a longer timeout, or look at the page.
    fn retry_hint(&self, req: &ExpectRequest, timeout: Duration) -> Hint {
        let longer = format::duration(timeout * 2);
        Hint::run(command(req).flag("timeout", &longer).scoped(&self.session))
            .why("if the page needs longer to get there")
            .or(format!(
                "{} shows the page as it is",
                Cmd::new("snapshot").scoped(&self.session)
            ))
    }

    /// The hint of a failed element matcher: the command that shows what
    /// it compared, the first match, or where to find the element.
    fn element_hint(
        &self,
        req: &ExpectRequest,
        matcher: &Matcher,
        observed: &Value,
        named: &[Named],
        locator: Option<&Locator>,
    ) -> Hint {
        let first = named
            .first()
            .and_then(|n| n.reference.clone())
            .or_else(|| locator.map(ToString::to_string))
            .unwrap_or_default();
        let count = observed["count"].as_u64().unwrap_or(0);
        let invalid = observed["invalid"].as_str().unwrap_or_default();
        if count > 1 && !invalid.is_empty() {
            if let (Some(reference), Expectation::Element { .. }) = (
                named.first().and_then(|n| n.reference.as_ref()),
                &req.expectation,
            ) {
                let mut again = req.clone();
                if let Expectation::Element { locator, .. } = &mut again.expectation {
                    reference.clone_into(locator);
                }
                return Hint::run(command(&again).scoped(&self.session)).why(format!(
                    "the first of {count} matches; or add >> nth=<index> to the locator"
                ));
            }
            return Hint::text(format!(
                "{count} elements match; add >> nth=<index> to the locator"
            ));
        }
        if observed["ref"] == json!("gone") {
            let what = locator.map(ToString::to_string).unwrap_or_default();
            return Hint::run(Cmd::new("snapshot").scoped(&self.session))
                .why(format!("{what}'s element is gone; find it again there"));
        }
        if count == 0
            && !matches!(
                matcher,
                Matcher::Detached | Matcher::Hidden | Matcher::Count { .. }
            )
        {
            let what = locator.map(ToString::to_string).unwrap_or_default();
            return Hint::run(Cmd::new("snapshot").scoped(&self.session))
                .why(format!("no element matches {what}; find it there"));
        }
        let timeout = millis(req.timeout_ms);
        let read = match matcher {
            Matcher::Text { .. } => Some(Cmd::new("text").arg(&first)),
            Matcher::Value { .. } | Matcher::Values { .. } => Some(Cmd::new("value").arg(&first)),
            Matcher::Attr { name, .. } => Some(Cmd::new("attr").arg(&first).arg(name)),
            Matcher::Class { .. } => Some(Cmd::new("attr").arg(&first).arg("class")),
            _ => None,
        };
        match read {
            Some(cmd) if !invalid.contains("not a") => Hint::run(cmd.scoped(&self.session))
                .why("shows all of it")
                .or(format!(
                    "--timeout {} to wait longer",
                    format::duration(timeout * 2)
                )),
            _ => self.retry_hint(req, timeout),
        }
    }

    /// Up to [`NAMED`] of the elements `locator` matches, described, with
    /// refs (none if a frame loaded a new document meanwhile). Best
    /// effort: empty when they cannot be read.
    async fn named(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        options: &LocateOptions,
        timeout: Duration,
    ) -> Vec<Named> {
        let Ok(place) = self
            .place(running, page, locator, options, FrameOnly::Owner, timeout)
            .await
        else {
            return Vec::new();
        };
        let generation = running
            .tabs
            .snapshot_memory(page.id, |m| m.refs.generation());
        let session = place.context.session.with_timeout(timeout);
        let mut params = runtime::CallFunctionOnParams::new(format!(
            "function (steps, root) {{ {GUARD} \
             return globalThis.__ccdp.expectNamed(globalThis.__ccdp.query(steps, root)); }}"
        ));
        params.arguments = Some(vec![
            runtime::CallArgument {
                value: serde_json::to_value(locator.in_frame_steps()).ok(),
                unserializable_value: None,
                object_id: None,
            },
            place.root_argument(),
        ]);
        params.object_group = Some(GROUP.to_owned());
        let result = injected::call(&session, place.context.context_id, params).await;
        let listed = match result {
            Ok(result) if result.exception_details.is_none() => match result.result.object_id {
                Some(object_id) => read_listed(&session, &object_id, &place.frame, NAMED)
                    .await
                    .ok(),
                None => None,
            },
            _ => None,
        };
        let _ = session
            .call(runtime::ReleaseObjectGroupParams::new(GROUP.to_owned()))
            .await;
        place.release();
        let Some((json, keys)) = listed else {
            return Vec::new();
        };
        let mut named: Vec<Named> = serde_json::from_str(&json).unwrap_or_default();
        running.tabs.snapshot_memory(page.id, |m| {
            if Some(m.refs.generation()) != generation {
                return;
            }
            for (n, key) in named.iter_mut().zip(&keys) {
                if let Some(key) = key {
                    let number = m.refs.get(key).unwrap_or_else(|| {
                        m.refs
                            .assign_from(key, &n.role, &n.name, SnapshotEngine::Dom)
                    });
                    n.reference = Some(format!("e{number}"));
                }
            }
        });
        named
    }
}

/// How many times a probe tried again.
fn retries(observed: &Value) -> u64 {
    observed["tries"].as_u64().unwrap_or(1).saturating_sub(1)
}

/// `data.actual`: the value compared, else the state in words.
fn shown_value(observed: &Value) -> Value {
    match observed.get("actual") {
        Some(actual) if !actual.is_null() => actual.clone(),
        _ => observed
            .get("state")
            .filter(|s| s.as_str().is_some_and(|s| !s.is_empty()))
            .cloned()
            .unwrap_or(Value::Null),
    }
}

/// What `-q` prints of a passed assertion: the value compared.
fn quiet_value(observed: &Value) -> String {
    match shown_value(observed) {
        Value::String(s) => s,
        Value::Null => String::new(),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// What a failed polled assertion saw last, in words.
fn actual_text(matcher: Option<&Matcher>, observed: &Value) -> String {
    if let Some(invalid) = observed["invalid"].as_str() {
        return invalid.to_owned();
    }
    if observed["ref"] == json!("gone") {
        return "its element is gone from the page".into();
    }
    if observed["frame"] == json!("not found") {
        return "the frame is not in the page".into();
    }
    if observed["document"].is_string() {
        return "the page has no document to look in yet".into();
    }
    if let Some(page) = observed["page"].as_str() {
        return format!("the page is {page}");
    }
    if let Some(error) = observed["error"].as_str() {
        return format!("the check failed: {error}");
    }
    let count = observed["count"].as_u64().unwrap_or(0);
    let elements = |n: u64| match n {
        0 => "no element matches".to_owned(),
        1 => "1 element matches".to_owned(),
        n => format!("{n} elements match"),
    };
    let quoted = |v: &Value| match v {
        Value::String(s) => format!("{:?}", cut(s)),
        Value::Null => "none".to_owned(),
        other => other.to_string(),
    };
    match matcher {
        None => format!("title {}", quoted(&observed["actual"])),
        Some(Matcher::Attached | Matcher::Detached | Matcher::Count { .. }) => elements(count),
        Some(Matcher::Text { .. }) => format!("text {}", quoted(&observed["actual"])),
        Some(Matcher::Value { .. }) => format!("value {}", quoted(&observed["actual"])),
        Some(Matcher::Values { .. }) => match observed["actual"].as_array() {
            Some(values) if values.is_empty() => "no values".into(),
            Some(values) => format!(
                "values {}",
                values.iter().map(quoted).collect::<Vec<_>>().join(", ")
            ),
            None => "no values".into(),
        },
        Some(Matcher::Attr { name, .. }) if observed["actual"].is_null() => {
            format!("no attribute {name}")
        }
        Some(Matcher::Attr { name, .. }) => format!("{name}={}", quoted(&observed["actual"])),
        Some(Matcher::Class { .. }) => format!("class {}", quoted(&observed["actual"])),
        Some(Matcher::Css { property, .. }) => {
            format!("{} {}", property.trim(), quoted(&observed["actual"]))
        }
        Some(_) => observed["state"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map_or_else(|| elements(count), str::to_owned),
    }
}

#[cfg(test)]
mod tests;

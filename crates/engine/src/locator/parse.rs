//! The locator grammar (plan 04, milestone 4.4): one syntax for every
//! command that names an element.
//!
//! A locator is a chain of steps joined by `>>` (outside quotes and
//! brackets): `A >> B` finds B inside A. A step is a ref (`e12`, first
//! only), `frame=…` (leading only), a finder (`css=`, `xpath=`, `text=`,
//! `role=`, `label=`, `placeholder=`, `alt=`, `title=`, `testid=`) or a
//! filter of what the steps before it found (`nth=`, `visible=`,
//! `has-text=`, `has-not-text=`, and, taking a whole inner locator,
//! `has=`, `has-not=`, `and=`, `or=`). Bare forms: `#id`, `.cls` and
//! `[attr]` are CSS, `//…` is XPath, and a quoted string is exact text.
//!
//! An inner locator (plan 14, milestone 14.1) runs to the next `>>`, or is
//! quoted when it holds one: `.row >> has="role=button >> visible=true"`;
//! inside the quotes, `\"` and `\\` escape. It cannot start with a ref
//! or `frame=`, and nests at most [`MAX_DEPTH`] deep. `has=` looks inside
//! each element; `and=` and `or=` look in the scopes the last finder step
//! searched (the chain's root if none did).
//!
//! Text values (`text=`, `name=`, `label=`, …) match a case-insensitive
//! substring, the whole text when quoted (`"Sign in"`), or a regex
//! (`/sign\s*in/i`), whitespace collapsed in all three.

use std::fmt;

use protocol::{ErrorInfo, ErrorKind, Hint};
use serde::Serialize;

use crate::snapshot::parse_ref;

/// The attribute `testid=` reads unless configured otherwise.
pub const DEFAULT_TESTID_ATTRIBUTE: &str = "data-testid";

/// A parsed locator: its steps, and the text it was parsed from (what
/// messages show).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locator {
    pub(crate) steps: Vec<Step>,
    source: String,
}

/// How a text value matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TextMode {
    /// A case-insensitive substring.
    Substring,
    /// The whole text, case-sensitive (quoted).
    Exact,
    /// A regex (`/…/flags`).
    Regex,
}

/// A text value to match; whitespace is collapsed on both sides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TextMatch {
    pub(crate) mode: TextMode,
    pub(crate) value: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) flags: String,
}

/// `checked=` and `pressed=` also take `mixed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tri {
    True,
    False,
    Mixed,
}

/// `role=<role>[attr…]`: an ARIA role, its accessible name and states.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleQuery {
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) checked: Option<Tri>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expanded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pressed: Option<Tri>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) selected: Option<bool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) include_hidden: bool,
}

/// One step of a locator. The engine resolves `Ref` and `Frame` itself;
/// the rest go to the injected bundle as JSON (`Step` in
/// `injected/src/locator.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Step {
    Ref { number: u32 },
    Frame { query: String },
    Css { value: String },
    Xpath { value: String },
    Text { text: TextMatch },
    Role(RoleQuery),
    Label { text: TextMatch },
    Placeholder { text: TextMatch },
    Alt { text: TextMatch },
    Title { text: TextMatch },
    Testid { value: String, attribute: String },
    Nth { index: i64 },
    Visible { visible: bool },
    HasText { text: TextMatch },
    HasNotText { text: TextMatch },
    Has(Inner),
    HasNot(Inner),
    And(Inner),
    Or(Inner),
}

/// The inner locator of `has=`, `has-not=`, `and=` and `or=`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Inner {
    pub(crate) steps: Vec<Step>,
    /// As typed, without its quotes (what messages show).
    #[serde(skip)]
    pub(crate) source: String,
}

impl Step {
    /// Whether the step narrows down (or, for `or=`, adds to) what came
    /// before instead of finding elements inside it.
    pub(crate) fn is_filter(&self) -> bool {
        matches!(
            self,
            Self::Nth { .. }
                | Self::Visible { .. }
                | Self::HasText { .. }
                | Self::HasNotText { .. }
                | Self::Has(_)
                | Self::HasNot(_)
                | Self::And(_)
                | Self::Or(_)
        )
    }

    /// The inner locator's, if it has one.
    pub(crate) fn inner(&self) -> Option<&Inner> {
        match self {
            Self::Has(i) | Self::HasNot(i) | Self::And(i) | Self::Or(i) => Some(i),
            _ => None,
        }
    }

    fn inner_mut(&mut self) -> Option<&mut Inner> {
        match self {
            Self::Has(i) | Self::HasNot(i) | Self::And(i) | Self::Or(i) => Some(i),
            _ => None,
        }
    }
}

impl fmt::Display for Locator {
    /// As the user typed it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Serialize for Locator {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.source)
    }
}

impl Locator {
    /// The steps the injected bundle evaluates in a frame: all but the
    /// leading ref and `frame=` steps.
    pub(crate) fn in_frame_steps(&self) -> &[Step] {
        let head = self
            .steps
            .iter()
            .take_while(|s| matches!(s, Step::Ref { .. } | Step::Frame { .. }))
            .count();
        &self.steps[head..]
    }

    /// Whether it starts from a ref (`e12`): session-only, so never in a
    /// committed scenario.
    pub fn has_ref(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, Step::Ref { .. }))
    }

    /// Reads `testid=` from `attribute` instead of `data-testid`.
    #[must_use]
    pub fn with_testid_attribute(mut self, attribute: Option<&str>) -> Self {
        if let Some(attribute) = attribute.map(str::trim).filter(|a| !a.is_empty()) {
            set_testid_attribute(&mut self.steps, attribute);
        }
        self
    }

    /// The locator without its last step, as typed (`.row` of
    /// `.row >> has=…`), if it has more than one.
    pub(crate) fn without_last(&self) -> Option<String> {
        let parts = self.typed_steps();
        (parts.len() > 1).then(|| {
            let kept: Vec<&str> = parts[..parts.len() - 1]
                .iter()
                .map(|(p, _)| p.as_str())
                .collect();
            kept.join(" >> ")
        })
    }

    /// Its steps as typed, each with whether it filters (or, `or=`, adds
    /// to) what the steps before it found rather than finding elements
    /// inside them.
    pub fn typed_steps(&self) -> Vec<(String, bool)> {
        split_chain(&self.source)
            .unwrap_or_default()
            .into_iter()
            .zip(&self.steps)
            .map(|(part, step)| (part, step.is_filter()))
            .collect()
    }
}

fn set_testid_attribute(steps: &mut [Step], attribute: &str) {
    for step in steps {
        if let Step::Testid { attribute: a, .. } = step {
            attribute.clone_into(a);
        } else if let Some(inner) = step.inner_mut() {
            set_testid_attribute(&mut inner.steps, attribute);
        }
    }
}

/// The kinds a step may name before `=`.
const KINDS: &str = "css, xpath, text, role, label, placeholder, alt, title, testid, frame, nth, \
                     visible, has-text, has-not-text, has, has-not, and, or";

/// How deep inner locators (`has=`, `and=`, …) may nest.
pub(crate) const MAX_DEPTH: usize = 4;

/// What every "not a locator" error suggests.
const FORMS: &str =
    "e.g. e12, #id, css=<selector>, text=Sign in, role=button[name=\"Save\"], label=Email";

/// Parses a locator as typed.
pub fn parse_locator(text: &str) -> Result<Locator, ErrorInfo> {
    parse_at(text, 0)
}

/// Parses a locator `depth` inner locators deep.
fn parse_at(text: &str, depth: usize) -> Result<Locator, ErrorInfo> {
    let source = text.trim();
    if source.is_empty() {
        return Err(bad("the locator is empty", FORMS));
    }
    let parts = split_chain(source).map_err(|why| bad(format!("`{source}`: {why}"), FORMS))?;
    let mut steps = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return Err(bad(
                format!("`{source}` has an empty step"),
                "join steps with >>, e.g. #list >> text=Delete",
            ));
        }
        let step = parse_step(part, i, depth)?;
        match (&step, steps.last()) {
            (Step::Ref { .. }, Some(_)) => {
                return Err(bad(
                    format!("`{source}`: a ref ({part}) can only come first"),
                    format!("start with it: {part} >> …"),
                ));
            }
            (Step::Frame { .. }, Some(prev)) if !matches!(prev, Step::Frame { .. }) => {
                return Err(bad(
                    format!("`{source}`: frame= can only come first"),
                    "name the frame, then what is in it: frame=payment >> #card",
                ));
            }
            (s, None) if s.is_filter() => {
                return Err(bad(
                    format!("`{source}`: {part} filters what comes before it, but nothing does"),
                    format!("e.g. .item >> {part}"),
                ));
            }
            _ => {}
        }
        steps.push(step);
    }
    Ok(Locator {
        steps,
        source: source.to_owned(),
    })
}

fn bad(message: impl Into<String>, hint: impl Into<String>) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadLocator, message).hint(Hint::text(hint))
}

/// Splits at `>>` outside quotes, brackets and parentheses. A quote
/// opens only where a value starts (a step's start, after `=`, `[`, `(`
/// or `,`), so an apostrophe in plain text (`text=Don't save`) is text.
fn split_chain(text: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut depth = 0i32;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let starts_value = current
            .trim_end()
            .chars()
            .last()
            .is_none_or(|p| matches!(p, '=' | '[' | '(' | ','));
        if let Some(q) = quote {
            current.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' if starts_value => quote = Some(c),
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            '>' if depth <= 0 && chars.peek() == Some(&'>') => {
                chars.next();
                parts.push(current.trim().to_owned());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if let Some(q) = quote {
        return Err(format!("a {q} quote is not closed"));
    }
    parts.push(current.trim().to_owned());
    Ok(parts)
}

fn parse_step(part: &str, index: usize, depth: usize) -> Result<Step, ErrorInfo> {
    if let Some(number) = parse_ref(part) {
        return Ok(Step::Ref { number });
    }
    if part.starts_with(['#', '.', '[']) {
        return Ok(Step::Css {
            value: part.to_owned(),
        });
    }
    if part.starts_with("//") || part.starts_with("(//") {
        return Ok(Step::Xpath {
            value: part.to_owned(),
        });
    }
    if part.starts_with(['"', '\'']) {
        return text_value(part, "text=").map(|text| Step::Text { text });
    }
    let Some((kind, value)) = part
        .split_once('=')
        .filter(|(k, _)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
    else {
        return Err(not_a_locator(part, index));
    };
    let value = value.trim();
    let needs_value = |what: &str| {
        if value.is_empty() {
            Err(bad(
                format!("`{part}` has no {what}"),
                format!("e.g. {}", example(kind)),
            ))
        } else {
            Ok(value.to_owned())
        }
    };
    Ok(match kind {
        "css" => Step::Css {
            value: needs_value("selector")?,
        },
        "xpath" => Step::Xpath {
            value: needs_value("expression")?,
        },
        "text" => Step::Text {
            text: text_value(value, part)?,
        },
        "has-text" => Step::HasText {
            text: text_value(value, part)?,
        },
        "has-not-text" => Step::HasNotText {
            text: text_value(value, part)?,
        },
        "has" => Step::Has(inner(value, part, depth)?),
        "has-not" => Step::HasNot(inner(value, part, depth)?),
        "and" => Step::And(inner(value, part, depth)?),
        "or" => Step::Or(inner(value, part, depth)?),
        "label" => Step::Label {
            text: text_value(value, part)?,
        },
        "placeholder" => Step::Placeholder {
            text: text_value(value, part)?,
        },
        "alt" => Step::Alt {
            text: text_value(value, part)?,
        },
        "title" => Step::Title {
            text: text_value(value, part)?,
        },
        "testid" => Step::Testid {
            value: unquote(&needs_value("test id")?),
            attribute: DEFAULT_TESTID_ATTRIBUTE.to_owned(),
        },
        "frame" => Step::Frame {
            query: unquote(&needs_value("frame")?),
        },
        "role" => Step::Role(parse_role(value, part)?),
        "nth" => Step::Nth {
            index: value.parse().map_err(|_| {
                bad(
                    format!("`{part}`: nth= takes a number"),
                    "nth=0 is the first match, nth=-1 the last",
                )
            })?,
        },
        "visible" => Step::Visible {
            visible: parse_bool(value).ok_or_else(|| {
                bad(
                    format!("`{part}`: visible= takes true or false"),
                    "e.g. .item >> visible=true",
                )
            })?,
        },
        _ => {
            return Err(bad(
                format!("`{part}`: unknown locator kind `{kind}=`"),
                format!("use one of {KINDS}"),
            ));
        }
    })
}

/// The inner locator of `part` (`has=<value>`), `depth` deep.
fn inner(value: &str, part: &str, depth: usize) -> Result<Inner, ErrorInfo> {
    let value = value.trim();
    if value.is_empty() {
        return Err(bad(
            format!("`{part}` has no locator"),
            "e.g. .row >> has=\"role=button[name=Delete]\"",
        ));
    }
    if depth + 1 > MAX_DEPTH {
        return Err(bad(
            format!("`{part}`: inner locators nest at most {MAX_DEPTH} deep"),
            "split it into steps joined by >>",
        ));
    }
    let source = unquote_inner(value);
    let locator = parse_at(&source, depth + 1)?;
    let first = match locator.steps.first() {
        Some(Step::Ref { number }) => Some(("ref", format!("e{number}"))),
        Some(Step::Frame { query }) => Some(("frame", format!("frame={query}"))),
        _ => None,
    };
    if let Some((what, first)) = first {
        return Err(bad(
            format!("`{part}`: an inner locator cannot be a ref or frame="),
            format!("put the {what} first: {first} >> …"),
        ));
    }
    Ok(Inner {
        steps: locator.steps,
        source: locator.source,
    })
}

/// Strips one pair of matching quotes, unescaping only `\\` and the
/// quote: the inner locator's own escapes (`/a\s+b/`, `\'`) stay.
fn unquote_inner(value: &str) -> String {
    for q in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
            let inner = &value[1..value.len() - 1];
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars().peekable();
            while let Some(c) = chars.next() {
                match chars.peek() {
                    Some(&next) if c == '\\' && (next == q || next == '\\') => {
                        out.push(next);
                        chars.next();
                    }
                    _ => out.push(c),
                }
            }
            return out;
        }
    }
    value.to_owned()
}

fn example(kind: &str) -> &'static str {
    match kind {
        "css" => "css=button.primary",
        "xpath" => "xpath=//button",
        "testid" => "testid=submit",
        "frame" => "frame=payment >> #card",
        _ => "text=Sign in",
    }
}

fn not_a_locator(part: &str, index: usize) -> ErrorInfo {
    let what = if index == 0 {
        format!("`{part}` is not a locator")
    } else {
        format!("`{part}` is not a locator step")
    };
    let quoted = if part.contains('"') {
        format!("'{part}'")
    } else {
        format!("\"{part}\"")
    };
    let hint = if part.contains(char::is_whitespace) {
        format!("use text={quoted} to find it by its text, or css=<selector>")
    } else {
        format!("use text={quoted} for text, or css={part} for a CSS selector")
    };
    bad(what, hint)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Strips one pair of matching quotes, unescaping `\"` and `\\`.
fn unquote(value: &str) -> String {
    let value = value.trim();
    for q in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
            let inner = &value[1..value.len() - 1];
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                } else {
                    out.push(c);
                }
            }
            return out;
        }
    }
    value.to_owned()
}

/// A text value: `"exact"`, `/regex/flags` or a substring.
fn text_value(value: &str, part: &str) -> Result<TextMatch, ErrorInfo> {
    let value = value.trim();
    let empty = || {
        bad(
            format!("`{part}` has no text"),
            "e.g. text=Sign in, text=\"Sign in\" (exact) or text=/sign.?in/i",
        )
    };
    if value.is_empty() {
        return Err(empty());
    }
    if value.len() >= 2 && value.starts_with(['"', '\'']) && value.ends_with(&value[..1]) {
        return Ok(TextMatch {
            mode: TextMode::Exact,
            value: unquote(value),
            flags: String::new(),
        });
    }
    if let Some(rest) = value.strip_prefix('/')
        && let Some(end) = rest.rfind('/')
    {
        let (source, flags) = (&rest[..end], &rest[end + 1..]);
        if !source.is_empty() && flags.chars().all(|c| "dimsuvy".contains(c)) {
            return Ok(TextMatch {
                mode: TextMode::Regex,
                value: source.to_owned(),
                flags: flags.to_owned(),
            });
        }
    }
    Ok(TextMatch {
        mode: TextMode::Substring,
        value: value.to_owned(),
        flags: String::new(),
    })
}

/// `button[name="Sign in"][pressed]`.
fn parse_role(value: &str, part: &str) -> Result<RoleQuery, ErrorInfo> {
    let role_end = value.find('[').unwrap_or(value.len());
    let role = value[..role_end].trim();
    if role.is_empty() || !role.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(bad(
            format!("`{part}`: role= needs an ARIA role"),
            "e.g. role=button, role=link[name=\"Home\"], role=heading[level=1]",
        ));
    }
    let mut query = RoleQuery {
        role: role.to_ascii_lowercase(),
        ..RoleQuery::default()
    };
    let mut rest = &value[role_end..];
    while !rest.trim().is_empty() {
        let body = rest.trim_start();
        let Some(body) = body.strip_prefix('[') else {
            return Err(bad(
                format!("`{part}`: expected [attribute] after the role"),
                "e.g. role=button[name=\"Save\"][disabled=false]",
            ));
        };
        let end = attribute_end(body).ok_or_else(|| {
            bad(
                format!("`{part}`: a [ is not closed"),
                "e.g. role=button[name=\"Save\"]",
            )
        })?;
        role_attribute(&mut query, &body[..end], part)?;
        rest = &body[end + 1..];
    }
    Ok(query)
}

/// Where the `]` that closes an attribute is (outside quotes and a regex).
fn attribute_end(body: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '/') if body[..i].ends_with('=') => quote = Some('/'),
            (None, ']') => return Some(i),
            _ => {}
        }
    }
    None
}

fn role_attribute(query: &mut RoleQuery, attr: &str, part: &str) -> Result<(), ErrorInfo> {
    let (key, value) = match attr.split_once('=') {
        Some((k, v)) => (k.trim(), Some(v.trim())),
        None => (attr.trim(), None),
    };
    let flag = |what: &str| -> Result<bool, ErrorInfo> {
        match value {
            None => Ok(true),
            Some(v) => parse_bool(v).ok_or_else(|| {
                bad(
                    format!("`{part}`: [{what}] takes true or false"),
                    format!("e.g. [{what}] or [{what}=false]"),
                )
            }),
        }
    };
    let tri = |what: &str| -> Result<Tri, ErrorInfo> {
        match value {
            None | Some("true") => Ok(Tri::True),
            Some("false") => Ok(Tri::False),
            Some("mixed") => Ok(Tri::Mixed),
            Some(_) => Err(bad(
                format!("`{part}`: [{what}] takes true, false or mixed"),
                format!("e.g. [{what}] or [{what}=false]"),
            )),
        }
    };
    match key {
        "name" => {
            let value = value.unwrap_or_default();
            query.name = Some(text_value(value, part)?);
        }
        "checked" => query.checked = Some(tri(key)?),
        "pressed" => query.pressed = Some(tri(key)?),
        "disabled" => query.disabled = Some(flag(key)?),
        "expanded" => query.expanded = Some(flag(key)?),
        "selected" => query.selected = Some(flag(key)?),
        "include-hidden" => query.include_hidden = flag(key)?,
        "level" => {
            query.level = Some(value.and_then(|v| v.parse().ok()).ok_or_else(|| {
                bad(
                    format!("`{part}`: [level] takes a number"),
                    "e.g. role=heading[level=2]",
                )
            })?);
        }
        _ => {
            return Err(bad(
                format!("`{part}`: unknown role attribute [{key}]"),
                "use name, checked, disabled, expanded, level, pressed, selected or include-hidden",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use protocol::ErrorKind;
    use serde_json::json;

    use super::{Locator, Step, parse_locator};

    fn steps(text: &str) -> serde_json::Value {
        let locator: Locator = parse_locator(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert_eq!(locator.to_string(), text.trim());
        serde_json::to_value(&locator.steps).unwrap()
    }

    fn error(text: &str) -> (String, String) {
        let err = parse_locator(text).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadLocator, "{text}");
        assert_eq!(err.exit_code(), 2);
        (err.message, err.hint.unwrap_or_default())
    }

    /// One row per grammar form: what it parses to.
    #[test]
    fn every_form_of_the_grammar() {
        let table = [
            ("e12", json!([{"kind": "ref", "number": 12}])),
            ("@e3", json!([{"kind": "ref", "number": 3}])),
            ("#submit", json!([{"kind": "css", "value": "#submit"}])),
            (".item > a", json!([{"kind": "css", "value": ".item > a"}])),
            ("[name=q]", json!([{"kind": "css", "value": "[name=q]"}])),
            (
                "css=button.primary",
                json!([{"kind": "css", "value": "button.primary"}]),
            ),
            ("//h1", json!([{"kind": "xpath", "value": "//h1"}])),
            (
                "(//li)[2]",
                json!([{"kind": "xpath", "value": "(//li)[2]"}]),
            ),
            (
                "xpath=//a[@href]",
                json!([{"kind": "xpath", "value": "//a[@href]"}]),
            ),
            (
                "text=Sign in",
                json!([{"kind": "text", "text": {"mode": "substring", "value": "Sign in"}}]),
            ),
            (
                "text=\"Sign in\"",
                json!([{"kind": "text", "text": {"mode": "exact", "value": "Sign in"}}]),
            ),
            (
                "text=/sign\\s*in/i",
                json!([{"kind": "text", "text": {"mode": "regex", "value": "sign\\s*in", "flags": "i"}}]),
            ),
            (
                "'Say \\'hi\\''",
                json!([{"kind": "text", "text": {"mode": "exact", "value": "Say 'hi'"}}]),
            ),
            (
                "role=button[name=\"Sign in\"]",
                json!([{"kind": "role", "role": "button", "name": {"mode": "exact", "value": "Sign in"}}]),
            ),
            (
                "role=checkbox[checked=mixed][disabled=false][include-hidden]",
                json!([{"kind": "role", "role": "checkbox", "checked": "mixed", "disabled": false, "includeHidden": true}]),
            ),
            (
                "role=heading[level=2][name=/intro/i]",
                json!([{"kind": "role", "role": "heading", "level": 2, "name": {"mode": "regex", "value": "intro", "flags": "i"}}]),
            ),
            (
                "role=button[pressed][expanded=true][selected]",
                json!([{"kind": "role", "role": "button", "pressed": "true", "expanded": true, "selected": true}]),
            ),
            ("role=link[name=a]b]", json!(null)),
            (
                "label=Email",
                json!([{"kind": "label", "text": {"mode": "substring", "value": "Email"}}]),
            ),
            (
                "placeholder=\"Search…\"",
                json!([{"kind": "placeholder", "text": {"mode": "exact", "value": "Search…"}}]),
            ),
            (
                "alt=logo",
                json!([{"kind": "alt", "text": {"mode": "substring", "value": "logo"}}]),
            ),
            (
                "title=Close",
                json!([{"kind": "title", "text": {"mode": "substring", "value": "Close"}}]),
            ),
            (
                "testid=login-form",
                json!([{"kind": "testid", "value": "login-form", "attribute": "data-testid"}]),
            ),
            (
                "#list >> text=Delete >> nth=-1",
                json!([
                    {"kind": "css", "value": "#list"},
                    {"kind": "text", "text": {"mode": "substring", "value": "Delete"}},
                    {"kind": "nth", "index": -1}
                ]),
            ),
            (
                ".row >> visible=true >> has-text=\"Invoice 12\"",
                json!([
                    {"kind": "css", "value": ".row"},
                    {"kind": "visible", "visible": true},
                    {"kind": "has-text", "text": {"mode": "exact", "value": "Invoice 12"}}
                ]),
            ),
            (
                "frame=payment >> frame=\"inner\" >> #card",
                json!([
                    {"kind": "frame", "query": "payment"},
                    {"kind": "frame", "query": "inner"},
                    {"kind": "css", "value": "#card"}
                ]),
            ),
            (
                "e8 >> role=textbox",
                json!([{"kind": "ref", "number": 8}, {"kind": "role", "role": "textbox"}]),
            ),
            // `>>` inside quotes or brackets does not split.
            (
                "text=Don't save >> nth=0",
                json!([
                    {"kind": "text", "text": {"mode": "substring", "value": "Don't save"}},
                    {"kind": "nth", "index": 0}
                ]),
            ),
            (
                "text=\"a >> b\"",
                json!([{"kind": "text", "text": {"mode": "exact", "value": "a >> b"}}]),
            ),
            (
                "[data-x=\"a>>b\"] >> nth=0",
                json!([{"kind": "css", "value": "[data-x=\"a>>b\"]"}, {"kind": "nth", "index": 0}]),
            ),
            (
                ".row >> has-not-text=/paid/i",
                json!([
                    {"kind": "css", "value": ".row"},
                    {"kind": "has-not-text", "text": {"mode": "regex", "value": "paid", "flags": "i"}}
                ]),
            ),
            // An inner locator runs to the next `>>`, or is quoted.
            (
                ".row >> has=role=button[name=\"Delete\"] >> nth=0",
                json!([
                    {"kind": "css", "value": ".row"},
                    {"kind": "has", "steps": [
                        {"kind": "role", "role": "button", "name": {"mode": "exact", "value": "Delete"}}
                    ]},
                    {"kind": "nth", "index": 0}
                ]),
            ),
            (
                ".row >> has-not=\"role=button >> visible=true\"",
                json!([
                    {"kind": "css", "value": ".row"},
                    {"kind": "has-not", "steps": [
                        {"kind": "role", "role": "button"},
                        {"kind": "visible", "visible": true}
                    ]}
                ]),
            ),
            (
                "role=button >> or='role=link >> has-text=\"Home\"'",
                json!([
                    {"kind": "role", "role": "button"},
                    {"kind": "or", "steps": [
                        {"kind": "role", "role": "link"},
                        {"kind": "has-text", "text": {"mode": "exact", "value": "Home"}}
                    ]}
                ]),
            ),
            // `\"` and `\\` unescape; the inner locator's own escapes stay.
            (
                "css=li >> and=\"text=\\\"Say \\\\\\\"hi\\\\\\\"\\\" >> nth=0\"",
                json!([
                    {"kind": "css", "value": "li"},
                    {"kind": "and", "steps": [
                        {"kind": "text", "text": {"mode": "exact", "value": "Say \"hi\""}},
                        {"kind": "nth", "index": 0}
                    ]}
                ]),
            ),
            (
                ".item >> and=\"text=/a\\s+b/ >> text=\\\"x y\\\"\"",
                json!([
                    {"kind": "css", "value": ".item"},
                    {"kind": "and", "steps": [
                        {"kind": "text", "text": {"mode": "regex", "value": "a\\s+b"}},
                        {"kind": "text", "text": {"mode": "exact", "value": "x y"}}
                    ]}
                ]),
            ),
            (
                "e4 >> or=#other",
                json!([{"kind": "ref", "number": 4}, {"kind": "or", "steps": [{"kind": "css", "value": "#other"}]}]),
            ),
            (
                ".a >> has=\".b >> has=\\\".c >> has=.d\\\"\"",
                json!([
                    {"kind": "css", "value": ".a"},
                    {"kind": "has", "steps": [
                        {"kind": "css", "value": ".b"},
                        {"kind": "has", "steps": [
                            {"kind": "css", "value": ".c"},
                            {"kind": "has", "steps": [{"kind": "css", "value": ".d"}]}
                        ]}
                    ]}
                ]),
            ),
        ];
        for (typed, want) in table {
            if want.is_null() {
                assert!(parse_locator(typed).is_err(), "{typed}");
                continue;
            }
            assert_eq!(steps(typed), want, "{typed}");
        }
    }

    #[test]
    fn inner_locators() {
        let locator = parse_locator(".row >> has=\"testid=del >> nth=0\"")
            .unwrap()
            .with_testid_attribute(Some("data-qa"));
        let inner = locator.steps[1].inner().unwrap();
        assert_eq!(inner.source, "testid=del >> nth=0");
        assert_eq!(
            inner.steps[0],
            Step::Testid {
                value: "del".into(),
                attribute: "data-qa".into()
            }
        );
        assert_eq!(locator.without_last().as_deref(), Some(".row"));
        assert_eq!(
            locator.typed_steps(),
            [
                (".row".to_owned(), false),
                ("has=\"testid=del >> nth=0\"".to_owned(), true)
            ]
        );
        assert_eq!(parse_locator(".row").unwrap().without_last(), None);

        // Four deep is the limit.
        let mut text = ".d".to_owned();
        for depth in 0..4 {
            text = format!(
                ".x{depth} >> has=\"{}\"",
                text.replace('\\', "\\\\").replace('"', "\\\"")
            );
        }
        assert!(parse_locator(&text).is_ok(), "{text}");
        let deeper = format!(
            ".top >> has=\"{}\"",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let (message, _) = error(&deeper);
        assert!(
            message.ends_with(": inner locators nest at most 4 deep"),
            "{message}"
        );
        assert!(message.starts_with("`has=\".d\"`"), "{message}");

        assert_eq!(
            error(".row >> has=e12"),
            (
                "`has=e12`: an inner locator cannot be a ref or frame=".to_owned(),
                "put the ref first: e12 >> …".to_owned()
            )
        );
        assert_eq!(
            error(".row >> and=\"frame=pay >> #card\""),
            (
                "`and=\"frame=pay >> #card\"`: an inner locator cannot be a ref or frame="
                    .to_owned(),
                "put the frame first: frame=pay >> …".to_owned()
            )
        );
        assert_eq!(
            error("has=.b").0,
            "`has=.b`: has=.b filters what comes before it, but nothing does"
        );
        assert_eq!(
            error("or=\"#a\" >> #b").0,
            "`or=\"#a\" >> #b`: or=\"#a\" filters what comes before it, but nothing does"
        );
        assert_eq!(error(".row >> has=").0, "`has=` has no locator");
        assert_eq!(
            error(".row >> has-not-text=").0,
            "`has-not-text=` has no text"
        );
        assert_eq!(
            error(".row >> has=\"nth=0\"").0,
            "`nth=0`: nth=0 filters what comes before it, but nothing does"
        );
        assert_eq!(error(".row >> has=Delete").0, "`Delete` is not a locator");
    }

    #[test]
    fn frame_and_ref_steps_are_left_to_the_engine() {
        let locator = parse_locator("frame=pay >> #card >> nth=0").unwrap();
        assert_eq!(locator.in_frame_steps().len(), 2);
        let locator = parse_locator("e4").unwrap();
        assert!(locator.in_frame_steps().is_empty());
    }

    #[test]
    fn the_testid_attribute_is_configurable() {
        let locator = parse_locator("#form >> testid=save")
            .unwrap()
            .with_testid_attribute(Some("data-qa"));
        assert_eq!(
            locator.steps[1],
            Step::Testid {
                value: "save".into(),
                attribute: "data-qa".into()
            }
        );
    }

    #[test]
    fn mistakes_say_what_works() {
        let (message, hint) = error("button");
        assert_eq!(message, "`button` is not a locator");
        assert_eq!(
            hint,
            "use text=\"button\" for text, or css=button for a CSS selector"
        );
        assert_eq!(error("").0, "the locator is empty");
        assert_eq!(error("css=").0, "`css=` has no selector");
        assert_eq!(error("text=").0, "`text=` has no text");
        assert_eq!(error("#a >> ").0, "`#a >>` has an empty step");
        assert_eq!(
            error("#a >> e5").0,
            "`#a >> e5`: a ref (e5) can only come first"
        );
        assert_eq!(
            error("#a >> frame=x").0,
            "`#a >> frame=x`: frame= can only come first"
        );
        assert_eq!(
            error("nth=0").0,
            "`nth=0`: nth=0 filters what comes before it, but nothing does"
        );
        assert_eq!(
            error("#a >> nth=first").0,
            "`nth=first`: nth= takes a number"
        );
        assert_eq!(
            error("#a >> visible=yes").0,
            "`visible=yes`: visible= takes true or false"
        );
        assert_eq!(error("foo=bar").0, "`foo=bar`: unknown locator kind `foo=`");
        assert_eq!(
            error("role=button[nme=x]").0,
            "`role=button[nme=x]`: unknown role attribute [nme]"
        );
        assert_eq!(
            error("role=[name=x]").0,
            "`role=[name=x]`: role= needs an ARIA role"
        );
        assert_eq!(
            error("role=heading[level=two]").0,
            "`role=heading[level=two]`: [level] takes a number"
        );
        assert_eq!(
            error("role=button[name=\"x\"").0,
            "`role=button[name=\"x\"`: a [ is not closed"
        );
        assert_eq!(
            error("text=\"open").0,
            "`text=\"open`: a \" quote is not closed"
        );
        assert_eq!(error("#list >> Delete").0, "`Delete` is not a locator step");
    }
}

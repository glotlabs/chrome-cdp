//! Locators: how commands name elements (overview §5, plan 04).
//!
//! Milestone 3.4's `wait <locator>` needs only the selector kinds the page
//! can evaluate itself: `css=…` (or a bare string starting with `#`, `.`
//! or `[`) and `xpath=…` (or a bare `//…`). Plan 04 adds refs (`e12`),
//! `text=`, `role=`, `label=`, `placeholder=`, `testid=` and `>>` chains
//! here; until then they are `BadLocator` errors that say what works.

use std::fmt;

use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde::Serialize;

/// A parsed locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Locator {
    Css(String),
    Xpath(String),
}

impl fmt::Display for Locator {
    /// As the user would type it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Css(css) if css.starts_with(['#', '.', '[']) => f.write_str(css),
            Self::Css(css) => write!(f, "css={css}"),
            Self::Xpath(xpath) if xpath.starts_with("//") => f.write_str(xpath),
            Self::Xpath(xpath) => write!(f, "xpath={xpath}"),
        }
    }
}

/// Parses a locator as typed.
pub fn parse_locator(text: &str) -> Result<Locator, ErrorInfo> {
    let text = text.trim();
    if text.is_empty() {
        return Err(
            ErrorInfo::new(ErrorKind::BadLocator, "the locator is empty")
                .hint(Hint::text("use css=<selector> or xpath=<expression>")),
        );
    }
    if let Some(css) = text.strip_prefix("css=") {
        return non_empty(css, text).map(Locator::Css);
    }
    if let Some(xpath) = text.strip_prefix("xpath=") {
        return non_empty(xpath, text).map(Locator::Xpath);
    }
    if text.starts_with(['#', '.', '[']) {
        return Ok(Locator::Css(text.to_owned()));
    }
    if text.starts_with("//") || text.starts_with("(//") {
        return Ok(Locator::Xpath(text.to_owned()));
    }
    Err(unsupported(text))
}

fn non_empty(value: &str, text: &str) -> Result<String, ErrorInfo> {
    let value = value.trim();
    if value.is_empty() {
        Err(
            ErrorInfo::new(ErrorKind::BadLocator, format!("`{text}` has no selector"))
                .hint(Hint::text("e.g. css=#submit or xpath=//button")),
        )
    } else {
        Ok(value.to_owned())
    }
}

/// A locator kind that snapshots and plan 04 bring.
fn unsupported(text: &str) -> ErrorInfo {
    let prefix = text.split_once('=').map(|(p, _)| p);
    let is_ref = text
        .strip_prefix('@')
        .unwrap_or(text)
        .strip_prefix('e')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let (message, hint) = match prefix {
        Some("text") => (
            format!("`{text}`: text= locators are not supported yet"),
            Hint::run(Cmd::new("wait").flag("text", text.trim_start_matches("text=")))
                .why("waits for visible text"),
        ),
        Some(kind @ ("role" | "label" | "placeholder" | "testid")) => (
            format!("`{text}`: {kind}= locators are not supported yet"),
            Hint::text("use css=<selector> or xpath=<expression>"),
        ),
        _ if is_ref => (
            format!("`{text}`: refs are not supported yet"),
            Hint::text("use css=<selector> or xpath=<expression>"),
        ),
        _ => (
            format!("`{text}` is not a locator"),
            Hint::text(format!(
                "use css={text} for a CSS selector, or xpath=<expression>"
            )),
        ),
    };
    ErrorInfo::new(ErrorKind::BadLocator, message).hint(hint)
}

#[cfg(test)]
mod tests {
    use protocol::ErrorKind;

    use super::{Locator, parse_locator};

    #[test]
    fn css_and_xpath() {
        let css = |s: &str| Locator::Css(s.to_owned());
        assert_eq!(parse_locator("#submit").unwrap(), css("#submit"));
        assert_eq!(parse_locator(".item > a").unwrap(), css(".item > a"));
        assert_eq!(parse_locator("[name=q]").unwrap(), css("[name=q]"));
        assert_eq!(
            parse_locator("css=button.primary").unwrap(),
            css("button.primary")
        );
        assert_eq!(
            parse_locator("//h1").unwrap(),
            Locator::Xpath("//h1".into())
        );
        assert_eq!(
            parse_locator("xpath=(//li)[2]").unwrap(),
            Locator::Xpath("(//li)[2]".into())
        );
        for typed in ["#submit", "css=button", "//h1", "xpath=(//li)[2]"] {
            assert_eq!(parse_locator(typed).unwrap().to_string(), typed);
        }
    }

    #[test]
    fn other_kinds_say_what_works() {
        let err = parse_locator("text=Sign in").unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadLocator);
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.hint.as_deref(),
            Some("chrome-cdp wait --text 'Sign in'   (waits for visible text)")
        );
        for typed in ["role=button", "e12", "@e3", "button", "css=", ""] {
            let err = parse_locator(typed).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadLocator, "{typed}");
        }
        assert_eq!(
            parse_locator("button").unwrap_err().hint.as_deref(),
            Some("use css=button for a CSS selector, or xpath=<expression>")
        );
    }
}

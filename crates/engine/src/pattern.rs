//! URL patterns for `wait --url`, `--request` and `--response`.
//!
//! - `/regex/flags`: a regular expression (Rust syntax; the flags `i`, `m`
//!   and `s` apply, JavaScript's others are ignored);
//! - a glob with `*` (any characters but `/`) or `**` (any characters):
//!   it must match the whole URL;
//! - anything else matches URLs that contain it (`/dashboard`).

use std::fmt;

use protocol::{ErrorInfo, ErrorKind, Hint};
use regex::Regex;

/// A compiled URL pattern.
#[derive(Debug, Clone)]
pub(crate) struct UrlPattern {
    /// As typed.
    source: String,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Contains(String),
    Regex(Regex),
}

impl fmt::Display for UrlPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl UrlPattern {
    /// Parses `text`; a bad regular expression is a `BadArgs` error.
    pub(crate) fn parse(text: &str) -> Result<Self, ErrorInfo> {
        let source = text.to_owned();
        if text.is_empty() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "the URL pattern is empty").hint(Hint::text(
                    "e.g. /dashboard, **/api/*, or /\\/items\\/\\d+/",
                )),
            );
        }
        if let Some((body, flags)) = regex_literal(text) {
            let inline: String = flags.chars().filter(|c| "ims".contains(*c)).collect();
            let full = if inline.is_empty() {
                body.to_owned()
            } else {
                format!("(?{inline}){body}")
            };
            let regex = Regex::new(&full).map_err(|e| {
                let why = e.to_string();
                let first = why.lines().last().unwrap_or_default().trim().to_owned();
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("`{text}` is not a valid regular expression: {first}"),
                )
                .hint(Hint::text(
                    "fix the expression, or use a glob like **/api/* or a part of the URL",
                ))
            })?;
            return Ok(Self {
                source,
                kind: Kind::Regex(regex),
            });
        }
        if text.contains('*') {
            let mut re = String::from("^");
            let mut rest = text;
            while let Some(i) = rest.find('*') {
                re.push_str(&regex::escape(&rest[..i]));
                if rest[i..].starts_with("**") {
                    re.push_str(".*");
                    rest = &rest[i + 2..];
                } else {
                    re.push_str("[^/]*");
                    rest = &rest[i + 1..];
                }
            }
            re.push_str(&regex::escape(rest));
            re.push('$');
            // Escaped literals and wildcards always compile.
            let regex = Regex::new(&re)
                .map_err(|e| ErrorInfo::new(ErrorKind::BadArgs, format!("`{text}`: {e}")))?;
            return Ok(Self {
                source,
                kind: Kind::Regex(regex),
            });
        }
        Ok(Self {
            source,
            kind: Kind::Contains(text.to_owned()),
        })
    }

    /// Whether `url` matches.
    pub(crate) fn matches(&self, url: &str) -> bool {
        match &self.kind {
            Kind::Contains(part) => url.contains(part.as_str()),
            Kind::Regex(regex) => regex.is_match(url),
        }
    }
}

/// `/body/flags` → `(body, flags)`.
fn regex_literal(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_prefix('/')?;
    let end = inner.rfind('/')?;
    let (body, flags) = (&inner[..end], &inner[end + 1..]);
    // JavaScript's flags, each at most once: `/api/items` is a path.
    let valid = flags
        .chars()
        .enumerate()
        .all(|(i, c)| "dgimsuvy".contains(c) && !flags[..i].contains(c));
    (!body.is_empty() && valid).then_some((body, flags))
}

#[cfg(test)]
mod tests {
    use protocol::ErrorKind;

    use super::UrlPattern;

    fn matches(pattern: &str, url: &str) -> bool {
        UrlPattern::parse(pattern).unwrap().matches(url)
    }

    #[test]
    fn plain_text_is_a_part_of_the_url() {
        assert!(matches("/dashboard", "https://x.test/dashboard?tab=1"));
        assert!(matches("x.test/a", "https://x.test/a"));
        assert!(!matches("/dashboard", "https://x.test/login"));
        // `?` and `.` are literal.
        assert!(matches("?view=about", "http://x/spa.html?view=about"));
    }

    #[test]
    fn globs_match_the_whole_url() {
        assert!(matches("**/api/*", "https://x.test/v1/api/items"));
        assert!(!matches("**/api/*", "https://x.test/api/items/3"));
        assert!(matches("**/api/**", "https://x.test/api/items/3"));
        assert!(matches("https://x.test/*.html", "https://x.test/a.html"));
        assert!(!matches("https://x.test/*.html", "https://x.test/a/b.html"));
        assert!(!matches("*.html", "https://x.test/a.html?q=1"));
    }

    #[test]
    fn regular_expressions() {
        assert!(matches(r"/\/items\/\d+$/", "https://x/items/42"));
        assert!(!matches(r"/\/items\/\d+$/", "https://x/items/new"));
        assert!(matches("/DASHBOARD/i", "https://x/dashboard"));
        // A path with two slashes but no valid flags is text.
        assert!(matches("/a/b.c", "https://x/a/b.c"));
        assert!(matches("/api/items", "https://x/api/items"));
        assert!(!matches("/api/items", "https://x/API"));
        let err = UrlPattern::parse("/(/").unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
        assert!(
            err.message
                .starts_with("`/(/` is not a valid regular expression")
        );
        assert_eq!(UrlPattern::parse("").unwrap_err().exit_code(), 2);
    }
}

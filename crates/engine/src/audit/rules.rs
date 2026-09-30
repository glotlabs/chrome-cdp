//! `audit`'s categories and the rules of those whose rules chrome-cdp
//! defines here (plan 17, §Severities): each with its severity, fixed per
//! rule. `issues` rules come from Chrome's codes (`observe::issues`),
//! `a11y` rules from `a11y.ts` (level A is an error, AA a warning). The
//! command reference's `audit` page is generated from [`RULES`].

use protocol::schema::Severity;

/// The categories, in the report's order.
pub const CATEGORIES: [&str; 7] = [
    "issues", "a11y", "layout", "console", "network", "security", "meta",
];

/// The categories a LOCATOR can scope: without `--category`, `audit
/// LOCATOR` checks only these.
pub const SCOPED: [&str; 3] = ["a11y", "layout", "security"];

/// A rule of `audit`'s own.
#[derive(Debug, Clone, Copy)]
pub struct AuditRule {
    pub category: &'static str,
    pub id: &'static str,
    /// Its severity (the most severe, where [`AuditRule::about`] says it
    /// can be less).
    pub severity: Severity,
    /// What it checks, for the docs.
    pub about: &'static str,
}

const fn rule(
    category: &'static str,
    id: &'static str,
    severity: Severity,
    about: &'static str,
) -> AuditRule {
    AuditRule {
        category,
        id,
        severity,
        about,
    }
}

use Severity::{Error, Info, Warning};

/// Every rule of the `layout`, `console`, `network`, `security` and
/// `meta` categories.
pub const RULES: &[AuditRule] = &[
    rule(
        "layout",
        "covered",
        Warning,
        "an interactive element whose centre something else covers",
    ),
    rule(
        "layout",
        "overlap",
        Warning,
        "two interactive elements whose boxes overlap",
    ),
    rule(
        "layout",
        "clipped",
        Warning,
        "text cut off by overflow: hidden or clip",
    ),
    rule(
        "layout",
        "truncated",
        Info,
        "text cut on purpose, by an ellipsis or a line clamp",
    ),
    rule("layout", "overflow-x", Warning, "the page scrolls sideways"),
    rule(
        "layout",
        "offscreen",
        Warning,
        "an interactive element no scroll can reach",
    ),
    rule(
        "layout",
        "small-target",
        Warning,
        "an interactive element under 24x24 px",
    ),
    rule(
        "layout",
        "contrast",
        Warning,
        "text below 4.5:1 (3:1 when large) against the pixels behind it",
    ),
    rule(
        "console",
        "uncaught-exception",
        Error,
        "an exception no code caught, since the page loaded",
    ),
    rule(
        "console",
        "console-error",
        Warning,
        "a console.error call, since the page loaded",
    ),
    rule(
        "network",
        "request-failed",
        Error,
        "a request that got no answer (net::ERR_…); a warning when not to the page's origin",
    ),
    rule(
        "network",
        "http-error",
        Error,
        "a response with a 4xx or 5xx status; a warning when not from the page's origin",
    ),
    rule(
        "security",
        "http-password",
        Error,
        "a password field on an http: page (not localhost)",
    ),
    rule(
        "security",
        "form-http-action",
        Error,
        "a form that sends its data to an http: URL (not localhost)",
    ),
    rule(
        "security",
        "http-page",
        Warning,
        "the page is served over http: (not localhost)",
    ),
    rule(
        "security",
        "no-csp",
        Warning,
        "the document has no Content-Security-Policy (header or meta tag)",
    ),
    rule(
        "security",
        "cookie-no-secure",
        Warning,
        "on an https: page, a cookie of its site without Secure",
    ),
    rule(
        "security",
        "cookie-no-httponly",
        Warning,
        "a cookie named like a session (sid, session, token, auth) that scripts can read",
    ),
    rule(
        "security",
        "cookie-no-samesite",
        Warning,
        "a cookie of the page's site without SameSite",
    ),
    rule(
        "security",
        "no-hsts",
        Info,
        "an https: page without Strict-Transport-Security (not localhost)",
    ),
    rule("meta", "no-title", Warning, "the document has no <title>"),
    rule("meta", "no-lang", Warning, "<html> has no lang attribute"),
    rule(
        "meta",
        "no-viewport",
        Warning,
        "no <meta name=viewport>: phones show the desktop layout, zoomed out",
    ),
    rule(
        "meta",
        "viewport-blocks-zoom",
        Warning,
        "the viewport meta tag blocks zooming (user-scalable=no, maximum-scale under 2)",
    ),
    rule("meta", "no-description", Info, "no <meta name=description>"),
    rule(
        "meta",
        "noindex",
        Info,
        "a robots meta tag keeps search engines out (noindex)",
    ),
];

/// The severity of `category`'s rule `id` (warning for one not in
/// [`RULES`]).
pub(crate) fn severity(category: &str, id: &str) -> Severity {
    RULES
        .iter()
        .find(|r| r.category == category && r.id == id)
        .map_or(Warning, |r| r.severity)
}

#[cfg(test)]
mod tests {
    use super::{CATEGORIES, RULES, SCOPED};

    #[test]
    fn rules_are_unique_and_in_known_categories() {
        let mut seen = std::collections::HashSet::new();
        for rule in RULES {
            assert!(CATEGORIES.contains(&rule.category), "{}", rule.category);
            assert!(seen.insert((rule.category, rule.id)), "{}", rule.id);
        }
        assert!(SCOPED.iter().all(|c| CATEGORIES.contains(c)));
    }
}

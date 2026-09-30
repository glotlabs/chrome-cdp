//! A tab's issues (plan 17, milestone 17.1): Chrome's own findings about
//! its pages, what DevTools shows in its Issues panel, for `issues`,
//! `expect --no-issues` and the post-action report.
//!
//! Every page, iframe and worker session has `Audits` enabled from
//! before its first script (see `tabs::features`); on a target that was
//! running already (`connect`) Chrome replays the issues raised before.
//! `Audits.issueAdded` brings each occurrence, which [`parse`] turns into
//! a rule id (the kebab-case of Chrome's code and of its reason), a fixed
//! severity ([`CODES`]), a one-line message and what it is about.
//!
//! Chrome raises one issue per occurrence: [`Issues`] folds those with
//! the same rule about the same thing into one with a count, within one
//! document of the main frame. An issue raised again moves to the end,
//! with a new number, so `issues` shows it again. The buffer is bounded
//! like the console's; the read commands share the console's selection
//! (see `super::commands`).
//!
//! What an issue is about is kept as Chrome gave it (a backend node id, a
//! request id) and named when read: a ref for an element (see
//! `super::issue_commands`), the network log's id for a request.

use std::collections::VecDeque;

use protocol::schema::{FindingCookie, FindingTarget, Issue, Severity, SourceLine};
use serde_json::Value;

use super::console::FromWorker;
use crate::input::origin;
use crate::tabs::commands::path_of;

/// How many issues a tab keeps. Repeats fold into one, so this many
/// distinct issues is a lot.
pub(crate) const CAPACITY: usize = 1000;

/// How much of an issue's message is kept.
const MESSAGE_LEN: usize = 500;

/// Every `InspectorIssueCode` of the vendored protocol: the rule id it
/// starts with, its severity (some depend on the details; see
/// [`parse`]), and what it is about. `docs/commands/issues.md` lists
/// this table; a test checks that it names every code.
pub(crate) const CODES: &[Code] = &[
    Code::new(
        "CookieIssue",
        "cookie",
        Severity::Warning,
        "a cookie was blocked, or will be",
    ),
    Code::new(
        "MixedContentIssue",
        "mixed-content",
        Severity::Error,
        "an http: resource on an https: page (error when blocked, else warning)",
    ),
    Code::new(
        "BlockedByResponseIssue",
        "blocked-by-response",
        Severity::Error,
        "a response's COEP, COOP or CORP header blocked a load",
    ),
    Code::new(
        "HeavyAdIssue",
        "heavy-ad",
        Severity::Warning,
        "an ad frame used too much network or CPU (warning when removed, else info)",
    ),
    Code::new(
        "ContentSecurityPolicyIssue",
        "content-security-policy",
        Severity::Error,
        "the page's CSP blocked something (warning when report-only)",
    ),
    Code::new(
        "SharedArrayBufferIssue",
        "shared-array-buffer",
        Severity::Error,
        "SharedArrayBuffer used without cross-origin isolation (warning when only warned)",
    ),
    Code::new(
        "CorsIssue",
        "cors",
        Severity::Error,
        "CORS blocked a request (warning when only warned)",
    ),
    Code::new(
        "QuirksModeIssue",
        "quirks-mode",
        Severity::Info,
        "a document renders in quirks mode (no <!DOCTYPE html>)",
    ),
    Code::new(
        "PartitioningBlobURLIssue",
        "partitioning-blob-url",
        Severity::Warning,
        "a blob: URL was used across storage partitions",
    ),
    Code::new(
        "NavigatorUserAgentIssue",
        "navigator-user-agent",
        Severity::Info,
        "a script reads the reduced navigator.userAgent",
    ),
    Code::new(
        "GenericIssue",
        "",
        Severity::Warning,
        "form problems (labels, autocomplete, ids) and other findings, by reason",
    ),
    Code::new(
        "DeprecationIssue",
        "deprecation",
        Severity::Warning,
        "a deprecated API was used",
    ),
    Code::new(
        "ClientHintIssue",
        "client-hint",
        Severity::Warning,
        "a Client Hints meta tag is not valid",
    ),
    Code::new(
        "FederatedAuthRequestIssue",
        "federated-auth-request",
        Severity::Info,
        "a FedCM sign-in request failed",
    ),
    Code::new(
        "BounceTrackingIssue",
        "bounce-tracking",
        Severity::Info,
        "Chrome will clear the storage of bounce-tracking sites",
    ),
    Code::new(
        "CookieDeprecationMetadataIssue",
        "cookie-deprecation-metadata",
        Severity::Info,
        "third-party cookie deprecation metadata",
    ),
    Code::new(
        "StylesheetLoadingIssue",
        "stylesheet-loading",
        Severity::Warning,
        "a stylesheet failed to load, or an @import came too late",
    ),
    Code::new(
        "FederatedAuthUserInfoRequestIssue",
        "federated-auth-user-info-request",
        Severity::Info,
        "a FedCM user info request failed",
    ),
    Code::new(
        "PropertyRuleIssue",
        "property-rule",
        Severity::Warning,
        "an @property rule is not valid",
    ),
    Code::new(
        "SharedDictionaryIssue",
        "shared-dictionary",
        Severity::Info,
        "a compression dictionary could not be used or stored",
    ),
    Code::new(
        "ElementAccessibilityIssue",
        "element-accessibility",
        Severity::Warning,
        "an element has content its role does not allow (<select>, <option>, <legend>, <summary>)",
    ),
    Code::new(
        "SRIMessageSignatureIssue",
        "sri-message-signature",
        Severity::Error,
        "a response's signature did not verify",
    ),
    Code::new(
        "UnencodedDigestIssue",
        "unencoded-digest",
        Severity::Warning,
        "a response's Unencoded-Digest header is not valid",
    ),
    Code::new(
        "ConnectionAllowlistIssue",
        "connection-allowlist",
        Severity::Warning,
        "a Connection-Allowlist header is not valid, or blocked a connection",
    ),
    Code::new(
        "UserReidentificationIssue",
        "user-reidentification",
        Severity::Warning,
        "a load or canvas read was blocked or noised against fingerprinting",
    ),
    Code::new(
        "PermissionElementIssue",
        "permission-element",
        Severity::Warning,
        "a <permission> element is not valid (info when only warned)",
    ),
    Code::new(
        "PerformanceIssue",
        "performance",
        Severity::Info,
        "a slow API was used (document.cookie)",
    ),
    Code::new(
        "SelectivePermissionsInterventionIssue",
        "selective-permissions-intervention",
        Severity::Info,
        "an ad script was denied a permission-gated API",
    ),
    Code::new(
        "EmailVerificationRequestIssue",
        "email-verification-request",
        Severity::Info,
        "an email verification request failed",
    ),
    Code::new(
        "LazyLoadImageIssue",
        "lazy-load-image",
        Severity::Info,
        "a lazy-loaded image is in view as the page loads",
    ),
];

/// A row of [`CODES`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Code {
    /// Chrome's `InspectorIssueCode`.
    pub(crate) name: &'static str,
    /// The start of its rule ids (the reason follows); empty for
    /// `GenericIssue`, whose reasons say it all.
    pub(crate) rule: &'static str,
    /// Its severity, unless the details say otherwise.
    pub(crate) severity: Severity,
    pub(crate) about: &'static str,
}

impl Code {
    const fn new(
        name: &'static str,
        rule: &'static str,
        severity: Severity,
        about: &'static str,
    ) -> Self {
        Self {
            name,
            rule,
            severity,
            about,
        }
    }
}

/// An issue as [`parse`] reads it from `Audits.issueAdded`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Raised {
    pub(crate) code: String,
    pub(crate) rule: String,
    pub(crate) severity: Severity,
    pub(crate) message: String,
    /// The URL, cookie, script place and frame it names (not the element
    /// or request: those are named when read).
    pub(crate) target: FindingTarget,
    /// The element it is about: its frame (if Chrome said) and backend
    /// node id.
    pub(crate) node: Option<(Option<String>, i64)>,
    /// Chrome's id of the request it is about.
    pub(crate) request: Option<String>,
    pub(crate) docs: Option<&'static str>,
    /// What makes it the same issue as another of its rule.
    pub(crate) about: About,
}

/// What an issue is about, for its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum About {
    /// An element (named when read).
    Element,
    /// A URL: a request, a resource, a document.
    Url(String),
    Cookie {
        name: String,
        domain: String,
    },
    /// A place in a script.
    Source(String, u32),
    /// The page: its path.
    Page,
}

/// The issue `Audits.issueAdded`'s `params` bring.
pub(crate) fn parse(params: &Value) -> Raised {
    let issue = &params["issue"];
    let code = issue["code"].as_str().unwrap_or_default().to_owned();
    let Some(row) = CODES.iter().find(|c| c.name == code) else {
        return unknown(code);
    };
    let details = &issue["details"][details_key(&code)];
    let mut raised = Raised {
        rule: row.rule.to_owned(),
        severity: row.severity,
        message: row.about.to_owned(),
        target: FindingTarget::default(),
        node: None,
        request: None,
        docs: None,
        about: About::Page,
        code,
    };
    let str_of = |key: &str| details[key].as_str().unwrap_or_default();
    // The reason, as the end of the rule id.
    let reason = |raised: &mut Raised, why: &str| {
        let end = sub_rule(row.rule, why);
        if !end.is_empty() {
            raised.rule = if row.rule.is_empty() {
                end
            } else {
                format!("{}-{end}", row.rule)
            };
        }
    };
    source(&mut raised, &details["sourceCodeLocation"]);
    source(&mut raised, &details["location"]);
    request(&mut raised, &details["request"]);
    frame(&mut raised, &details["frame"]["frameId"]);
    frame(&mut raised, &details["affectedFrame"]["frameId"]);
    frame(&mut raised, &details["frameId"]);
    match raised.code.as_str() {
        "CookieIssue" => {
            let why = details["cookieExclusionReasons"]
                .as_array()
                .and_then(|r| r.first())
                .or_else(|| details["cookieWarningReasons"].as_array()?.first())
                .and_then(Value::as_str)
                .unwrap_or_default();
            reason(&mut raised, why);
            let cookie = &details["cookie"];
            let field = |key: &str| cookie[key].as_str().unwrap_or_default().to_owned();
            let (name, domain, path) = if cookie.is_object() {
                (field("name"), field("domain"), field("path"))
            } else {
                let line = str_of("rawCookieLine");
                let name = line.split(['=', ';']).next().unwrap_or_default().trim();
                (name.to_owned(), String::new(), String::new())
            };
            let set = str_of("operation") == "SetCookie";
            let domain_text = if domain.is_empty() {
                String::new()
            } else {
                format!(" ({domain})")
            };
            raised.message = format!("cookie {name:?}{domain_text} {}", cookie_reason(why, set));
            raised.target.cookie = Some(FindingCookie {
                name: name.clone(),
                domain: domain.clone(),
                path,
            });
            if raised.target.url.is_none() {
                raised.target.url = details["cookieUrl"].as_str().map(str::to_owned);
            }
            raised.about = About::Cookie { name, domain };
            raised.docs = Some("https://developer.mozilla.org/docs/Web/HTTP/Guides/Cookies");
        }
        "MixedContentIssue" => {
            let status = str_of("resolutionStatus");
            reason(&mut raised, status);
            let url = str_of("insecureURL");
            let what = match status {
                "MixedContentBlocked" => "blocked",
                "MixedContentAutomaticallyUpgraded" => {
                    raised.severity = Severity::Warning;
                    "upgraded to https:"
                }
                _ => {
                    raised.severity = Severity::Warning;
                    "loaded"
                }
            };
            let kind = details["resourceType"]
                .as_str()
                .map(|t| format!("{} ", t.to_lowercase()))
                .unwrap_or_default();
            raised.message = format!("{kind}{url} {what} on an https: page");
            url_about(&mut raised, url);
            raised.docs = Some("https://developer.mozilla.org/docs/Web/Security/Mixed_content");
        }
        "BlockedByResponseIssue" => {
            let why = str_of("reason");
            reason(&mut raised, why);
            let url = details["request"]["url"].as_str().unwrap_or_default();
            raised.message = format!("{url} blocked: {}", words(&kebab(why)));
            url_about(&mut raised, url);
            frame(&mut raised, &details["blockedFrame"]["frameId"]);
            raised.docs = Some(
                "https://developer.mozilla.org/docs/Web/HTTP/Reference/Headers/Cross-Origin-Resource-Policy",
            );
        }
        "HeavyAdIssue" => {
            reason(&mut raised, str_of("reason"));
            let removed = str_of("resolution") == "HeavyAdBlocked";
            if !removed {
                raised.severity = Severity::Info;
            }
            raised.message = format!(
                "an ad frame {} ({})",
                if removed {
                    "was removed for using too much"
                } else {
                    "uses too much"
                },
                words(&kebab(str_of("reason")))
            );
        }
        "ContentSecurityPolicyIssue" => {
            let kind = str_of("contentSecurityPolicyViolationType");
            reason(&mut raised, kind);
            let report_only = details["isReportOnly"].as_bool().unwrap_or(false);
            if report_only {
                raised.severity = Severity::Warning;
            }
            let blocked = details["blockedURL"].as_str();
            let what = match (kind, blocked) {
                ("kInlineViolation", _) => "an inline script or style".to_owned(),
                ("kEvalViolation", _) => "eval()".to_owned(),
                ("kWasmEvalViolation", _) => "WebAssembly compilation".to_owned(),
                ("kTrustedTypesSinkViolation", _) => {
                    "a string passed to a Trusted Types sink".to_owned()
                }
                ("kTrustedTypesPolicyViolation", _) => "a Trusted Types policy".to_owned(),
                (_, Some(url)) => url.to_owned(),
                _ => "a load".to_owned(),
            };
            let verb = if report_only {
                "would be blocked (report-only)"
            } else {
                "blocked"
            };
            raised.message = format!(
                "{what} {verb} by the Content-Security-Policy's {}",
                str_of("violatedDirective")
            );
            if let Some(node) = details["violatingNodeId"].as_i64() {
                raised.node = Some((None, node));
            }
            match blocked {
                Some(url) if kind == "kURLViolation" => url_about(&mut raised, url),
                _ => raised.about = source_about(&raised),
            }
            raised.docs = Some("https://developer.mozilla.org/docs/Web/HTTP/Guides/CSP");
        }
        "SharedArrayBufferIssue" => {
            let kind = str_of("type");
            reason(&mut raised, kind);
            if details["isWarning"].as_bool().unwrap_or(false) {
                raised.severity = Severity::Warning;
            }
            let what = if kind == "TransferIssue" {
                "transferring"
            } else {
                "creating"
            };
            raised.message = format!(
                "{what} a SharedArrayBuffer needs cross-origin isolation (COOP and COEP headers)"
            );
            raised.about = source_about(&raised);
        }
        "CorsIssue" => {
            let status = &details["corsErrorStatus"];
            let error = status["corsError"].as_str().unwrap_or_default();
            reason(&mut raised, error);
            if details["isWarning"].as_bool().unwrap_or(false) {
                raised.severity = Severity::Warning;
            }
            let url = details["request"]["url"].as_str().unwrap_or_default();
            let mut why = cors_reason(error);
            if let Some(p) = status["failedParameter"].as_str().filter(|p| !p.is_empty()) {
                why = format!("{why} ({p})");
            }
            raised.message = format!("{url} blocked by CORS: {why}");
            url_about(&mut raised, url);
            raised.docs = Some("https://developer.mozilla.org/docs/Web/HTTP/Guides/CORS/Errors");
        }
        "QuirksModeIssue" => {
            let limited = details["isLimitedQuirksMode"].as_bool().unwrap_or(false);
            if limited {
                "quirks-mode-limited".clone_into(&mut raised.rule);
            }
            let url = str_of("url");
            raised.message = format!(
                "{url} renders in {}quirks mode: it has no <!DOCTYPE html>",
                if limited { "limited-" } else { "" }
            );
            url_about(&mut raised, url);
            raised.docs = Some(
                "https://developer.mozilla.org/docs/Web/HTML/Guides/Quirks_mode_and_standards_mode",
            );
        }
        "PartitioningBlobURLIssue" => {
            let info = str_of("partitioningBlobURLInfo");
            reason(&mut raised, info);
            let url = str_of("url");
            raised.message = format!("{url}: {}", words(&kebab(info)));
            raised.about = About::Page;
        }
        "NavigatorUserAgentIssue" => {
            "reads navigator.userAgent, which Chrome reduces; use User-Agent Client Hints"
                .clone_into(&mut raised.message);
            raised.about = source_about(&raised);
            raised.docs =
                Some("https://developer.mozilla.org/docs/Web/API/User-Agent_Client_Hints_API");
        }
        "GenericIssue" => {
            let kind = str_of("errorType");
            let kind = kind.strip_suffix("Error").unwrap_or(kind);
            reason(&mut raised, kind);
            let (severity, text) = generic(kind);
            raised.severity = severity;
            raised.message = match details["violatingNodeAttribute"].as_str() {
                Some(attr) if !attr.is_empty() => format!("{text} (attribute {attr})"),
                _ => text.to_owned(),
            };
            if let Some(node) = details["violatingNodeId"].as_i64() {
                raised.node = Some((raised.target.frame.clone(), node));
                raised.about = About::Element;
            } else if let Some(url) = details["request"]["url"].as_str() {
                let url = url.to_owned();
                url_about(&mut raised, &url);
            }
            if kind.starts_with("Form") {
                raised.docs =
                    Some("https://developer.mozilla.org/docs/Web/HTML/Reference/Elements/label");
                if kind.contains("Autocomplete") {
                    raised.docs = Some(
                        "https://developer.mozilla.org/docs/Web/HTML/Reference/Attributes/autocomplete",
                    );
                }
            }
        }
        "DeprecationIssue" => {
            let kind = str_of("type");
            reason(&mut raised, kind);
            raised.message = deprecation(kind);
            raised.about = source_about(&raised);
            raised.docs = Some("https://chromestatus.com/features");
        }
        "ClientHintIssue" => {
            let why = str_of("clientHintIssueReason");
            reason(&mut raised, why);
            raised.message = format!("a Client Hints meta tag: {}", words(&kebab(why)));
            raised.about = source_about(&raised);
        }
        "FederatedAuthRequestIssue"
        | "FederatedAuthUserInfoRequestIssue"
        | "EmailVerificationRequestIssue" => {
            let key = match raised.code.as_str() {
                "FederatedAuthRequestIssue" => "federatedAuthRequestIssueReason",
                "FederatedAuthUserInfoRequestIssue" => "federatedAuthUserInfoRequestIssueReason",
                _ => "emailVerificationRequestIssueReason",
            };
            let why = str_of(key);
            reason(&mut raised, why);
            raised.message = format!("{}: {}", words(row.rule), words(&kebab(why)));
        }
        "BounceTrackingIssue" => {
            let sites: Vec<&str> = details["trackingSites"]
                .as_array()
                .map(|s| s.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            raised.message = format!(
                "Chrome will clear the storage of bounce-tracking sites: {}",
                sites.join(", ")
            );
        }
        "CookieDeprecationMetadataIssue" => {
            "a third-party cookie is allowed by deprecation metadata"
                .clone_into(&mut raised.message);
        }
        "StylesheetLoadingIssue" => {
            let why = str_of("styleSheetLoadingIssueReason");
            reason(&mut raised, why);
            let failed = &details["failedRequestInfo"];
            raised.message = match failed["url"].as_str() {
                Some(url) if why == "RequestFailed" => {
                    let message = failed["failureMessage"].as_str().unwrap_or_default();
                    url_about(&mut raised, url);
                    format!("stylesheet {url} failed to load: {message}")
                }
                _ => {
                    raised.about = source_about(&raised);
                    "an @import rule after other rules is ignored".to_owned()
                }
            };
        }
        "PropertyRuleIssue" => {
            let why = str_of("propertyRuleIssueReason");
            reason(&mut raised, why);
            let value = details["propertyValue"]
                .as_str()
                .map(|v| format!(" ({v:?})"))
                .unwrap_or_default();
            raised.message = format!("an @property rule: {}{value}", words(&kebab(why)));
            raised.about = source_about(&raised);
        }
        "SharedDictionaryIssue"
        | "SRIMessageSignatureIssue"
        | "UnencodedDigestIssue"
        | "ConnectionAllowlistIssue" => {
            let key = if raised.code == "SharedDictionaryIssue" {
                "sharedDictionaryError"
            } else {
                "error"
            };
            let why = str_of(key);
            reason(&mut raised, why);
            let url = details["request"]["url"].as_str().unwrap_or_default();
            raised.message = format!("{url}: {}: {}", words(row.rule), words(&kebab(why)));
            url_about(&mut raised, url);
        }
        "ElementAccessibilityIssue" => {
            let why = str_of("elementAccessibilityIssueReason");
            reason(&mut raised, why);
            element_reason(why).clone_into(&mut raised.message);
            if details["hasDisallowedAttributes"].as_bool() == Some(true) {
                raised
                    .message
                    .push_str(", with attributes it does not allow");
            }
            if let Some(node) = details["nodeId"].as_i64() {
                raised.node = Some((None, node));
                raised.about = About::Element;
            }
        }
        "UserReidentificationIssue" => {
            let kind = str_of("type");
            reason(&mut raised, kind);
            let url = details["request"]["url"].as_str().map(str::to_owned);
            raised.message = match &url {
                Some(url) => format!("{url}: {}", words(&kebab(kind))),
                None => words(&kebab(kind)),
            };
            match url {
                Some(url) => url_about(&mut raised, &url),
                None => raised.about = source_about(&raised),
            }
        }
        "PermissionElementIssue" => {
            let kind = str_of("issueType");
            reason(&mut raised, kind);
            if details["isWarning"].as_bool() == Some(true) {
                raised.severity = Severity::Info;
            }
            let what = details["type"].as_str().map_or_else(
                || "a <permission> element".to_owned(),
                |t| format!("<permission type={t:?}>"),
            );
            raised.message = format!("{what}: {}", words(&kebab(kind)));
            if let Some(node) = details["nodeId"].as_i64() {
                raised.node = Some((None, node));
                raised.about = About::Element;
            }
        }
        "PerformanceIssue" => {
            let kind = str_of("performanceIssueType");
            reason(&mut raised, kind);
            raised.message = if kind == "DocumentCookie" {
                "reads document.cookie, which is slow; use the Cookie Store API".to_owned()
            } else {
                words(&kebab(kind))
            };
            raised.about = source_about(&raised);
        }
        "SelectivePermissionsInterventionIssue" => {
            raised.message = format!(
                "{} was denied to an ad script",
                details["apiName"].as_str().unwrap_or("an API")
            );
        }
        "LazyLoadImageIssue" => {
            let url = str_of("url");
            raised.message = format!(
                "image {url} is lazy-loaded but in view as the page loads (it may be the LCP \
                 image); drop loading=lazy"
            );
            if let Some(node) = details["nodeId"].as_i64() {
                raised.node = Some((raised.target.frame.clone(), node));
            }
            url_about(&mut raised, url);
            raised.docs = Some("https://web.dev/articles/lcp-lazy-loading");
        }
        _ => {}
    }
    format_cut(&mut raised.message);
    raised
}

/// A code this chrome-cdp does not know (a newer Chrome's): shown, with
/// its raw code.
fn unknown(code: String) -> Raised {
    let shown = if code.is_empty() { "(none)" } else { &code };
    Raised {
        rule: format!(
            "unknown-{}",
            kebab(shown.strip_suffix("Issue").unwrap_or(shown))
        ),
        severity: Severity::Info,
        message: format!("Chrome raised an issue chrome-cdp does not know: {shown}"),
        target: FindingTarget::default(),
        node: None,
        request: None,
        docs: None,
        about: About::Page,
        code,
    }
}

/// The key of a code's details in `InspectorIssueDetails`:
/// `cookieIssueDetails`, `sriMessageSignatureIssueDetails`.
fn details_key(code: &str) -> String {
    let lead = code
        .char_indices()
        .take_while(|(i, c)| {
            c.is_ascii_uppercase()
                && code[i + 1..]
                    .chars()
                    .next()
                    .is_some_and(|n| n.is_ascii_uppercase())
        })
        .count()
        .max(1);
    format!(
        "{}{}Details",
        code[..lead].to_ascii_lowercase(),
        &code[lead..]
    )
}

/// `CamelCase` (or `kInlineViolation`) as kebab-case, acronyms kept as a
/// word: `ResponseWasBlockedByORB` → `response-was-blocked-by-orb`.
pub(crate) fn kebab(text: &str) -> String {
    let text = match text.strip_prefix('k') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_uppercase()) => rest,
        _ => text,
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
            if prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_lower)
            {
                out.push('-');
            }
        }
        if c == '_' || c == ' ' {
            out.push('-');
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// A reason as the end of a rule id: kebab-case, without the words the
/// rule starts with (`MixedContentBlocked` of `mixed-content` →
/// `blocked`) or a trailing `-issue`.
fn sub_rule(rule: &str, reason: &str) -> String {
    let reason = kebab(reason);
    let reason = if rule.is_empty() {
        &reason
    } else {
        reason
            .strip_prefix(rule)
            .and_then(|r| r.strip_prefix('-'))
            .unwrap_or(&reason)
    };
    reason.strip_suffix("-issue").unwrap_or(reason).to_owned()
}

fn words(kebab: &str) -> String {
    kebab.replace('-', " ")
}

fn format_cut(message: &mut String) {
    super::format::cut(message, MESSAGE_LEN);
}

/// Adds the script place in `location` (a `SourceCodeLocation`) to the
/// issue's target.
fn source(raised: &mut Raised, location: &Value) {
    let Some(url) = location["url"].as_str().filter(|u| !u.is_empty()) else {
        return;
    };
    let one_based = |v: &Value| {
        v.as_u64()
            .and_then(|n| u32::try_from(n + 1).ok())
            .unwrap_or(1)
    };
    raised.target.source = Some(SourceLine {
        url: url.to_owned(),
        line: one_based(&location["lineNumber"]),
        column: one_based(&location["columnNumber"]),
    });
}

/// Adds the request in `request` (an `AffectedRequest`) to the issue.
fn request(raised: &mut Raised, request: &Value) {
    if let Some(id) = request["requestId"].as_str() {
        raised.request = Some(id.to_owned());
    }
    if let Some(url) = request["url"].as_str().filter(|u| !u.is_empty()) {
        raised.target.url = Some(url.to_owned());
    }
}

fn frame(raised: &mut Raised, id: &Value) {
    if raised.target.frame.is_none()
        && let Some(id) = id.as_str()
    {
        raised.target.frame = Some(id.to_owned());
    }
}

/// The issue is about `url`.
fn url_about(raised: &mut Raised, url: &str) {
    if url.is_empty() {
        return;
    }
    raised.target.url = Some(url.to_owned());
    raised.about = About::Url(url.to_owned());
}

/// About its script place, if it has one, else the page.
fn source_about(raised: &Raised) -> About {
    raised
        .target
        .source
        .as_ref()
        .map_or(About::Page, |s| About::Source(s.url.clone(), s.line))
}

/// What a cookie's exclusion or warning reason means.
fn cookie_reason(reason: &str, set: bool) -> String {
    let done = if set { "set" } else { "sent" };
    match reason {
        "ExcludeSameSiteUnspecifiedTreatedAsLax" => {
            format!("has no SameSite, so it counts as Lax and was not {done} cross-site")
        }
        "ExcludeSameSiteNoneInsecure" => {
            format!("is SameSite=None without Secure, so it was not {done}")
        }
        "ExcludeSameSiteLax" => format!("is SameSite=Lax, so it was not {done} cross-site"),
        "ExcludeSameSiteStrict" => format!("is SameSite=Strict, so it was not {done} cross-site"),
        "ExcludeThirdPartyPhaseout" | "ExcludeThirdPartyCookieBlockedInFirstPartySet" => {
            format!("is a third-party cookie and was not {done}")
        }
        "ExcludeDomainNonASCII" => format!("has a non-ASCII domain and was not {done}"),
        "ExcludePortMismatch" => format!("was not {done}: the port does not match"),
        "ExcludeSchemeMismatch" => format!("was not {done}: the scheme does not match"),
        "WarnSameSiteUnspecifiedCrossSiteContext" | "WarnSameSiteUnspecifiedLaxAllowUnsafe" => {
            "has no SameSite; it will not be sent cross-site".to_owned()
        }
        "WarnSameSiteNoneInsecure" => {
            "is SameSite=None without Secure; it will be blocked".to_owned()
        }
        "WarnThirdPartyPhaseout" | "WarnThirdPartyCookieHeuristic" => {
            "is a third-party cookie; Chrome is phasing them out".to_owned()
        }
        "WarnAttributeValueExceedsMaxSize" => "has an attribute over 1024 bytes".to_owned(),
        "" => "has a problem".to_owned(),
        other => words(&sub_rule("", other)),
    }
}

/// What a CORS error means.
fn cors_reason(error: &str) -> String {
    match error {
        "MissingAllowOriginHeader" => "no Access-Control-Allow-Origin header".to_owned(),
        "PreflightMissingAllowOriginHeader" => {
            "the preflight has no Access-Control-Allow-Origin header".to_owned()
        }
        "AllowOriginMismatch" | "PreflightAllowOriginMismatch" => {
            "Access-Control-Allow-Origin names another origin".to_owned()
        }
        "WildcardOriginNotAllowed" | "PreflightWildcardOriginNotAllowed" => {
            "Access-Control-Allow-Origin is * on a request with credentials".to_owned()
        }
        "InvalidAllowCredentials" | "PreflightInvalidAllowCredentials" => {
            "Access-Control-Allow-Credentials is not true".to_owned()
        }
        "PreflightInvalidStatus" => "the preflight's status is not 2xx".to_owned(),
        "MethodDisallowedByPreflightResponse" => {
            "the method is not in Access-Control-Allow-Methods".to_owned()
        }
        "HeaderDisallowedByPreflightResponse" => {
            "a header is not in Access-Control-Allow-Headers".to_owned()
        }
        "DisallowedByMode" => "the request's mode does not allow it".to_owned(),
        other => words(&kebab(other)),
    }
}

/// The severity and message of a `GenericIssue` reason (without its
/// `Error` suffix).
fn generic(kind: &str) -> (Severity, &'static str) {
    let warning = |text| (Severity::Warning, text);
    let info = |text| (Severity::Info, text);
    match kind {
        "FormLabelForName" => warning("a <label for> names a field's name, not its id"),
        "FormDuplicateIdForInput" => warning("two form fields have the same id"),
        "FormInputWithNoLabel" => warning("a form field has no label"),
        "FormAutocompleteAttributeEmpty" => warning("a form field's autocomplete is empty"),
        "FormEmptyIdAndNameAttributesForInput" => {
            warning("a form field has neither an id nor a name")
        }
        "FormAriaLabelledByToNonExistingId" => {
            warning("aria-labelledby names an id no element has")
        }
        "FormInputAssignedAutocompleteValueToIdOrNameAttribute" => {
            warning("an autocomplete value is in a field's id or name, not in autocomplete")
        }
        "FormLabelHasNeitherForNorNestedInput" => {
            warning("a <label> has no for and no field inside")
        }
        "FormLabelForMatchesNonExistingId" => warning("a <label for> names an id no element has"),
        "FormInputHasWrongButWellIntendedAutocompleteValue" => {
            warning("a form field's autocomplete value is not a valid token")
        }
        "ResponseWasBlockedByORB" => warning("a response was blocked by Opaque Response Blocking"),
        "NavigationEntryMarkedSkippable" => {
            info("a history entry the page added without a user gesture is skipped by Back")
        }
        "BackUINavigationWouldSkipAd" => info("Back would skip an ad"),
        "AutofillAndManualTextPolicyControlledFeaturesInfo"
        | "AutofillPolicyControlledFeatureInfo"
        | "ManualTextPolicyControlledFeatureInfo" => {
            info("a permissions policy turns autofill or manual text entry off")
        }
        _ if kind.starts_with("FormModelContext") => {
            info("a form's model context (tool) is missing a name, title or description")
        }
        _ => info("Chrome found a problem"),
    }
}

/// What a `DeprecationIssue` type means: the common ones in words, the
/// rest by Chrome's name for them (which searches well).
fn deprecation(kind: &str) -> String {
    let known = match kind {
        "XMLHttpRequestSynchronousInNonWorkerOutsideBeforeUnload" => {
            "a synchronous XMLHttpRequest on the main thread is deprecated"
        }
        "UnloadHandler" => "unload event listeners are deprecated; use pagehide",
        "DocumentDomainSettingWithoutOriginAgentClusterHeader" => {
            "setting document.domain is deprecated"
        }
        "GeolocationInsecureOrigin" | "GeolocationInsecureOriginDeprecatedNotRemoved" => {
            "geolocation on an insecure origin is deprecated"
        }
        "NotificationInsecureOrigin" => "notifications on an insecure origin are deprecated",
        "PrefixedStorageInfo" => "window.webkitStorageInfo is deprecated",
        "RangeExpand" => "Range.expand() is deprecated; use Selection.modify()",
        "MediaSourceAbortRemove" => "SourceBuffer.abort() during remove() is deprecated",
        "LocalCSSFileExtensionRejected" => "a local CSS file without the .css extension is ignored",
        "OverflowVisibleOnReplacedElement" => {
            "overflow: visible on img, video and canvas is changing"
        }
        _ => "",
    };
    if known.is_empty() {
        format!("deprecated: {kind}")
    } else {
        known.to_owned()
    }
}

/// What an `ElementAccessibilityIssue` reason means.
fn element_reason(reason: &str) -> &'static str {
    match reason {
        "DisallowedSelectChild" => "a <select> has a child it does not allow",
        "DisallowedOptGroupChild" => "an <optgroup> has a child it does not allow",
        "NonPhrasingContentOptionChild" => "an <option> has content that is not phrasing content",
        "InteractiveContentOptionChild" => "an <option> has interactive content",
        "InteractiveContentLegendChild" => "a <legend> has interactive content",
        "InteractiveContentSummaryDescendant" => "a <summary> has interactive content",
        _ => "an element has content its role does not allow",
    }
}

/// `url` for a key: no query or fragment, no port, and `self` for the
/// page's origin (`self/api/tax`).
pub(crate) fn key_url(url: &str, page_origin: Option<&str>) -> String {
    let url = url.split(['?', '#']).next().unwrap_or_default();
    if page_origin.is_some() && origin(url) == page_origin {
        return format!("self{}", path_of(url));
    }
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = host.rsplit_once(':').map_or(host, |(h, port)| {
                if port.chars().all(|c| c.is_ascii_digit()) {
                    h
                } else {
                    host
                }
            });
            format!("{scheme}://{host}{path}")
        }
        None => url.to_owned(),
    }
}

/// The part of a key after the rule, for what `about` is.
fn key_where(about: &About, page_url: &str) -> String {
    let page_origin = origin(page_url);
    match about {
        About::Url(url) => key_url(url, page_origin),
        About::Cookie { name, domain } => format!("cookie {name}@{domain}"),
        About::Source(url, line) => format!("{}:{line}", key_url(url, page_origin)),
        About::Page | About::Element => {
            path_of(page_url.split(['?', '#']).next().unwrap_or_default()).to_owned()
        }
    }
}

/// What folds occurrences of `raised` into one: its rule and what it is
/// about (an element by its node).
fn fold_of(raised: &Raised, session: &str, page_url: &str) -> String {
    let place = match (&raised.about, &raised.node) {
        (About::Element, Some((frame, node))) => {
            format!(
                "node {session} {} {node}",
                frame.as_deref().unwrap_or_default()
            )
        }
        (about, _) => key_where(about, page_url),
    };
    format!("{}/{place}", raised.rule)
}

/// An issue as a tab keeps it.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub(crate) issue: Issue,
    /// What folds occurrences into it: its rule and what it is about
    /// (an element by its node).
    fold: String,
    /// The session that raised it: its request and node ids are that
    /// session's.
    pub(crate) session: String,
    pub(crate) node: Option<(Option<String>, i64)>,
    pub(crate) request: Option<String>,
    /// Whether its element was named (see `issue_commands`).
    pub(crate) named: bool,
    /// Whether it is about its element (else about a place, which its
    /// key keeps unless the element has a stable locator).
    pub(crate) by_element: bool,
    /// See [`super::console::ConsoleError::once`].
    pub(crate) once: Option<(String, u64)>,
}

/// The issues of one tab.
#[derive(Debug, Default)]
pub(crate) struct Issues {
    entries: VecDeque<Stored>,
    seq: u64,
    /// The newest issue dropped to make room.
    evicted: u64,
    /// The last issue before the main frame's current document: issues
    /// fold only within a document, and `expect --no-issues` starts here.
    document: u64,
    /// `issues`' read cursor.
    pub(crate) cursor: u64,
}

impl Issues {
    /// The number of the last issue raised (or raised again).
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn evicted(&self) -> u64 {
        self.evicted
    }

    /// The last issue before the main frame's current document.
    pub(crate) fn document(&self) -> u64 {
        self.document
    }

    /// The main frame committed a new document.
    pub(crate) fn navigated(&mut self) {
        self.document = self.seq;
    }

    /// The issues kept, oldest first.
    pub(crate) fn entries(&self) -> impl DoubleEndedIterator<Item = &Stored> {
        self.entries.iter()
    }

    /// Issue `seq`, to name its element.
    pub(crate) fn get_mut(&mut self, seq: u64) -> Option<&mut Stored> {
        self.entries.iter_mut().find(|s| s.issue.seq == seq)
    }

    /// Adds an issue session `session` raised on a page at `page_url`
    /// (of worker `from`, if given), folded into the same one of this
    /// document if there is one, keeping at most `capacity`.
    pub(crate) fn add(
        &mut self,
        raised: Raised,
        session: &str,
        page_url: &str,
        from: Option<FromWorker<'_>>,
        capacity: usize,
    ) {
        let fold = fold_of(&raised, session, page_url);
        let key = format!(
            "issues/{}/{}",
            raised.rule,
            key_where(&raised.about, page_url)
        );
        let document = self.document;
        let found = self
            .entries
            .iter()
            .rposition(|s| s.issue.seq > document && s.fold == fold);
        // Chrome reports an element's issue again each time it looks
        // (form issues, on each parse): the same finding, not another.
        if found.is_some() && raised.about == About::Element {
            return;
        }
        self.seq += 1;
        let seq = self.seq;
        let time_ms = super::console::now_ms();
        if let Some(i) = found
            && let Some(mut again) = self.entries.remove(i)
        {
            again.issue.seq = seq;
            again.issue.time_ms = time_ms;
            again.issue.count = again.issue.count.saturating_add(1);
            again.once = from.and_then(|f| f.once).map(|(id, n)| (id.to_owned(), n));
            self.entries.push_back(again);
            return;
        }
        let mut target = raised.target;
        target.worker = from.map(|f| f.worker.clone());
        let issue = Issue {
            seq,
            time_ms,
            tab: String::new(),
            category: "issues".to_owned(),
            rule: raised.rule,
            severity: raised.severity,
            code: raised.code,
            message: raised.message,
            target,
            key,
            count: 1,
            docs: raised.docs.map(str::to_owned),
        };
        while self.entries.len() >= capacity.max(1) {
            if let Some(dropped) = self.entries.pop_front() {
                self.evicted = self.evicted.max(dropped.issue.seq);
            }
        }
        self.entries.push_back(Stored {
            issue,
            fold,
            session: session.to_owned(),
            node: raised.node,
            request: raised.request,
            named: false,
            by_element: raised.about == About::Element,
            once: from.and_then(|f| f.once).map(|(id, n)| (id.to_owned(), n)),
        });
    }

    /// Adds an issue found by asking (`Audits.checkFormsIssues`), unless
    /// this document has it already: asking again is no new occurrence.
    pub(crate) fn add_current(
        &mut self,
        raised: Raised,
        session: &str,
        page_url: &str,
        capacity: usize,
    ) {
        let fold = fold_of(&raised, session, page_url);
        let document = self.document;
        if !self
            .entries
            .iter()
            .any(|s| s.issue.seq > document && s.fold == fold)
        {
            self.add(raised, session, page_url, None, capacity);
        }
    }

    /// The issues of severity `min` or more raised after mark `after`,
    /// oldest first.
    pub(crate) fn since(&self, after: u64, min: Severity) -> Vec<Stored> {
        self.entries
            .iter()
            .filter(|s| s.issue.seq > after && s.issue.severity <= min)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::{Severity, Worker, WorkerKind};
    use serde_json::{Value, json};

    use super::{CODES, FromWorker, Issues, details_key, kebab, key_url, parse};

    fn issue(code: &str, details: &Value) -> Value {
        let key = details_key(code);
        json!({ "issue": { "code": code, "details": { key: details } } })
    }

    #[test]
    fn every_code_of_the_protocol_has_a_row() {
        let protocol: Value =
            serde_json::from_str(include_str!("../../../cdp/protocol/browser_protocol.json"))
                .unwrap();
        let audits = protocol["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["domain"] == "Audits")
            .unwrap();
        let types = audits["types"].as_array().unwrap();
        let codes = types
            .iter()
            .find(|t| t["id"] == "InspectorIssueCode")
            .unwrap()["enum"]
            .as_array()
            .unwrap();
        let details = types
            .iter()
            .find(|t| t["id"] == "InspectorIssueDetails")
            .unwrap()["properties"]
            .as_array()
            .unwrap();
        for code in codes {
            let code = code.as_str().unwrap();
            assert!(
                CODES.iter().any(|c| c.name == code),
                "{code} has no row in CODES"
            );
            // Its details are where `details_key` looks.
            assert!(
                details.iter().any(|d| d["name"] == details_key(code)),
                "{code}: no {}",
                details_key(code)
            );
        }
        assert_eq!(
            CODES.len(),
            codes.len(),
            "CODES has a code Chrome no longer has"
        );
    }

    #[test]
    fn rule_ids() {
        assert_eq!(
            kebab("ResponseWasBlockedByORB"),
            "response-was-blocked-by-orb"
        );
        assert_eq!(kebab("kInlineViolation"), "inline-violation");
        assert_eq!(
            kebab("SRIMessageSignatureIssue"),
            "sri-message-signature-issue"
        );
        assert_eq!(
            details_key("SRIMessageSignatureIssue"),
            "sriMessageSignatureIssueDetails"
        );
        assert_eq!(details_key("CorsIssue"), "corsIssueDetails");

        let cookie = parse(&issue(
            "CookieIssue",
            &json!({
                "cookie": { "name": "sid", "domain": "example.com", "path": "/" },
                "cookieWarningReasons": [],
                "cookieExclusionReasons": ["ExcludeSameSiteUnspecifiedTreatedAsLax"],
                "operation": "ReadCookie",
                "request": { "requestId": "R1", "url": "http://example.com/api?x=1" },
            }),
        ));
        assert_eq!(
            cookie.rule,
            "cookie-exclude-same-site-unspecified-treated-as-lax"
        );
        assert_eq!(cookie.severity, Severity::Warning);
        assert_eq!(
            cookie.message,
            "cookie \"sid\" (example.com) has no SameSite, so it counts as Lax and was not sent cross-site"
        );
        assert_eq!(cookie.request.as_deref(), Some("R1"));

        let mixed = |status: &str| {
            parse(&issue(
                "MixedContentIssue",
                &json!({
                    "resourceType": "Script",
                    "resolutionStatus": status,
                    "insecureURL": "http://cdn.test/app.js",
                    "mainResourceURL": "https://shop.test/",
                }),
            ))
        };
        let blocked = mixed("MixedContentBlocked");
        assert_eq!(blocked.rule, "mixed-content-blocked");
        assert_eq!(blocked.severity, Severity::Error);
        assert_eq!(
            blocked.message,
            "script http://cdn.test/app.js blocked on an https: page"
        );
        let upgraded = mixed("MixedContentAutomaticallyUpgraded");
        assert_eq!(upgraded.rule, "mixed-content-automatically-upgraded");
        assert_eq!(upgraded.severity, Severity::Warning);

        let csp = parse(&issue(
            "ContentSecurityPolicyIssue",
            &json!({
                "violatedDirective": "script-src-elem",
                "isReportOnly": false,
                "contentSecurityPolicyViolationType": "kInlineViolation",
                "sourceCodeLocation": { "url": "http://x.test/csp.html", "lineNumber": 4, "columnNumber": 0 },
                "violatingNodeId": 12,
            }),
        ));
        assert_eq!(csp.rule, "content-security-policy-inline-violation");
        assert_eq!(csp.severity, Severity::Error);
        assert_eq!(
            csp.message,
            "an inline script or style blocked by the Content-Security-Policy's script-src-elem"
        );
        assert_eq!(csp.target.source.as_ref().map(|s| s.line), Some(5));
        assert_eq!(csp.node, Some((None, 12)));

        let form = parse(&issue(
            "GenericIssue",
            &json!({
                "errorType": "FormInputHasWrongButWellIntendedAutocompleteValueError",
                "frameId": "F1",
                "violatingNodeId": 7,
                "violatingNodeAttribute": "autocomplete",
            }),
        ));
        assert_eq!(
            form.rule,
            "form-input-has-wrong-but-well-intended-autocomplete-value"
        );
        assert_eq!(form.node, Some((Some("F1".into()), 7)));
        assert!(form.message.ends_with("(attribute autocomplete)"));

        let cors = parse(&issue(
            "CorsIssue",
            &json!({
                "corsErrorStatus": { "corsError": "MissingAllowOriginHeader", "failedParameter": "" },
                "isWarning": false,
                "request": { "requestId": "R2", "url": "http://api.test:9000/tax" },
            }),
        ));
        assert_eq!(cors.rule, "cors-missing-allow-origin-header");
        assert_eq!(cors.severity, Severity::Error);

        let quirks = parse(&issue(
            "QuirksModeIssue",
            &json!({ "isLimitedQuirksMode": false, "documentNodeId": 1, "url": "http://x.test/q.html", "frameId": "F", "loaderId": "L" }),
        ));
        assert_eq!(
            (quirks.rule.as_str(), quirks.severity),
            ("quirks-mode", Severity::Info)
        );

        let deprecation = parse(&issue(
            "DeprecationIssue",
            &json!({ "type": "UnloadHandler", "sourceCodeLocation": { "url": "http://x.test/app.js", "lineNumber": 11, "columnNumber": 2 } }),
        ));
        assert_eq!(deprecation.rule, "deprecation-unload-handler");
        assert_eq!(deprecation.message, "unload handler is deprecated");

        let unknown = parse(&json!({ "issue": { "code": "BrandNewIssue", "details": {} } }));
        assert_eq!(unknown.rule, "unknown-brand-new");
        assert_eq!(unknown.severity, Severity::Info);
        assert!(unknown.message.contains("BrandNewIssue"));
    }

    #[test]
    fn keys_leave_out_queries_ports_and_the_page_origin() {
        let page = Some("http://127.0.0.1:4100");
        assert_eq!(
            key_url("http://127.0.0.1:4100/api/tax?q=1", page),
            "self/api/tax"
        );
        assert_eq!(
            key_url("http://127.0.0.1:4200/x.js#a", page),
            "http://127.0.0.1/x.js"
        );
        assert_eq!(
            key_url("https://cdn.test/app.js", None),
            "https://cdn.test/app.js"
        );
    }

    #[test]
    fn repeats_fold_within_a_document() {
        let mut issues = Issues::default();
        let cookie = issue(
            "CookieIssue",
            &json!({
                "cookie": { "name": "sid", "domain": "x.test", "path": "/" },
                "cookieWarningReasons": ["WarnSameSiteUnspecifiedCrossSiteContext"],
                "cookieExclusionReasons": [],
                "operation": "SetCookie",
            }),
        );
        let page = "http://x.test:4100/shop?a=1";
        for _ in 0..50 {
            issues.add(parse(&cookie), "S1", page, None, 1000);
        }
        let all = issues.since(0, Severity::Info);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].issue.count, 50);
        assert_eq!(all[0].issue.seq, 50);
        assert_eq!(
            all[0].issue.key,
            "issues/cookie-warn-same-site-unspecified-cross-site-context/cookie sid@x.test"
        );
        // A new document starts over.
        issues.navigated();
        issues.add(parse(&cookie), "S1", page, None, 1000);
        let all = issues.since(0, Severity::Info);
        assert_eq!(all.len(), 2);
        assert_eq!(
            issues.since(issues.document(), Severity::Info)[0]
                .issue
                .count,
            1
        );
        // Severity filters.
        assert!(issues.since(0, Severity::Error).is_empty());
        // Asking again finds what is there: no new occurrence.
        issues.add_current(parse(&cookie), "S1", page, 1000);
        assert_eq!(
            issues.since(issues.document(), Severity::Info)[0]
                .issue
                .count,
            1
        );
        assert_eq!(issues.seq(), 52);
    }

    #[test]
    fn workers_and_the_bound() {
        let mut issues = Issues::default();
        let worker = Worker {
            kind: WorkerKind::Dedicated,
            url: "http://x.test/w.js".into(),
        };
        for n in 0..5 {
            let deprecation = issue(
                "DeprecationIssue",
                &json!({ "type": "X", "sourceCodeLocation": { "url": "http://x.test/w.js", "lineNumber": n, "columnNumber": 0 } }),
            );
            let from = FromWorker {
                worker: &worker,
                once: None,
            };
            issues.add(parse(&deprecation), "W", "http://x.test/", Some(from), 3);
        }
        let all = issues.since(0, Severity::Info);
        assert_eq!(all.len(), 3);
        assert_eq!(issues.evicted(), 2);
        assert_eq!(all[0].issue.target.worker.as_ref(), Some(&worker));
        assert_eq!(all[0].issue.key, "issues/deprecation-x/self/w.js:3");
    }
}

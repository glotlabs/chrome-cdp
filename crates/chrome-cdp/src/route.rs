//! Turning `route` and `block` arguments into routing rules (plan 07,
//! milestone 7.1). Everything that needs no daemon is checked here: one
//! action per rule, headers as `Name: value`, JSON that parses. The
//! engine checks the rest (the pattern, header names, statuses) when the
//! rule is added.

use std::path::PathBuf;

use engine::{RouteAction, RouteBody, RouteSpec};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};
use serde_json::Value;

use crate::cli::{BlockArgs, RouteArgs};
use crate::input;

/// The ad and tracking hosts `block --ads` blocks, with their subdomains.
const AD_HOSTS: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "google-analytics.com",
    "googletagmanager.com",
    "googletagservices.com",
    "adservice.google.com",
    "amazon-adsystem.com",
    "adnxs.com",
    "adsrvr.org",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "pubmatic.com",
    "rubiconproject.com",
    "openx.net",
    "moatads.com",
    "scorecardresearch.com",
    "quantserve.com",
    "hotjar.com",
    "clarity.ms",
    "connect.facebook.net",
    "ads-twitter.com",
    "bat.bing.com",
];

/// The rule `route` adds.
pub(crate) fn spec(args: &RouteArgs) -> Result<RouteSpec, ErrorInfo> {
    let fulfil = args.status.is_some()
        || args.body.is_some()
        || args.json_body.is_some()
        || args.body_file.is_some()
        || !args.headers.is_empty();
    let change = !args.set_headers.is_empty()
        || !args.remove_headers.is_empty()
        || args.rewrite_url.is_some();
    let actions: Vec<&str> = [
        (
            fulfil,
            "answer (--json-body, --body, --body-file, --status, --header)",
        ),
        (args.abort.is_some(), "--abort"),
        (
            change,
            "change (--set-header, --remove-header, --rewrite-url)",
        ),
        (args.patch_json.is_some(), "--patch-json"),
    ]
    .into_iter()
    .filter_map(|(given, name)| given.then_some(name))
    .collect();
    if actions.len() > 1 {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("a rule has one action, not {}", actions.join(" and ")),
        )
        .hint(Hint::text(
            "add one rule per action; the first rule that matches a request handles it",
        )));
    }
    let action = if fulfil {
        RouteAction::Fulfill {
            status: args.status.unwrap_or(200),
            headers: args
                .headers
                .iter()
                .map(|h| header(h, "--header"))
                .collect::<Result<_, _>>()?,
            body: body(args)?,
        }
    } else if let Some(reason) = &args.abort {
        RouteAction::Abort {
            reason: reason.clone(),
        }
    } else if let Some(patch) = &args.patch_json {
        RouteAction::PatchJson {
            patch: json(patch, "--patch-json")?,
        }
    } else if change || args.delay.is_some() {
        RouteAction::Continue {
            set_headers: args
                .set_headers
                .iter()
                .map(|h| header(h, "--set-header"))
                .collect::<Result<_, _>>()?,
            remove_headers: args.remove_headers.clone(),
            url: args.rewrite_url.clone(),
        }
    } else {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("no action for `{}`", args.pattern),
        )
        .hint(
            Hint::run(Cmd::new("route").arg(&args.pattern).flag("json-body", "{}"))
                .why("answers with JSON")
                .or("--body, --status, --abort, --delay, --set-header or --patch-json (see chrome-cdp route --help)"),
        ));
    };
    Ok(RouteSpec {
        pattern: args.pattern.clone(),
        label: None,
        methods: args.method.clone(),
        resource_types: args.resource_type.clone(),
        times: args.times,
        delay_ms: args
            .delay
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        context: None,
        action,
    })
}

/// The rules `block` adds: one per pattern, then one per preset.
pub(crate) fn block_specs(args: &BlockArgs) -> Result<Vec<RouteSpec>, ErrorInfo> {
    let blocked = || RouteAction::Abort {
        reason: "blockedbyclient".into(),
    };
    let rule = |pattern: String, label: Option<&str>, types: &[&str]| RouteSpec {
        pattern,
        label: label.map(str::to_owned),
        methods: Vec::new(),
        resource_types: types.iter().map(|&t| t.to_owned()).collect(),
        times: None,
        delay_ms: 0,
        context: None,
        action: blocked(),
    };
    let mut rules: Vec<RouteSpec> = args
        .patterns
        .iter()
        .map(|p| rule(p.clone(), None, &[]))
        .collect();
    for (on, label, kind) in [
        (args.images, "images", "image"),
        (args.fonts, "fonts", "font"),
        (args.media, "media", "media"),
    ] {
        if on {
            rules.push(rule("**".into(), Some(label), &[kind]));
        }
    }
    if args.ads {
        rules.push(rule(ads_pattern(), Some("ads"), &[]));
    }
    if rules.is_empty() {
        return Err(ErrorInfo::new(ErrorKind::BadArgs, "nothing to block").hint(
            Hint::run(Cmd::new("block").switch("images").switch("fonts"))
                .or("or give URL patterns: chrome-cdp block '**/*.mp4'"),
        ));
    }
    Ok(rules)
}

/// A regular expression for URLs on [`AD_HOSTS`] and their subdomains.
fn ads_pattern() -> String {
    let hosts: Vec<String> = AD_HOSTS.iter().map(|h| h.replace('.', "\\.")).collect();
    format!(
        "/^https?://([^/?#]*\\.)?({})(:[0-9]+)?([/?#]|$)/",
        hosts.join("|")
    )
}

/// `Name: value` (or `Name:value`).
pub(crate) fn header(text: &str, flag: &str) -> Result<(String, String), ErrorInfo> {
    match text.split_once(':') {
        Some((name, value)) if !name.trim().is_empty() => {
            Ok((name.trim().to_owned(), value.trim().to_owned()))
        }
        _ => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{flag} `{text}` is not a header; give it as 'Name: value'"),
        )
        .hint(Hint::text(format!("e.g. {flag} 'X-Mock: 1'")))),
    }
}

fn body(args: &RouteArgs) -> Result<RouteBody, ErrorInfo> {
    if let Some(text) = &args.body {
        return Ok(RouteBody::Text {
            text: input::text_arg(text, "--body")?,
        });
    }
    if let Some(text) = &args.json_body {
        return Ok(RouteBody::Json {
            value: json(text, "--json-body")?,
        });
    }
    if let Some(path) = &args.body_file {
        let path = PathBuf::from(input::absolute_path(path)?);
        if !path.is_file() {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--body-file {} is not a file", path.display()),
            ));
        }
        return Ok(RouteBody::File { path });
    }
    Ok(RouteBody::Empty)
}

/// Parses a JSON argument (`-` reads stdin).
pub(crate) fn json(arg: &str, flag: &str) -> Result<Value, ErrorInfo> {
    let text = input::text_arg(arg, flag)?;
    serde_json::from_str(&text).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{flag} is not valid JSON: {e}"),
        )
        .hint(Hint::text(
            "quote it for the shell, e.g. --json-body '{\"items\": []}', or pass - and send it on stdin",
        ))
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use serde_json::json;

    use super::*;
    use crate::cli::{Cli, Command};

    fn parse(args: &[&str]) -> Result<RouteSpec, ErrorInfo> {
        let cli = Cli::try_parse_from(["chrome-cdp", "route"].iter().chain(args)).unwrap();
        let Some(Command::Route(route)) = cli.command else {
            panic!("not route")
        };
        spec(&route)
    }

    #[test]
    fn actions() {
        let rule = parse(&[
            "/api",
            "--json-body",
            "[1]",
            "--status",
            "201",
            "--header",
            "X-A: 1",
        ])
        .unwrap();
        assert_eq!(
            rule.action,
            RouteAction::Fulfill {
                status: 201,
                headers: vec![("X-A".into(), "1".into())],
                body: RouteBody::Json { value: json!([1]) },
            }
        );
        let rule = parse(&["/api", "--abort"]).unwrap();
        assert_eq!(
            rule.action,
            RouteAction::Abort {
                reason: "failed".into()
            }
        );
        let rule = parse(&["/api", "--abort", "timedout", "--times", "2"]).unwrap();
        assert_eq!(rule.times, Some(2));
        let rule = parse(&["/api", "--delay", "2s"]).unwrap();
        assert_eq!(rule.delay_ms, 2000);
        assert!(matches!(rule.action, RouteAction::Continue { .. }));
        let rule = parse(&[
            "/api",
            "--patch-json",
            "{\"a\": null}",
            "--method",
            "get,post",
        ])
        .unwrap();
        assert_eq!(rule.methods, ["get", "post"]);
        assert_eq!(
            rule.action,
            RouteAction::PatchJson {
                patch: json!({"a": null})
            }
        );
    }

    #[test]
    fn mistakes() {
        let err = parse(&["/api"]).unwrap_err();
        assert_eq!(err.message, "no action for `/api`");
        let err = parse(&["/api", "--abort", "--json-body", "{}"]).unwrap_err();
        assert!(
            err.message.starts_with("a rule has one action, not answer"),
            "{}",
            err.message
        );
        let err = parse(&["/api", "--json-body", "{nope"]).unwrap_err();
        assert!(
            err.message.starts_with("--json-body is not valid JSON"),
            "{}",
            err.message
        );
        let err = parse(&["/api", "--set-header", "nocolon"]).unwrap_err();
        assert!(err.message.contains("is not a header"), "{}", err.message);
    }

    #[test]
    fn the_ads_pattern_matches_hosts_and_subdomains() {
        let pattern = ads_pattern();
        let re = regex::Regex::new(pattern.trim_matches('/')).unwrap();
        assert!(re.is_match("https://www.googletagmanager.com/gtm.js?id=1"));
        assert!(re.is_match("https://securepubads.g.doubleclick.net/tag"));
        assert!(re.is_match("http://doubleclick.net"));
        assert!(!re.is_match("https://notdoubleclick.net/"));
        assert!(!re.is_match("https://example.com/doubleclick.net/"));
    }
}

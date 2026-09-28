//! `expect` (plan 09, milestone 9.1): its arguments as the engine's
//! [`Expectation`], checked before any daemon starts.

use std::time::Duration;

use engine::{A11yLevel, Expectation};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use crate::cli::{A11yLevelArg, ExpectArgs};
use crate::input;

/// How long `expect` retries unless `--timeout` says otherwise
/// (Playwright's default too).
pub(crate) const EXPECT_TIMEOUT: Duration = Duration::from_secs(5);

fn bad_args(message: impl Into<String>, hint: Hint) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message).hint(hint)
}

/// What `args` asserts.
pub(crate) fn expectation(args: &ExpectArgs) -> Result<Expectation, ErrorInfo> {
    let page_check = [
        (args.url.is_some(), "--url"),
        (args.title.is_some(), "--title"),
        (args.no_errors, "--no-errors"),
        (args.no_failed_requests, "--no-failed-requests"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));
    if let Some(flag) = page_check
        && let Some(target) = &args.target
    {
        return Err(bad_args(
            format!("{flag} checks the page; it takes no LOCATOR, got {target:?}"),
            Hint::run(Cmd::new("expect").arg(target).raw("visible"))
                .why("to check the element")
                .or(format!("drop {target:?} to check the page")),
        ));
    }
    if args.since.is_some() && !(args.no_errors || args.no_failed_requests) {
        return Err(bad_args(
            "--since is for --no-errors and --no-failed-requests",
            Hint::run(
                Cmd::new("expect")
                    .switch("no-errors")
                    .flag("since", "before-save"),
            ),
        ));
    }
    if args.exact && args.matcher.is_none() && args.title.is_none() {
        return Err(bad_args(
            "--exact is for the text matcher and --title",
            Hint::run(
                Cmd::new("expect")
                    .flag("title", "Dashboard")
                    .switch("exact"),
            ),
        ));
    }
    if (args.min.is_some() || args.max.is_some()) && args.matcher.is_none() {
        return Err(bad_args(
            "--min and --max are for the count matcher",
            Hint::run(Cmd::new("expect").arg(".row").raw("count").flag("min", "1")),
        ));
    }
    if let Some(pattern) = &args.url {
        return Ok(Expectation::Url {
            pattern: pattern.clone(),
        });
    }
    if let Some(title) = &args.title {
        return Ok(Expectation::Title {
            text: input::text_arg(title, "--title")?,
            exact: args.exact,
        });
    }
    if args.no_errors {
        return Ok(Expectation::NoErrors {
            since: args.since.clone(),
        });
    }
    if args.no_failed_requests {
        return Ok(Expectation::NoFailedRequests {
            since: args.since.clone(),
            origin: args.origin.clone(),
        });
    }
    if args.a11y {
        return Ok(Expectation::A11y {
            scope: args.target.clone(),
            level: match args.level {
                Some(A11yLevelArg::A) => A11yLevel::A,
                Some(A11yLevelArg::Aa) | None => A11yLevel::AA,
            },
        });
    }
    let Some(locator) = &args.target else {
        return Err(bad_args(
            "expect needs a LOCATOR and a MATCHER, or a page check",
            Hint::run(Cmd::new("expect").arg("#main").raw("visible"))
                .or("--url, --title, --no-errors, --no-failed-requests or --a11y"),
        ));
    };
    let Some(name) = &args.matcher else {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("expect {locator} needs a matcher"),
        )
        .note("matchers", engine::MATCHERS.join(", "))
        .hint(Hint::run(Cmd::new("expect").arg(locator).raw("visible"))));
    };
    let mut values = Vec::with_capacity(args.values.len());
    for value in &args.values {
        values.push(input::text_arg(value, "VALUE")?);
    }
    let matcher = engine::parse_matcher(locator, name, &values, args.exact, args.min, args.max)?;
    Ok(Expectation::Element {
        locator: locator.clone(),
        matcher,
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use engine::{Expectation, Matcher};
    use protocol::ErrorKind;

    use super::expectation;
    use crate::cli::{Cli, Command};

    fn parse(args: &[&str]) -> Result<Expectation, protocol::ErrorInfo> {
        let cli = Cli::try_parse_from(["chrome-cdp", "expect"].iter().chain(args))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        let Some(Command::Expect(args)) = cli.command else {
            panic!("not expect");
        };
        expectation(&args)
    }

    #[test]
    fn matchers_and_their_values() {
        assert_eq!(
            parse(&["role=alert", "text", "Saved", "--exact"]).unwrap(),
            Expectation::Element {
                locator: "role=alert".into(),
                matcher: Matcher::Text {
                    text: "Saved".into(),
                    exact: true
                },
            }
        );
        assert_eq!(
            parse(&[".row", "count", "--min", "2"]).unwrap(),
            Expectation::Element {
                locator: ".row".into(),
                matcher: Matcher::Count {
                    count: None,
                    min: Some(2),
                    max: None
                },
            }
        );
        assert_eq!(
            parse(&["#a", "css", "color", "red", "--not"]).unwrap(),
            Expectation::Element {
                locator: "#a".into(),
                matcher: Matcher::Css {
                    property: "color".into(),
                    value: "red".into()
                },
            }
        );
        assert!(matches!(
            parse(&["--a11y", "--level", "a"]).unwrap(),
            Expectation::A11y {
                scope: None,
                level: engine::A11yLevel::A
            }
        ));
    }

    #[test]
    fn usage_errors_name_the_fix() {
        let cases: &[(&[&str], &str)] = &[
            (
                &["e1", "visble"],
                "chrome-cdp expect e1 visible   (did you mean this?)",
            ),
            (&["e1"], "chrome-cdp expect e1 visible"),
            (&["e1", "text"], "chrome-cdp expect e1 text Saved"),
            (&["e1", "visible", "yes"], "chrome-cdp expect e1 visible"),
            (&["e1", "count", "three"], "chrome-cdp expect e1 count 3"),
            (
                &["e1", "visible", "--exact"],
                "chrome-cdp expect e1 text Saved --exact",
            ),
            (
                &["--no-errors", "--exact"],
                "chrome-cdp expect --title Dashboard --exact",
            ),
            (
                &["e1", "--url", "/x"],
                "chrome-cdp expect e1 visible   (to check the element), or drop \"e1\" to check the page",
            ),
        ];
        for (args, hint) in cases {
            let err = parse(args).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{args:?}");
            assert_eq!(err.hint.as_deref(), Some(*hint), "{args:?}");
        }
    }

    #[test]
    fn clap_rejects_mixed_checks() {
        for args in [
            &["--url", "/a", "--title", "A"][..],
            &["e1", "visible", "--a11y"],
            &["--level", "A"],
            &["--origin", "self"],
            &[],
        ] {
            assert!(
                Cli::try_parse_from(["chrome-cdp", "expect"].iter().chain(args)).is_err(),
                "{args:?}"
            );
        }
    }
}

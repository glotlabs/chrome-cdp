//! `expect` (plan 09, milestone 9.1): its arguments as the engine's
//! [`Expectation`], checked before any daemon starts; and where
//! `expect-snapshot` and `expect-screenshot` (milestone 9.2) keep their
//! baselines.

use std::path::{Path, PathBuf};
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

/// The baselines' directory in a project: `.chrome-cdp/<kind>` of the
/// nearest directory up from `cwd` with `.chrome-cdp.toml`, `.chrome-cdp/`
/// or `.git`, else of `cwd`.
fn project_baselines(cwd: &Path, kind: &str) -> PathBuf {
    let root = cwd
        .ancestors()
        .find(|dir| {
            dir.join(crate::config::PROJECT_FILE).is_file()
                || dir.join(".chrome-cdp").is_dir()
                || dir.join(".git").exists()
        })
        .unwrap_or(cwd);
    root.join(".chrome-cdp").join(kind)
}

/// The baseline file of `name`: in `dir` (`--dir`) or the project's
/// `.chrome-cdp/<kind>`, named `<name><suffix>` (a trailing `extension`
/// typed with the name is not doubled).
fn baseline_path(
    dir: Option<&Path>,
    kind: &str,
    name: &str,
    extension: &str,
    suffix: &str,
) -> Result<PathBuf, ErrorInfo> {
    let dir = if let Some(dir) = dir {
        PathBuf::from(input::absolute_path(dir)?)
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot read the current directory: {e}"),
            )
        })?;
        project_baselines(&cwd, kind)
    };
    let stem = name.strip_suffix(extension).unwrap_or(name);
    Ok(dir.join(format!("{stem}{suffix}")))
}

/// `expect-snapshot`'s baseline: `<name>.txt`.
pub(crate) fn snapshot_baseline(dir: Option<&Path>, name: &str) -> Result<PathBuf, ErrorInfo> {
    baseline_path(dir, "snapshots", name, ".txt", ".txt")
}

/// `expect-screenshot`'s baseline: `<name>-<os>.png`, one per platform,
/// since fonts render differently on each.
pub(crate) fn screenshot_baseline(dir: Option<&Path>, name: &str) -> Result<PathBuf, ErrorInfo> {
    let suffix = format!("-{}.png", std::env::consts::OS);
    baseline_path(dir, "screenshots", name, ".png", &suffix)
}

/// Whether a missing baseline is stored (the first run) rather than a
/// failure: not in CI, where it would pass without checking anything.
pub(crate) fn create_missing() -> bool {
    !std::env::var("CI").is_ok_and(|v| !v.is_empty() && v != "0" && v != "false")
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use engine::{Expectation, Matcher};
    use protocol::ErrorKind;

    use super::{expectation, project_baselines, screenshot_baseline, snapshot_baseline};
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

    #[test]
    fn baselines_live_in_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("app/src/.git")).unwrap();
        std::fs::create_dir_all(root.join("app/src/deep")).unwrap();
        // Nothing marks a project: the current directory.
        assert_eq!(
            project_baselines(&root.join("app"), "snapshots"),
            root.join("app/.chrome-cdp/snapshots")
        );
        // The nearest of .git, .chrome-cdp.toml and .chrome-cdp/.
        assert_eq!(
            project_baselines(&root.join("app/src/deep"), "snapshots"),
            root.join("app/src/.chrome-cdp/snapshots")
        );
        std::fs::write(root.join("app/src/deep/.chrome-cdp.toml"), "").unwrap();
        assert_eq!(
            project_baselines(&root.join("app/src/deep"), "screenshots"),
            root.join("app/src/deep/.chrome-cdp/screenshots")
        );
    }

    #[test]
    fn baseline_files_are_named_after_the_baseline() {
        let dir = std::path::Path::new("/b");
        assert_eq!(
            snapshot_baseline(Some(dir), "checkout/cart").unwrap(),
            dir.join("checkout/cart.txt")
        );
        assert_eq!(
            snapshot_baseline(Some(dir), "login.txt").unwrap(),
            dir.join("login.txt")
        );
        let os = std::env::consts::OS;
        assert_eq!(
            screenshot_baseline(Some(dir), "home.png").unwrap(),
            dir.join(format!("home-{os}.png"))
        );
    }
}

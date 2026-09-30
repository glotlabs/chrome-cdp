//! `expect` (plan 09, milestone 9.1): its arguments as the engine's
//! [`Expectation`], checked before any daemon starts; and where
//! `expect-snapshot` and `expect-screenshot` (milestone 9.2) keep their
//! baselines.

use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::{A11yLevel, Expectation, MockStateCheck, RequestCheck};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use crate::cli::{A11yLevelArg, ExpectArgs, PerfMissingArg, SeverityArg};
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
        (args.no_issues, "--no-issues"),
        (args.no_failed_requests, "--no-failed-requests"),
        (args.perf.is_some(), "--perf"),
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
    if matches!(args.target.as_deref(), Some("request" | "no-request")) {
        return request(args);
    }
    let request_flag = [
        (args.count.is_some(), "--count"),
        (!args.queries.is_empty(), "--query"),
        (!args.headers.is_empty(), "--header"),
        (args.json_body.is_some(), "--json-body"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));
    if let Some(flag) = request_flag {
        return Err(bad_args(
            format!("{flag} is for expect request"),
            Hint::run(
                Cmd::new("expect")
                    .raw("request")
                    .arg("POST")
                    .arg("/api/orders")
                    .flag("count", "1"),
            ),
        ));
    }
    if args.since.is_some() && !(args.no_errors || args.no_issues || args.no_failed_requests) {
        return Err(bad_args(
            "--since is for --no-errors, --no-issues and --no-failed-requests",
            Hint::run(
                Cmd::new("expect")
                    .switch("no-errors")
                    .flag("since", "before-save"),
            ),
        ));
    }
    if args.all_tabs && !(args.no_errors || args.no_issues || args.no_failed_requests) {
        return Err(bad_args(
            "--all-tabs is for --no-errors, --no-issues and --no-failed-requests",
            Hint::run(
                Cmd::new("expect")
                    .switch("no-failed-requests")
                    .switch("all-tabs"),
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
            all_tabs: args.all_tabs,
        });
    }
    if args.no_issues {
        return Ok(Expectation::NoIssues {
            since: args.since.clone(),
            all_tabs: args.all_tabs,
            min_severity: args.min_severity.map_or(
                protocol::schema::Severity::Warning,
                SeverityArg::to_severity,
            ),
        });
    }
    if args.no_failed_requests {
        return Ok(Expectation::NoFailedRequests {
            since: args.since.clone(),
            origin: args.origin.clone(),
            all_tabs: args.all_tabs,
        });
    }
    if let Some(budgets) = &args.perf {
        engine::check_perf_budgets(budgets)?;
        return Ok(Expectation::Perf {
            budgets: budgets.clone(),
            missing_ok: args.perf_missing == Some(PerfMissingArg::Ok),
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
    if args.target.as_deref() == Some("mock-state") {
        return mock_state(args);
    }
    let Some(locator) = &args.target else {
        return Err(bad_args(
            "expect needs a LOCATOR and a MATCHER, or a page check",
            Hint::run(Cmd::new("expect").arg("#main").raw("visible")).or(
                "--url, --title, --no-errors, --no-issues, --no-failed-requests, --a11y or --perf",
            ),
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

/// `expect request <METHOD> <PATH>` and `expect no-request <METHOD>
/// <PATH>`.
fn request(args: &ExpectArgs) -> Result<Expectation, ErrorInfo> {
    let none = args.target.as_deref() == Some("no-request");
    let word = if none { "no-request" } else { "request" };
    let example = Cmd::new("expect").raw(word).arg("POST").arg("/api/orders");
    let (Some(method), [target]) = (&args.matcher, args.values.as_slice()) else {
        return Err(bad_args(
            format!("expect {word} needs a METHOD and a PATH (a glob), and nothing more"),
            Hint::run(example),
        ));
    };
    if args.exact {
        return Err(bad_args(
            format!("--exact is not for expect {word}"),
            Hint::run(example),
        ));
    }
    let queries = args
        .queries
        .iter()
        .map(|q| match q.split_once('=') {
            Some((name, value)) if !name.is_empty() => Ok((name.to_owned(), value.to_owned())),
            _ => Err(bad_args(
                format!("--query `{q}` is not name=value"),
                Hint::run(
                    Cmd::new("expect")
                        .raw(word)
                        .arg("GET")
                        .arg("/api/search")
                        .flag("query", "q=keyboard"),
                ),
            )),
        })
        .collect::<Result<_, _>>()?;
    let headers = args
        .headers
        .iter()
        .map(|h| crate::route::header(h, "--header"))
        .collect::<Result<_, _>>()?;
    let json = args
        .json_body
        .as_deref()
        .map(|text| crate::route::json(text, "--json-body"))
        .transpose()?;
    Ok(Expectation::Request(RequestCheck {
        method: method.clone(),
        target: target.clone(),
        query: queries,
        headers,
        json,
        count: args.count,
        min: args.min,
        max: args.max,
        since: args.since.clone(),
        none,
    }))
}

/// `expect mock-state <name> count|value|contains [value]`.
fn mock_state(args: &ExpectArgs) -> Result<Expectation, ErrorInfo> {
    let example = |rest: &str| Cmd::new("expect").arg("mock-state").raw(rest);
    let Some(name) = args.matcher.clone() else {
        return Err(bad_args(
            "expect mock-state needs a NAME (a named value or collection) and a check",
            Hint::run(example("todos count 3")),
        ));
    };
    let Some((check, rest)) = args.values.split_first() else {
        return Err(bad_args(
            format!("expect mock-state {name} needs a check: count, value or contains"),
            Hint::run(example(&format!("{name} count 3"))),
        ));
    };
    if args.exact {
        return Err(bad_args(
            "--exact is not for expect mock-state",
            Hint::run(example(&format!("{name} value 'the value'"))),
        ));
    }
    if (args.min.is_some() || args.max.is_some()) && check != "count" {
        return Err(bad_args(
            format!("--min and --max are for count, not {check}"),
            Hint::run(example(&format!("{name} count --min 1"))),
        ));
    }
    let one = |shape: &str, sample: &str| -> Result<serde_json::Value, ErrorInfo> {
        match rest {
            [value] => Ok(crate::mock::state_value(&input::text_arg(value, "VALUE")?)),
            _ => Err(bad_args(
                format!("expect mock-state … {check} takes {shape}"),
                Hint::run(example(&format!("{name} {check} {sample}"))),
            )),
        }
    };
    let check = match check.as_str() {
        "count" => {
            let count = match rest {
                [] => None,
                [n] => Some(n.parse::<u64>().map_err(|_| {
                    bad_args(
                        format!("expect mock-state … count takes a number, got {n:?}"),
                        Hint::run(example(&format!("{name} count 3"))),
                    )
                })?),
                _ => {
                    return Err(bad_args(
                        "expect mock-state … count takes one number",
                        Hint::run(example(&format!("{name} count 3"))),
                    ));
                }
            };
            if count.is_none() && args.min.is_none() && args.max.is_none() {
                return Err(bad_args(
                    "expect mock-state … count takes a number, or --min/--max",
                    Hint::run(example(&format!("{name} count 3"))),
                ));
            }
            MockStateCheck::Count {
                count,
                min: args.min,
                max: args.max,
            }
        }
        "value" => MockStateCheck::Value {
            value: one("one value (JSON, else text)", "true")?,
        },
        "contains" => MockStateCheck::Contains {
            value: one("one JSON value", "'{\"title\":\"Milk\"}'")?,
        },
        other => {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{other:?} is not a check of expect mock-state"),
            )
            .note("checks", "count, value, contains")
            .hint(Hint::run(example(&format!("{name} count 3")))));
        }
    };
    Ok(Expectation::MockState { name, check })
}

/// The baselines' directory in a project: `.chrome-cdp/<kind>` of the
/// nearest directory up from `cwd` with a project config
/// (`chrome-cdp.toml` or `.chrome-cdp.toml`), `.chrome-cdp/` or `.git`,
/// else of `cwd`.
fn project_baselines(cwd: &Path, kind: &str) -> PathBuf {
    crate::config::project_root(cwd)
        .join(".chrome-cdp")
        .join(kind)
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

/// `expect --layout`'s baseline: `<dir>/layout/<name>.txt`, `<dir>` being
/// `--dir` or the project's `.chrome-cdp`.
pub(crate) fn layout_baseline(dir: Option<&Path>, name: &str) -> Result<PathBuf, ErrorInfo> {
    let dir = dir.map(|d| d.join("layout"));
    baseline_path(dir.as_deref(), "layout", name, ".txt", ".txt")
}

/// An audit baseline: a `.json` file as given (relative to the current
/// directory), else NAME's file, `<dir>/audit/<NAME>.json`, `<dir>` being
/// `--dir` or the project's `.chrome-cdp`.
pub(crate) fn audit_baseline(dir: Option<&Path>, name: &str) -> Result<PathBuf, ErrorInfo> {
    if Path::new(name).extension().is_some_and(|e| e == "json") {
        return input::absolute_path(Path::new(name)).map(PathBuf::from);
    }
    let dir = dir.map(|d| d.join("audit"));
    baseline_path(dir.as_deref(), "audit", name, ".json", ".json")
}

/// `expect-screenshot`'s baseline: `<name>-<os>.png`, one per platform,
/// since fonts render differently on each.
pub(crate) fn screenshot_baseline(dir: Option<&Path>, name: &str) -> Result<PathBuf, ErrorInfo> {
    let suffix = format!("-{}.png", std::env::consts::OS);
    baseline_path(dir, "screenshots", name, ".png", &suffix)
}

/// The operating systems whose screenshot baselines may sit side by side.
const OSES: [&str; 3] = ["linux", "macos", "windows"];

/// When screenshot baseline `path` (this OS's) does not exist but another
/// OS's does, the warning that says so (plan 11.6): fonts render
/// differently on each OS, so this one cannot be compared with it.
pub(crate) fn other_os_baselines(path: &Path) -> Option<String> {
    if path.exists() {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(&format!("-{}.png", std::env::consts::OS))?;
    let others: Vec<&str> = OSES
        .into_iter()
        .filter(|&os| {
            os != std::env::consts::OS && path.with_file_name(format!("{stem}-{os}.png")).exists()
        })
        .collect();
    if others.is_empty() {
        return None;
    }
    Some(format!(
        "the baseline {stem} was made on {} only; fonts render differently on each OS, so \
         {} needs one of its own (or run the tests in the Docker image of docs/e2e/determinism.md \
         everywhere)",
        others.join(" and "),
        std::env::consts::OS
    ))
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

    use super::{
        expectation, other_os_baselines, project_baselines, screenshot_baseline, snapshot_baseline,
    };
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
    fn mock_state_checks() {
        use engine::MockStateCheck;
        use serde_json::json;
        let state = |name: &str, check| Expectation::MockState {
            name: name.into(),
            check,
        };
        assert_eq!(
            parse(&["mock-state", "todos", "count", "4"]).unwrap(),
            state(
                "todos",
                MockStateCheck::Count {
                    count: Some(4),
                    min: None,
                    max: None
                }
            )
        );
        assert_eq!(
            parse(&["mock-state", "todos", "count", "--min", "1"]).unwrap(),
            state(
                "todos",
                MockStateCheck::Count {
                    count: None,
                    min: Some(1),
                    max: None
                }
            )
        );
        assert_eq!(
            parse(&["mock-state", "logged_in", "value", "true"]).unwrap(),
            state("logged_in", MockStateCheck::Value { value: json!(true) })
        );
        assert_eq!(
            parse(&["mock-state", "user", "value", "ada@example.com"]).unwrap(),
            state(
                "user",
                MockStateCheck::Value {
                    value: json!("ada@example.com")
                }
            )
        );
        assert_eq!(
            parse(&["mock-state", "todos", "contains", r#"{"title":"Milk"}"#]).unwrap(),
            state(
                "todos",
                MockStateCheck::Contains {
                    value: json!({"title": "Milk"})
                }
            )
        );
        let cases: &[(&[&str], &str)] = &[
            (
                &["mock-state"],
                "chrome-cdp expect mock-state todos count 3",
            ),
            (
                &["mock-state", "todos"],
                "chrome-cdp expect mock-state todos count 3",
            ),
            (
                &["mock-state", "todos", "size", "3"],
                "chrome-cdp expect mock-state todos count 3",
            ),
            (
                &["mock-state", "todos", "count", "many"],
                "chrome-cdp expect mock-state todos count 3",
            ),
            (
                &["mock-state", "flag", "value"],
                "chrome-cdp expect mock-state flag value true",
            ),
            (
                &["mock-state", "flag", "value", "1", "--min", "1"],
                "chrome-cdp expect mock-state flag count --min 1",
            ),
        ];
        for (args, hint) in cases {
            let err = parse(args).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{args:?}");
            assert_eq!(err.hint.as_deref(), Some(*hint), "{args:?}");
        }
    }

    #[test]
    fn request_checks() {
        use engine::RequestCheck;
        use serde_json::json;
        let Expectation::Request(check) = parse(&[
            "request",
            "post",
            "/api/orders",
            "--json-body",
            r#"{"qty":2}"#,
            "--query",
            "source=web",
            "--header",
            "X-Src: w*",
            "--count",
            "2",
            "--since",
            "before",
        ])
        .unwrap() else {
            panic!("not a request check");
        };
        assert_eq!(
            check,
            RequestCheck {
                method: "post".into(),
                target: "/api/orders".into(),
                query: vec![("source".into(), "web".into())],
                headers: vec![("X-Src".into(), "w*".into())],
                json: Some(json!({"qty": 2})),
                count: Some(2),
                min: None,
                max: None,
                since: Some("before".into()),
                none: false,
            }
        );
        let Expectation::Request(check) = parse(&["no-request", "DELETE", "/api/cart/**"]).unwrap()
        else {
            panic!("not a request check");
        };
        assert!(check.none);
        let cases: &[(&[&str], &str)] = &[
            (
                &["request", "POST"],
                "chrome-cdp expect request POST /api/orders",
            ),
            (
                &["no-request", "GET", "/a", "/b"],
                "chrome-cdp expect no-request POST /api/orders",
            ),
            (
                &["request", "GET", "/a", "--query", "q"],
                "chrome-cdp expect request GET /api/search --query q=keyboard",
            ),
            (
                &["e1", "visible", "--count", "2"],
                "chrome-cdp expect request POST /api/orders --count 1",
            ),
            (
                &["request", "GET", "/a", "--exact"],
                "chrome-cdp expect request POST /api/orders",
            ),
        ];
        for (args, hint) in cases {
            let err = parse(args).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadArgs, "{args:?}");
            assert_eq!(err.hint.as_deref(), Some(*hint), "{args:?}");
        }
        assert!(parse(&["request", "GET", "/a", "--json-body", "{nope"]).is_err());
        assert!(
            Cli::try_parse_from([
                "chrome-cdp",
                "expect",
                "request",
                "GET",
                "/a",
                "--count",
                "1",
                "--min",
                "1"
            ])
            .is_err()
        );
    }

    #[test]
    fn perf_budgets_are_checked_before_the_browser() {
        assert_eq!(
            parse(&["--perf", "lcp<2.5s cls<0.1"]).unwrap(),
            Expectation::Perf {
                budgets: "lcp<2.5s cls<0.1".into(),
                missing_ok: false,
            }
        );
        assert_eq!(
            parse(&["--perf", "good", "--perf-missing", "ok"]).unwrap(),
            Expectation::Perf {
                budgets: "good".into(),
                missing_ok: true,
            }
        );
        let err = parse(&["--perf", "lpc<2500"]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadArgs);
        assert_eq!(
            err.hint.as_deref(),
            Some("chrome-cdp expect --perf 'lcp<2500'   (did you mean this?)")
        );
        let err = parse(&["e1", "--perf", "lcp<2500"]).unwrap_err();
        assert!(
            err.message.starts_with("--perf checks the page"),
            "{}",
            err.message
        );
        for args in [
            &["--perf", "good", "--a11y"][..],
            &["--perf", "good", "--no-errors"],
            &["--perf-missing", "ok"],
            &["--perf", "good", "--perf-missing", "maybe"],
        ] {
            assert!(
                Cli::try_parse_from(["chrome-cdp", "expect"].iter().chain(args)).is_err(),
                "{args:?}"
            );
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

    #[test]
    fn a_baseline_of_another_os_only_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = screenshot_baseline(Some(dir.path()), "home").unwrap();
        assert_eq!(other_os_baselines(&path), None);
        let other = if std::env::consts::OS == "macos" {
            "linux"
        } else {
            "macos"
        };
        std::fs::write(dir.path().join(format!("home-{other}.png")), b"png").unwrap();
        let warning = other_os_baselines(&path).unwrap();
        assert!(
            warning.starts_with(&format!("the baseline home was made on {other} only;")),
            "{warning}"
        );
        std::fs::write(&path, b"png").unwrap();
        assert_eq!(other_os_baselines(&path), None);
    }
}

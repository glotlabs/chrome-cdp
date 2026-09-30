//! `history` and `history --as-scenario` (plan 11, milestone 11.8): the
//! commands the session ran, and the test they make.
//!
//! The daemon keeps each command's words, whether it succeeded and the
//! stable locators of the refs it named (see `daemon::history`); this
//! module turns them into a scenario: read-only and failed commands are
//! left out, refs become their locators, `open URL` becomes `goto`, URLs
//! under `base_url` become paths, and `mock use` becomes `@mocks`.

use std::fmt::Write as _;

use clap::{ArgMatches, CommandFactory as _};
use daemon::rpc;
use protocol::format::shell_quote;
use protocol::schema::{HistoryData, HistoryEntry};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output};
use serde_json::json;

use crate::cli::{Cli, GlobalArgs, HistoryCmdArgs};
use crate::session;

/// Commands that only read the page: a test does not need them. By
/// their first word, or their first two.
const READS: &[&str] = &[
    "attr",
    "box",
    "console",
    "count",
    "downloads",
    "errors",
    "find",
    "frames",
    "highlight",
    "html",
    "inspect",
    "locator",
    "markdown",
    "network",
    "pdf",
    "routes",
    "screenshot",
    "snapshot",
    "tabs",
    "text",
    "value",
    "clipboard read",
    "init-script list",
    "mock explain",
    "mock list",
    "mock log",
];

/// Commands about the session, files or chrome-cdp itself, not steps of a
/// test.
const TOOLS: &[&str] = &[
    "cdp",
    "close",
    "completions",
    "connect",
    "context",
    "convert",
    "coverage",
    "har",
    "help",
    "history",
    "init",
    "install",
    "perf",
    "schema",
    "sessions",
    "skill",
    "status",
    "trace",
    "uninstall",
    "mock check",
    "mock import",
    "mock record",
    "state save",
];

/// Global flags that say where a command ran or how it printed, not what
/// it did: `--flag value` ones, then switches.
const WHERE: &[&str] = &["--session", "--context", "--testid-attribute", "--base-url"];
const HOW: &[&str] = &["--json", "-q", "--quiet", "-v", "-vv", "-vvv", "--verbose"];

pub(crate) fn run(args: &HistoryCmdArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let output = session::call_running(global, rpc::HISTORY, &json!({ "clear": args.clear }))?;
    let mut data: HistoryData = serde_json::from_value(output.data).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!("the daemon sent a bad history: {e}"),
        )
    })?;
    // The context's own commands (the session's, without --context).
    data.entries
        .retain(|e| e.context == global.context && args.from.is_none_or(|n| e.n >= n));
    match &args.as_scenario {
        Some(name) => {
            let scenario = scenario(name, &data.entries, global.base_url.as_deref());
            data.scenario = Some(scenario.text.clone());
            data.left_out.clone_from(&scenario.left_out);
            let mut output = Output::typed(&data, scenario.text.trim_end().to_owned())
                .quiet(scenario.text.trim_end().to_owned())
                .limit(args.max_chars, "--max-chars 0 for all of it, or --from N");
            if scenario.steps == 0 {
                output = output.warning(
                    "no command made it into the test; `chrome-cdp history` lists what the \
                     session ran",
                );
            }
            if !scenario.left_out.is_empty() {
                output = output.warning(left_out_summary(&scenario.left_out));
            }
            for w in scenario.warnings {
                output = output.warning(w);
            }
            Ok(output)
        }
        None => Ok(listing(data, args)),
    }
}

/// `history`: one line per command, the newest `--limit`.
fn listing(mut data: HistoryData, args: &HistoryCmdArgs) -> Output {
    let total = data.entries.len();
    if args.limit > 0 && total > args.limit {
        data.entries.drain(..total - args.limit);
    }
    let mut text = String::new();
    if data.entries.is_empty() {
        text.push_str("no commands yet");
    }
    for e in &data.entries {
        let mark = if e.ok { "✓" } else { "✗" };
        let words: Vec<String> = e
            .words
            .iter()
            .map(|w| shell_quote(w).into_owned())
            .collect();
        let _ = write!(text, "{:>4} {mark} {}", e.n, words.join(" "));
        if let Some(kind) = &e.error {
            let _ = write!(text, "   ({kind})");
        }
        for r in &e.refs {
            match &r.locator {
                Some(l) => {
                    let _ = write!(text, "\n       {} = {l}", r.reference);
                }
                None => {
                    let _ = write!(
                        text,
                        "\n       {}: no stable locator ({})",
                        r.reference,
                        r.reason.as_deref().unwrap_or("unknown")
                    );
                }
            }
        }
        text.push('\n');
    }
    let shown = data.entries.len();
    if shown < total {
        let _ = writeln!(
            text,
            "… {} older commands; --limit 0 shows all",
            total - shown
        );
    }
    if args.clear {
        text.push_str("(history cleared)\n");
    }
    let next = Cmd::new("history").flag("as-scenario", "name");
    if shown > 0 && !args.clear {
        let _ = write!(text, "next: {next}");
    }
    let quiet = data
        .entries
        .last()
        .map_or_else(String::new, |e| e.n.to_string());
    Output::typed(&data, text.trim_end().to_owned())
        .quiet(quiet)
        .limit(args.max_chars, "--max-chars 0 for all of it, or --limit N")
}

/// A scenario made of commands.
#[derive(Debug, Default)]
struct Scenario {
    text: String,
    steps: usize,
    /// `12: snapshot -i (only reads)`.
    left_out: Vec<String>,
    warnings: Vec<String>,
}

/// "left out 3 commands: 4 snapshot (only reads), …".
fn left_out_summary(left: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut text = format!(
        "left out {} {}: {}",
        left.len(),
        if left.len() == 1 {
            "command"
        } else {
            "commands"
        },
        left.iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    );
    if left.len() > SHOWN {
        let _ = write!(text, "; … (--json lists all)");
    }
    text
}

fn scenario(name: &str, entries: &[HistoryEntry], base_url: Option<&str>) -> Scenario {
    let mut out = Scenario::default();
    let mut body: Vec<String> = Vec::new();
    let mut mocks: Option<Vec<String>> = None;
    for e in entries {
        let words = strip_globals(&e.words);
        let shown = words
            .iter()
            .map(|w| shell_quote(w).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let skip = |why: &str| format!("{}: {shown} ({why})", e.n);
        if !e.ok {
            let kind = e.error.as_deref().unwrap_or("failed");
            out.left_out.push(skip(&format!("failed: {kind}")));
            continue;
        }
        let Some(matches) = parse(&words) else {
            out.left_out.push(skip("does not parse"));
            continue;
        };
        let path = command_path(&matches);
        let first_two = path.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
        let first = path.first().cloned().unwrap_or_default();
        let listed =
            |list: &[&str]| list.contains(&first.as_str()) || list.contains(&first_two.as_str());
        if listed(READS) {
            out.left_out.push(skip("only reads"));
            continue;
        }
        if listed(TOOLS) {
            out.left_out.push(skip("not a step"));
            continue;
        }
        if first == "cookies" && path.len() == 1 {
            out.left_out.push(skip("only reads"));
            continue;
        }
        if first == "storage"
            && matches!(
                words.get(1).map(String::as_str),
                Some("list" | "get") | None
            )
        {
            out.left_out.push(skip("only reads"));
            continue;
        }
        if first_two == "mock state" && !words.iter().any(|w| w.starts_with("--set")) {
            out.left_out.push(skip("only reads"));
            continue;
        }
        if first_two == "mock use" {
            let flags = words.iter().skip(2).any(|w| w.starts_with('-'));
            if !flags {
                let sets: Vec<String> = words.iter().skip(2).cloned().collect();
                if sets.is_empty() {
                    out.left_out
                        .push(skip("the defaults apply to every test anyway"));
                } else {
                    mocks = Some(sets);
                }
                continue;
            }
        }
        let mut words = words;
        if first == "open" {
            let Some(url) = positional_url(&words) else {
                out.left_out.push(skip("opens the session"));
                continue;
            };
            words = vec!["goto".to_owned(), url];
        }
        if matches!(first.as_str(), "goto" | "open") || (first_two == "tab new" && words.len() > 2)
        {
            relative_urls(&mut words, base_url);
        }
        let mut comments = Vec::new();
        for index in locator_indices(&matches) {
            let Some(word) = words.get_mut(index) else {
                continue;
            };
            let bare = word.trim_start_matches('@').to_owned();
            let Some(r) = e.refs.iter().find(|r| r.reference == bare) else {
                continue;
            };
            if let Some(locator) = &r.locator {
                if r.how.as_deref() == Some("nth") {
                    out.warnings.push(format!(
                        "{}: {bare} became {locator}, by position (nothing else found it alone); \
                         a test id on the element would be sturdier",
                        e.n
                    ));
                }
                word.clone_from(locator);
            } else {
                let reason = r.reason.as_deref().unwrap_or("unknown");
                comments.push(format!(
                    "# {bare}: no stable locator ({reason}); replace it"
                ));
                out.warnings.push(format!(
                    "{}: {bare} stays a ref, which only works in this session: {reason}",
                    e.n
                ));
            }
        }
        body.extend(comments.into_iter().map(|c| format!("  {c}")));
        let line: Vec<String> = words.iter().map(|w| shell_quote(w).into_owned()).collect();
        body.push(format!("  {}", line.join(" ")));
        out.steps += 1;
    }
    let _ = writeln!(out.text, "@test {}", quoted_name(name));
    if let Some(sets) = mocks {
        let sets: Vec<String> = sets.iter().map(|s| shell_quote(s).into_owned()).collect();
        let _ = writeln!(out.text, "  @mocks {}", sets.join(" "));
    }
    for line in body {
        let _ = writeln!(out.text, "{line}");
    }
    out
}

/// `"adds item to cart"`, with `"` and `\` escaped and `${` made literal.
pub(crate) fn quoted_name(name: &str) -> String {
    let escaped = name
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "$${");
    format!("\"{escaped}\"")
}

/// `words` without the global flags that say where a command ran or how
/// it printed.
fn strip_globals(words: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    let mut iter = words.iter();
    while let Some(w) = iter.next() {
        if WHERE.contains(&w.as_str()) {
            iter.next();
            continue;
        }
        if WHERE
            .iter()
            .any(|f| w.strip_prefix(f).is_some_and(|rest| rest.starts_with('=')))
            || HOW.contains(&w.as_str())
        {
            continue;
        }
        out.push(w.clone());
    }
    out
}

fn parse(words: &[String]) -> Option<ArgMatches> {
    Cli::command()
        .try_get_matches_from(std::iter::once(protocol::PROGRAM.to_owned()).chain(words.to_vec()))
        .ok()
}

/// `["mock", "use"]`.
fn command_path(matches: &ArgMatches) -> Vec<String> {
    let mut path = Vec::new();
    let mut m = matches;
    while let Some((name, sub)) = m.subcommand() {
        path.push(name.to_owned());
        m = sub;
    }
    path
}

/// The indices in the words (without the program name) of the command's
/// locator arguments (those named `LOCATOR`, `FROM` and `TO`), as the
/// script checker finds them.
fn locator_indices(matches: &ArgMatches) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cmd = Cli::command();
    let mut m = matches;
    let mut depth = 0;
    while let Some((name, sub)) = m.subcommand() {
        let Some(next) = cmd.find_subcommand(name).cloned() else {
            break;
        };
        cmd = next;
        m = sub;
        for arg in cmd.get_arguments() {
            let locator = arg.get_value_names().is_some_and(|names| {
                names
                    .iter()
                    .any(|n| ["LOCATOR", "FROM", "TO"].contains(&n.as_str()))
            });
            if !locator {
                continue;
            }
            // A subcommand's indices count from its name (0).
            if let Some(indices) = m.indices_of(arg.get_id().as_str()) {
                out.extend(indices.map(|i| depth + i));
            }
        }
        depth += 1;
    }
    out
}

/// `open`'s URL: its first word that is not a flag or a flag's value.
fn positional_url(words: &[String]) -> Option<String> {
    let matches = parse(words)?;
    let (_, sub) = matches.subcommand()?;
    sub.get_raw("url")
        .and_then(|mut v| v.next())
        .and_then(|v| v.to_str())
        .map(str::to_owned)
}

/// Makes URLs under `base_url` paths: `http://localhost:5173/cart` is
/// `/cart` when base_url is `http://localhost:5173`.
fn relative_urls(words: &mut [String], base_url: Option<&str>) {
    let Some(base) = base_url.map(|b| b.trim_end_matches('/')) else {
        return;
    };
    for w in words.iter_mut().skip(1) {
        if let Some(rest) = w.strip_prefix(base)
            && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
        {
            *w = if rest.is_empty() {
                "/".to_owned()
            } else if rest.starts_with('/') {
                rest.to_owned()
            } else {
                format!("/{rest}")
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::RefLocator;

    use super::*;

    fn entry(n: u64, line: &str, refs: &[(&str, Option<&str>, &str)]) -> HistoryEntry {
        HistoryEntry {
            n,
            words: shell_words(line),
            context: None,
            ok: true,
            error: None,
            refs: refs
                .iter()
                .map(|(r, l, how)| RefLocator {
                    reference: (*r).to_owned(),
                    locator: l.map(str::to_owned),
                    how: Some((*how).to_owned()),
                    reason: l.is_none().then(|| "it is in a frame".to_owned()),
                })
                .collect(),
        }
    }

    /// Splits like the scenario syntax.
    fn shell_words(line: &str) -> Vec<String> {
        let parsed = protocol::script::parse("t", line);
        let line = parsed.script.commands().next().unwrap();
        line.words.iter().map(|w| w.literal().unwrap()).collect()
    }

    #[test]
    fn a_session_becomes_a_test() {
        let mut failed = entry(5, "click e9", &[]);
        failed.ok = false;
        failed.error = Some("NotFound".into());
        let entries = vec![
            entry(1, "open http://localhost:5173/shop --session s", &[]),
            entry(2, "mock use checkout", &[]),
            entry(3, "snapshot -i", &[]),
            entry(
                4,
                "click e12 --json",
                &[("e12", Some("role=button[name=\"Add to cart\"]"), "role")],
            ),
            failed,
            entry(
                6,
                "fill e3 'Ada Lovelace'",
                &[("e3", Some("label=\"Name\""), "label")],
            ),
            entry(
                7,
                "drag e4 e5",
                &[
                    ("e4", Some("text=\"Card\" >> nth=1"), "nth"),
                    ("e5", None, ""),
                ],
            ),
            entry(8, "expect text=Total visible", &[]),
            entry(9, "goto http://localhost:5173", &[]),
            entry(10, "console", &[]),
            entry(11, "type e12", &[("e12", Some("x"), "role")]),
        ];
        let s = scenario(
            "adds ${x} \"item\"",
            &entries,
            Some("http://localhost:5173/"),
        );
        assert_eq!(
            s.text,
            "@test \"adds $${x} \\\"item\\\"\"\n  @mocks checkout\n  goto /shop\n  click 'role=button[name=\"Add to cart\"]'\n  fill 'label=\"Name\"' 'Ada Lovelace'\n  # e5: no stable locator (it is in a frame); replace it\n  drag 'text=\"Card\" >> nth=1' e5\n  expect text=Total visible\n  goto /\n  type e12\n"
        );
        assert_eq!(s.steps, 7);
        assert_eq!(
            s.left_out,
            vec![
                "3: snapshot -i (only reads)",
                "5: click e9 (failed: NotFound)",
                "10: console (only reads)"
            ]
        );
        assert_eq!(s.warnings.len(), 2, "{:?}", s.warnings);
    }

    #[test]
    fn the_scenario_parses_as_a_test() {
        let entries = vec![
            entry(1, "goto /x", &[]),
            entry(
                2,
                "click e1",
                &[(
                    "e1",
                    Some("role=listitem >> has-text=Mouse >> role=button[name=\"Add\"]"),
                    "scoped",
                )],
            ),
        ];
        let s = scenario("x", &entries, None);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.cdp");
        std::fs::write(&file, &s.text).unwrap();
        let loaded = e2e::scenario::load(&file, "x.cdp");
        assert!(loaded.is_ok(), "{:?}\n{}", loaded.err(), s.text);
        assert_eq!(
            s.text,
            "@test \"x\"\n  goto /x\n  click 'role=listitem >> has-text=Mouse >> role=button[name=\"Add\"]'\n"
        );
    }
}

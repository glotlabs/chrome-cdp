//! `batch` and `run` (plan 08, milestone 8.2): run many commands in one
//! call, from stdin or a `.cdp` file (docs/scenario-syntax.md).
//!
//! The whole script is parsed, its variables filled in, and every line
//! checked against the clap definitions before the first step runs, so a
//! typo fails before the browser is touched. Then each step runs in this
//! process through the same code a single command uses, and prints as it
//! ends: a `[3/7] click e6 ✓` line and its usual output (text), its usual
//! output only (`-q`), or one JSON line (`--json`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::parser::ValueSource;
use clap::{ArgMatches, CommandFactory as _, FromArgMatches as _};
use protocol::schema::StepInfo;
use protocol::script::{Item, Line, Resolve, ScriptError, Span, Unresolved, Var};
use protocol::{Cmd, Envelope, ErrorInfo, ErrorKind, Hint, Mode};
use serde::Serialize;
use serde_json::json;

use crate::cli::{BatchArgs, Cli, Command, GlobalArgs, RunArgs, ScriptArgs};
use crate::{input, output};

/// How many problems one error lists as notes; the rest are counted.
const MAX_LISTED: usize = 10;

/// The directives `batch` and `run` know, for "did you mean". The parser
/// takes `@syntax` out of the tree, so any directive left is unknown.
const DIRECTIVES: &[&str] = &["syntax"];

/// `batch`: the script is stdin.
pub(crate) fn batch(args: &BatchArgs, global: &GlobalArgs) -> u8 {
    let mode = global.mode();
    let src = match input::take_stdin("the batch script") {
        Ok(src) => src,
        Err(info) => return output::print(mode, Err(info)),
    };
    let base = std::env::current_dir().unwrap_or_default();
    let usage = Cmd::new("batch")
        .session(&global.session)
        .context(global.context.as_deref());
    execute("<stdin>", &src, &base, usage, &args.script, global)
}

/// `run <file>`.
pub(crate) fn run(args: &RunArgs, global: &GlobalArgs) -> u8 {
    let mode = global.mode();
    let name = args.file.display().to_string();
    let src = match std::fs::read_to_string(&args.file) {
        Ok(src) => src,
        Err(e) => {
            let info = ErrorInfo::new(ErrorKind::BadArgs, format!("cannot read {name}: {e}")).hint(
                Hint::text("check the path; it is relative to the current directory"),
            );
            return output::print(mode, Err(info));
        }
    };
    let base = args
        .file
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf);
    let usage = Cmd::new("run")
        .arg(&name)
        .session(&global.session)
        .context(global.context.as_deref());
    execute(&name, &src, &base, usage, &args.script, global)
}

/// Checks the script, then runs it. `usage` is the command that ran it,
/// for hints (`chrome-cdp run login.cdp`).
fn execute(
    file: &str,
    src: &str,
    base: &Path,
    usage: Cmd,
    args: &ScriptArgs,
    global: &GlobalArgs,
) -> u8 {
    let mode = global.mode();
    let vars = Vars {
        names: args.vars.iter().cloned().collect(),
        base: base.to_path_buf(),
        usage,
    };
    match prepare(file, src, &vars, global) {
        Ok(steps) => run_steps(steps, args.keep_going, mode),
        Err(problems) => output::print(mode, Err(invalid(problems))),
    }
}

/// A checked step, ready to run.
pub(crate) struct Step {
    pub(crate) cli: Cli,
    pub(crate) span: Span,
    /// The line as written.
    pub(crate) text: String,
    /// The words, variables filled in: [`command`] makes the step's
    /// [`Cli`] again from them.
    pub(crate) words: Vec<String>,
}

/// A problem found while checking a script.
pub(crate) struct Problem {
    pub(crate) span: Span,
    pub(crate) info: ErrorInfo,
}

impl From<ScriptError> for Problem {
    fn from(e: ScriptError) -> Self {
        let span = e.span.clone();
        let hint = e.hint.clone();
        let info = ErrorInfo::new(ErrorKind::BadArgs, e.message);
        Self {
            span,
            info: match hint {
                Some(hint) => info.hint(hint),
                None => info,
            },
        }
    }
}

/// Parses `src` and checks every line; the steps, or every problem.
fn prepare(
    file: &str,
    src: &str,
    vars: &dyn Resolve,
    global: &GlobalArgs,
) -> Result<Vec<Step>, Vec<Problem>> {
    let parsed = protocol::script::parse(file, src);
    let mut problems: Vec<Problem> = parsed.errors.into_iter().map(Problem::from).collect();
    let mut steps = Vec::new();
    for item in &parsed.script.items {
        match item {
            Item::Comment(_) => {}
            Item::Directive(d) => {
                let known = protocol::suggest::similar(&d.name, DIRECTIVES.iter().copied(), 1);
                let hint = match known.first() {
                    Some(name) => format!("did you mean @{name}?"),
                    None => "batch and run know only @syntax; see docs/scenario-syntax.md".into(),
                };
                problems.push(
                    ScriptError::new(d.span.clone(), format!("unknown directive @{}", d.name))
                        .hint(hint)
                        .into(),
                );
            }
            Item::Command(line) => match step(line, vars, global) {
                Ok(step) => steps.push(step),
                Err(problem) => problems.push(problem),
            },
        }
    }
    if !problems.is_empty() {
        problems.sort_by_key(|p| p.span.start);
        return Err(problems);
    }
    if steps.is_empty() {
        return Err(vec![Problem {
            span: Span {
                file: parsed.script.file.clone(),
                line: 1,
                column: 1,
                start: 0,
                end: 0,
            },
            info: ErrorInfo::new(ErrorKind::BadArgs, "the script has no commands").hint(
                Hint::text("write one command per line, e.g. goto example.com"),
            ),
        }]);
    }
    Ok(steps)
}

/// Checks a generated script (`convert`'s) as `run` would, every
/// variable standing for `x`: its steps' text, or each problem.
#[cfg(test)]
pub(crate) fn check(src: &str) -> Result<Vec<String>, Vec<String>> {
    struct Any;
    impl Resolve for Any {
        fn resolve(&self, _: &Var) -> Result<String, Unresolved> {
            Ok("x".into())
        }
    }
    let global = <Cli as clap::Parser>::try_parse_from([protocol::PROGRAM])
        .expect("no arguments parse")
        .global;
    match prepare("generated.cdp", src, &Any, &global) {
        Ok(steps) => Ok(steps.into_iter().map(|s| s.text).collect()),
        Err(problems) => Err(problems
            .into_iter()
            .map(|p| format!("line {}: {}", p.span.line, p.info.message))
            .collect()),
    }
}

/// Expands and checks one command line.
// Checked once per line before anything runs; the size does not matter.
#[allow(clippy::result_large_err)]
pub(crate) fn step(line: &Line, vars: &dyn Resolve, global: &GlobalArgs) -> Result<Step, Problem> {
    let words = line
        .words
        .iter()
        .map(|w| w.expand(vars))
        .collect::<Result<Vec<_>, _>>()?;
    let problem = |info: ErrorInfo| Problem {
        span: line.span.clone(),
        info,
    };
    let matches = Cli::command()
        .try_get_matches_from(std::iter::once(protocol::PROGRAM.to_owned()).chain(words.clone()))
        .map_err(|e| problem(clap_problem(&e, &words, line)))?;
    let mut cli =
        Cli::from_arg_matches(&matches).map_err(|e| problem(clap_problem(&e, &words, line)))?;
    match &cli.command {
        None if !cli.version => {
            return Err(problem(
                ErrorInfo::new(ErrorKind::BadArgs, "the line has no command")
                    .hint(Hint::text("start the line with a command, e.g. goto")),
            ));
        }
        Some(Command::Batch(_) | Command::Run(_) | Command::Test(_) | Command::Daemon(_)) => {
            return Err(problem(
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    "batch, run and test cannot run inside a script",
                )
                .hint(Hint::text("put the other script's lines in this one")),
            ));
        }
        _ => {}
    }
    let given = |id: &str| matches.value_source(id) == Some(ValueSource::CommandLine);
    if given("json") && !global.json {
        return Err(problem(
            ErrorInfo::new(ErrorKind::BadArgs, "--json on a line of a text script").hint(
                Hint::text("remove it from the line and pass --json to batch or run instead"),
            ),
        ));
    }
    check_locators(&matches).map_err(problem)?;
    inherit(&mut cli.global, &matches, global);
    Ok(Step {
        cli,
        span: line.span.clone(),
        text: line.text.clone(),
        words,
    })
}

/// The [`Cli`] of a checked step's `words` again (a step that runs more
/// than once needs a fresh one each time), with `global`'s flags except
/// those the line sets.
pub(crate) fn command(words: &[String], global: &GlobalArgs) -> Result<Cli, ErrorInfo> {
    let matches = Cli::command()
        .try_get_matches_from(std::iter::once(protocol::PROGRAM.to_owned()).chain(words.to_vec()))
        .map_err(|e| crate::usage_info(&e, words))?;
    let mut cli = Cli::from_arg_matches(&matches).map_err(|e| crate::usage_info(&e, words))?;
    inherit(&mut cli.global, &matches, global);
    Ok(cli)
}

/// A clap error about a line, in the §3 form. A bare `#word` that became a
/// comment is the likely cause when a line is missing an argument.
fn clap_problem(e: &clap::Error, words: &[String], line: &Line) -> ErrorInfo {
    let info = if matches!(
        e.kind(),
        clap::error::ErrorKind::DisplayHelp
            | clap::error::ErrorKind::DisplayVersion
            | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) && words.iter().any(|w| w == "--help" || w == "-h")
    {
        ErrorInfo::new(ErrorKind::BadArgs, "--help does not work in a script")
            .hint(Cmd::new(&crate::typed_command(words)).switch("help"))
    } else {
        crate::usage_info(e, words)
    };
    // `click #submit`: the comment starts right after the `#`. Said in the
    // message, which is all a problem after the first one shows.
    let hidden = line
        .comment
        .as_ref()
        .filter(|c| c.text.starts_with(|ch: char| !ch.is_whitespace()))
        .and_then(|c| c.text.split_whitespace().next());
    match hidden {
        Some(word) => ErrorInfo {
            message: format!(
                "{} (#{word} starts a comment; quote it to pass it: '#{word}')",
                info.message
            ),
            ..info
        },
        None => info,
    }
}

/// Parses the line's locators (the arguments named `LOCATOR`, and
/// `drag`'s `FROM` and `TO`), so a bad one fails before the script starts
/// rather than when its step runs.
fn check_locators(matches: &ArgMatches) -> Result<(), ErrorInfo> {
    for value in locator_values(matches) {
        engine::parse_locator(&value)?;
    }
    Ok(())
}

/// The values of a parsed line's locator arguments (named `LOCATOR`,
/// `FROM` or `TO`).
pub(crate) fn locator_values(matches: &ArgMatches) -> Vec<String> {
    let mut out = Vec::new();
    let mut cmd = Cli::command();
    let mut matches = matches;
    while let Some((name, sub)) = matches.subcommand() {
        let Some(next) = cmd.find_subcommand(name).cloned() else {
            break;
        };
        cmd = next;
        matches = sub;
        for arg in cmd.get_arguments() {
            let locator = arg.get_value_names().is_some_and(|names| {
                names
                    .iter()
                    .any(|n| ["LOCATOR", "FROM", "TO"].contains(&n.as_str()))
            });
            if !locator {
                continue;
            }
            if let Ok(Some(values)) = matches.try_get_raw(arg.get_id().as_str()) {
                for value in values.filter_map(|v| v.to_str()) {
                    // `expect mock-state <name> …` and `expect request …`
                    // check no element.
                    if name == "expect" && matches!(value, "mock-state" | "request" | "no-request")
                    {
                        continue;
                    }
                    out.push(value.to_owned());
                }
            }
        }
    }
    out
}

/// Gives a step the script's global flags, except those its line sets.
fn inherit(step: &mut GlobalArgs, matches: &ArgMatches, script: &GlobalArgs) {
    let given = |id: &str| matches.value_source(id) == Some(ValueSource::CommandLine);
    if !given("session") {
        step.session.clone_from(&script.session);
    }
    if !given("timeout") {
        step.timeout = script.timeout;
    }
    if !given("tab") {
        step.tab.clone_from(&script.tab);
    }
    if !given("context") {
        step.context.clone_from(&script.context);
    }
    if !given("quiet") {
        step.quiet = script.quiet;
    }
    step.json = script.json;
    step.verbose = script.verbose;
    step.base_url.clone_from(&script.base_url);
    step.testid_attribute.clone_from(&script.testid_attribute);
    step.snap_after_action = script.snap_after_action;
}

/// The one error for a script that did not start: the first problem, the
/// others as notes, and all of them in `details`.
pub(crate) fn invalid(problems: Vec<Problem>) -> ErrorInfo {
    let details = json!({
        "problems": problems
            .iter()
            .map(|p| json!({"at": p.span.to_string(), "message": p.info.message}))
            .collect::<Vec<_>>()
    });
    let mut problems = problems.into_iter();
    let Some(first) = problems.next() else {
        return ErrorInfo::new(ErrorKind::BadArgs, "the script is not valid");
    };
    let mut info = first.info;
    info.message = format!("{}: {}", first.span, info.message);
    let rest: Vec<Problem> = problems.collect();
    for p in rest.iter().take(MAX_LISTED) {
        info = info.note("also", format!("{}: {}", p.span, p.info.message));
    }
    if rest.len() > MAX_LISTED {
        info = info.note(
            "also",
            format!("{} more problems (see --json)", rest.len() - MAX_LISTED),
        );
    }
    info.details(details)
}

/// One `--json` line: the step's place, then its command's envelope.
#[derive(Serialize)]
struct StepLine<'a> {
    #[serde(flatten)]
    step: &'a StepInfo,
    #[serde(flatten)]
    envelope: &'a Envelope,
}

/// Runs the steps in order and returns the exit code: the first failed
/// step's, else 0.
fn run_steps(steps: Vec<Step>, keep_going: bool, mode: Mode) -> u8 {
    let total = steps.len();
    let mut failed: Vec<usize> = Vec::new();
    let mut exit = 0;
    for (index, step) in steps.into_iter().enumerate() {
        let n = index + 1;
        let Step {
            cli,
            span,
            text,
            words,
        } = step;
        let step_mode = if mode == Mode::Json {
            Mode::Json
        } else {
            cli.global.mode()
        };
        daemon::client::set_history_line(Some(words));
        let result = crate::run(cli).map_err(|mut info| {
            info.message = format!("{span}: {}", info.message);
            info
        });
        daemon::client::set_history_line(None);
        let code = result.as_ref().map_or_else(ErrorInfo::exit_code, |_| 0);
        if code != 0 {
            failed.push(n);
            if exit == 0 {
                exit = code;
            }
        }
        match step_mode {
            Mode::Json => {
                let info = StepInfo {
                    step: n,
                    total,
                    at: span.to_string(),
                    command: text.clone(),
                };
                let envelope = match result {
                    Ok(out) => Envelope::from_output(out),
                    Err(err) => Envelope::err(err),
                };
                let line = serde_json::to_string(&StepLine {
                    step: &info,
                    envelope: &envelope,
                })
                .unwrap_or_else(|_| envelope.to_json());
                output::stdout(&format!("{line}\n"));
            }
            Mode::Text | Mode::Quiet => {
                let rendered = output::render(step_mode, result);
                if mode == Mode::Text {
                    let mark = if code == 0 { "✓" } else { "✗" };
                    output::stdout(&format!("[{n}/{total}] {text} {mark}\n"));
                }
                output::stdout(&rendered.stdout);
                output::stderr(&rendered.stderr);
            }
        }
        if code != 0 && !keep_going {
            break;
        }
    }
    if mode != Mode::Json {
        output::stderr(&summary(&failed, total, keep_going));
    }
    exit
}

/// The closing note on stderr after a failure; empty when all passed.
fn summary(failed: &[usize], total: usize, keep_going: bool) -> String {
    let mut out = String::new();
    match failed {
        [] => {}
        [step] if !keep_going => {
            let left = total - step;
            if left > 0 {
                let _ = writeln!(
                    out,
                    "stopped at step {step} of {total}; {left} step{} did not run (--continue runs them anyway)",
                    if left == 1 { "" } else { "s" }
                );
            }
        }
        steps => {
            let list: Vec<String> = steps.iter().map(ToString::to_string).collect();
            let _ = writeln!(
                out,
                "{} of {total} steps failed: {}",
                steps.len(),
                list.join(", ")
            );
        }
    }
    out
}

/// Variables of `batch` and `run`: `--var`, `env:` and `data:`.
struct Vars {
    names: HashMap<String, String>,
    /// `data:` files are relative to it: the script's folder (the current
    /// one for `batch`).
    base: PathBuf,
    /// The command that runs the script, for hints.
    usage: Cmd,
}

impl Resolve for Vars {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        match var {
            Var::Name(name) => self.names.get(name).cloned().ok_or_else(|| {
                Unresolved::new(format!("{var} is not set")).hint(
                    Hint::run(self.usage.clone().flag("var", format!("{name}=…"))).to_string(),
                )
            }),
            Var::Env(name) => protocol::script::env_value(var, name),
            Var::Data { file, pointer } => protocol::script::data_value(
                var,
                &self.base,
                file,
                pointer,
                "data: paths are relative to the script's folder",
            ),
            Var::Var(_) | Var::Secret(_) | Var::Path(_) => Err(Unresolved::new(format!(
                "{var} has no value in batch and run scripts"
            ))
            .hint("pass the value with --var name=value and write ${name}")),
        }
    }
}

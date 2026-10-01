//! The `chrome-cdp` binary: parses the command line, runs the command and
//! renders its result (plan 08 §2-§4).
//!
//! Every command returns `Result<Output, ErrorInfo>`; [`output::print`]
//! renders it as text, quiet or `--json` output (with the shared renderer in
//! `protocol`) and picks the exit code.

use std::process::ExitCode;

use clap::error::{ContextKind, ContextValue, ErrorKind as ClapErrorKind};
use clap::{CommandFactory as _, FromArgMatches as _};
use protocol::{Cmd, ErrorInfo, ErrorKind, Mode, Output};

mod cli;
mod completions;
mod config;
mod convert;
mod doctor;
mod e2e_files;
mod emulate;
mod expect;
mod fmt;
mod help;
mod history;
mod http;
mod init;
mod input;
mod install;
mod lint;
mod log;
mod mock;
mod output;
mod progress;
mod repro;
mod route;
mod schema;
mod script;
mod session;
mod skill;
#[cfg(test)]
mod surface;
mod test;
mod upgrade;
mod version;

use cli::{Cli, ClipboardCommand, Command, KeyboardCommand};

fn main() -> ExitCode {
    let parsed = Cli::command()
        .try_get_matches()
        .and_then(|m| Cli::from_arg_matches(&m).map(|cli| (cli, m)));
    let (mut cli, matches) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => return ExitCode::from(usage_error(&e)),
    };
    if let Some(Command::Daemon(args)) = &cli.command {
        // The daemon logs to daemon.log (its stderr) at info level, and has
        // no output to render.
        log::init(cli.global.verbose.max(1));
        return ExitCode::from(daemon::server::run(&daemon::server::ServeOptions {
            session: cli.global.session.clone(),
            build: args.build.clone(),
            no_browser_grace: args
                .no_browser_grace
                .unwrap_or(daemon::server::NO_BROWSER_GRACE),
        }));
    }
    log::init(cli.global.verbose);
    let mode = cli.global.mode();
    // `test DIR` of a folder with its own project file runs in it (18.5).
    if let Some(Command::Test(args)) = &mut cli.command
        && let Err(info) = test::enter_project(args)
    {
        return ExitCode::from(output::print(mode, Err(info)));
    }
    if let Err(info) = config::apply(&mut cli.global, &matches) {
        return ExitCode::from(output::print(mode, Err(info)));
    }
    // Places in the page's code print relative to the project (plan 15).
    daemon::client::set_project_root(
        cli.global
            .source_root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned()),
    );
    // The daemon's stable locators for the history read the configured
    // attribute (plan 18.1).
    daemon::client::set_testid_attribute(cli.global.testid_attribute.clone());
    // `batch`, `run` and `test` print each step or test as it ends.
    match &cli.command {
        Some(Command::Batch(args)) => return ExitCode::from(script::batch(args, &cli.global)),
        Some(Command::Run(args)) => return ExitCode::from(script::run(args, &cli.global)),
        Some(Command::Lint(args)) => return ExitCode::from(lint::run(args, &cli.global)),
        Some(Command::Test(args)) => {
            let explicit = matches!(
                matches.value_source("session"),
                Some(
                    clap::parser::ValueSource::CommandLine | clap::parser::ValueSource::EnvVariable
                )
            );
            return ExitCode::from(test::run(args, &cli.global, explicit));
        }
        _ => {}
    }
    // The session keeps the command's words in its history (plan 11.8).
    daemon::client::set_history_line(Some(
        std::env::args_os()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    ));
    let exit_on_finding = exits_on_finding(&cli);
    let result = run(cli);
    let differs = exit_on_finding && result.as_ref().is_ok_and(found);
    match output::print(mode, result) {
        0 if differs => ExitCode::from(1),
        code => ExitCode::from(code),
    }
}

/// Whether the command was given `--exit-code`: `compare` then exits 1
/// when the pages differ, like `git diff --exit-code`, and `explore` when
/// it found a problem. A step of `run`, `batch` or `test` fails the same
/// way.
pub(crate) fn exits_on_finding(cli: &Cli) -> bool {
    match &cli.command {
        Some(Command::Compare(args)) => args.exit_code,
        Some(Command::Explore(args)) => args.exit_code,
        _ => false,
    }
}

/// Whether `compare` found a difference or `explore` a problem.
pub(crate) fn found(out: &Output) -> bool {
    out.data["differs"] == serde_json::Value::Bool(true)
        || out.data["problems"].as_u64().is_some_and(|n| n > 0)
}

fn run(cli: Cli) -> Result<Output, ErrorInfo> {
    if cli.version {
        return Ok(version::run());
    }
    match cli.command {
        None => Err(ErrorInfo::new(ErrorKind::BadArgs, "no command given")
            .hint(Cmd::new("").switch("help"))),
        Some(Command::Schema(args)) => schema::run(&args),
        Some(Command::Help(args)) => help::run(&args),
        Some(Command::Skill(args)) => skill::run(&args),
        Some(Command::Convert(args)) => convert::run(&args),
        Some(Command::Fmt(args)) => fmt::run(&args),
        Some(Command::Init(args)) => init::run(&args),
        Some(Command::Completions(args)) => Ok(completions::run(&args)),
        Some(Command::Install(args)) => {
            install::install(&args, cli.global.timeout_or(cli::DEFAULT_TIMEOUT))
        }
        Some(Command::Uninstall(args)) => install::uninstall(&args),
        Some(Command::Doctor(args)) => {
            doctor::run(&args, cli.global.timeout_or(cli::DEFAULT_TIMEOUT))
        }
        Some(Command::Upgrade(args)) => {
            upgrade::run(&args, cli.global.timeout_or(cli::DEFAULT_TIMEOUT))
        }
        Some(Command::Open(args)) => session::open(&args, &cli.global),
        Some(Command::Connect(args)) => session::connect(&args, &cli.global),
        Some(Command::Close(args)) => session::close(&args, &cli.global),
        Some(Command::Status(args)) => session::status(&args, &cli.global),
        Some(Command::Sessions(args)) => session::sessions(&args, &cli.global),
        Some(Command::Cdp(args)) => session::cdp(&args, &cli.global),
        Some(Command::Tabs(args)) => session::tabs(&args, &cli.global),
        Some(Command::Frames(args)) => session::frames(&args, &cli.global),
        Some(Command::Context(args)) => session::context(&args, &cli.global),
        Some(Command::Tab(args)) => session::tab(&args, &cli.global),
        Some(Command::Goto(args)) => session::goto(&args, &cli.global),
        Some(Command::Back(args)) => session::history(engine::Navigation::Back, &args, &cli.global),
        Some(Command::Forward(args)) => {
            session::history(engine::Navigation::Forward, &args, &cli.global)
        }
        Some(Command::Reload(args)) => session::reload(&args, &cli.global),
        Some(Command::Wait(args)) => session::wait(&args, &cli.global),
        Some(Command::Expect(args)) => session::expect(&args, &cli.global),
        Some(Command::Mark(args)) => session::mark(&args, &cli.global),
        Some(Command::History(args)) => history::run(&args, &cli.global),
        Some(Command::Repro(args)) => repro::run(&args, &cli.global),
        Some(Command::ExpectSnapshot(args)) => session::expect_snapshot(&args, &cli.global),
        Some(Command::ExpectScreenshot(args)) => session::expect_screenshot(&args, &cli.global),
        Some(Command::Dialog(args)) => session::dialog(&args, &cli.global),
        Some(Command::Downloads(args)) => session::downloads(&args, &cli.global),
        Some(Command::Console(args)) => {
            session::console(&args.read, false, args.level.to_level(), &cli.global)
        }
        Some(Command::Errors(args)) => session::console(
            &args.read,
            true,
            protocol::schema::Level::Error,
            &cli.global,
        ),
        Some(Command::Issues(args)) => session::issues(&args, &cli.global),
        Some(Command::Audit(args)) => session::audit(&args, &cli.global),
        Some(Command::Network(args)) => session::network(&args, &cli.global),
        Some(Command::Route(args)) => session::route(&args, &cli.global),
        Some(Command::Block(args)) => session::block(&args, &cli.global),
        Some(Command::Routes(args)) => session::routes(&args, &cli.global),
        Some(Command::Unroute(args)) => session::unroute(&args, &cli.global),
        Some(Command::Mock(args)) => mock::run(&args, &cli.global),
        Some(Command::Http(args)) => http::run(&args, &cli.global),
        Some(Command::InitScript(args)) => session::init_script(&args, &cli.global),
        Some(Command::Expose(args)) => session::expose(
            &args.names,
            session::Exposing::Add {
                returns: args.returns.as_deref(),
                returns_seq: args.returns_seq.as_deref(),
            },
            &cli.global,
        ),
        Some(Command::Unexpose(args)) => session::expose(
            &args.names,
            session::Exposing::Remove { all: args.all },
            &cli.global,
        ),
        Some(Command::Emulate(args)) => {
            if args.list {
                Ok(engine::device_list())
            } else {
                session::emulate(emulate::emulate(&args, &cli.global.session)?, &cli.global)
            }
        }
        Some(Command::Viewport(args)) => session::emulate(emulate::viewport(&args)?, &cli.global),
        Some(Command::Geo(args)) => session::emulate(emulate::geo(&args)?, &cli.global),
        Some(Command::Tz(args)) => session::emulate(emulate::timezone(&args), &cli.global),
        Some(Command::Locale(args)) => session::emulate(emulate::locale(&args), &cli.global),
        Some(Command::Ua(args)) => session::emulate(emulate::user_agent(&args)?, &cli.global),
        Some(Command::Media(args)) => session::emulate(emulate::media(&args)?, &cli.global),
        Some(Command::Offline(args)) => session::emulate(emulate::offline(&args), &cli.global),
        Some(Command::Throttle(args)) => session::emulate(emulate::throttle(&args)?, &cli.global),
        Some(Command::CpuThrottle(args)) => {
            session::emulate(emulate::cpu_throttle(&args)?, &cli.global)
        }
        Some(Command::Time(args)) => session::emulate(emulate::time(&args.command)?, &cli.global),
        Some(Command::Permissions(args)) => session::permissions(&args, &cli.global),
        Some(Command::Extensions(args)) => session::extensions(&args, &cli.global),
        Some(Command::Cookies(args)) => session::cookies(&args, &cli.global),
        Some(Command::Storage(args)) => session::storage(&args, &cli.global),
        Some(Command::State(args)) => session::state(&args, &cli.global),
        Some(Command::Screenshot(args)) => session::screenshot(&args, &cli.global),
        Some(Command::Pdf(args)) => session::pdf(&args, &cli.global),
        Some(Command::Eval(args)) => session::eval(&args, &cli.global),
        Some(Command::Har(args)) => session::har(&args, &cli.global),
        Some(Command::Trace(args)) => session::trace(&args, &cli.global),
        Some(Command::Perf(_)) => session::perf(&cli.global),
        Some(Command::Coverage(args)) => session::coverage(&args, &cli.global),
        Some(Command::Debug(args)) => session::debug(&args, &cli.global),
        Some(Command::Highlight(args)) => session::highlight(&args, &cli.global),
        Some(Command::Source(args)) => session::source(&args, &cli.global),
        Some(Command::Why(args)) => session::why(&args, &cli.global),
        Some(Command::Forms(args)) => session::forms(&args, &cli.global),
        Some(Command::FillForm(args)) => session::fill_form(&args, &cli.global),
        Some(Command::Layout(args)) => session::layout(&args, &cli.global),
        Some(Command::Compare(args)) => session::compare(&args, &cli.global),
        Some(Command::Explore(args)) => session::explore(&args, &cli.global),
        Some(Command::Components(args)) => session::components(&args, &cli.global),
        Some(Command::Inspect(args)) => session::inspect(&args, &cli.global),
        Some(Command::Clipboard(args)) => match &args.command {
            ClipboardCommand::Read(read) => session::clipboard_read(read, &cli.global),
            ClipboardCommand::Write(write) => {
                if write.text.as_deref() == Some("-") && write.html.as_deref() == Some("-") {
                    return Err(ErrorInfo::new(
                        ErrorKind::BadArgs,
                        "TEXT and --html cannot both come from stdin",
                    ));
                }
                let op = engine::ClipboardOp::Write {
                    text: write
                        .text
                        .as_deref()
                        .map(|text| line_arg(text, "TEXT"))
                        .transpose()?,
                    html: write
                        .html
                        .as_deref()
                        .map(|html| input::text_arg(html, "the HTML"))
                        .transpose()?,
                    image: write
                        .file
                        .as_deref()
                        .map(input::absolute_path)
                        .transpose()?
                        .map(std::path::PathBuf::from),
                };
                session::clipboard(op, 0, &cli.global)
            }
        },
        Some(Command::Snapshot(args)) => session::snapshot(&args, &cli.global),
        Some(Command::Find(args)) => session::find(&args, &cli.global),
        Some(Command::Locator(args)) => session::locator(&args, &cli.global),
        Some(Command::Text(args)) => session::read(
            engine::Read::Text,
            args.locator.clone(),
            &args.refs,
            args.max_chars,
            &cli.global,
        ),
        Some(Command::Html(args)) => session::read(
            engine::Read::Html {
                outer: args.outer,
                raw: args.raw,
            },
            args.locator.clone(),
            &args.refs,
            args.max_chars,
            &cli.global,
        ),
        Some(Command::Attr(args)) => session::read(
            engine::Read::Attr {
                name: args.name.clone(),
            },
            Some(args.locator.clone()),
            &args.refs,
            args.max_chars,
            &cli.global,
        ),
        Some(Command::Value(args)) => session::read(
            engine::Read::Value,
            Some(args.locator.clone()),
            &args.refs,
            args.max_chars,
            &cli.global,
        ),
        Some(Command::Count(args)) => session::read(
            engine::Read::Count,
            Some(args.locator.clone()),
            &args.refs,
            0,
            &cli.global,
        ),
        Some(Command::BoundingBox(args)) => session::read(
            engine::Read::Box,
            Some(args.locator.clone()),
            &args.refs,
            0,
            &cli.global,
        ),
        Some(Command::Markdown(args)) => session::read(
            engine::Read::Markdown,
            args.locator.clone(),
            &args.refs,
            args.max_chars,
            &cli.global,
        ),
        Some(Command::Click(args)) => session::act(
            pointer_action(&args.pointer, |locator, modifiers, position, force| {
                engine::Action::Click {
                    locator,
                    button: args.button.to_button(),
                    count: args.count,
                    modifiers,
                    position,
                    force,
                    dblclick: false,
                }
            }),
            args.pointer.refs.strict_refs,
            &args.pointer.after,
            &cli.global,
        ),
        Some(Command::Dblclick(args)) => session::act(
            pointer_action(&args.pointer, |locator, modifiers, position, force| {
                engine::Action::Click {
                    locator,
                    button: args.button.to_button(),
                    count: 2,
                    modifiers,
                    position,
                    force,
                    dblclick: true,
                }
            }),
            args.pointer.refs.strict_refs,
            &args.pointer.after,
            &cli.global,
        ),
        Some(Command::Hover(args)) => session::act(
            pointer_action(&args.pointer, |locator, modifiers, position, force| {
                engine::Action::Hover {
                    locator,
                    modifiers,
                    position,
                    force,
                }
            }),
            args.pointer.refs.strict_refs,
            &args.pointer.after,
            &cli.global,
        ),
        Some(Command::Tap(args)) => session::act(
            pointer_action(&args.pointer, |locator, modifiers, position, force| {
                engine::Action::Tap {
                    locator,
                    modifiers,
                    position,
                    force,
                }
            }),
            args.pointer.refs.strict_refs,
            &args.pointer.after,
            &cli.global,
        ),
        Some(Command::Drag(args)) => session::act(
            engine::Action::Drag {
                from: args.from.clone(),
                to: args.to.clone(),
                steps: args.steps,
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Scroll(args)) => session::act(
            engine::Action::Scroll {
                locator: args.locator.clone(),
                by: args.by,
                to: args.to,
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Mouse(args)) => session::act(
            engine::Action::Mouse(mouse_op(&args.command)),
            false,
            &args.after,
            &cli.global,
        ),
        Some(Command::Keyboard(args)) => session::act(
            engine::Action::Keyboard(match &args.command {
                KeyboardCommand::Down(down) => engine::KeyboardOp::Down {
                    key: down.key.clone(),
                },
                KeyboardCommand::Up(up) => engine::KeyboardOp::Up {
                    key: up.key.clone(),
                },
            }),
            false,
            &args.after,
            &cli.global,
        ),
        Some(Command::Press(args)) => session::act(
            engine::Action::Press {
                keys: args.keys.clone(),
                locator: args.locator.clone(),
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Type(args)) => session::act(
            engine::Action::Type {
                text: line_arg(&args.text, "TEXT")?,
                locator: args.locator.clone(),
                delay_ms: args
                    .delay
                    .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Fill(args)) => session::act(
            engine::Action::Fill {
                locator: args.locator.clone(),
                text: line_arg(&args.text, "TEXT")?,
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Select(args)) => session::act(
            engine::Action::Select {
                locator: args.locator.clone(),
                values: args.values.clone(),
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Check(args)) => session::act(
            engine::Action::Check {
                locator: args.locator.clone(),
                checked: true,
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Uncheck(args)) => session::act(
            engine::Action::Check {
                locator: args.locator.clone(),
                checked: false,
                force: args.force,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Focus(args)) => session::act(
            engine::Action::Focus {
                locator: args.locator.clone(),
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Blur(args)) => session::act(
            engine::Action::Blur {
                locator: args.locator.clone(),
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        Some(Command::Upload(args)) => session::act(
            engine::Action::Upload {
                locator: args.locator.clone(),
                files: args
                    .files
                    .iter()
                    .map(|f| input::absolute_path(f))
                    .collect::<Result<_, _>>()?,
                clear: args.clear,
                force: args.force,
                drop: args.drop,
            },
            args.refs.strict_refs,
            &args.after,
            &cli.global,
        ),
        // Handled in `main`; a script cannot hold them (`script` checks).
        Some(Command::Daemon(_)) => Ok(Output::new(serde_json::Value::Null, "")),
        Some(Command::Lint(args)) => lint::output(&args, &cli.global),
        Some(Command::Batch(_) | Command::Run(_) | Command::Test(_)) => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            "batch, run and test cannot run inside a script",
        )),
    }
}

/// A text argument, or stdin for `-` without the line's end (`echo`
/// adds one).
fn line_arg(value: &str, what: &str) -> Result<String, ErrorInfo> {
    let text = input::text_arg(value, what)?;
    Ok(if value == "-" {
        text.strip_suffix('\n')
            .map_or(text.as_str(), |t| t.strip_suffix('\r').unwrap_or(t))
            .to_owned()
    } else {
        text
    })
}

/// Reports a clap error (or prints `--help`) and returns the exit code.
/// A usage error is a `BadArgs` error like any other: the §3 text form on
/// stderr, or an envelope on stdout with `--json`.
fn usage_error(e: &clap::Error) -> u8 {
    if matches!(
        e.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    ) {
        let _ = e.print();
        return u8::try_from(e.exit_code()).unwrap_or(0);
    }
    let mode = if json_requested() {
        Mode::Json
    } else {
        Mode::Text
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    output::print(mode, Err(usage_info(e, &args)))
}

/// Turns clap's error text into an [`ErrorInfo`]: its first line is the
/// message, its `tip:` lines ("a similar argument exists: '--json'") are
/// notes, and the hint is the command line with the mistyped subcommand or
/// flag corrected, else the failing command's `--help`. `args` are the
/// arguments clap parsed, without the program name.
fn usage_info(e: &clap::Error, args: &[String]) -> ErrorInfo {
    let rendered = e.render().to_string();
    let command = rendered
        .lines()
        .find_map(|l| l.strip_prefix("Usage: "))
        // An invalid value's error shows no usage.
        .map_or_else(|| typed_command(args), command_path);
    let missing = [
        ClapErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
        ClapErrorKind::MissingSubcommand,
    ];
    let message = if missing.contains(&e.kind()) {
        format!("{} needs a subcommand", Cmd::new(&command))
    } else {
        let mut lines = rendered.lines();
        let first = lines
            .next()
            .unwrap_or_default()
            .trim_start_matches("error: ");
        if first.ends_with(':') {
            // "the following required arguments were not provided:" lists
            // them on the lines below.
            let listed: Vec<&str> = lines
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect();
            format!("{first} {}", listed.join(", "))
        } else {
            first.to_owned()
        }
    };
    let tips = rendered
        .lines()
        .filter_map(|l| l.trim().strip_prefix("tip: "));
    let info = tips.fold(ErrorInfo::new(ErrorKind::BadArgs, message), |info, tip| {
        info.note("tip", tip)
    });
    match corrected(e, args) {
        Some(fixed) => info.hint(fixed),
        None => info.hint(Cmd::new(&command).switch("help")),
    }
}

/// The command line with its mistyped subcommand or flag replaced by the
/// one clap suggests (`clik e1` → `chrome-cdp click e1`), when one
/// suggestion is closest.
fn corrected(e: &clap::Error, args: &[String]) -> Option<Cmd> {
    let (typo, suggestion) = match e.kind() {
        ClapErrorKind::InvalidSubcommand => (
            ContextKind::InvalidSubcommand,
            ContextKind::SuggestedSubcommand,
        ),
        ClapErrorKind::UnknownArgument => (ContextKind::InvalidArg, ContextKind::SuggestedArg),
        _ => return None,
    };
    let Some(ContextValue::String(typo)) = e.get(typo) else {
        return None;
    };
    let suggestion = match e.get(suggestion)? {
        ContextValue::String(one) => one.as_str(),
        // clap's matching is loose (`scheme`: check, schema); keep the
        // closest.
        ContextValue::Strings(list) => {
            match protocol::suggest::similar(typo, list.iter().map(String::as_str), 2)[..] {
                [one] => one,
                _ => return None,
            }
        }
        _ => return None,
    };
    // clap also suggests `-- --jsn`, to pass the flag as a value.
    if suggestion.contains(' ') {
        return None;
    }
    let at = args.iter().position(|a| a == typo)?;
    Some(args.iter().enumerate().fold(Cmd::new(""), |cmd, (i, arg)| {
        cmd.arg(if i == at { suggestion } else { arg })
    }))
}

/// `chrome-cdp tab close [OPTIONS] <N>` → `tab close`.
fn command_path(usage: &str) -> String {
    usage
        .split_whitespace()
        .skip(1)
        .take_while(|w| !w.starts_with(['[', '<', '-']))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The subcommand typed (`mouse move`), from the raw arguments: the words
/// that name a subcommand of the one before (values of flags before it do
/// not).
fn typed_command(args: &[String]) -> String {
    let mut cmd = Cli::command();
    let mut path = Vec::new();
    for arg in args.iter().take_while(|a| *a != "--") {
        if arg.starts_with('-') {
            continue;
        }
        let Some(sub) = cmd.find_subcommand(arg).cloned() else {
            if path.is_empty() {
                continue;
            }
            break;
        };
        path.push(arg.as_str());
        cmd = sub;
    }
    path.join(" ")
}

/// Whether `--json` was asked for, from the raw arguments (clap failed to
/// parse them) or `CHROME_CDP_JSON`.
fn json_requested() -> bool {
    let flag = std::env::args_os()
        .skip(1)
        .take_while(|a| a != "--")
        .any(|a| a == "--json");
    let env = std::env::var("CHROME_CDP_JSON").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "y"
        )
    });
    flag || env
}

/// The action of `click`, `dblclick`, `hover` or `tap`, from their shared
/// arguments.
fn pointer_action(
    args: &cli::PointerArgs,
    action: impl FnOnce(String, Vec<engine::Modifier>, Option<(f64, f64)>, bool) -> engine::Action,
) -> engine::Action {
    action(
        args.locator.clone(),
        args.modifiers.clone().unwrap_or_default().0,
        args.position,
        args.force,
    )
}

fn mouse_op(command: &cli::MouseCommand) -> engine::MouseOp {
    let at = |x: Option<f64>, y: Option<f64>| x.zip(y);
    match command {
        cli::MouseCommand::Move(a) => engine::MouseOp::Move {
            x: a.x,
            y: a.y,
            steps: a.steps,
        },
        cli::MouseCommand::Down(a) => engine::MouseOp::Down {
            at: at(a.x, a.y),
            button: a.button.unwrap_or_default().to_button(),
            count: a.count,
        },
        cli::MouseCommand::Up(a) => engine::MouseOp::Up {
            at: at(a.x, a.y),
            button: a.button.map(cli::ButtonArg::to_button),
            count: a.count,
        },
        cli::MouseCommand::Wheel(a) => engine::MouseOp::Wheel { dx: a.dx, dy: a.dy },
    }
}

#[cfg(test)]
mod tests {
    use super::command_path;

    #[test]
    fn command_path_from_usage() {
        assert_eq!(
            command_path("chrome-cdp cdp --json <Domain.method> [JSON]"),
            "cdp"
        );
        assert_eq!(command_path("chrome-cdp [OPTIONS] [COMMAND]"), "");
        assert_eq!(
            command_path("chrome-cdp schema [OPTIONS] [COMMAND]..."),
            "schema"
        );
    }
}

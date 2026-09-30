//! `chrome-cdp lint [paths…]` (plan 11, milestone 11.12): checks scenario
//! files, step files and mock files without running anything.
//!
//! A scenario is read as `test` reads it ([`e2e::scenario::read`], which
//! also keeps what it could read of a file with directive errors), and
//! each step checked as `test` checks it ([`crate::script::step`]: clap,
//! locators), for every backend that runs it (its `${var:…}` values).
//! On top come the rules only a linter has: refs, sleeps, tests without
//! an `expect`, duplicate test names, mock-only commands in tests real
//! backends run, and values of the mock data written into such tests.
//! Mock files get `mock check`'s rules. Every problem is a
//! [`LintProblem`]: file, line, column, severity, rule, message, hint.
//!
//! The plan calls it `check`, which is taken: `chrome-cdp check <loc>`
//! ticks a checkbox (plan 05).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::CommandFactory as _;
use e2e::Config;
use e2e::backend::Mode as BackendMode;
use e2e::scenario::{self, Scenario, Step, Vars};
use protocol::schema::{LintData, LintProblem, MockProblem};
use protocol::script::{self, Item, Line, Resolve, ScriptError, Span, Unresolved, Var};
use protocol::{ErrorInfo, ErrorKind, Hint, Output};

use crate::cli::{Cli, Command, GlobalArgs, LintArgs, LintFormat};
use crate::{e2e_files, output};

/// Shortest mock data value `mock-literal` looks for: shorter ones (ids,
/// `ok`) are everywhere.
const MIN_LITERAL: usize = 4;

/// `lint` from the shell: prints the problems (as annotations with
/// `--format github`) and returns the exit code.
pub(crate) fn run(args: &LintArgs, global: &GlobalArgs) -> u8 {
    let mut global = global.clone();
    if args.format == LintFormat::Json {
        global.json = true;
        global.quiet = false;
    }
    let mode = global.mode();
    let data = match lint(args, &global) {
        Ok(data) => data,
        Err(info) => return output::print(mode, Err(info)),
    };
    if args.format == LintFormat::Github {
        output::stdout(&github(&data));
        output::stderr(&format!("{}\n", summary(&data)));
        return u8::from(data.errors > 0);
    }
    output::print(mode, result(&data, args.max_chars))
}

/// `lint` as a script step: the usual output.
pub(crate) fn output(args: &LintArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let data = lint(args, global)?;
    result(&data, args.max_chars)
}

fn lint(args: &LintArgs, global: &GlobalArgs) -> Result<LintData, ErrorInfo> {
    let files = e2e_files::find(&args.paths)?;
    let mut linter = Linter::new(&files.config, global, None)?;
    for path in &files.cdp {
        linter.file(path);
    }
    let mut problems = linter.problems;
    if !files.mocks.is_empty() {
        let openapi = args
            .openapi
            .as_ref()
            .map(|p| std::env::current_dir().unwrap_or_default().join(p));
        let checked = crate::mock::check_files(&files.config, &files.mocks, openapi.as_deref())?;
        problems.extend(checked.problems.iter().map(from_mock));
    }
    for path in &files.audits {
        problems.extend(audit_baseline(path));
    }
    Ok(data(
        files
            .cdp
            .iter()
            .chain(&files.mocks)
            .chain(&files.audits)
            .map(PathBuf::as_path),
        problems,
    ))
}

/// The problem of audit baseline `path`, if it has one: not JSON, a field
/// that does not exist, a bad version or key, a key twice.
fn audit_baseline(path: &Path) -> Option<LintProblem> {
    let problem = |line, col, message: String| LintProblem {
        file: normal(&absolute(path)).display().to_string(),
        line,
        col,
        severity: "error".to_owned(),
        rule: "audit-baseline".to_owned(),
        message,
        hint: Some("chrome-cdp schema audit-baseline has its format".to_owned()),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => return Some(problem(1, 1, format!("cannot read it: {e}"))),
    };
    engine::parse_audit_baseline(&text)
        .err()
        .map(|p| problem(p.line, p.column, p.message))
}

/// The lint `test` runs before it starts: `scenarios`' rules for backend
/// `backend` only (mock files are checked by the run's `mock check`).
pub(crate) fn for_run(
    config: &Config,
    scenarios: &[Scenario],
    backend: &str,
    global: &GlobalArgs,
) -> Result<LintData, ErrorInfo> {
    let mut linter = Linter::new(config, global, Some(backend))?;
    for s in scenarios {
        linter.scenario(s, true);
    }
    let problems = linter.problems;
    Ok(data(scenarios.iter().map(|s| s.path.as_path()), problems))
}

fn data<'a>(files: impl Iterator<Item = &'a Path>, mut problems: Vec<LintProblem>) -> LintData {
    let mut seen = HashSet::new();
    problems.retain(|p| {
        seen.insert((
            p.file.clone(),
            p.line,
            p.col,
            p.rule.clone(),
            p.message.clone(),
        ))
    });
    problems.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    let errors = problems.iter().filter(|p| p.severity == "error").count();
    LintData {
        files: files.map(|p| absolute(p).display().to_string()).collect(),
        errors,
        warnings: problems.len() - errors,
        problems,
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(path)
}

/// `path` without `.` and `..` parts (`e2e/scenarios/../steps/a.cdp` is
/// `e2e/steps/a.cdp`).
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn from_mock(p: &MockProblem) -> LintProblem {
    LintProblem {
        file: p.file.clone(),
        line: p.line,
        col: p.col,
        severity: p.severity.clone(),
        rule: p.rule.clone(),
        message: p.message.clone(),
        hint: p.hint.clone(),
    }
}

/// The output of a lint: the problems and a summary, or, with errors, the
/// first as the error and the rest as notes.
fn result(data: &LintData, max_chars: usize) -> Result<Output, ErrorInfo> {
    if data.errors > 0 {
        return Err(failed(data, "lint"));
    }
    let mut text = String::new();
    for p in &data.problems {
        let _ = writeln!(text, "{}", line(p));
        if let Some(hint) = &p.hint {
            let _ = writeln!(text, "  hint: {hint}");
        }
    }
    text.push_str(&summary(data));
    Ok(Output::typed(data, text)
        .quiet(format!("{} {}", data.errors, data.warnings))
        .limit(max_chars, "--max-chars 0 for all of it, or --json"))
}

/// The error of a lint with errors; `command` reruns it.
pub(crate) fn failed(data: &LintData, command: &str) -> ErrorInfo {
    const LISTED: usize = 10;
    // Errors first: the message is an error.
    let mut sorted: Vec<&LintProblem> = data.problems.iter().collect();
    sorted.sort_by_key(|p| p.severity != "error");
    let mut problems = sorted.into_iter();
    let first = problems.next();
    let mut info = ErrorInfo::new(ErrorKind::ExpectFailed, first.map(line).unwrap_or_default());
    if let Some(hint) = first.and_then(|p| p.hint.clone()) {
        info = info.note("fix", hint);
    }
    let rest: Vec<&LintProblem> = problems.collect();
    for p in rest.iter().take(LISTED) {
        info = info.note("also", line(p));
    }
    if rest.len() > LISTED {
        info = info.note("also", format!("{} more (see --json)", rest.len() - LISTED));
    }
    info.note("checked", summary(data))
        .hint(Hint::text(format!(
            "fix the errors, then run chrome-cdp {command} again"
        )))
        .details(serde_json::to_value(data).unwrap_or_default())
}

/// `file:12:5: error [rule] message`, the file relative to the current
/// directory when below it.
fn line(p: &LintProblem) -> String {
    let file = e2e_files::shown(Path::new(&p.file));
    let at = if p.line == 0 {
        file
    } else {
        format!("{file}:{}:{}", p.line, p.col)
    };
    format!("{at}: {} [{}] {}", p.severity, p.rule, p.message)
}

pub(crate) fn summary(data: &LintData) -> String {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    format!(
        "{}: {}, {}",
        plural(data.files.len(), "file", "files"),
        plural(data.errors, "error", "errors"),
        plural(data.warnings, "warning", "warnings"),
    )
}

/// GitHub workflow annotations: `::error file=…,line=…,col=…,title=…::…`.
fn github(data: &LintData) -> String {
    let escape_data = |s: &str| {
        s.replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
    };
    let escape_property = |s: &str| escape_data(s).replace(':', "%3A").replace(',', "%2C");
    let mut out = String::new();
    for p in &data.problems {
        let file = e2e_files::shown(Path::new(&p.file));
        let mut props = format!("file={}", escape_property(&file));
        if p.line > 0 {
            let _ = write!(props, ",line={},col={}", p.line, p.col);
        }
        let _ = write!(
            props,
            ",title={}",
            escape_property(&format!("chrome-cdp lint: {}", p.rule))
        );
        let mut message = p.message.clone();
        if let Some(hint) = &p.hint {
            let _ = write!(message, "\nhint: {hint}");
        }
        let _ = writeln!(out, "::{} {props}::{}", p.severity, escape_data(&message));
    }
    out
}

/// A backend a test may run on, and its `${var:…}` values.
struct Backend {
    name: String,
    real: bool,
    values: BTreeMap<String, String>,
    /// Its vars files, for hints.
    files: String,
}

struct Linter {
    /// Steps are checked as in a text script: `--json` on a line is an
    /// error whatever `lint` prints.
    global: GlobalArgs,
    backends: Vec<Backend>,
    /// Every backend's name, for `@backend`.
    known: Vec<String>,
    /// String values of the mock data files: `(value, file)`.
    data: Vec<(String, String)>,
    problems: Vec<LintProblem>,
}

impl Linter {
    /// A linter for `config`'s backends (only `only` when given).
    fn new(config: &Config, global: &GlobalArgs, only: Option<&str>) -> Result<Self, ErrorInfo> {
        let mut global = global.clone();
        global.json = false;
        global.quiet = false;
        let known = config.backend_names();
        let mut backends = Vec::new();
        for name in &known {
            if only.is_some_and(|o| o != name) {
                continue;
            }
            let selected = config.backend(name)?;
            let vars = e2e::backend::Vars::load(&selected.vars)?;
            backends.push(Backend {
                name: name.clone(),
                real: selected.mode == BackendMode::Real,
                files: vars.describe(),
                values: vars.values,
            });
        }
        let vars: BTreeSet<&String> = backends.iter().flat_map(|b| b.values.values()).collect();
        let has_real = known.len() > 1;
        let mut data = Vec::new();
        if has_real {
            mock_data(&config.mocks, &config.mocks, &mut data);
            data.retain(|(value, _)| !vars.contains(value));
        }
        Ok(Self {
            global,
            backends,
            known,
            data,
            problems: Vec::new(),
        })
    }

    fn push(
        &mut self,
        span: &Span,
        severity: &str,
        rule: &str,
        message: impl Into<String>,
        hint: Option<String>,
    ) {
        self.problems.push(LintProblem {
            file: normal(&absolute(Path::new(span.file.as_ref())))
                .display()
                .to_string(),
            line: span.line as usize,
            col: span.column as usize,
            severity: severity.to_owned(),
            rule: rule.to_owned(),
            message: message.into(),
            hint,
        });
    }

    fn error(
        &mut self,
        span: &Span,
        rule: &str,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.push(span, "error", rule, message, Some(hint.into()));
    }

    fn warning(
        &mut self,
        span: &Span,
        rule: &str,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.push(span, "warning", rule, message, Some(hint.into()));
    }

    fn script_error(&mut self, e: &ScriptError, rule: &str) {
        self.push(&e.span, "error", rule, e.message.clone(), e.hint.clone());
    }

    /// A `.cdp` file: a scenario when it has a `@test`, else a step file
    /// or script.
    fn file(&mut self, path: &Path) {
        let name = e2e_files::shown(path);
        let src = match std::fs::read_to_string(path) {
            Ok(src) => src,
            Err(e) => {
                let span = start_of(&name);
                self.error(
                    &span,
                    "syntax",
                    format!("cannot read {name}: {e}"),
                    "check the file's permissions",
                );
                return;
            }
        };
        let parsed = script::parse(&name, &src);
        for e in &parsed.errors {
            self.script_error(e, "syntax");
        }
        let has_test = parsed
            .script
            .items
            .iter()
            .any(|i| matches!(i, Item::Directive(d) if d.name == "test"));
        // Lines with syntax errors are not in the tree: what else is
        // wrong would be guesses.
        if !parsed.errors.is_empty() {
            return;
        }
        if !has_test {
            self.step_file(path, &parsed.script.items);
            return;
        }
        // A file with directive errors still gets its steps checked: what
        // `read` could make of it.
        let (s, errors) = scenario::read(path, &name);
        // A cycle leaves the step files in it without steps; that is the
        // same problem.
        let cycle = errors
            .iter()
            .any(|e| e.message.starts_with("@include cycle"));
        for e in &errors {
            if cycle && e.message.ends_with("has no steps") {
                continue;
            }
            self.script_error(e, load_rule(e));
        }
        self.scenario(&s, errors.is_empty());
    }

    /// A file without tests: a step file (`@include`d) or a script of
    /// `run`. Its `${name}` values come from whoever includes or runs it,
    /// so every variable gets a stand-in; the scenarios including it
    /// check it with their values.
    fn step_file(&mut self, path: &Path, items: &[Item]) {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        for item in items {
            match item {
                Item::Comment(_) => {}
                Item::Directive(d) if d.name == "include" => {
                    if let Some(file) = d.args.first().and_then(script::Word::literal)
                        && !dir.join(&file).is_file()
                    {
                        let span = d.args[0].span.clone();
                        self.error(
                            &span,
                            "include-missing",
                            format!("cannot read {}", dir.join(&file).display()),
                            "@include paths are relative to the file that includes them",
                        );
                    }
                }
                Item::Directive(d) => {
                    let span = d.span.clone();
                    self.error(
                        &span,
                        "unknown-directive",
                        format!("@{} in a step file", d.name),
                        "a step file (or run script) holds commands and @include lines only; @test and settings go in scenarios",
                    );
                }
                Item::Command(line) => {
                    let vars = Arc::new(Vars {
                        params: BTreeMap::new(),
                        dir: dir.clone(),
                        include: None,
                    });
                    let step = Step {
                        line: line.clone(),
                        vars,
                        via: Vec::new(),
                    };
                    self.step(&step, &[], &BTreeSet::new(), false, true);
                }
            }
        }
    }

    /// Checks the tests and hooks of `s`. When it was not read `complete`ly
    /// (it has directive errors), a test's missing steps may hold its
    /// `expect`, so `no-expect` is left out.
    fn scenario(&mut self, s: &Scenario, complete: bool) {
        let saved = saved_names(s);
        let mut names: BTreeMap<&str, &Span> = BTreeMap::new();
        let mut file_real = false;
        let mut file_backends: BTreeSet<usize> = BTreeSet::new();
        for (index, test) in s.tests.iter().enumerate() {
            let settings = s.settings_of(index);
            if let Some(first) = names.get(test.name.as_str()) {
                let at = format!("{}:{}", first.file, first.line);
                self.error(
                    &test.span,
                    "duplicate-test",
                    format!(
                        "a second test named \"{}\" (the first is at {at})",
                        test.name
                    ),
                    "rename one: --grep, reports and artifacts tell tests apart by name",
                );
            } else {
                names.insert(&test.name, &test.span);
            }
            for word in &settings.backend {
                if !matches!(word.as_str(), "mock" | "real") && !self.known.contains(word) {
                    let known = self.known.join(", ");
                    let close =
                        protocol::suggest::similar(word, self.known.iter().map(String::as_str), 1);
                    let hint = match close.first() {
                        Some(name) => format!("did you mean @backend {name}?"),
                        None => {
                            "write @backend mock, @backend real, or a backend's name".to_owned()
                        }
                    };
                    self.error(
                        &test.span,
                        "unknown-backend",
                        format!("@backend {word}: no such backend; the backends are {known}"),
                        hint,
                    );
                }
            }
            if let Some(device) = &settings.device
                && let Err(info) = engine::find_device(device, &self.global.session)
            {
                let hint = info
                    .hint
                    .clone()
                    .unwrap_or_else(|| "chrome-cdp emulate list shows the devices".into());
                self.error(&test.span, "unknown-device", info.message, hint);
            }
            let runs: Vec<usize> = self
                .backends
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    settings
                        .skipped_on(&b.name, if b.real { "real" } else { "mock" })
                        .is_none()
                })
                .map(|(i, _)| i)
                .collect();
            let real = runs.iter().any(|&i| self.backends[i].real);
            file_real |= real;
            file_backends.extend(runs.iter().copied());
            let mut expects = false;
            for step in &test.steps {
                let facts = self.step(step, &runs, &saved, real, false);
                expects |= facts.expect;
            }
            if !expects && complete && settings.skip.is_none() && !test.steps.is_empty() {
                self.warning(
                    &test.span,
                    "no-expect",
                    format!(
                        "test \"{}\" has no expect step, so it only fails when a step errors",
                        test.name
                    ),
                    "end it with what should be true, e.g. expect text=Saved visible",
                );
            }
        }
        let runs: Vec<usize> = file_backends.into_iter().collect();
        for hook in [&s.before_all, &s.before, &s.after, &s.after_all] {
            for step in hook {
                self.step(step, &runs, &saved, file_real, false);
            }
        }
    }

    /// Checks `step` for the backends `runs` (indexes into
    /// `self.backends`); `real` says whether a real backend runs it. The
    /// variables of a `standalone` line (a step file's) are not checked:
    /// their values come from whoever includes it.
    fn step(
        &mut self,
        step: &Step,
        runs: &[usize],
        saved: &BTreeSet<String>,
        real: bool,
        standalone: bool,
    ) -> Facts {
        let mut facts = Facts::default();
        let line = &step.line;
        let mut bad_vars = false;
        if !standalone {
            for word in &line.words {
                for v in word.vars() {
                    bad_vars |= self.var(&v.var, &v.span, step, runs, saved);
                }
            }
        }
        if bad_vars {
            return facts;
        }
        let resolver = Values {
            vars: &step.vars,
            backends: &self.backends,
            runs,
        };
        let checked = match crate::script::step(line, &resolver, &self.global) {
            Ok(checked) => checked,
            Err(problem) => {
                let words: Vec<String> = line
                    .words
                    .iter()
                    .filter_map(|w| w.expand(&resolver).ok())
                    .collect();
                let rule = clap_rule(&words, &problem.info);
                let hint = problem.info.hint.clone();
                self.push(
                    &problem.span,
                    "error",
                    rule,
                    problem.info.message.clone(),
                    hint,
                );
                return facts;
            }
        };
        let Ok(matches) = Cli::command().try_get_matches_from(
            std::iter::once(protocol::PROGRAM.to_owned()).chain(checked.words.clone()),
        ) else {
            return facts;
        };
        facts.expect = matches
            .subcommand_name()
            .is_some_and(|n| n.starts_with("expect"));
        let mut locators = crate::script::locator_values(&matches);
        if let Some(Command::Wait(args)) = &checked.cli.command
            && let Some(target) = &args.target
        {
            if let Some(ms) = crate::session::delay_ms(target) {
                self.warning(
                    &line.span,
                    "sleep",
                    format!("`{}` sleeps {ms}ms whatever the page does: slow when it is quick, flaky when it is slow", line.text),
                    "wait for what the sleep stands for: wait <locator>, wait --text …, wait --url …, or wait --idle",
                );
            } else {
                locators.push(target.clone());
            }
        }
        for value in locators {
            if engine::parse_locator(&value).is_ok_and(|l| l.has_ref()) {
                self.error(
                    &line.span,
                    "ref",
                    format!("`{value}` is a ref: refs name elements of one session's snapshot, so they break in a scenario"),
                    "use a stable locator such as role=button[name=\"Save\"], label=Email or text=Sign in; `chrome-cdp history --as-scenario` writes them for the refs of a session",
                );
            }
        }
        if let Some(command) = crate::test::mock_only(&checked.cli)
            && real
        {
            self.warning(
                &line.span,
                "mock-only",
                format!("{command} needs the mocks, and a real backend runs this test"),
                "mark the test (or the file) @backend mock, so real backends skip it",
            );
        }
        if real {
            self.literals(line);
        }
        facts
    }

    /// Checks one variable; whether it has no value somewhere.
    fn var(
        &mut self,
        var: &Var,
        span: &Span,
        step: &Step,
        runs: &[usize],
        saved: &BTreeSet<String>,
    ) -> bool {
        match var {
            Var::Var(name) => {
                let missing: Vec<usize> = runs
                    .iter()
                    .copied()
                    .filter(|&i| !self.backends[i].values.contains_key(name))
                    .collect();
                for i in &missing {
                    let b = &self.backends[*i];
                    let (message, hint) = (
                        format!("{var} is not set for backend {}", b.name),
                        if b.real {
                            format!(
                                "add `{name} = \"…\"` to {} (vars in [e2e] or [e2e.backends.{}])",
                                b.files, b.name
                            )
                        } else {
                            format!("add `{name} = \"…\"` to {} (vars in [e2e])", b.files)
                        },
                    );
                    self.error(span, "undefined-var", message, hint);
                }
                !missing.is_empty()
            }
            Var::Env(_) | Var::Secret(_) => false,
            Var::Name(name) if name == "backend" => false,
            Var::Path(path) => match path.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                ["run" | "test", "id"] => false,
                ["saved", name, ..] if saved.contains(name) => false,
                ["saved", name, ..] => {
                    self.error(
                        span,
                        "undefined-var",
                        format!("{var}: no http step of the file saves {name}"),
                        format!("add --save {name} to the http step whose answer it is"),
                    );
                    true
                }
                _ => {
                    self.error(
                        span,
                        "undefined-var",
                        format!("unknown variable {var}"),
                        "the dotted variables are ${run.id}, ${test.id} and ${saved.NAME.field}",
                    );
                    true
                }
            },
            Var::Name(_) | Var::Data { .. } => match step.vars.resolve(var) {
                Ok(_) => false,
                Err(u) => {
                    let rule = if matches!(var, Var::Name(_)) && step.vars.include.is_some() {
                        "include-param"
                    } else {
                        "undefined-var"
                    };
                    self.push(span, "error", rule, u.message, u.hint);
                    true
                }
            },
        }
    }

    /// `mock-literal`: values of the mock data written into a step of a
    /// test that real backends run.
    fn literals(&mut self, line: &Line) {
        let mut found = Vec::new();
        for word in &line.words {
            for part in &word.parts {
                let script::Part::Text(text) = part else {
                    continue;
                };
                for (value, file) in &self.data {
                    if text.contains(value.as_str())
                        && !found.iter().any(|(v, _): &(String, String)| v == value)
                    {
                        found.push((value.clone(), file.clone()));
                    }
                }
            }
        }
        for (value, file) in found {
            self.warning(
                &line.span,
                "mock-literal",
                format!("`{value}` comes from the mock data ({file}), and a real backend runs this test, whose data may differ"),
                "put the value in the vars files (mock's and each real backend's) and write ${var:name}, or mark the test @backend mock",
            );
        }
    }
}

/// What a checked step is.
#[derive(Default)]
struct Facts {
    /// An `expect…` command.
    expect: bool,
}

/// Values for checking a step: the scenario's own, the first backend's
/// `${var:…}` that has it, and stand-ins for the environment and what
/// only a run knows.
struct Values<'a> {
    vars: &'a Vars,
    backends: &'a [Backend],
    runs: &'a [usize],
}

impl Resolve for Values<'_> {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        match var {
            Var::Var(name) => Ok(self
                .runs
                .iter()
                .find_map(|&i| self.backends[i].values.get(name).cloned())
                .unwrap_or_else(|| "x".to_owned())),
            Var::Env(_) | Var::Secret(_) | Var::Path(_) => Ok("x".to_owned()),
            Var::Name(name) if name == "backend" && !self.vars.params.contains_key(name) => {
                Ok("mock".to_owned())
            }
            // A step file's parameter, or a missing one (already reported).
            Var::Name(name) if !self.vars.params.contains_key(name) => Ok("x".to_owned()),
            Var::Data { .. } => Ok(self.vars.resolve(var).unwrap_or_else(|_| "x".to_owned())),
            Var::Name(_) => self.vars.resolve(var),
        }
    }
}

/// The rule of a step clap or the script checker refused.
fn clap_rule(words: &[String], info: &ErrorInfo) -> &'static str {
    if info.kind == ErrorKind::BadLocator {
        return "locator";
    }
    match Cli::command()
        .try_get_matches_from(std::iter::once(protocol::PROGRAM.to_owned()).chain(words.to_vec()))
    {
        Err(e) => match e.kind() {
            clap::error::ErrorKind::InvalidSubcommand => "unknown-command",
            clap::error::ErrorKind::UnknownArgument => "unknown-flag",
            _ => "bad-args",
        },
        Ok(_) => "bad-args",
    }
}

/// The rule of a problem `scenario::read` found.
fn load_rule(e: &ScriptError) -> &'static str {
    let hint = e.hint.as_deref().unwrap_or("");
    if e.message.starts_with("unknown directive") {
        "unknown-directive"
    } else if e.message.starts_with("@include cycle") {
        "include-cycle"
    } else if e.message.starts_with("cannot read") && hint.contains("@include") {
        "include-missing"
    } else if e.message.ends_with("is not set") && hint.contains("@include line") {
        "include-param"
    } else if e.message.ends_with("is not set") {
        "undefined-var"
    } else {
        "syntax"
    }
}

/// The names the file's `http … --save NAME` steps save.
fn saved_names(s: &Scenario) -> BTreeSet<String> {
    crate::test::vars_saved_names(s)
}

fn start_of(file: &str) -> Span {
    Span {
        file: file.into(),
        line: 1,
        column: 1,
        start: 0,
        end: 0,
    }
}

/// The string values of the JSON data files below `dir`, with their files
/// relative to `mocks`.
fn mock_data(dir: &Path, mocks: &Path, out: &mut Vec<(String, String)>) {
    fn strings(value: &serde_json::Value, out: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::String(s) => {
                let s = s.trim();
                if s.chars().count() >= MIN_LITERAL
                    && s.chars().any(char::is_alphabetic)
                    && !s.contains("{{")
                {
                    out.insert(s.to_owned());
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
            serde_json::Value::Object(map) => map.values().for_each(|v| strings(v, out)),
            _ => {}
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            mock_data(&path, mocks, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(value) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            continue;
        };
        let mut found = BTreeSet::new();
        strings(&value, &mut found);
        let file = path
            .strip_prefix(mocks)
            .unwrap_or(&path)
            .display()
            .to_string();
        out.extend(found.into_iter().map(|s| (s, file.clone())));
    }
}

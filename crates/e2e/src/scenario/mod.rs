//! Scenario files (plan 11, milestone 11.4): `.cdp` scripts (plan 08 §8.2)
//! with test directives.
//!
//! ```text
//! @mocks base checkout             # layered onto default_mocks
//! @tags smoke
//!
//! @before
//!   goto /
//!
//! @test "lists products"
//!   expect 'role=listitem' count 3
//!
//! @test "empty cart" @skip "flaky upstream component"
//!   click text=Cart
//! ```
//!
//! [`load`] reads one file into a [`Scenario`]: its settings (file-level
//! `@mocks`, `@tags`, `@skip`, `@only`, `@timeout`, `@viewport`,
//! `@device`, `@retries`, `@allow-console-errors`, `@backend`,
//! `@safe`), its hooks (`@before`, `@after`, `@before-all`,
//! `@after-all`) and its tests, each with its own settings (written in its
//! body or after its name) and steps. `@include file k=v` inlines a step
//! file (commands and further `@include`s only), relative to the file that
//! includes it; its lines see the parameters as `${k}`.
//!
//! Steps stay unexpanded [`Line`]s with the [`Vars`] that fill them in, so
//! the runner shows each line as written and checks every one against the
//! CLI's commands before anything runs. [`select`] picks the tests of a
//! run: paths and `file.cdp:line`, `--grep`, `--tags`, `@only`, `@skip`
//! and `--shard`.

pub mod format;
pub mod select;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use protocol::format::parse_duration;
use protocol::script::{self, Directive, Item, Line, Resolve, ScriptError, Span, Unresolved, Var};
use protocol::suggest;

use crate::config::Viewport;

/// Every directive a scenario knows.
pub const DIRECTIVES: &[&str] = &[
    "test",
    "before",
    "after",
    "before-all",
    "after-all",
    "include",
    "mocks",
    "tags",
    "skip",
    "only",
    "timeout",
    "viewport",
    "device",
    "retries",
    "allow-console-errors",
    "backend",
    "safe",
    "syntax",
];

/// What a directive is, for `chrome-cdp schema scenario`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectiveDoc {
    pub name: &'static str,
    /// How it is written.
    pub usage: &'static str,
    /// `block` (indented steps below it), `setting` (for the file, or a
    /// test when in its body or after its name), `include` or `syntax`.
    pub kind: &'static str,
    pub summary: &'static str,
}

/// Every directive of [`DIRECTIVES`], documented.
pub const DIRECTIVE_DOCS: &[DirectiveDoc] = &[
    DirectiveDoc {
        name: "test",
        usage: "@test \"name\" [@setting args…]…",
        kind: "block",
        summary: "a test: its steps are indented 2 spaces below; settings may follow the name",
    },
    DirectiveDoc {
        name: "before",
        usage: "@before",
        kind: "block",
        summary: "steps run before each test of the file, in the test's browser context",
    },
    DirectiveDoc {
        name: "after",
        usage: "@after",
        kind: "block",
        summary: "steps run after each test of the file, failed ones too",
    },
    DirectiveDoc {
        name: "before-all",
        usage: "@before-all",
        kind: "block",
        summary: "steps run once before the file's first test, in a context of their own",
    },
    DirectiveDoc {
        name: "after-all",
        usage: "@after-all",
        kind: "block",
        summary: "steps run once after the file's last test",
    },
    DirectiveDoc {
        name: "include",
        usage: "@include path/to/steps.cdp [name=value…]",
        kind: "include",
        summary: "the steps of a step file (relative to this file), its ${name} set by the parameters",
    },
    DirectiveDoc {
        name: "mocks",
        usage: "@mocks set…",
        kind: "setting",
        summary: "mock sets layered onto default_mocks (a set named again moves last)",
    },
    DirectiveDoc {
        name: "tags",
        usage: "@tags tag…",
        kind: "setting",
        summary: "tags for --tags",
    },
    DirectiveDoc {
        name: "skip",
        usage: "@skip [reason…]",
        kind: "setting",
        summary: "skip the test (or every test of the file)",
    },
    DirectiveDoc {
        name: "only",
        usage: "@only",
        kind: "setting",
        summary: "run only the tests marked @only",
    },
    DirectiveDoc {
        name: "timeout",
        usage: "@timeout 60s",
        kind: "setting",
        summary: "the test's time limit",
    },
    DirectiveDoc {
        name: "viewport",
        usage: "@viewport 390x844",
        kind: "setting",
        summary: "the page size (replaces @device)",
    },
    DirectiveDoc {
        name: "device",
        usage: "@device \"iPhone 15\"",
        kind: "setting",
        summary: "a device of `chrome-cdp emulate list` (replaces @viewport)",
    },
    DirectiveDoc {
        name: "retries",
        usage: "@retries 2",
        kind: "setting",
        summary: "retries of a failed test (one that then passes is flaky)",
    },
    DirectiveDoc {
        name: "allow-console-errors",
        usage: "@allow-console-errors",
        kind: "setting",
        summary: "console errors do not fail the test",
    },
    DirectiveDoc {
        name: "backend",
        usage: "@backend mock|real|name…",
        kind: "setting",
        summary: "the backends that run the test; the others skip it",
    },
    DirectiveDoc {
        name: "safe",
        usage: "@safe",
        kind: "setting",
        summary: "the test only reads, so it may run against a protected backend",
    },
    DirectiveDoc {
        name: "syntax",
        usage: "@syntax 1",
        kind: "syntax",
        summary: "the grammar version; the first line of the file when written",
    },
];

/// The directives that hold indented steps.
const BLOCKS: &[&str] = &["test", "before", "after", "before-all", "after-all"];

/// The directives that set something, for the file or one test.
const SETTINGS: &[&str] = &[
    "mocks",
    "tags",
    "skip",
    "only",
    "timeout",
    "viewport",
    "device",
    "retries",
    "allow-console-errors",
    "backend",
    "safe",
];

/// A file's or a test's settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// Mock sets layered onto `default_mocks`, in order.
    pub mocks: Vec<String>,
    pub tags: Vec<String>,
    /// `@skip [reason]`: the reason, empty when none was given.
    pub skip: Option<String>,
    pub only: bool,
    pub timeout: Option<Duration>,
    pub viewport: Option<Viewport>,
    /// A device of `chrome-cdp emulate list`.
    pub device: Option<String>,
    pub retries: Option<u32>,
    /// `@allow-console-errors`: the runner does not check `expect
    /// --no-errors` at the end of the test.
    pub allow_console_errors: bool,
    /// `@backend mock|real|<name>…`: the backends the test applies to;
    /// empty for all (plan 11.11).
    pub backend: Vec<String>,
    /// `@safe`: the test only reads, so it may run against a protected
    /// backend.
    pub safe: bool,
}

impl Settings {
    /// These settings with `inner`'s (a test's) on top: mock sets and tags
    /// add up, the rest is replaced where `inner` sets it.
    #[must_use]
    pub fn with(&self, inner: &Self) -> Self {
        let mut mocks = self.mocks.clone();
        for name in &inner.mocks {
            mocks.retain(|m| m != name);
            mocks.push(name.clone());
        }
        let mut tags = self.tags.clone();
        for tag in &inner.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        let (viewport, device) = if inner.viewport.is_some() || inner.device.is_some() {
            (inner.viewport, inner.device.clone())
        } else {
            (self.viewport, self.device.clone())
        };
        Self {
            mocks,
            tags,
            skip: inner.skip.clone().or_else(|| self.skip.clone()),
            only: self.only || inner.only,
            timeout: inner.timeout.or(self.timeout),
            viewport,
            device,
            retries: inner.retries.or(self.retries),
            allow_console_errors: self.allow_console_errors || inner.allow_console_errors,
            backend: if inner.backend.is_empty() {
                self.backend.clone()
            } else {
                inner.backend.clone()
            },
            safe: self.safe || inner.safe,
        }
    }

    /// Whether a run against backend `name` (in `mode`, `mock` or `real`)
    /// has the test; `None` when it does, else why not.
    pub fn skipped_on(&self, name: &str, mode: &str) -> Option<String> {
        if self.backend.is_empty() || self.backend.iter().any(|b| b == name || b == mode) {
            return None;
        }
        Some(format!(
            "@backend {}; this run's backend is {name}",
            self.backend.join(" ")
        ))
    }
}

/// One command line to run, and what fills in its variables.
#[derive(Debug, Clone)]
pub struct Step {
    pub line: Line,
    pub vars: Arc<Vars>,
    /// The `@include` lines it came through, outermost first.
    pub via: Vec<Span>,
}

/// A `@test`.
#[derive(Debug, Clone)]
pub struct Test {
    pub name: String,
    /// The `@test` word.
    pub span: Span,
    /// The last line that belongs to it (before the next top-level line),
    /// for `file.cdp:line`.
    pub end_line: u32,
    /// Its own settings; [`Settings::with`] puts them on the file's.
    pub settings: Settings,
    pub steps: Vec<Step>,
}

/// A scenario file.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub path: PathBuf,
    /// How it is named in output and spans (relative to the current
    /// directory when it is below it).
    pub name: String,
    pub settings: Settings,
    pub before_all: Vec<Step>,
    pub after_all: Vec<Step>,
    pub before: Vec<Step>,
    pub after: Vec<Step>,
    pub tests: Vec<Test>,
}

impl Scenario {
    /// Test `index`'s settings on top of the file's.
    pub fn settings_of(&self, index: usize) -> Settings {
        self.settings.with(&self.tests[index].settings)
    }
}

/// What `${…}` means in a scenario: `@include` parameters, `env:` and
/// `data:` (relative to the file the line is in).
#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub params: BTreeMap<String, String>,
    /// The folder of the line's file.
    pub dir: PathBuf,
    /// The `@include` line that gave the parameters, as `file:line:col`.
    pub include: Option<String>,
}

impl Resolve for Vars {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        match var {
            Var::Name(name) => self.params.get(name).cloned().ok_or_else(|| {
                let unresolved = Unresolved::new(format!("{var} is not set"));
                match &self.include {
                    Some(at) => unresolved.hint(format!(
                        "pass it on the @include line at {at}: {name}=…"
                    )),
                    None => unresolved.hint(
                        "a scenario's ${name} values are @include parameters; use ${env:X} or ${data:file.json#/key} here",
                    ),
                }
            }),
            Var::Env(name) => script::env_value(var, name),
            Var::Data { file, pointer } => script::data_value(
                var,
                &self.dir,
                file,
                pointer,
                "data: paths are relative to the scenario or step file",
            ),
            Var::Var(_) | Var::Secret(_) | Var::Path(_) => Err(Unresolved::new(format!(
                "{var} has a value only while `chrome-cdp test` runs the scenario"
            ))
            .hint("run the scenario with chrome-cdp test")),
        }
    }
}

/// Reads scenario `path`, naming it `name` in spans. Every problem is in
/// the error (syntax, directives, includes); variables are not filled in
/// yet.
pub fn load(path: &Path, name: &str) -> Result<Scenario, Vec<ScriptError>> {
    let (scenario, errors) = read(path, name);
    if errors.is_empty() {
        Ok(scenario)
    } else {
        Err(errors)
    }
}

/// Like [`load`], but also returns what it could read when there are
/// problems (for `lint`, which checks the steps of a file that has
/// directive errors too): the tests that have a name, with the steps,
/// settings and includes that were right, and the hooks.
pub fn read(path: &Path, name: &str) -> (Scenario, Vec<ScriptError>) {
    let mut scenario = Scenario {
        path: path.to_path_buf(),
        name: name.to_owned(),
        settings: Settings::default(),
        before_all: Vec::new(),
        after_all: Vec::new(),
        before: Vec::new(),
        after: Vec::new(),
        tests: Vec::new(),
    };
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            let error = ScriptError::new(
                start_of(name),
                format!("cannot read {}: {e}", path.display()),
            );
            return (scenario, vec![error]);
        }
    };
    let parsed = script::parse(name, &src);
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut loader = Loader {
        errors: parsed.errors,
        stack: vec![canonical(path)],
    };
    let vars = Arc::new(Vars {
        params: BTreeMap::new(),
        dir,
        include: None,
    });
    let items = &parsed.script.items;
    let mut hooks_seen: Vec<&str> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Comment(_) => {}
            Item::Command(line) => loader.errors.push(
                ScriptError::new(line.span.clone(), "a command outside a block").hint(
                    "indent it 2 spaces under @test \"name\" (or @before, @after, @before-all, @after-all)",
                ),
            ),
            Item::Directive(d) => match d.name.as_str() {
                "test" => {
                    let end_line = items[i + 1..]
                        .iter()
                        .map(item_line)
                        .next()
                        .map_or(u32::MAX, |l| l.saturating_sub(1));
                    if let Some(test) = loader.test(d, &vars, end_line) {
                        scenario.tests.push(test);
                    }
                }
                hook @ ("before" | "after" | "before-all" | "after-all") => {
                    if let Some(word) = d.args.first() {
                        loader.errors.push(
                            ScriptError::new(word.span.clone(), format!("@{hook} takes no arguments"))
                                .hint(format!("put its steps on the lines below @{hook}, indented 2 spaces")),
                        );
                    }
                    if hooks_seen.contains(&hook) {
                        loader.errors.push(
                            ScriptError::new(d.span.clone(), format!("a second @{hook} block"))
                                .hint(format!("put all @{hook} steps in one block")),
                        );
                        continue;
                    }
                    hooks_seen.push(hook);
                    let steps = loader.steps(&d.body, &vars, &[], hook);
                    if steps.is_empty() && d.body.iter().all(|i| matches!(i, Item::Comment(_))) {
                        loader.errors.push(
                            ScriptError::new(d.span.clone(), format!("@{hook} has no steps"))
                                .hint("indent its commands 2 spaces below it, or remove it"),
                        );
                    }
                    match hook {
                        "before" => scenario.before = steps,
                        "after" => scenario.after = steps,
                        "before-all" => scenario.before_all = steps,
                        _ => scenario.after_all = steps,
                    }
                }
                name if SETTINGS.contains(&name) => {
                    loader.no_body(d);
                    loader.setting(&mut scenario.settings, d, &d.args);
                }
                "include" => loader.errors.push(
                    ScriptError::new(d.span.clone(), "@include outside a block").hint(
                        "put it in a @test, @before or other block, where its steps run",
                    ),
                ),
                other => loader.unknown(d, other),
            },
        }
    }
    if scenario.tests.is_empty() && loader.errors.is_empty() {
        loader.errors.push(
            ScriptError::new(start_of(name), "the scenario has no @test").hint(
                "add one: a line `@test \"what it checks\"` with its steps indented 2 spaces below",
            ),
        );
    }
    loader.errors.sort_by(|a, b| {
        (a.span.file.as_ref(), a.span.start).cmp(&(b.span.file.as_ref(), b.span.start))
    });
    (scenario, loader.errors)
}

/// Line 1, column 1 of `file`.
fn start_of(file: &str) -> Span {
    Span {
        file: Arc::from(file),
        line: 1,
        column: 1,
        start: 0,
        end: 0,
    }
}

fn item_line(item: &Item) -> u32 {
    match item {
        Item::Command(line) => line.span.line,
        Item::Directive(d) => d.span.line,
        Item::Comment(c) => c.span.line,
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// How a file below the current directory is named: relative to it.
pub fn display_name(path: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}

struct Loader {
    errors: Vec<ScriptError>,
    /// The files being read, the scenario first: an `@include` of one of
    /// them is a cycle.
    stack: Vec<PathBuf>,
}

impl Loader {
    fn unknown(&mut self, d: &Directive, name: &str) {
        let close = suggest::similar(name, DIRECTIVES.iter().copied(), 1);
        let hint = match close.first() {
            Some(known) => format!("did you mean @{known}?"),
            None => format!(
                "scenarios know @{}; see docs/e2e/scenarios.md",
                DIRECTIVES[..DIRECTIVES.len() - 1].join(", @")
            ),
        };
        self.errors.push(
            ScriptError::new(d.span.clone(), format!("unknown directive @{name}")).hint(hint),
        );
    }

    /// An error for a setting written with indented lines.
    fn no_body(&mut self, d: &Directive) {
        if let Some(first) = d.body.iter().find(|i| !matches!(i, Item::Comment(_))) {
            let span = match first {
                Item::Command(line) => line.span.clone(),
                Item::Directive(inner) => inner.span.clone(),
                Item::Comment(c) => c.span.clone(),
            };
            self.errors.push(
                ScriptError::new(span, format!("@{} takes no indented lines", d.name))
                    .hint("steps go in a @test or hook block"),
            );
        }
    }

    /// Applies setting `d` (with arguments `args`) to `settings`.
    fn setting(&mut self, settings: &mut Settings, d: &Directive, args: &[script::Word]) {
        let mut words = Vec::new();
        for word in args {
            let Some(text) = word.literal() else {
                self.errors.push(
                    ScriptError::new(
                        word.span.clone(),
                        format!("@{} cannot use variables", d.name),
                    )
                    .hint("write the value itself"),
                );
                return;
            };
            words.push(text);
        }
        let err = |message: String, hint: &str| {
            ScriptError::new(d.span.clone(), message).hint(hint.to_owned())
        };
        let one = |words: &[String]| match words {
            [one] => Some(one.clone()),
            _ => None,
        };
        let problem = match d.name.as_str() {
            "mocks" if words.is_empty() => Some(err(
                "@mocks needs a mock set".into(),
                "name the sets, e.g. @mocks base checkout",
            )),
            "mocks" => {
                for name in words {
                    settings.mocks.retain(|m| *m != name);
                    settings.mocks.push(name);
                }
                None
            }
            "tags" if words.is_empty() => Some(err(
                "@tags needs a tag".into(),
                "name the tags, e.g. @tags smoke checkout",
            )),
            "tags" => {
                for tag in words {
                    if tag.starts_with('!') || tag.contains(',') {
                        return self.errors.push(err(
                            format!("`{tag}` is not a tag"),
                            "tags are words without ! or ,",
                        ));
                    }
                    if !settings.tags.contains(&tag) {
                        settings.tags.push(tag);
                    }
                }
                None
            }
            "skip" => {
                settings.skip = Some(words.join(" "));
                None
            }
            "only" if !words.is_empty() => {
                Some(err("@only takes no arguments".into(), "write @only alone"))
            }
            "only" => {
                settings.only = true;
                None
            }
            "allow-console-errors" if !words.is_empty() => Some(err(
                "@allow-console-errors takes no arguments".into(),
                "write @allow-console-errors alone",
            )),
            "allow-console-errors" => {
                settings.allow_console_errors = true;
                None
            }
            "safe" if !words.is_empty() => {
                Some(err("@safe takes no arguments".into(), "write @safe alone"))
            }
            "safe" => {
                settings.safe = true;
                None
            }
            "backend" if words.is_empty() => Some(err(
                "@backend needs a backend".into(),
                "e.g. @backend mock (mock-only), @backend real, or a backend's name",
            )),
            "backend" => {
                settings.backend = words;
                None
            }
            "timeout" => match one(&words).map(|w| parse_duration(&w)) {
                Some(Ok(timeout)) if !timeout.is_zero() => {
                    settings.timeout = Some(timeout);
                    None
                }
                _ => Some(err(
                    "@timeout takes one duration".into(),
                    "e.g. @timeout 60s",
                )),
            },
            "viewport" => match one(&words).and_then(|w| Viewport::parse(&w)) {
                Some(viewport) => {
                    settings.viewport = Some(viewport);
                    settings.device = None;
                    None
                }
                None => Some(err(
                    "@viewport takes WIDTHxHEIGHT".into(),
                    "e.g. @viewport 390x844",
                )),
            },
            "device" => match one(&words) {
                Some(device) => {
                    settings.device = Some(device);
                    settings.viewport = None;
                    None
                }
                None => Some(err(
                    "@device takes one device name".into(),
                    "quote a name with spaces: @device \"iPhone 15\" (chrome-cdp emulate list shows them)",
                )),
            },
            "retries" => match one(&words).and_then(|w| w.parse::<u32>().ok()) {
                Some(retries) => {
                    settings.retries = Some(retries);
                    None
                }
                None => Some(err("@retries takes a number".into(), "e.g. @retries 2")),
            },
            _ => None,
        };
        if let Some(problem) = problem {
            self.errors.push(problem);
        }
    }

    /// A `@test` block; `None` when it has no name. Its other problems are
    /// recorded, and it keeps what was right.
    fn test(&mut self, d: &Directive, vars: &Arc<Vars>, end_line: u32) -> Option<Test> {
        let Some(first) = d.args.first() else {
            self.errors.push(
                ScriptError::new(d.span.clone(), "@test needs a name")
                    .hint("e.g. @test \"adds an item to the cart\""),
            );
            return None;
        };
        let name = match first.literal() {
            Some(name) if !name.trim().is_empty() && !is_directive_word(first) => name,
            _ => {
                self.errors.push(
                    ScriptError::new(first.span.clone(), "@test needs a name of plain text")
                        .hint("e.g. @test \"adds an item to the cart\""),
                );
                return None;
            }
        };
        let mut settings = Settings::default();
        // Settings on the @test line: `@test "x" @skip "why" @tags a`.
        let rest = &d.args[1..];
        let mut i = 0;
        if let Some(word) = rest.first()
            && !is_directive_word(word)
        {
            self.errors.push(
                ScriptError::new(word.span.clone(), "a test name is one word")
                    .hint("quote the whole name: @test \"adds an item\""),
            );
        }
        while i < rest.len() {
            if !is_directive_word(&rest[i]) {
                i += 1;
                continue;
            }
            let word = &rest[i];
            let inline = word.raw.trim_start_matches('@');
            let end = rest[i + 1..]
                .iter()
                .position(is_directive_word)
                .map_or(rest.len(), |p| i + 1 + p);
            let inner = Directive {
                name: inline.to_owned(),
                args: Vec::new(),
                body: Vec::new(),
                span: word.span.clone(),
                comment: None,
            };
            if SETTINGS.contains(&inline) {
                self.setting(&mut settings, &inner, &rest[i + 1..end]);
            } else if DIRECTIVES.contains(&inline) {
                self.errors.push(
                    ScriptError::new(word.span.clone(), format!("@{inline} cannot follow a test name"))
                        .hint("only settings (@skip, @only, @tags, @mocks, @timeout, …) go on the @test line"),
                );
            } else {
                self.unknown(&inner, inline);
            }
            i = end;
        }
        let mut steps = Vec::new();
        for item in &d.body {
            match item {
                Item::Comment(_) => {}
                Item::Command(line) => steps.push(Step {
                    line: line.clone(),
                    vars: vars.clone(),
                    via: Vec::new(),
                }),
                Item::Directive(inner) if inner.name == "include" => {
                    steps.extend(self.include(inner, vars, &[]));
                }
                Item::Directive(inner) if SETTINGS.contains(&inner.name.as_str()) => {
                    self.no_body(inner);
                    self.setting(&mut settings, inner, &inner.args);
                }
                Item::Directive(inner) if BLOCKS.contains(&inner.name.as_str()) => {
                    self.errors.push(
                        ScriptError::new(
                            inner.span.clone(),
                            format!("@{} inside a @test", inner.name),
                        )
                        .hint("blocks do not nest; write it at the start of a line"),
                    );
                }
                Item::Directive(inner) => self.unknown(inner, &inner.name),
            }
        }
        let written = d.body.iter().any(|item| match item {
            Item::Command(_) => true,
            Item::Directive(inner) => inner.name == "include",
            Item::Comment(_) => false,
        });
        if !written && settings.skip.is_none() {
            self.errors.push(
                ScriptError::new(d.span.clone(), format!("test \"{name}\" has no steps"))
                    .hint("indent its commands 2 spaces below the @test line"),
            );
        }
        Some(Test {
            name,
            span: d.span.clone(),
            end_line,
            settings,
            steps,
        })
    }

    /// The steps of a hook block: commands and `@include`s.
    fn steps(&mut self, body: &[Item], vars: &Arc<Vars>, via: &[Span], hook: &str) -> Vec<Step> {
        let mut steps = Vec::new();
        for item in body {
            match item {
                Item::Comment(_) => {}
                Item::Command(line) => steps.push(Step {
                    line: line.clone(),
                    vars: vars.clone(),
                    via: via.to_vec(),
                }),
                Item::Directive(d) if d.name == "include" => {
                    steps.extend(self.include(d, vars, via));
                }
                Item::Directive(d) if SETTINGS.contains(&d.name.as_str()) => {
                    self.errors.push(
                        ScriptError::new(d.span.clone(), format!("@{} inside @{hook}", d.name))
                            .hint(
                                "settings go at the top of the file (for every test) or in a @test",
                            ),
                    );
                }
                Item::Directive(d) => self.unknown(d, &d.name),
            }
        }
        steps
    }

    /// The steps of `@include file k=v…`.
    fn include(&mut self, d: &Directive, vars: &Arc<Vars>, via: &[Span]) -> Vec<Step> {
        self.no_body(d);
        let Some(first) = d.args.first() else {
            self.errors.push(
                ScriptError::new(d.span.clone(), "@include needs a file")
                    .hint("e.g. @include ../steps/login.cdp email=admin@example.com"),
            );
            return Vec::new();
        };
        let file = match first.expand(vars.as_ref()) {
            Ok(file) => file,
            Err(e) => {
                self.errors.push(e);
                return Vec::new();
            }
        };
        let mut params = BTreeMap::new();
        for word in &d.args[1..] {
            let text = match word.expand(vars.as_ref()) {
                Ok(text) => text,
                Err(e) => {
                    self.errors.push(e);
                    continue;
                }
            };
            match text.split_once('=') {
                Some((key, value)) if script::is_name(key) => {
                    params.insert(key.to_owned(), value.to_owned());
                }
                _ => self.errors.push(
                    ScriptError::new(
                        word.span.clone(),
                        format!("`{}` is not a parameter", word.raw),
                    )
                    .hint("write name=value, e.g. email=admin@example.com"),
                ),
            }
        }
        let path = vars.dir.join(&file);
        let key = canonical(&path);
        if let Some(at) = self.stack.iter().position(|p| *p == key) {
            let cycle: Vec<String> = self.stack[at..]
                .iter()
                .chain(std::iter::once(&key))
                .map(|p| display_name(p))
                .collect();
            self.errors.push(
                ScriptError::new(
                    first.span.clone(),
                    format!("@include cycle: {}", cycle.join(" -> ")),
                )
                .hint("a step file cannot include itself, directly or through others"),
            );
            return Vec::new();
        }
        let src = match std::fs::read_to_string(&path) {
            Ok(src) => src,
            Err(e) => {
                self.errors.push(
                    ScriptError::new(
                        first.span.clone(),
                        format!("cannot read {}: {e}", path.display()),
                    )
                    .hint("@include paths are relative to the file that includes them"),
                );
                return Vec::new();
            }
        };
        let name = display_name(&path);
        let parsed = script::parse(&name, &src);
        self.errors.extend(parsed.errors);
        let inner = Arc::new(Vars {
            params,
            dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            include: Some(d.span.to_string()),
        });
        let mut via = via.to_vec();
        via.push(d.span.clone());
        self.stack.push(key);
        let mut steps = Vec::new();
        for item in &parsed.script.items {
            match item {
                Item::Comment(_) => {}
                Item::Command(line) => steps.push(Step {
                    line: line.clone(),
                    vars: inner.clone(),
                    via: via.clone(),
                }),
                Item::Directive(inner_d) if inner_d.name == "include" => {
                    steps.extend(self.include(inner_d, &inner, &via));
                }
                Item::Directive(inner_d) => self.errors.push(
                    ScriptError::new(
                        inner_d.span.clone(),
                        format!("@{} in a step file", inner_d.name),
                    )
                    .hint("a step file (@include) holds commands and @include lines only"),
                ),
            }
        }
        self.stack.pop();
        if steps.is_empty() {
            self.errors.push(
                ScriptError::new(first.span.clone(), format!("{name} has no steps"))
                    .hint("a step file holds one command per line"),
            );
        }
        steps
    }
}

/// Whether `word` starts an inline directive: `@skip`, unquoted.
fn is_directive_word(word: &script::Word) -> bool {
    !word.quoted && word.raw.len() > 1 && word.raw.starts_with('@')
}

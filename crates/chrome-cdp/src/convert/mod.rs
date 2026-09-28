//! `chrome-cdp convert <spec.ts>` (plan 09, milestone 9.4): a best-effort
//! translation of a Playwright test file into a `.cdp` script, following
//! the tables of `docs/migrating-from-playwright.md`.
//!
//! It reads the file with a small, forgiving JavaScript parser of its own
//! (`parse`), not tree-sitter: the calls a Playwright test is made of are
//! member chains, literals and arrow functions, and a grammar crate would
//! add megabytes to the binary for them. Each statement becomes command
//! lines, or a `# TODO line N (why): code` line; nothing is run.

mod lex;
mod parse;
mod translate;

#[cfg(test)]
mod tests;

use std::path::Path;

use protocol::schema::{ConvertData, ConvertTodo};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use crate::cli::ConvertArgs;

/// One line of the script.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Line {
    Cmd(Vec<Word>),
    Comment(String),
    Todo {
        /// The statement's line in the spec file.
        line: usize,
        reason: String,
        code: String,
    },
    Blank,
}

/// One word of a command line: text, and `${…}` variables.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Word(Vec<Part>);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Part {
    Lit(String),
    /// A variable's name, e.g. `env:PASSWORD`.
    Var(String),
}

impl Word {
    fn lit(text: impl Into<String>) -> Self {
        Self(vec![Part::Lit(text.into())])
    }

    /// The word in the `.cdp` quoting (docs/scenario-syntax.md): bare when
    /// that is safe, else single quotes, else double quotes.
    fn render(&self) -> String {
        let has_var = self.0.iter().any(|p| matches!(p, Part::Var(_)));
        let text: String = self
            .0
            .iter()
            .map(|p| match p {
                Part::Lit(t) => t.as_str(),
                Part::Var(_) => "",
            })
            .collect();
        if !has_var {
            let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+%^~".contains(c);
            if !text.is_empty() && text.chars().all(safe) {
                return text;
            }
            if !text.contains(['\'', '\n', '\r']) {
                return format!("'{text}'");
            }
        }
        let mut out = String::from('"');
        for p in &self.0 {
            match p {
                Part::Lit(t) => {
                    let mut chars = t.chars().peekable();
                    while let Some(c) = chars.next() {
                        match c {
                            '"' => out.push_str("\\\""),
                            '\\' => out.push_str("\\\\"),
                            '\n' => out.push_str("\\n"),
                            '\r' => {}
                            '$' if chars.peek() == Some(&'{') => out.push_str("$$"),
                            c => out.push(c),
                        }
                    }
                }
                Part::Var(name) => {
                    out.push_str("${");
                    out.push_str(name);
                    out.push('}');
                }
            }
        }
        out.push('"');
        out
    }
}

impl Line {
    fn render(&self) -> String {
        match self {
            Line::Cmd(words) => words.iter().map(Word::render).collect::<Vec<_>>().join(" "),
            Line::Comment(text) => format!("# {text}"),
            Line::Todo { line, reason, code } => format!("# TODO line {line} ({reason}): {code}"),
            Line::Blank => String::new(),
        }
    }
}

/// What converting a file gave.
pub(crate) struct Converted {
    pub(crate) script: String,
    pub(crate) tests: usize,
    pub(crate) steps: usize,
    pub(crate) todos: Vec<ConvertTodo>,
}

/// Converts Playwright source; `name` goes in the header comment.
pub(crate) fn convert(name: &str, src: &str) -> Converted {
    let stmts = parse::parse(src);
    let mut converter = translate::Converter::new(src);
    converter.file(&stmts);
    let mut lines = vec![Line::Comment(format!(
        "Converted from {name} by chrome-cdp convert: check the TODO lines."
    ))];
    if converter.tests > 1 {
        lines.push(Line::Comment(
            "The tests run one after another, in one session.".into(),
        ));
    }
    lines.push(Line::Blank);
    lines.extend(converter.lines);
    let mut script = String::new();
    let mut steps = 0;
    let mut todos = Vec::new();
    let mut blank = false;
    for line in &lines {
        match line {
            Line::Blank if blank => continue,
            Line::Cmd(_) => steps += 1,
            Line::Todo { line, reason, code } => todos.push(ConvertTodo {
                line: *line,
                reason: reason.clone(),
                code: code.clone(),
            }),
            _ => {}
        }
        blank = matches!(line, Line::Blank);
        script.push_str(&line.render());
        script.push('\n');
    }
    let script = format!("{}\n", script.trim_end());
    Converted {
        script,
        tests: converter.tests,
        steps,
        todos,
    }
}

pub(crate) fn run(args: &ConvertArgs) -> Result<Output, ErrorInfo> {
    let (name, src) = read(&args.file)?;
    let converted = convert(&name, &src);
    if converted.steps == 0 && converted.tests == 0 {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("found no Playwright tests or page calls in {name}"),
        )
        .hint(Hint::text(
            "convert takes a Playwright test file: test('…', async ({ page }) => { … })",
        )));
    }
    let counts = format!(
        "{} {}, {} {}, {} TODO{}",
        converted.tests,
        if converted.tests == 1 {
            "test"
        } else {
            "tests"
        },
        converted.steps,
        if converted.steps == 1 {
            "step"
        } else {
            "steps"
        },
        converted.todos.len(),
        if converted.todos.len() == 1 { "" } else { "s" },
    );
    let mut data = ConvertData {
        file: name.clone(),
        script: converted.script.clone(),
        tests: converted.tests,
        steps: converted.steps,
        todos: converted.todos.clone(),
        path: None,
    };
    let todo_warning = (!converted.todos.is_empty()).then(|| {
        format!(
            "{} TODO line{}: statements convert could not translate; rewrite them before running the script",
            converted.todos.len(),
            if converted.todos.len() == 1 { "" } else { "s" },
        )
    });
    let output = if let Some(path) = &args.output {
        let shown = crate::input::absolute_path(path)?;
        std::fs::write(path, &converted.script).map_err(|e| {
            ErrorInfo::new(ErrorKind::BadArgs, format!("cannot write {shown}: {e}"))
        })?;
        data.path = Some(shown.clone());
        Output::typed(&data, format!("converted {name}: {counts} → {shown}")).quiet(shown)
    } else {
        let text = converted.script.trim_end().to_owned();
        Output::typed(&data, text.clone())
            .quiet(text)
            .limit(args.max_chars, "-o FILE to write all of it")
    };
    Ok(match todo_warning {
        Some(w) => output.warning(w),
        None => output,
    })
}

/// The file's name as given and its text; `-` is stdin.
fn read(file: &Path) -> Result<(String, String), ErrorInfo> {
    if file == Path::new("-") {
        let mut src = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut src)
            .map_err(|e| ErrorInfo::new(ErrorKind::BadArgs, format!("cannot read stdin: {e}")))?;
        return Ok(("stdin".into(), src));
    }
    let name = file.display().to_string();
    let src = std::fs::read_to_string(file).map_err(|e| {
        ErrorInfo::new(ErrorKind::BadArgs, format!("cannot read {name}: {e}")).hint(
            Hint::run(Cmd::new("convert").raw("tests/login.spec.ts")).why("a Playwright test file"),
        )
    })?;
    Ok((name, src))
}

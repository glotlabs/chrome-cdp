//! The `.cdp` script format (plan 08, milestone 8.2): what `batch` and
//! `run` read, and what plan 11's scenarios build on.
//!
//! The grammar is specified in `docs/scenario-syntax.md`. [`parse`] is a
//! hand-written lexer and parser: every node of the [`Script`] it returns
//! carries a [`Span`], so any later error (an unknown command, a bad flag,
//! a failed step) can point at `file.cdp:12:5`.
//!
//! Parsing knows nothing about commands or directives beyond `@syntax`:
//! the CLI checks each command line against its clap definitions, and each
//! runner decides which directives it accepts. Variables stay unexpanded in
//! the tree; [`Word::expand`] fills them in through a [`Resolve`].

use std::fmt;
use std::sync::Arc;

use crate::{ErrorInfo, ErrorKind};

mod parse;
#[cfg(test)]
mod tests;

pub use parse::parse;

/// The newest syntax version this build reads (`@syntax 1`).
pub const SYNTAX_VERSION: u32 = 1;

/// Where a node is in its file. `line` and `column` count from 1, and
/// `column` counts characters; `start..end` is the byte range in the
/// source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The file as named to the parser (`flow.cdp`, `<stdin>`).
    pub file: Arc<str>,
    pub line: u32,
    pub column: u32,
    pub start: usize,
    pub end: usize,
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// A parsed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub file: Arc<str>,
    /// The version from `@syntax N`, else [`SYNTAX_VERSION`].
    pub syntax: u32,
    /// The top-level items, in order. `@syntax` is not among them.
    pub items: Vec<Item>,
}

impl Script {
    /// The top-level command lines, in order.
    pub fn commands(&self) -> impl Iterator<Item = &Line> {
        self.items.iter().filter_map(|item| match item {
            Item::Command(line) => Some(line),
            _ => None,
        })
    }
}

/// One line (or `\`-continued lines) of a script, or a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A command line: the CLI's arguments without the program name.
    Command(Line),
    /// `@name args…` with its indented body.
    Directive(Directive),
    /// A line holding only a comment.
    Comment(Comment),
}

/// A command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// At least one word.
    pub words: Vec<Word>,
    /// The words as written, joined by single spaces (continuations and
    /// runs of spaces collapse, variables stay unexpanded): what progress
    /// output shows.
    pub text: String,
    /// The first word.
    pub span: Span,
    /// A comment after the words.
    pub comment: Option<Comment>,
}

/// `@name args…` and the items indented 2 spaces below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    /// Without the `@`.
    pub name: String,
    pub args: Vec<Word>,
    pub body: Vec<Item>,
    /// The `@name` word.
    pub span: Span,
    pub comment: Option<Comment>,
}

/// A `#` comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// The text after `#`, as written.
    pub text: String,
    /// From the `#` to the end of the line.
    pub span: Span,
}

/// One argument: text and variables, after quotes and escapes are
/// removed. A variable's value never splits a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// Never empty: `''` is one empty [`Part::Text`].
    pub parts: Vec<Part>,
    /// Whether any part was quoted or escaped with `\`.
    pub quoted: bool,
    /// The source text of the word.
    pub raw: String,
    pub span: Span,
}

impl Word {
    /// The word's text when it holds no variables.
    pub fn literal(&self) -> Option<String> {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                Part::Var(_) => None,
            })
            .collect()
    }

    /// The word with its variables filled in by `resolve`.
    pub fn expand(&self, resolve: &dyn Resolve) -> Result<String, ScriptError> {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Var(v) => {
                    let value = resolve.resolve(&v.var).map_err(|u| ScriptError {
                        span: v.span.clone(),
                        message: u.message,
                        hint: u.hint,
                    })?;
                    out.push_str(&value);
                }
            }
        }
        Ok(out)
    }

    /// The variables the word uses.
    pub fn vars(&self) -> impl Iterator<Item = &VarRef> {
        self.parts.iter().filter_map(|p| match p {
            Part::Var(v) => Some(v),
            Part::Text(_) => None,
        })
    }
}

/// A piece of a [`Word`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Var(VarRef),
}

/// `${…}` where it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarRef {
    pub var: Var,
    /// From `$` to `}`.
    pub span: Span,
}

/// What a `${…}` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    /// `${name}`: a `--var` of `run` (or an `@include` parameter).
    Name(String),
    /// `${saved.order.id}`, `${test.id}`, `${run.id}`: values the test
    /// runner provides.
    Path(Vec<String>),
    /// `${env:HOME}`.
    Env(String),
    /// `${var:product}`: the backend's vars file (plan 11).
    Var(String),
    /// `${secret:password}`: like `env`, but masked in output (plan 11).
    Secret(String),
    /// `${data:users.json#/0/email}`: a value from a JSON file; an empty
    /// pointer is the whole file.
    Data { file: String, pointer: String },
}

impl fmt::Display for Var {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(n) => write!(f, "${{{n}}}"),
            Self::Path(p) => write!(f, "${{{}}}", p.join(".")),
            Self::Env(n) => write!(f, "${{env:{n}}}"),
            Self::Var(n) => write!(f, "${{var:{n}}}"),
            Self::Secret(n) => write!(f, "${{secret:{n}}}"),
            Self::Data { file, pointer } if pointer.is_empty() => write!(f, "${{data:{file}}}"),
            Self::Data { file, pointer } => write!(f, "${{data:{file}#{pointer}}}"),
        }
    }
}

/// Fills in variables. Each runner has its own: `run` knows `--var`,
/// `env` and `data`; plan 11's test runner adds the rest.
pub trait Resolve {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved>;
}

/// Why a variable has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub message: String,
    pub hint: Option<String>,
}

impl Unresolved {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// A problem at a place in a script: a syntax error, an unknown variable,
/// a bad command line, ….
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    pub span: Span,
    pub message: String,
    pub hint: Option<String>,
}

impl ScriptError {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            hint: None,
        }
    }

    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.span, self.message)
    }
}

impl std::error::Error for ScriptError {}

impl From<ScriptError> for ErrorInfo {
    /// A `BadArgs` error whose message starts with the place.
    fn from(e: ScriptError) -> Self {
        let info = ErrorInfo::new(ErrorKind::BadArgs, e.to_string());
        match e.hint {
            Some(hint) => info.hint(hint),
            None => info,
        }
    }
}

/// What [`parse`] returns: the tree, and every syntax error found. Lines
/// with errors are left out of the tree, so check `errors` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub script: Script,
    pub errors: Vec<ScriptError>,
}

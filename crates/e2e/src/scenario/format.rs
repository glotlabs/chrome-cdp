//! `chrome-cdp fmt` for `.cdp` files (plan 11, milestone 11.12): scenario
//! files, step files and `run` scripts.
//!
//! [`format`] lays a parsed file out again:
//!
//! - bodies indented 2 spaces a level, continuation lines 4 more than
//!   their line, comments at the depth of the block they are in;
//! - one blank line before each block (`@test`, `@before`, …) and the
//!   comments right above it, at most one elsewhere, none at the start
//!   or end of a block;
//! - quoted words quoted one way: on command lines single quotes unless
//!   the word has a `${…}` or a `'`; in directive arguments (test names,
//!   `@skip` reasons) double quotes, the form `history --as-scenario`
//!   writes. Unquoted words stay as written;
//! - trailing comments of neighbouring lines lined up, 2 spaces after
//!   the longest line;
//! - short flags written long, by the [`Commands`] the caller gives (the
//!   CLI knows the flags; this crate does not);
//! - settings in a stable order: each run of setting lines, and the
//!   settings after a test's name, by [`ORDER`].
//!
//! Layout only: the file is parsed again after formatting and its tree
//! must be the one before (spans aside, and settings runs compared in
//! [`ORDER`]), or [`format`] fails instead of changing what the file does.

use protocol::script::{self, Comment, Directive, Item, Line, Part, Script, ScriptError, Word};

use super::{BLOCKS, SETTINGS};

/// The order settings are written in. `@viewport` and `@device` replace
/// each other, so they share a place and keep their own order.
pub const ORDER: &[&[&str]] = &[
    &["skip"],
    &["only"],
    &["mocks"],
    &["tags"],
    &["backend"],
    &["safe"],
    &["timeout"],
    &["retries"],
    &["viewport", "device"],
    &["allow-console-errors"],
];

/// A body is indented this much more than its directive.
const INDENT: usize = 2;
/// A continuation line is indented this much more than its line.
const CONTINUATION: usize = 4;

/// What the formatter needs to know about commands.
pub trait Commands {
    /// Long forms of the short flags among `line`'s words: the index of a
    /// word and the words that replace it (`-o` → `--output`; `-qv` →
    /// `--quiet --verbose`). Nothing when unsure.
    fn long_flags(&self, line: &Line) -> Vec<(usize, Vec<String>)> {
        let _ = line;
        Vec::new()
    }

    /// What `line` does, for checking that formatting kept it; `None`
    /// compares its words.
    fn meaning(&self, line: &Line) -> Option<String> {
        let _ = line;
        None
    }
}

/// Knows no commands: words stay as they are.
#[derive(Debug, Clone, Copy, Default)]
pub struct Plain;

impl Commands for Plain {}

/// `src` (named `file` in errors) formatted. Fails with the syntax errors
/// of a file that does not parse, since their lines are not in the tree.
pub fn format(file: &str, src: &str, commands: &dyn Commands) -> Result<String, Vec<ScriptError>> {
    let parsed = script::parse(file, src);
    if !parsed.errors.is_empty() {
        return Err(parsed.errors);
    }
    let mut out = Writer {
        src,
        commands,
        lines: Vec::new(),
    };
    let items = &parsed.script.items;
    match syntax_line(src) {
        Some((line, text)) => {
            let head = items
                .iter()
                .take_while(|i| matches!(i, Item::Comment(c) if c.span.line < line))
                .count();
            out.block(&items[..head], 0, false);
            out.lines.push(Out::code(0, vec![text.0], text.1));
            if head < items.len() {
                out.lines.push(Out::Blank);
                out.block(&items[head..], 0, false);
            }
        }
        None => out.block(items, 0, false),
    }
    let text = render(&out.lines);
    let again = script::parse(file, &text);
    let same = again.errors.is_empty()
        && again.script.syntax == parsed.script.syntax
        && shape(&again.script, commands) == shape(&parsed.script, commands);
    if !same {
        return Err(vec![
            ScriptError::new(start_of(file), "formatting would change what the file does")
                .hint("this is a chrome-cdp bug: please report it with the file; the file was left as it was"),
        ]);
    }
    Ok(text)
}

/// The tree of `script` without spans or layout, settings in [`ORDER`]:
/// what [`format`] must keep.
pub fn shape(script: &Script, commands: &dyn Commands) -> Vec<Shape> {
    shapes(&script.items, commands)
}

/// A node of [`shape`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Comment(String),
    Command(String),
    Directive {
        name: String,
        args: Vec<String>,
        body: Vec<Shape>,
        comment: Option<String>,
    },
}

fn shapes(items: &[Item], commands: &dyn Commands) -> Vec<Shape> {
    let mut out: Vec<Shape> = items
        .iter()
        .map(|item| match item {
            Item::Comment(c) => Shape::Comment(c.text.trim_end().to_owned()),
            Item::Command(line) => {
                let words = commands.meaning(line).unwrap_or_else(|| {
                    line.words
                        .iter()
                        .map(word_shape)
                        .collect::<Vec<_>>()
                        .join(" ")
                });
                let comment = line.comment.as_ref().map(comment_shape).unwrap_or_default();
                Shape::Command(format!("{words}{comment}"))
            }
            Item::Directive(d) => {
                let mut args: Vec<String> = d.args.iter().map(word_shape).collect();
                if d.name == "test" && args.len() > 1 {
                    let groups = sorted_inline(&d.args[1..]);
                    args.truncate(1);
                    args.extend(groups.iter().flat_map(|g| g.iter().map(word_shape)));
                }
                Shape::Directive {
                    name: d.name.clone(),
                    args,
                    body: shapes(&d.body, commands),
                    comment: d.comment.as_ref().map(comment_shape),
                }
            }
        })
        .collect();
    // Runs of settings, in ORDER.
    let mut i = 0;
    while i < out.len() {
        let end = (i..out.len())
            .find(|&j| !is_setting_shape(&out[j]))
            .unwrap_or(out.len());
        if end > i + 1 {
            out[i..end].sort_by_key(|s| match s {
                Shape::Directive { name, .. } => rank(name),
                _ => usize::MAX,
            });
        }
        i = end.max(i + 1);
    }
    out
}

fn is_setting_shape(shape: &Shape) -> bool {
    matches!(shape, Shape::Directive { name, body, .. } if body.is_empty() && SETTINGS.contains(&name.as_str()))
}

fn comment_shape(c: &Comment) -> String {
    format!(" #{}", c.text.trim_end())
}

fn word_shape(word: &Word) -> String {
    word.parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => format!("{t:?}"),
            Part::Var(v) => v.var.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Where a setting goes in [`ORDER`]; unknown names last.
fn rank(name: &str) -> usize {
    ORDER
        .iter()
        .position(|group| group.contains(&name))
        .unwrap_or(ORDER.len())
}

/// The settings after a test's name: each directive word with the words
/// up to the next one. Words before the first directive word form a
/// group of their own (the loader reports them).
fn inline_settings(words: &[Word]) -> Vec<&[Word]> {
    let mut groups = Vec::new();
    let mut start = 0;
    for i in 1..=words.len() {
        if i == words.len() || is_directive_word(&words[i]) {
            groups.push(&words[start..i]);
            start = i;
        }
    }
    groups
}

/// [`inline_settings`] in [`ORDER`], stably; as written when words
/// other than settings come first (the loader reports those).
fn sorted_inline(words: &[Word]) -> Vec<&[Word]> {
    let mut groups = inline_settings(words);
    if groups.iter().all(|g| is_directive_word(&g[0])) {
        groups.sort_by_key(|g| rank(g[0].raw.trim_start_matches('@')));
    }
    groups
}

fn is_directive_word(word: &Word) -> bool {
    super::is_directive_word(word)
}

/// The `@syntax` line (the parser takes it out of the tree): its physical
/// line number, and its words and comment as written.
fn syntax_line(src: &str) -> Option<(u32, (String, Option<String>))> {
    for (n, line) in src.lines().enumerate() {
        let text = line.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let rest = text.strip_prefix("@syntax")?;
        if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            return None;
        }
        // Parsed as another directive, so the parser keeps it.
        let parsed = script::parse("", &format!("@x{rest}"));
        let Some(Item::Directive(d)) = parsed.script.items.first() else {
            return None;
        };
        let words: Vec<&str> = d.args.iter().map(|w| w.raw.as_str()).collect();
        let code = std::iter::once("@syntax")
            .chain(words)
            .collect::<Vec<_>>()
            .join(" ");
        let comment = d.comment.as_ref().map(|c| c.text.trim_end().to_owned());
        return Some((u32::try_from(n + 1).unwrap_or(u32::MAX), (code, comment)));
    }
    None
}

fn start_of(file: &str) -> script::Span {
    script::Span {
        file: file.into(),
        line: 1,
        column: 1,
        start: 0,
        end: 0,
    }
}

/// An output line.
enum Out {
    Blank,
    Code {
        indent: usize,
        /// The physical lines of a continued line; one otherwise.
        segments: Vec<String>,
        /// The text after `#`.
        comment: Option<String>,
    },
    /// A full-line comment.
    Comment {
        indent: usize,
        text: String,
    },
}

impl Out {
    fn code(indent: usize, segments: Vec<String>, comment: Option<String>) -> Self {
        Self::Code {
            indent,
            segments,
            comment,
        }
    }
}

struct Writer<'a> {
    src: &'a str,
    commands: &'a dyn Commands,
    lines: Vec<Out>,
}

impl Writer<'_> {
    /// Writes `items` at `indent`. `nested` blocks keep the source's blank
    /// lines (one at most); the top level has one before each block.
    fn block(&mut self, items: &[Item], indent: usize, nested: bool) {
        // Blank lines go by position in the source's order: a sorted run
        // of settings has none inside it, and only settings move.
        let written: Vec<&Item> = items.iter().collect();
        let order = self.sorted(items);
        for (i, &k) in order.iter().enumerate() {
            if i > 0 {
                let (prev, item) = (written[i - 1], written[i]);
                let blank = if nested {
                    self.blank_between(prev, item)
                } else if matches!(prev, Item::Comment(_))
                    && self.touching(prev, item)
                    && self.leads_to_block(&written[i..])
                {
                    false
                } else if is_block(prev) || self.leads_to_block(&written[i..]) {
                    true
                } else {
                    self.blank_between(prev, item)
                };
                if blank {
                    self.lines.push(Out::Blank);
                }
            }
            self.item(&items[k], indent);
        }
    }

    /// The order to write `items` in: each run of settings (no comments
    /// or blank lines between them) sorted by [`ORDER`], stably.
    fn sorted(&self, items: &[Item]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..items.len()).collect();
        let mut i = 0;
        while i < items.len() {
            let mut end = i;
            while end < items.len()
                && is_setting(&items[end])
                && (end == i || !self.blank_between(&items[end - 1], &items[end]))
            {
                end += 1;
            }
            if end > i + 1 {
                order[i..end].sort_by_key(|&k| match &items[k] {
                    Item::Directive(d) => rank(&d.name),
                    _ => usize::MAX,
                });
            }
            i = end.max(i + 1);
        }
        order
    }

    /// Whether `items[0]` is a block, or comments right above one.
    fn leads_to_block(&self, items: &[&Item]) -> bool {
        let mut i = 0;
        loop {
            match items.get(i) {
                Some(item) if is_block(item) => return true,
                Some(Item::Comment(_))
                    if items
                        .get(i + 1)
                        .is_some_and(|next| self.touching(items[i], next)) =>
                {
                    i += 1;
                }
                _ => return false,
            }
        }
    }

    /// Whether `b` starts on the line after `a` ends.
    fn touching(&self, a: &Item, b: &Item) -> bool {
        !self.blank_between(a, b)
    }

    /// Whether the source has a blank line between `a` and `b`.
    fn blank_between(&self, a: &Item, b: &Item) -> bool {
        let end = end_of(a);
        let start = start_line_offset(b);
        self.src
            .get(end..start)
            .is_some_and(|between| between.matches('\n').count() > 1)
    }

    fn item(&mut self, item: &Item, indent: usize) {
        match item {
            Item::Comment(c) => self.lines.push(Out::Comment {
                indent,
                text: c.text.trim_end().to_owned(),
            }),
            Item::Command(line) => {
                let segments = self.command(line);
                let comment = line.comment.as_ref().map(|c| c.text.trim_end().to_owned());
                self.lines.push(Out::code(indent, segments, comment));
            }
            Item::Directive(d) => {
                let segments = self.directive(d);
                let comment = d.comment.as_ref().map(|c| c.text.trim_end().to_owned());
                self.lines.push(Out::code(indent, segments, comment));
                self.block(&d.body, indent + INDENT, true);
            }
        }
    }

    /// A command line's words, split where the source continued the line.
    fn command(&self, line: &Line) -> Vec<String> {
        let mut replaced = self.commands.long_flags(line);
        replaced.sort_by_key(|(i, _)| *i);
        let mut words: Vec<(Vec<String>, &Word)> = Vec::new();
        for (i, word) in line.words.iter().enumerate() {
            let text = match replaced.iter().find(|(at, _)| *at == i) {
                Some((_, new)) => new.clone(),
                None => vec![self.word(word, '\'')],
            };
            words.push((text, word));
        }
        Self::segments(words)
    }

    fn directive(&self, d: &Directive) -> Vec<String> {
        let head = format!("@{}", d.name);
        let mut args: Vec<&Word> = d.args.iter().collect();
        if d.name == "test" && args.len() > 1 {
            let groups = sorted_inline(&d.args[1..]);
            args.truncate(1);
            args.extend(groups.into_iter().flatten());
        }
        let mut segments = vec![head];
        // Continuations of directive lines keep their breaks too.
        let mut last_line = d.span.line;
        for word in args {
            let text = self.word(word, '"');
            match segments.last_mut() {
                Some(last) if word.span.line == last_line => {
                    last.push(' ');
                    last.push_str(&text);
                }
                _ => segments.push(text),
            }
            last_line = word.span.line;
        }
        segments
    }

    /// Joins words into physical lines, breaking where the source did.
    fn segments(words: Vec<(Vec<String>, &Word)>) -> Vec<String> {
        let mut segments: Vec<String> = Vec::new();
        let mut last_line = 0;
        for (texts, word) in words {
            match segments.last_mut() {
                Some(last) if word.span.line == last_line => {
                    last.push(' ');
                    last.push_str(&texts.join(" "));
                }
                _ => segments.push(texts.join(" ")),
            }
            last_line = word.span.line;
        }
        segments
    }

    /// A word as it is written: unquoted ones as they are; quoted ones
    /// with `quote` (`'` or `"`), or the other when they need it.
    fn word(&self, word: &Word, quote: char) -> String {
        if !word.quoted {
            return word.raw.clone();
        }
        // `--flag='…'` and `key="…"` keep their name outside the quotes.
        let prefix = unquoted_prefix(word);
        let mut parts = word.parts.clone();
        if let Some(Part::Text(first)) = parts.first_mut() {
            first.drain(..prefix.len());
        }
        let has_var = parts.iter().any(|p| matches!(p, Part::Var(_)));
        let text: String = parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                Part::Var(_) => None,
            })
            .collect();
        let single_ok = !has_var && !text.contains(['\'', '\n', '\r']);
        let double_ok = !text.contains('\r');
        let candidate = if single_ok && (quote == '\'' || !double_ok) {
            format!("{prefix}'{text}'")
        } else if double_ok {
            format!("{prefix}{}", self.double(&parts))
        } else {
            return word.raw.clone();
        };
        // Keep the source's form when the new one would read differently
        // (`$$${`, a trailing `\`, …).
        if same_word(&candidate, word) {
            candidate
        } else {
            word.raw.clone()
        }
    }

    /// `parts` in double quotes.
    fn double(&self, parts: &[Part]) -> String {
        let mut out = String::from('"');
        for part in parts {
            match part {
                Part::Var(v) => out.push_str(&self.src[v.span.start..v.span.end]),
                Part::Text(t) => {
                    let chars: Vec<char> = t.chars().collect();
                    for (i, &c) in chars.iter().enumerate() {
                        match c {
                            '"' => out.push_str("\\\""),
                            '\n' => out.push_str("\\n"),
                            '\\' => match chars.get(i + 1) {
                                Some('"' | '\\' | 'n' | '$') | None => out.push_str("\\\\"),
                                Some(_) => out.push('\\'),
                            },
                            '$' if chars.get(i + 1) == Some(&'{') => out.push_str("$$"),
                            c => out.push(c),
                        }
                    }
                }
            }
        }
        out.push('"');
        out
    }
}

/// The unquoted `name=` or `--flag=` a quoted word starts with, if any.
fn unquoted_prefix(word: &Word) -> &str {
    let raw = word.raw.as_str();
    let Some(eq) = raw.find('=') else {
        return "";
    };
    let name = raw[..eq].trim_start_matches('-');
    let simple = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let text_starts =
        matches!(word.parts.first(), Some(Part::Text(t)) if t.starts_with(&raw[..=eq]));
    if simple && text_starts && eq + 1 < raw.len() {
        &raw[..=eq]
    } else {
        ""
    }
}

/// Whether `text` reads as the same word as `word`.
fn same_word(text: &str, word: &Word) -> bool {
    let parsed = script::parse("", &format!("x {text}"));
    let strip = |parts: &[Part]| -> Vec<String> {
        parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => format!("t{t}"),
                Part::Var(v) => format!("v{}", v.var),
            })
            .collect()
    };
    match parsed.script.items.as_slice() {
        [Item::Command(line)]
            if parsed.errors.is_empty() && line.words.len() == 2 && line.comment.is_none() =>
        {
            strip(&line.words[1].parts) == strip(&word.parts)
        }
        _ => false,
    }
}

fn is_block(item: &Item) -> bool {
    matches!(item, Item::Directive(d) if BLOCKS.contains(&d.name.as_str()) || d.body.iter().any(|i| !matches!(i, Item::Comment(_))))
}

fn is_setting(item: &Item) -> bool {
    matches!(item, Item::Directive(d) if d.body.is_empty() && SETTINGS.contains(&d.name.as_str()))
}

/// The byte offset where `item` ends: its last word, comment or body item.
fn end_of(item: &Item) -> usize {
    match item {
        Item::Comment(c) => c.span.end,
        Item::Command(line) => line
            .comment
            .as_ref()
            .map(|c| c.span.end)
            .or_else(|| line.words.last().map(|w| w.span.end))
            .unwrap_or(line.span.end),
        Item::Directive(d) => {
            let own = d
                .comment
                .as_ref()
                .map(|c| c.span.end)
                .or_else(|| d.args.last().map(|w| w.span.end))
                .unwrap_or(d.span.end);
            d.body.last().map_or(own, |last| end_of(last).max(own))
        }
    }
}

/// The byte offset where `item` starts.
fn start_line_offset(item: &Item) -> usize {
    match item {
        Item::Comment(c) => c.span.start,
        Item::Command(line) => line.span.start,
        Item::Directive(d) => d.span.start,
    }
}

/// The text of `lines`: trailing comments of neighbouring code lines lined
/// up, one line break at the end.
fn render(lines: &[Out]) -> String {
    // The column each trailing comment starts at.
    let widths: Vec<Option<usize>> = lines
        .iter()
        .map(|l| match l {
            Out::Code {
                indent,
                segments,
                comment: Some(_),
            } => {
                let last = segments.last().map_or(0, |s| s.chars().count());
                let extra = if segments.len() > 1 { CONTINUATION } else { 0 };
                Some(indent + extra + last)
            }
            _ => None,
        })
        .collect();
    // Groups: neighbouring one-line code lines with trailing comments; a
    // continued line is a group of its own.
    let mut columns = vec![0; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if widths[i].is_none() {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        if single_segment(&lines[i]) {
            while end < lines.len() && widths[end].is_some() && single_segment(&lines[end]) {
                end += 1;
            }
        }
        let column = widths[i..end].iter().flatten().max().copied().unwrap_or(0) + 2;
        for c in &mut columns[i..end] {
            *c = column;
        }
        i = end;
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        match line {
            Out::Blank => {}
            Out::Comment { indent, text } => {
                push_indent(&mut out, *indent);
                out.push('#');
                out.push_str(text);
            }
            Out::Code {
                indent,
                segments,
                comment,
            } => {
                for (n, segment) in segments.iter().enumerate() {
                    if n > 0 {
                        out.push_str(" \\\n");
                        push_indent(&mut out, indent + CONTINUATION);
                    } else {
                        push_indent(&mut out, *indent);
                    }
                    out.push_str(segment);
                }
                if let Some(text) = comment {
                    let width = widths[i].unwrap_or(0);
                    for _ in width..columns[i] {
                        out.push(' ');
                    }
                    out.push('#');
                    out.push_str(text);
                }
            }
        }
        out.push('\n');
    }
    // No blank lines at the end.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Whether `line` is code on one physical line.
fn single_segment(line: &Out) -> bool {
    matches!(line, Out::Code { segments, .. } if segments.len() == 1)
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests;

//! The lexer and parser. Each physical line is lexed on its own (a quote
//! never spans lines), joined with the next one after a trailing ` \`, and
//! the resulting logical lines are nested by indentation.

use std::sync::Arc;

use super::{
    Comment, Directive, Item, Line, Parsed, Part, SYNTAX_VERSION, Script, ScriptError, Span, Var,
    VarRef, Word,
};

/// A directive body is indented this much more than its directive.
const INDENT: usize = 2;

/// Parses `src`, naming it `file` in spans. Never fails: syntax errors are
/// listed in [`Parsed::errors`] and their lines left out.
pub fn parse(file: &str, src: &str) -> Parsed {
    let mut lexer = Lexer::new(file, src);
    let lines = lexer.lines();
    let mut next = 0;
    let mut items = lexer.block(&lines, &mut next, 0);
    let syntax = lexer.syntax(&mut items);
    Parsed {
        script: Script {
            file: lexer.file.clone(),
            syntax,
            items,
        },
        errors: lexer.errors,
    }
}

/// A logical line.
struct Logical {
    indent: usize,
    /// Where its content starts, for errors about indentation.
    start: usize,
    kind: Kind,
}

enum Kind {
    Comment(Comment),
    Command(Line),
    Directive(Directive),
}

struct Lexer<'a> {
    src: &'a str,
    file: Arc<str>,
    /// Each physical line's content range, without its line break.
    lines: Vec<(usize, usize)>,
    errors: Vec<ScriptError>,
}

impl<'a> Lexer<'a> {
    fn new(file: &str, src: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                let end = if i > start && src.as_bytes()[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                lines.push((start, end));
                start = i + 1;
            }
        }
        if start < src.len() {
            let end = if src.ends_with('\r') {
                src.len() - 1
            } else {
                src.len()
            };
            lines.push((start, end));
        }
        Self {
            src,
            file: Arc::from(file),
            lines,
            errors: Vec::new(),
        }
    }

    /// The span of bytes `start..end`.
    fn span(&self, start: usize, end: usize) -> Span {
        // The physical line holding `start` (the one after it for a
        // position on a line break, which never happens for a token).
        let index = self.lines.partition_point(|&(s, _)| s <= start);
        let line_start = index.checked_sub(1).map_or(0, |i| self.lines[i].0);
        let column = self.src[line_start..start].chars().count() + 1;
        Span {
            file: self.file.clone(),
            line: u32::try_from(index.max(1)).unwrap_or(u32::MAX),
            column: u32::try_from(column).unwrap_or(u32::MAX),
            start,
            end,
        }
    }

    fn error(&mut self, start: usize, end: usize, message: impl Into<String>, hint: &str) {
        let e = ScriptError::new(self.span(start, end), message);
        self.errors
            .push(if hint.is_empty() { e } else { e.hint(hint) });
    }

    /// Lexes every line; blank ones and ones with errors are dropped.
    fn lines(&mut self) -> Vec<Logical> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.lines.len() {
            if let Some(line) = self.logical(&mut i) {
                out.push(line);
            }
            i += 1;
        }
        out
    }

    /// Lexes the logical line starting at physical line `*i`, leaving `*i`
    /// on its last physical line.
    fn logical(&mut self, i: &mut usize) -> Option<Logical> {
        let (start, end) = self.lines[*i];
        let text = &self.src[start..end];
        let indent = text.len() - text.trim_start_matches(' ').len();
        let content = start + indent;
        if text.trim().is_empty() {
            return None;
        }
        if self.src.as_bytes()[content] == b'\t' {
            self.error(
                content,
                content + 1,
                "a tab in the indentation",
                "indent with spaces: 2 per level",
            );
            return None;
        }
        if self.src.as_bytes()[content] == b'#' {
            return Some(Logical {
                indent,
                start: content,
                kind: Kind::Comment(self.comment(content, end)),
            });
        }
        let (words, comment) = self.words(i, content)?;
        let Some(first) = words.first() else {
            // A lone `\` continued onto a comment.
            return comment.map(|c| Logical {
                indent,
                start: content,
                kind: Kind::Comment(c),
            });
        };
        let kind = if let Some(name) = first.raw.strip_prefix('@') {
            if !is_directive_name(name) {
                let (start, end) = (first.span.start, first.span.end);
                let message = format!("bad directive name {}", first.raw);
                self.error(
                    start,
                    end,
                    message,
                    "directive names are lowercase letters, digits and -, e.g. @syntax",
                );
                return None;
            }
            let name = name.to_owned();
            let mut words = words;
            let head = words.remove(0);
            Kind::Directive(Directive {
                name,
                args: words,
                body: Vec::new(),
                span: head.span,
                comment,
            })
        } else {
            let text = words
                .iter()
                .map(|w| w.raw.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            Kind::Command(Line {
                span: first.span.clone(),
                words,
                text,
                comment,
            })
        };
        Some(Logical {
            indent,
            start: content,
            kind,
        })
    }

    fn comment(&self, hash: usize, end: usize) -> Comment {
        Comment {
            text: self.src[hash + 1..end].to_owned(),
            span: self.span(hash, end),
        }
    }

    /// The words from `pos` on physical line `*i` to the end of the logical
    /// line, and its comment. `None` after an error (already recorded).
    fn words(&mut self, i: &mut usize, mut pos: usize) -> Option<(Vec<Word>, Option<Comment>)> {
        let mut end = self.lines[*i].1;
        let mut words = Vec::new();
        let bytes = self.src.as_bytes();
        loop {
            while pos < end && matches!(bytes[pos], b' ' | b'\t') {
                pos += 1;
            }
            if pos >= end {
                return Some((words, None));
            }
            match bytes[pos] {
                b'#' => return Some((words, Some(self.comment(pos, end)))),
                b'\\' if pos + 1 == end => {
                    // A continuation: go on with the next physical line.
                    if *i + 1 >= self.lines.len() {
                        self.error(
                            pos,
                            end,
                            "a line continuation at the end of the file",
                            "remove the trailing \\",
                        );
                        return None;
                    }
                    *i += 1;
                    let (next, next_end) = self.lines[*i];
                    let text = &self.src[next..next_end];
                    if text.trim().is_empty() {
                        self.error(
                            pos,
                            end,
                            "a line continuation followed by a blank line",
                            "remove the trailing \\, or continue on the next line",
                        );
                        return None;
                    }
                    pos = next;
                    end = next_end;
                }
                _ => match self.word(pos, end) {
                    Ok((word, next)) => {
                        words.push(word);
                        pos = next;
                    }
                    Err(e) => {
                        self.errors.push(e);
                        // Skip the continuation lines of a broken line.
                        while self.src[self.lines[*i].0..self.lines[*i].1].ends_with(" \\")
                            && *i + 1 < self.lines.len()
                        {
                            *i += 1;
                        }
                        return None;
                    }
                },
            }
        }
    }

    /// One word from `pos` (not a space), and where it ends.
    fn word(&self, start: usize, end: usize) -> Result<(Word, usize), ScriptError> {
        let src = self.src;
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut quoted = false;
        let mut pos = start;
        while pos < end {
            let c = next_char(src, pos);
            match c {
                ' ' | '\t' => break,
                '\'' => {
                    quoted = true;
                    let Some(close) = src[pos + 1..end].find('\'') else {
                        return Err(ScriptError::new(
                            self.span(pos, end),
                            "a single quote without its closing quote",
                        )
                        .hint("close it on the same line; quotes never span lines"));
                    };
                    text.push_str(&src[pos + 1..pos + 1 + close]);
                    pos += close + 2;
                }
                '"' => {
                    quoted = true;
                    pos = self.double_quoted(pos, end, &mut text, &mut parts)?;
                }
                '\\' => {
                    if pos + 1 >= end {
                        return Err(ScriptError::new(
                            self.span(pos, end),
                            "a backslash at the end of a word",
                        )
                        .hint(
                            "a line continuation is a space and then \\ at the end of the line",
                        ));
                    }
                    quoted = true;
                    let escaped = next_char(src, pos + 1);
                    text.push(escaped);
                    pos += 1 + escaped.len_utf8();
                }
                '$' => pos = self.dollar(pos, end, &mut text, &mut parts)?,
                c => {
                    text.push(c);
                    pos += c.len_utf8();
                }
            }
        }
        if !text.is_empty() || parts.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok((
            Word {
                parts,
                quoted,
                raw: src[start..pos].to_owned(),
                span: self.span(start, pos),
            },
            pos,
        ))
    }

    /// A `"…"` string at `open`; returns the position after it.
    fn double_quoted(
        &self,
        open: usize,
        end: usize,
        text: &mut String,
        parts: &mut Vec<Part>,
    ) -> Result<usize, ScriptError> {
        let src = self.src;
        let unterminated = || {
            ScriptError::new(
                self.span(open, end),
                "a double quote without its closing quote",
            )
            .hint("close it on the same line; quotes never span lines")
        };
        let mut pos = open + 1;
        loop {
            if pos >= end {
                return Err(unterminated());
            }
            match next_char(src, pos) {
                '"' => return Ok(pos + 1),
                '\\' => {
                    if pos + 1 >= end {
                        return Err(unterminated());
                    }
                    let escaped = next_char(src, pos + 1);
                    match escaped {
                        '"' | '\\' => text.push(escaped),
                        'n' => text.push('\n'),
                        // Other backslashes stay, as in a shell: "\d+".
                        other => {
                            text.push('\\');
                            text.push(other);
                        }
                    }
                    pos += 1 + escaped.len_utf8();
                }
                '$' => pos = self.dollar(pos, end, text, parts)?,
                c => {
                    text.push(c);
                    pos += c.len_utf8();
                }
            }
        }
    }

    /// `$${` (a literal `${`), `${…}`, or a plain `$` at `pos`; returns the
    /// position after it.
    fn dollar(
        &self,
        pos: usize,
        end: usize,
        text: &mut String,
        parts: &mut Vec<Part>,
    ) -> Result<usize, ScriptError> {
        let rest = &self.src[pos..end];
        if rest.starts_with("$${") {
            text.push_str("${");
            return Ok(pos + 3);
        }
        if !rest.starts_with("${") {
            text.push('$');
            return Ok(pos + 1);
        }
        let Some(close) = rest.find('}') else {
            return Err(
                ScriptError::new(self.span(pos, end), "a ${ without its closing }")
                    .hint("close the variable with }, or write $${ for a literal ${"),
            );
        };
        let span = self.span(pos, pos + close + 1);
        let var = var(&rest[2..close])
            .map_err(|(message, hint)| ScriptError::new(span.clone(), message).hint(hint))?;
        if !text.is_empty() {
            parts.push(Part::Text(std::mem::take(text)));
        }
        parts.push(Part::Var(VarRef { var, span }));
        Ok(pos + close + 1)
    }

    /// The items indented exactly `indent`, from `lines[*next]` on, up to
    /// the first line indented less.
    fn block(&mut self, lines: &[Logical], next: &mut usize, indent: usize) -> Vec<Item> {
        let mut items = Vec::new();
        let mut body_allowed = false;
        while let Some(line) = lines.get(*next) {
            if line.indent < indent {
                break;
            }
            *next += 1;
            match &line.kind {
                // A comment may sit at any depth.
                Kind::Comment(c) => {
                    items.push(Item::Comment(c.clone()));
                }
                _ if line.indent > indent => {
                    let hint = if body_allowed {
                        format!(
                            "a directive's body is indented exactly {INDENT} spaces more than the directive"
                        )
                    } else {
                        "only a directive's body is indented; line it up with the line above"
                            .to_owned()
                    };
                    let end = line.start;
                    let start = end - (line.indent - indent);
                    self.error(
                        start,
                        end,
                        format!(
                            "unexpected indentation: {} spaces where {indent} were expected",
                            line.indent
                        ),
                        &hint,
                    );
                    // One error for the whole stray block.
                    while lines.get(*next).is_some_and(|l| l.indent > indent) {
                        *next += 1;
                    }
                }
                Kind::Command(c) => {
                    items.push(Item::Command(c.clone()));
                    body_allowed = false;
                }
                Kind::Directive(d) => {
                    let mut d = d.clone();
                    d.body = self.block(lines, next, indent + INDENT);
                    items.push(Item::Directive(d));
                    body_allowed = true;
                }
            }
        }
        items
    }

    /// Takes `@syntax N` out of `items`, checks it, and returns the
    /// version. It must be the first line other than comments.
    fn syntax(&mut self, items: &mut Vec<Item>) -> u32 {
        let first = items.iter().position(|i| !matches!(i, Item::Comment(_)));
        let mut version = SYNTAX_VERSION;
        if let Some(index) = first
            && let Item::Directive(d) = &items[index]
            && d.name == "syntax"
        {
            let Item::Directive(d) = items.remove(index) else {
                unreachable!()
            };
            version = self.syntax_version(&d);
        }
        self.misplaced_syntax(items);
        version
    }

    fn syntax_version(&mut self, d: &Directive) -> u32 {
        let usage = format!("write @syntax {SYNTAX_VERSION}");
        if let Some(item) = d.body.iter().find(|i| !matches!(i, Item::Comment(_))) {
            let span = item_span(item);
            self.error(
                span.start,
                span.end,
                "@syntax has no body",
                "start the line at column 1",
            );
        }
        let [arg] = d.args.as_slice() else {
            self.error(
                d.span.start,
                d.span.end,
                "@syntax takes one version number",
                &usage,
            );
            return SYNTAX_VERSION;
        };
        match arg.literal().and_then(|v| v.parse::<u32>().ok()) {
            Some(v) if (1..=SYNTAX_VERSION).contains(&v) => v,
            Some(v) if v > SYNTAX_VERSION => {
                self.error(
                    arg.span.start,
                    arg.span.end,
                    format!(
                        "syntax version {v} is newer than this chrome-cdp reads ({SYNTAX_VERSION})"
                    ),
                    "upgrade chrome-cdp",
                );
                SYNTAX_VERSION
            }
            _ => {
                self.error(
                    arg.span.start,
                    arg.span.end,
                    format!("bad syntax version {}", arg.raw),
                    &usage,
                );
                SYNTAX_VERSION
            }
        }
    }

    /// Reports every other `@syntax`, at any depth.
    fn misplaced_syntax(&mut self, items: &[Item]) {
        for item in items {
            if let Item::Directive(d) = item {
                if d.name == "syntax" {
                    self.error(
                        d.span.start,
                        d.span.end,
                        "@syntax must be the first line of the file",
                        "move it to the top, before any command or directive",
                    );
                }
                self.misplaced_syntax(&d.body);
            }
        }
    }
}

fn item_span(item: &Item) -> &Span {
    match item {
        Item::Command(l) => &l.span,
        Item::Directive(d) => &d.span,
        Item::Comment(c) => &c.span,
    }
}

/// The character at byte `pos`, which is a char boundary.
fn next_char(src: &str, pos: usize) -> char {
    src[pos..].chars().next().unwrap_or('\0')
}

/// The contents of `${…}`, or a message and hint.
fn var(inner: &str) -> Result<Var, (String, String)> {
    let shown = format!("${{{inner}}}");
    if inner.is_empty() {
        return Err((
            "an empty variable ${}".into(),
            "name a variable: ${name}, ${env:NAME}, …; write $${ for a literal ${".into(),
        ));
    }
    if let Some((ns, arg)) = inner.split_once(':') {
        let checked = |ok: bool, what: &str| {
            if ok {
                Ok(arg.to_owned())
            } else {
                Err((
                    format!("bad variable {shown}"),
                    format!("{ns}: takes {what}"),
                ))
            }
        };
        return match ns {
            "env" => checked(is_env_name(arg), "an environment variable name").map(Var::Env),
            "var" => checked(is_ident(arg), "a name").map(Var::Var),
            "secret" => checked(is_ident(arg), "a name").map(Var::Secret),
            "data" => {
                let (file, pointer) = arg.split_once('#').unwrap_or((arg, ""));
                if file.is_empty() || !(pointer.is_empty() || pointer.starts_with('/')) {
                    return Err((
                        format!("bad variable {shown}"),
                        "data: takes a JSON file and a JSON pointer: ${data:users.json#/0/email}"
                            .into(),
                    ));
                }
                Ok(Var::Data {
                    file: file.to_owned(),
                    pointer: pointer.to_owned(),
                })
            }
            _ => Err((
                format!("unknown variable kind {ns}: in {shown}"),
                "use ${name}, ${env:NAME}, ${var:name}, ${secret:name} or ${data:file#/pointer}"
                    .into(),
            )),
        };
    }
    let path: Vec<&str> = inner.split('.').collect();
    if !path.iter().all(|p| is_ident(p)) {
        return Err((
            format!("bad variable name {shown}"),
            "names are letters, digits, _ and -, starting with a letter or _".into(),
        ));
    }
    Ok(match path.as_slice() {
        [name] => Var::Name((*name).to_owned()),
        _ => Var::Path(path.into_iter().map(str::to_owned).collect()),
    })
}

/// `[A-Za-z_][A-Za-z0-9_-]*`.
pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
fn is_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `[a-z][a-z0-9-]*`.
fn is_directive_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

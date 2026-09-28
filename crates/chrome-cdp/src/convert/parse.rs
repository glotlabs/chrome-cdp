//! A forgiving parser for the JavaScript a Playwright test is made of:
//! statements, calls on member chains, arrow functions, object and array
//! literals. It does not reject anything: a statement it cannot read is
//! [`StmtKind::Other`], and an expression it cannot read is
//! [`Expr::Other`], which `convert` turns into a TODO line.

use super::lex::{Kind, TemplatePart, Token, tokenize};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Stmt {
    pub(super) kind: StmtKind,
    /// Byte offsets of the statement in the source.
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum StmtKind {
    Expr(Expr),
    /// `const`, `let` or `var` with a value.
    Let {
        pattern: Pattern,
        init: Expr,
    },
    /// `import …`, `export …` without a value, TypeScript `type` and
    /// `interface` declarations: nothing to convert.
    Ignored,
    /// Anything else: `if`, `for`, `function`, `class`, `return`, …
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Pattern {
    Name(String),
    /// `[a, , b]`.
    Array(Vec<Option<String>>),
    /// `{ a, b }`.
    Object(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Expr {
    Name(String),
    Str(String),
    Template(Vec<TemplatePart>),
    Num(String),
    Regex {
        pattern: String,
        flags: String,
    },
    Array(Vec<Expr>),
    Object(Vec<Prop>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Await(Box<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Func(Func),
    Spread(Box<Expr>),
    /// Parsed past, not understood.
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Prop {
    Pair(String, Expr),
    Spread(Expr),
    /// A computed key or a method.
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Func {
    /// Every name in the parameter list (`({ page }: Args)` gives `page`
    /// and `Args`): enough to know which fixtures a test takes.
    pub(super) params: Vec<String>,
    pub(super) body: Body,
    /// The function's source, e.g. for `wait --fn`.
    pub(super) source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Body {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

impl Expr {
    /// The dotted path of a member chain of names: `page.keyboard.press`.
    pub(super) fn path(&self) -> Option<String> {
        match self {
            Expr::Name(name) => Some(name.clone()),
            Expr::Member(object, name) => Some(format!("{}.{name}", object.path()?)),
            _ => None,
        }
    }

    /// Without a leading `await`.
    pub(super) fn awaited(&self) -> &Expr {
        match self {
            Expr::Await(inner) => inner.awaited(),
            other => other,
        }
    }
}

/// Parses a whole file.
pub(super) fn parse(src: &str) -> Vec<Stmt> {
    let tokens = tokenize(src);
    let pairs = pairs(&tokens);
    let parser = Parser {
        src,
        tokens: &tokens,
        pairs: &pairs,
    };
    parser.statements(0, tokens.len())
}

/// For each opening bracket, the index of its closing one.
fn pairs(tokens: &[Token]) -> Vec<Option<usize>> {
    let mut out = vec![None; tokens.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.is("(") || t.is("[") || t.is("{") {
            stack.push(i);
        } else if (t.is(")") || t.is("]") || t.is("}"))
            && let Some(open) = stack.pop()
        {
            out[open] = Some(i);
        }
    }
    out
}

struct Parser<'a> {
    src: &'a str,
    tokens: &'a [Token],
    pairs: &'a [Option<usize>],
}

/// Names that go on with the statement when a line starts with them.
const CONTINUE_NAMES: &[&str] = &[
    "as",
    "in",
    "instanceof",
    "else",
    "catch",
    "finally",
    "satisfies",
];

/// Punctuation that goes on with the statement when a line starts with it.
const CONTINUE_PUNCTS: &[&str] = &[
    ".", "?.", "=>", "=", "==", "===", "!=", "!==", "+", "*", "/", "%", "&&", "||", "??", "?", ":",
    ",", ")", "]", "}", "<", ">", "<=", ">=", "{", "&", "|", "^", "+=", "-=",
];

/// Operators the expression parser knows, loosest first.
const BINARY: &[(&str, u8)] = &[
    ("??", 1),
    ("||", 1),
    ("&&", 2),
    ("|", 3),
    ("^", 3),
    ("&", 3),
    ("==", 4),
    ("!=", 4),
    ("===", 4),
    ("!==", 4),
    ("<", 5),
    (">", 5),
    ("<=", 5),
    (">=", 5),
    ("+", 6),
    ("-", 6),
    ("*", 7),
    ("/", 7),
    ("%", 7),
    ("**", 8),
];

impl Parser<'_> {
    /// The statements of `tokens[from..to]`.
    fn statements(&self, from: usize, to: usize) -> Vec<Stmt> {
        let mut out = Vec::new();
        let mut i = from;
        while i < to {
            if self.tokens[i].is(";") {
                i += 1;
                continue;
            }
            let end = self.statement_end(i, to);
            let body_end = if end > i && self.tokens[end - 1].is(";") {
                end - 1
            } else {
                end
            };
            out.push(Stmt {
                kind: self.statement(i, body_end),
                start: self.tokens[i].start,
                end: self.tokens[end.max(i + 1) - 1].end,
            });
            i = end.max(i + 1);
        }
        out
    }

    /// Where the statement starting at `from` ends: after its `;`, or
    /// before a line that starts a new statement.
    fn statement_end(&self, from: usize, to: usize) -> usize {
        let mut i = from;
        while i < to {
            let t = &self.tokens[i];
            if i > from
                && t.newline_before
                && Self::ends(&self.tokens[i - 1])
                && !Self::continues(t)
            {
                return i;
            }
            if t.is(";") {
                return i + 1;
            }
            i = match self.pairs[i] {
                Some(close) if close < to => close + 1,
                _ => i + 1,
            };
        }
        to
    }

    /// A token that can end a statement.
    fn ends(t: &Token) -> bool {
        match &t.kind {
            Kind::Name(_)
            | Kind::Str(_)
            | Kind::Template(_)
            | Kind::Num(_)
            | Kind::Regex { .. } => true,
            Kind::Punct(p) => matches!(*p, ")" | "]" | "}" | "++" | "--" | "!"),
        }
    }

    /// A token that, starting a line, goes on with the statement before.
    fn continues(t: &Token) -> bool {
        match &t.kind {
            Kind::Name(name) => CONTINUE_NAMES.contains(&name.as_str()),
            Kind::Punct(p) => CONTINUE_PUNCTS.contains(p),
            _ => false,
        }
    }

    fn statement(&self, from: usize, to: usize) -> StmtKind {
        let Some(first) = self.tokens.get(from).filter(|_| from < to) else {
            return StmtKind::Ignored;
        };
        if let Kind::Name(word) = &first.kind {
            match word.as_str() {
                "import" if !self.tokens.get(from + 1).is_some_and(|t| t.is("(")) => {
                    return StmtKind::Ignored;
                }
                "export" => return self.statement(from + 1, to),
                "type" | "interface" | "declare"
                    if self
                        .tokens
                        .get(from + 1)
                        .is_some_and(|t| matches!(t.kind, Kind::Name(_))) =>
                {
                    return StmtKind::Ignored;
                }
                "const" | "let" | "var" => return self.declaration(from + 1, to),
                "if" | "for" | "while" | "do" | "switch" | "try" | "function" | "class"
                | "return" | "throw" | "break" | "continue" | "enum" => return StmtKind::Other,
                _ => {}
            }
        }
        let mut cursor = Cursor {
            parser: self,
            i: from,
            to,
        };
        match cursor.expr(0) {
            Some(expr) if cursor.i == to => StmtKind::Expr(expr),
            _ => StmtKind::Other,
        }
    }

    /// `name = value`, `{ a, b } = value` or `[a, b] = value`, with an
    /// optional type annotation. Several declarators are `Other`.
    fn declaration(&self, from: usize, to: usize) -> StmtKind {
        let Some(first) = self.tokens.get(from) else {
            return StmtKind::Other;
        };
        let (pattern, mut i) = match &first.kind {
            Kind::Name(name) => (Pattern::Name(name.clone()), from + 1),
            Kind::Punct("[" | "{") => {
                let Some(close) = self.pairs[from].filter(|c| *c < to) else {
                    return StmtKind::Other;
                };
                let names = |i: usize| -> Option<String> {
                    match &self.tokens[i].kind {
                        Kind::Name(n) => Some(n.clone()),
                        _ => None,
                    }
                };
                let pattern = if first.is("[") {
                    let mut items = Vec::new();
                    let mut current = None;
                    for i in from + 1..close {
                        if self.tokens[i].is(",") {
                            items.push(current.take());
                        } else if current.is_none() {
                            current = names(i);
                        }
                    }
                    items.push(current);
                    Pattern::Array(items)
                } else {
                    Pattern::Object((from + 1..close).filter_map(names).collect())
                };
                (pattern, close + 1)
            }
            _ => return StmtKind::Other,
        };
        // A type annotation: skip to the `=` at this level.
        if self.tokens.get(i).is_some_and(|t| t.is(":")) {
            while i < to && !self.tokens[i].is("=") {
                i = self.pairs[i].map_or(i + 1, |c| c + 1);
            }
        }
        if !self.tokens.get(i).is_some_and(|t| t.is("=")) || i + 1 >= to {
            return StmtKind::Other;
        }
        let mut cursor = Cursor {
            parser: self,
            i: i + 1,
            to,
        };
        match cursor.expr(0) {
            Some(init) if cursor.i == to => StmtKind::Let { pattern, init },
            _ => StmtKind::Other,
        }
    }
}

/// Reads one expression from `tokens[i..to]`.
struct Cursor<'p, 'a> {
    parser: &'p Parser<'a>,
    i: usize,
    to: usize,
}

impl<'a> Cursor<'_, 'a> {
    fn peek(&self) -> Option<&'a Token> {
        let tokens: &'a [Token] = self.parser.tokens;
        tokens.get(self.i).filter(|_| self.i < self.to)
    }

    fn peek_is(&self, punct: &str) -> bool {
        self.peek().is_some_and(|t| t.is(punct))
    }

    fn eat(&mut self, punct: &str) -> bool {
        let yes = self.peek_is(punct);
        if yes {
            self.i += 1;
        }
        yes
    }

    fn close_of(&self, open: usize) -> Option<usize> {
        self.parser.pairs[open].filter(|c| *c < self.to)
    }

    fn expr(&mut self, min: u8) -> Option<Expr> {
        let mut left = self.unary()?;
        while let Some(t) = self.peek() {
            if t.is_name("as") || t.is_name("satisfies") {
                // A TypeScript cast: skip the type.
                self.i += 1;
                self.skip_type();
                continue;
            }
            if t.is("?") && min == 0 {
                // A conditional: read past it.
                self.i += 1;
                self.expr(0)?;
                if !self.eat(":") {
                    return None;
                }
                self.expr(0)?;
                left = Expr::Other;
                continue;
            }
            if t.is("=") || t.is("+=") || t.is("-=") {
                self.i += 1;
                self.expr(0)?;
                left = Expr::Other;
                continue;
            }
            let Some((op, prec)) = BINARY.iter().find(|(op, _)| t.is(op)).copied() else {
                break;
            };
            if prec < min.max(1) {
                break;
            }
            self.i += 1;
            let right = self.expr(prec + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Some(left)
    }

    /// A type after `as`: a name path with type arguments, or `const`.
    fn skip_type(&mut self) {
        while let Some(t) = self.peek() {
            if matches!(t.kind, Kind::Name(_)) || t.is(".") {
                self.i += 1;
            } else if t.is("<") {
                let mut depth = 0;
                while let Some(t) = self.peek() {
                    self.i += 1;
                    if t.is("<") {
                        depth += 1;
                    } else if t.is(">") {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            } else if t.is("[")
                && self
                    .parser
                    .tokens
                    .get(self.i + 1)
                    .is_some_and(|n| n.is("]"))
            {
                self.i += 2;
            } else {
                break;
            }
        }
    }

    fn unary(&mut self) -> Option<Expr> {
        let t = self.peek()?;
        if t.is_name("await") {
            self.i += 1;
            return Some(Expr::Await(Box::new(self.unary()?)));
        }
        if t.is_name("new") {
            self.i += 1;
            // `new Date('…')` reads as `Date('…')`; the rest is not understood.
            return Some(match self.postfix()? {
                call @ Expr::Call(..) if matches!(&call, Expr::Call(f, _) if f.path().as_deref() == Some("Date")) => {
                    call
                }
                _ => Expr::Other,
            });
        }
        for op in ["!", "-", "+", "~"] {
            if t.is(op) {
                self.i += 1;
                return Some(Expr::Unary(op, Box::new(self.unary()?)));
            }
        }
        if t.is_name("typeof") || t.is_name("void") || t.is_name("delete") {
            self.i += 1;
            self.unary()?;
            return Some(Expr::Other);
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Option<Expr> {
        let mut expr = self.primary()?;
        while let Some(t) = self.peek() {
            if t.is(".") || t.is("?.") {
                self.i += 1;
                if self.peek_is("(") || self.peek_is("[") {
                    continue;
                }
                let Kind::Name(name) = &self.peek()?.kind else {
                    return None;
                };
                expr = Expr::Member(Box::new(expr), name.clone());
                self.i += 1;
            } else if t.is("(") && !t.newline_before {
                let close = self.close_of(self.i)?;
                let args = self.list(self.i + 1, close)?;
                self.i = close + 1;
                expr = Expr::Call(Box::new(expr), args);
            } else if t.is("[") && !t.newline_before {
                let close = self.close_of(self.i)?;
                let mut inner = Cursor {
                    parser: self.parser,
                    i: self.i + 1,
                    to: close,
                };
                let index = inner.expr(0)?;
                if inner.i != close {
                    return None;
                }
                self.i = close + 1;
                expr = Expr::Index(Box::new(expr), Box::new(index));
            } else if t.is("!") && self.non_null() {
                // TypeScript's non-null assertion: `process.env.X!`.
                self.i += 1;
            } else if t.is("<") && self.type_arguments() {
                // `f<Type>(…)`: skip the type arguments.
            } else if matches!(t.kind, Kind::Template(_)) && !t.newline_before {
                // A tagged template.
                self.i += 1;
                expr = Expr::Other;
            } else {
                break;
            }
        }
        Some(expr)
    }

    /// The `!` at the cursor is a postfix non-null assertion.
    fn non_null(&self) -> bool {
        match self.parser.tokens.get(self.i + 1) {
            None => true,
            Some(next) if self.i + 1 >= self.to => {
                let _ = next;
                true
            }
            Some(next) => {
                next.newline_before
                    || [")", "]", "}", ",", ";", ".", "?."]
                        .iter()
                        .any(|p| next.is(p))
            }
        }
    }

    /// Skips `<…>` type arguments before a call, if that is what they are.
    fn type_arguments(&mut self) -> bool {
        let mut j = self.i + 1;
        let mut depth = 1;
        while j < self.to {
            let t = &self.parser.tokens[j];
            if t.is("<") {
                depth += 1;
            } else if t.is(">") {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if !(matches!(t.kind, Kind::Name(_) | Kind::Str(_))
                || t.is(",")
                || t.is(".")
                || t.is("|")
                || t.is("[")
                || t.is("]")
                || t.is("{")
                || t.is("}")
                || t.is(":")
                || t.is(";"))
            {
                return false;
            }
            j += 1;
        }
        if depth != 0 || !self.parser.tokens.get(j + 1).is_some_and(|t| t.is("(")) {
            return false;
        }
        self.i = j + 1;
        true
    }

    /// Comma-separated expressions in `tokens[from..to]` (holes allowed).
    fn list(&self, from: usize, to: usize) -> Option<Vec<Expr>> {
        let mut out = Vec::new();
        let mut inner = Cursor {
            parser: self.parser,
            i: from,
            to,
        };
        while inner.i < to {
            if inner.peek_is(",") {
                inner.i += 1;
                continue;
            }
            let item = if inner.eat("...") {
                Expr::Spread(Box::new(inner.expr(0)?))
            } else {
                inner.expr(0)?
            };
            out.push(item);
            if inner.i < to && !inner.eat(",") {
                return None;
            }
        }
        Some(out)
    }

    fn primary(&mut self) -> Option<Expr> {
        let start = self.i;
        let t = self.peek()?.clone();
        match &t.kind {
            Kind::Name(name) if name == "async" => {
                let next = self.parser.tokens.get(self.i + 1)?;
                if next.is("(") || matches!(next.kind, Kind::Name(_)) {
                    self.i += 1;
                    if next.is_name("function") {
                        return self.function(start);
                    }
                    return self.arrow(start);
                }
                self.i += 1;
                Some(Expr::Name(name.clone()))
            }
            Kind::Name(name) if name == "function" => self.function(start),
            Kind::Name(name) => {
                if self
                    .parser
                    .tokens
                    .get(self.i + 1)
                    .is_some_and(|n| n.is("=>"))
                    && self.i + 1 < self.to
                {
                    return self.arrow(start);
                }
                self.i += 1;
                Some(Expr::Name(name.clone()))
            }
            Kind::Str(s) => {
                self.i += 1;
                Some(Expr::Str(s.clone()))
            }
            Kind::Template(parts) => {
                self.i += 1;
                Some(Expr::Template(parts.clone()))
            }
            Kind::Num(n) => {
                self.i += 1;
                Some(Expr::Num(n.clone()))
            }
            Kind::Regex { pattern, flags } => {
                self.i += 1;
                Some(Expr::Regex {
                    pattern: pattern.clone(),
                    flags: flags.clone(),
                })
            }
            Kind::Punct("(") => {
                let close = self.close_of(self.i)?;
                if self.arrow_after(close) {
                    return self.arrow(start);
                }
                let mut inner = Cursor {
                    parser: self.parser,
                    i: self.i + 1,
                    to: close,
                };
                let expr = inner.expr(0)?;
                if inner.i != close {
                    return None;
                }
                self.i = close + 1;
                Some(expr)
            }
            Kind::Punct("[") => {
                let close = self.close_of(self.i)?;
                let items = self.list(self.i + 1, close)?;
                self.i = close + 1;
                Some(Expr::Array(items))
            }
            Kind::Punct("{") => {
                let close = self.close_of(self.i)?;
                let props = self.object(self.i + 1, close)?;
                self.i = close + 1;
                Some(Expr::Object(props))
            }
            Kind::Punct(_) => None,
        }
    }

    /// The `)` at `close` is followed by `=>`, maybe after a return type.
    fn arrow_after(&self, close: usize) -> bool {
        let tokens = self.parser.tokens;
        let mut j = close + 1;
        if tokens.get(j).is_some_and(|t| t.is(":")) {
            while j < self.to && !tokens[j].is("=>") {
                j += 1;
            }
        }
        j < self.to && tokens[j].is("=>")
    }

    /// An arrow function from its parameters (the cursor is past `async`).
    fn arrow(&mut self, start: usize) -> Option<Expr> {
        let tokens = self.parser.tokens;
        let params_end = if self.peek_is("(") {
            self.close_of(self.i)? + 1
        } else {
            self.i + 1
        };
        let params = (self.i..params_end)
            .filter_map(|j| match &tokens[j].kind {
                Kind::Name(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        self.i = params_end;
        while self.i < self.to && !self.peek_is("=>") {
            self.i += 1;
        }
        if !self.eat("=>") {
            return None;
        }
        let body = if self.peek_is("{") {
            let close = self.close_of(self.i)?;
            let stmts = self.parser.statements(self.i + 1, close);
            self.i = close + 1;
            Body::Block(stmts)
        } else {
            Body::Expr(Box::new(self.expr(1)?))
        };
        Some(Expr::Func(Func {
            params,
            body,
            source: self.source(start),
        }))
    }

    /// `function name(params) { … }` (the cursor is at `function`).
    fn function(&mut self, start: usize) -> Option<Expr> {
        self.i += 1;
        self.eat("*");
        if matches!(self.peek()?.kind, Kind::Name(_)) {
            self.i += 1;
        }
        if !self.peek_is("(") {
            return None;
        }
        let close = self.close_of(self.i)?;
        let params = (self.i..close)
            .filter_map(|j| match &self.parser.tokens[j].kind {
                Kind::Name(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        self.i = close + 1;
        while self.i < self.to && !self.peek_is("{") {
            self.i += 1;
        }
        let open = self.i;
        let close = self.close_of(open)?;
        let body = Body::Block(self.parser.statements(open + 1, close));
        self.i = close + 1;
        Some(Expr::Func(Func {
            params,
            body,
            source: self.source(start),
        }))
    }

    /// The object literal's properties in `tokens[from..to]`.
    fn object(&self, from: usize, to: usize) -> Option<Vec<Prop>> {
        let tokens = self.parser.tokens;
        let mut out = Vec::new();
        let mut inner = Cursor {
            parser: self.parser,
            i: from,
            to,
        };
        while inner.i < to {
            if inner.eat(",") {
                continue;
            }
            if inner.eat("...") {
                out.push(Prop::Spread(inner.expr(0)?));
            } else {
                let key = match &tokens[inner.i].kind {
                    Kind::Name(n) | Kind::Str(n) | Kind::Num(n) => Some(n.clone()),
                    _ => None,
                };
                match key {
                    Some(key)
                        if tokens.get(inner.i + 1).is_some_and(|t| t.is(":"))
                            && inner.i + 1 < to =>
                    {
                        inner.i += 2;
                        out.push(Prop::Pair(key, inner.expr(0)?));
                    }
                    Some(key) if inner.i + 1 >= to || tokens[inner.i + 1].is(",") => {
                        inner.i += 1;
                        out.push(Prop::Pair(key.clone(), Expr::Name(key)));
                    }
                    _ => {
                        // A method or a computed key: skip to the next comma.
                        while inner.i < to && !inner.peek_is(",") {
                            inner.i = self.parser.pairs[inner.i].map_or(inner.i + 1, |c| c + 1);
                        }
                        out.push(Prop::Other);
                    }
                }
            }
            if inner.i < to && !inner.eat(",") {
                return None;
            }
        }
        Some(out)
    }

    /// The source from token `start` to the cursor.
    fn source(&self, start: usize) -> String {
        let tokens = self.parser.tokens;
        let from = tokens[start].start;
        let to = tokens[self.i.max(start + 1) - 1].end;
        self.parser.src[from..to].to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{Body, Expr, Pattern, Prop, StmtKind, parse};

    fn one(src: &str) -> StmtKind {
        let stmts = parse(src);
        assert_eq!(stmts.len(), 1, "{stmts:#?}");
        stmts.into_iter().next().unwrap().kind
    }

    fn call(path: &str, args: Vec<Expr>) -> Expr {
        let mut parts = path.split('.');
        let mut e = Expr::Name(parts.next().unwrap().into());
        for p in parts {
            e = Expr::Member(Box::new(e), p.into());
        }
        Expr::Call(Box::new(e), args)
    }

    #[test]
    fn reads_awaited_call_chains() {
        let StmtKind::Expr(e) = one("await page.getByRole('button', { name: 'Go' }).click();")
        else {
            panic!()
        };
        let inner = call(
            "page.getByRole",
            vec![
                Expr::Str("button".into()),
                Expr::Object(vec![Prop::Pair("name".into(), Expr::Str("Go".into()))]),
            ],
        );
        assert_eq!(
            e,
            Expr::Await(Box::new(Expr::Call(
                Box::new(Expr::Member(Box::new(inner), "click".into())),
                vec![]
            )))
        );
    }

    #[test]
    fn splits_statements_without_semicolons() {
        let stmts = parse("await a()\nawait b\n  .c()\nconst x = 1\n");
        assert_eq!(stmts.len(), 3);
        assert!(
            matches!(&stmts[2].kind, StmtKind::Let { pattern: Pattern::Name(n), .. } if n == "x")
        );
    }

    #[test]
    fn reads_tests_and_their_bodies() {
        let StmtKind::Expr(Expr::Call(_, args)) = one(
            "test('a', async ({ page }: { page: Page }) => {\n  await page.goto('/')\n  if (x) { y() }\n})",
        ) else {
            panic!()
        };
        let Expr::Func(f) = &args[1] else { panic!() };
        assert!(f.params.contains(&"page".to_owned()));
        let Body::Block(body) = &f.body else { panic!() };
        assert_eq!(body.len(), 2);
        assert!(matches!(body[1].kind, StmtKind::Other));
    }

    #[test]
    fn skips_types_and_non_null_assertions() {
        let StmtKind::Let { init, .. } = one("const p: string = process.env.PASSWORD!;") else {
            panic!()
        };
        assert_eq!(init.path().as_deref(), Some("process.env.PASSWORD"));
        assert!(matches!(
            one("import { test } from '@playwright/test';"),
            StmtKind::Ignored
        ));
        assert!(matches!(one("type A = { b: string }"), StmtKind::Ignored));
        let StmtKind::Let { pattern, .. } =
            one("const [download] = await Promise.all([a(), b()]);")
        else {
            panic!()
        };
        assert_eq!(pattern, Pattern::Array(vec![Some("download".into())]));
    }

    #[test]
    fn keeps_function_source() {
        let StmtKind::Expr(e) = one("await page.waitForFunction(() => window.ready === true)")
        else {
            panic!()
        };
        let Expr::Call(_, args) = e.awaited() else {
            panic!()
        };
        let Expr::Func(f) = &args[0] else { panic!() };
        assert_eq!(f.source, "() => window.ready === true");
    }
}

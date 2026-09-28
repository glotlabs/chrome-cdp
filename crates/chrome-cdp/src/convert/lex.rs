//! Tokens of JavaScript and TypeScript source, as much as `convert` needs:
//! names, strings, template literals, numbers, regex literals and
//! punctuation, with comments dropped. Anything it cannot read becomes a
//! one-character punctuation token, so a file always lexes and the parser
//! turns what it does not understand into TODO lines.

/// One token and where it is in the source.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Token {
    pub(super) kind: Kind,
    /// Byte offsets of the token in the source.
    pub(super) start: usize,
    pub(super) end: usize,
    /// A line break comes between the previous token and this one.
    pub(super) newline_before: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Kind {
    /// A name or keyword.
    Name(String),
    /// A string literal, escapes resolved.
    Str(String),
    /// A template literal: text, and the source of each `${…}`.
    Template(Vec<TemplatePart>),
    /// A number, as written.
    Num(String),
    Regex {
        pattern: String,
        flags: String,
    },
    Punct(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum TemplatePart {
    Text(String),
    /// The source between `${` and `}`.
    Expr(String),
}

impl Token {
    pub(super) fn is(&self, punct: &str) -> bool {
        matches!(&self.kind, Kind::Punct(p) if *p == punct)
    }

    pub(super) fn is_name(&self, name: &str) -> bool {
        matches!(&self.kind, Kind::Name(n) if n == name)
    }
}

/// Longest first, so `===` wins over `==`.
const PUNCTS: &[&str] = &[
    "...", "===", "!==", "**=", "<<=", ">>=", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=",
    "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**", "<<",
    ">>", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%", "&", "|", "^",
    "!", "~", "?", ":", "=", ".", "@", "#",
];

/// After these names a `/` starts a regex, not a division.
const REGEX_AFTER: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "await",
    "yield",
];

pub(super) fn tokenize(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;
    let mut newline = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            newline = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            i = src[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        if src[i..].starts_with("/*") {
            let end = src[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |n| i + 2 + n + 2);
            newline |= src[i..end].contains('\n');
            i = end;
            continue;
        }
        let start = i;
        let kind = if c == b'"' || c == b'\'' {
            let (text, end) = string(src, i);
            i = end;
            Kind::Str(text)
        } else if c == b'`' {
            let (parts, end) = template(src, i);
            i = end;
            Kind::Template(parts)
        } else if c.is_ascii_digit()
            || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let end = src[i..]
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'))
                .map_or(bytes.len(), |n| i + n);
            let text = src[i..end].replace('_', "");
            i = end;
            Kind::Num(text)
        } else if is_name_start(src[i..].chars().next().unwrap_or(' ')) {
            let end = src[i..]
                .find(|ch: char| !is_name_char(ch))
                .map_or(bytes.len(), |n| i + n);
            let name = src[i..end].to_owned();
            i = end;
            Kind::Name(name)
        } else if c == b'/' && regex_allowed(tokens.last()) {
            if let Some((pattern, flags, end)) = regex(src, i) {
                i = end;
                Kind::Regex { pattern, flags }
            } else {
                i += 1;
                Kind::Punct("/")
            }
        } else {
            if let Some(p) = PUNCTS.iter().find(|p| src[i..].starts_with(**p)) {
                i += p.len();
                Kind::Punct(p)
            } else {
                // Not JavaScript we know: skip the character.
                i += src[i..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: i,
            newline_before: newline,
        });
        newline = false;
    }
    tokens
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn regex_allowed(prev: Option<&Token>) -> bool {
    match prev.map(|t| &t.kind) {
        None => true,
        Some(Kind::Name(name)) => REGEX_AFTER.contains(&name.as_str()),
        Some(Kind::Punct(p)) => !matches!(*p, ")" | "]" | "}" | "++" | "--"),
        Some(_) => false,
    }
}

/// A quoted string from `start` (the quote); its text and the end offset.
fn string(src: &str, start: usize) -> (String, usize) {
    let quote = src.as_bytes()[start] as char;
    let mut out = String::new();
    let mut chars = src[start + 1..].char_indices();
    while let Some((n, c)) = chars.next() {
        match c {
            c if c == quote => return (out, start + 1 + n + 1),
            '\n' => return (out, start + 1 + n),
            '\\' => escape(&mut chars, &mut out),
            c => out.push(c),
        }
    }
    (out, src.len())
}

/// Resolves the escape after a backslash into `out`.
fn escape(chars: &mut std::str::CharIndices<'_>, out: &mut String) {
    let Some((_, c)) = chars.next() else { return };
    match c {
        'n' => out.push('\n'),
        't' => out.push('\t'),
        'r' => out.push('\r'),
        'b' => out.push('\u{8}'),
        'f' => out.push('\u{c}'),
        'v' => out.push('\u{b}'),
        '0' => out.push('\0'),
        '\n' => {}
        'x' => {
            let hex: String = chars.by_ref().take(2).map(|(_, c)| c).collect();
            out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
        }
        'u' => {
            let rest = chars.as_str();
            let (hex, used) = if let Some(braced) = rest.strip_prefix('{') {
                let close = braced.find('}').unwrap_or(braced.len());
                (&braced[..close], close + 2)
            } else {
                (&rest[..rest.len().min(4)], 4)
            };
            out.extend(u32::from_str_radix(hex, 16).ok().and_then(char::from_u32));
            for _ in 0..used {
                chars.next();
            }
        }
        c => out.push(c),
    }
}

/// A template literal from `start` (the backtick).
fn template(src: &str, start: usize) -> (Vec<TemplatePart>, usize) {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut chars = src[start + 1..].char_indices();
    while let Some((n, c)) = chars.next() {
        match c {
            '`' => {
                if !text.is_empty() {
                    parts.push(TemplatePart::Text(text));
                }
                return (parts, start + 1 + n + 1);
            }
            '\\' => escape(&mut chars, &mut text),
            '$' if chars.as_str().starts_with('{') => {
                chars.next();
                if !text.is_empty() {
                    parts.push(TemplatePart::Text(std::mem::take(&mut text)));
                }
                let from = start + 1 + n + 2;
                let mut depth = 1;
                let mut to = src.len();
                for (m, c) in chars.by_ref() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                to = start + 1 + m;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                parts.push(TemplatePart::Expr(
                    src[from..to.max(from)].trim().to_owned(),
                ));
            }
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        parts.push(TemplatePart::Text(text));
    }
    (parts, src.len())
}

/// A regex literal from `start` (the slash): pattern, flags and end, or
/// `None` when the line ends first.
fn regex(src: &str, start: usize) -> Option<(String, String, usize)> {
    let mut in_class = false;
    let mut escaped = false;
    for (n, c) in src[start + 1..].char_indices() {
        let at = start + 1 + n;
        match c {
            '\n' => return None,
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => in_class = true,
            ']' => in_class = false,
            '/' if !in_class => {
                let pattern = src[start + 1..at].to_owned();
                let flags_end = src[at + 1..]
                    .find(|ch: char| !ch.is_ascii_alphabetic())
                    .map_or(src.len(), |m| at + 1 + m);
                return Some((pattern, src[at + 1..flags_end].to_owned(), flags_end));
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Kind, TemplatePart, tokenize};

    fn kinds(src: &str) -> Vec<Kind> {
        tokenize(src).into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn reads_strings_names_and_punctuation() {
        assert_eq!(
            kinds(r#"await page.fill('a\'b', "c\n") // comment"#),
            vec![
                Kind::Name("await".into()),
                Kind::Name("page".into()),
                Kind::Punct("."),
                Kind::Name("fill".into()),
                Kind::Punct("("),
                Kind::Str("a'b".into()),
                Kind::Punct(","),
                Kind::Str("c\n".into()),
                Kind::Punct(")"),
            ]
        );
        assert_eq!(
            kinds("a?.b !== c"),
            vec![
                Kind::Name("a".into()),
                Kind::Punct("?."),
                Kind::Name("b".into()),
                Kind::Punct("!=="),
                Kind::Name("c".into()),
            ]
        );
        assert_eq!(
            kinds("'\\u0041\\x42\\u{43}'"),
            vec![Kind::Str("ABC".into())]
        );
    }

    #[test]
    fn tells_regexes_from_division() {
        assert_eq!(
            kinds("x(/a\\/b[/]/i)")[2],
            Kind::Regex {
                pattern: "a\\/b[/]".into(),
                flags: "i".into()
            }
        );
        assert_eq!(kinds("a / b / c")[1], Kind::Punct("/"));
        assert_eq!(kinds("f() / 2")[3], Kind::Punct("/"));
    }

    #[test]
    fn splits_templates() {
        assert_eq!(
            kinds("`a ${ b.c } d`"),
            vec![Kind::Template(vec![
                TemplatePart::Text("a ".into()),
                TemplatePart::Expr("b.c".into()),
                TemplatePart::Text(" d".into()),
            ])]
        );
    }

    #[test]
    fn marks_line_breaks() {
        let tokens = tokenize("a /* x\n */ b\nc");
        let breaks: Vec<bool> = tokens.iter().map(|t| t.newline_before).collect();
        assert_eq!(breaks, vec![false, true, true]);
    }
}

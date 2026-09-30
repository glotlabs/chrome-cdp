//! `expect-snapshot` baselines as templates (plan 14, milestone 14.7).
//!
//! A baseline line may leave a value open:
//!
//! - `"*"` as a whole name or text, or a field's `value=`: any value
//!   (`button "*"`, `textbox "Since" value="*"`);
//! - `/regex/` (or `/regex/i`) in place of the quoted name, text or
//!   `value=`: a value the regex finds a match in (`heading /Order #\d+/`);
//! - `→ "*"` in place of a link's href: any href. An href is not quoted,
//!   and `/docs/` is one, so it takes no regex;
//! - a line `...`: any number of lines at its depth (with the lines below
//!   them).
//!
//! Matching is line by line on the `snapshot -i` text, the rest of a line
//! as it is. One alignment of the template with the page decides both
//! whether they match and how the page reads in the template's terms
//! ([`Template::project`]): each line the template matches as the
//! template has it, the lines a `...` stands for as that `...`, the rest
//! as the page has them. A failure's diff is from the template to that,
//! so the open values are not noise; `--update` stores it, so it keeps
//! the template lines that still match and rewrites the rest.

use regex::Regex;

/// A baseline with at least one open value or `...` line.
#[derive(Debug)]
pub(crate) struct Template {
    lines: Vec<Line>,
}

#[derive(Debug)]
struct Line {
    depth: usize,
    /// As written, indentation included.
    raw: String,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    /// `...`.
    Dots,
    /// A line to match as it is: what follows `- `.
    Plain(String),
    /// A line with an open value: its pieces in order.
    Open(Vec<Piece>),
}

#[derive(Debug)]
enum Piece {
    /// Text to match as it is.
    Text(String),
    /// A quoted value (name, text or `value=`).
    Value(Name),
    /// Any href, to the end of the line.
    Href,
}

#[derive(Debug)]
enum Name {
    Any,
    Regex(Regex),
}

impl Name {
    /// An open value at the start of `text` (`"*"` or `/regex/`), and
    /// what follows it.
    fn parse(text: &str) -> Result<Option<(Self, &str)>, String> {
        if let Some(rest) = text.strip_prefix("\"*\"")
            && (rest.is_empty() || rest.starts_with(' '))
        {
            return Ok(Some((Self::Any, rest)));
        }
        regex(text)
            .transpose()
            .map(|found| found.map(|(re, rest)| (Self::Regex(re), rest)))
    }

    fn fits(&self, value: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Regex(re) => re.is_match(value),
        }
    }
}

/// What comes before a link's href.
const ARROW: &str = " → ";

/// The pieces of a line's content, or `None` when it opens no value.
fn pieces(content: &str) -> Result<Option<Vec<Piece>>, String> {
    let Some((role, mut rest)) = content.split_once(' ') else {
        return Ok(None);
    };
    let mut pieces = Vec::new();
    let mut text = format!("{role} ");
    // A value here (the name, or after `value=`): open, or quoted as is.
    let value = |rest: &mut &str, text: &mut String, pieces: &mut Vec<Piece>| {
        if let Some((name, after)) = Name::parse(rest)? {
            pieces.push(Piece::Text(std::mem::take(text)));
            pieces.push(Piece::Value(name));
            *rest = after;
        } else if let Some((_, after)) = quoted(rest) {
            text.push_str(&rest[..rest.len() - after.len()]);
            *rest = after;
        }
        Ok::<_, String>(())
    };
    value(&mut rest, &mut text, &mut pieces)?;
    if let Some(i) = rest.find(" value=") {
        let end = i + " value=".len();
        text.push_str(&rest[..end]);
        rest = &rest[end..];
        value(&mut rest, &mut text, &mut pieces)?;
    }
    if let Some(i) = rest.find(ARROW)
        && &rest[i + ARROW.len()..] == "\"*\""
    {
        text.push_str(&rest[..i + ARROW.len()]);
        pieces.push(Piece::Text(std::mem::take(&mut text)));
        pieces.push(Piece::Href);
        rest = "";
    }
    if pieces.is_empty() {
        return Ok(None);
    }
    text.push_str(rest);
    pieces.push(Piece::Text(text));
    Ok(Some(pieces))
}

/// A line of the page: its depth and what follows `- `.
struct Actual<'a> {
    depth: usize,
    content: &'a str,
    raw: &'a str,
}

fn split(raw: &str) -> (usize, &str) {
    let trimmed = raw.trim_start_matches(' ');
    let depth = (raw.len() - trimmed.len()) / 2;
    (depth, trimmed.strip_prefix("- ").unwrap_or(trimmed))
}

/// A quoted value at the start of `text` (`"a \"b\""`), unescaped, and
/// what follows it.
fn quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('"')?.char_indices();
    let mut value = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => value.push(chars.next()?.1),
            '"' => return Some((value, &text[i + 2..])),
            c => value.push(c),
        }
    }
    None
}

/// A `/regex/` or `/regex/i` at the start of `text`, and what follows it;
/// `\/` in it is a `/`.
fn regex(text: &str) -> Option<Result<(Regex, &str), String>> {
    let body = text.strip_prefix('/')?;
    let mut pattern = String::new();
    let mut chars = body.char_indices();
    let end = loop {
        match chars.next()? {
            (_, '\\') => match chars.next()? {
                (_, '/') => pattern.push('/'),
                (_, c) => {
                    pattern.push('\\');
                    pattern.push(c);
                }
            },
            (i, '/') => break i + 1,
            (_, c) => pattern.push(c),
        }
    };
    let mut rest = &body[end..];
    let mut insensitive = false;
    if let Some(after) = rest.strip_prefix('i')
        && (after.is_empty() || after.starts_with(' '))
    {
        insensitive = true;
        rest = after;
    }
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let full = if insensitive {
        format!("(?i){pattern}")
    } else {
        pattern.clone()
    };
    Some(Regex::new(&full).map(|re| (re, rest)).map_err(|e| {
        let why = e.to_string();
        let why = why.lines().last().unwrap_or_default().trim().to_owned();
        format!("/{pattern}/ is not a regular expression: {why}")
    }))
}

impl Template {
    /// The template in `text`, a normalized baseline; `None` if it has no
    /// open value or `...` line (it is compared as text). `Err` names a
    /// line whose regex does not compile.
    pub(crate) fn parse(text: &str) -> Result<Option<Self>, String> {
        let mut lines = Vec::new();
        let mut open = false;
        for (n, raw) in text.lines().enumerate() {
            let (depth, content) = split(raw);
            let kind = if content == "..." {
                Kind::Dots
            } else {
                match pieces(content).map_err(|why| format!("line {}: {why}", n + 1))? {
                    Some(pieces) => Kind::Open(pieces),
                    None => Kind::Plain(content.to_owned()),
                }
            };
            open |= !matches!(kind, Kind::Plain(_));
            lines.push(Line {
                depth,
                raw: raw.to_owned(),
                kind,
            });
        }
        Ok(open.then_some(Self { lines }))
    }

    /// Whether the page's text `actual` matches.
    pub(crate) fn matches(&self, actual: &str) -> bool {
        self.align(actual).0 == 0
    }

    /// `actual` in the template's terms (see the module docs); it matches
    /// the template when `matches` does, and is the page's text otherwise
    /// where the two differ.
    pub(crate) fn project(&self, actual: &str) -> String {
        self.align(actual).1
    }

    /// The fewest template lines left out, page lines added and lines
    /// changed (at the same depth) to align the two, and `actual` in the template's terms along that alignment.
    fn align(&self, actual: &str) -> (usize, String) {
        let page: Vec<Actual<'_>> = actual
            .lines()
            .map(|raw| {
                let (depth, content) = split(raw);
                Actual {
                    depth,
                    content,
                    raw,
                }
            })
            .collect();
        let t = &self.lines;
        let fits = |line: &Line, a: &Actual<'_>| line.depth == a.depth && line.fits(a.content);
        // What is alike at both ends needs no table.
        let mut start = 0;
        while start < t.len()
            && start < page.len()
            && !matches!(t[start].kind, Kind::Dots)
            && fits(&t[start], &page[start])
        {
            start += 1;
        }
        let (mut t_end, mut p_end) = (t.len(), page.len());
        while t_end > start
            && p_end > start
            && !matches!(t[t_end - 1].kind, Kind::Dots)
            && fits(&t[t_end - 1], &page[p_end - 1])
        {
            t_end -= 1;
            p_end -= 1;
        }
        let (tm, pm) = (&t[start..t_end], &page[start..p_end]);
        // cost[i][j]: aligning tm[i..] with pm[j..].
        let width = pm.len() + 1;
        let mut cost = vec![0usize; (tm.len() + 1) * width];
        let at = |i: usize, j: usize| i * width + j;
        for i in (0..=tm.len()).rev() {
            for j in (0..=pm.len()).rev() {
                cost[at(i, j)] = if i == tm.len() {
                    pm.len() - j
                } else if j == pm.len() {
                    usize::from(!matches!(tm[i].kind, Kind::Dots)) + cost[at(i + 1, j)]
                } else if matches!(tm[i].kind, Kind::Dots) {
                    let skip = cost[at(i + 1, j)];
                    if pm[j].depth >= tm[i].depth {
                        skip.min(cost[at(i, j + 1)])
                    } else {
                        skip.min(1 + cost[at(i, j + 1)])
                    }
                } else {
                    let mut best = 1 + cost[at(i + 1, j)].min(cost[at(i, j + 1)]);
                    if fits(&tm[i], &pm[j]) {
                        best = best.min(cost[at(i + 1, j + 1)]);
                    } else if tm[i].depth == pm[j].depth {
                        // A changed line: the page's in its place.
                        best = best.min(1 + cost[at(i + 1, j + 1)]);
                    }
                    best
                };
            }
        }
        let mut out: Vec<&str> = t[..start].iter().map(|l| l.raw.as_str()).collect();
        let (mut i, mut j) = (0, 0);
        while i < tm.len() || j < pm.len() {
            let here = cost[at(i, j)];
            if i == tm.len() {
                out.push(pm[j].raw);
                j += 1;
            } else if matches!(tm[i].kind, Kind::Dots) {
                if j < pm.len() && pm[j].depth >= tm[i].depth && cost[at(i, j + 1)] == here {
                    j += 1;
                } else if cost[at(i + 1, j)] == here {
                    out.push(&tm[i].raw);
                    i += 1;
                } else {
                    out.push(pm[j].raw);
                    j += 1;
                }
            } else if j < pm.len() && fits(&tm[i], &pm[j]) && cost[at(i + 1, j + 1)] == here {
                out.push(&tm[i].raw);
                i += 1;
                j += 1;
            } else if j < pm.len()
                && tm[i].depth == pm[j].depth
                && 1 + cost[at(i + 1, j + 1)] == here
            {
                out.push(pm[j].raw);
                i += 1;
                j += 1;
            } else if j < pm.len() && 1 + cost[at(i, j + 1)] == here {
                out.push(pm[j].raw);
                j += 1;
            } else {
                i += 1;
            }
        }
        out.extend(t[t_end..].iter().map(|l| l.raw.as_str()));
        (cost[at(0, 0)], out.join("\n"))
    }
}

impl Line {
    /// Whether this (not `...`) line matches a page line's content.
    fn fits(&self, content: &str) -> bool {
        match &self.kind {
            Kind::Dots => false,
            Kind::Plain(plain) => plain == content,
            Kind::Open(pieces) => {
                let mut rest = content;
                for piece in pieces {
                    match piece {
                        Piece::Text(text) => match rest.strip_prefix(text.as_str()) {
                            Some(after) => rest = after,
                            None => return false,
                        },
                        Piece::Value(name) => match quoted(rest) {
                            Some((value, after)) if name.fits(&value) => rest = after,
                            _ => return false,
                        },
                        Piece::Href if rest.is_empty() => return false,
                        Piece::Href => rest = "",
                    }
                }
                rest.is_empty()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "\
- main
  - heading \"Order #1234\" (h1)
  - text \"Placed 2026-09-30 10:02\"
  - list
    - listitem
      - text \"Socks\"
    - listitem
      - text \"Shoes\"
  - button \"Pay 49 €\"
  - button \"Cancel\"";

    fn template(text: &str) -> Template {
        Template::parse(text).unwrap().expect("a template")
    }

    #[test]
    fn plain_baselines_are_no_templates() {
        assert!(Template::parse(PAGE).unwrap().is_none());
        // A name that merely starts with a slash is quoted, so plain.
        assert!(Template::parse("- link \"/a/\" → /a").unwrap().is_none());
    }

    #[test]
    fn open_values_and_dots_match() {
        let t = template(
            "\
- main
  - heading /Order #\\d+/ (h1)
  - text \"*\"
  - list
    ...
  - button /pay \\d+/i
  - button \"Cancel\"",
        );
        assert!(t.matches(PAGE));
        assert_eq!(
            t.project(PAGE),
            t.lines
                .iter()
                .map(|l| l.raw.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        // The rest of an open line counts: no (h1), no match.
        let t2 = template("- main\n  - heading \"*\"\n  ...");
        assert!(!t2.matches(PAGE));
        // `...` stands for lines at its depth and below, not above.
        let t3 = template("- main\n  ...\n  - button \"Cancel\"");
        assert!(t3.matches(PAGE));
        let t4 = template("- main\n    ...\n  - button \"Cancel\"");
        assert!(!t4.matches(PAGE));
        // Any number, none too.
        let t5 = template("...\n- main\n  ...\n  - button \"Cancel\"\n  ...");
        assert!(t5.matches(PAGE));
    }

    #[test]
    fn a_missing_line_fails_and_the_projection_shows_the_template() {
        let t = template(
            "\
- main
  - heading /Order #\\d+/ (h1)
  ...
  - button \"Pay\"
  - button \"Cancel\"",
        );
        let page = "- main\n  - heading \"Order #99\" (h1)\n  - text \"x\"\n  - button \"Cancel\"";
        assert!(!t.matches(page));
        // The template's lines where they match; `...` for what it stands
        // for; the missing button is simply not there.
        assert_eq!(
            t.project(page),
            "- main\n  - heading /Order #\\d+/ (h1)\n  ...\n  - button \"Cancel\""
        );
        // A changed value no longer fits the regex: the page's line.
        let page =
            "- main\n  - heading \"Invoice\" (h1)\n  - button \"Pay\"\n  - button \"Cancel\"";
        assert_eq!(
            t.project(page),
            "- main\n  - heading \"Invoice\" (h1)\n  ...\n  - button \"Pay\"\n  - button \"Cancel\""
        );
    }

    #[test]
    fn values_and_hrefs_open_too() {
        let page = "\
- form
  - textbox \"Since\" value=\"2026-09-30 10:02\" focused
  - textbox \"Code\" value=\"A-1\"
  - link \"Invoice\" → /invoices/9?session=f00";
        let t = template(
            "\
- form
  - textbox \"Since\" value=\"*\" focused
  - textbox /code/i value=/^A-\\d+$/
  - link \"Invoice\" → \"*\"",
        );
        assert!(t.matches(page));
        // The rest of the line still counts: no `focused`, no match.
        assert!(!template("- form\n  - textbox \"Since\" value=\"*\"\n  ...").matches(page));
        // A value the regex does not find, a missing href.
        assert!(
            !template("- textbox \"Code\" value=/B/").matches("- textbox \"Code\" value=\"A-1\"")
        );
        assert!(!template("- link \"x\" → \"*\"").matches("- link \"x\""));
        // A name holding ` value=` or ` → ` is the name's.
        let t = template("- button \"a value=b → c\" value=\"*\"");
        assert!(t.matches("- button \"a value=b → c\" value=\"z\""));
        // An href is not a regex: `→ /a/` is plain.
        assert!(Template::parse("- link \"a\" → /a/").unwrap().is_none());
        let err = Template::parse("- textbox \"a\" value=/(/").unwrap_err();
        assert!(err.starts_with("line 1: /(/"), "{err}");
    }

    #[test]
    fn bad_regexes_name_their_line() {
        let err = Template::parse("- main\n  - heading /(/").unwrap_err();
        assert!(
            err.starts_with("line 2: /(/ is not a regular expression"),
            "{err}"
        );
    }

    #[test]
    fn quoted_values_unescape() {
        assert_eq!(
            quoted(r#""a \"b\" \\ c" rest"#),
            Some((r#"a "b" \ c"#.to_owned(), " rest"))
        );
        let t = template(r#"- text /"b"/"#);
        assert!(t.matches(r#"- text "a \"b\"""#));
        let t = template(r"- link /a\/b/ → /x");
        assert!(t.matches("- link \"a/b\" → /x"));
    }
}

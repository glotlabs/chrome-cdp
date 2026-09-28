//! `chrome-cdp fmt [paths…] [--check]` (plan 11, milestone 11.12):
//! formats `.cdp` files ([`e2e::scenario::format`]) and mock files
//! ([`mock::format`]) in place.
//!
//! [`Clap`] tells the `.cdp` formatter what the CLI knows: the long form
//! of each short flag, and what a line means (the parsed command), so a
//! line whose words change must still parse to the same command.

use std::fmt::Write as _;
use std::path::Path;

use clap::{CommandFactory as _, FromArgMatches as _};
use e2e::scenario::format::Commands;
use protocol::schema::FmtData;
use protocol::script::{Line, Resolve, Unresolved, Var};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use crate::cli::{Cli, FmtArgs};
use crate::e2e_files::{self, shown};

/// `fmt`.
pub(crate) fn run(args: &FmtArgs) -> Result<Output, ErrorInfo> {
    let files = e2e_files::find(&args.paths)?;
    let clap = Clap::new();
    let mut changed = Vec::new();
    let mut diff = String::new();
    let mut problems: Vec<String> = Vec::new();
    let all = files
        .cdp
        .iter()
        .map(|p| (p, true))
        .chain(files.mocks.iter().map(|p| (p, false)));
    for (path, cdp) in all {
        let name = shown(path);
        let src = match std::fs::read_to_string(path) {
            Ok(src) => src,
            Err(e) => {
                problems.push(format!("{name}: cannot read it: {e}"));
                continue;
            }
        };
        let formatted = if cdp {
            format_cdp(&name, &src, &clap)
        } else {
            mock::format::format(&src).map_err(|e| format!("{name}: {e}"))
        };
        let text = match formatted {
            Ok(text) => text,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };
        if text == src {
            continue;
        }
        if args.check {
            diff.push_str(&unified(&src, &text, &name));
        } else if let Err(e) = std::fs::write(path, &text) {
            problems.push(format!("{name}: cannot write it: {e}"));
            continue;
        }
        changed.push(path.display().to_string());
    }
    if let Some(first) = problems.first() {
        return Err(cannot_format(first, &problems));
    }
    let data = FmtData {
        files: files.len(),
        changed: changed.clone(),
        check: args.check,
        diff: (!diff.is_empty()).then(|| diff.clone()),
    };
    let names: Vec<String> = changed.iter().map(|c| shown(Path::new(c))).collect();
    if args.check && !changed.is_empty() {
        let mut command = Cmd::new("fmt");
        for path in &args.paths {
            command = command.arg(path.display().to_string());
        }
        let count = plural(changed.len(), "file is", "files are");
        let mut info = ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!("{count} not formatted: {}", names.join(", ")),
        );
        info = info.block("diff", bounded(&diff, args.max_chars));
        return Err(info
            .hint(Hint::run(command).why("formats them"))
            .details(serde_json::to_value(&data).unwrap_or_default()));
    }
    let mut text = String::new();
    for name in &names {
        let _ = writeln!(text, "formatted {name}");
    }
    let _ = write!(
        text,
        "{}, {}",
        plural(files.len(), "file", "files"),
        if args.check {
            "all formatted".to_owned()
        } else {
            format!("{} formatted", changed.len())
        }
    );
    Ok(Output::typed(&data, text)
        .quiet(names.join("\n"))
        .limit(args.max_chars, "--max-chars 0 for all of it, or --json"))
}

/// A `.cdp` file formatted, or its first problem as `file:line:col: …`.
fn format_cdp(name: &str, src: &str, clap: &Clap) -> Result<String, String> {
    e2e::scenario::format::format(name, src, clap).map_err(|errors| {
        let first = errors.first().map_or_else(String::new, ToString::to_string);
        match errors.len() {
            0 | 1 => first,
            n => format!("{first} (and {} more)", n - 1),
        }
    })
}

/// The error for files that could not be formatted (syntax errors).
fn cannot_format(first: &str, problems: &[String]) -> ErrorInfo {
    let mut info = ErrorInfo::new(ErrorKind::BadArgs, format!("cannot format {first}"));
    for p in problems.iter().skip(1).take(10) {
        info = info.note("also", p.clone());
    }
    info.hint(Hint::run(Cmd::new("lint")).why("lists the problems of every file"))
        .details(serde_json::json!({ "problems": problems }))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A unified diff from `old` to `new` of file `name`.
fn unified(old: &str, new: &str, name: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// `text` cut to about `max` characters (0: all of it).
fn bounded(text: &str, max: usize) -> String {
    if max == 0 || text.chars().count() <= max {
        return text.trim_end().to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = cut.rsplit_once('\n').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}\n… diff truncated; use --max-chars 0 or --json for all of it")
}

/// What the formatter asks the CLI.
pub(crate) struct Clap {
    root: clap::Command,
}

/// Fills every variable with a stand-in: formatting reads no values.
struct StandIn;

impl Resolve for StandIn {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        Ok(format!("x{}", var.to_string().len()))
    }
}

impl Clap {
    pub(crate) fn new() -> Self {
        let mut root = Cli::command();
        root.build();
        Self { root }
    }

    /// The line's words with stand-ins for variables.
    fn words(line: &Line) -> Option<Vec<String>> {
        line.words.iter().map(|w| w.expand(&StandIn).ok()).collect()
    }

    /// `words` parsed, as a string to compare.
    fn parsed(&self, words: &[String]) -> Option<String> {
        let matches = self
            .root
            .clone()
            .try_get_matches_from(
                std::iter::once(protocol::PROGRAM.to_owned()).chain(words.to_vec()),
            )
            .ok()?;
        let cli = Cli::from_arg_matches(&matches).ok()?;
        Some(format!("{cli:?}"))
    }

    /// The subcommand `words` name.
    fn leaf(&self, words: &[String]) -> Option<clap::Command> {
        let matches = self
            .root
            .clone()
            .try_get_matches_from(
                std::iter::once(protocol::PROGRAM.to_owned()).chain(words.to_vec()),
            )
            .ok()?;
        let mut cmd = &self.root;
        let mut m = &matches;
        while let Some((name, sub)) = m.subcommand() {
            cmd = cmd.find_subcommand(name)?;
            m = sub;
        }
        Some(cmd.clone())
    }
}

impl Commands for Clap {
    fn long_flags(&self, line: &Line) -> Vec<(usize, Vec<String>)> {
        let Some(words) = Self::words(line) else {
            return Vec::new();
        };
        let Some(meaning) = self.parsed(&words) else {
            return Vec::new();
        };
        let Some(cmd) = self.leaf(&words) else {
            return Vec::new();
        };
        let mut replaced = Vec::new();
        for (i, word) in line.words.iter().enumerate() {
            if word.raw == "--" {
                break;
            }
            if word.quoted || !word.raw.starts_with('-') || word.raw.starts_with("--") {
                continue;
            }
            let shorts: Vec<char> = word.raw.chars().skip(1).collect();
            let Some(&first) = shorts.first() else {
                continue;
            };
            let find = |c: char| cmd.get_arguments().find(|a| a.get_short() == Some(c));
            let Some(arg) = find(first) else {
                continue;
            };
            let Some(long) = arg.get_long() else {
                continue;
            };
            if arg.get_action().takes_values() {
                let rest: String = shorts[1..].iter().collect();
                let rest = rest.strip_prefix('=').unwrap_or(&rest);
                replaced.push((
                    i,
                    vec![if rest.is_empty() {
                        format!("--{long}")
                    } else {
                        format!("--{long}={rest}")
                    }],
                ));
                continue;
            }
            // `-qv`: flags without values only.
            let longs: Option<Vec<String>> = shorts
                .iter()
                .map(|&c| {
                    let arg = find(c)?;
                    (!arg.get_action().takes_values())
                        .then(|| arg.get_long().map(|l| format!("--{l}")))
                        .flatten()
                })
                .collect();
            if let Some(longs) = longs {
                replaced.push((i, longs));
            }
        }
        if replaced.is_empty() {
            return replaced;
        }
        // The words of the new line must parse to the same command.
        let mut new_words = Vec::new();
        for (i, word) in words.iter().enumerate() {
            match replaced.iter().find(|(at, _)| *at == i) {
                Some((_, new)) => new_words.extend(new.iter().cloned()),
                None => new_words.push(word.clone()),
            }
        }
        if self.parsed(&new_words).as_deref() == Some(meaning.as_str()) {
            replaced
        } else {
            Vec::new()
        }
    }

    fn meaning(&self, line: &Line) -> Option<String> {
        self.parsed(&Self::words(line)?)
    }
}

#[cfg(test)]
mod tests {
    use super::Clap;

    fn fmt(src: &str) -> String {
        e2e::scenario::format::format("t.cdp", src, &Clap::new()).unwrap()
    }

    #[test]
    fn writes_short_flags_long() {
        assert_eq!(fmt("screenshot -o a.png\n"), "screenshot --output a.png\n");
        assert_eq!(fmt("text h1 -qv\n"), "text h1 --quiet --verbose\n");
        assert_eq!(fmt("screenshot -oa.png\n"), "screenshot --output=a.png\n");
        // Values and words after -- are not flags.
        assert_eq!(fmt("fill e1 -- -o\n"), "fill e1 -- -o\n");
        assert_eq!(fmt("fill e1 '-o'\n"), "fill e1 '-o'\n");
    }

    #[test]
    fn leaves_lines_that_do_not_parse() {
        assert_eq!(fmt("clik -o x\n"), "clik -o x\n");
    }
}

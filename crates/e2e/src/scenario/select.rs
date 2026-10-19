//! Which tests a run has (plan 11, milestone 11.4): the files named on the
//! command line (a file, a folder, or `file.cdp:12` for the test at that
//! line) or found by the config's `tests` globs, then `--grep`, `--tags`,
//! `@only`, `@skip` and `--shard`. The order is always the same: files by
//! path, tests as written. With the tests' durations known, `--shard`
//! splits by time instead of by count ([`pack`], plan 22.5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use protocol::{ErrorInfo, ErrorKind, Hint};
use regex::Regex;

use super::{Scenario, Settings};

/// Folders never searched for scenarios.
const SKIPPED_DIRS: &[&str] = &["node_modules", "target", "dist", "build"];

/// A path argument of `chrome-cdp test`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Absolute.
    pub path: PathBuf,
    /// `file.cdp:12`: the test at line 12.
    pub line: Option<u32>,
}

/// Parses a path argument relative to `cwd`: `a.cdp`, `dir/`, `a.cdp:12`.
pub fn parse_target(arg: &str, cwd: &Path) -> Target {
    if let Some((file, line)) = arg.rsplit_once(':')
        && let Ok(line) = line.parse::<u32>()
        && !file.is_empty()
        && !cwd.join(arg).exists()
    {
        return Target {
            path: cwd.join(file),
            line: Some(line),
        };
    }
    Target {
        path: cwd.join(arg),
        line: None,
    }
}

/// The scenario files of `targets` (folders searched for `*.cdp`), each
/// once, sorted, with the lines asked for in each (empty: every test).
pub fn files_of(targets: &[Target]) -> Result<BTreeMap<PathBuf, Vec<u32>>, ErrorInfo> {
    let mut files: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    // A file named without a line runs whole, even if also named with one.
    let mut whole: Vec<PathBuf> = Vec::new();
    for target in targets {
        if target.path.is_dir() {
            if target.line.is_some() {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("{} is a folder; :LINE needs a file", target.path.display()),
                ));
            }
            let mut found = Vec::new();
            walk(&target.path, &mut found);
            for file in found.into_iter().filter(|f| has_ext(f)) {
                whole.push(file.clone());
                files.entry(file).or_default();
            }
        } else if target.path.is_file() {
            let entry = files.entry(target.path.clone()).or_default();
            match target.line {
                Some(line) => entry.push(line),
                None => whole.push(target.path.clone()),
            }
        } else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("no such file or folder: {}", target.path.display()),
            )
            .hint(Hint::text(
                "name .cdp files or folders, relative to the current directory",
            )));
        }
    }
    for file in whole {
        if let Some(lines) = files.get_mut(&file) {
            lines.clear();
        }
    }
    Ok(files)
}

/// The files below `root` whose path relative to it matches one of
/// `patterns` (`e2e/scenarios/**/*.cdp`), sorted.
pub fn files_matching(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, &mut found);
    found.retain(|file| {
        let relative = file.strip_prefix(root).unwrap_or(file);
        let relative = relative.to_string_lossy().replace('\\', "/");
        patterns.iter().any(|p| glob_matches(p, &relative))
    });
    found.sort();
    found
}

/// The files below `root` that match `patterns`, walking from each
/// glob's literal folder and, unlike [`files_matching`], into build output
/// (`dist/**`) too; hidden folders and `node_modules` are left out unless
/// the glob names them.
pub fn files_under_globs(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for pattern in patterns {
        let base: PathBuf = Path::new(pattern)
            .components()
            .take_while(|c| {
                !c.as_os_str()
                    .to_string_lossy()
                    .contains(['*', '?', '[', '{'])
            })
            .collect();
        let start = root.join(&base);
        if start.is_file() {
            found.push(start);
            continue;
        }
        let mut files = Vec::new();
        walk_all(&start, &mut files);
        files.retain(|file| {
            let relative = file.strip_prefix(root).unwrap_or(file);
            glob_matches(pattern, &relative.to_string_lossy().replace('\\', "/"))
        });
        found.extend(files);
    }
    found.sort();
    found.dedup();
    found
}

fn walk_all(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') && name != "node_modules" {
                walk_all(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}

fn has_ext(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "cdp")
}

/// Every file below `dir`, leaving out hidden folders and build output.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref()) {
                walk(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}

/// Whether `path` (`/`-separated) matches `pattern`: `**` is any number of
/// folders, `*` any text within one name, `?` one character.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern
        .trim_start_matches("./")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let path: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(name, more)| name_matches(first, name) && segments(rest, more)),
    }
}

fn name_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    chars_match(&pattern, &name)
}

fn chars_match(p: &[char], n: &[char]) -> bool {
    match p.split_first() {
        None => n.is_empty(),
        Some(('*', rest)) => (0..=n.len()).any(|skip| chars_match(rest, &n[skip..])),
        Some(('?', rest)) => !n.is_empty() && chars_match(rest, &n[1..]),
        Some((c, rest)) => n.first() == Some(c) && chars_match(rest, &n[1..]),
    }
}

/// `--grep` and `--tags`.
#[derive(Debug, Default)]
pub struct Filter {
    /// Matched against `file > test name`.
    pub grep: Option<Regex>,
    /// A test needs one of these, if any are given.
    pub tags: Vec<String>,
    /// A test with one of these is left out (`--tags !slow`).
    pub not_tags: Vec<String>,
}

impl Filter {
    /// `--grep`: a regular expression.
    pub fn set_grep(&mut self, pattern: &str) -> Result<(), ErrorInfo> {
        let re = Regex::new(pattern).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("--grep is not a regular expression: {e}"),
            )
        })?;
        self.grep = Some(re);
        Ok(())
    }

    /// `--tags smoke,!slow`.
    pub fn set_tags(&mut self, list: &str) {
        for tag in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            match tag.strip_prefix('!') {
                Some(tag) => self.not_tags.push(tag.to_owned()),
                None => self.tags.push(tag.to_owned()),
            }
        }
    }

    fn keeps(&self, title: &str, settings: &Settings) -> bool {
        self.grep.as_ref().is_none_or(|re| re.is_match(title))
            && (self.tags.is_empty() || self.tags.iter().any(|t| settings.tags.contains(t)))
            && !self.not_tags.iter().any(|t| settings.tags.contains(t))
    }
}

/// A test of the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    /// Index into the scenarios.
    pub file: usize,
    /// Index into the file's tests.
    pub test: usize,
    /// The file's settings with the test's on top.
    pub settings: Settings,
    /// `file > name`.
    pub title: String,
}

impl Selected {
    /// `@skip`'s reason, when it is skipped.
    pub fn skipped(&self) -> Option<&str> {
        self.settings.skip.as_deref()
    }
}

/// The tests of `scenarios` a run has, in order: those at `lines` (by
/// file index; none means all), that `filter` keeps, and only the `@only`
/// ones if there are any. An asked-for line with no test is an error.
pub fn select(
    scenarios: &[Scenario],
    lines: &[Vec<u32>],
    filter: &Filter,
) -> Result<Vec<Selected>, ErrorInfo> {
    let mut selected = Vec::new();
    for (f, scenario) in scenarios.iter().enumerate() {
        let wanted = lines.get(f).map_or(&[][..], Vec::as_slice);
        for &line in wanted {
            let hit = scenario
                .tests
                .iter()
                .any(|t| (t.span.line..=t.end_line).contains(&line));
            if !hit {
                let tests: Vec<String> = scenario
                    .tests
                    .iter()
                    .map(|t| format!("{}:{} {}", scenario.name, t.span.line, t.name))
                    .collect();
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("{}:{line} is not in a test", scenario.name),
                )
                .note("tests", tests.join("; "))
                .hint(Hint::text(
                    "name a line of a @test block, e.g. its @test line",
                )));
            }
        }
        for (t, test) in scenario.tests.iter().enumerate() {
            let in_lines = wanted.is_empty()
                || wanted
                    .iter()
                    .any(|l| (test.span.line..=test.end_line).contains(l));
            if !in_lines {
                continue;
            }
            let settings = scenario.settings_of(t);
            let title = format!("{} > {}", scenario.name, test.name);
            if filter.keeps(&title, &settings) {
                selected.push(Selected {
                    file: f,
                    test: t,
                    settings,
                    title,
                });
            }
        }
    }
    if selected.iter().any(|s| s.settings.only) {
        selected.retain(|s| s.settings.only);
    }
    Ok(selected)
}

/// `--shard 2/4`: the second of four near-equal parts of the tests.
pub fn parse_shard(text: &str) -> Result<(usize, usize), ErrorInfo> {
    let parsed = text
        .split_once('/')
        .and_then(|(i, n)| Some((i.trim().parse().ok()?, n.trim().parse().ok()?)))
        .filter(|&(i, n): &(usize, usize)| n > 0 && (1..=n).contains(&i));
    parsed.ok_or_else(|| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--shard takes INDEX/TOTAL, e.g. 2/4, got `{text}`"),
        )
        .hint(Hint::text("INDEX counts from 1 and is at most TOTAL"))
    })
}

/// Shard `index` (from 1) of `total` of `items`: consecutive runs whose
/// sizes differ by one at most, which together hold every item once.
pub fn shard<T>(items: Vec<T>, index: usize, total: usize) -> Vec<T> {
    let len = items.len();
    let start = len * (index - 1) / total;
    let end = len * index / total;
    items.into_iter().skip(start).take(end - start).collect()
}

/// Which of `total` shards (from 0) each item goes to, so that their
/// total `weights` are as equal as greedy packing gives: the heaviest item
/// first (the earlier of equal ones), each into the shard with the least
/// so far (the first of equal ones). The same weights always give the same
/// shards, so every CI machine computes the same split.
pub fn pack(weights: &[u64], total: usize) -> Vec<usize> {
    let total = total.max(1);
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(weights[i]));
    let mut loads = vec![0u64; total];
    let mut shards = vec![0; weights.len()];
    for i in order {
        let (lightest, _) = loads
            .iter()
            .enumerate()
            .min_by_key(|&(s, &load)| (load, s))
            .unwrap_or((0, &0));
        shards[i] = lightest;
        loads[lightest] = loads[lightest].saturating_add(weights[i]);
    }
    shards
}

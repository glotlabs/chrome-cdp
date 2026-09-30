//! Fingerprints (plan 18, milestone 18.2): what the element each step's
//! locator found looks like, kept in `e2e/fingerprints/<file>.json` so a
//! locator that no longer finds anything can be healed ([`super::heal`]).
//!
//! Steps whose locator must find one element have one: actions, `wait
//! <loc>` for an element that is there, and `expect <loc>` with a matcher
//! that reads the element. The runner sends the step's locator with the
//! step's call; the daemon reads the element before an action acts
//! (`engine::fingerprint`). A file holds one object per test name, then
//! one per step key, `<line>:<locator as written>` ([`key`]); keys are
//! sorted and the JSON is pretty-printed, so diffs stay small, and a file
//! is rewritten only when something in it changed. Each passed test's
//! fingerprints replace the ones it had, so those of steps that are gone
//! go too; a step that gave none this time keeps its old one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{CommandFactory as _, FromArgMatches as _};
use e2e::scenario::Scenario;
use protocol::schema::Fingerprint;
use protocol::script::{Part, Var};
use serde::{Deserialize, Serialize};

use super::plan::{Plan, Prepared};
use crate::cli::{Cli, Command, ElementStateArg};

/// Where the files go, relative to the project root.
pub(super) const DIR: &str = "e2e/fingerprints";

/// The format's version.
const VERSION: u32 = 1;

/// Matchers of `expect <loc>` that read the one element the locator
/// finds; `count`, `hidden` and `detached` do not.
const ELEMENT_MATCHERS: &[&str] = &[
    "visible",
    "attached",
    "enabled",
    "disabled",
    "editable",
    "checked",
    "unchecked",
    "focused",
    "text",
    "value",
    "values",
    "attr",
    "class",
    "css",
    "in-viewport",
];

/// A file as written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct File {
    pub(super) version: u32,
    /// By test name, then by step key.
    pub(super) tests: BTreeMap<String, BTreeMap<String, Fingerprint>>,
}

impl File {
    /// The text written: pretty-printed JSON and a newline.
    fn text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// The locator of a step whose element gets a fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    /// Its word in the step's line.
    pub(super) word: usize,
    /// Its value, variables filled in.
    pub(super) locator: String,
}

/// The locator of step `words` (variables filled in) whose element a
/// fingerprint describes, if the step needs one element: an action, a
/// `wait` for an element that is there, or an `expect` that reads one.
/// A ref (`e12`) has none: scenarios should not hold refs.
pub(super) fn target(words: &[String]) -> Option<Target> {
    let args = std::iter::once(protocol::PROGRAM.to_owned()).chain(words.iter().cloned());
    let matches = Cli::command().try_get_matches_from(args).ok()?;
    let cli = Cli::from_arg_matches(&matches).ok()?;
    let locator = match cli.command.as_ref()? {
        Command::Click(_)
        | Command::Dblclick(_)
        | Command::Hover(_)
        | Command::Tap(_)
        | Command::Drag(_)
        | Command::Fill(_)
        | Command::Select(_)
        | Command::Check(_)
        | Command::Uncheck(_)
        | Command::Focus(_)
        | Command::Blur(_)
        | Command::Upload(_)
        | Command::Scroll(_)
        | Command::Press(_)
        | Command::Type(_) => crate::script::locator_values(&matches).into_iter().next()?,
        Command::Expect(args) => {
            let reads = args
                .matcher
                .as_deref()
                .is_none_or(|m| ELEMENT_MATCHERS.contains(&m));
            if args.not || !reads {
                return None;
            }
            crate::script::locator_values(&matches).into_iter().next()?
        }
        Command::Wait(args) => {
            let there = matches!(
                args.state,
                None | Some(
                    ElementStateArg::Visible | ElementStateArg::Attached | ElementStateArg::Enabled
                )
            );
            let target = args.target.clone()?;
            let ms = target.bytes().all(|b| b.is_ascii_digit())
                || protocol::format::parse_duration(&target).is_ok();
            if !there || ms {
                return None;
            }
            target
        }
        _ => return None,
    };
    let parsed = engine::parse_locator(&locator).ok()?;
    if parsed.has_ref() {
        return None;
    }
    let word = words.iter().skip(1).position(|w| *w == locator)? + 1;
    Some(Target { word, locator })
}

/// The key of `step`'s locator word `word` in its scenario's file:
/// `<line>:<locator as written>`, with the step file first (relative to
/// the project `root`) for a step of another file, and the `@include`
/// parameters the locator uses last: `e2e/steps/sign-in.cdp:4:label=Email`,
/// `e2e/steps/pick.cdp:2:${name} [name=Keyboard]`.
pub(super) fn key(step: &Prepared, word: usize, scenario: &Scenario, root: &Path) -> String {
    let Some(w) = step.line.words.get(word) else {
        return String::new();
    };
    let text = w.literal().unwrap_or_else(|| w.raw.clone());
    let file = &*step.line.span.file;
    let mut key = if file == scenario.name {
        String::new()
    } else {
        format!("{}:", relative(root, file))
    };
    key.push_str(&format!("{}:{text}", step.line.span.line));
    let params: Vec<String> = w
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::Var(v) => match &v.var {
                Var::Name(name) => step
                    .vars
                    .params
                    .get(name)
                    .map(|value| format!("{name}={value}")),
                _ => None,
            },
            Part::Text(_) => None,
        })
        .collect();
    if !params.is_empty() {
        key.push_str(&format!(" [{}]", params.join(", ")));
    }
    key
}

/// `file` (as spans name it: relative to the current directory, or
/// absolute) relative to `root`, `..` resolved, with `/` separators.
pub(super) fn relative(root: &Path, file: &str) -> String {
    let path = normalize(&std::env::current_dir().unwrap_or_default().join(file));
    let path = path.strip_prefix(normalize(root)).unwrap_or(&path);
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` with `.` and `..` resolved without touching the disk.
pub(super) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The fingerprint file of scenario `scenario` of `plan`.
pub(super) fn path(plan: &Plan, scenario: &Scenario) -> PathBuf {
    plan.config
        .root
        .join(DIR)
        .join(super::plan::file_rel(&plan.config, scenario).with_extension("json"))
}

/// The fingerprints of a run's scenario files, as they were when it
/// started.
#[derive(Debug, Default)]
pub(super) struct Store {
    /// By scenario (the plan's index).
    files: Vec<File>,
}

impl Store {
    /// Reads the files of `plan`'s scenarios; a missing or unreadable one
    /// is empty (a bad one is also a warning).
    pub(super) fn load(plan: &Plan, warnings: &mut Vec<String>) -> Self {
        let files = plan
            .scenarios
            .iter()
            .map(|scenario| {
                let path = path(plan, scenario);
                match std::fs::read_to_string(&path) {
                    Err(_) => File::default(),
                    Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                        warnings.push(format!(
                            "{} is not a fingerprint file ({e}); it is read as empty",
                            path.display()
                        ));
                        File::default()
                    }),
                }
            })
            .collect();
        Self { files }
    }

    /// The fingerprint of step `key` of test `test` in file `file`, else
    /// the one another test of the file has for it (a hook's step, or a
    /// renamed test).
    pub(super) fn get(&self, file: usize, test: &str, key: &str) -> Option<&Fingerprint> {
        let tests = &self.files.get(file)?.tests;
        tests
            .get(test)
            .and_then(|steps| steps.get(key))
            .or_else(|| tests.values().find_map(|steps| steps.get(key)))
    }
}

/// `fp` without what holds a secret's value: a field that holds one is
/// left out, and so is a candidate locator.
pub(super) fn scrub(fp: &mut Fingerprint, secrets: &[String]) {
    let holds = |text: &str| {
        secrets
            .iter()
            .any(|s| !s.is_empty() && text.contains(s.as_str()))
    };
    for field in [
        &mut fp.name,
        &mut fp.text,
        &mut fp.label,
        &mut fp.placeholder,
        &mut fp.testid,
        &mut fp.id,
        &mut fp.page,
    ] {
        if holds(field) {
            field.clear();
        }
    }
    if let Some(container) = &mut fp.container
        && holds(&container.name)
    {
        container.name.clear();
    }
    fp.candidates.retain(|c| !holds(c));
    fp.classes.retain(|c| !holds(c));
}

/// What a passed test's steps gave: each fingerprinted step's key and
/// its fingerprint (`None`: the step gave none this time).
pub(super) type Taken = Vec<(String, Option<Fingerprint>)>;

/// Merges the fingerprints of passed tests (by file, then test name) into
/// `file`: each test's object becomes its steps', the old fingerprint kept
/// for a step that gave none; tests the scenario no longer has are
/// dropped. `renamed` maps old step keys to new ones (`--write` changed
/// the locators).
pub(super) fn merge(
    mut file: File,
    passed: &BTreeMap<String, Taken>,
    names: &[String],
    renamed: &BTreeMap<String, String>,
) -> File {
    file.version = VERSION;
    for (test, taken) in passed {
        let old = file.tests.remove(test).unwrap_or_default();
        let mut steps = BTreeMap::new();
        for (key, fp) in taken {
            let fp = fp.clone().or_else(|| old.get(key).cloned());
            let key = renamed.get(key).cloned().unwrap_or_else(|| key.clone());
            if let Some(fp) = fp {
                steps.insert(key, fp);
            }
        }
        if !steps.is_empty() {
            file.tests.insert(test.clone(), steps);
        }
    }
    file.tests.retain(|test, _| names.contains(test));
    file
}

/// Writes the merged fingerprints of `passed` (by the plan's scenario
/// index, then test name) into their files; returns the files it changed
/// and warnings. A file is rewritten only when its text changed.
pub(super) fn write(
    plan: &Plan,
    passed: &BTreeMap<usize, BTreeMap<String, Taken>>,
    renamed: &BTreeMap<String, String>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut changed = Vec::new();
    let mut warnings = Vec::new();
    for (&index, tests) in passed {
        let scenario = &plan.scenarios[index];
        let path = path(plan, scenario);
        let before = std::fs::read_to_string(&path).ok();
        let file = before
            .as_deref()
            .and_then(|t| serde_json::from_str::<File>(t).ok())
            .unwrap_or_default();
        let names: Vec<String> = scenario.tests.iter().map(|t| t.name.clone()).collect();
        let merged = merge(file, tests, &names, renamed);
        let text = merged.text();
        if before.as_deref() == Some(text.as_str()) || (before.is_none() && merged.tests.is_empty())
        {
            continue;
        }
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, &text));
        match written {
            Ok(()) => changed.push(path),
            Err(e) => warnings.push(format!("cannot write {}: {e}", path.display())),
        }
    }
    (changed, warnings)
}

#[cfg(test)]
mod tests {
    use protocol::schema::{Fingerprint, FingerprintContainer};

    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn steps_that_need_one_element_have_a_target() {
        let t = target(&words("click text=Buy")).unwrap();
        assert_eq!((t.word, t.locator.as_str()), (1, "text=Buy"));
        assert_eq!(target(&words("fill label=Email a@b.c")).unwrap().word, 1);
        assert_eq!(
            target(&words("drag #a #b")).unwrap().locator,
            "#a",
            "drag's FROM"
        );
        assert!(target(&words("expect #a visible")).is_some());
        assert!(target(&words("expect #a text Hi")).is_some());
        assert!(target(&words("expect #a")).is_some());
        assert!(target(&words("expect #a count 2")).is_none());
        assert!(target(&words("expect #a hidden")).is_none());
        assert!(target(&words("expect #a visible --not")).is_none());
        assert!(target(&words("expect --url **/x")).is_none());
        assert!(target(&words("expect request GET /api")).is_none());
        assert!(target(&words("wait #a")).is_some());
        assert!(target(&words("wait #a --state enabled")).is_some());
        assert!(target(&words("wait #a --state hidden")).is_none());
        assert!(target(&words("wait 500")).is_none());
        assert!(target(&words("click e12")).is_none(), "a ref");
        assert!(target(&words("goto /")).is_none());
        assert!(target(&words("text #a")).is_none());
        assert_eq!(target(&words("press Enter --in #q")).unwrap().locator, "#q");
        assert!(target(&words("press Enter")).is_none());
    }

    fn fp(name: &str) -> Fingerprint {
        Fingerprint {
            role: "button".into(),
            name: name.into(),
            ..Fingerprint::default()
        }
    }

    #[test]
    fn a_passed_test_replaces_its_fingerprints() {
        let mut file = File::default();
        file.tests.insert(
            "t".into(),
            BTreeMap::from([
                ("3:#a".into(), fp("A")),
                ("4:#gone".into(), fp("Gone")),
                ("5:#kept".into(), fp("Kept")),
            ]),
        );
        file.tests
            .insert("removed".into(), BTreeMap::from([("1:#x".into(), fp("X"))]));
        file.tests
            .insert("other".into(), BTreeMap::from([("1:#o".into(), fp("O"))]));
        let passed = BTreeMap::from([(
            "t".to_owned(),
            vec![
                ("3:#a".to_owned(), Some(fp("A2"))),
                ("5:#kept".to_owned(), None),
                ("6:#new".to_owned(), Some(fp("New"))),
            ],
        )]);
        let names = ["t".to_owned(), "other".to_owned()];
        let merged = merge(file, &passed, &names, &BTreeMap::new());
        assert_eq!(merged.version, 1);
        let t = &merged.tests["t"];
        assert_eq!(
            t.keys().collect::<Vec<_>>(),
            ["3:#a", "5:#kept", "6:#new"],
            "a step that is gone goes"
        );
        assert_eq!(t["3:#a"].name, "A2");
        assert_eq!(t["5:#kept"].name, "Kept", "no new one: the old stays");
        assert!(merged.tests.contains_key("other"));
        assert!(!merged.tests.contains_key("removed"));
        // Renamed keys (`--write`).
        let renamed = BTreeMap::from([("6:#new".to_owned(), "6:#newer".to_owned())]);
        let again = merge(merged, &passed, &names, &renamed);
        assert!(again.tests["t"].contains_key("6:#newer"));
    }

    #[test]
    fn the_text_is_sorted_and_stable() {
        let mut file = File {
            version: 1,
            ..File::default()
        };
        file.tests.insert(
            "b".into(),
            BTreeMap::from([("9:#z".into(), fp("Z")), ("10:#y".into(), fp("Y"))]),
        );
        file.tests.insert("a".into(), BTreeMap::new());
        let text = file.text();
        assert!(text.find("\"a\"").unwrap() < text.find("\"b\"").unwrap());
        assert!(text.ends_with("}\n"));
        let back: File = serde_json::from_str(&text).unwrap();
        assert_eq!(back.text(), text);
    }

    #[test]
    fn secrets_are_scrubbed() {
        let mut f = Fingerprint {
            name: "Hello hunter2".into(),
            text: "fine".into(),
            container: Some(FingerprintContainer {
                role: "form".into(),
                name: "hunter2".into(),
            }),
            candidates: vec!["text=hunter2".into(), "#ok".into()],
            ..Fingerprint::default()
        };
        scrub(&mut f, &["hunter2".into(), String::new()]);
        assert_eq!(f.name, "");
        assert_eq!(f.text, "fine");
        assert_eq!(f.container.unwrap().name, "");
        assert_eq!(f.candidates, ["#ok"]);
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(
            normalize(Path::new("/p/e2e/scenarios/../steps/./a.cdp")),
            Path::new("/p/e2e/steps/a.cdp")
        );
        assert_eq!(relative(Path::new("/p"), "/p/e2e/x/../a.cdp"), "e2e/a.cdp");
    }
}

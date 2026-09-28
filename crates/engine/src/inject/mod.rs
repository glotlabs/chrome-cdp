//! Init scripts and exposed functions (plan 07, milestone 7.3):
//! `init-script add|list|remove`, `expose` and `unexpose`.
//!
//! Like routes and emulation, both belong to the session ([`Injector`], in
//! the tab manager's settings, so they outlive a browser): every page and
//! iframe session gets them while it is paused, before its first script,
//! and a change goes to every session there is (see `tabs::inject`, which
//! keeps each session's script ids to take them out again).
//!
//! An init script is `Page.addScriptToEvaluateOnNewDocument`, in the
//! page's own world or chrome-cdp's isolated one (the world `eval --world
//! isolated` uses). An exposed function is a `Runtime.addBinding` under a
//! name of our own ([`BINDING_PREFIX`]) plus a small script ([`wrapper`])
//! that puts a function under the asked name in each document: it takes any
//! arguments, sends them as JSON, and returns a promise, as Playwright's
//! `exposeFunction` does. Each call goes into the tab's console as
//! `binding <name>(args)` (see `observe::console`).
//!
//! Workers get neither (`Page` is not theirs).

pub(crate) mod commands;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use protocol::schema::InitScriptInfo;
use protocol::{ErrorInfo, ErrorKind};

use crate::observe::format;
use crate::request::EvalWorld;

/// The prefix of the binding behind an exposed function: `onSave` is
/// `__chromeCdpExpose_onSave` to Chrome, and the page only sees the
/// wrapper.
pub(crate) const BINDING_PREFIX: &str = "__chromeCdpExpose_";

/// How much of a script's first line `init-script list` shows.
const PREVIEW: usize = 60;

/// The largest init script taken.
pub const MAX_SCRIPT: usize = 5 * 1024 * 1024;

/// The session's init scripts and exposed functions.
#[derive(Debug, Default)]
pub(crate) struct Injector {
    table: Mutex<Table>,
}

#[derive(Debug, Default)]
struct Table {
    /// The last script id given.
    last_id: u32,
    /// In the order they run.
    scripts: Vec<Arc<Script>>,
    /// Exposed functions, oldest first, each with the serial number it was
    /// exposed under (a name exposed again is a new binding to sessions).
    bindings: Vec<(String, u32)>,
    /// The last binding serial number given.
    last_serial: u32,
}

/// An init script.
#[derive(Debug)]
pub(crate) struct Script {
    pub(crate) id: u32,
    /// What Chrome gets: the source, with a `sourceURL` so errors name it.
    pub(crate) source: String,
    pub(crate) world: EvalWorld,
    file: Option<PathBuf>,
    preview: String,
    size: u64,
}

/// What a page or iframe session has (or should have) of the session's
/// injections. Keys are never reused: script ids are not, and a binding
/// exposed again gets a new serial number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Script(u32),
    Binding { name: String, serial: u32 },
}

/// What a [`Key`] stands for.
#[derive(Debug, Clone)]
pub(crate) enum Wanted {
    Script(Arc<Script>),
    /// An exposed function's name.
    Binding(String),
}

fn lock(m: &Mutex<Table>) -> MutexGuard<'_, Table> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `s3`.
pub(crate) fn script_name(id: u32) -> String {
    format!("s{id}")
}

/// Parses a script id: `s3` or `3`.
pub(crate) fn parse_script_id(text: &str) -> Result<u32, ErrorInfo> {
    let digits = text.strip_prefix('s').unwrap_or(text);
    digits
        .parse::<u32>()
        .ok()
        .filter(|&n| n > 0 && !digits.starts_with('+'))
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{text}` is not an init script id; they look like s1, s2, …"),
            )
        })
}

/// Checks that `name` can be a global function's name: a JavaScript
/// identifier of ASCII letters, digits, `_` and `$`.
pub fn check_binding_name(name: &str) -> Result<(), ErrorInfo> {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$');
    if first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') {
        return Ok(());
    }
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!(
            "`{name}` cannot be a function name; use letters, digits, _ and $, not starting with a digit"
        ),
    ))
}

impl Script {
    fn info(&self) -> InitScriptInfo {
        InitScriptInfo {
            id: script_name(self.id),
            world: self.world.as_str().to_owned(),
            file: self.file.as_ref().map(|f| f.display().to_string()),
            preview: self.preview.clone(),
            size: self.size,
        }
    }
}

impl Injector {
    /// Adds a script that runs after the ones there are.
    pub(crate) fn add(&self, source: &str, file: Option<PathBuf>, world: EvalWorld) -> Arc<Script> {
        let mut table = lock(&self.table);
        table.last_id += 1;
        let id = table.last_id;
        let script = Arc::new(Script {
            id,
            source: with_source_url(source, id, file.as_deref()),
            world,
            preview: preview(source),
            size: source.len() as u64,
            file,
        });
        table.scripts.push(script.clone());
        script
    }

    /// The scripts, in the order they run.
    pub(crate) fn list(&self) -> Vec<InitScriptInfo> {
        lock(&self.table).scripts.iter().map(|s| s.info()).collect()
    }

    pub(crate) fn len(&self) -> usize {
        lock(&self.table).scripts.len()
    }

    /// Removes scripts `ids`, or none if one of them is not there (its id
    /// is the error).
    pub(crate) fn remove(&self, ids: &[u32]) -> Result<Vec<u32>, u32> {
        let mut table = lock(&self.table);
        if let Some(&missing) = ids
            .iter()
            .find(|&&id| !table.scripts.iter().any(|s| s.id == id))
        {
            return Err(missing);
        }
        table.scripts.retain(|s| !ids.contains(&s.id));
        let mut removed = ids.to_vec();
        removed.dedup();
        Ok(removed)
    }

    /// Removes every script; returns their ids.
    pub(crate) fn clear(&self) -> Vec<u32> {
        lock(&self.table).scripts.drain(..).map(|s| s.id).collect()
    }

    /// Exposes `names` (checked); returns those that were not yet.
    pub(crate) fn expose(&self, names: &[String]) -> Vec<String> {
        let mut table = lock(&self.table);
        let mut added = Vec::new();
        for name in names {
            if table.bindings.iter().any(|(n, _)| n == name) {
                continue;
            }
            table.last_serial += 1;
            let serial = table.last_serial;
            table.bindings.push((name.clone(), serial));
            added.push(name.clone());
        }
        added
    }

    /// Takes `names` away, or none if one of them is not exposed (it is the
    /// error).
    pub(crate) fn unexpose(&self, names: &[String]) -> Result<Vec<String>, String> {
        let mut table = lock(&self.table);
        if let Some(missing) = names
            .iter()
            .find(|name| !table.bindings.iter().any(|(n, _)| n == *name))
        {
            return Err(missing.clone());
        }
        table.bindings.retain(|(n, _)| !names.contains(n));
        let mut removed = names.to_vec();
        removed.dedup();
        Ok(removed)
    }

    /// Takes every exposed function away; returns their names.
    pub(crate) fn unexpose_all(&self) -> Vec<String> {
        lock(&self.table)
            .bindings
            .drain(..)
            .map(|(n, _)| n)
            .collect()
    }

    /// The exposed functions' names, oldest first.
    pub(crate) fn bindings(&self) -> Vec<String> {
        lock(&self.table)
            .bindings
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// What every page and iframe session should have, in the order it is
    /// added: the exposed functions first (so init scripts can call them),
    /// then the scripts.
    pub(crate) fn wanted(&self) -> Vec<(Key, Wanted)> {
        let table = lock(&self.table);
        let bindings = table.bindings.iter().map(|(name, serial)| {
            (
                Key::Binding {
                    name: name.clone(),
                    serial: *serial,
                },
                Wanted::Binding(name.clone()),
            )
        });
        let scripts = table
            .scripts
            .iter()
            .map(|s| (Key::Script(s.id), Wanted::Script(s.clone())));
        bindings.chain(scripts).collect()
    }
}

/// `source` with a `sourceURL` comment (unless it has one), so its errors
/// and console calls name its file (at its own line numbers) or
/// `init-script-s1.js`.
fn with_source_url(source: &str, id: u32, file: Option<&std::path::Path>) -> String {
    if source.contains("//# sourceURL=") {
        return source.to_owned();
    }
    let url = match file {
        Some(file) => format!("file://{}", file.display()),
        None => format!("init-script-{}.js", script_name(id)),
    };
    format!("{source}\n//# sourceURL={url}")
}

/// The first line with something on it, cut at [`PREVIEW`] characters.
fn preview(source: &str) -> String {
    let line = source
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut preview = line.to_owned();
    format::cut(&mut preview, PREVIEW);
    preview
}

/// The script that puts exposed function `name` in a document: it takes
/// the binding (and hides it) and calls it with the arguments as a JSON
/// array.
pub(crate) fn wrapper(name: &str) -> String {
    let binding = format!("{BINDING_PREFIX}{name}");
    include_str!("expose.js")
        .replace("__NAME__", &serde_json::to_string(name).unwrap_or_default())
        .replace(
            "__BINDING__",
            &serde_json::to_string(&binding).unwrap_or_default(),
        )
}

/// The script that takes exposed function `name` out of the documents
/// there are.
pub(crate) fn unwrapper(name: &str) -> String {
    format!(
        "delete globalThis[{}];",
        serde_json::to_string(name).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_get_ids_that_are_never_reused() {
        let injector = Injector::default();
        let a = injector.add("window.a = 1", None, EvalWorld::Main);
        let b = injector.add("\n\n  window.b = 2;\nmore", None, EvalWorld::Isolated);
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(injector.remove(&[1]), Ok(vec![1]));
        assert_eq!(injector.remove(&[1]), Err(1));
        let c = injector.add("x", Some("/tmp/x.js".into()), EvalWorld::Main);
        assert_eq!(c.id, 3);
        let list = injector.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "s2");
        assert_eq!(list[0].world, "isolated");
        assert_eq!(list[0].preview, "window.b = 2;");
        assert_eq!(list[1].file.as_deref(), Some("/tmp/x.js"));
        assert!(c.source.ends_with("\n//# sourceURL=file:///tmp/x.js"));
        assert!(b.source.ends_with("\n//# sourceURL=init-script-s2.js"));
        assert_eq!(injector.clear(), vec![2, 3]);
        assert_eq!(injector.len(), 0);
    }

    #[test]
    fn a_name_exposed_again_is_a_new_binding() {
        let injector = Injector::default();
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            injector.expose(&names(&["a", "b", "a"])),
            names(&["a", "b"])
        );
        assert_eq!(injector.expose(&names(&["a"])), names(&[]));
        assert_eq!(injector.unexpose(&names(&["c"])), Err("c".to_owned()));
        assert_eq!(injector.unexpose(&names(&["a"])), Ok(names(&["a"])));
        injector.expose(&names(&["a"]));
        injector.add("x", None, EvalWorld::Main);
        let keys: Vec<Key> = injector.wanted().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            vec![
                Key::Binding {
                    name: "b".into(),
                    serial: 2
                },
                Key::Binding {
                    name: "a".into(),
                    serial: 3
                },
                Key::Script(1),
            ]
        );
    }

    #[test]
    fn names_and_ids_are_checked() {
        for good in ["onSave", "_x", "$", "a1"] {
            assert!(check_binding_name(good).is_ok(), "{good}");
        }
        for bad in ["", "1a", "a-b", "a.b", "é"] {
            assert!(check_binding_name(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_script_id("s4").ok(), Some(4));
        assert_eq!(parse_script_id("4").ok(), Some(4));
        assert!(parse_script_id("s0").is_err());
        assert!(parse_script_id("r1").is_err());
    }

    #[test]
    fn the_wrapper_names_its_binding() {
        let script = wrapper("onSave");
        assert!(script.contains("\"__chromeCdpExpose_onSave\""), "{script}");
        assert!(script.contains("\"onSave\""), "{script}");
        assert!(!script.contains("__NAME__") && !script.contains("__BINDING__"));
    }
}

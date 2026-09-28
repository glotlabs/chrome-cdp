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
//!
//! Milestone 7.4's `state load` adds seeds here: for an origin with no page
//! open, an isolated-world script that gives the origin's first document
//! its storage and then reports on [`SEED_BINDING`], upon which the seed is
//! taken out ([`Injector::seeded`]). Seeds are not init scripts to the
//! user: `init-script list` does not show them.

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

/// The binding a storage seed reports on, in chrome-cdp's isolated world
/// only; its payload is the origin that got its storage.
pub(crate) const SEED_BINDING: &str = "__chromeCdpStateSeeded";

/// How much of a script's first line `init-script list` shows.
const PREVIEW: usize = 60;

/// The largest init script taken.
pub const MAX_SCRIPT: usize = 5 * 1024 * 1024;

/// The session's init scripts and exposed functions.
#[derive(Debug, Default)]
pub(crate) struct Injector {
    table: Mutex<Table>,
}

/// Every item belongs to a scope (see `crate::context`): `None` for the
/// session's default context, or a browser context of ours. Ids and
/// serial numbers are the session's, whatever the scope.
#[derive(Debug, Default)]
struct Table {
    /// The last script id given.
    last_id: u32,
    /// In the order they run.
    scripts: Vec<Arc<Script>>,
    /// Exposed functions, oldest first, each with the serial number it was
    /// exposed under (a name exposed again is a new binding to sessions),
    /// and its scope.
    bindings: Vec<(String, u32, Option<String>)>,
    /// The last binding serial number given.
    last_serial: u32,
    /// Storage seeds (`state load`), by origin; at most one per origin and
    /// scope.
    seeds: Vec<(String, Arc<Script>)>,
    /// The last seed id given.
    last_seed: u32,
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
    scope: Option<String>,
}

/// What a page or iframe session has (or should have) of the session's
/// injections. Keys are never reused: script ids are not, and a binding
/// exposed again gets a new serial number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Script(u32),
    Binding {
        name: String,
        serial: u32,
    },
    /// A storage seed, by its own ids.
    Seed(u32),
    /// [`SEED_BINDING`], while there are seeds.
    SeedBinding,
}

/// What a [`Key`] stands for.
#[derive(Debug, Clone)]
pub(crate) enum Wanted {
    Script(Arc<Script>),
    /// An exposed function's name.
    Binding(String),
    /// [`SEED_BINDING`].
    SeedBinding,
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
    /// Adds a script of `scope` that runs after the ones there are.
    pub(crate) fn add(
        &self,
        scope: Option<&str>,
        source: &str,
        file: Option<PathBuf>,
        world: EvalWorld,
    ) -> Arc<Script> {
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
            scope: scope.map(str::to_owned),
        });
        table.scripts.push(script.clone());
        script
    }

    /// The scripts of `scope`, in the order they run.
    pub(crate) fn list(&self, scope: Option<&str>) -> Vec<InitScriptInfo> {
        lock(&self.table)
            .scripts
            .iter()
            .filter(|s| s.scope.as_deref() == scope)
            .map(|s| s.info())
            .collect()
    }

    pub(crate) fn len(&self, scope: Option<&str>) -> usize {
        lock(&self.table)
            .scripts
            .iter()
            .filter(|s| s.scope.as_deref() == scope)
            .count()
    }

    /// Removes scripts `ids` of `scope`, or none if one of them is not
    /// there (its id is the error).
    pub(crate) fn remove(&self, scope: Option<&str>, ids: &[u32]) -> Result<Vec<u32>, u32> {
        let mut table = lock(&self.table);
        if let Some(&missing) = ids.iter().find(|&&id| {
            !table
                .scripts
                .iter()
                .any(|s| s.id == id && s.scope.as_deref() == scope)
        }) {
            return Err(missing);
        }
        table.scripts.retain(|s| !ids.contains(&s.id));
        let mut removed = ids.to_vec();
        removed.dedup();
        Ok(removed)
    }

    /// Removes every script of `scope`; returns their ids.
    pub(crate) fn clear(&self, scope: Option<&str>) -> Vec<u32> {
        let mut table = lock(&self.table);
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut table.scripts)
            .into_iter()
            .partition(|s| s.scope.as_deref() == scope);
        table.scripts = kept;
        gone.iter().map(|s| s.id).collect()
    }

    /// Exposes `names` (checked) in `scope`; returns those that were not
    /// yet.
    pub(crate) fn expose(&self, scope: Option<&str>, names: &[String]) -> Vec<String> {
        let mut table = lock(&self.table);
        let mut added = Vec::new();
        for name in names {
            if table
                .bindings
                .iter()
                .any(|(n, _, s)| n == name && s.as_deref() == scope)
            {
                continue;
            }
            table.last_serial += 1;
            let serial = table.last_serial;
            table
                .bindings
                .push((name.clone(), serial, scope.map(str::to_owned)));
            added.push(name.clone());
        }
        added
    }

    /// Takes `names` away from `scope`, or none if one of them is not
    /// exposed (it is the error).
    pub(crate) fn unexpose(
        &self,
        scope: Option<&str>,
        names: &[String],
    ) -> Result<Vec<String>, String> {
        let mut table = lock(&self.table);
        if let Some(missing) = names.iter().find(|name| {
            !table
                .bindings
                .iter()
                .any(|(n, _, s)| n == *name && s.as_deref() == scope)
        }) {
            return Err(missing.clone());
        }
        table
            .bindings
            .retain(|(n, _, s)| s.as_deref() != scope || !names.contains(n));
        let mut removed = names.to_vec();
        removed.dedup();
        Ok(removed)
    }

    /// Takes every exposed function of `scope` away; returns their names.
    pub(crate) fn unexpose_all(&self, scope: Option<&str>) -> Vec<String> {
        let mut table = lock(&self.table);
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut table.bindings)
            .into_iter()
            .partition(|(_, _, s)| s.as_deref() == scope);
        table.bindings = kept;
        gone.into_iter().map(|(n, _, _)| n).collect()
    }

    /// The names of the functions exposed in `scope`, oldest first.
    pub(crate) fn bindings(&self, scope: Option<&str>) -> Vec<String> {
        lock(&self.table)
            .bindings
            .iter()
            .filter(|(_, _, s)| s.as_deref() == scope)
            .map(|(n, _, _)| n.clone())
            .collect()
    }

    /// Adds `source`, an isolated-world script, as the storage seed of
    /// `origin` in `scope`, in place of an earlier one.
    pub(crate) fn seed(&self, scope: Option<&str>, origin: &str, source: &str) {
        let mut table = lock(&self.table);
        table.last_seed += 1;
        let id = table.last_seed;
        let script = Arc::new(Script {
            id,
            source: format!("{source}\n//# sourceURL=chrome-cdp-state-seed-{id}.js"),
            world: EvalWorld::Isolated,
            file: None,
            preview: String::new(),
            size: source.len() as u64,
            scope: scope.map(str::to_owned),
        });
        table
            .seeds
            .retain(|(o, s)| o != origin || s.scope.as_deref() != scope);
        table.seeds.push((origin.to_owned(), script));
    }

    /// Takes out `origin`'s seed in `scope`, which has done its work;
    /// returns whether there was one.
    pub(crate) fn seeded(&self, scope: Option<&str>, origin: &str) -> bool {
        let mut table = lock(&self.table);
        let before = table.seeds.len();
        table
            .seeds
            .retain(|(o, s)| o != origin || s.scope.as_deref() != scope);
        table.seeds.len() != before
    }

    /// Drops everything of `scope`, a context that closed.
    pub(crate) fn drop_scope(&self, scope: &str) {
        let mut table = lock(&self.table);
        let theirs = |s: &Option<String>| s.as_deref() == Some(scope);
        table.scripts.retain(|s| !theirs(&s.scope));
        table.bindings.retain(|(_, _, s)| !theirs(s));
        table.seeds.retain(|(_, s)| !theirs(&s.scope));
    }

    /// What every page and iframe session of `scope` should have, in the
    /// order it is added: the exposed functions first (so init scripts can
    /// call them), then the storage seeds (after their binding), so init
    /// scripts see the storage they give, then the scripts.
    pub(crate) fn wanted(&self, scope: Option<&str>) -> Vec<(Key, Wanted)> {
        let table = lock(&self.table);
        let bindings = table
            .bindings
            .iter()
            .filter(|(_, _, s)| s.as_deref() == scope)
            .map(|(name, serial, _)| {
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
            .filter(|s| s.scope.as_deref() == scope)
            .map(|s| (Key::Script(s.id), Wanted::Script(s.clone())));
        let seeds: Vec<_> = table
            .seeds
            .iter()
            .filter(|(_, s)| s.scope.as_deref() == scope)
            .map(|(_, s)| (Key::Seed(s.id), Wanted::Script(s.clone())))
            .collect();
        let seed_binding = (!seeds.is_empty()).then_some((Key::SeedBinding, Wanted::SeedBinding));
        bindings
            .chain(seed_binding)
            .chain(seeds)
            .chain(scripts)
            .collect()
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
        let a = injector.add(None, "window.a = 1", None, EvalWorld::Main);
        let b = injector.add(None, "\n\n  window.b = 2;\nmore", None, EvalWorld::Isolated);
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(injector.remove(None, &[1]), Ok(vec![1]));
        assert_eq!(injector.remove(None, &[1]), Err(1));
        let c = injector.add(None, "x", Some("/tmp/x.js".into()), EvalWorld::Main);
        assert_eq!(c.id, 3);
        let list = injector.list(None);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "s2");
        assert_eq!(list[0].world, "isolated");
        assert_eq!(list[0].preview, "window.b = 2;");
        assert_eq!(list[1].file.as_deref(), Some("/tmp/x.js"));
        assert!(c.source.ends_with("\n//# sourceURL=file:///tmp/x.js"));
        assert!(b.source.ends_with("\n//# sourceURL=init-script-s2.js"));
        assert_eq!(injector.clear(None), vec![2, 3]);
        assert_eq!(injector.len(None), 0);
    }

    #[test]
    fn each_scope_has_its_own_injections() {
        let injector = Injector::default();
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        injector.add(None, "a", None, EvalWorld::Main);
        let b = injector.add(Some("C1"), "b", None, EvalWorld::Main);
        injector.expose(Some("C1"), &names(&["f"]));
        injector.seed(Some("C1"), "https://x", "seed()");
        assert_eq!(injector.len(None), 1);
        assert_eq!(injector.list(Some("C1"))[0].id, "s2");
        // Another scope's script is not there to remove.
        assert_eq!(injector.remove(None, &[b.id]), Err(b.id));
        assert!(injector.bindings(None).is_empty());
        let keys: Vec<Key> = injector.wanted(None).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![Key::Script(1)]);
        let keys: Vec<Key> = injector
            .wanted(Some("C1"))
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec![
                Key::Binding {
                    name: "f".into(),
                    serial: 1
                },
                Key::SeedBinding,
                Key::Seed(1),
                Key::Script(2),
            ]
        );
        assert!(!injector.seeded(None, "https://x"));
        injector.drop_scope("C1");
        assert!(injector.wanted(Some("C1")).is_empty());
        assert_eq!(injector.len(None), 1);
    }

    #[test]
    fn a_name_exposed_again_is_a_new_binding() {
        let injector = Injector::default();
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            injector.expose(None, &names(&["a", "b", "a"])),
            names(&["a", "b"])
        );
        assert_eq!(injector.expose(None, &names(&["a"])), names(&[]));
        assert_eq!(injector.unexpose(None, &names(&["c"])), Err("c".to_owned()));
        assert_eq!(injector.unexpose(None, &names(&["a"])), Ok(names(&["a"])));
        injector.expose(None, &names(&["a"]));
        injector.add(None, "x", None, EvalWorld::Main);
        let keys: Vec<Key> = injector.wanted(None).into_iter().map(|(k, _)| k).collect();
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

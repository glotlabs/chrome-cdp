//! Init scripts and exposed functions (plan 07, milestone 7.3):
//! `init-script add|list|remove`, `expose` and `unexpose`.
//!
//! Like routes and emulation, both belong to the session's default browser
//! context, or with `--context` to that one ([`Injector`], in the tab
//! manager's settings, so they outlive a browser): every page and
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
//! `binding <name>(args)` (see `observe::console`). A function exposed with
//! answers (`expose NAME --returns`, plan 14.7) numbers its calls and waits:
//! the daemon hands each call its answer ([`Injector::answer`]) in the
//! calling context, which resolves the call's promise.
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

/// The binding the mock shim (plan 11.9: mocked WebSockets and
/// EventSources) asks the daemon on, in the page's own world.
pub(crate) const MOCK_BINDING: &str = "__chromeCdpMock";

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
    /// Exposed functions, oldest first.
    bindings: Vec<Binding>,
    /// The last binding serial number given.
    last_serial: u32,
    /// Storage seeds (`state load`), by origin; at most one per origin and
    /// scope.
    seeds: Vec<(String, Arc<Script>)>,
    /// The last seed id given.
    last_seed: u32,
    /// The scopes whose documents get the mock shim, each with its
    /// serial number (a shim turned on again is new to sessions).
    shims: Vec<(Option<String>, u32)>,
}

/// An exposed function.
#[derive(Debug)]
struct Binding {
    name: String,
    /// The serial number it was exposed under: a name exposed again, or
    /// given other answers, is a new binding to sessions.
    serial: u32,
    scope: Option<String>,
    /// What its calls get, in call order, the last repeating; none for a
    /// function whose calls resolve at once to `undefined`.
    answers: Vec<serde_json::Value>,
    /// How many calls got an answer.
    answered: usize,
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
    /// The mock shim and [`MOCK_BINDING`], by serial number.
    MockShim(u32),
}

/// What a [`Key`] stands for.
#[derive(Debug, Clone)]
pub(crate) enum Wanted {
    Script(Arc<Script>),
    /// An exposed function's name, and whether its calls wait for an
    /// answer.
    Binding {
        name: String,
        answers: bool,
    },
    /// [`SEED_BINDING`].
    SeedBinding,
    /// The mock shim, behind [`MOCK_BINDING`].
    MockShim,
}

impl Wanted {
    /// Whether workers get it too, not only pages and iframes: the mock
    /// shim does (a worker's sockets are mocked like a page's), and so do
    /// init scripts of the page's world (plan 13.3: the test runner's
    /// seeded `Math.random` and frozen clock reach workers that way). A
    /// worker has no isolated world, and no exposed functions yet.
    pub(crate) fn in_workers(&self) -> bool {
        match self {
            Self::MockShim => true,
            Self::Script(script) => script.world == EvalWorld::Main,
            Self::Binding { .. } | Self::SeedBinding => false,
        }
    }
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

    /// The script of `scope` added with this source and file, if there is
    /// one (`open --init-script` given again adds nothing).
    pub(crate) fn find(
        &self,
        scope: Option<&str>,
        source: &str,
        file: Option<&std::path::Path>,
    ) -> Option<InitScriptInfo> {
        lock(&self.table)
            .scripts
            .iter()
            .find(|s| {
                s.scope.as_deref() == scope
                    && s.file.as_deref() == file
                    && s.source == with_source_url(source, s.id, file)
            })
            .map(|s| s.info())
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

    /// Exposes `names` (checked) in `scope`, their calls answered with
    /// `answers` (see [`Binding::answers`]); returns those that were not
    /// yet, or had other answers.
    pub(crate) fn expose(
        &self,
        scope: Option<&str>,
        names: &[String],
        answers: &[serde_json::Value],
    ) -> Vec<String> {
        let mut table = lock(&self.table);
        let mut added = Vec::new();
        for name in names {
            let same = |b: &Binding| b.name == *name && b.scope.as_deref() == scope;
            if table
                .bindings
                .iter()
                .any(|b| same(b) && b.answers == answers)
                || added.contains(name)
            {
                continue;
            }
            table.last_serial += 1;
            let binding = Binding {
                name: name.clone(),
                serial: table.last_serial,
                scope: scope.map(str::to_owned),
                answers: answers.to_vec(),
                answered: 0,
            };
            match table.bindings.iter_mut().find(|b| same(b)) {
                Some(slot) => *slot = binding,
                None => table.bindings.push(binding),
            }
            added.push(name.clone());
        }
        added
    }

    /// The answer to the next call of exposed function `name` of `scope`,
    /// if it has answers.
    pub(crate) fn answer(&self, scope: Option<&str>, name: &str) -> Option<serde_json::Value> {
        let mut table = lock(&self.table);
        let binding = table
            .bindings
            .iter_mut()
            .find(|b| b.name == name && b.scope.as_deref() == scope)?;
        let answer = binding
            .answers
            .get(binding.answered.min(binding.answers.len().checked_sub(1)?))?
            .clone();
        binding.answered += 1;
        Some(answer)
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
                .any(|b| b.name == **name && b.scope.as_deref() == scope)
        }) {
            return Err(missing.clone());
        }
        table
            .bindings
            .retain(|b| b.scope.as_deref() != scope || !names.contains(&b.name));
        let mut removed = names.to_vec();
        removed.dedup();
        Ok(removed)
    }

    /// Takes every exposed function of `scope` away; returns their names.
    pub(crate) fn unexpose_all(&self, scope: Option<&str>) -> Vec<String> {
        let mut table = lock(&self.table);
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut table.bindings)
            .into_iter()
            .partition(|b| b.scope.as_deref() == scope);
        table.bindings = kept;
        gone.into_iter().map(|b| b.name).collect()
    }

    /// The functions exposed in `scope`, oldest first, each with its
    /// answers.
    pub(crate) fn bindings(&self, scope: Option<&str>) -> Vec<(String, Vec<serde_json::Value>)> {
        lock(&self.table)
            .bindings
            .iter()
            .filter(|b| b.scope.as_deref() == scope)
            .map(|b| (b.name.clone(), b.answers.clone()))
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

    /// Gives `scope`'s documents the mock shim, or takes it away; returns
    /// whether that changed anything.
    pub(crate) fn mock_shim(&self, scope: Option<&str>, on: bool) -> bool {
        let mut table = lock(&self.table);
        let has = table.shims.iter().any(|(s, _)| s.as_deref() == scope);
        if has == on {
            return false;
        }
        if on {
            table.last_serial += 1;
            let serial = table.last_serial;
            table.shims.push((scope.map(str::to_owned), serial));
        } else {
            table.shims.retain(|(s, _)| s.as_deref() != scope);
        }
        true
    }

    /// Drops everything of `scope`, a context that closed.
    pub(crate) fn drop_scope(&self, scope: &str) {
        let mut table = lock(&self.table);
        let theirs = |s: &Option<String>| s.as_deref() == Some(scope);
        table.shims.retain(|(s, _)| !theirs(s));
        table.scripts.retain(|s| !theirs(&s.scope));
        table.bindings.retain(|b| !theirs(&b.scope));
        table.seeds.retain(|(_, s)| !theirs(&s.scope));
    }

    /// What every page and iframe session of `scope` should have, in the
    /// order it is added: the exposed functions first (so init scripts can
    /// call them), then the storage seeds (after their binding), so init
    /// scripts see the storage they give, then the mock shim, then the
    /// scripts.
    pub(crate) fn wanted(&self, scope: Option<&str>) -> Vec<(Key, Wanted)> {
        let table = lock(&self.table);
        let bindings = table
            .bindings
            .iter()
            .filter(|b| b.scope.as_deref() == scope)
            .map(|b| {
                (
                    Key::Binding {
                        name: b.name.clone(),
                        serial: b.serial,
                    },
                    Wanted::Binding {
                        name: b.name.clone(),
                        answers: !b.answers.is_empty(),
                    },
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
        let shim = table
            .shims
            .iter()
            .find(|(s, _)| s.as_deref() == scope)
            .map(|(_, serial)| (Key::MockShim(*serial), Wanted::MockShim));
        bindings
            .chain(seed_binding)
            .chain(seeds)
            .chain(shim)
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
/// array, or with `answers` as `{"call":N,"args":[…]}`, keeping the call's
/// promise for its answer ([`answer_script`]).
pub(crate) fn wrapper(name: &str, answers: bool) -> String {
    let binding = format!("{BINDING_PREFIX}{name}");
    include_str!("expose.js")
        .replace("__NAME__", &serde_json::to_string(name).unwrap_or_default())
        .replace(
            "__BINDING__",
            &serde_json::to_string(&binding).unwrap_or_default(),
        )
        .replace("__ANSWERS__", if answers { "true" } else { "false" })
}

/// The symbol an answering wrapper of `name` keeps its answer function
/// under.
fn answer_symbol(name: &str) -> String {
    format!(
        "Symbol.for({})",
        serde_json::to_string(&format!("chrome-cdp.expose.{name}")).unwrap_or_default()
    )
}

/// The script that hands call `call` of exposed function `name` its
/// answer, in the context the call came from.
pub(crate) fn answer_script(name: &str, call: u64, answer: &serde_json::Value) -> String {
    format!("globalThis[{}]?.({call}, {answer});", answer_symbol(name))
}

/// The mock shim: `WebSocket` and `EventSource` stand-ins that ask the
/// daemon on [`MOCK_BINDING`].
pub(crate) fn mock_shim() -> String {
    format!(
        "{}\n//# sourceURL=chrome-cdp-mock-shim.js",
        include_str!("../route/shim.js").replace("__BINDING__", MOCK_BINDING)
    )
}

/// The script that takes exposed function `name` out of the documents
/// there are.
pub(crate) fn unwrapper(name: &str) -> String {
    format!(
        "delete globalThis[{}]; delete globalThis[{}];",
        serde_json::to_string(name).unwrap_or_default(),
        answer_symbol(name)
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
        injector.expose(Some("C1"), &names(&["f"]), &[]);
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
            injector.expose(None, &names(&["a", "b", "a"]), &[]),
            names(&["a", "b"])
        );
        assert_eq!(injector.expose(None, &names(&["a"]), &[]), names(&[]));
        assert_eq!(injector.unexpose(None, &names(&["c"])), Err("c".to_owned()));
        assert_eq!(injector.unexpose(None, &names(&["a"])), Ok(names(&["a"])));
        injector.expose(None, &names(&["a"]), &[]);
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
        let script = wrapper("onSave", false);
        assert!(script.contains("\"__chromeCdpExpose_onSave\""), "{script}");
        assert!(script.contains("\"onSave\""), "{script}");
        assert!(!script.contains("__NAME__") && !script.contains("__BINDING__"));
    }

    #[test]
    fn answers_go_in_call_order_and_the_last_repeats() {
        use serde_json::json;
        let injector = Injector::default();
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        injector.expose(None, &names(&["plain"]), &[]);
        injector.expose(None, &names(&["seq"]), &[json!(1), json!("two")]);
        assert_eq!(injector.answer(None, "plain"), None);
        assert_eq!(injector.answer(None, "nope"), None);
        assert_eq!(injector.answer(Some("C1"), "seq"), None);
        let answers: Vec<_> = (0..3).map(|_| injector.answer(None, "seq")).collect();
        assert_eq!(
            answers,
            [Some(json!(1)), Some(json!("two")), Some(json!("two"))]
        );
        // The same answers change nothing; others are a new binding that
        // starts over.
        assert!(
            injector
                .expose(None, &names(&["seq"]), &[json!(1), json!("two")])
                .is_empty()
        );
        assert_eq!(
            injector.expose(None, &names(&["seq"]), &[json!(3)]),
            names(&["seq"])
        );
        assert_eq!(injector.answer(None, "seq"), Some(json!(3)));
        let wanted = injector.wanted(None);
        assert!(matches!(
            &wanted[1],
            (
                Key::Binding { serial: 3, .. },
                Wanted::Binding { answers: true, .. }
            )
        ));
        assert!(wrapper("seq", true).contains("const answers = true;"));
        assert_eq!(
            answer_script("seq", 4, &json!({"a": [1]})),
            r#"globalThis[Symbol.for("chrome-cdp.expose.seq")]?.(4, {"a":[1]});"#
        );
    }
}

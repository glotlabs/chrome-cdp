//! Mock sets in use, layered: [`Mocks`] decides what a request gets.
//!
//! Sets applied later are tried first; within a set, the first mock that
//! matches answers, then its collections are tried. A request no mock
//! answers is *unmatched* if some set's `scope` owns it (the first such
//! set's `unmatched` policy handles it); otherwise it goes to the network.
//!
//! The state (plan 11.3) lives here too: hit counters (`{{seq}}`, and
//! where a `sequence` is), the named values of `[state]` that every set
//! shares, and the collections' items. [`Mocks::reset`] starts it all
//! over.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use protocol::schema::{
    MockCheck, MockCollectionInfo, MockExplainData, MockInfo, MockRef, MockSetInfo,
};
use serde_json::{Value, json};

use crate::closest::{CLOSEST, Closeness, closest, glob_similarity, path_similarity};
use crate::collection::{self, Collection, Op};
use crate::file::{AfterSequence, Cors, Unmatched};
use crate::load::{Mock, MockSet, Step};
use crate::matcher::{Parsed, Request, json_subset};
use crate::respond::{Respond, Response};
use crate::template::{self, Rng, Vars};

/// How often [`Mocks::refresh`] looks at the files at most.
const REFRESH: Duration = Duration::from_millis(250);

/// A mock's answer to a request.
#[derive(Debug)]
pub struct Answer {
    pub mock: MockRef,
    pub response: Response,
    /// How long to hold the request first.
    pub delay: Duration,
    /// What the journal says: `fulfill 200`, `fulfill 404 (no
    /// data/products/9.json)`, `fulfill 500 (mock error: …)`.
    pub outcome: String,
    /// The JSON data file the body is, when it is one as is (not a
    /// template): what `test --verify-mocks --update` rewrites.
    pub data_file: Option<std::path::PathBuf>,
    /// An `sse` answer's events, for those who can send them one by one
    /// (the body has them all).
    pub events: Option<Vec<crate::SseEvent>>,
}

/// What mocks do with a request.
#[derive(Debug)]
pub enum Outcome {
    /// A mock answers it.
    Answer(Answer),
    /// No mock answers it, and set `set` owns it: `response` is its
    /// `unmatched` policy's (`policy`) answer. `closest` are the mocks
    /// that come closest to matching it, with the condition each fails.
    Unmatched {
        set: String,
        policy: Unmatched,
        response: Response,
        outcome: String,
        closest: Vec<MockCheck>,
    },
    /// A CORS preflight for a request the mocks answer (or own): allow it.
    Preflight { mock: Option<MockRef> },
    /// No mock answers it and no set owns it: it goes to the network.
    Pass,
}

/// The mock sets in use, in the order they were applied. A copy has
/// their state as it is, and goes on alone (`explore`'s context).
#[derive(Debug, Clone)]
pub struct Mocks {
    pub(crate) sets: Vec<MockSet>,
    seed: u64,
    pub(crate) rng: Rng,
    checked: Option<Instant>,
    /// The named values, shared by every set.
    pub(crate) state: BTreeMap<String, Value>,
}

/// What `mock seed` fills a collection with.
#[derive(Debug, Clone)]
pub enum Seed {
    /// Its starting items (its `seed` file read again, or `items`).
    Start,
    /// A data file: absolute, or relative to the mocks directory, else to
    /// `cwd`.
    File {
        path: String,
        cwd: std::path::PathBuf,
    },
    /// Items given as JSON (from stdin).
    Items(Value),
}

/// A named value or a collection, as `mock state <name>` and `expect
/// mock-state` see it.
#[derive(Debug, Clone, PartialEq)]
pub enum StateItem {
    Value(Value),
    Collection {
        info: MockCollectionInfo,
        items: Vec<Value>,
    },
}

/// What answers a request.
enum Found {
    Mock {
        mock: usize,
        params: BTreeMap<String, String>,
        step: usize,
    },
    Collection {
        collection: usize,
        op: Op,
    },
}

impl Mocks {
    /// No sets; random values (`uuid`, `faker`, latency ranges) follow
    /// `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            sets: Vec::new(),
            seed,
            rng: Rng::new(seed),
            checked: None,
            state: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// The sets' names, in the order they were applied.
    pub fn names(&self) -> Vec<String> {
        self.sets.iter().map(|s| s.name().to_owned()).collect()
    }

    /// Uses `sets` (in layering order): instead of the sets in use, or with
    /// `add` on top of them (a set already in use moves to the top).
    pub fn apply(&mut self, sets: Vec<MockSet>, add: bool) {
        if !add {
            self.sets.clear();
            self.state.clear();
        }
        for set in sets {
            self.sets.retain(|s| s.name() != set.name());
            for (name, value) in &set.state {
                if add {
                    self.state
                        .entry(name.clone())
                        .or_insert_with(|| value.clone());
                } else {
                    self.state.insert(name.clone(), value.clone());
                }
            }
            self.sets.push(set);
        }
    }

    /// Warnings about the sets in use: `when` and `set_state` names that
    /// no set's `[state]` declares (a typo, or a set left out).
    pub fn undeclared(&self) -> Vec<String> {
        let declared = |name: &str| {
            self.sets
                .iter()
                .any(|s| s.state.iter().any(|(n, _)| n == name))
        };
        let mut warnings = Vec::new();
        for set in &self.sets {
            for mock in &set.mocks {
                for (key, names) in [("when", &mock.when), ("set_state", &mock.set_state)] {
                    for (name, value) in names {
                        let key = if value.is_null() && key == "set_state" {
                            "unset_state"
                        } else {
                            key
                        };
                        if !declared(name) {
                            let r = mock_ref(set, mock);
                            warnings.push(format!(
                                "{} \"{}\": {key} names `{name}`, which no [state] of the sets in use declares",
                                r.at, r.name
                            ));
                        }
                    }
                }
            }
        }
        warnings
    }

    /// Turns sets `names` off; a name not in use is `Err` and nothing
    /// changes.
    pub fn remove(&mut self, names: &[String]) -> Result<Vec<String>, String> {
        if let Some(missing) = names
            .iter()
            .find(|n| !self.sets.iter().any(|s| s.name() == n.as_str()))
        {
            return Err(missing.clone());
        }
        self.sets.retain(|s| !names.iter().any(|n| n == s.name()));
        Ok(names.to_vec())
    }

    /// Turns every set off; returns their names.
    pub fn clear(&mut self) -> Vec<String> {
        let names = self.names();
        self.sets.clear();
        names
    }

    /// Starts the state over: hit counters (`{{seq}}`, sequences), named
    /// values, collections (their `seed` files read again) and random
    /// values. A seed file that no longer reads keeps the items it had,
    /// and the set's [`MockSetInfo::reload_error`] says why.
    pub fn reset(&mut self) {
        self.state.clear();
        for set in &mut self.sets {
            for mock in &mut set.mocks {
                mock.hits = 0;
            }
            for ws in &mut set.websockets {
                ws.hits = 0;
                ws.answered = 0;
            }
            for (name, value) in &set.state {
                self.state.insert(name.clone(), value.clone());
            }
            for c in &mut set.collections {
                if let Some(file) = &c.seed_file {
                    match collection::read_items(&set.source.base, file, &c.id) {
                        Ok(items) => c.seed = items,
                        Err(e) => {
                            set.reload_error = Some(format!("collection {}: {e}", c.name));
                        }
                    }
                }
                c.reset();
            }
        }
        self.rng = Rng::new(self.seed);
    }

    /// Reloads the sets whose file changed (at most every 250ms). A file
    /// that no longer loads leaves its set as it was, with the error in
    /// [`MockSetInfo::reload_error`].
    pub fn refresh(&mut self) {
        let now = Instant::now();
        if self
            .checked
            .is_some_and(|t| now.duration_since(t) < REFRESH)
        {
            return;
        }
        self.checked = Some(now);
        for set in &mut self.sets {
            let modified = std::fs::metadata(&set.source.file)
                .and_then(|m| m.modified())
                .ok();
            if modified == set.modified {
                continue;
            }
            match MockSet::load(&set.source) {
                Ok(mut new) => {
                    let hits: HashMap<&str, u64> = set
                        .mocks
                        .iter()
                        .map(|m| (m.name.as_str(), m.hits))
                        .collect();
                    for mock in &mut new.mocks {
                        mock.hits = hits.get(mock.name.as_str()).copied().unwrap_or(0);
                    }
                    for ws in &mut new.websockets {
                        if let Some(old) = set.websockets.iter().find(|o| o.name == ws.name) {
                            ws.hits = old.hits;
                            ws.answered = old.answered;
                        }
                    }
                    // Collections keep their items, and new values start.
                    for c in &mut new.collections {
                        if let Some(old) = set.collections.iter().find(|o| o.name == c.name) {
                            c.items.clone_from(&old.items);
                            c.hits = old.hits;
                        }
                    }
                    for (name, value) in &new.state {
                        self.state
                            .entry(name.clone())
                            .or_insert_with(|| value.clone());
                    }
                    *set = new;
                }
                Err(e) => {
                    set.modified = modified;
                    set.reload_error = Some(e.message);
                }
            }
        }
    }

    /// The sets, in the order they are tried; with `mocks`, their mocks.
    pub fn info(&self, mocks: bool) -> Vec<MockSetInfo> {
        self.sets
            .iter()
            .rev()
            .map(|set| MockSetInfo {
                name: set.name().to_owned(),
                file: set.source.file.display().to_string(),
                count: set.mocks.len(),
                collections: set.collections.len(),
                websockets: set.websockets.len(),
                scope: set.scope_text.clone(),
                unmatched: set.unmatched.as_str().into(),
                cors: match set.cors {
                    Cors::Auto => "auto".into(),
                    Cors::Off => "off".into(),
                },
                latency: set.latency.text(),
                reload_error: set.reload_error.clone(),
                mocks: if mocks {
                    let listed = set.mocks.iter().map(|m| MockInfo {
                        mock: mock_ref(set, m),
                        matches: matches_text(m),
                        responds: describe(m, None),
                        hits: m.hits,
                    });
                    let collections = set.collections.iter().map(|c| MockInfo {
                        mock: collection_ref(set, c),
                        matches: c.summary(),
                        responds: format!(
                            "collection {}: {} {}",
                            c.name,
                            c.items.len(),
                            if c.items.len() == 1 { "item" } else { "items" }
                        ),
                        hits: c.hits,
                    });
                    let websockets = set.websockets.iter().map(|ws| ws.info(set));
                    listed.chain(collections).chain(websockets).collect()
                } else {
                    Vec::new()
                },
            })
            .collect()
    }

    /// The named values, and the collections in the order they are
    /// tried.
    pub fn state(&self) -> (BTreeMap<String, Value>, Vec<MockCollectionInfo>) {
        let collections = self
            .sets
            .iter()
            .rev()
            .flat_map(|set| set.collections.iter().map(|c| collection_info(set, c)))
            .collect();
        (self.state.clone(), collections)
    }

    /// Every name `mock state` knows: the collections', then the values'.
    pub fn state_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for set in self.sets.iter().rev() {
            for c in &set.collections {
                if !names.contains(&c.name) {
                    names.push(c.name.clone());
                }
            }
        }
        for set in &self.sets {
            for (name, _) in &set.state {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        for name in self.state.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    }

    /// Collection `name` (the one tried first), else named value `name`;
    /// a value declared in a `[state]` but cleared is `null`.
    pub fn lookup(&self, name: &str) -> Option<StateItem> {
        if let Some((set, c)) = self.find_collection(name) {
            return Some(StateItem::Collection {
                info: collection_info(set, c),
                items: c.items.clone(),
            });
        }
        if let Some(value) = self.state.get(name) {
            return Some(StateItem::Value(value.clone()));
        }
        self.declares(name).then_some(StateItem::Value(Value::Null))
    }

    fn declares(&self, name: &str) -> bool {
        self.sets
            .iter()
            .any(|s| s.state.iter().any(|(n, _)| n == name))
    }

    /// `test --verify-mocks`: collection `mock` answered a POST with
    /// `created`, and the real API with `real`. The mock's item takes the
    /// real id, so later requests for `path/<real id>` find it as they
    /// did on the real API. Whether it changed.
    pub fn adopt_id(&mut self, mock: &MockRef, created: &Value, real: &Value) -> bool {
        self.sets
            .iter_mut()
            .filter(|set| set.name() == mock.set)
            .flat_map(|set| set.collections.iter_mut())
            .find(|c| c.line == mock.line && c.name == mock.name)
            .is_some_and(|c| c.adopt_id(created, real))
    }

    fn find_collection(&self, name: &str) -> Option<(&MockSet, &Collection)> {
        self.sets
            .iter()
            .rev()
            .find_map(|set| Some((set, set.collections.iter().find(|c| c.name == name)?)))
    }

    /// Sets named values (`null` clears one). A name that no `[state]`
    /// declares and nothing has set is `Err(name)`, and nothing changes.
    pub fn set_values(&mut self, values: &[(String, Value)]) -> Result<(), String> {
        if let Some((name, _)) = values
            .iter()
            .find(|(n, _)| !self.state.contains_key(n) && !self.declares(n))
        {
            return Err(name.clone());
        }
        for (name, value) in values {
            if value.is_null() {
                self.state.remove(name);
            } else {
                self.state.insert(name.clone(), value.clone());
            }
        }
        Ok(())
    }

    /// Fills collection `name` (the one tried first) with `seed`'s items;
    /// returns it and where the items came from. `Err` says why not, and
    /// nothing changes.
    pub fn seed(
        &mut self,
        name: &str,
        seed: &Seed,
    ) -> Result<(MockCollectionInfo, String), String> {
        let Some(set) = self
            .sets
            .iter_mut()
            .rev()
            .find(|s| s.collections.iter().any(|c| c.name == name))
        else {
            return Err(format!("no collection {name} in the mock sets in use"));
        };
        let base = set.source.base.clone();
        let Some(c) = set.collections.iter_mut().find(|c| c.name == name) else {
            return Err(format!("no collection {name}"));
        };
        let (items, from) = match seed {
            Seed::Start => {
                if let Some(file) = &c.seed_file {
                    c.seed = collection::read_items(&base, file, &c.id)?;
                }
                (c.seed.clone(), "seed".to_owned())
            }
            Seed::Items(value) => (
                collection::check_items(value.clone(), &c.id)?,
                "stdin".to_owned(),
            ),
            Seed::File { path, cwd } => {
                let file = seed_file(&base, cwd, path)?;
                let items = collection::read_items(&base, &file.to_string_lossy(), &c.id)?;
                (items, file.display().to_string())
            }
        };
        c.items = items;
        let info = collection_info(
            set,
            set.collections
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| unreachable!()),
        );
        Ok((info, from))
    }

    /// What `req` gets. `now_ms` is `{{now}}` (the frozen time, if any).
    /// A mock that answers counts a hit.
    pub fn decide(&mut self, req: &Request<'_>, now_ms: i64) -> Outcome {
        let parsed = Parsed::new(req);
        if let Some(outcome) = self.preflight(&parsed) {
            return outcome;
        }
        let state = &self.state;
        let found = self.sets.iter().enumerate().rev().find_map(|(s, set)| {
            let mock = set.mocks.iter().enumerate().find_map(|(m, mock)| {
                let params = mock.matcher.check(&parsed, &parsed.method, false).ok()?;
                holds(&mock.when, state).ok()?;
                let step = mock.next_step()?;
                Some(Found::Mock {
                    mock: m,
                    params,
                    step,
                })
            });
            mock.or_else(|| {
                set.collections.iter().enumerate().find_map(|(c, coll)| {
                    let op = coll.op(&parsed.path, &parsed.method).ok()?;
                    Some(Found::Collection { collection: c, op })
                })
            })
            .map(|found| (s, found))
        });
        let Some((s, found)) = found else {
            return self.unmatched(&parsed);
        };
        let (m, params, step) = match found {
            Found::Mock { mock, params, step } => (mock, params, step),
            Found::Collection { collection, op } => {
                return Outcome::Answer(self.collection_answer(s, collection, &op, &parsed));
            }
        };
        let seed = self.rng.next_u64();
        let state = self.state.clone();
        let set = &mut self.sets[s];
        let cors = set.cors == Cors::Auto;
        let latency = set.latency;
        let mock = &mut set.mocks[m];
        mock.hits += 1;
        let vars = vars(&parsed, params, mock.hits, now_ms, seed, state);
        let ctx = vars.context();
        let Step { respond, delay } = &mock.steps[step];
        let rendered = respond.render(&ctx, &set.source.base, cors).and_then(|r| {
            let changes = mock
                .set_state
                .iter()
                .map(|(name, value)| Ok((name.clone(), template::render_json(value, &ctx)?)))
                .collect::<Result<Vec<_>, String>>()?;
            Ok((r, changes))
        });
        let delay = *delay;
        let mock = mock_ref(set, &set.mocks[m]);
        let delay = delay.unwrap_or_else(|| latency.sample(|n| self.rng.below(n)));
        let rendered = rendered.map(|(r, changes)| {
            for (name, value) in changes {
                if value.is_null() {
                    self.state.remove(&name);
                } else {
                    self.state.insert(name, value);
                }
            }
            r
        });
        let answer = match rendered {
            Ok(rendered) => {
                let mut outcome = rendered.response.outcome();
                if let Some(note) = rendered.note {
                    outcome = format!("{outcome} ({note})");
                }
                Answer {
                    mock,
                    response: rendered.response,
                    delay,
                    outcome,
                    data_file: rendered.data_file,
                    events: rendered.events,
                }
            }
            Err(error) => {
                let body = json!({
                    "error": format!("chrome-cdp mock error in {} \"{}\": {error}", mock.at, mock.name),
                });
                Answer {
                    outcome: format!("fulfill 500 (mock error: {error})"),
                    mock,
                    response: Response::json(500, &body, cors),
                    delay: Duration::ZERO,
                    data_file: None,
                    events: None,
                }
            }
        };
        Outcome::Answer(answer)
    }

    /// Collection `c` of set `s` carries out `op`.
    fn collection_answer(&mut self, s: usize, c: usize, op: &Op, parsed: &Parsed) -> Answer {
        let set = &mut self.sets[s];
        let cors = set.cors == Cors::Auto;
        let latency = set.latency;
        let (response, note) = set.collections[c].handle(op, parsed, &mut self.rng, cors);
        let mock = collection_ref(set, &set.collections[c]);
        Answer {
            mock,
            outcome: format!("{} ({note})", response.outcome()),
            response,
            delay: latency.sample(|n| self.rng.below(n)),
            data_file: None,
            events: None,
        }
    }

    /// A CORS preflight (`OPTIONS` with `Access-Control-Request-Method`)
    /// for a request a mock answers, or a set with `cors = "auto"` owns.
    fn preflight(&self, parsed: &Parsed) -> Option<Outcome> {
        if parsed.method != "OPTIONS" {
            return None;
        }
        let method = parsed
            .header("access-control-request-method")?
            .to_ascii_uppercase();
        let auto = || self.sets.iter().rev().filter(|s| s.cors == Cors::Auto);
        for set in auto() {
            for mock in &set.mocks {
                let answers = mock
                    .next_step()
                    .is_some_and(|i| !mock.steps[i].respond.is_passthrough());
                if answers
                    && holds(&mock.when, &self.state).is_ok()
                    && mock.matcher.check(parsed, &method, true).is_ok()
                {
                    return Some(Outcome::Preflight {
                        mock: Some(mock_ref(set, mock)),
                    });
                }
            }
            for c in &set.collections {
                if c.op(&parsed.path, &method).is_ok() {
                    return Some(Outcome::Preflight {
                        mock: Some(collection_ref(set, c)),
                    });
                }
            }
        }
        auto()
            .any(|s| s.owns(&parsed.path, &parsed.base))
            .then_some(Outcome::Preflight { mock: None })
    }

    fn unmatched(&self, parsed: &Parsed) -> Outcome {
        let Some(set) = self
            .sets
            .iter()
            .rev()
            .find(|s| s.owns(&parsed.path, &parsed.base))
        else {
            return Outcome::Pass;
        };
        let response = unmatched_response(set, parsed);
        let outcome = format!("unmatched, {}", response.outcome());
        Outcome::Unmatched {
            set: set.name().to_owned(),
            policy: set.unmatched,
            response,
            outcome,
            closest: self.closest(parsed),
        }
    }

    /// The mocks (and collections) that come closest to answering a
    /// request none answers, best first, with why each does not.
    fn closest(&self, parsed: &Parsed) -> Vec<MockCheck> {
        let mut candidates = Vec::new();
        for set in self.sets.iter().rev() {
            for mock in &set.mocks {
                let (closeness, reason) = match mock.matcher.check(parsed, &parsed.method, false) {
                    Err(why) => (mock.matcher.closeness(parsed), why),
                    // It matches, but does not answer.
                    Ok(_) => (
                        Closeness {
                            path: 100,
                            close: true,
                            score: u32::MAX,
                        },
                        holds(&mock.when, &self.state).err().unwrap_or_else(|| {
                            format!(
                                "its sequence of {} is used up, and after_sequence = \"unmatched\"",
                                answers(mock.steps.len())
                            )
                        }),
                    ),
                };
                candidates.push((
                    closeness,
                    MockCheck {
                        mock: mock_ref(set, mock),
                        matches: false,
                        reason: Some(reason),
                    },
                ));
            }
            for coll in &set.collections {
                let Err(reason) = coll.op(&parsed.path, &parsed.method) else {
                    continue;
                };
                let item = format!("{}/:id", coll.path.to_string().trim_end_matches('/'));
                let list = glob_similarity(&coll.path, &parsed.path);
                let one = path_similarity(&item, &parsed.path);
                let best = if one.percent > list.percent {
                    one
                } else {
                    list
                };
                let path = best.percent.min(99);
                candidates.push((
                    Closeness {
                        path,
                        close: best.close,
                        score: path * 4 + 50,
                    },
                    MockCheck {
                        mock: collection_ref(set, coll),
                        matches: false,
                        reason: Some(reason),
                    },
                ));
            }
        }
        closest(candidates, CLOSEST)
    }

    /// Which mock would answer `req` and why the others would not, without
    /// counting a hit.
    pub fn explain(&self, req: &Request<'_>) -> MockExplainData {
        let parsed = Parsed::new(req);
        let mut checked = Vec::new();
        let mut answer: Option<(usize, Found)> = None;
        for (s, set) in self.sets.iter().enumerate().rev() {
            for (m, mock) in set.mocks.iter().enumerate() {
                let result =
                    mock.matcher
                        .check(&parsed, &parsed.method, false)
                        .and_then(|params| {
                            holds(&mock.when, &self.state)?;
                            let step = mock.next_step().ok_or_else(|| {
                            format!(
                                "its sequence of {} is used up, and after_sequence = \"unmatched\"",
                                answers(mock.steps.len())
                            )
                        })?;
                            Ok(Found::Mock {
                                mock: m,
                                params,
                                step,
                            })
                        });
                let (matches, reason) = match result {
                    Ok(found) => {
                        if answer.is_none() {
                            answer = Some((s, found));
                            (true, None)
                        } else {
                            (
                                true,
                                Some("matches too, but an earlier mock answers".into()),
                            )
                        }
                    }
                    Err(why) => (false, Some(why)),
                };
                checked.push(MockCheck {
                    mock: mock_ref(set, mock),
                    matches,
                    reason,
                });
            }
            for (c, coll) in set.collections.iter().enumerate() {
                let (matches, reason) = match coll.op(&parsed.path, &parsed.method) {
                    Ok(op) if answer.is_none() => {
                        answer = Some((s, Found::Collection { collection: c, op }));
                        (true, None)
                    }
                    Ok(_) => (
                        true,
                        Some("matches too, but an earlier mock answers".into()),
                    ),
                    Err(why) => (false, Some(why)),
                };
                checked.push(MockCheck {
                    mock: collection_ref(set, coll),
                    matches,
                    reason,
                });
            }
        }
        let scope = self
            .sets
            .iter()
            .rev()
            .find(|s| s.owns(&parsed.path, &parsed.base));
        let (answer, outcome, params) = if let Some((s, found)) = answer {
            let set = &self.sets[s];
            match found {
                Found::Mock { mock, params, .. } => {
                    let mock = &set.mocks[mock];
                    let vars = vars(
                        &parsed,
                        params.clone(),
                        mock.hits + 1,
                        0,
                        0,
                        self.state.clone(),
                    );
                    (
                        Some(mock_ref(set, mock)),
                        describe(mock, Some(&vars.context())),
                        params,
                    )
                }
                Found::Collection { collection, op } => {
                    let coll = &set.collections[collection];
                    (
                        Some(collection_ref(set, coll)),
                        collection_outcome(coll, &op, &parsed.path),
                        coll.path_params(&parsed.path),
                    )
                }
            }
        } else {
            let outcome = match scope {
                Some(set) => match set.unmatched {
                    Unmatched::Fail => format!(
                        "unmatched: in scope of {}, whose unmatched = \"fail\" answers 501, which fails a test",
                        set.name()
                    ),
                    Unmatched::Passthrough => format!(
                        "unmatched: in scope of {}, whose unmatched = \"passthrough\" sends it to the network",
                        set.name()
                    ),
                    Unmatched::Abort => format!(
                        "unmatched: in scope of {}, whose unmatched = \"abort\" fails it with a network error",
                        set.name()
                    ),
                },
                None => "no mock matches and no set's scope owns it: it goes to the network".into(),
            };
            (None, outcome, BTreeMap::new())
        };
        MockExplainData {
            method: parsed.method.clone(),
            url: parsed.url.clone(),
            route: None,
            answer,
            outcome,
            params,
            scope: scope.map(|s| s.name().to_owned()),
            checked,
        }
    }
}

/// A `mock seed` file: absolute, or in mocks directory `base`, else
/// relative to `cwd`.
fn seed_file(base: &Path, cwd: &Path, path: &str) -> Result<std::path::PathBuf, String> {
    let given = Path::new(path);
    if given.is_absolute() {
        return if given.is_file() {
            Ok(given.to_path_buf())
        } else {
            Err(format!("no file {path}"))
        };
    }
    [base.join(given), cwd.join(given)]
        .into_iter()
        .find(|f| f.is_file())
        .ok_or_else(|| {
            format!(
                "no file {path} in the mocks directory {} or in {}",
                base.display(),
                cwd.display()
            )
        })
}

/// The answer to an in-scope request no mock answers.
fn unmatched_response(set: &MockSet, parsed: &Parsed) -> Response {
    let cors = set.cors == Cors::Auto;
    match set.unmatched {
        Unmatched::Passthrough => Response::Passthrough { patch: None },
        Unmatched::Abort => Response::Abort { reason: "Failed" },
        Unmatched::Fail => {
            let body = json!({
                "error": format!(
                    "chrome-cdp: no mock for {} {} (in the scope of mock set {})",
                    parsed.method, parsed.path, set.name()
                ),
                "hint": format!("chrome-cdp mock explain {} '{}'", parsed.method, parsed.url),
            });
            let Response::Fulfill {
                status,
                mut headers,
                body,
                cors,
            } = Response::json(501, &body, cors)
            else {
                unreachable!("a JSON response fulfils");
            };
            headers.push(("X-Chrome-Cdp-Unmatched".into(), set.name().to_owned()));
            Response::Fulfill {
                status,
                headers,
                body,
                cors,
            }
        }
    }
}

fn mock_ref(set: &MockSet, mock: &Mock) -> MockRef {
    MockRef {
        set: set.name().to_owned(),
        name: mock.name.clone(),
        file: set.source.file.display().to_string(),
        line: mock.line,
        at: format!("{}:{}", set.file_name(), mock.line),
    }
}

fn collection_ref(set: &MockSet, c: &Collection) -> MockRef {
    MockRef {
        set: set.name().to_owned(),
        name: c.name.clone(),
        file: set.source.file.display().to_string(),
        line: c.line,
        at: format!("{}:{}", set.file_name(), c.line),
    }
}

fn collection_info(set: &MockSet, c: &Collection) -> MockCollectionInfo {
    MockCollectionInfo {
        mock: collection_ref(set, c),
        path: c.path.to_string(),
        count: c.items.len(),
    }
}

/// What a collection would do: `collection todos: get 7 (200)`.
fn collection_outcome(c: &Collection, op: &Op, path: &str) -> String {
    let status = match op {
        Op::List => "200".to_owned(),
        Op::Create => "201, or 400 without a JSON object body".to_owned(),
        Op::Get(id) | Op::Replace(id) | Op::Update(id) | Op::Delete(id) => {
            if c.has_at(id, path) {
                "200".to_owned()
            } else {
                format!("404, no {}/{id}", c.name)
            }
        }
        Op::NotAllowed(allow) => format!("405, it takes {allow}"),
    };
    format!("collection {}: {} ({status})", c.name, op.text())
}

/// `1 answer`, `3 answers`.
fn answers(n: usize) -> String {
    if n == 1 {
        "1 answer".into()
    } else {
        format!("{n} answers")
    }
}

/// Whether the named values `when` requires hold; else the first that
/// does not. `"*"` means any value but none.
fn holds(when: &[(String, Value)], state: &BTreeMap<String, Value>) -> Result<(), String> {
    for (name, want) in when {
        let Some(got) = state.get(name) else {
            if want.is_null() {
                continue;
            }
            return Err(format!(
                "state {name}: expects {}, but it is not set",
                shown(want)
            ));
        };
        let ok = if want.as_str() == Some("*") {
            !got.is_null()
        } else {
            json_subset(want, got, false, &mut String::new()).is_ok()
        };
        if !ok {
            return Err(format!(
                "state {name}: expects {}, it is {}",
                shown(want),
                shown(got)
            ));
        }
    }
    Ok(())
}

fn shown(value: &Value) -> String {
    let text = value.to_string();
    let cut: String = text.chars().take(60).collect();
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// What a mock matches, with its `when`: `POST /api/orders when
/// logged_in = true`.
fn matches_text(mock: &Mock) -> String {
    let mut text = mock.matcher.summary();
    if !mock.when.is_empty() {
        let when: Vec<String> = mock
            .when
            .iter()
            .map(|(n, v)| format!("{n} = {}", shown(v)))
            .collect();
        text.push_str(" when ");
        text.push_str(&when.join(", "));
    }
    text
}

/// What one answer is: `fulfill 200 json_file data/items.json`, with the
/// delay first if it has one.
fn describe_step(step: &Step, ctx: Option<&minijinja::Value>) -> String {
    let summary = step.respond.summary(ctx);
    let summary = if matches!(step.respond, Respond::Fulfill { .. }) {
        format!("fulfill {summary}")
    } else {
        summary
    };
    match step.delay {
        Some(delay) if !delay.is_zero() => {
            format!(
                "delay {}, then {summary}",
                protocol::format::duration(delay)
            )
        }
        _ => summary,
    }
}

/// What a mock answers next: its answer, or where its sequence is; and
/// the values it sets.
fn describe(mock: &Mock, ctx: Option<&minijinja::Value>) -> String {
    let mut text = match mock.sequence {
        None => describe_step(&mock.steps[0], ctx),
        Some(after) => {
            let n = mock.steps.len();
            match mock.next_step() {
                Some(i) => format!(
                    "sequence {}/{n} (then {}): {}",
                    i + 1,
                    after.as_str(),
                    describe_step(&mock.steps[i], ctx)
                ),
                None => format!(
                    "sequence of {} used up (after_sequence = \"{}\")",
                    answers(n),
                    AfterSequence::Unmatched.as_str()
                ),
            }
        }
    };
    let (unset, set): (Vec<_>, Vec<_>) = mock.set_state.iter().partition(|(_, v)| v.is_null());
    if !set.is_empty() {
        let set: Vec<String> = set
            .iter()
            .map(|(n, v)| format!("{n} = {}", shown(v)))
            .collect();
        text.push_str("; set_state ");
        text.push_str(&set.join(", "));
    }
    if !unset.is_empty() {
        let unset: Vec<&str> = unset.iter().map(|(n, _)| n.as_str()).collect();
        text.push_str("; unset_state ");
        text.push_str(&unset.join(", "));
    }
    text
}

pub(crate) fn vars(
    parsed: &Parsed,
    params: BTreeMap<String, String>,
    seq: u64,
    now_ms: i64,
    seed: u64,
    state: BTreeMap<String, Value>,
) -> Vars {
    let mut query = BTreeMap::new();
    for (k, v) in &parsed.query {
        query.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let mut headers = BTreeMap::new();
    for (k, v) in &parsed.headers {
        headers.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let form = parsed.form.as_ref().map(|f| {
        let mut map = BTreeMap::new();
        for (k, v) in f {
            map.entry(k.clone()).or_insert_with(|| v.clone());
        }
        map
    });
    let form_files = parsed.multipart.then(|| {
        let mut map = BTreeMap::new();
        for (k, f) in &parsed.files {
            map.entry(k.clone()).or_insert_with(|| f.clone());
        }
        map
    });
    Vars {
        method: parsed.method.clone(),
        url: parsed.url.clone(),
        path: parsed.path.clone(),
        params,
        query,
        headers,
        body: parsed.body.clone(),
        json: parsed.json.clone(),
        form,
        form_files,
        seq,
        now_ms,
        seed,
        state,
    }
}

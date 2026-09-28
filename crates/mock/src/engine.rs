//! Mock sets in use, layered: [`Mocks`] decides what a request gets.
//!
//! Sets applied later are tried first; within a set, the first mock that
//! matches answers. A request no mock answers is *unmatched* if some set's
//! `scope` owns it (the first such set's `unmatched` policy handles it);
//! otherwise it goes to the network.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use protocol::schema::{MockCheck, MockExplainData, MockInfo, MockRef, MockSetInfo};
use serde_json::json;

use crate::file::{Cors, Unmatched};
use crate::load::{Mock, MockSet};
use crate::matcher::{Parsed, Request};
use crate::respond::{Respond, Response};
use crate::template::{Rng, Vars};

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
}

/// What mocks do with a request.
#[derive(Debug)]
pub enum Outcome {
    /// A mock answers it.
    Answer(Answer),
    /// No mock answers it, and set `set` owns it: `response` is its
    /// `unmatched` policy's answer.
    Unmatched {
        set: String,
        response: Response,
        outcome: String,
    },
    /// A CORS preflight for a request the mocks answer (or own): allow it.
    Preflight { mock: Option<MockRef> },
    /// No mock answers it and no set owns it: it goes to the network.
    Pass,
}

/// The mock sets in use, in the order they were applied.
#[derive(Debug)]
pub struct Mocks {
    sets: Vec<MockSet>,
    seed: u64,
    rng: Rng,
    checked: Option<Instant>,
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
        }
        for set in sets {
            self.sets.retain(|s| s.name() != set.name());
            self.sets.push(set);
        }
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

    /// Starts the state over: hit counters (`{{seq}}`) and random values.
    pub fn reset(&mut self) {
        for set in &mut self.sets {
            for mock in &mut set.mocks {
                mock.hits = 0;
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
                scope: set.scope_text.clone(),
                unmatched: set.unmatched.as_str().into(),
                cors: match set.cors {
                    Cors::Auto => "auto".into(),
                    Cors::Off => "off".into(),
                },
                latency: set.latency.text(),
                reload_error: set.reload_error.clone(),
                mocks: if mocks {
                    set.mocks
                        .iter()
                        .map(|m| MockInfo {
                            mock: mock_ref(set, m),
                            matches: m.matcher.summary(),
                            responds: describe(m, None),
                            hits: m.hits,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
            })
            .collect()
    }

    /// What `req` gets. `now_ms` is `{{now}}` (the frozen time, if any).
    /// A mock that answers counts a hit.
    pub fn decide(&mut self, req: &Request<'_>, now_ms: i64) -> Outcome {
        let parsed = Parsed::new(req);
        if let Some(outcome) = self.preflight(&parsed) {
            return outcome;
        }
        let found = self.sets.iter().enumerate().rev().find_map(|(s, set)| {
            set.mocks.iter().enumerate().find_map(|(m, mock)| {
                mock.matcher
                    .check(&parsed, &parsed.method, false)
                    .ok()
                    .map(|params| (s, m, params))
            })
        });
        let Some((s, m, params)) = found else {
            return self.unmatched(&parsed);
        };
        let seed = self.rng.next_u64();
        let set = &mut self.sets[s];
        let cors = set.cors == Cors::Auto;
        let latency = set.latency;
        let mock = &mut set.mocks[m];
        mock.hits += 1;
        let vars = vars(&parsed, params, mock.hits, now_ms, seed);
        let rendered = mock.respond.render(&vars.context(), &set.source.base, cors);
        let delay = mock.delay;
        let mock = mock_ref(set, &set.mocks[m]);
        let delay = delay.unwrap_or_else(|| latency.sample(|n| self.rng.below(n)));
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
                }
            }
        };
        Outcome::Answer(answer)
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
                if !mock.respond.is_passthrough()
                    && mock.matcher.check(parsed, &method, true).is_ok()
                {
                    return Some(Outcome::Preflight {
                        mock: Some(mock_ref(set, mock)),
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
            response,
            outcome,
        }
    }

    /// Which mock would answer `req` and why the others would not, without
    /// counting a hit.
    pub fn explain(&self, req: &Request<'_>) -> MockExplainData {
        let parsed = Parsed::new(req);
        let mut checked = Vec::new();
        let mut answer: Option<(usize, usize, BTreeMap<String, String>)> = None;
        for (s, set) in self.sets.iter().enumerate().rev() {
            for (m, mock) in set.mocks.iter().enumerate() {
                let result = mock.matcher.check(&parsed, &parsed.method, false);
                let (matches, reason) = match result {
                    Ok(params) => {
                        if answer.is_none() {
                            answer = Some((s, m, params));
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
        }
        let scope = self
            .sets
            .iter()
            .rev()
            .find(|s| s.owns(&parsed.path, &parsed.base));
        let (answer, outcome, params) = if let Some((s, m, params)) = answer {
            let set = &self.sets[s];
            let mock = &set.mocks[m];
            let vars = vars(&parsed, params.clone(), mock.hits + 1, 0, 0);
            (
                Some(mock_ref(set, mock)),
                describe(mock, Some(&vars.context())),
                params,
            )
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

/// What a mock answers: `fulfill 200 json_file data/items.json`, with the
/// delay first if it has one.
fn describe(mock: &Mock, ctx: Option<&minijinja::Value>) -> String {
    let summary = mock.respond.summary(ctx);
    let summary = if matches!(mock.respond, Respond::Fulfill { .. }) {
        format!("fulfill {summary}")
    } else {
        summary
    };
    match mock.delay {
        Some(delay) if !delay.is_zero() => {
            format!(
                "delay {}, then {summary}",
                protocol::format::duration(delay)
            )
        }
        _ => summary,
    }
}

fn vars(
    parsed: &Parsed,
    params: BTreeMap<String, String>,
    seq: u64,
    now_ms: i64,
    seed: u64,
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
        seq,
        now_ms,
        seed,
    }
}

//! `mock use`, `mock off`, `mock list`, `mock log`, `mock reset` and
//! `mock explain` (plan 11, milestone 11.2); `mock state` and `mock seed`
//! (milestone 11.3).
//!
//! Mock sets belong to the session, as routing rules do: they apply to
//! every tab, popup and frame, and stay when the browser is restarted.
//! With `--context`, a browser context has its own sets, state and log
//! (milestone 11.4), which is how each test of a run gets fresh ones. The
//! CLI finds the sets' files; the daemon reads them here, so a mock file's
//! problems come back as the command's error.

use std::fmt::Write as _;

use mock::{MockSet, Mocks, Seed, StateItem};
use protocol::schema::{
    MockCollectionInfo, MockExplainData, MockLogData, MockOffData, MockSeedData, MockSetInfo,
    MockSetsData, MockStateData,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, suggest};
use serde_json::Value;

use super::{Request, Router, lock, route_name};
use crate::Engine;
use crate::request::{
    MockExplainRequest, MockListRequest, MockLogRequest, MockOffRequest, MockSeedRequest,
    MockStateRequest, MockUseRequest, millis,
};

impl Router {
    /// Uses `sets`, instead of the sets in use or (`add`) on top of them;
    /// also returns warnings about them.
    fn use_mocks(
        &self,
        scope: Option<&str>,
        sets: Vec<MockSet>,
        add: bool,
        seed: u64,
    ) -> (Vec<MockSetInfo>, Vec<String>) {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(scope);
        if !add {
            *mocks = Mocks::new(seed);
        }
        mocks.apply(sets, add);
        (mocks.info(false), mocks.undeclared())
    }

    /// Runs `f` on the mocks in use in `scope`, after looking for changed
    /// files.
    pub(crate) fn with_mocks<T>(&self, scope: Option<&str>, f: impl FnOnce(&mut Mocks) -> T) -> T {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(scope);
        mocks.refresh();
        f(mocks)
    }

    /// Turns sets `names` off (all without names); a name not in use is
    /// `Err`.
    fn mocks_off(
        &self,
        scope: Option<&str>,
        names: &[String],
    ) -> Result<(Vec<String>, Vec<MockSetInfo>), String> {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(scope);
        let removed = if names.is_empty() {
            mocks.clear()
        } else {
            mocks.remove(names)?
        };
        Ok((removed, mocks.info(false)))
    }

    /// The sets in use, with their mocks.
    fn mock_sets(&self, scope: Option<&str>, with_mocks: bool) -> Vec<MockSetInfo> {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(scope);
        mocks.refresh();
        mocks.info(with_mocks)
    }

    fn reset_mocks(&self, scope: Option<&str>) -> Vec<MockSetInfo> {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(scope);
        mocks.reset();
        mocks.info(false)
    }

    /// Which rule or mock would answer `req`, counting nothing.
    fn explain(&self, req: &Request<'_>, headers: &[(String, String)]) -> MockExplainData {
        let mut table = lock(&self.table);
        let mocks = table.mocks_in(req.context);
        mocks.refresh();
        let mut data = mocks.explain(&mock::Request {
            method: req.method,
            url: req.url,
            headers,
            body: req.body,
        });
        data.route = table
            .rules
            .iter()
            .find(|r| r.matches(req, req.method))
            .map(|r| route_name(r.id));
        data
    }
}

impl Engine {
    /// `mock use`: reads and checks the sets, then applies them and turns
    /// interception on.
    pub async fn mock_use(&self, req: MockUseRequest) -> Result<Output, ErrorInfo> {
        if req.sets.is_empty() {
            return Err(ErrorInfo::new(ErrorKind::BadArgs, "no mock set to use")
                .hint(Hint::run(Cmd::new("mock").arg("use").arg("base"))));
        }
        let sets = req
            .sets
            .iter()
            .map(MockSet::load)
            .collect::<Result<Vec<_>, _>>()?;
        let scope = self.context_key()?;
        let (sets, warnings) =
            self.settings
                .routes
                .use_mocks(scope.as_deref(), sets, req.add, req.seed);
        self.apply_routes(millis(req.timeout_ms)).await;
        let quiet: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
        let quiet = quiet.join("\n");
        let text = mock::text::sets(&sets);
        let data = MockSetsData {
            sets,
            available: None,
            dir: None,
        };
        let mut output = Output::typed(&data, text).quiet(quiet);
        for warning in warnings {
            output = output.warning(warning);
        }
        Ok(output)
    }

    /// `mock off`: turns sets off; without any left (and no rules),
    /// requests are no longer intercepted.
    pub async fn mock_off(&self, req: MockOffRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let (removed, sets) = self
            .settings
            .routes
            .mocks_off(scope.as_deref(), &req.sets)
            .map_err(|missing| {
                ErrorInfo::new(
                    ErrorKind::NotFound,
                    format!("mock set {missing} is not in use"),
                )
                .hint(
                    Hint::run(Cmd::new("mock").arg("list").session(&self.session))
                        .why("lists the sets in use"),
                )
            })?;
        self.apply_routes(millis(req.timeout_ms)).await;
        let mut text = if removed.is_empty() {
            "no mock sets were in use".to_owned()
        } else {
            format!("turned off {}", removed.join(", "))
        };
        if sets.is_empty() {
            text.push_str("; no mock sets in use");
        } else {
            let names: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
            let _ = write!(text, "; still in use: {}", names.join(", "));
        }
        let quiet = removed.join("\n");
        let data = MockOffData { removed, sets };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// `mock list`: the sets in use with their mocks and hits.
    pub fn mock_list(&self, req: &MockListRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let sets = self.settings.routes.mock_sets(scope.as_deref(), true);
        let text = if sets.is_empty() {
            format!(
                "no mock sets in use; {} applies one",
                Cmd::new("mock")
                    .arg("use")
                    .raw("<set>")
                    .session(&self.session)
            )
        } else {
            mock::text::sets(&sets)
        };
        let quiet: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
        let quiet = quiet.join("\n");
        let data = MockSetsData {
            sets,
            available: None,
            dir: None,
        };
        Ok(Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// `mock log`: the requests a mock answered or a set left unmatched
    /// (with `--all`, every request routing saw).
    pub fn mock_log(&self, req: &MockLogRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let (all, _) = self.settings.routes.journal(scope.as_deref(), 0);
        let kept: Vec<_> = all
            .into_iter()
            .filter(|e| {
                if req.unmatched {
                    e.unmatched.is_some()
                } else {
                    req.all || e.mock.is_some() || e.unmatched.is_some()
                }
            })
            .collect();
        let skip = if req.limit == 0 {
            0
        } else {
            kept.len().saturating_sub(req.limit)
        };
        let log: Vec<_> = kept.into_iter().skip(skip).collect();
        let mut text = String::new();
        if skip > 0 {
            let mut cmd = Cmd::new("mock").arg("log");
            if req.unmatched {
                cmd = cmd.switch("unmatched");
            }
            if req.all {
                cmd = cmd.switch("all");
            }
            let _ = writeln!(
                text,
                "… {skip} earlier {} not shown; {} shows them",
                if skip == 1 { "request" } else { "requests" },
                cmd.flag("limit", (skip + req.limit).to_string())
                    .session(&self.session)
            );
        }
        for entry in &log {
            let _ = writeln!(text, "{}", mock::text::log_line(entry));
            for check in &entry.closest {
                let _ = writeln!(text, "  closest mock: {}", mock::text::closest_line(check));
            }
        }
        if log.is_empty() {
            text.push_str(
                match (
                    self.settings.routes.has_mocks(scope.as_deref()),
                    req.unmatched,
                ) {
                    (false, _) => {
                        "no mocked requests; no mock sets are in use (chrome-cdp mock use <set>)"
                    }
                    (true, true) => "no unmatched requests",
                    (true, false) => "no mocked requests yet",
                },
            );
        }
        let quiet: Vec<&str> = log.iter().map(|e| e.url.as_str()).collect();
        let quiet = quiet.join("\n");
        let data = MockLogData {
            log,
            skipped: skip as u64,
        };
        Ok(Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(
                req.max_chars,
                "--max-chars 0 for all of it, or --limit N for fewer",
            ))
    }

    /// `mock reset`: hit counters (`{{seq}}`), sequences, named values,
    /// collections and random values start over.
    pub fn mock_reset(&self) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let sets = self.settings.routes.reset_mocks(scope.as_deref());
        let names: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
        let text = if sets.is_empty() {
            "no mock sets in use; nothing to reset".to_owned()
        } else {
            format!(
                "reset {}: hit counters, sequences, named values, collections and random values start over",
                names.join(", ")
            )
        };
        let quiet = names.join("\n");
        let data = MockSetsData {
            sets,
            available: None,
            dir: None,
        };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// `mock state [name] [--set name=value…]`: the named values and
    /// collections, or one of them; `--set` changes values first.
    pub fn mock_state(&self, req: &MockStateRequest) -> Result<Output, ErrorInfo> {
        let session = self.session.clone();
        let scope = self.context_key()?;
        let result = self.settings.routes.with_mocks(scope.as_deref(), |mocks| {
            if mocks.is_empty() {
                return Err(no_sets(&session, "state"));
            }
            if !req.set.is_empty() {
                mocks
                    .set_values(&req.set)
                    .map_err(|name| unknown_state(&session, &name, &mocks.state_names()))?;
            }
            let changed: Vec<String> = req.set.iter().map(|(n, _)| n.clone()).collect();
            let Some(name) = &req.name else {
                let (values, collections) = mocks.state();
                return Ok(MockStateData {
                    name: None,
                    value: None,
                    collection: None,
                    values,
                    collections,
                    changed,
                });
            };
            let item = mocks
                .lookup(name)
                .ok_or_else(|| unknown_state(&session, name, &mocks.state_names()))?;
            let (value, collection) = match item {
                StateItem::Value(value) => (value, None),
                StateItem::Collection { info, items } => (Value::Array(items), Some(info)),
            };
            Ok(MockStateData {
                name: Some(name.clone()),
                value: Some(value),
                collection,
                values: std::collections::BTreeMap::new(),
                collections: Vec::new(),
                changed,
            })
        });
        let data = result?;
        let mut text = mock::text::state(&data);
        if text.is_empty() {
            text = "no named values or collections in the mock sets in use".into();
        }
        if !data.changed.is_empty() && data.name.is_none() {
            text = format!("set {}\n{text}", data.changed.join(", "));
        }
        let quiet = if let Some(value) = &data.value {
            value.to_string()
        } else {
            let names: Vec<&str> = data
                .values
                .keys()
                .map(String::as_str)
                .chain(data.collections.iter().map(|c| c.mock.name.as_str()))
                .collect();
            names.join("\n")
        };
        Ok(Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it"))
    }

    /// `mock seed <collection> [file|-]`: the collection's items from a
    /// data file, from stdin, or its starting items again.
    pub fn mock_seed(&self, req: &MockSeedRequest) -> Result<Output, ErrorInfo> {
        let session = self.session.clone();
        let seed = match (&req.items, &req.file) {
            (Some(items), _) => Seed::Items(items.clone()),
            (None, Some(file)) => Seed::File {
                path: file.clone(),
                cwd: req.cwd.clone().unwrap_or_default(),
            },
            (None, None) => Seed::Start,
        };
        let scope = self.context_key()?;
        let (info, from) = self.settings.routes.with_mocks(scope.as_deref(), |mocks| {
            if mocks.is_empty() {
                return Err(no_sets(&session, "seed"));
            }
            if !matches!(mocks.lookup(&req.name), Some(StateItem::Collection { .. })) {
                let collections: Vec<String> = mocks
                    .state()
                    .1
                    .into_iter()
                    .map(|c| c.mock.name)
                    .collect();
                return Err(unknown_collection(&session, &req.name, &collections));
            }
            mocks
                .seed(&req.name, &seed)
                .map_err(|e| {
                    ErrorInfo::new(ErrorKind::BadArgs, format!("mock seed {}: {e}", req.name)).hint(
                        "FILE is in the mocks directory or relative to the current one, and holds a JSON array of objects, each with the collection's id field",
                    )
                })
        })?;
        let text = seeded_text(&info, &from);
        let quiet = info.count.to_string();
        let data = MockSeedData {
            collection: info,
            from,
        };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// `mock explain`: which rule or mock of the session would answer a
    /// request, and why each other mock would not.
    pub fn mock_explain(&self, req: &MockExplainRequest) -> Result<Output, ErrorInfo> {
        let scope = self.context_key()?;
        let headers_json: serde_json::Map<String, serde_json::Value> = req
            .headers
            .iter()
            .map(|(n, v)| (n.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let headers_json = serde_json::Value::Object(headers_json);
        let data = self.settings.routes.explain(
            &Request {
                url: &req.url,
                method: &req.method,
                resource_type: "Fetch",
                headers: &headers_json,
                context: scope.as_deref(),
                tab: None,
                body: req.body.as_deref().map(str::as_bytes),
                now_ms: 0,
            },
            &req.headers,
        );
        let text = mock::text::explain(&data, req.limit);
        let quiet = data
            .answer
            .as_ref()
            .map_or_else(String::new, |m| m.at.clone());
        Ok(Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it, or --limit N"))
    }
}

/// `seeded todos with 3 items from /abs/data/other.json`.
fn seeded_text(info: &MockCollectionInfo, from: &str) -> String {
    let items = if info.count == 1 { "item" } else { "items" };
    match from {
        "seed" => format!(
            "{} starts over with its {} seed {items}",
            info.mock.name, info.count
        ),
        "stdin" => format!(
            "seeded {} with {} {items} from stdin",
            info.mock.name, info.count
        ),
        file => format!(
            "seeded {} with {} {items} from {file}",
            info.mock.name, info.count
        ),
    }
}

/// `mock state|seed` without mock sets in use.
fn no_sets(session: &str, command: &str) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::NotFound,
        format!("no mock sets in use, so there is no mock {command}"),
    )
    .hint(Hint::run(
        Cmd::new("mock").arg("use").raw("<set>").session(session),
    ))
}

/// A name `mock state` does not know.
pub(crate) fn unknown_state(session: &str, name: &str, names: &[String]) -> ErrorInfo {
    let mut info = ErrorInfo::new(
        ErrorKind::NotFound,
        format!("the mocks have no named value or collection {name}"),
    );
    if !names.is_empty() {
        info = info.note("names", names.join(", "));
    }
    match suggest::similar(name, names.iter().map(String::as_str), 1).first() {
        Some(close) => info.hint(
            Hint::run(Cmd::new("mock").arg("state").arg(*close).session(session))
                .why("did you mean this?"),
        ),
        None => info.hint(format!(
            "declare it in a mock file's [state], or run {}",
            Cmd::new("mock").arg("state").session(session)
        )),
    }
}

fn unknown_collection(session: &str, name: &str, names: &[String]) -> ErrorInfo {
    let mut info = ErrorInfo::new(
        ErrorKind::NotFound,
        format!("no collection {name} in the mock sets in use"),
    );
    if !names.is_empty() {
        info = info.note("collections", names.join(", "));
    }
    match suggest::similar(name, names.iter().map(String::as_str), 1).first() {
        Some(close) => info.hint(
            Hint::run(Cmd::new("mock").arg("seed").arg(*close).session(session))
                .why("did you mean this?"),
        ),
        None => info.hint("add a [[collection]] to a mock file (see `chrome-cdp mock --help`)"),
    }
}

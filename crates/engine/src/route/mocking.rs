//! `mock use`, `mock off`, `mock list`, `mock log`, `mock reset` and
//! `mock explain` (plan 11, milestone 11.2).
//!
//! Mock sets belong to the session, as routing rules do: they apply to
//! every tab, popup and frame, and stay when the browser is restarted. The
//! CLI finds the sets' files; the daemon reads them here, so a mock file's
//! problems come back as the command's error.

use std::fmt::Write as _;

use mock::{MockSet, Mocks};
use protocol::schema::{MockExplainData, MockLogData, MockOffData, MockSetInfo, MockSetsData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use super::{Request, Router, lock, route_name};
use crate::Engine;
use crate::request::{
    MockExplainRequest, MockListRequest, MockLogRequest, MockOffRequest, MockUseRequest, millis,
};

impl Router {
    /// Uses `sets`, instead of the sets in use or (`add`) on top of them.
    fn use_mocks(&self, sets: Vec<MockSet>, add: bool, seed: u64) -> Vec<MockSetInfo> {
        let mut table = lock(&self.table);
        if !add {
            table.mocks = Mocks::new(seed);
        }
        table.mocks.apply(sets, add);
        table.mocks.info(false)
    }

    /// Turns sets `names` off (all without names); a name not in use is
    /// `Err`.
    fn mocks_off(&self, names: &[String]) -> Result<(Vec<String>, Vec<MockSetInfo>), String> {
        let mut table = lock(&self.table);
        let removed = if names.is_empty() {
            table.mocks.clear()
        } else {
            table.mocks.remove(names)?
        };
        Ok((removed, table.mocks.info(false)))
    }

    /// The sets in use, with their mocks.
    fn mock_sets(&self, mocks: bool) -> Vec<MockSetInfo> {
        let mut table = lock(&self.table);
        table.mocks.refresh();
        table.mocks.info(mocks)
    }

    fn reset_mocks(&self) -> Vec<MockSetInfo> {
        let mut table = lock(&self.table);
        table.mocks.reset();
        table.mocks.info(false)
    }

    /// Which rule or mock would answer `req`, counting nothing.
    fn explain(&self, req: &Request<'_>, headers: &[(String, String)]) -> MockExplainData {
        let mut table = lock(&self.table);
        table.mocks.refresh();
        let mut data = table.mocks.explain(&mock::Request {
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
        let sets = self.settings.routes.use_mocks(sets, req.add, req.seed);
        self.apply_routes(millis(req.timeout_ms)).await;
        let quiet: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
        let quiet = quiet.join("\n");
        let text = mock::text::sets(&sets);
        let data = MockSetsData {
            sets,
            available: None,
            dir: None,
        };
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// `mock off`: turns sets off; without any left (and no rules),
    /// requests are no longer intercepted.
    pub async fn mock_off(&self, req: MockOffRequest) -> Result<Output, ErrorInfo> {
        let (removed, sets) = self
            .settings
            .routes
            .mocks_off(&req.sets)
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
    pub fn mock_list(&self, req: &MockListRequest) -> Output {
        let sets = self.settings.routes.mock_sets(true);
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
        Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it")
    }

    /// `mock log`: the requests a mock answered or a set left unmatched
    /// (with `--all`, every request routing saw).
    pub fn mock_log(&self, req: &MockLogRequest) -> Output {
        let (all, _) = self.settings.routes.journal(0);
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
        }
        if log.is_empty() {
            text.push_str(match (self.settings.routes.has_mocks(), req.unmatched) {
                (false, _) => {
                    "no mocked requests; no mock sets are in use (chrome-cdp mock use <set>)"
                }
                (true, true) => "no unmatched requests",
                (true, false) => "no mocked requests yet",
            });
        }
        let quiet: Vec<&str> = log.iter().map(|e| e.url.as_str()).collect();
        let quiet = quiet.join("\n");
        let data = MockLogData {
            log,
            skipped: skip as u64,
        };
        Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(
                req.max_chars,
                "--max-chars 0 for all of it, or --limit N for fewer",
            )
    }

    /// `mock reset`: hit counters (`{{seq}}`) and random values start over.
    pub fn mock_reset(&self) -> Output {
        let sets = self.settings.routes.reset_mocks();
        let names: Vec<&str> = sets.iter().map(|s| s.name.as_str()).collect();
        let text = if sets.is_empty() {
            "no mock sets in use; nothing to reset".to_owned()
        } else {
            format!(
                "reset {}: hit counters and random values start over",
                names.join(", ")
            )
        };
        let quiet = names.join("\n");
        let data = MockSetsData {
            sets,
            available: None,
            dir: None,
        };
        Output::typed(&data, text).quiet(quiet)
    }

    /// `mock explain`: which rule or mock of the session would answer a
    /// request, and why each other mock would not.
    pub fn mock_explain(&self, req: &MockExplainRequest) -> Output {
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
                context: None,
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
        Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it, or --limit N")
    }
}

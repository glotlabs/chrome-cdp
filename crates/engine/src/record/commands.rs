//! `record start`, `record status` and `record stop` (plan 18, milestone
//! 18.6): the daemon's side. `stop` hands the recorded steps to the CLI,
//! which writes them as a test (base URL, quoting, `-o`).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use protocol::format::iso8601;
use protocol::schema::{RecordStartData, RecordStatusData, RecordedData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use super::{Nav, RECORD_BINDING, Raw, Recording, What, steps};
use crate::Engine;
use crate::context::Scoped as _;
use crate::request::{RecordStartRequest, RecordStopRequest, millis};

/// How long `stop` lets reports already sent arrive.
const SETTLE: Duration = Duration::from_millis(100);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The recorder's call in each document: `testid=` locators for any
/// attribute, since the recorded test runs where the same config holds.
fn recorder_source(testid: Option<&str>) -> String {
    let attribute = testid.unwrap_or(crate::locator::DEFAULT_TESTID_ATTRIBUTE);
    format!(
        "globalThis.__ccdp?.record({}, {}, true);",
        serde_json::json!(RECORD_BINDING),
        serde_json::json!(attribute)
    )
}

/// How many steps `recording` has so far.
fn count(recording: &Recording) -> usize {
    steps(recording)
        .0
        .iter()
        .filter(|s| !s.words.is_empty())
        .count()
}

impl Engine {
    /// `record start`: records from now on in the session's (or
    /// `--context`'s) tabs, from the active one, whose page is where the
    /// test starts (a `goto` step).
    pub async fn record_start(&self, req: RecordStartRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, None, timeout).await?;
        let scope = self.context_key()?;
        let started_ms = now_ms();
        // The test starts where the recording does.
        let url = running
            .tabs
            .info(page.id)
            .map(|info| info.url)
            .filter(|url| url.starts_with("http") || url.starts_with("file:"));
        let events = url
            .into_iter()
            .map(|url| Raw {
                tab: page.id,
                what: What::Navigate {
                    how: Nav::Goto,
                    url,
                },
            })
            .collect();
        let recording = Recording {
            name: req.name.clone(),
            scope: scope.clone(),
            started_ms,
            tab: page.id,
            existing: running
                .tabs
                .tab_ids_in(scope.as_deref())
                .into_iter()
                .collect::<HashSet<u32>>(),
            opened: HashMap::new(),
            asked: HashSet::new(),
            answered: HashMap::new(),
            events,
        };
        let dropped = self.settings.recorder.start(recording).map(|r| count(&r));
        self.settings.injections.set_recorder(Some((
            scope.as_deref(),
            recorder_source(req.testid_attribute.as_deref()),
        )));
        self.apply_injections(true, timeout).await;
        let tab = format!("t{}", page.id);
        let stop = Cmd::new("record")
            .arg("stop")
            .arg("-o")
            .arg("test.cdp")
            .scoped(&self.session);
        let mut text = format!("recording \"{}\" from tab {tab}", req.name);
        if let Some(n) = dropped {
            let _ = write!(text, " (the earlier recording, {n} steps, is dropped)");
        }
        let _ = write!(text, "\nact in the page (or send it input); then: {stop}");
        let data = RecordStartData {
            name: req.name,
            tab: tab.clone(),
            started: iso8601(started_ms),
            dropped,
        };
        Ok(Output::typed(&data, text).quiet(tab))
    }

    /// `record status`.
    pub fn record_status(&self) -> Result<Output, ErrorInfo> {
        let data = match self.settings.recorder.current() {
            Some(r) => RecordStatusData {
                recording: true,
                name: Some(r.name.clone()),
                started: Some(iso8601(r.started_ms)),
                steps: count(&r),
            },
            None => RecordStatusData {
                recording: false,
                name: None,
                started: None,
                steps: 0,
            },
        };
        let text = match &data.name {
            Some(name) => format!(
                "recording \"{name}\" since {}: {} {}",
                data.started.as_deref().unwrap_or_default(),
                data.steps,
                if data.steps == 1 { "step" } else { "steps" }
            ),
            None => "not recording".to_owned(),
        };
        let quiet = data.steps.to_string();
        Ok(Output::typed(&data, text).quiet(quiet))
    }

    /// `record stop`: ends the recording and gives its steps.
    pub async fn record_stop(&self, req: RecordStopRequest) -> Result<Output, ErrorInfo> {
        if self.settings.recorder.current().is_none() {
            return Err(
                ErrorInfo::new(ErrorKind::BadArgs, "no recording runs in this session").hint(
                    Hint::run(Cmd::new("record").arg("start").scoped(&self.session))
                        .why("then act in the page"),
                ),
            );
        }
        // Reports the page sent just now are still on their way.
        tokio::time::sleep(SETTLE).await;
        let Some(recording) = self.settings.recorder.stop() else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "no recording runs in this session",
            ));
        };
        self.settings.injections.set_recorder(None);
        self.apply_injections(false, millis(req.timeout_ms)).await;
        let (steps, warnings) = steps(&recording);
        let data = RecordedData {
            name: recording.name.clone(),
            started: iso8601(recording.started_ms),
            tab: format!("t{}", recording.tab),
            steps,
            warnings,
        };
        Ok(Output::typed(&data, String::new()))
    }
}

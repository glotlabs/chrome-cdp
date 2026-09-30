//! Stable locators of refs (plan 11, milestone 11.8): refs live only as
//! long as their session, so `history --as-scenario` writes the most
//! robust locator that finds the ref's element alone (the bundle's
//! `stable`, `injected/src/stable.ts`). The daemon asks for them before a
//! command that names refs runs, while the elements are still there.
//! `locator <LOC>` (plan 14, milestone 14.1) prints one, or with `--all`
//! every candidate tried.

use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{LocatorCandidate, LocatorData, RefLocator};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;

use super::{FrameOnly, LocateOptions, Want, until_found};
use crate::Engine;
use crate::frames::FrameInfo;
use crate::request::{LocatorRequest, millis};
use crate::snapshot::parse_ref;

/// Whether `text` is a ref (`e12`, `@e12`).
pub fn is_ref(text: &str) -> bool {
    parse_ref(text).is_some()
}

/// What the bundle's `stable` answers.
#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    locator: Option<String>,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    /// With `all`.
    #[serde(default)]
    candidates: Vec<LocatorCandidate>,
}

impl Found {
    fn none(reason: impl Into<String>) -> Self {
        Self {
            locator: None,
            how: None,
            reason: Some(reason.into()),
            candidates: Vec::new(),
        }
    }
}

/// How wide `locator --all`'s locator column may get.
const COLUMN: usize = 80;

impl Engine {
    /// The stable locator of each of `refs` in tab `tab` (the active one
    /// if `None`), or why there is none. An element in an iframe gets a
    /// `frame=` step first (see [`frame_step`]).
    pub async fn stable_locators(
        &self,
        tab: Option<&str>,
        refs: &[String],
        testid_attribute: Option<&str>,
        timeout: Duration,
    ) -> Vec<RefLocator> {
        let options = LocateOptions {
            strict_refs: true,
            testid_attribute: testid_attribute.map(str::to_owned),
        };
        let mut out = Vec::with_capacity(refs.len());
        for reference in refs {
            let found = self
                .stable_locator(tab, reference, &options, false, None, true, timeout)
                .await
                .unwrap_or_else(|e| Found::none(e.message));
            out.push(RefLocator {
                reference: reference.trim_start_matches('@').to_owned(),
                locator: found.locator,
                how: found.how,
                reason: found.reason,
            });
        }
        out
    }

    /// `locator <LOCATOR>`: the most robust locator that finds the one
    /// element `LOCATOR` names, and with `--all` every candidate tried.
    pub async fn locator(&self, req: LocatorRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let retry = |r: &str| {
            let cmd = Cmd::new("locator").arg(r);
            if req.all { cmd.switch("all") } else { cmd }
        };
        let (options, retry, request) = (&options, &retry, &req);
        // It waits for the element, as the read commands do.
        let timeout = millis(req.timeout_ms);
        let found = until_found(
            tokio::time::Instant::now() + timeout,
            timeout,
            || async move {
                self.stable_locator(
                    request.tab.as_deref(),
                    &request.locator,
                    options,
                    request.all,
                    Some(retry),
                    true,
                    timeout,
                )
                .await
            },
        )
        .await?;
        let (Some(locator), Some(how)) = (found.locator, found.how) else {
            let reason = found
                .reason
                .unwrap_or_else(|| "the page did not say".to_owned());
            return Err(ErrorInfo::new(
                ErrorKind::NotFound,
                format!("no locator finds {} alone: {reason}", req.locator),
            )
            .hint(Hint::text(
                "use its ref in this session, or give it a test id (testid=…) for scripts",
            ))
            .details(serde_json::json!({
                "locator": req.locator,
                "reason": reason,
                "candidates": found.candidates,
            })));
        };
        let text = if req.all {
            let width = found
                .candidates
                .iter()
                .map(|c| c.locator.chars().count())
                .filter(|&n| n <= COLUMN)
                .max()
                .unwrap_or(0);
            let mut text = String::new();
            for c in &found.candidates {
                let _ = writeln!(text, "{:<width$}  {}", c.locator, c.count);
            }
            text.trim_end().to_owned()
        } else {
            locator.clone()
        };
        let data = LocatorData {
            locator: locator.clone(),
            how,
            candidates: req.all.then_some(found.candidates),
        };
        Ok(Output::typed(&data, text).quiet(locator))
    }

    /// The stable locator of ref `reference` in tab `tab`, or `None`,
    /// for a caller that holds the tab's lock already (it only reads).
    pub(crate) async fn stable_locator_held(
        &self,
        tab: &str,
        reference: &str,
        timeout: Duration,
    ) -> Option<String> {
        let options = LocateOptions {
            strict_refs: true,
            testid_attribute: None,
        };
        self.stable_locator(Some(tab), reference, &options, false, None, false, timeout)
            .await
            .ok()?
            .locator
    }

    /// The stable locator of the one element `text` names (with `all`,
    /// every candidate too), each with a `frame=` step first for an
    /// element in an iframe. With `lock`, it takes the tab's lock.
    #[allow(clippy::too_many_arguments)]
    async fn stable_locator(
        &self,
        tab: Option<&str>,
        text: &str,
        options: &LocateOptions,
        all: bool,
        retry: Option<&(dyn Fn(&str) -> Cmd + Sync)>,
        lock: bool,
        timeout: Duration,
    ) -> Result<Found, ErrorInfo> {
        let locator = options.parse(text)?;
        let attribute = serde_json::to_string(
            options
                .testid_attribute
                .as_deref()
                .unwrap_or(super::DEFAULT_TESTID_ATTRIBUTE),
        )
        .unwrap_or_else(|_| "\"data-testid\"".to_owned());
        let function =
            format!("function () {{ return globalThis.__ccdp.stable(this, {attribute}, {all}); }}");
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let _tab = if lock {
            Some(self.tabs.lock(&page.target_id).await)
        } else {
            None
        };
        let called = self
            .call_on(
                &running,
                &page,
                Some(&locator),
                options,
                Want::One(retry),
                FrameOnly::Owner,
                &function,
                timeout,
            )
            .await?;
        let mut found = called
            .value
            .as_str()
            .and_then(|s| serde_json::from_str::<Found>(s).ok())
            .unwrap_or_else(|| Found::none("the page did not say"));
        let frames = running.tabs.frames(page.id);
        if frames.first().is_none_or(|main| main.id == called.frame) {
            return Ok(found);
        }
        let Some(step) = frame_step(&frames, &called.frame) else {
            return Ok(Found::none(
                "it is in an iframe with no name or URL that only it has",
            ));
        };
        found.locator = found.locator.map(|inner| format!("{step} >> {inner}"));
        for c in &mut found.candidates {
            c.locator = format!("{step} >> {}", c.locator);
        }
        Ok(found)
    }
}

/// The `frame=` step that finds frame `id` among `frames` (a tab's), and
/// no other: its name, else its URL's path and query, else its path.
/// `frame=` looks in the whole tab, so the innermost frame is enough.
pub(crate) fn frame_step(frames: &[FrameInfo], id: &str) -> Option<String> {
    let frame = frames.iter().find(|f| f.id == id)?;
    let path = frame
        .url
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
        .map(|p| p.split('#').next().unwrap_or_default())
        .unwrap_or_default();
    let candidates = [
        frame.name.as_str(),
        path,
        path.split('?').next().unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .filter(|c| !c.is_empty() && *c != "/")
        .find(|c| finds_only(frames, c, id))
        .map(|c| format!("frame={}", quote(c)))
}

/// Whether `frame=<query>` finds frame `id` among `frames` and no other,
/// as [`crate::frames::FrameTree::find`] looks: by id, then by name, then
/// by a part of the URL.
fn finds_only(frames: &[FrameInfo], query: &str, id: &str) -> bool {
    if frames.iter().any(|f| f.id == query) {
        return query == id;
    }
    let iframes = || frames.iter().filter(|f| f.parent.is_some());
    let by_name: Vec<&FrameInfo> = iframes().filter(|f| f.name == query).collect();
    let matches: Vec<&FrameInfo> = if by_name.is_empty() {
        iframes().filter(|f| f.url.contains(query)).collect()
    } else {
        by_name
    };
    matches.len() == 1 && matches[0].id == id
}

/// A locator value, quoted when it would not parse bare.
fn quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:?=&%+,".contains(c))
    {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameInfo, frame_step};

    fn frame(id: &str, parent: Option<&str>, name: &str, url: &str) -> FrameInfo {
        FrameInfo {
            id: id.into(),
            parent: parent.map(Into::into),
            name: name.into(),
            url: url.into(),
            depth: usize::from(parent.is_some()),
            oopif: false,
        }
    }

    #[test]
    fn frames_are_named_by_name_then_url() {
        let frames = [
            frame("M", None, "", "http://a.test/checkout.html"),
            frame("P", Some("M"), "pay", "http://b.test/pay.html?x=1"),
            frame("Q", Some("M"), "", "http://b.test/ad.html?slot=1"),
            frame("R", Some("M"), "", "http://b.test/ad.html?slot=2"),
            frame("S", Some("M"), "my frame", "http://b.test/s.html"),
            frame("T", Some("M"), "", "http://b.test/same.html"),
            frame("U", Some("M"), "", "http://b.test/same.html"),
        ];
        assert_eq!(frame_step(&frames, "P").as_deref(), Some("frame=pay"));
        assert_eq!(
            frame_step(&frames, "Q").as_deref(),
            Some("frame=/ad.html?slot=1")
        );
        assert_eq!(
            frame_step(&frames, "S").as_deref(),
            Some("frame=\"my frame\"")
        );
        assert_eq!(frame_step(&frames, "T"), None);
    }
}

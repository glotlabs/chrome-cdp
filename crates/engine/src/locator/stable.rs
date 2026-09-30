//! Stable locators of refs (plan 11, milestone 11.8): refs live only as
//! long as their session, so `history --as-scenario` writes the most
//! robust locator that finds the ref's element alone (the bundle's
//! `stable`, `injected/src/stable.ts`). The daemon asks for them before a
//! command that names refs runs, while the elements are still there.

use std::time::Duration;

use protocol::ErrorInfo;
use protocol::schema::RefLocator;
use serde::Deserialize;

use super::{FrameOnly, LocateOptions, Want};
use crate::Engine;
use crate::frames::FrameInfo;
use crate::snapshot::parse_ref;

/// Whether `text` is a ref (`e12`, `@e12`).
pub fn is_ref(text: &str) -> bool {
    parse_ref(text).is_some()
}

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    locator: Option<String>,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

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
        let attribute = serde_json::to_string(testid_attribute.unwrap_or("data-testid"))
            .unwrap_or_else(|_| "\"data-testid\"".to_owned());
        let function =
            format!("function () {{ return globalThis.__ccdp.stable(this, {attribute}); }}");
        let mut out = Vec::with_capacity(refs.len());
        for reference in refs {
            let found = match self
                .stable_locator(tab, reference, &options, &function, timeout)
                .await
            {
                Ok(found) => found,
                Err(e) => Found {
                    locator: None,
                    how: None,
                    reason: Some(e.message),
                },
            };
            out.push(RefLocator {
                reference: reference.trim_start_matches('@').to_owned(),
                locator: found.locator,
                how: found.how,
                reason: found.reason,
            });
        }
        out
    }

    async fn stable_locator(
        &self,
        tab: Option<&str>,
        reference: &str,
        options: &LocateOptions,
        function: &str,
        timeout: Duration,
    ) -> Result<Found, ErrorInfo> {
        let locator = options.parse(reference)?;
        let running = self.running()?;
        let page = self.page(&running, tab, timeout).await?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let called = self
            .call_on(
                &running,
                &page,
                Some(&locator),
                options,
                Want::One(None),
                FrameOnly::Owner,
                function,
                timeout,
            )
            .await?;
        let found = called
            .value
            .as_str()
            .and_then(|s| serde_json::from_str::<Found>(s).ok())
            .unwrap_or(Found {
                locator: None,
                how: None,
                reason: Some("the page did not say".to_owned()),
            });
        let frames = running.tabs.frames(page.id);
        let Some(inner) = found.locator else {
            return Ok(found);
        };
        if frames.first().is_none_or(|main| main.id == called.frame) {
            return Ok(Found {
                locator: Some(inner),
                ..found
            });
        }
        Ok(match frame_step(&frames, &called.frame) {
            Some(step) => Found {
                locator: Some(format!("{step} >> {inner}")),
                ..found
            },
            None => Found {
                locator: None,
                how: None,
                reason: Some("it is in an iframe with no name or URL that only it has".to_owned()),
            },
        })
    }
}

/// The `frame=` step that finds frame `id` among `frames` (a tab's), and
/// no other: its name, else its URL's path and query, else its path.
/// `frame=` looks in the whole tab, so the innermost frame is enough.
fn frame_step(frames: &[FrameInfo], id: &str) -> Option<String> {
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

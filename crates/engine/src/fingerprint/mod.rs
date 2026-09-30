//! Fingerprints of the elements `test`'s steps find, and finding them
//! again (plan 18, milestone 18.2).
//!
//! The runner sends a step's locator with its call (`Request::fingerprint`);
//! the daemon runs the call in [`Engine::fingerprinting`]. The first time
//! the call resolves that locator to one element ([`Engine::locate_one`],
//! before an action acts), the bundle's `fingerprint` reads the element.
//! A call that resolves it some other way (`expect`, `wait`) gets it
//! read once it passed. When a step later fails because its locator
//! finds nothing, [`Engine::heal_match`] scores the elements of the page
//! against the fingerprint ([`score`]).

pub mod score;

use std::cell::RefCell;
use std::future::Future;
use std::time::Duration;

use protocol::schema::{Fingerprint, HealMatchData};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde::Deserialize;
use serde_json::Value;

use crate::errors::from_cdp;
use crate::frames::World;
use crate::injected::{self, GUARD};
use crate::locator::{FrameOnly, LocateOptions, Locator, frame_step};
use crate::request::{HealMatchRequest, millis};
use crate::tabs::{FrameContext, Page};
use crate::{Engine, Running};

/// How long reading a fingerprint after a call may take.
const AFTER: Duration = Duration::from_secs(2);

/// How many candidates [`Engine::heal_match`] returns.
const CANDIDATES: usize = 3;

/// What the call being handled wants fingerprinted, and what it got.
struct Wanted {
    locator: String,
    testid: Option<String>,
    taken: Option<Fingerprint>,
}

tokio::task_local! {
    static WANTED: RefCell<Wanted>;
}

/// An element of `healCandidates`.
#[derive(Deserialize)]
struct Read {
    i: usize,
    #[serde(flatten)]
    fingerprint: Fingerprint,
}

#[derive(Deserialize)]
struct Candidates {
    found: usize,
    elements: Vec<Read>,
}

impl Engine {
    /// Runs `call` (a daemon call) and returns the fingerprint of the
    /// element `locator` found, when asked for one (`testid`: the
    /// attribute test ids are read from). When the call did not resolve
    /// the locator to one element itself and `passed` says it succeeded,
    /// the element is read after it.
    pub async fn fingerprinting<F: Future>(
        &self,
        wanted: Option<(String, Option<String>)>,
        call: F,
        passed: impl FnOnce(&F::Output) -> bool,
    ) -> (F::Output, Option<Fingerprint>) {
        let Some((locator, testid)) = wanted else {
            return (call.await, None);
        };
        let wanted = RefCell::new(Wanted {
            locator: locator.clone(),
            testid: testid.clone(),
            taken: None,
        });
        WANTED
            .scope(wanted, async {
                let output = call.await;
                let mut taken = WANTED.with(|w| w.borrow_mut().taken.take());
                if taken.is_none() && passed(&output) {
                    taken = self.fingerprint_of(&locator, testid.as_deref()).await;
                }
                (output, taken)
            })
            .await
    }

    /// Called by [`Engine::locate_one`] with the one element (`object_id`
    /// in frame `frame`) that `locator` found: fingerprints it when the
    /// call wants it and has none yet.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn note_fingerprint(
        &self,
        running: &Running,
        page: &Page,
        locator: &Locator,
        context: &FrameContext,
        object_id: &str,
        frame: &str,
        timeout: Duration,
    ) {
        let testid = WANTED
            .try_with(|w| {
                let w = w.borrow();
                (w.taken.is_none() && w.locator == locator.to_string()).then(|| w.testid.clone())
            })
            .ok()
            .flatten();
        let Some(testid) = testid else { return };
        let taken = self
            .read_fingerprint(
                running,
                page,
                context,
                object_id,
                frame,
                testid.as_deref(),
                timeout,
            )
            .await;
        if let Some(taken) = taken {
            let _ = WANTED.try_with(|w| w.borrow_mut().taken = Some(taken));
        }
    }

    /// The fingerprint of the one element `locator` finds in the active
    /// tab now, if it finds one.
    async fn fingerprint_of(&self, locator: &str, testid: Option<&str>) -> Option<Fingerprint> {
        let options = LocateOptions {
            strict_refs: true,
            testid_attribute: testid.map(str::to_owned),
        };
        let parsed = options.parse(locator).ok()?;
        let running = self.running().ok()?;
        let page = self.page(&running, None, AFTER).await.ok()?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let target = self
            .locate_one(&running, &page, &parsed, &options, None, AFTER)
            .await
            .ok()?;
        // `locate_one` took it, as the call is still being handled.
        let taken = WANTED.try_with(|w| w.borrow_mut().taken.take()).ok()?;
        target.release();
        taken
    }

    /// Reads the fingerprint of element `object_id` of `context` (in
    /// frame `frame` of `page`).
    #[allow(clippy::too_many_arguments)]
    async fn read_fingerprint(
        &self,
        running: &Running,
        page: &Page,
        context: &FrameContext,
        object_id: &str,
        frame: &str,
        testid: Option<&str>,
        timeout: Duration,
    ) -> Option<Fingerprint> {
        let attribute = serde_json::to_string(testid.unwrap_or_default()).ok()?;
        let mut params = cdp::domains::runtime::CallFunctionOnParams::new(format!(
            "function () {{ {GUARD} return globalThis.__ccdp.fingerprint(this, {attribute}); }}"
        ));
        params.object_id = Some(object_id.to_owned());
        params.return_by_value = Some(true);
        let result = injected::call(
            &context.session.with_timeout(timeout.min(AFTER)),
            context.context_id,
            params,
        )
        .await
        .ok()?;
        let text = result.result.value.as_ref()?.as_str()?.to_owned();
        let mut fingerprint: Fingerprint = serde_json::from_str(&text).ok()?;
        fingerprint.classes = score::kept_classes(&fingerprint.classes);
        fingerprint.page = score::page_template(&fingerprint.page);
        if let Some(step) = in_frame(running, page, frame) {
            for c in &mut fingerprint.candidates {
                *c = format!("{step} >> {c}");
            }
        }
        Some(fingerprint)
    }

    /// The elements of the page that best match a step's fingerprint, and
    /// how many elements its locator finds now: for `test`'s heal (not a
    /// command of its own). The page is the frame the locator's `frame=`
    /// steps name, else the main one.
    pub async fn heal_match(&self, req: HealMatchRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let options = LocateOptions {
            strict_refs: true,
            testid_attribute: req.testid_attribute.clone(),
        };
        let locator = options.parse(&req.locator)?;
        let running = self.running()?;
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let _tab = self.tabs.lock(&page.target_id).await;
        let (frame, context) = match locator.frame_head() {
            Some(head) => {
                let place = self
                    .place(
                        &running,
                        &page,
                        &head,
                        &options,
                        FrameOnly::Document,
                        timeout,
                    )
                    .await?;
                let found = (place.frame.clone(), place.context.clone());
                place.release();
                found
            }
            None => {
                self.frame_and_context(&running, &page, None, World::Isolated, timeout)
                    .await?
            }
        };
        let mut fingerprint = req.fingerprint.clone();
        fingerprint.candidates.clear();
        let args = [
            serde_json::to_value(&fingerprint).unwrap_or(Value::Null),
            Value::String(req.testid_attribute.clone().unwrap_or_default()),
            serde_json::to_value(locator.in_frame_steps()).unwrap_or(Value::Null),
        ];
        let text = call_with(
            &context,
            "function (fp, testid, steps) { return globalThis.__ccdp.healCandidates(fp, testid, steps.length ? steps : null); }",
            &args,
            timeout,
            &self.session,
        )
        .await?;
        let read: Candidates = serde_json::from_str(&text).map_err(|e| {
            ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!("the page's candidates did not read: {e}"),
            )
        })?;
        let mut elements: Vec<Fingerprint> = Vec::with_capacity(read.elements.len());
        let mut ids: Vec<usize> = Vec::with_capacity(read.elements.len());
        for mut el in read.elements {
            el.fingerprint.classes = score::kept_classes(&el.fingerprint.classes);
            elements.push(el.fingerprint);
            ids.push(el.i);
        }
        let ranked = score::rank(&req.fingerprint, &elements, CANDIDATES);
        let mut candidates = Vec::with_capacity(ranked.len());
        for (n, r) in ranked.iter().enumerate() {
            let locator = if n == 0 {
                self.heal_locator(
                    &running,
                    &page,
                    &context,
                    &frame,
                    ids[r.index],
                    &req,
                    timeout,
                )
                .await
            } else {
                None
            };
            candidates.push(score::candidate(&elements[r.index], r, locator));
        }
        let data = HealMatchData {
            found: read.found,
            candidates,
        };
        let text = data
            .candidates
            .first()
            .map(|c| format!("{} ({:.2})", c.element, c.confidence))
            .unwrap_or_default();
        Ok(Output::typed(&data, text))
    }

    /// The stable locator of element `i` of the last `healCandidates` in
    /// `context`, with the `frame=` step of `frame` when it is an iframe.
    #[allow(clippy::too_many_arguments)]
    async fn heal_locator(
        &self,
        running: &Running,
        page: &Page,
        context: &FrameContext,
        frame: &str,
        i: usize,
        req: &HealMatchRequest,
        timeout: Duration,
    ) -> Option<String> {
        #[derive(Deserialize)]
        struct Found {
            locator: Option<String>,
        }
        let args = [
            Value::from(i),
            Value::String(req.testid_attribute.clone().unwrap_or_default()),
        ];
        let text = call_with(
            context,
            "function (i, testid) { return globalThis.__ccdp.healStable(i, testid); }",
            &args,
            timeout,
            &self.session,
        )
        .await
        .ok()?;
        let inner = serde_json::from_str::<Found>(&text).ok()?.locator?;
        match in_frame(running, page, frame) {
            Some(step) => Some(format!("{step} >> {inner}")),
            None => Some(inner),
        }
    }
}

/// The `frame=` step of frame `frame` of `page`, unless it is the main
/// frame (or has none that finds it alone).
fn in_frame(running: &Running, page: &Page, frame: &str) -> Option<String> {
    let frames = running.tabs.frames(page.id);
    if frames.first().is_none_or(|main| main.id == frame) {
        return None;
    }
    frame_step(&frames, frame)
}

/// Calls bundle function `function` with `args` in `context`; its string
/// result.
async fn call_with(
    context: &FrameContext,
    function: &str,
    args: &[Value],
    timeout: Duration,
    session: &str,
) -> Result<String, ErrorInfo> {
    let mut params = cdp::domains::runtime::CallFunctionOnParams::new(format!(
        "function (...args) {{ {GUARD} return ({function}).apply(null, args); }}"
    ));
    params.arguments = Some(
        args.iter()
            .map(|v| cdp::domains::runtime::CallArgument {
                value: Some(v.clone()),
                unserializable_value: None,
                object_id: None,
            })
            .collect(),
    );
    params.return_by_value = Some(true);
    let result = injected::call(
        &context.session.with_timeout(timeout),
        context.context_id,
        params,
    )
    .await
    .map_err(|e| from_cdp(&e, session))?;
    if let Some(e) = result.exception_details {
        return Err(ErrorInfo::new(
            ErrorKind::EvalError,
            format!("matching the fingerprint failed: {}", e.text),
        ));
    }
    Ok(result
        .result
        .value
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned())
}

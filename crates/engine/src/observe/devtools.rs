//! `highlight` and `inspect` (plan 06, milestone 6.6): helpers for a human
//! watching a headed browser while an agent drives it.
//!
//! `highlight` scrolls an element into view and paints DevTools' element
//! highlight over it (`Overlay.highlightNode`: content, padding, border
//! and margin boxes, and the info tooltip) for a while. It works headless
//! too, where only screenshots show it. `inspect` opens DevTools for the
//! tab (`Target.openDevTools`), which needs a window.

use std::sync::atomic::Ordering;
use std::time::Duration;

use cdp::domains::{dom, runtime, target as cdp_target};
use protocol::schema::{ActedElement, HighlightData, InspectData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde::Deserialize;
use serde_json::json;

use crate::errors::from_cdp;
use crate::injected::{self, GUARD};
use crate::locator::LocateOptions;
use crate::request::{HighlightRequest, InspectRequest, SnapshotEngine, millis};
use crate::tabs::tab_name;
use crate::{Engine, Running, Spec};

/// An element as `__ccdp.acted` describes it.
#[derive(Debug, Default, Deserialize)]
struct Described {
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    tag: String,
}

impl Engine {
    /// `highlight`: DevTools' highlight on one element, for a while.
    pub async fn highlight(&self, req: HighlightRequest) -> Result<Output, ErrorInfo> {
        let options = LocateOptions {
            strict_refs: req.strict_refs,
            testid_attribute: req.testid_attribute.clone(),
        };
        let locator = options.parse(&req.locator)?;
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let work = async {
            // Elements seen from now on get refs only while this holds.
            let generation = running
                .tabs
                .snapshot_memory(page.id, |m| m.refs.generation());
            let retry = |r: &str| Cmd::new("highlight").arg(r);
            let target = self
                .locate_one(&running, &page, &locator, &options, Some(&retry), timeout)
                .await?;
            let session = target.context.session.with_timeout(timeout);
            let mut describe = runtime::CallFunctionOnParams::new(format!(
                "function () {{ {GUARD} return globalThis.__ccdp.acted(this); }}"
            ));
            describe.object_id = Some(target.object_id.clone());
            describe.return_by_value = Some(true);
            let described = injected::call(&session, target.context.context_id, describe)
                .await
                .ok()
                .and_then(|r| r.result.value)
                .and_then(|v| serde_json::from_value::<Described>(v).ok())
                .unwrap_or_default();
            let backend = target.key.backend;
            // Its ref, given now if it has none (as actions do), unless a
            // frame loaded a new document meanwhile.
            let reference = running
                .tabs
                .snapshot_memory(page.id, |m| {
                    m.refs.get(&target.key).or_else(|| {
                        (Some(m.refs.generation()) == generation).then(|| {
                            m.refs.assign_from(
                                &target.key,
                                &described.role,
                                &described.name,
                                SnapshotEngine::Dom,
                            )
                        })
                    })
                })
                .flatten()
                .map(|n| format!("e{n}"));
            let note = target.note.clone();
            target.release();
            // Shown where a human can see it.
            let _ = session
                .call(dom::ScrollIntoViewIfNeededParams {
                    node_id: None,
                    backend_node_id: Some(backend),
                    object_id: None,
                    rect: None,
                })
                .await;
            // `Overlay` needs `DOM` on.
            let dom_on = session.call(dom::EnableParams::default());
            let overlay_on = session.call_raw("Overlay.enable", json!({}));
            dom_on.await.map_err(|e| from_cdp(&e, &self.session))?;
            overlay_on.await.map_err(|e| from_cdp(&e, &self.session))?;
            session
                .call_raw(
                    "Overlay.highlightNode",
                    json!({
                        "backendNodeId": backend,
                        "highlightConfig": highlight_config(),
                    }),
                )
                .await
                .map_err(|e| from_cdp(&e, &self.session))?;
            let element = ActedElement {
                reference,
                role: described.role,
                name: described.name,
                tag: described.tag,
            };
            Ok((element, note, session))
        };
        let (element, note, session) =
            Box::pin(self.unless_blocked(&running, &page, mark, work)).await?;
        // A later highlight replaces this one; only the latest hides.
        let generation = running.highlights.fetch_add(1, Ordering::Relaxed) + 1;
        if req.duration_ms > 0 {
            let running = running.clone();
            let duration = Duration::from_millis(req.duration_ms);
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                if running.highlights.load(Ordering::Relaxed) == generation {
                    let _ = session.call_raw("Overlay.hideHighlight", json!({})).await;
                    let _ = session.call_raw("Overlay.disable", json!({})).await;
                    // `coverage` needs `DOM` on until it stops.
                    if !running.coverage_uses(&session) {
                        let _ = session.call(dom::DisableParams {}).await;
                    }
                }
            });
        }
        let named = crate::input::describe(&element);
        let text = if req.duration_ms > 0 {
            format!(
                "highlighted {named} for {}",
                format::duration(Duration::from_millis(req.duration_ms))
            )
        } else {
            format!("highlighted {named} until the next highlight")
        };
        let quiet = element.reference.clone().unwrap_or_default();
        let data = HighlightData {
            tab: tab_name(page.id),
            element,
            duration_ms: req.duration_ms,
        };
        let mut output = Output::typed(&data, text).quiet(quiet);
        if let Some(note) = note {
            output = output.warning(note);
        }
        if headless(&running) {
            output = output.warning(format!(
                "session {} is headless: only screenshots show the highlight (open --headed \
                 --restart for a window)",
                self.session
            ));
        }
        Ok(output)
    }

    /// `inspect`: opens DevTools for the tab, in its own window.
    pub async fn inspect(&self, req: InspectRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        if headless(&running) {
            return Err(ErrorInfo::new(
                ErrorKind::ProtocolError,
                format!(
                    "DevTools needs a window, and session {} is headless",
                    self.session
                ),
            )
            .hint(
                Hint::run(
                    Cmd::new("open")
                        .switch("headed")
                        .switch("restart")
                        .session(&self.session),
                )
                .why("restarts the browser with a window; tabs and their state are lost"),
            ));
        }
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        let opened = running
            .conn
            .browser()
            .with_timeout(timeout)
            .call(cdp_target::OpenDevToolsParams {
                target_id: page.target_id.clone(),
                panel_id: req.panel.clone(),
            })
            .await
            .map_err(|e| from_cdp(&e, &self.session))?;
        let tab = tab_name(page.id);
        let text = match &req.panel {
            Some(panel) => format!("opened DevTools for {tab} on the {panel} panel"),
            None => format!("opened DevTools for {tab}"),
        };
        let data = InspectData {
            tab: tab.clone(),
            panel: req.panel,
            devtools_target: opened.target_id,
        };
        Ok(Output::typed(&data, text).quiet(tab))
    }
}

/// Whether the browser has no window: a launched headless one, or a
/// connected one that says it is headless.
fn headless(running: &Running) -> bool {
    match &running.spec {
        Spec::Launched { headed, .. } => !headed,
        Spec::Connected { .. } => running.info.product.starts_with("HeadlessChrome"),
    }
}

/// DevTools' own colours for the element highlight.
fn highlight_config() -> serde_json::Value {
    json!({
        "showInfo": true,
        "showStyles": true,
        "showAccessibilityInfo": true,
        "contentColor": { "r": 111, "g": 168, "b": 220, "a": 0.66 },
        "paddingColor": { "r": 147, "g": 196, "b": 125, "a": 0.55 },
        "borderColor": { "r": 255, "g": 229, "b": 153, "a": 0.66 },
        "marginColor": { "r": 246, "g": 178, "b": 107, "a": 0.66 },
    })
}

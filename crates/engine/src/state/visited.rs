//! `state save`'s storage of origins the context visited but has no frame
//! open in now (an SSO provider the login went through), read
//! in a page of the context whose
//! request for each origin is answered with an empty document.
//!
//! The page is a hidden target (`Target.createTarget` with `hidden`): no
//! entry in the tab strip, and to the tab manager a target of type
//! `other`, which it resumes but neither lists as a tab nor gives the
//! session's routes, init scripts or emulation. The page intercepts its
//! own requests on a session of ours, so the empty documents reach
//! neither the server nor the session's routes, and no script of the
//! origin runs there.

use std::time::Duration;

use base64::Engine as _;
use cdp::domains::{fetch, network, page, target};
use cdp::{Connection, Session};
use serde_json::Value;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::commands::evaluate;

/// What each of the page's requests is answered with.
const EMPTY_PAGE: &str = "<html></html>";

/// How long to wait between checks that the page's document is the
/// origin's yet.
const COMMIT_POLL: Duration = Duration::from_millis(50);

/// A hidden page of one context, open until [`HiddenPage::close`].
pub(super) struct HiddenPage {
    conn: Connection,
    target_id: String,
    session: Session,
    /// The task that answers the page's requests.
    answers: AbortHandle,
}

impl HiddenPage {
    /// Opens a hidden blank page in Chrome's context `scope` (the default
    /// one with `None`), answering every request of its own.
    pub(super) async fn open(
        conn: &Connection,
        scope: Option<&str>,
        timeout: Duration,
    ) -> cdp::Result<Self> {
        let browser = conn.browser().with_timeout(timeout);
        let mut create = target::CreateTargetParams::new("about:blank".to_owned());
        create.browser_context_id = scope.map(str::to_owned);
        create.hidden = Some(true);
        create.background = Some(true);
        let target_id = browser.call(create).await?.target_id;
        let mut attach = target::AttachToTargetParams::new(target_id.clone());
        attach.flatten = Some(true);
        let attached = browser.call(attach).await;
        let session_id = match attached {
            Ok(attached) => attached.session_id,
            Err(e) => {
                let _ = browser
                    .call(target::CloseTargetParams::new(target_id))
                    .await;
                return Err(e);
            }
        };
        let session = conn.session(session_id).with_timeout(timeout);
        // Listening before `Fetch.enable`, so no paused request is missed.
        let mut events = session.events_lossless("Fetch.");
        let answerer = session.clone();
        let answers = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                if event.method != "Fetch.requestPaused" {
                    continue;
                }
                let Some(id) = event.params["requestId"].as_str() else {
                    continue;
                };
                if let Err(e) = answerer.call(empty_page(id)).await {
                    tracing::debug!("cannot answer the hidden page's request: {e}");
                }
            }
        })
        .abort_handle();
        let page = Self {
            conn: conn.clone(),
            target_id,
            session,
            answers,
        };
        let enable = fetch::EnableParams {
            patterns: Some(vec![fetch::RequestPattern {
                url_pattern: Some("*".into()),
                resource_type: None,
                request_stage: Some(fetch::RequestStage::Request),
            }]),
            handle_auth_requests: None,
        };
        // A service worker of the origin would answer the page's request
        // before `Fetch` sees it, with the app itself.
        let bypass = network::SetBypassServiceWorkerParams::new(true);
        let ready =
            futures_util::future::try_join(page.session.call(enable), page.session.call(bypass))
                .await;
        if let Err(e) = ready {
            page.close(timeout).await;
            return Err(e);
        }
        Ok(page)
    }

    /// Loads `origin`'s empty document and evaluates `expression` there;
    /// returns its value.
    pub(super) async fn run_in(
        &self,
        origin: &str,
        expression: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        let navigated = self
            .session
            .call(page::NavigateParams::new(format!("{origin}/")))
            .await
            .map_err(|e| e.to_string())?;
        if let Some(error) = navigated.error_text {
            return Err(error);
        }
        // `Page.navigate` answers once the document committed; a script
        // sent right then can still reach the one before it.
        loop {
            let here = evaluate(&self.session, None, "location.origin".to_owned(), timeout).await?;
            if here.as_str() == Some(origin) {
                break;
            }
            if Instant::now() + COMMIT_POLL > deadline {
                return Err(format!("the page did not load {origin}"));
            }
            tokio::time::sleep(COMMIT_POLL).await;
        }
        evaluate(&self.session, None, expression.to_owned(), timeout).await
    }

    /// Closes the page.
    pub(super) async fn close(self, timeout: Duration) {
        self.answers.abort();
        let browser = self.conn.browser().with_timeout(timeout);
        if let Err(e) = browser
            .call(target::CloseTargetParams::new(self.target_id))
            .await
        {
            tracing::debug!("cannot close the hidden page: {e}");
        }
    }
}

/// `Fetch.fulfillRequest` with [`EMPTY_PAGE`].
fn empty_page(request_id: &str) -> fetch::FulfillRequestParams {
    let mut fulfill = fetch::FulfillRequestParams::new(request_id.to_owned(), 200);
    fulfill.response_headers = Some(vec![fetch::HeaderEntry {
        name: "Content-Type".to_owned(),
        value: "text/html".to_owned(),
    }]);
    fulfill.body = Some(base64::engine::general_purpose::STANDARD.encode(EMPTY_PAGE));
    fulfill
}

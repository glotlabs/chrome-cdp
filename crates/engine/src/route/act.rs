//! Carrying a routing decision out on the paused request's session.
//!
//! Every paused request gets exactly one answer: `Fetch.fulfillRequest`,
//! `Fetch.failRequest` or `Fetch.continueRequest`. A call that fails means
//! the request or its target went away meanwhile (a navigation, a closed
//! tab); that is logged and nothing else.
//!
//! `--patch-json` needs the real response: the request goes on with
//! `interceptResponse`, so Chrome pauses it again once the response headers
//! are in (the response stage); [`patch_response`] then reads the body,
//! merges the patch in and answers with the result.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use cdp::domains::fetch;
use cdp::{RawEvent, Session};
use serde_json::Value;

use super::{Act, Decision, Fulfill, Handler, Router, header, merge_patch};

/// Requests of one session whose response is to be patched, by request
/// id, with their journal entry.
#[derive(Debug, Default)]
pub(crate) struct PendingPatches(HashMap<String, (Arc<Handler>, u64)>);

impl PendingPatches {
    /// Remembers to patch the response of the request `event` paused, if
    /// `decision` says so. Call before carrying the decision out.
    pub(crate) fn expect(&mut self, event: &RawEvent, decision: &Decision) {
        if let Decision::Route { handler, seq, .. } | Decision::Mock { handler, seq } = decision
            && matches!(handler.act, Act::PatchJson(_))
            && let Some(id) = event.params["requestId"].as_str()
        {
            if self.0.len() > 1024 {
                // Requests that never came back (their target went away).
                self.0.clear();
            }
            self.0.insert(id.to_owned(), (handler.clone(), *seq));
        }
    }

    /// The patch waiting for request `id`'s response, if any.
    pub(crate) fn take(&mut self, id: &str) -> Option<(Arc<Handler>, u64)> {
        self.0.remove(id)
    }
}

/// Lets request `request_id` go on unchanged; the call is sent before this
/// returns.
pub(crate) fn continue_paused(
    session: &Session,
    request_id: &str,
) -> impl Future<Output = ()> + Send + 'static + use<> {
    let call = session.call(fetch::ContinueRequestParams::new(request_id.to_owned()));
    async move {
        if let Err(e) = call.await {
            tracing::debug!("cannot continue a request: {e}");
        }
    }
}

/// Carries `decision` out for the request `event` paused (at the request
/// stage). `Pass` is answered at once; the rest after the rule's delay.
pub(crate) fn carry_out(
    session: &Session,
    event: Arc<RawEvent>,
    decision: Decision,
) -> impl Future<Output = ()> + Send + 'static + use<> {
    let params = &event.params;
    let request_id = params["requestId"].as_str().unwrap_or_default().to_owned();
    let pass = matches!(decision, Decision::Pass).then(|| continue_paused(session, &request_id));
    let session = session.clone();
    async move {
        if let Some(pass) = pass {
            return pass.await;
        }
        let request = &event.params["request"];
        let origin = header(&request["headers"], "Origin").map(str::to_owned);
        let result = match decision {
            Decision::Pass => Ok(()),
            Decision::Preflight => {
                let headers = preflight_headers(&request["headers"], origin.as_deref());
                let mut params = fetch::FulfillRequestParams::new(request_id, 204);
                params.response_headers = Some(entries(headers));
                session.call(params).await.map(drop)
            }
            Decision::Route { handler, .. } | Decision::Mock { handler, .. } => {
                if !handler.delay.is_zero() {
                    tokio::time::sleep(handler.delay).await;
                }
                match &handler.act {
                    Act::Fulfill(fulfill) => {
                        let params = fulfill_params(request_id, fulfill, origin.as_deref());
                        session.call(params).await.map(drop)
                    }
                    Act::Abort(reason) => session
                        .call(fetch::FailRequestParams::new(request_id, *reason))
                        .await
                        .map(drop),
                    Act::Continue {
                        set_headers,
                        remove_headers,
                        url,
                    } => {
                        let mut params = fetch::ContinueRequestParams::new(request_id);
                        if !set_headers.is_empty() || !remove_headers.is_empty() {
                            params.headers = Some(changed_headers(
                                &request["headers"],
                                set_headers,
                                remove_headers,
                            ));
                        }
                        params.url.clone_from(url);
                        session.call(params).await.map(drop)
                    }
                    Act::PatchJson(_) => {
                        let mut params = fetch::ContinueRequestParams::new(request_id);
                        params.intercept_response = Some(true);
                        session.call(params).await.map(drop)
                    }
                }
            }
        };
        if let Err(e) = result {
            tracing::debug!("cannot answer a routed request: {e}");
        }
    }
}

/// Answers the response stage of a request `--patch-json` sent on: its
/// JSON body with the patch merged in. A response that is no JSON (or has
/// no body, or is a redirect) goes on unchanged, and the journal says so.
pub(crate) async fn patch_response(
    router: &Router,
    session: &Session,
    event: &RawEvent,
    handler: &Handler,
    seq: u64,
) {
    let params = &event.params;
    let request_id = params["requestId"].as_str().unwrap_or_default().to_owned();
    let Act::PatchJson(patch) = &handler.act else {
        return continue_paused(session, &request_id).await;
    };
    let status = params["responseStatusCode"].as_u64().unwrap_or(0);
    let skip = |why: &str| {
        router.amend(seq, why);
        continue_paused(session, &request_id)
    };
    if params.get("responseErrorReason").is_some() {
        return skip("failed; not patched").await;
    }
    if (300..400).contains(&status) {
        return skip("redirect; not patched").await;
    }
    let body = match session
        .call(fetch::GetResponseBodyParams::new(request_id.clone()))
        .await
    {
        Ok(body) => body,
        Err(e) => {
            tracing::debug!("cannot read a response to patch: {e}");
            return skip("no body; not patched").await;
        }
    };
    let bytes = if body.base64_encoded {
        base64::engine::general_purpose::STANDARD
            .decode(body.body.as_bytes())
            .unwrap_or_default()
    } else {
        body.body.into_bytes()
    };
    let Ok(mut json) = serde_json::from_slice::<Value>(&bytes) else {
        return skip("not JSON; not patched").await;
    };
    merge_patch(&mut json, patch);
    let patched = serde_json::to_vec(&json).unwrap_or_default();
    // The body is now plain and of another length.
    let headers: Vec<fetch::HeaderEntry> = params["responseHeaders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let name = h["name"].as_str()?;
            let value = h["value"].as_str()?;
            let dropped = name.eq_ignore_ascii_case("Content-Length")
                || name.eq_ignore_ascii_case("Content-Encoding");
            (!dropped).then(|| fetch::HeaderEntry {
                name: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .collect();
    let mut fulfill =
        fetch::FulfillRequestParams::new(request_id, i64::try_from(status).unwrap_or(200));
    fulfill.response_headers = Some(headers);
    fulfill.body = Some(base64::engine::general_purpose::STANDARD.encode(&patched));
    fulfill.response_phrase = params["responseStatusText"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Err(e) = session.call(fulfill).await {
        tracing::debug!("cannot answer with a patched response: {e}");
    }
}

/// `Fetch.fulfillRequest` for a rule's or mock's response. A cross-origin
/// request (it has an `Origin`) is allowed by CORS unless the answer's
/// headers say otherwise (or its mock set has `cors = "off"`): the mocked
/// server is the rule, not a server that could.
fn fulfill_params(
    request_id: String,
    fulfill: &Fulfill,
    origin: Option<&str>,
) -> fetch::FulfillRequestParams {
    let mut headers = fulfill.headers.clone();
    if let Some(origin) = origin
        && fulfill.cors
        && fulfill.header("Access-Control-Allow-Origin").is_none()
    {
        headers.extend(allow_origin(origin));
        // The page may read the headers the answer was made with.
        let exposed: Vec<&str> = fulfill
            .headers
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| {
                !n.eq_ignore_ascii_case("Content-Type") && !n.eq_ignore_ascii_case("Set-Cookie")
            })
            .collect();
        if !exposed.is_empty() && fulfill.header("Access-Control-Expose-Headers").is_none() {
            headers.push(("Access-Control-Expose-Headers".into(), exposed.join(", ")));
        }
    }
    let mut params = fetch::FulfillRequestParams::new(request_id, i64::from(fulfill.status));
    params.response_headers = Some(entries(headers));
    if fulfill.size > 0 {
        params.body = Some(fulfill.body.clone());
    }
    params
}

/// The headers that allow `origin`'s requests, with credentials.
fn allow_origin(origin: &str) -> [(String, String); 3] {
    [
        ("Access-Control-Allow-Origin".into(), origin.to_owned()),
        ("Access-Control-Allow-Credentials".into(), "true".into()),
        ("Vary".into(), "Origin".into()),
    ]
}

/// The answer to a CORS preflight: whatever it asks for is allowed.
fn preflight_headers(request: &Value, origin: Option<&str>) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = match origin {
        Some(origin) => allow_origin(origin).into(),
        None => vec![("Access-Control-Allow-Origin".into(), "*".into())],
    };
    if let Some(method) = header(request, "Access-Control-Request-Method") {
        headers.push(("Access-Control-Allow-Methods".into(), method.to_owned()));
    }
    if let Some(asked) = header(request, "Access-Control-Request-Headers") {
        headers.push(("Access-Control-Allow-Headers".into(), asked.to_owned()));
    }
    headers
}

fn entries(headers: Vec<(String, String)>) -> Vec<fetch::HeaderEntry> {
    headers
        .into_iter()
        .map(|(name, value)| fetch::HeaderEntry { name, value })
        .collect()
}

/// The request's headers with `set` set (replacing any of the same name)
/// and `remove` (lower case) removed.
fn changed_headers(
    request: &Value,
    set: &[(String, String)],
    remove: &[String],
) -> Vec<fetch::HeaderEntry> {
    let mut headers: Vec<fetch::HeaderEntry> = request
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            !remove.contains(&name) && !set.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name))
        })
        .filter_map(|(name, value)| {
            Some(fetch::HeaderEntry {
                name: name.clone(),
                value: value.as_str()?.to_owned(),
            })
        })
        .collect();
    headers.extend(set.iter().map(|(name, value)| fetch::HeaderEntry {
        name: name.clone(),
        value: value.clone(),
    }));
    headers
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn changing_headers() {
        let request = json!({"Accept": "*/*", "X-Old": "1", "cookie": "a=1", "x-set": "old"});
        let headers = changed_headers(
            &request,
            &[("X-Set".into(), "new".into())],
            &["x-old".into(), "cookie".into()],
        );
        let pairs: Vec<(&str, &str)> = headers
            .iter()
            .map(|h| (h.name.as_str(), h.value.as_str()))
            .collect();
        assert_eq!(pairs, [("Accept", "*/*"), ("X-Set", "new")]);
    }

    #[test]
    fn preflights_allow_what_they_ask_for() {
        let request = json!({
            "Origin": "http://a.test",
            "Access-Control-Request-Method": "PUT",
            "Access-Control-Request-Headers": "content-type,x-token",
        });
        let headers = preflight_headers(&request, Some("http://a.test"));
        let find = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(find("Access-Control-Allow-Origin"), Some("http://a.test"));
        assert_eq!(find("Access-Control-Allow-Methods"), Some("PUT"));
        assert_eq!(
            find("Access-Control-Allow-Headers"),
            Some("content-type,x-token")
        );
    }
}

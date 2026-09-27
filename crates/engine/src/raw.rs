//! `cdp <Domain.method> [json]`: one raw CDP call on the session's
//! browser or current page, the escape hatch (overview principle 6).

use cdp::CdpError;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde_json::Value;

use crate::Engine;
use crate::errors::from_cdp;
use crate::frames::World;
use crate::request::{CdpRequest, Target, millis};
use crate::tabs::FrameContext;

impl Engine {
    /// Makes the call and returns Chrome's result object. A page call holds
    /// the page's lock.
    pub async fn cdp(&self, req: CdpRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let CdpRequest {
            method,
            params,
            target,
            tab,
            frame,
            max_chars,
            ..
        } = req;
        if frame.is_some() && target == Target::Browser {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "--frame needs a page call, not a browser one",
            )
            .hint(Hint::run(
                Cmd::new("cdp")
                    .session(&self.session)
                    .flag("target", "page")
                    .raw("--frame … Runtime.evaluate …"),
            )));
        }
        let mark = running.tabs.mark();
        let params = Value::Object(params);
        let result = match target {
            Target::Browser => {
                running
                    .conn
                    .browser()
                    .with_timeout(timeout)
                    .call_raw(&method, params)
                    .await
            }
            Target::Page => {
                let page = self.page(&running, tab.as_deref(), timeout).await?;
                let _tab = self.tabs.lock(&page.target_id).await;
                match frame.as_deref() {
                    Some(frame) => {
                        let call = |context: &FrameContext| {
                            let params = in_context(&method, params.clone(), context.context_id);
                            let session = context.session.with_timeout(timeout);
                            session.call_raw(&method, params)
                        };
                        let frame = Some(frame);
                        self.in_frame(&running, &page, frame, World::Main, timeout, call)
                            .await?
                    }
                    None => {
                        page.session
                            .with_timeout(timeout)
                            .call_raw(&method, params)
                            .await
                    }
                }
            }
        };
        let value = result.map_err(|e| self.cdp_error(&e, &method, target))?;
        let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        let compact = value.to_string();
        let output = Output::new(value, pretty)
            .quiet(compact)
            .limit(max_chars, "--max-chars 0 for all of it");
        Ok(self.report_opened(&running, mark, output).await)
    }

    fn cdp_error(&self, e: &CdpError, method: &str, target: Target) -> ErrorInfo {
        let info = from_cdp(e, &self.session);
        let browser_domain = ["Browser.", "Target."]
            .iter()
            .any(|d| method.starts_with(d));
        match e {
            CdpError::Protocol { code: -32601, .. }
                if target == Target::Browser && !browser_domain =>
            {
                info.hint(format!(
                    "if {m} is a page method, send it to a page: {cmd}",
                    m = method,
                    cmd = Cmd::new("cdp")
                        .session(&self.session)
                        .flag("target", "page")
                        .arg(method)
                        .raw("…")
                ))
            }
            _ => info,
        }
    }
}

/// Adds the frame's main-world context to a `Runtime` call that runs
/// script and names no context or object of its own.
fn in_context(method: &str, mut params: Value, context_id: i64) -> Value {
    let key = match method {
        "Runtime.evaluate" => "contextId",
        "Runtime.callFunctionOn" | "Runtime.compileScript" | "Runtime.globalLexicalScopeNames" => {
            "executionContextId"
        }
        _ => return params,
    };
    let own = [
        "contextId",
        "executionContextId",
        "uniqueContextId",
        "objectId",
    ];
    if let Value::Object(map) = &mut params
        && !own.iter().any(|k| map.contains_key(*k))
    {
        map.insert(key.to_owned(), context_id.into());
    }
    params
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::in_context;

    #[test]
    fn frame_calls_get_its_context_unless_they_name_one() {
        let eval = in_context("Runtime.evaluate", json!({ "expression": "1" }), 7);
        assert_eq!(eval, json!({ "expression": "1", "contextId": 7 }));
        let call = json!({ "functionDeclaration": "f" });
        let call = in_context("Runtime.callFunctionOn", call, 7);
        assert_eq!(call["executionContextId"], json!(7));
        let own = json!({ "expression": "1", "uniqueContextId": "u" });
        assert_eq!(in_context("Runtime.evaluate", own.clone(), 7), own);
        let bound = json!({ "functionDeclaration": "f", "objectId": "o" });
        assert_eq!(
            in_context("Runtime.callFunctionOn", bound.clone(), 7),
            bound
        );
        assert_eq!(in_context("Page.reload", json!({}), 7), json!({}));
    }
}

//! `cdp <Domain.method> [json]`: one raw CDP call on the session's
//! browser or current page, the escape hatch (overview principle 6).

use cdp::CdpError;
use protocol::{Cmd, ErrorInfo, Output};
use serde_json::Value;

use crate::Engine;
use crate::errors::from_cdp;
use crate::request::{CdpRequest, Target, millis};

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
            max_chars,
            ..
        } = req;
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
                let page = self.page(&running).await?;
                let _tab = self.tabs.lock(&page.target_id).await;
                page.session
                    .with_timeout(timeout)
                    .call_raw(&method, params)
                    .await
            }
        };
        let value = result.map_err(|e| self.cdp_error(&e, &method, target))?;
        let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        let compact = value.to_string();
        Ok(Output::new(value, pretty)
            .quiet(compact)
            .limit(max_chars, "--max-chars 0 for all of it"))
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

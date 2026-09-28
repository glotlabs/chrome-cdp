//! Stable locators of refs (plan 11, milestone 11.8): refs live only as
//! long as their session, so `history --as-scenario` writes the most
//! robust locator that finds the ref's element alone (the bundle's
//! `stable`, `injected/src/stable.ts`). The daemon asks for them before a
//! command that names refs runs, while the elements are still there.

use std::time::Duration;

use protocol::schema::RefLocator;
use serde::Deserialize;

use super::LocateOptions;
use crate::Engine;
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
    /// if `None`), or why there is none.
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
            let called = self
                .call_on_locator(tab, reference, &options, &function, timeout)
                .await;
            let found = match called {
                Ok(call) => call
                    .value
                    .as_str()
                    .and_then(|s| serde_json::from_str::<Found>(s).ok())
                    .unwrap_or(Found {
                        locator: None,
                        how: None,
                        reason: Some("the page did not say".to_owned()),
                    }),
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
}

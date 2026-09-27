//! chrome-cdp's injected helper bundle, `__ccdp` (plan 04, milestone 4.4).
//!
//! The bundle is TypeScript in `injected/src/`, built by `cargo xtask
//! build-js` into `injected/ccdp.js`, which is committed so a plain
//! `cargo build` needs no Node. Every tab loads it into chrome-cdp's
//! isolated world of each new document (`Page.addScriptToEvaluateOnNewDocument`
//! with the world's name, see `tabs::setup`), and an isolated world made
//! later on demand gets it at once. It holds the locator engine and the
//! snapshot's DOM walker.
//!
//! Functions that use it start with [`GUARD`]; [`call`] loads the bundle
//! and calls again when the guard says it is missing.

use cdp::domains::runtime;
use cdp::{CdpError, Session};

/// The built bundle.
pub(crate) const BUNDLE: &str = include_str!("injected/ccdp.js");

/// The first statement of a function that uses `__ccdp`.
pub(crate) const GUARD: &str = "if (!globalThis.__ccdp) throw new Error('__ccdp is not loaded');";

/// Calls a function that starts with [`GUARD`] in context `context_id`.
/// When the bundle is not there (a world made before chrome-cdp attached
/// could miss it), loads it and calls again.
pub(crate) async fn call(
    session: &Session,
    context_id: runtime::ExecutionContextId,
    params: runtime::CallFunctionOnParams,
) -> cdp::Result<runtime::CallFunctionOnReturns> {
    let mut params = params;
    params.execution_context_id = Some(context_id);
    let result = session.call(params.clone()).await?;
    if !missing(&result) {
        return Ok(result);
    }
    load(session, context_id).await?;
    session.call(params).await
}

/// Loads the bundle into context `context_id`.
pub(crate) async fn load(
    session: &Session,
    context_id: runtime::ExecutionContextId,
) -> cdp::Result<()> {
    let mut params = runtime::EvaluateParams::new(BUNDLE.to_owned());
    params.context_id = Some(context_id);
    let result = session.call(params).await?;
    match result.exception_details {
        None => Ok(()),
        Some(e) => Err(CdpError::Protocol {
            method: "Runtime.evaluate".to_owned(),
            code: -1,
            message: format!("loading chrome-cdp's helper failed: {}", e.text),
            data: None,
        }),
    }
}

/// The own properties of object `object`.
pub(crate) async fn own_properties(
    session: &cdp::Session,
    object: &str,
) -> cdp::Result<Vec<runtime::PropertyDescriptor>> {
    let mut params = runtime::GetPropertiesParams::new(object.to_owned());
    params.own_properties = Some(true);
    Ok(session.call(params).await?.result)
}

fn missing(result: &runtime::CallFunctionOnReturns) -> bool {
    result.exception_details.as_ref().is_some_and(|e| {
        e.exception
            .as_ref()
            .and_then(|x| x.description.as_deref())
            .unwrap_or(&e.text)
            .contains("__ccdp is not loaded")
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::BUNDLE;

    /// The hash `cargo xtask build-js` puts on the bundle's first line:
    /// FNV-1a over the sources' names and bytes.
    fn sources_hash(dir: &Path) -> u64 {
        let mut files: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "ts"))
            .collect();
        files.sort();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = fs::read(&file).unwrap();
            for byte in name.bytes().chain([0]).chain(bytes).chain([0]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    #[test]
    fn the_bundle_is_built_from_the_current_sources() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/injected/src");
        let first = BUNDLE.lines().next().unwrap();
        let recorded = first.rsplit("sources: ").next().unwrap();
        assert_eq!(
            recorded,
            format!("{:016x}", sources_hash(&dir)),
            "src/injected/ccdp.js is out of date: run `cargo xtask build-js`"
        );
    }
}

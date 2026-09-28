//! Unpacked extensions for `open --extension` (plan 07, milestone 7.5).
//!
//! They are loaded over the pipe with `Extensions.loadUnpacked` right after
//! the launch, before any URL, rather than with `--load-extension`: Google
//! Chrome 137 and later ignore that flag, and a broken manifest there is a
//! silent no-op, where `loadUnpacked` says what is wrong.

use std::path::{Path, PathBuf};

use cdp::domains::extensions as cdp_extensions;
use cdp::{CdpError, Connection};
use protocol::schema::ExtensionInfo;
use protocol::{ErrorInfo, ErrorKind, Hint};

use crate::errors::from_cdp;

/// Loads each extension in `dirs` (absolute paths) into the browser behind
/// `conn`, in order, and describes them.
pub(crate) async fn load(
    conn: &Connection,
    dirs: &[PathBuf],
    session: &str,
) -> Result<Vec<ExtensionInfo>, ErrorInfo> {
    let browser = conn.browser();
    let mut ids = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let loaded = browser
            .call(cdp_extensions::LoadUnpackedParams::new(
                dir.display().to_string(),
            ))
            .await
            .map_err(|e| load_failed(dir, &e, session))?;
        ids.push(loaded.id);
    }
    // The name and version as Chrome read them (a localized name resolved).
    let known = browser
        .call(cdp_extensions::GetExtensionsParams::default())
        .await
        .map(|r| r.extensions)
        .unwrap_or_default();
    Ok(ids
        .into_iter()
        .zip(dirs)
        .map(|(id, dir)| {
            let found = known.iter().find(|e| e.id == id);
            ExtensionInfo {
                name: found.map(|e| e.name.clone()).unwrap_or_default(),
                version: found.map(|e| e.version.clone()).unwrap_or_default(),
                path: dir.display().to_string(),
                id,
            }
        })
        .collect())
}

fn load_failed(dir: &Path, e: &CdpError, session: &str) -> ErrorInfo {
    match e {
        CdpError::Protocol { code: -32601, .. } => ErrorInfo::new(
            ErrorKind::LaunchFailed,
            "this browser cannot load extensions over CDP (Extensions.loadUnpacked is missing)",
        )
        .hint(Hint::text(
            "use Chrome 126 or later, e.g. Chrome for Testing from `chrome-cdp install`",
        )),
        CdpError::Protocol { message, .. } => ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot load the extension in {}: {message}", dir.display()),
        )
        .hint(Hint::text(
            "fix what Chrome names in the extension's files, then open again",
        )),
        _ => from_cdp(e, session),
    }
}

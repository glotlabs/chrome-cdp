//! `chrome-cdp --version`.

use serde_json::json;

use crate::output::Success;

/// The crate version.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The git commit the binary was built from (see `build.rs`).
pub(crate) const GIT_SHA: &str = env!("CHROME_CDP_GIT_SHA");

/// The version, git commit and vendored CDP protocol revision.
pub(crate) fn run() -> Success {
    let protocol = cdp::domains::PROTOCOL_REVISION;
    Success::new(
        json!({
            "version": VERSION,
            "git_sha": GIT_SHA,
            "protocol_revision": protocol,
        }),
        format!("chrome-cdp {VERSION} (git {GIT_SHA}, CDP protocol r{protocol})"),
    )
    .quiet(VERSION)
}

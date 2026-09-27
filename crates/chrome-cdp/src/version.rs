//! `chrome-cdp --version`.

use protocol::Output;
use protocol::schema::VersionData;

/// The crate version.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The git commit the binary was built from (see `build.rs`).
pub(crate) const GIT_SHA: &str = env!("CHROME_CDP_GIT_SHA");

/// The version, git commit and vendored CDP protocol revision.
pub(crate) fn run() -> Output {
    let protocol = cdp::domains::PROTOCOL_REVISION;
    let data = VersionData {
        version: VERSION.into(),
        git_sha: GIT_SHA.into(),
        protocol_revision: protocol,
    };
    Output::typed(
        &data,
        format!("chrome-cdp {VERSION} (git {GIT_SHA}, CDP protocol r{protocol})"),
    )
    .quiet(VERSION)
}

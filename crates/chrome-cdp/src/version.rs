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

/// Identifies this exact binary to the session daemon: the version, the
/// git commit, and the binary's modification time, so a rebuilt or
/// upgraded binary replaces a daemon started by the old one.
pub(crate) fn build_id() -> String {
    let modified = std::env::current_exe()
        .and_then(|exe| exe.metadata())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("{VERSION}+{GIT_SHA}+{modified}")
}

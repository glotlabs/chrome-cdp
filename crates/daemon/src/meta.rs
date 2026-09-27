//! `meta.json`: what a session runs, for `status`, `sessions`, reaping a
//! dead daemon's Chrome, and relaunching after an upgrade.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use engine::OpenRequest;
use protocol::schema::BrowserInfo;
use serde::{Deserialize, Serialize};

/// The contents of `meta.json`. The daemon rewrites it whenever its browser
/// changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub session: String,
    /// [`crate::paths::SessionPaths::home`], which tells sessions with
    /// shortened dirs apart.
    pub home: PathBuf,
    /// The build of the daemon (see [`crate::client`]).
    pub build: String,
    pub daemon_pid: u32,
    /// When the daemon started, in seconds since the Unix epoch.
    pub started_at: u64,
    pub idle_timeout_ms: u64,
    /// The browser, while there is one.
    #[serde(default)]
    pub browser: Option<BrowserInfo>,
    /// What the running browser was opened with (no `chrome`, no `url`),
    /// to open the same after an upgrade replaced the daemon.
    #[serde(default)]
    pub launch: Option<OpenRequest>,
    /// What `connect` attached to, for the same reason.
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl Meta {
    /// Reads `path`; `None` if it is missing or unreadable.
    pub fn read(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes `path` atomically (a temp file renamed over it), mode 0600.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Meta;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        assert!(Meta::read(&path).is_none());
        let meta = Meta {
            session: "s".into(),
            home: PathBuf::from("/run/user/1/chrome-cdp/s"),
            build: "0.1.0+abc".into(),
            daemon_pid: 42,
            started_at: 1,
            idle_timeout_ms: 1_800_000,
            browser: None,
            launch: Some(engine::OpenRequest {
                headed: Some(true),
                timeout_ms: 1000,
                ..Default::default()
            }),
            endpoint: None,
        };
        meta.write(&path).unwrap();
        assert_eq!(Meta::read(&path), Some(meta));
        std::fs::write(&path, "{").unwrap();
        assert!(Meta::read(&path).is_none());
    }
}

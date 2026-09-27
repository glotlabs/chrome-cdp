//! Downloads (plan 03, milestone 3.5).
//!
//! The browser saves every download into the session's `downloads`
//! directory (`Browser.setDownloadBehavior`, set when the tabs manager
//! starts). Chrome names each file by its guid while it downloads
//! (`allowAndName`); once it completes, it is renamed to the name the
//! server suggested, made unique in the directory (`report (1).pdf`), so
//! a finished download's path never changes and a half-written file never
//! has the final name.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cdp::RawEvent;
use cdp::domains::browser;
use protocol::schema::DownloadInfo;
use tokio::time::Instant;

use super::{Check, Shared, State, Tabs, decode, tab_name};

/// How many downloads a session remembers.
const KEPT: usize = 256;

/// One download of the session.
#[derive(Debug, Clone)]
pub(super) struct Download {
    guid: String,
    /// When it began, in the tabs' event sequence.
    pub(super) seq: u64,
    tab: Option<u32>,
    url: String,
    /// The server's name for it, made safe as a file name.
    file: String,
    /// Where it is, once it completed.
    path: Option<PathBuf>,
    state: DownloadState,
    received: u64,
    total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadState {
    InProgress,
    Completed,
    Canceled,
}

impl Download {
    fn done(&self) -> bool {
        self.state != DownloadState::InProgress
    }

    pub(super) fn info(&self) -> DownloadInfo {
        DownloadInfo {
            tab: self.tab.map(tab_name),
            url: self.url.clone(),
            file: self.file.clone(),
            path: self.path.as_ref().map(|p| p.display().to_string()),
            state: match self.state {
                DownloadState::InProgress => "in_progress",
                DownloadState::Completed => "completed",
                DownloadState::Canceled => "canceled",
            }
            .to_owned(),
            received_bytes: self.received,
            total_bytes: self.total,
        }
    }
}

/// `Browser.downloadWillBegin`.
pub(super) fn will_begin(shared: &Shared, event: &RawEvent) {
    let begin = match decode::<browser::DownloadWillBegin>(event) {
        Ok(begin) => begin,
        Err(e) => {
            tracing::warn!("cannot decode Browser.downloadWillBegin: {e}");
            return;
        }
    };
    let mut state = shared.lock();
    let tab = state
        .tabs
        .iter()
        .find(|(_, t)| t.frames.owner(&begin.frame_id).is_some())
        .map(|(&id, _)| id);
    state.seq += 1;
    let seq = state.seq;
    state.downloads.push(Download {
        guid: begin.guid,
        seq,
        tab,
        url: begin.url,
        file: safe_name(&begin.suggested_filename),
        path: None,
        state: DownloadState::InProgress,
        received: 0,
        total: None,
    });
    if state.downloads.len() > KEPT {
        state.downloads.remove(0);
    }
    drop(state);
    shared.bump();
}

/// `Browser.downloadProgress`: counts bytes, and moves a completed file to
/// its name.
pub(super) fn progress(shared: &Shared, event: &RawEvent) {
    let progress = match decode::<browser::DownloadProgress>(event) {
        Ok(progress) => progress,
        Err(e) => {
            tracing::warn!("cannot decode Browser.downloadProgress: {e}");
            return;
        }
    };
    let dir = shared.settings.downloads_dir.clone();
    let mut state = shared.lock();
    let Some(download) = state.downloads.iter_mut().find(|d| d.guid == progress.guid) else {
        return;
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bytes = |n: f64| n.max(0.0) as u64;
    download.received = bytes(progress.received_bytes);
    download.total = Some(bytes(progress.total_bytes)).filter(|&n| n > 0);
    match progress.state {
        browser::DownloadProgressState::Completed => {
            let saved = progress
                .file_path
                .map(PathBuf::from)
                .or_else(|| dir.as_ref().map(|d| d.join(&download.guid)));
            download.path = saved.map(|saved| match &dir {
                Some(dir) => move_to_name(&saved, dir, &download.file),
                None => saved,
            });
            download.state = DownloadState::Completed;
        }
        browser::DownloadProgressState::Canceled => download.state = DownloadState::Canceled,
        _ => {}
    }
    drop(state);
    shared.bump();
}

/// Renames the file Chrome saved as `saved` to `name` in `dir` (unique
/// there); keeps `saved` if that fails.
fn move_to_name(saved: &Path, dir: &Path, name: &str) -> PathBuf {
    let target = unique_path(dir, name);
    match std::fs::rename(saved, &target) {
        Ok(()) => target,
        Err(e) => {
            tracing::warn!(
                "cannot rename {} to {}: {e}",
                saved.display(),
                target.display()
            );
            saved.to_owned()
        }
    }
}

/// `dir/name`, or `dir/name (1).ext`, … if that exists.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (1..=10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// A server's file name as a plain file name: no directories, no control
/// characters, not hidden, not empty.
fn safe_name(suggested: &str) -> String {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = last
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_start_matches('.');
    if clean.is_empty() {
        "download".to_owned()
    } else {
        clean.to_owned()
    }
}

impl State {
    /// The downloads that began after event mark `mark`.
    pub(super) fn downloads_since(&self, mark: u64) -> impl Iterator<Item = &Download> {
        self.downloads.iter().filter(move |d| d.seq > mark)
    }
}

impl Tabs {
    /// Where the session's downloads go, if the session manages them.
    pub(crate) fn downloads_dir(&self) -> Option<PathBuf> {
        self.shared.settings.downloads_dir.clone()
    }

    /// Every download the session remembers, oldest first.
    pub(crate) fn downloads(&self) -> Vec<DownloadInfo> {
        self.shared
            .lock()
            .downloads
            .iter()
            .map(Download::info)
            .collect()
    }

    /// Waits until a download that began after event mark `after` (and
    /// whose URL is `url`, if given) completed or was canceled, up to
    /// `deadline`. On timeout, returns the newest such download still in
    /// progress, if one began.
    pub(crate) async fn download_done(
        &self,
        after: u64,
        url: Option<&str>,
        deadline: Instant,
    ) -> Result<DownloadInfo, Option<Box<DownloadInfo>>> {
        let wanted = |d: &&Download| url.is_none_or(|u| d.url == u);
        let found = self
            .watch(deadline, |state, _| {
                let mut begun = state.downloads_since(after).filter(wanted).peekable();
                if begun.peek().is_none() {
                    return Check::Later(None);
                }
                match begun.find(|d| d.done()) {
                    Some(done) => Check::Done(done.info()),
                    None => Check::Later(None),
                }
            })
            .await;
        found.ok_or_else(|| {
            let state = self.shared.lock();
            state
                .downloads_since(after)
                .filter(wanted)
                .last()
                .map(|d| Box::new(d.info()))
        })
    }

    /// Whether a download began after event mark `after`, waiting up to
    /// `wait` for one.
    pub(crate) async fn download_began(&self, after: u64, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        self.watch(deadline, |state, _| {
            if state.downloads_since(after).next().is_some() {
                Check::Done(())
            } else {
                Check::Later(None)
            }
        })
        .await
        .is_some()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{safe_name, unique_path};

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_name("report.pdf"), "report.pdf");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("a\\b.txt"), "b.txt");
        assert_eq!(safe_name(".bashrc"), "bashrc");
        assert_eq!(safe_name("tab\there"), "tab_here");
        assert_eq!(safe_name(""), "download");
        assert_eq!(safe_name(".."), "download");
    }

    #[test]
    fn names_are_unique_in_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        assert_eq!(unique_path(dir.path(), "a.txt"), path("a.txt"));
        std::fs::write(path("a.txt"), "").unwrap();
        assert_eq!(unique_path(dir.path(), "a.txt"), path("a (1).txt"));
        std::fs::write(path("a (1).txt"), "").unwrap();
        assert_eq!(unique_path(dir.path(), "a.txt"), path("a (2).txt"));
        std::fs::write(path("README"), "").unwrap();
        assert_eq!(unique_path(dir.path(), "README"), path("README (1)"));
    }
}

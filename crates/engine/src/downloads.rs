//! `downloads` and how commands report downloads (plan 03, milestone 3.5;
//! `--clear`, plan 14, milestone 14.3).
//! The tab manager tracks them (see [`crate::tabs`]'s `downloads`).

use protocol::schema::{DownloadInfo, DownloadsData};
use protocol::{Cmd, ErrorInfo, Output, format};

use crate::Engine;
use crate::context::Scoped as _;
use crate::observe::iso8601;
use crate::request::DownloadsRequest;

impl Engine {
    /// `downloads`: every download of the session, oldest first. With
    /// `--clear`, deletes the files of the finished ones and forgets them.
    pub fn downloads(&self, req: DownloadsRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let dir = running
            .tabs
            .downloads_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let since = self.settings.downloads_since().map(iso8601);
        let mut warnings = Vec::new();
        if let Some(since) = &since {
            warnings.push(format!(
                "the list starts at {since} (the session's daemon was replaced); \
                 earlier downloads are still in {dir}"
            ));
        }
        if req.clear {
            let (deleted, kept) = running.tabs.clear_downloads();
            let deleted: Vec<String> = deleted.iter().map(|p| p.display().to_string()).collect();
            if !kept.is_empty() {
                let names: Vec<&str> = kept.iter().map(|d| d.file.as_str()).collect();
                warnings.push(format!(
                    "kept {} still downloading: {}",
                    kept.len(),
                    names.join(", ")
                ));
            }
            let plural = if deleted.len() == 1 { "" } else { "s" };
            let text = format!(
                "deleted {} downloaded file{plural}; the list is {}",
                deleted.len(),
                if kept.is_empty() { "empty" } else { "cleared" }
            );
            let quiet = deleted.join("\n");
            let data = DownloadsData {
                dir,
                downloads: kept,
                deleted,
                since,
            };
            return Ok(warnings
                .into_iter()
                .fold(Output::typed(&data, text).quiet(quiet), Output::warning));
        }
        let downloads = running.tabs.downloads();
        let text = if downloads.is_empty() {
            format!("no downloads yet; files the pages download go to {dir}")
        } else {
            let rows: Vec<Vec<String>> = downloads
                .iter()
                .map(|d| {
                    vec![
                        d.file.clone(),
                        size(d),
                        d.state.replace('_', " "),
                        d.path.clone().unwrap_or_else(|| d.url.clone()),
                    ]
                })
                .collect();
            format::table(&rows)
        };
        let quiet: Vec<&str> = downloads.iter().filter_map(|d| d.path.as_deref()).collect();
        let quiet = quiet.join("\n");
        let data = DownloadsData {
            dir,
            downloads,
            deleted: Vec::new(),
            since,
        };
        let output = Output::typed(&data, text)
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it");
        Ok(warnings.into_iter().fold(output, Output::warning))
    }
}

/// `27B`, or `12KB of 50KB` while it downloads.
fn size(download: &DownloadInfo) -> String {
    let received = format::size(download.received_bytes);
    match download.total_bytes {
        Some(total) if download.state == "in_progress" => {
            format!("{received} of {}", format::size(total))
        }
        _ => received,
    }
}

/// The line a command prints for a download: `downloaded: report.pdf
/// (12KB) → /path/report.pdf`.
pub(crate) fn line(download: &DownloadInfo, session: &str) -> String {
    match (download.state.as_str(), &download.path) {
        ("completed", Some(path)) => {
            format!(
                "downloaded: {} ({}) → {path}",
                download.file,
                size(download)
            )
        }
        ("canceled", _) => format!("download canceled: {} ({})", download.file, download.url),
        _ => format!(
            "downloading: {} ({} so far); {} waits for it",
            download.file,
            size(download),
            Cmd::new("wait").switch("download").scoped(session)
        ),
    }
}

#[cfg(test)]
mod tests {
    use protocol::schema::DownloadInfo;

    use super::line;

    fn download(state: &str, path: Option<&str>) -> DownloadInfo {
        DownloadInfo {
            tab: Some("t1".into()),
            url: "http://x/report.pdf".into(),
            file: "report.pdf".into(),
            path: path.map(Into::into),
            state: state.into(),
            received_bytes: 12 * 1024,
            total_bytes: Some(50 * 1024),
        }
    }

    #[test]
    fn report_lines() {
        assert_eq!(
            line(&download("completed", Some("/d/report.pdf")), "default"),
            "downloaded: report.pdf (12KB) → /d/report.pdf"
        );
        assert_eq!(
            line(&download("in_progress", None), "shop"),
            "downloading: report.pdf (12KB of 50KB so far); \
             chrome-cdp wait --download --session shop waits for it"
        );
        assert_eq!(
            line(&download("canceled", None), "default"),
            "download canceled: report.pdf (http://x/report.pdf)"
        );
    }
}

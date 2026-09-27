//! Chrome's stderr: the last lines are kept for launch-failure messages,
//! every line is logged at debug level (`-vv`), and with a log file it is
//! written to `chrome.log`, rotated to `chrome.log.1` at [`LOG_LIMIT`].

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Lines kept for error messages.
const TAIL: usize = 20;

/// `chrome.log` is rotated when it grows past this.
pub const LOG_LIMIT: u64 = 8 * 1024 * 1024;

/// A size-capped log file with one old generation (`<name>.1`).
#[derive(Debug)]
pub(crate) struct RotatingLog {
    path: PathBuf,
    limit: u64,
    file: Option<File>,
    written: u64,
}

impl RotatingLog {
    /// Starts a new log at `path`; an existing one becomes `<path>.1`.
    pub(crate) fn create(path: &Path, limit: u64) -> std::io::Result<Self> {
        let mut log = Self {
            path: path.to_owned(),
            limit,
            file: None,
            written: 0,
        };
        log.rotate()?;
        Ok(log)
    }

    fn old_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(".1");
        PathBuf::from(name)
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt as _;
        self.file = None;
        if self.path.exists() {
            std::fs::rename(&self.path, self.old_path())?;
        }
        self.file = Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&self.path)?,
        );
        self.written = 0;
        Ok(())
    }

    /// Appends one line, rotating first if it would pass the limit.
    pub(crate) fn line(&mut self, line: &str) {
        let len = line.len() as u64 + 1;
        if self.written > 0
            && self.written + len > self.limit
            && let Err(e) = self.rotate()
        {
            tracing::warn!(log = %self.path.display(), "cannot rotate the Chrome log: {e}");
        }
        if let Some(file) = &mut self.file
            && writeln!(file, "{line}").is_ok()
        {
            self.written += len;
        }
    }
}

/// Reads Chrome's stderr in a task.
#[derive(Debug)]
pub(crate) struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
    task: Option<JoinHandle<()>>,
}

impl StderrTail {
    pub(crate) fn start(
        stderr: Option<impl AsyncRead + Unpin + Send + 'static>,
        mut log: Option<RotatingLog>,
    ) -> Self {
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let task = stderr.map(|stderr| {
            let lines = lines.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::new();
                while let Ok(n) = reader.read_until(b'\n', &mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
                    buf.clear();
                    tracing::debug!(target: "chrome", "{line}");
                    if let Some(log) = &mut log {
                        log.line(&line);
                    }
                    if let Ok(mut lines) = lines.lock() {
                        if lines.len() == TAIL {
                            lines.pop_front();
                        }
                        lines.push_back(line);
                    }
                }
            })
        });
        Self { lines, task }
    }

    /// The lines kept so far.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|l| l.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Waits (briefly) for stderr to close and returns the kept lines.
    pub(crate) async fn finish(&mut self) -> Vec<String> {
        if let Some(task) = self.task.take() {
            // Chrome's helper processes can keep stderr open after the
            // browser exits; do not wait for them long.
            let abort = task.abort_handle();
            if timeout(Duration::from_secs(1), task).await.is_err() {
                abort.abort();
            }
        }
        self.lines()
    }
}

impl Drop for StderrTail {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RotatingLog, StderrTail};

    #[test]
    fn the_log_rotates_at_its_limit_and_on_create() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chrome.log");
        std::fs::write(&path, "previous run\n").unwrap();
        let mut log = RotatingLog::create(&path, 10).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("chrome.log.1")).unwrap(),
            "previous run\n"
        );
        log.line("12345");
        log.line("678");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "12345\n678\n");
        log.line("abcdef");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abcdef\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("chrome.log.1")).unwrap(),
            "12345\n678\n"
        );
        // A line longer than the limit is still written whole.
        log.line("a line longer than ten");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a line longer than ten\n"
        );
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn keeps_the_tail_and_writes_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chrome.log");
        let text: String = (0..30)
            .map(|i| format!("line {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let log = RotatingLog::create(&path, super::LOG_LIMIT).unwrap();
        let mut tail = StderrTail::start(
            Some(std::io::Cursor::new(text.clone().into_bytes())),
            Some(log),
        );
        let lines = tail.finish().await;
        assert_eq!(lines.len(), 20);
        assert_eq!(lines[0], "line 10");
        assert_eq!(lines[19], "line 29");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}

//! Text and `-q` forms of the session commands (plan 08 §2). The daemon
//! uses them for its answers, and the CLI for sessions without a daemon.

use std::fmt::Write as _;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::time::Duration;

use engine::{Opened, describe};
use protocol::schema::{
    BrowserInfo, BrowserKind, CloseData, ClosedSession, SessionEntry, SessionState, SessionsData,
    StatusData,
};
use protocol::{Cmd, Output, format};

/// `open` and `connect`.
pub fn opened(opened: &Opened) -> Output {
    let data = &opened.data;
    let session = &data.session;
    let browser = describe(&data.browser);
    let mut text = match (data.started, data.browser.kind) {
        (true, BrowserKind::Launched) => format!("opened session {session}: {browser}"),
        (true, BrowserKind::Connected) => format!("connected session {session}: {browser}"),
        (false, _) => format!("session {session} is already open: {browser}"),
    };
    if let Some(device) = &data.device {
        let _ = write!(text, "\ndevice: {device}");
    }
    if let Some(state) = &data.state {
        let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        let _ = write!(
            text,
            "\nstate: {}, {}",
            plural(state.cookies, "cookie"),
            plural(state.origins.len(), "origin")
        );
        if let Some(path) = &state.path {
            let _ = write!(text, " from {path}");
        }
    }
    let quiet = match &data.page {
        Some(page) => {
            let _ = write!(text, "\npage: {} {:?}", page.url, page.title);
            page.url.clone()
        }
        None => session.clone(),
    };
    opened
        .warnings
        .iter()
        .fold(Output::typed(data, text).quiet(quiet), Output::warning)
}

/// `status`.
pub fn status(data: &StatusData) -> Output {
    let mut text = format!("session {}: {}", data.session, data.state.as_str());
    if let Some(uptime) = data.uptime_ms {
        let _ = write!(text, ", up {}", uptime_text(uptime));
    }
    if let Some(browser) = &data.browser {
        let _ = write!(text, "\nbrowser: {}", describe(browser));
        chrome_line(&mut text, browser);
    }
    if let Some(crash) = &data.crash {
        let _ = write!(text, "\ncrash: {crash}");
    }
    if let Some(tabs) = data.tabs {
        let _ = write!(text, "\ntabs: {tabs}");
    }
    if let Some(pid) = data.daemon_pid {
        let _ = write!(text, "\ndaemon: pid {pid}");
        if let Some(idle) = data.idle_timeout_ms {
            let _ = match idle {
                0 => write!(text, ", no idle timeout"),
                ms => write!(
                    text,
                    ", idle timeout {}",
                    format::duration(Duration::from_millis(ms))
                ),
            };
        }
    }
    let _ = write!(text, "\ndir: {}", data.dir);
    match data.state {
        SessionState::Stopped => {
            let _ = write!(
                text,
                "\nstart it: {}",
                Cmd::new("open").session(&data.session)
            );
        }
        SessionState::Crashed => {
            let _ = write!(
                text,
                "\nstart a new browser: {}",
                Cmd::new("open").session(&data.session)
            );
        }
        SessionState::Running | SessionState::Idle => {}
    }
    if let Some(logs) = &data.logs {
        let _ = write!(text, "\nchrome.log ({} lines):", logs.len());
        for line in logs {
            let _ = write!(text, "\n  {line}");
        }
    }
    Output::typed(data, text).quiet(data.state.as_str())
}

fn chrome_line(text: &mut String, browser: &BrowserInfo) {
    if let Some(path) = &browser.path {
        let _ = write!(text, "\nchrome: {path}");
        if let Some(source) = &browser.source {
            let _ = write!(text, " ({source})");
        }
    }
    if let Some(profile) = &browser.profile {
        let _ = write!(text, "\nprofile: {} ({})", profile.dir, profile.kind);
    }
}

/// `status` for a session without a daemon.
pub fn stopped(session: &str, dir: &Path, logs: Option<Vec<String>>) -> StatusData {
    StatusData {
        session: session.to_owned(),
        state: SessionState::Stopped,
        browser: None,
        crash: None,
        tabs: None,
        uptime_ms: None,
        daemon_pid: None,
        idle_timeout_ms: None,
        dir: dir.display().to_string(),
        logs,
    }
}

/// `close` and `close --all`.
pub fn closed(sessions: Vec<ClosedSession>, all: bool) -> Output {
    let lines: Vec<String> = sessions
        .iter()
        .map(|s| match (s.was_running, s.browser.as_str()) {
            (false, _) => format!("session {} was not running", s.session),
            (true, "closed") => format!("closed session {} and its browser", s.session),
            (true, "detached") => format!(
                "closed session {}; the browser it was connected to keeps running",
                s.session
            ),
            (true, _) => format!("closed session {}", s.session),
        })
        .collect();
    let text = if all && lines.is_empty() {
        "no sessions were running".to_owned()
    } else {
        lines.join("\n")
    };
    let quiet: Vec<&str> = sessions
        .iter()
        .filter(|s| s.was_running)
        .map(|s| s.session.as_str())
        .collect();
    let quiet = quiet.join("\n");
    Output::typed(&CloseData { sessions }, text).quiet(quiet)
}

/// `sessions`.
pub fn sessions(sessions: Vec<SessionEntry>) -> Output {
    let text = if sessions.is_empty() {
        format!("no sessions; start one: {}", Cmd::new("open"))
    } else {
        let mut rows = vec![vec![
            "SESSION".to_owned(),
            "STATE".into(),
            "TABS".into(),
            "UPTIME".into(),
            "PID".into(),
            "BROWSER".into(),
        ]];
        for s in &sessions {
            rows.push(vec![
                s.session.clone(),
                s.state.as_str().into(),
                s.tabs.map_or("-".into(), |t| t.to_string()),
                s.uptime_ms.map_or("-".into(), uptime_text),
                s.daemon_pid.to_string(),
                match (&s.product, s.kind) {
                    (Some(p), Some(BrowserKind::Connected)) => format!("{p} (connected)"),
                    (Some(p), _) => p.clone(),
                    (None, _) => "-".into(),
                },
            ]);
        }
        format::table(&rows)
    };
    let quiet: Vec<&str> = sessions.iter().map(|s| s.session.as_str()).collect();
    let quiet = quiet.join("\n");
    Output::typed(&SessionsData { sessions }, text).quiet(quiet)
}

/// `1h5m`, `3m12s`, `40s`: an uptime to the second.
fn uptime_text(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m{s}s"),
        _ => format!("{h}h{m}m"),
    }
}

/// The last `lines` lines of a log file (reading at most its last 256 KB).
pub fn log_tail(path: &Path, lines: usize) -> Vec<String> {
    const WINDOW: u64 = 256 * 1024;
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(WINDOW);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut all: Vec<&str> = text.lines().collect();
    if start > 0 && !all.is_empty() {
        // The first line was cut by the window.
        all.remove(0);
    }
    let skip = all.len().saturating_sub(lines);
    all[skip..].iter().map(|l| (*l).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use protocol::schema::{ClosedSession, SessionEntry, SessionState};

    use super::{closed, log_tail, sessions, uptime_text};

    #[test]
    fn uptimes() {
        assert_eq!(uptime_text(999), "0s");
        assert_eq!(uptime_text(40_000), "40s");
        assert_eq!(uptime_text(192_000), "3m12s");
        assert_eq!(uptime_text(3_900_000), "1h5m");
    }

    #[test]
    fn log_tails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chrome.log");
        assert!(log_tail(&path, 5).is_empty());
        let mut f = std::fs::File::create(&path).unwrap();
        for i in 0..100 {
            writeln!(f, "line {i}").unwrap();
        }
        assert_eq!(log_tail(&path, 2), ["line 98", "line 99"]);
        assert_eq!(log_tail(&path, 500).len(), 100);
    }

    #[test]
    fn close_and_sessions_text() {
        let out = closed(
            vec![
                ClosedSession {
                    session: "a".into(),
                    was_running: true,
                    browser: "closed".into(),
                },
                ClosedSession {
                    session: "b".into(),
                    was_running: false,
                    browser: "none".into(),
                },
            ],
            false,
        );
        assert_eq!(
            out.text,
            "closed session a and its browser\nsession b was not running"
        );
        assert_eq!(out.quiet.as_deref(), Some("a"));
        assert_eq!(closed(vec![], true).text, "no sessions were running");

        let out = sessions(vec![SessionEntry {
            session: "default".into(),
            state: SessionState::Running,
            product: Some("HeadlessChrome/154.0.8037.57".into()),
            kind: None,
            tabs: Some(1),
            uptime_ms: Some(61_000),
            daemon_pid: 42,
        }]);
        assert_eq!(
            out.text,
            "SESSION  STATE    TABS  UPTIME  PID  BROWSER\n\
             default  running  1     1m1s    42   HeadlessChrome/154.0.8037.57"
        );
        assert_eq!(
            sessions(vec![]).text,
            "no sessions; start one: chrome-cdp open"
        );
    }
}

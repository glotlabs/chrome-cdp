//! `repro start` and `repro save` (plan 18, milestone 18.5): a bundle
//! that replays what a session did, offline.
//!
//! `repro start` reads the history position and has the daemon keep the
//! state, the time and the response bodies from then on (see
//! `engine::observe::repro`). `repro save DIR` writes a small project:
//! the commands since then as a test (`history --as-scenario`'s rules, see
//! [`history::build`], ending with the last failed command), every request
//! as a mock set (`mock record`'s conversion), the state at the start, the
//! evidence (snapshot, screenshot, console, errors, requests) and a
//! README. `chrome-cdp test DIR` replays it as that project (see
//! `test::plan`).

use std::fmt::Write as _;
use std::path::Path;

use daemon::rpc;
use engine::{HarExportRequest, ReproStartRequest};
use protocol::format::size;
use protocol::schema::{HistoryData, HistoryEntry, ReproLeftOut, ReproSaveData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cli::{GlobalArgs, ReproArgs, ReproCommand, ReproSaveArgs, ReproStartArgs};
use crate::history::{self, PASSWORD_SECRET, Rules};
use crate::{mock, output, script, session};

/// The files a bundle has, which `--force` replaces.
const OWN: &[&str] = &[
    "chrome-cdp.toml",
    "repro.cdp",
    "state.json",
    "README.md",
    "mocks",
    "evidence",
];

pub(crate) fn run(args: &ReproArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    match &args.command {
        ReproCommand::Start(args) => start(args, global),
        ReproCommand::Save(args) => save(args, global),
    }
}

/// `repro start`.
fn start(args: &ReproStartArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    ::mock::record::Scope::new(&args.scope).map_err(|e| ErrorInfo::new(ErrorKind::BadArgs, e))?;
    let history = history_data(global)?;
    // Numbers go on after dropped commands: the last given is this.
    let last = history.dropped + history.entries.len() as u64;
    let request = ReproStartRequest {
        scope: args.scope.clone(),
        history_from: history.entries.last().map_or(last, |e| e.n) + 1,
        timeout_ms: session::timeout_ms(global),
    };
    session::call_running(global, rpc::REPRO_START, &request)
}

/// What `repro start` kept, as the daemon's `repro_info` gives it.
#[derive(Debug, Deserialize)]
struct Recording {
    started: String,
    history_from: u64,
    state: Value,
    url: Option<String>,
    #[serde(default)]
    page: Value,
}

/// `repro save DIR`.
fn save(args: &ReproSaveArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let cwd = std::env::current_dir().map_err(|e| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read the current directory: {e}"),
        )
    })?;
    let dir = std::path::absolute(cwd.join(&args.dir)).unwrap_or_else(|_| cwd.join(&args.dir));
    ready_dir(&dir, args.force)?;
    let info = session::call_running(global, rpc::REPRO_INFO, &json!({}))?;
    let recording: Option<Recording> = serde_json::from_value(info.data["recording"].clone())
        .ok()
        .flatten();
    let mut warnings = Vec::new();
    if recording.is_none() {
        warnings.push(format!(
            "no `repro start` in this session: the bundle has the whole history, no cookies or \
             storage, and only the bodies Chrome still had; `{}` first next time",
            Cmd::new("repro").arg("start")
        ));
    }
    let entries: Vec<HistoryEntry> = history_data(global)?
        .entries
        .into_iter()
        .filter(|e| {
            e.context == global.context
                && recording.as_ref().is_none_or(|r| e.n >= r.history_from)
                && history::command_of(&e.words)
                    .first()
                    .is_none_or(|c| c != "repro")
        })
        .collect();
    let name = args.name.clone().unwrap_or_else(|| {
        dir.file_name()
            .map_or_else(|| "repro".to_owned(), |n| n.to_string_lossy().into_owned())
    });
    let built = history::build(&name, &entries, None, Rules { repro: true });
    warnings.extend(built.warnings.iter().cloned());

    // Every request since the start, with its body.
    let request = HarExportRequest {
        tab: None,
        scope: vec![engine::REPRO_EVERYTHING.to_owned()],
        repro: recording.is_some(),
        timeout_ms: session::timeout_ms(global),
    };
    let exported = session::call_running(global, rpc::HAR_EXPORT, &request)?;
    let har = &exported.data["har"];
    let mut exchanges = ::mock::record::har_exchanges(har)
        .map_err(|e| ErrorInfo::new(ErrorKind::ProtocolError, format!("bad export: {e}")))?;
    let left_out = limit_bodies(&mut exchanges, args.max_body);

    // The test.
    let start_url = recording
        .as_ref()
        .and_then(|r| r.url.clone())
        .filter(|_| !built.starts_with_goto);
    let mut steps = built.steps;
    let mut test = String::new();
    let _ = writeln!(
        test,
        "# Written by `chrome-cdp repro save`: what session {} did{}.",
        global.session,
        recording.as_ref().map_or_else(String::new, |r| format!(
            " from `repro start` at {}",
            r.started
        ))
    );
    test.push_str(
        "# `chrome-cdp test <this folder>` replays it from mocks/repro.toml; see README.md.\n\n",
    );
    test.push_str("@before\n  state load state.json\n\n");
    let _ = writeln!(test, "@test {}", history::quoted_name(&name));
    if let Some(url) = &start_url {
        let _ = writeln!(test, "  goto {}", protocol::format::shell_quote(url));
        steps += 1;
    }
    for line in &built.body {
        let _ = writeln!(test, "{line}");
    }
    if steps == 0 {
        warnings.push("the test has no step: the session ran no command a test keeps".to_owned());
    } else if start_url.is_none() && !built.starts_with_goto {
        warnings.push(
            "the test does not start with `goto`: add the page it starts on to repro.cdp"
                .to_owned(),
        );
    }
    let mut secrets = secret_names(&test);
    secrets.sort();
    secrets.dedup();

    // The files.
    let mut files = Vec::new();
    let mut write = |rel: &str, bytes: &[u8]| -> Result<(), ErrorInfo> {
        write_file(&dir.join(rel), bytes)?;
        files.push(rel.to_owned());
        Ok(())
    };
    let base_url = global
        .base_url
        .clone()
        .or_else(|| start_url.as_deref().and_then(origin))
        .or_else(|| first_goto(&built.body).and_then(|u| origin(&u)));
    write(
        "chrome-cdp.toml",
        config(base_url.as_deref(), recording.as_ref()).as_bytes(),
    )?;
    write("repro.cdp", output::mask(&test).as_bytes())?;
    let state = recording.as_ref().map_or_else(
        || json!({ "cookies": [], "origins": [] }),
        |r| r.state.clone(),
    );
    let mut state_text = serde_json::to_string_pretty(&state).unwrap_or_default();
    state_text.push('\n');
    write_private(&dir.join("state.json"), state_text.as_bytes())?;
    files.push("state.json".to_owned());
    let origin_line = format!(
        "Recorded by `chrome-cdp repro save` from session {}{}.",
        global.session,
        if recording.is_some() {
            ", since `repro start`"
        } else {
            ""
        }
    );
    let recorded = mock::record_bundle(&dir.join("mocks"), &exchanges, &origin_line)?;
    files.push("mocks/repro.toml".to_owned());
    files.extend(recorded.files.iter().filter_map(|f| {
        Path::new(f)
            .strip_prefix(&dir)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }));
    let evidence = evidence(&dir, global, &mut warnings);
    files.extend(evidence);

    let browser = har
        .pointer("/log/browser")
        .map(|b| {
            format!(
                "{} {}",
                b["name"].as_str().unwrap_or("Chrome"),
                b["version"].as_str().unwrap_or("")
            )
        })
        .unwrap_or_default();
    let data = ReproSaveData {
        path: dir.display().to_string(),
        name: name.clone(),
        started: recording.is_some(),
        steps,
        failed: built.failed.clone(),
        requests: recorded.requests,
        mocks: recorded.mocks,
        left_out,
        secrets,
        files: Vec::new(),
    };
    let readme = readme(&data, recording.as_ref(), &browser, &built.left_out);
    write_file(&dir.join("README.md"), output::mask(&readme).as_bytes())?;
    files.push("README.md".to_owned());
    files.sort();
    let data = ReproSaveData { files, ..data };

    let mut output = Output::typed(&data, summary(&data)).quiet(data.path.clone());
    for w in warnings {
        output = output.warning(w);
    }
    for skipped in recorded.skipped.iter().take(3) {
        output = output.warning(format!("not in the mocks: {skipped}"));
    }
    Ok(output.limit(
        args.max_chars,
        "--max-chars 0 for all of it; README.md has the rest",
    ))
}

/// The history, as the daemon keeps it.
fn history_data(global: &GlobalArgs) -> Result<HistoryData, ErrorInfo> {
    let output = session::call_running(global, rpc::HISTORY, &json!({ "clear": false }))?;
    serde_json::from_value(output.data).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!("the daemon sent a bad history: {e}"),
        )
    })
}

/// Checks that `dir` is new or empty, or with `force` clears the bundle's
/// own files from it.
fn ready_dir(dir: &Path, force: bool) -> Result<(), ErrorInfo> {
    let Ok(mut entries) = std::fs::read_dir(dir) else {
        if dir.exists() {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("{} is not a directory", dir.display()),
            ));
        }
        return Ok(());
    };
    if entries.next().is_none() {
        return Ok(());
    }
    if !force {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("{} has files already", dir.display()),
        )
        .hint(Hint::text(
            "name a new directory, or --force replaces the bundle's files in it",
        )));
    }
    for own in OWN {
        let path = dir.join(own);
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else if path.exists() {
            std::fs::remove_file(&path)
        } else {
            Ok(())
        };
        removed.map_err(|e| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("cannot remove {}: {e}", path.display()),
            )
        })?;
    }
    Ok(())
}

/// Replaces bodies over the limit (over a fifth of it for images and
/// fonts) with a placeholder; returns those, and the bodies Chrome no
/// longer had.
fn limit_bodies(exchanges: &mut [::mock::record::Exchange], max_body: u64) -> Vec<ReproLeftOut> {
    let mut left = Vec::new();
    for ex in exchanges.iter_mut() {
        let media = matches!(ex.resource_type.as_str(), "image" | "font" | "media")
            || ex.mime.starts_with("image/")
            || ex.mime.starts_with("font/");
        let limit = if media { max_body / 5 } else { max_body };
        let request = format!("{} {}", ex.method, ex.url);
        match &ex.response_body {
            Some(body) if body.len() as u64 > limit => {
                let why = format!("over {}", size(limit));
                left.push(ReproLeftOut {
                    request,
                    bytes: Some(body.len() as u64),
                    why: why.clone(),
                });
                ex.response_body = None;
                ex.left_out = Some(why);
            }
            None if ex.status >= 200 && !(300..400).contains(&ex.status) => {
                left.push(ReproLeftOut {
                    request,
                    bytes: None,
                    why: "Chrome no longer had it".to_owned(),
                });
            }
            _ => {}
        }
    }
    left
}

/// The `${secret:NAME}` names `text` uses.
fn secret_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("${secret:") {
        rest = &rest[at + "${secret:".len()..];
        if let Some(end) = rest.find('}') {
            names.push(rest[..end].to_owned());
            rest = &rest[end..];
        }
    }
    names
}

/// `http://host:port` of an http(s) URL.
fn origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.origin().ascii_serialization())
}

/// The URL of the first `goto` line of a test body.
fn first_goto(body: &[String]) -> Option<String> {
    body.iter().find_map(|line| {
        let rest = line.trim().strip_prefix("goto ")?;
        Some(rest.trim_matches('\'').to_owned())
    })
}

/// The bundle's `chrome-cdp.toml`.
fn config(base_url: Option<&str>, recording: Option<&Recording>) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut text = String::from(
        "# A repro bundle written by `chrome-cdp repro save`.\n\
         # `chrome-cdp test <this folder>` replays it: every request is answered by\n\
         # mocks/repro.toml, and one no mock answers fails (no web server, no network).\n\n\
         [e2e]\n\
         tests = [\"repro.cdp\"]\n\
         mocks = \"mocks\"\n\
         default_mocks = [\"repro\"]\n",
    );
    if let Some(base) = base_url {
        let _ = writeln!(text, "base_url = {}", quote(base));
    }
    let page = recording.map(|r| &r.page);
    if let Some(page) = page
        && let (Some(w), Some(h)) = (page["width"].as_u64(), page["height"].as_u64())
        && w > 0
        && h > 0
    {
        let _ = writeln!(text, "viewport = \"{w}x{h}\"");
    }
    text.push_str(
        "# The session's page logged what it logged: the replay does not fail on it.\n\
         allow_console_errors = true\n\
         fingerprints = \"off\"\n",
    );
    text.push_str("\n[e2e.determinism]\n");
    if let Some(r) = recording {
        let _ = writeln!(text, "# when `repro start` ran");
        let _ = writeln!(text, "freeze_time = {}", quote(&r.started));
    } else {
        let _ = writeln!(text, "# no `repro start`: the time of the save");
        let _ = writeln!(
            text,
            "freeze_time = {}",
            quote(&protocol::format::iso8601_now())
        );
    }
    if let Some(page) = page {
        if let Some(locale) = page["locale"].as_str() {
            let _ = writeln!(text, "locale = {}", quote(locale));
        }
        if let Some(tz) = page["timezone"].as_str() {
            let _ = writeln!(text, "timezone = {}", quote(tz));
        }
    }
    text
}

/// The screenshot of `evidence/`.
const SHOT: &str = "evidence/screenshot.png";

/// Writes `evidence/`: the page as the session left it. A command that
/// fails writes its error instead, with a warning. Returns the files.
fn evidence(dir: &Path, global: &GlobalArgs, warnings: &mut Vec<String>) -> Vec<String> {
    let shot = dir.join("evidence").join("screenshot.png");
    let shot_arg = shot.display().to_string();
    let commands: [(&str, Vec<&str>); 5] = [
        (
            "evidence/snapshot.txt",
            vec!["snapshot", "--max-chars", "0"],
        ),
        (SHOT, vec!["screenshot", "-o", &shot_arg]),
        (
            "evidence/console.txt",
            vec!["console", "--limit", "0", "--max-chars", "0"],
        ),
        (
            "evidence/errors.txt",
            vec!["errors", "--limit", "0", "--max-chars", "0"],
        ),
        (
            "evidence/requests.txt",
            vec!["network", "--limit", "0", "--max-chars", "0"],
        ),
    ];
    let mut files = Vec::new();
    for (rel, words) in commands {
        let words: Vec<String> = words.into_iter().map(str::to_owned).collect();
        let result = script::command(&words, global).and_then(crate::run);
        let path = dir.join(rel);
        let text = match result {
            Ok(_) if rel == SHOT => {
                if path.is_file() {
                    files.push(rel.to_owned());
                }
                continue;
            }
            Ok(out) => format!("{}\n", out.text.trim_end()),
            Err(info) => {
                warnings.push(format!("{rel}: {}", info.message));
                if rel == SHOT {
                    continue;
                }
                format!("error: {}\n", info.message)
            }
        };
        if write_file(&path, output::mask(&text).as_bytes()).is_ok() {
            files.push(rel.to_owned());
        }
    }
    files
}

/// The bundle's README.md.
fn readme(
    data: &ReproSaveData,
    recording: Option<&Recording>,
    browser: &str,
    left_out: &[String],
) -> String {
    let mut text = format!("# Repro: {}\n\n", data.name);
    let _ = writeln!(
        text,
        "Written by `chrome-cdp repro save` at {}{}.\n",
        protocol::format::iso8601_now(),
        recording.map_or_else(
            || " from the whole history (no `repro start`)".to_owned(),
            |r| format!(", from `repro start` at {}", r.started)
        )
    );
    text.push_str("## What happened\n\n");
    let _ = writeln!(
        text,
        "`repro.cdp` has {} {}, from the session's commands.",
        data.steps,
        if data.steps == 1 { "step" } else { "steps" }
    );
    match &data.failed {
        Some(failed) => {
            let _ = writeln!(
                text,
                "The last one (command {} of the session) failed:\n\n```\n{}\n{}{}\n```\n",
                failed.n,
                failed.step,
                failed.kind,
                failed
                    .message
                    .as_deref()
                    .map_or_else(String::new, |m| format!(": {m}"))
            );
            text.push_str(
                "The test fails there the same way; once the bug is fixed it passes, and it is a \
                 regression test.\n\n",
            );
        }
        None => text.push_str("No command of the session failed.\n\n"),
    }
    if !left_out.is_empty() {
        text.push_str("Commands not in the test:\n\n");
        for line in left_out.iter().take(50) {
            let _ = writeln!(text, "- {line}");
        }
        if left_out.len() > 50 {
            let _ = writeln!(text, "- … {} more", left_out.len() - 50);
        }
        text.push('\n');
    }
    text.push_str("## Replay\n\n```\nchrome-cdp test <this folder>\n```\n\n");
    let _ = writeln!(
        text,
        "Every request is answered by `mocks/repro.toml` ({} {} from {} {}, bodies in \
         `mocks/data/`); a request no mock answers fails, so nothing reaches the network. \
         `@before` loads `state.json`, the cookies and storage at the start. The clock is \
         frozen at the start, and the viewport, locale and time zone are the session's \
         (`chrome-cdp.toml`). What the session's page showed last is in `evidence/`.\n",
        data.mocks,
        if data.mocks == 1 { "mock" } else { "mocks" },
        data.requests,
        if data.requests == 1 {
            "request"
        } else {
            "requests"
        },
    );
    if !data.secrets.is_empty() {
        text.push_str("## Secrets\n\nThe replay reads these from the environment:\n\n");
        for name in &data.secrets {
            let why = if name == PASSWORD_SECRET {
                " (typed into a password field; its value was not kept)"
            } else {
                ""
            };
            let _ = writeln!(text, "- `{name}`{why}");
        }
        text.push('\n');
    }
    if !data.left_out.is_empty() {
        text.push_str(
            "## Left out\n\nThese response bodies are not in the mocks (their mocks answer with \
             no body; add one if the replay needs it):\n\n",
        );
        for left in &data.left_out {
            let bytes = left
                .bytes
                .map_or_else(String::new, |b| format!(" ({})", size(b)));
            let _ = writeln!(text, "- {}{bytes}: {}", left.request, left.why);
        }
        text.push('\n');
    }
    text.push_str("## Versions\n\n");
    let _ = writeln!(text, "- chrome-cdp {}", env!("CARGO_PKG_VERSION"));
    if !browser.trim().is_empty() {
        let _ = writeln!(text, "- {}", browser.trim());
    }
    text
}

/// The text output.
fn summary(data: &ReproSaveData) -> String {
    let mut text = format!(
        "{}: repro {} with {} {}",
        data.path,
        history::quoted_name(&data.name),
        data.steps,
        if data.steps == 1 { "step" } else { "steps" }
    );
    if let Some(failed) = &data.failed {
        let _ = write!(
            text,
            ", ending with the failed `{}` ({})",
            failed.step, failed.kind
        );
    }
    let _ = write!(
        text,
        "; {} {} from {} {}",
        data.mocks,
        if data.mocks == 1 { "mock" } else { "mocks" },
        data.requests,
        if data.requests == 1 {
            "request"
        } else {
            "requests"
        }
    );
    if !data.left_out.is_empty() {
        let _ = write!(
            text,
            "\n  left out {} {} (README.md lists them)",
            data.left_out.len(),
            if data.left_out.len() == 1 {
                "body"
            } else {
                "bodies"
            }
        );
    }
    if !data.secrets.is_empty() {
        let _ = write!(
            text,
            "\n  the replay needs secrets: {}",
            data.secrets.join(", ")
        );
    }
    let _ = write!(text, "\nnext: {}", Cmd::new("test").arg(&data.path));
    text
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ErrorInfo> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| cannot_write(parent, &e))?;
    }
    std::fs::write(path, bytes).map_err(|e| cannot_write(path, &e))
}

/// Writes a file only its owner can read: the state holds cookies.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ErrorInfo> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| cannot_write(parent, &e))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| cannot_write(path, &e))?;
    file.write_all(bytes).map_err(|e| cannot_write(path, &e))
}

fn cannot_write(path: &Path, e: &std::io::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("cannot write {}: {e}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(url: &str, kind: &str, mime: &str, body: Option<usize>) -> ::mock::record::Exchange {
        ::mock::record::Exchange {
            method: "GET".into(),
            url: url.into(),
            status: 200,
            response_body: body.map(|n| vec![b'x'; n]),
            mime: mime.into(),
            resource_type: kind.into(),
            ..Default::default()
        }
    }

    #[test]
    fn big_bodies_become_placeholders() {
        let mut exchanges = vec![
            ex(
                "http://a.test/app.js",
                "script",
                "text/javascript",
                Some(900),
            ),
            ex(
                "http://a.test/big.json",
                "fetch",
                "application/json",
                Some(1001),
            ),
            ex("http://a.test/hero.png", "image", "image/png", Some(201)),
            ex("http://a.test/logo.png", "image", "image/png", Some(200)),
            ex("http://a.test/gone.css", "stylesheet", "text/css", None),
        ];
        let left = limit_bodies(&mut exchanges, 1000);
        let shown: Vec<String> = left
            .iter()
            .map(|l| format!("{} {:?} {}", l.request, l.bytes, l.why))
            .collect();
        assert_eq!(
            shown,
            [
                "GET http://a.test/big.json Some(1001) over 1000B",
                "GET http://a.test/hero.png Some(201) over 200B",
                "GET http://a.test/gone.css None Chrome no longer had it",
            ]
        );
        assert!(exchanges[0].response_body.is_some() && exchanges[3].response_body.is_some());
        assert_eq!(exchanges[1].response_body, None);
        assert_eq!(exchanges[1].left_out.as_deref(), Some("over 1000B"));
    }

    #[test]
    fn secrets_are_found_by_name() {
        assert_eq!(
            secret_names("fill a ${secret:PASSWORD}\nfill b '${secret:API_KEY}' ${env:X}"),
            ["PASSWORD", "API_KEY"]
        );
    }

    #[test]
    fn the_config_pins_the_session() {
        let recording = Recording {
            started: "2026-10-01T10:00:00.000Z".into(),
            history_from: 3,
            state: Value::Null,
            url: None,
            page: json!({"width": 1280, "height": 720, "locale": "nb-NO", "timezone": "Europe/Oslo"}),
        };
        let text = config(Some("http://127.0.0.1:8080"), Some(&recording));
        let parsed = e2e::Config::parse(&text, Path::new("/b/chrome-cdp.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.tests, ["repro.cdp"]);
        assert_eq!(parsed.default_mocks, ["repro"]);
        assert_eq!(parsed.base_url.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!((parsed.viewport.width, parsed.viewport.height), (1280, 720));
        assert!(parsed.allow_console_errors);
        assert!(parsed.web_servers.is_empty());
        let fixed = parsed.determinism.resolve();
        assert_eq!(
            fixed.freeze_time.as_deref(),
            Some("2026-10-01T10:00:00.000Z")
        );
        assert_eq!(fixed.locale.as_deref(), Some("nb-NO"));
        assert_eq!(fixed.timezone.as_deref(), Some("Europe/Oslo"));
    }
}

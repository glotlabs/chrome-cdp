//! `record start|status|stop` (plan 18, milestone 18.6): the daemon
//! records what happens in the page (see `engine::record`); `stop` writes
//! its steps as a test, as `history --as-scenario` writes commands: URLs
//! under `base_url` become paths, and a password typed becomes
//! `${secret:PASSWORD}` in double quotes.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use daemon::rpc;
use engine::{RecordStartRequest, RecordStopRequest};
use protocol::schema::{RecordStopData, RecordedData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Output};
use serde_json::json;

use crate::cli::{GlobalArgs, RecordArgs, RecordCommand, RecordStopArgs};
use crate::history::{quoted_name, relative_urls, script_word};
use crate::session;

pub(crate) fn run(args: &RecordArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    match &args.command {
        RecordCommand::Start(args) => {
            let request = RecordStartRequest {
                name: args.name.clone(),
                testid_attribute: global.testid_attribute.clone(),
                timeout_ms: session::timeout_ms(global),
            };
            session::call_running(global, rpc::RECORD_START, &request)
        }
        RecordCommand::Status(_) => session::call_running(global, rpc::RECORD_STATUS, &json!({})),
        RecordCommand::Stop(args) => stop(args, global),
    }
}

/// `record stop`.
fn stop(args: &RecordStopArgs, global: &GlobalArgs) -> Result<Output, ErrorInfo> {
    let request = RecordStopRequest {
        timeout_ms: session::timeout_ms(global),
    };
    let output = session::call_running(global, rpc::RECORD_STOP, &request)?;
    let recorded: RecordedData = serde_json::from_value(output.data).map_err(|e| {
        ErrorInfo::new(
            ErrorKind::DaemonUnavailable,
            format!("the daemon sent a bad recording: {e}"),
        )
    })?;
    let (scenario, steps) = scenario(&recorded, global.base_url.as_deref());
    let file = match &args.output {
        Some(path) => Some(append(path, &scenario)?),
        None => None,
    };
    let text = match &file {
        Some(path) => format!(
            "appended \"{}\" ({steps} {}) to {path}\nnext: {}",
            recorded.name,
            if steps == 1 { "step" } else { "steps" },
            Cmd::new("test").arg(path)
        ),
        None => scenario.trim_end().to_owned(),
    };
    let data = RecordStopData {
        name: recorded.name,
        steps,
        scenario: scenario.clone(),
        file: file.clone(),
    };
    let mut out = Output::typed(&data, text)
        .quiet(file.unwrap_or_else(|| scenario.trim_end().to_owned()))
        .limit(args.max_chars, "--max-chars 0 for all of it, or -o FILE");
    if steps == 0 {
        out = out.warning(
            "nothing was recorded: act in the page between `record start` and `record stop`",
        );
    }
    for warning in recorded.warnings {
        out = out.warning(warning);
    }
    Ok(out)
}

/// The test `recorded` makes, and its number of steps.
fn scenario(recorded: &RecordedData, base_url: Option<&str>) -> (String, usize) {
    let mut text = String::new();
    let _ = writeln!(text, "@test {}", quoted_name(&recorded.name));
    let mut steps = 0;
    for step in &recorded.steps {
        for comment in &step.comments {
            let _ = writeln!(text, "  {comment}");
        }
        if step.words.is_empty() {
            continue;
        }
        let mut words = step.words.clone();
        if words[0] == "goto" || (words[0] == "tab" && words.get(1).is_some_and(|w| w == "new")) {
            relative_urls(&mut words, base_url);
        }
        let line: Vec<String> = words.iter().map(|w| script_word(w).into_owned()).collect();
        let _ = writeln!(text, "  {}", line.join(" "));
        steps += 1;
    }
    text.push_str("  # add expect steps: the recorder does not guess what to check\n");
    (text, steps)
}

/// Appends `scenario` to `path` (a blank line before it in a file that
/// has text); returns the file's absolute path.
fn append(path: &Path, scenario: &str) -> Result<String, ErrorInfo> {
    let fail = |e: std::io::Error| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot write {}: {e}", path.display()),
        )
    };
    let before = std::fs::read_to_string(path).unwrap_or_default();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(fail)?;
    let mut text = String::new();
    if !before.trim().is_empty() {
        if !before.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(scenario);
    file.write_all(text.as_bytes()).map_err(fail)?;
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(absolute.display().to_string())
}

#[cfg(test)]
mod tests {
    use protocol::schema::RecordedStep;

    use super::*;

    fn step(comments: &[&str], words: &[&str]) -> RecordedStep {
        RecordedStep {
            comments: comments.iter().map(|&c| c.to_owned()).collect(),
            words: words.iter().map(|&w| w.to_owned()).collect(),
        }
    }

    #[test]
    fn the_test_is_quoted_relative_and_asks_for_expects() {
        let recorded = RecordedData {
            name: "signs \"in\"".into(),
            steps: vec![
                step(&[], &["goto", "http://localhost:3000/login?next=/"]),
                step(&[], &["fill", "label=Email", "ada@example.com"]),
                step(
                    &["# a password field"],
                    &["fill", "label=Password", "${secret:PASSWORD}"],
                ),
                step(&["# no locator"], &[]),
                step(&[], &["click", "role=button[name=\"Sign in\"]"]),
                step(&[], &["tab", "new", "http://localhost:3000/"]),
                step(&[], &["fill", "label=Note", "costs ${price}"]),
            ],
            ..RecordedData::default()
        };
        let (text, steps) = scenario(&recorded, Some("http://localhost:3000/"));
        assert_eq!(steps, 6);
        assert_eq!(
            text,
            "@test \"signs \\\"in\\\"\"\n  goto '/login?next=/'\n  fill label=Email ada@example.com\n  \
             # a password field\n  fill label=Password \"${secret:PASSWORD}\"\n  # no locator\n  \
             click 'role=button[name=\"Sign in\"]'\n  tab new /\n  fill label=Note 'costs ${price}'\n  \
             # add expect steps: the recorder does not guess what to check\n"
        );
    }

    #[test]
    fn a_secret_inside_a_word_keeps_the_rest_literal() {
        assert_eq!(
            script_word("{\"p\":\"${secret:PASSWORD}\",\"x\":\"${y}\"}"),
            "\"{\\\"p\\\":\\\"${secret:PASSWORD}\\\",\\\"x\\\":\\\"$${y}\\\"}\""
        );
        assert_eq!(script_word("plain"), "plain");
        assert_eq!(script_word("one\ntwo\r\n"), "\"one\\ntwo\\n\"");
    }
}

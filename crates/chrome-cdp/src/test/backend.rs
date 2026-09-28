//! A real backend's `setup` and `teardown` commands (plan 11, milestone
//! 11.11): shell commands (`sh -c`) run in the project root once the web
//! servers answer, before the first test, and after the last. They see
//! `CHROME_CDP_BACKEND` and `CHROME_CDP_BASE_URL`, and their output goes
//! to `<artifacts>/backend-setup.log` (or `-teardown`). A failed setup
//! command stops the run; a failed teardown command is a warning.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use protocol::{ErrorInfo, ErrorKind, Hint, Mode, format};

use super::plan::Plan;
use crate::output;

/// How many lines of a failed command's output the error shows.
const TAIL: usize = 15;

/// Runs the backend's `setup` commands in order; the first that fails
/// is the error.
pub(super) fn setup(plan: &Plan, mode: Mode) -> Result<(), ErrorInfo> {
    for command in &plan.backend.setup {
        run(plan, command, "setup", mode)?;
    }
    Ok(())
}

/// Runs every `teardown` command; the failures come back as warnings.
pub(super) fn teardown(plan: &Plan, mode: Mode) -> Vec<String> {
    plan.backend
        .teardown
        .iter()
        .filter_map(|command| run(plan, command, "teardown", mode).err())
        .map(|info| info.message)
        .collect()
}

fn log_file(plan: &Plan, what: &str) -> PathBuf {
    plan.config.artifacts.join(format!("backend-{what}.log"))
}

fn run(plan: &Plan, command: &str, what: &str, mode: Mode) -> Result<(), ErrorInfo> {
    let backend = &plan.backend;
    let started = Instant::now();
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(&plan.config.root)
        .env("CHROME_CDP_BACKEND", &backend.name);
    if let Some(url) = &backend.base_url {
        cmd.env("CHROME_CDP_BASE_URL", url);
    }
    let out = cmd.output().map_err(|e| {
        ErrorInfo::new(
            ErrorKind::LaunchFailed,
            format!(
                "cannot run the {what} command `{command}` of backend {}: {e}",
                backend.name
            ),
        )
    })?;
    let log = log_file(plan, what);
    let mut text = format!("$ {command}\n");
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::create_dir_all(&plan.config.artifacts);
    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .and_then(|mut f| f.write_all(output::mask(&text).as_bytes()));
    if let Err(e) = appended {
        tracing::warn!("cannot write {}: {e}", log.display());
    }
    let took = format::duration(started.elapsed());
    if out.status.success() {
        if mode == Mode::Text {
            output::stdout(&format!("{what}: {command} ({took})\n"));
        }
        return Ok(());
    }
    let mut info = ErrorInfo::new(
        ErrorKind::LaunchFailed,
        format!(
            "the {what} command `{command}` of backend {} failed ({})",
            backend.name, out.status
        ),
    );
    let lines: Vec<&str> = text.lines().skip(1).collect();
    let mut tail = String::new();
    for line in &lines[lines.len().saturating_sub(TAIL)..] {
        let _ = writeln!(tail, "{line}");
    }
    if !tail.is_empty() {
        info = info.block("output", tail);
    }
    Err(info
        .note("log", log.display().to_string())
        .hint(Hint::text(format!(
            "run it yourself in {} to see why: {command}",
            plan.config.root.display()
        ))))
}

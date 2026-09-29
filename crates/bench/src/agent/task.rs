//! Agent benchmark tasks: `bench/agent/tasks/<id>/task.toml`, plus files
//! the task's working dir starts with in `bench/agent/tasks/<id>/files/`.
//! bench/agent/README.md documents the format.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::report;

/// The tasks' dir.
pub fn tasks_dir() -> PathBuf {
    report::workspace_root().join("bench/agent/tasks")
}

/// The two ways an agent gets a browser (plan 10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Arm {
    /// Only `chrome-cdp skill`, installed in the project.
    #[serde(rename = "chrome-cdp")]
    ChromeCdp,
    /// Only Playwright MCP.
    #[serde(rename = "playwright-mcp")]
    PlaywrightMcp,
}

impl Arm {
    pub const ALL: [Self; 2] = [Self::ChromeCdp, Self::PlaywrightMcp];

    pub fn name(self) -> &'static str {
        match self {
            Self::ChromeCdp => "chrome-cdp",
            Self::PlaywrightMcp => "playwright-mcp",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    #[serde(skip)]
    pub id: String,
    #[serde(skip)]
    pub dir: PathBuf,
    pub title: String,
    /// What the agent is told, after its arm's preamble. `{site}` and the
    /// other placeholders are expanded (see [`Vars`]).
    pub prompt: String,
    /// The arms that run it (both unless given).
    #[serde(default)]
    pub arms: Option<Vec<Arm>>,
    /// Needs the internet (a public sandbox site); `--offline` skips it.
    #[serde(default)]
    pub network: bool,
    /// A run is stopped and fails after this long.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub setup: Setup,
    pub verify: Verify,
    /// A known solution: `bench agent --check` applies it and expects the
    /// verifier to pass (and to fail without it), so a task's verifier is
    /// tested without an agent.
    #[serde(default)]
    pub solution: Solution,
}

fn default_timeout() -> u64 {
    600
}

impl Task {
    pub fn arms(&self) -> Vec<Arm> {
        self.arms.clone().unwrap_or_else(|| Arm::ALL.to_vec())
    }

    /// The task's own files, copied into its working dir after
    /// `setup.copy` (so they can replace copied files).
    pub fn files(&self) -> PathBuf {
        self.dir.join("files")
    }
}

/// How a task's working dir is prepared, in this order.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    /// Repository dirs or files to copy in.
    #[serde(default)]
    pub copy: Vec<Copy>,
    /// Paths to delete (after copying).
    #[serde(default)]
    pub remove: Vec<String>,
    /// Text replacements in files.
    #[serde(default)]
    pub replace: Vec<Replace>,
    /// A dir of the working dir to serve as `{dev}`: a local dev server
    /// (the fixture server, reading the files on every request).
    #[serde(default)]
    pub serve: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copy {
    /// Relative to the repository root.
    pub from: String,
    /// Relative to the working dir.
    #[serde(default = "dot")]
    pub to: String,
    /// File or dir names left out, at any depth.
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn dot() -> String {
    ".".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replace {
    /// Relative to the working dir; its last part may have one `*`
    /// (`dist/assets/index-*.js`), which must match exactly one file.
    pub file: String,
    pub from: String,
    pub to: String,
}

/// Every check given must pass.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verify {
    /// The agent's last `ANSWER:` line contains each (ignoring case,
    /// spaces, commas, `$` and quotes).
    #[serde(default)]
    pub answer: Vec<String>,
    /// The fixture server got a `/bench/record` of each.
    #[serde(default)]
    pub record: Vec<RecordCheck>,
    /// A `chrome-cdp batch` script run in a fresh session afterwards
    /// passes (for tasks that change the app: its `expect` lines check it).
    #[serde(default)]
    pub page: Option<String>,
    /// `chrome-cdp test --json <args>` in the working dir passes.
    #[serde(default)]
    pub test: Vec<TestCheck>,
    /// These paths are as the setup left them: the agent may not make a
    /// check pass by editing the check.
    #[serde(default)]
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordCheck {
    pub form: String,
    /// Each field of the record equals this (numbers and strings compare
    /// as text; `*` accepts any value).
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCheck {
    #[serde(default)]
    pub args: Vec<String>,
    /// At least this many tests passed (none failed).
    #[serde(default = "one")]
    pub min_passed: usize,
}

fn one() -> usize {
    1
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Solution {
    /// The reply a correct agent ends with (its `ANSWER:` line).
    #[serde(default)]
    pub answer: Option<String>,
    /// A `chrome-cdp batch` script that does the task in the browser.
    #[serde(default)]
    pub batch: Option<String>,
    /// Run the batch with `--continue`, for a step that is meant to stop
    /// (a click that opens a dialog exits 5 until it is answered).
    #[serde(default)]
    pub continue_on_error: bool,
    #[serde(default)]
    pub copy: Vec<Copy>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default)]
    pub replace: Vec<Replace>,
}

/// Loads every task, by id (its dir's name).
pub fn load_all() -> Result<Vec<Task>, String> {
    let dir = tasks_dir();
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.path().join("task.toml").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids.iter().map(|id| load(&dir.join(id))).collect()
}

pub fn load(dir: &Path) -> Result<Task, String> {
    let path = dir.join("task.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut task: Task = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    task.id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.clone_into(&mut task.dir);
    Ok(task)
}

/// The placeholders of a run: `{site}` and `{site_b}` (the fixture site's
/// two origins), `{dev}` (the served dir, if any), `{workdir}`, `{port}` and
/// `{port2}` (free ports), `{fixtures}` (the fixtures binary).
#[derive(Debug, Clone, Default)]
pub struct Vars(pub BTreeMap<&'static str, String>);

impl Vars {
    pub fn set(&mut self, name: &'static str, value: impl Into<String>) {
        self.0.insert(name, value.into());
    }

    /// `text` with every `{name}` of a known name replaced.
    pub fn expand(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (name, value) in &self.0 {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_loads_and_is_checkable() {
        let tasks = load_all().unwrap();
        assert!(tasks.len() >= 20, "only {} tasks", tasks.len());
        for t in &tasks {
            let v = &t.verify;
            assert!(
                !v.answer.is_empty()
                    || !v.record.is_empty()
                    || v.page.is_some()
                    || !v.test.is_empty(),
                "{}: no check",
                t.id
            );
            if !v.answer.is_empty() {
                assert!(
                    t.prompt.contains("ANSWER:"),
                    "{}: says nothing of the ANSWER: line",
                    t.id
                );
            }
            let s = &t.solution;
            assert!(
                s.answer.is_some()
                    || s.batch.is_some()
                    || !s.copy.is_empty()
                    || !s.replace.is_empty(),
                "{}: no solution for --check",
                t.id
            );
            assert!(!t.arms().is_empty(), "{}", t.id);
        }
    }

    #[test]
    fn placeholders_expand_known_names_only() {
        let mut v = Vars::default();
        v.set("site", "http://127.0.0.1:1");
        assert_eq!(
            v.expand("go to {site}/a, keep {other} and {x:1}"),
            "go to http://127.0.0.1:1/a, keep {other} and {x:1}"
        );
    }

    #[test]
    fn arms_by_name() {
        assert_eq!(Arm::parse("chrome-cdp"), Some(Arm::ChromeCdp));
        assert_eq!(Arm::parse("playwright-mcp"), Some(Arm::PlaywrightMcp));
        assert_eq!(Arm::parse("x"), None);
    }
}

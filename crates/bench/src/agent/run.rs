//! Runs one agent: Claude Code headless (`claude -p`) in a task's working
//! dir, with chrome-cdp as its browser and nothing else.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sandbox::Sandbox;

/// What the agent is told before the task.
pub const PREAMBLE: &str = "You have a browser through the `chrome-cdp` command; its skill says how \
     to use it. Use it for everything you do in the browser.";

/// The built-in tools the agent gets; the browser is `chrome-cdp`.
pub const TOOLS: &str = "Bash,Read,Edit,Write,Glob,Grep,Skill";

#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// The `claude` executable.
    pub claude: PathBuf,
    /// `--model`; Claude Code's default when unset.
    pub model: Option<String>,
    /// The chrome-cdp binary under test.
    pub bin: PathBuf,
    pub chrome: PathBuf,
}

/// Tokens a run used, over every model it called.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentRun {
    /// The agent's final reply.
    pub reply: String,
    /// Claude Code reported an error (or printed no result).
    pub is_error: bool,
    pub timed_out: bool,
    pub turns: u64,
    pub tokens: Tokens,
    pub cost_usd: f64,
    pub wall_ms: u64,
    /// Read, Grep and Glob calls in the transcript: how much of the code
    /// the agent looked through (plan 15.7).
    #[serde(default)]
    pub file_reads: u64,
    /// Commands run through Bash that read or list files (`cat`, `grep`,
    /// `sed`, `find`, …; see [`shell_reads`]), which `file_reads` misses.
    #[serde(default)]
    pub shell_reads: u64,
}

/// The tools that read the working dir's files.
pub const READ_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];

/// The shell commands that read or list files.
pub const READ_COMMANDS: [&str; 18] = [
    "cat", "head", "tail", "less", "more", "nl", "bat", "grep", "egrep", "fgrep", "rg", "ag",
    "sed", "awk", "find", "fd", "ls", "tree",
];

/// How many commands of a Bash call read or list files: each command
/// (split at `;`, `&&`, `||` and newlines) whose pipeline starts with one
/// of [`READ_COMMANDS`] counts once, so `grep -n x a.ts | head` is one
/// read. Leading `VAR=value` assignments, `cd dir &&` and a path before
/// the command name (`/usr/bin/grep`) are looked past.
pub fn shell_reads(command: &str) -> u64 {
    command
        .split(['\n', ';'])
        .flat_map(|part| part.split("&&"))
        .flat_map(|part| part.split("||"))
        .filter(|cmd| {
            let first_stage = cmd.split('|').next().unwrap_or_default();
            first_stage
                .split_whitespace()
                .find(|word| !word.contains('=') && *word != "(" && *word != "{")
                .map(|word| word.trim_start_matches(['(', '{']))
                .map(|word| word.rsplit('/').next().unwrap_or(word))
                .is_some_and(|name| READ_COMMANDS.contains(&name))
        })
        .count() as u64
}

/// Reads Claude Code's `--output-format stream-json` transcript (one JSON
/// message a line): the last `result` message, and the reading tool
/// calls of the assistant's messages.
pub fn parse_transcript(text: &str) -> AgentRun {
    let mut result = None;
    let mut file_reads = 0;
    let mut shell_reads = 0;
    for msg in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        match msg.get("type").and_then(Value::as_str) {
            Some("result") => result = Some(msg),
            Some("assistant") => {
                let content = msg
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                let tools = content
                    .iter()
                    .filter(|c| c.get("type").and_then(Value::as_str) == Some("tool_use"));
                for tool in tools {
                    match tool.get("name").and_then(Value::as_str) {
                        Some(name) if READ_TOOLS.contains(&name) => file_reads += 1,
                        Some("Bash") => {
                            let command = tool.pointer("/input/command").and_then(Value::as_str);
                            shell_reads += command.map_or(0, self::shell_reads);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let mut run = result.as_ref().map_or_else(
        || AgentRun {
            is_error: true,
            ..AgentRun::default()
        },
        parse_result,
    );
    run.file_reads = file_reads;
    run.shell_reads = shell_reads;
    run
}

/// Reads Claude Code's result message (`--output-format json`, or the
/// last line of `stream-json`).
pub fn parse_result(out: &Value) -> AgentRun {
    let u64_at = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut tokens = Tokens::default();
    if let Some(models) = out.get("modelUsage").and_then(Value::as_object) {
        for m in models.values() {
            tokens.input += u64_at(m, "inputTokens");
            tokens.output += u64_at(m, "outputTokens");
            tokens.cache_read += u64_at(m, "cacheReadInputTokens");
            tokens.cache_creation += u64_at(m, "cacheCreationInputTokens");
        }
    } else if let Some(u) = out.get("usage") {
        tokens = Tokens {
            input: u64_at(u, "input_tokens"),
            output: u64_at(u, "output_tokens"),
            cache_read: u64_at(u, "cache_read_input_tokens"),
            cache_creation: u64_at(u, "cache_creation_input_tokens"),
        };
    }
    AgentRun {
        reply: out
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        is_error: out.get("is_error").and_then(Value::as_bool).unwrap_or(true),
        timed_out: false,
        turns: u64_at(out, "num_turns"),
        tokens,
        cost_usd: out
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        wall_ms: 0,
        file_reads: 0,
        shell_reads: 0,
    }
}

/// Gets `workdir` ready: `chrome-cdp skill` installed in it, and the
/// binary under test first on the PATH returned.
fn setup(
    workdir: &Path,
    scratch: &Path,
    sandbox: &Sandbox,
    opts: &AgentOptions,
) -> Result<OsString, String> {
    let bin = scratch.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&opts.bin, bin.join("chrome-cdp"))
        .map_err(|e| format!("link chrome-cdp: {e}"))?;
    let out = sandbox
        .cli()
        .args(["skill", "--install"])
        .current_dir(workdir)
        .output()
        .map_err(|e| format!("chrome-cdp skill --install: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "chrome-cdp skill --install failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path)))
        .map_err(|e| e.to_string())
}

/// Runs the agent on `prompt` in `workdir`; its output goes to `log`
/// (the transcript in `<log>.jsonl`, and `<log>.stderr`). `sandbox` is
/// chrome-cdp's session dirs.
pub fn run_agent(
    prompt: &str,
    workdir: &Path,
    sandbox: &Sandbox,
    opts: &AgentOptions,
    timeout: Duration,
    log: &Path,
) -> Result<AgentRun, String> {
    let scratch = sandbox.dir.path().join("agent");
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let path = setup(workdir, &scratch, sandbox, opts)?;
    let prompt = format!("{PREAMBLE}\n\n{}", prompt.trim());

    let mut cmd = Command::new(&opts.claude);
    cmd.arg("-p")
        .arg(&prompt)
        // Every message, for the tool calls; the last is the result.
        .args(["--output-format", "stream-json", "--verbose"])
        .arg("--no-session-persistence")
        // The project's settings (the installed skill) and nothing of the
        // user's: no CLAUDE.md, skills, hooks or MCP servers of theirs.
        .args(["--setting-sources", "project"])
        .arg("--strict-mcp-config")
        .arg("--dangerously-skip-permissions")
        .args(["--tools", TOOLS]);
    if let Some(model) = &opts.model {
        cmd.args(["--model", model]);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CHROME_CDP_") {
            cmd.env_remove(key);
        }
    }
    for (key, value) in sandbox.env() {
        // Claude Code keeps its own config under ~/.claude, but leave
        // XDG_CONFIG_HOME alone all the same.
        if key != "XDG_CONFIG_HOME" {
            cmd.env(key, value);
        }
    }
    let stderr = std::fs::File::create(log.with_extension("stderr")).map_err(|e| e.to_string())?;
    cmd.env("PATH", path)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(stderr);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("run {}: {e}", opts.claude.display()))?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let mut timed_out = false;
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if start.elapsed() > timeout {
            timed_out = true;
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let wall = start.elapsed();
    // The MCP server and browsers it started go with it.
    kill_group(child.id());
    let bytes = reader.join().unwrap_or_default();
    std::fs::write(log.with_extension("jsonl"), &bytes).map_err(|e| e.to_string())?;
    let mut run = parse_transcript(&String::from_utf8_lossy(&bytes));
    run.timed_out = timed_out;
    run.wall_ms = u64::try_from(wall.as_millis()).unwrap_or(u64::MAX);
    Ok(run)
}

/// Kills the process group `pid` leads (a no-op once it is gone).
pub(super) fn kill_group(pid: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn results_sum_every_models_tokens() {
        let out = json!({
            "type": "result", "is_error": false, "num_turns": 7, "result": "ANSWER: 3",
            "total_cost_usd": 0.12,
            "usage": {"input_tokens": 1, "output_tokens": 1},
            "modelUsage": {
                "claude-opus-5-5": {"inputTokens": 10, "outputTokens": 20,
                    "cacheReadInputTokens": 300, "cacheCreationInputTokens": 40},
                "claude-haiku-4-5": {"inputTokens": 5, "outputTokens": 1,
                    "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0}
            }
        });
        let run = parse_result(&out);
        assert_eq!(run.turns, 7);
        assert_eq!(run.reply, "ANSWER: 3");
        assert!(!run.is_error);
        assert_eq!(
            run.tokens,
            Tokens {
                input: 15,
                output: 21,
                cache_read: 300,
                cache_creation: 40
            }
        );
        assert_eq!(run.tokens.total(), 376);
    }

    #[test]
    fn transcripts_count_reading_tools() {
        let lines = [
            json!({"type": "system", "subtype": "init"}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "Looking."},
                {"type": "tool_use", "name": "Bash", "input": {"command": "chrome-cdp source e3"}},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "src/a.tsx"}},
            ]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result"}]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "Grep", "input": {"pattern": "Add"}},
                {"type": "tool_use", "name": "Glob", "input": {"pattern": "**/*.vue"}},
                {"type": "tool_use", "name": "Edit", "input": {}},
                {"type": "tool_use", "name": "Bash", "input": {"command": "cat src/a.tsx | head"}},
            ]}}),
            json!({"type": "result", "is_error": false, "num_turns": 3, "result": "Fixed.",
                "usage": {"input_tokens": 2, "output_tokens": 3}}),
        ];
        let text = lines.map(|l| l.to_string()).join("\n");
        let run = parse_transcript(&text);
        assert_eq!(run.file_reads, 3);
        assert_eq!(run.shell_reads, 1);
        assert_eq!(run.turns, 3);
        assert_eq!(run.reply, "Fixed.");
        assert_eq!(run.tokens.total(), 5);
        // No result: an error, the reads still counted.
        let cut = parse_transcript(text.trim_end().rsplit_once('\n').unwrap().0);
        assert!(cut.is_error);
        assert_eq!(cut.file_reads, 3);
    }

    #[test]
    fn shell_commands_that_read_files() {
        assert_eq!(shell_reads("chrome-cdp source e3"), 0);
        assert_eq!(shell_reads("grep -rn Add src | head -5"), 1);
        assert_eq!(
            shell_reads("cd app && cat src/a.ts; sed -n 1,9p src/b.ts"),
            2
        );
        assert_eq!(shell_reads("LC_ALL=C /usr/bin/rg total\nls src"), 2);
        assert_eq!(shell_reads("npm test || tail log.txt"), 1);
        assert_eq!(shell_reads("echo cat"), 0);
    }

    #[test]
    fn without_model_usage_the_usage_counts() {
        let out = json!({"is_error": false, "usage": {"input_tokens": 3, "output_tokens": 4,
            "cache_read_input_tokens": 5, "cache_creation_input_tokens": 6}});
        assert_eq!(parse_result(&out).tokens.total(), 18);
        assert!(parse_result(&json!({})).is_error);
    }
}

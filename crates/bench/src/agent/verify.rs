//! Checks a task's outcome, independently of the agent's own account of
//! it: what the fixture server was sent, the state of the app afterwards,
//! the working dir's tests. Only answer tasks read the agent's reply.

use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

use serde::Serialize;
use serde_json::Value;

use super::task::{RecordCheck, Task, TestCheck, Vars};
use super::workdir::{self, Files};
use crate::sandbox::Sandbox;

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }
}

#[derive(Debug)]
pub struct Context<'a> {
    pub workdir: &'a Path,
    /// The agent's final reply.
    pub reply: &'a str,
    /// What the fixture servers were sent.
    pub records: &'a [Value],
    pub vars: &'a Vars,
    /// The `unchanged` files as the setup left them.
    pub before: &'a Files,
    pub bin: &'a Path,
    pub chrome: &'a Path,
}

pub fn verify(task: &Task, cx: &Context<'_>) -> Vec<Check> {
    let v = &task.verify;
    let mut out = Vec::new();
    if !v.answer.is_empty() {
        out.push(answer(cx.reply, &v.answer));
    }
    for r in &v.record {
        out.push(record(cx.records, r));
    }
    if let Some(script) = &v.page {
        out.push(page(&cx.vars.expand(script), cx));
    }
    for t in &v.test {
        out.push(test(t, cx));
    }
    if !v.unchanged.is_empty() {
        let now = workdir::files_under(cx.workdir, &v.unchanged);
        let changed: Vec<String> = cx
            .before
            .keys()
            .chain(now.keys())
            .filter(|p| cx.before.get(*p) != now.get(*p))
            .map(|p| {
                p.strip_prefix(cx.workdir)
                    .unwrap_or(p)
                    .display()
                    .to_string()
            })
            .collect();
        out.push(Check::new(
            format!("unchanged {}", v.unchanged.join(", ")),
            changed.is_empty(),
            if changed.is_empty() {
                String::new()
            } else {
                format!("changed: {}", changed.join(", "))
            },
        ));
    }
    out
}

/// The text after the reply's last `ANSWER:` (any case, markdown
/// emphasis and backticks dropped).
pub fn answer_line(reply: &str) -> Option<String> {
    let clean = |s: &str| s.replace(['*', '`', '_'], "");
    reply.lines().rev().find_map(|line| {
        let line = clean(line);
        let at = line.to_lowercase().find("answer:")?;
        Some(line[at + "answer:".len()..].trim().to_owned())
    })
}

/// Lower case, without spaces, commas, `$` and quotes: `$17,449.75`
/// and `17449.75` are the same answer.
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| !matches!(c, ' ' | ',' | '$' | '"' | '\'' | '‘' | '’' | '“' | '”'))
        .collect()
}

fn answer(reply: &str, expected: &[String]) -> Check {
    let name = format!("answer contains {}", expected.join(" + "));
    let Some(line) = answer_line(reply) else {
        return Check::new(name, false, "the reply has no ANSWER: line");
    };
    let got = normalize(&line);
    let missing: Vec<&String> = expected
        .iter()
        .filter(|e| !got.contains(&normalize(e)))
        .collect();
    let detail = format!("ANSWER: {line}");
    Check::new(name, missing.is_empty(), detail)
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_owned(),
        other => other.to_string(),
    }
}

/// Whether `record` has the form and every field `check` wants.
pub fn record_matches(record: &Value, check: &RecordCheck) -> bool {
    record.get("form").map(text_of).as_deref() == Some(check.form.as_str())
        && check.fields.iter().all(|(key, want)| {
            record.get(key).is_some_and(|got| {
                want.as_str() == Some("*") || text_of(got).eq_ignore_ascii_case(&text_of(want))
            })
        })
}

fn record(records: &[Value], check: &RecordCheck) -> Check {
    let name = format!("recorded {}", check.form);
    if records.iter().any(|r| record_matches(r, check)) {
        return Check::new(name, true, "");
    }
    let same_form: Vec<String> = records
        .iter()
        .filter(|r| r.get("form").map(text_of).as_deref() == Some(check.form.as_str()))
        .map(Value::to_string)
        .collect();
    let detail = if same_form.is_empty() {
        format!("no {} record", check.form)
    } else {
        format!("wanted {:?}; got {}", check.fields, same_form.join(", "))
    };
    Check::new(name, false, detail)
}

/// Runs `script` with `chrome-cdp batch` in a session of its own.
fn page(script: &str, cx: &Context<'_>) -> Check {
    let name = "page script passes".to_owned();
    let run = || -> Result<(bool, String), String> {
        let sandbox = Sandbox::new(cx.bin, cx.chrome)?;
        let mut child = sandbox
            .cli()
            .arg("batch")
            .current_dir(cx.workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("run batch: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(script.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.success(), tail(&text, 12)))
    };
    match run() {
        Ok((ok, detail)) => Check::new(name, ok, detail),
        Err(e) => Check::new(name, false, e),
    }
}

/// The last `n` lines of `text`.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `passed` and `failed` of a `chrome-cdp test --json` envelope: `data`
/// when the run passed, the error's `details` when it did not.
pub fn test_counts(envelope: &Value) -> Option<(u64, u64)> {
    let data = envelope
        .get("data")
        .filter(|d| d.get("passed").is_some())
        .or_else(|| envelope.pointer("/error/details"))?;
    Some((data.get("passed")?.as_u64()?, data.get("failed")?.as_u64()?))
}

fn test(check: &TestCheck, cx: &Context<'_>) -> Check {
    let name = if check.args.is_empty() {
        "chrome-cdp test passes".to_owned()
    } else {
        format!("chrome-cdp test {} passes", check.args.join(" "))
    };
    let run = || -> Result<Check, String> {
        let sandbox = Sandbox::new(cx.bin, cx.chrome)?;
        let args: Vec<String> = check.args.iter().map(|a| cx.vars.expand(a)).collect();
        let out = sandbox
            .cli()
            .arg("test")
            .arg("--json")
            .args(&args)
            .current_dir(cx.workdir)
            .output()
            .map_err(|e| format!("run test: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let Some((passed, failed)) = serde_json::from_str::<Value>(&stdout)
            .ok()
            .as_ref()
            .and_then(test_counts)
        else {
            return Ok(Check::new(
                &name,
                false,
                format!(
                    "no test results ({}): {}",
                    out.status,
                    tail(&String::from_utf8_lossy(&out.stderr), 8)
                ),
            ));
        };
        let ok = out.status.success() && failed == 0 && passed >= check.min_passed as u64;
        Ok(Check::new(
            &name,
            ok,
            format!(
                "{passed} passed, {failed} failed (wanted ≥ {} passed)",
                check.min_passed
            ),
        ))
    };
    run().unwrap_or_else(|e| Check::new(name.clone(), false, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_answer_is_the_last_answer_line() {
        let reply = "I looked.\nANSWER: wrong\nOn second thought:\n**Answer:** `$74.50`\n";
        assert_eq!(answer_line(reply).as_deref(), Some("$74.50"));
        assert_eq!(answer_line("no line"), None);
        assert!(answer(reply, &["$74.50".into()]).ok);
        assert!(!answer(reply, &["wrong".into()]).ok);
        assert!(answer("ANSWER: 17449.75", &["$17,449.75".into()]).ok);
        assert!(answer("Answer: \"Go to Checkout\"", &["go to checkout".into()]).ok);
    }

    #[test]
    fn records_match_by_form_and_fields() {
        let check = RecordCheck {
            form: "login".into(),
            fields: [
                ("email".to_owned(), json!("Ada@Example.com")),
                ("n".to_owned(), json!(3)),
                ("any".to_owned(), json!("*")),
            ]
            .into(),
        };
        assert!(record_matches(
            &json!({"form": "login", "email": "ada@example.com", "n": "3", "any": false}),
            &check
        ));
        assert!(!record_matches(
            &json!({"form": "login", "email": "ada@example.com", "n": 3}),
            &check
        ));
        assert!(!record_matches(
            &json!({"form": "other", "email": "ada@example.com", "n": 3, "any": 1}),
            &check
        ));
        let failed = record(&[json!({"form": "login", "email": "x"})], &check);
        assert!(!failed.ok);
        assert!(failed.detail.contains("got {"), "{}", failed.detail);
    }

    #[test]
    fn test_counts_from_either_envelope() {
        let ok = json!({"ok": true, "data": {"passed": 3, "failed": 0}});
        let failed = json!({"ok": false, "error": {"details": {"passed": 2, "failed": 1}}});
        assert_eq!(test_counts(&ok), Some((3, 0)));
        assert_eq!(test_counts(&failed), Some((2, 1)));
        assert_eq!(test_counts(&json!({"ok": false, "error": {}})), None);
    }
}

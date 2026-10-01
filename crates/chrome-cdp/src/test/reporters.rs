//! `test --reporter` (plan 11, milestone 11.7): files written at the end
//! of a run besides what stdout shows. `json` is the `--json` data,
//! `junit` a JUnit XML file for CI, and `html` one self-contained page a
//! human opens: a timeline of each test's steps (with `--trace`, a
//! screenshot per step), its failure and its request journal. `text` is
//! stdout, which is always written.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use protocol::schema::{TestResult, TestRunData};
use protocol::{ErrorInfo, ErrorKind, Mode, Style};

use super::artifacts::{self, StepRecord};

/// A reporter named by `--reporter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    Json,
    Junit,
    Html,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Junit => "junit",
            Self::Html => "html",
        }
    }

    /// Its file in the artifacts directory, unless `name=path` says.
    fn file(self) -> &'static str {
        match self {
            Self::Text => "",
            Self::Json => "results.json",
            Self::Junit => "junit.xml",
            Self::Html => "report.html",
        }
    }
}

/// `--reporter`'s `name` or `name=path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reporter {
    pub(crate) kind: Kind,
    pub(crate) path: Option<PathBuf>,
}

/// Parses `name[=path]`.
pub(crate) fn parse(value: &str) -> Result<Reporter, String> {
    let (name, path) = match value.split_once('=') {
        Some((name, path)) if !path.is_empty() => (name, Some(PathBuf::from(path))),
        Some((name, _)) => return Err(format!("{name}= needs a file after the =")),
        None => (value, None),
    };
    let kind = match name {
        "text" => Kind::Text,
        "json" => Kind::Json,
        "junit" => Kind::Junit,
        "html" => Kind::Html,
        _ => {
            return Err(format!(
                "unknown reporter {name:?}: text, json, junit or html (a file: junit=out/junit.xml)"
            ));
        }
    };
    if kind == Kind::Text && path.is_some() {
        return Err("text goes to stdout; it takes no file".into());
    }
    Ok(Reporter { kind, path })
}

/// Writes the reporters' files; returns `(reporter, absolute path)` of
/// each.
pub(super) fn write(
    reporters: &[Reporter],
    data: &TestRunData,
    artifacts_dir: &Path,
) -> Result<Vec<(String, String)>, ErrorInfo> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut written = Vec::new();
    for reporter in reporters {
        if reporter.kind == Kind::Text {
            continue;
        }
        let path = match &reporter.path {
            Some(path) => cwd.join(path),
            None => artifacts_dir.join(reporter.kind.file()),
        };
        let text = match reporter.kind {
            Kind::Text => continue,
            Kind::Json => serde_json::to_string_pretty(data).unwrap_or_default() + "\n",
            Kind::Junit => junit(data),
            Kind::Html => html(data),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| cannot_write(&path, &e))?;
        }
        std::fs::write(&path, crate::output::mask(&text).as_bytes())
            .map_err(|e| cannot_write(&path, &e))?;
        written.push((reporter.kind.name().to_owned(), path.display().to_string()));
    }
    Ok(written)
}

fn cannot_write(path: &Path, e: &std::io::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("cannot write the report {}: {e}", path.display()),
    )
}

/// `&`, `<`, `>`, `"` and `'` as entities, and control characters XML
/// does not allow left out.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\t' | '\r' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// A failure as the text output shows it.
fn failure_text(test: &TestResult) -> String {
    let Some(failure) = &test.error else {
        return String::new();
    };
    let mut text = format!("at {}: {}\n", failure.at, failure.step);
    for via in &failure.via {
        let _ = writeln!(text, "  included by {via}");
    }
    let rendered = protocol::render(Mode::Text, Err(failure.error.clone()), Style::new(false));
    text.push_str(&rendered.stderr);
    if let Some(report) = &failure.report {
        let _ = writeln!(text, "report: {report}");
    }
    if let Some(flake) = &test.flake {
        text.push_str(&super::flake::text(flake));
    }
    text
}

/// JUnit XML: a `<testsuite>` per scenario file. A flaky test passes with
/// its first failure as a `<flakyFailure>` (as Maven Surefire writes
/// reruns); skipped and not-run tests are `<skipped>`.
pub(super) fn junit(data: &TestRunData) -> String {
    // A suite per file, and per backend with `--backend all`.
    let mut files: Vec<(Option<&str>, &str)> = Vec::new();
    for test in &data.tests {
        let key = (test.backend.as_deref(), test.file.as_str());
        if !files.contains(&key) {
            files.push(key);
        }
    }
    let count =
        |tests: &[&TestResult], status: &str| tests.iter().filter(|t| t.status == status).count();
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let all: Vec<&TestResult> = data.tests.iter().collect();
    let _ = writeln!(
        xml,
        "<testsuites name=\"chrome-cdp test\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{}\">",
        all.len(),
        count(&all, "failed"),
        count(&all, "skipped") + count(&all, "not-run"),
        seconds(data.duration_ms)
    );
    for (backend, file) in files {
        let tests: Vec<&TestResult> = data
            .tests
            .iter()
            .filter(|t| t.file == file && t.backend.as_deref() == backend)
            .collect();
        let suite = match backend {
            Some(backend) => format!("{backend}: {file}"),
            None => file.to_owned(),
        };
        let _ = writeln!(
            xml,
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{}\">",
            escape(&suite),
            tests.len(),
            count(&tests, "failed"),
            count(&tests, "skipped") + count(&tests, "not-run"),
            seconds(tests.iter().map(|t| t.duration_ms).sum())
        );
        for test in tests {
            let _ = write!(
                xml,
                "    <testcase name=\"{}\" classname=\"{}\" file=\"{}\" line=\"{}\" time=\"{}\"",
                escape(&test.name),
                escape(&suite),
                escape(file),
                test.line,
                seconds(test.duration_ms)
            );
            let mut body = String::new();
            let message = test
                .error
                .as_ref()
                .map(|f| escape(&f.error.message))
                .unwrap_or_default();
            let kind = test
                .error
                .as_ref()
                .map(|f| format!("{:?}", f.error.kind))
                .unwrap_or_default();
            match test.status.as_str() {
                "failed" => {
                    let _ = writeln!(
                        body,
                        "      <failure message=\"{message}\" type=\"{kind}\">{}</failure>",
                        escape(&failure_text(test))
                    );
                }
                "flaky" => {
                    let _ = writeln!(
                        body,
                        "      <flakyFailure message=\"{message}\" type=\"{kind}\">{}</flakyFailure>",
                        escape(&failure_text(test))
                    );
                }
                "skipped" | "not-run" => {
                    let _ = writeln!(
                        body,
                        "      <skipped message=\"{}\"/>",
                        escape(test.reason.as_deref().unwrap_or_default())
                    );
                }
                _ => {}
            }
            let mut out = String::new();
            if let Some(dir) = &test.artifacts {
                let _ = writeln!(out, "artifacts: {dir}");
            }
            for warning in &test.warnings {
                let _ = writeln!(out, "warning: {warning}");
            }
            if !out.is_empty() {
                let _ = writeln!(body, "      <system-out>{}</system-out>", escape(&out));
            }
            if body.is_empty() {
                xml.push_str("/>\n");
            } else {
                let _ = write!(xml, ">\n{body}    </testcase>\n");
            }
        }
        xml.push_str("  </testsuite>\n");
    }
    xml.push_str("</testsuites>\n");
    xml
}

/// Reads a test's `steps.jsonl`.
fn steps(dir: &Path) -> Vec<StepRecord> {
    std::fs::read_to_string(dir.join(artifacts::STEPS))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// A PNG as a `data:` URL, if it can be read.
fn image(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

const STYLE: &str = "\
body{font:14px/1.4 system-ui,sans-serif;margin:2em;color:#222}\
h1{font-size:1.4em}summary{cursor:pointer;padding:.3em 0}\
.passed{color:#1a7f37}.failed{color:#cf222e}.flaky{color:#9a6700}.skipped,.not-run{color:#6e7781}\
table{border-collapse:collapse;margin:.5em 0}td,th{border-bottom:1px solid #ddd;padding:.2em .6em;text-align:left;vertical-align:top}\
td.n{text-align:right;color:#6e7781}pre{background:#f6f8fa;padding:.6em;overflow:auto;white-space:pre-wrap}\
img{max-width:360px;border:1px solid #ccc}img.big{max-width:min(100%,800px)}.test{margin-left:1em}";

/// One self-contained page for a human: every test, its steps as a
/// timeline, its screenshots and its request journal.
pub(super) fn html(data: &TestRunData) -> String {
    let mut page = String::from("<!doctype html>\n<html><head><meta charset=\"utf-8\">");
    let _ = write!(
        page,
        "<title>chrome-cdp test</title><style>{STYLE}</style></head><body>\n<h1>chrome-cdp test</h1>\n<p>"
    );
    let _ = writeln!(
        page,
        "{} tests: <span class=passed>{} passed</span>, <span class=failed>{} failed</span>, \
         <span class=flaky>{} flaky</span>, <span class=skipped>{} skipped</span>, {} not run ({})</p>",
        data.tests.len(),
        data.passed,
        data.failed,
        data.flaky,
        data.skipped,
        data.not_run,
        protocol::format::duration(std::time::Duration::from_millis(data.duration_ms))
    );
    for test in &data.tests {
        let icon = match test.status.as_str() {
            "passed" => "✓",
            "failed" => "✗",
            "flaky" => "~",
            _ => "-",
        };
        let open = if test.status == "failed" { " open" } else { "" };
        let _ = write!(
            page,
            "<details{open}><summary class=\"{}\">{icon} {}:{} {} ({}{})</summary>\n<div class=test>\n",
            escape(&test.status),
            escape(&test.file),
            test.line,
            escape(&test.name),
            escape(&test.status),
            if test.duration_ms > 0 {
                format!(
                    ", {}",
                    protocol::format::duration(std::time::Duration::from_millis(test.duration_ms))
                )
            } else {
                String::new()
            }
        );
        if let Some(reason) = &test.reason {
            let _ = writeln!(page, "<p>{}</p>", escape(reason));
        }
        if test.error.is_some() {
            let _ = writeln!(page, "<pre>{}</pre>", escape(&failure_text(test)));
        }
        for warning in &test.warnings {
            let _ = writeln!(page, "<p>warning: {}</p>", escape(warning));
        }
        if let Some(dir) = test.artifacts.as_deref().map(Path::new) {
            test_body(&mut page, dir);
        }
        page.push_str("</div></details>\n");
    }
    page.push_str("</body></html>\n");
    page
}

/// A test's timeline, screenshots and journals, from its artifacts.
fn test_body(page: &mut String, dir: &Path) {
    let steps = steps(dir);
    if !steps.is_empty() {
        page.push_str("<table><tr><th>#</th><th>at</th><th>took</th><th>step</th><th></th></tr>\n");
        for step in &steps {
            let _ = write!(
                page,
                "<tr><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=\"{}\">{} {}{}<br><small>{}</small>",
                step.n,
                seconds(step.start_ms),
                protocol::format::duration(std::time::Duration::from_millis(step.duration_ms)),
                if step.ok { "passed" } else { "failed" },
                if step.ok { "✓" } else { "✗" },
                escape(&step.command),
                step.hook
                    .as_deref()
                    .map(|h| format!(" <small>[{}]</small>", escape(h)))
                    .unwrap_or_default(),
                escape(&step.at)
            );
            if let Some(error) = &step.error {
                let _ = write!(page, "<pre>{}</pre>", escape(&error.message));
            } else if !step.output.is_empty() {
                let _ = write!(page, "<pre>{}</pre>", escape(&step.output));
            }
            page.push_str("</td><td>");
            if let Some(src) = step.screenshot.as_deref().and_then(|s| image(&dir.join(s))) {
                let _ = write!(
                    page,
                    "<img loading=lazy src=\"{src}\" alt=\"after step {}\">",
                    step.n
                );
            }
            page.push_str("</td></tr>\n");
        }
        page.push_str("</table>\n");
    }
    if let Some(src) = image(&dir.join(artifacts::SCREENSHOT)) {
        let _ = writeln!(
            page,
            "<p>When it failed:</p><img class=big src=\"{src}\" alt=\"the page when it failed\">"
        );
    }
    for (file, title) in [
        (artifacts::REQUESTS, "Requests"),
        (artifacts::CONSOLE, "Console"),
        (artifacts::SNAPSHOT, "Snapshot when it failed"),
    ] {
        if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
            let _ = writeln!(
                page,
                "<details><summary>{title}</summary><pre>{}</pre></details>",
                escape(&text)
            );
        }
    }
    let _ = writeln!(
        page,
        "<p><small>{}</small></p>",
        escape(&dir.display().to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::schema::TestFailure;

    fn result(name: &str, status: &str) -> TestResult {
        TestResult {
            file: "e2e/a.cdp".into(),
            line: 3,
            name: name.into(),
            status: status.into(),
            tags: Vec::new(),
            duration_ms: 1234,
            tries: 1,
            failed_runs: None,
            warnings: Vec::new(),
            reason: (status == "skipped").then(|| "not <ready>".into()),
            artifacts: None,
            error: (status == "failed" || status == "flaky").then(|| TestFailure {
                at: "e2e/a.cdp:4:3".into(),
                step: "click text=Buy".into(),
                hook: None,
                via: Vec::new(),
                error: ErrorInfo::new(ErrorKind::NotFound, "no element matches text=Buy & more"),
                report: Some("/r/report.md".into()),
                likely: None,
            }),
            backend: None,
            healed: Vec::new(),
            flake: None,
        }
    }

    fn data(tests: Vec<TestResult>) -> TestRunData {
        TestRunData {
            passed: tests.iter().filter(|t| t.status == "passed").count(),
            failed: tests.iter().filter(|t| t.status == "failed").count(),
            flaky: tests.iter().filter(|t| t.status == "flaky").count(),
            skipped: tests.iter().filter(|t| t.status == "skipped").count(),
            not_run: 0,
            duration_ms: 4200,
            shard: None,
            listed: false,
            repeat: None,
            reporters: std::collections::BTreeMap::new(),
            debug: None,
            backend: None,
            backends: Vec::new(),
            verify: None,
            recorded: Vec::new(),
            heal: None,
            tests,
        }
    }

    #[test]
    fn reporters_parse_with_an_optional_file() {
        assert_eq!(
            parse("junit=out/j.xml"),
            Ok(Reporter {
                kind: Kind::Junit,
                path: Some("out/j.xml".into())
            })
        );
        assert_eq!(parse("html").unwrap().path, None);
        assert!(parse("xml").unwrap_err().contains("unknown reporter"));
        assert!(parse("text=x").is_err());
        assert!(parse("json=").is_err());
    }

    #[test]
    fn junit_has_a_suite_per_file_and_escapes_text() {
        let mut flakes = result("flakes", "flaky");
        flakes.flake = Some(protocol::schema::TestFlake {
            cause: "race".into(),
            summary: "requests raced".into(),
            confidence: 0.85,
            evidence: vec!["`GET /a` was answered before `GET /b`".into()],
            hint: None,
            runs: 2,
            failed_runs: 1,
            at: Some("e2e/a.cdp:4:3".into()),
            step: Some("click text=Buy".into()),
            others: Vec::new(),
        });
        let xml = junit(&data(vec![
            result("passes", "passed"),
            result("fails", "failed"),
            result("is skipped", "skipped"),
            flakes,
        ]));
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"chrome-cdp test\" tests=\"4\" failures=\"1\" skipped=\"1\" time=\"4.200\">\n"), "{xml}");
        assert!(xml.contains("  <testsuite name=\"e2e/a.cdp\" tests=\"4\" failures=\"1\" skipped=\"1\" time=\"4.936\">\n"), "{xml}");
        assert!(xml.contains("    <testcase name=\"passes\" classname=\"e2e/a.cdp\" file=\"e2e/a.cdp\" line=\"3\" time=\"1.234\"/>\n"), "{xml}");
        assert!(xml.contains("<failure message=\"no element matches text=Buy &amp; more\" type=\"NotFound\">at e2e/a.cdp:4:3: click text=Buy\nerror: no element matches text=Buy &amp; more\nreport: /r/report.md\n</failure>"), "{xml}");
        assert!(
            xml.contains("<skipped message=\"not &lt;ready&gt;\"/>"),
            "{xml}"
        );
        assert!(xml.contains("<flakyFailure message="), "{xml}");
        assert!(
            xml.contains("    likely cause: requests raced (confidence 0.85)\n      `GET /a` was answered before `GET /b`\n</flakyFailure>"),
            "{xml}"
        );
        assert!(xml.ends_with("  </testsuite>\n</testsuites>\n"), "{xml}");
    }

    #[test]
    fn html_shows_every_test_with_its_steps() {
        let dir = tempfile::tempdir().unwrap();
        let step = StepRecord {
            n: 1,
            start_ms: 5,
            duration_ms: 20,
            at: "e2e/a.cdp:4:3".into(),
            command: "click <b>".into(),
            hook: None,
            via: Vec::new(),
            ok: true,
            output: String::new(),
            warnings: Vec::new(),
            error: None,
            screenshot: Some("trace/001.png".into()),
            masked: 0,
            healed: None,
        };
        std::fs::create_dir_all(dir.path().join("trace")).unwrap();
        std::fs::write(dir.path().join("trace/001.png"), b"png").unwrap();
        std::fs::write(
            dir.path().join(artifacts::STEPS),
            serde_json::to_string(&step).unwrap() + "\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(artifacts::REQUESTS),
            "== t1\n#1 GET 200 /a\n",
        )
        .unwrap();
        let mut failed = result("fails", "failed");
        failed.artifacts = Some(dir.path().display().to_string());
        let page = html(&data(vec![result("passes", "passed"), failed]));
        assert!(page.starts_with("<!doctype html>"), "{page}");
        assert!(
            page.contains(
                "<details><summary class=\"passed\">✓ e2e/a.cdp:3 passes (passed, 1.2s)</summary>"
            ),
            "{page}"
        );
        assert!(
            page.contains("<details open><summary class=\"failed\">✗ e2e/a.cdp:3 fails"),
            "{page}"
        );
        assert!(page.contains("✓ click &lt;b&gt;"), "{page}");
        assert!(
            page.contains("src=\"data:image/png;base64,cG5n\""),
            "{page}"
        );
        assert!(
            page.contains("<summary>Requests</summary><pre>== t1\n#1 GET 200 /a\n</pre>"),
            "{page}"
        );
        assert!(!page.contains("<summary>Console</summary>"), "{page}");
    }
}

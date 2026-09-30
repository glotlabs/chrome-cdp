//! `report.md` (plan 11, milestone 11.7): the failure summary an agent
//! reads instead of running the test again. It is small and stands on its
//! own: the failing step and its line, the error with its hint, the steps
//! before it, the part of the page around the step's target, console
//! errors, the requests that went wrong, and where the other artifacts
//! are.

use std::fmt::Write as _;
use std::path::Path;

use protocol::schema::TestFailure;
use protocol::{Mode, Style};

use super::artifacts::{self, Capture, Logs, StepRecord};

/// How many steps before the failing one the report shows.
const BEFORE: usize = 3;
/// Snapshot lines shown on each side of the target.
const AROUND: usize = 6;
/// Snapshot lines shown when no target is found.
const TOP: usize = 30;
/// Console errors and requests listed at most.
const LISTED: usize = 10;
/// How long a listed line may be.
const LINE_CHARS: usize = 300;

/// What a report is about.
pub(super) struct Subject<'a> {
    /// The test's name, or the hook's (`@before-all`).
    pub(super) title: &'a str,
    /// `file.cdp:12`.
    pub(super) place: &'a str,
    /// `try 2 of 3`, `run 4 of 20`, when there were several.
    pub(super) attempt: Option<String>,
    /// The command that runs the test again.
    pub(super) rerun: String,
}

/// The text of `report.md`.
pub(super) fn render(
    subject: &Subject<'_>,
    failure: &TestFailure,
    steps: &[StepRecord],
    capture: Option<&Capture>,
    logs: &Logs,
    dir: &Path,
) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Failed: {}\n", subject.title);
    let _ = write!(md, "`{}`", subject.place);
    if let Some(attempt) = &subject.attempt {
        let _ = write!(md, " ({attempt})");
    }
    md.push_str("\n\n## Failing step\n\n");
    let hook = match failure.hook.as_deref() {
        Some("setup") => " (the runner setting up the test)".to_owned(),
        Some("check") => " (the runner's check after the test's steps)".to_owned(),
        Some(hook) => format!(" (in @{hook})"),
        None => String::new(),
    };
    let _ = writeln!(md, "`{}`{hook}:\n", failure.at);
    let _ = writeln!(md, "    {}", failure.step);
    for via in &failure.via {
        let _ = writeln!(md, "\nincluded by `{via}`");
    }
    let rendered = protocol::render(Mode::Text, Err(failure.error.clone()), Style::new(false));
    md.push_str("\n```\n");
    md.push_str(rendered.stderr.trim_end());
    md.push_str("\n```\n");

    // The steps before it, and it.
    let failed = steps.iter().rposition(|s| !s.ok);
    let end = failed.map_or(steps.len(), |i| i + 1);
    let start = end.saturating_sub(BEFORE + 1);
    if end > 0 {
        let _ = writeln!(
            md,
            "\n## Last steps\n\n{} step{} ran{}.\n",
            steps.len(),
            if steps.len() == 1 { "" } else { "s" },
            if start > 0 {
                format!("; the {} before the failing one", end - start - 1)
            } else {
                String::new()
            }
        );
        md.push_str("```\n");
        for step in &steps[start..end] {
            let _ = writeln!(md, "{}", step_line(step));
        }
        md.push_str("```\n");
    }

    if let Some(likely) = &failure.likely {
        md.push_str("\n## Likely the same element\n\n");
        md.push_str(
            "The step's locator finds no element now (or several). The element its \
             fingerprint (e2e/fingerprints) matches best:\n\n",
        );
        let _ = writeln!(
            md,
            "    {}\n\nconfidence {:.2} ({}): {}.",
            likely.locator.as_deref().unwrap_or(&likely.element),
            likely.confidence,
            likely.level,
            super::heal::reasons_text(&likely.reasons)
        );
        md.push_str(match likely.level.as_str() {
            "high" => "Run the test with --heal to use it, or change the locator.\n",
            "medium" => "Run the test with --heal=medium to use it, or change the locator.\n",
            _ => "Too low to heal: the element may be gone. Change the step.\n",
        });
    }
    let healed: Vec<&protocol::schema::TestHeal> =
        steps.iter().filter_map(|s| s.healed.as_ref()).collect();
    if !healed.is_empty() {
        md.push_str(
            "\n## Healed steps\n\nThese steps' locators found nothing, and they ran with \
             the element their fingerprint matched (--heal). The test failed after, so \
             the heals are suspect, and heal.patch leaves them out.\n\n",
        );
        for h in healed {
            let _ = writeln!(
                md,
                "- `{}:{}`: `{}` -> `{}` (confidence {:.2}: {})",
                h.file,
                h.line,
                h.from,
                h.to,
                h.confidence,
                super::heal::reasons_text(&h.reasons)
            );
        }
    }

    md.push_str("\n## Page\n\n");
    match capture {
        None => md.push_str("Not captured: the failure came before a page was open.\n"),
        Some(capture) => {
            match &capture.snapshot {
                Ok(snapshot) => {
                    let needles = needles(&failure.step, &failure.error);
                    let (excerpt, found) = excerpt(snapshot, &needles);
                    let total = snapshot.lines().count();
                    let _ = match found {
                        Some(needle) => writeln!(
                            md,
                            "`snapshot -i` when it failed, around `{needle}` ({} of {total} lines):\n",
                            excerpt.len()
                        ),
                        None => writeln!(
                            md,
                            "`snapshot -i` when it failed ({} of {total} lines):\n",
                            excerpt.len()
                        ),
                    };
                    md.push_str("```\n");
                    for line in excerpt {
                        let _ = writeln!(md, "{line}");
                    }
                    md.push_str("```\n");
                }
                Err(e) => {
                    let _ = writeln!(md, "No snapshot: {e}");
                }
            }
            if let Err(e) = &capture.screenshot {
                let _ = writeln!(md, "\nNo screenshot: {e}");
            }
        }
    }
    // Secrets typed into ordinary fields are painted over in screenshots.
    let masked = capture.map_or(0, |c| c.masked);
    let traced = steps.iter().filter(|s| s.masked > 0).count();
    if masked > 0 || traced > 0 {
        let plural = |n: usize, one: &str, many: &str| {
            if n == 1 {
                format!("1 {one}")
            } else {
                format!("{n} {many}")
            }
        };
        let mut shots = Vec::new();
        if masked > 0 {
            shots.push(format!(
                "{} in {}",
                plural(masked, "field", "fields"),
                artifacts::SCREENSHOT
            ));
        }
        if traced > 0 {
            shots.push(format!(
                "in {}",
                plural(traced, "`--trace` screenshot", "`--trace` screenshots")
            ));
        }
        let _ = writeln!(
            md,
            "\nPainted over because they hold a `${{secret:…}}` value: {}.",
            shots.join(", and ")
        );
    }

    list(&mut md, "Console errors", &logs.errors);
    list(&mut md, "Failed and unmatched requests", &logs.bad_requests);

    md.push_str("\n## Files\n\n");
    let files = [
        (artifacts::SCREENSHOT, "the page when it failed"),
        (artifacts::SNAPSHOT, "all of `snapshot -i`"),
        (artifacts::STEPS, "every step, with its output"),
        (
            artifacts::REQUESTS,
            "every request and the mock that answered it",
        ),
        (artifacts::CONSOLE, "the console of every tab"),
        (artifacts::TRACE, "a screenshot after every step (--trace)"),
    ];
    for (name, what) in files {
        let path = dir.join(name);
        if path.exists() {
            let _ = writeln!(md, "- {}: {what}", path.display());
        }
    }
    let _ = writeln!(md, "\nRun it again: `{}`", subject.rerun);
    md
}

/// `✓ file.cdp:4:3 goto / (240ms)`, with the hook and a failure's
/// message.
fn step_line(step: &StepRecord) -> String {
    let mut line = format!(
        "{} {} {} ({})",
        if step.ok { "✓" } else { "✗" },
        step.at,
        step.command,
        protocol::format::duration(std::time::Duration::from_millis(step.duration_ms))
    );
    if let Some(hook) = &step.hook {
        let _ = write!(line, " [{hook}]");
    }
    for warning in &step.warnings {
        let _ = write!(line, "\n    warning: {warning}");
    }
    artifacts::cut(&line, LINE_CHARS * 2)
}

fn list(md: &mut String, title: &str, lines: &[String]) {
    let _ = writeln!(md, "\n## {title}\n");
    if lines.is_empty() {
        md.push_str("None.\n");
        return;
    }
    for line in lines.iter().take(LISTED) {
        let _ = writeln!(md, "- {}", artifacts::cut(line, LINE_CHARS));
    }
    if lines.len() > LISTED {
        let _ = writeln!(md, "- … {} more", lines.len() - LISTED);
    }
}

/// What to look for in the snapshot: the refs the error's message names,
/// the step's locators, then the refs of the error's notes.
fn needles(step: &str, error: &protocol::ErrorInfo) -> Vec<String> {
    let mut needles = Vec::new();
    let mut add = |needle: String| {
        if !needles.contains(&needle) {
            needles.push(needle);
        }
    };
    let refs = |text: &str| -> Vec<String> {
        text.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| is_ref(w))
            .map(|w| format!("[{w}]"))
            .collect()
    };
    refs(&error.message).into_iter().for_each(&mut add);
    for word in words(step).iter().skip(1) {
        let word = word.trim_start_matches('@');
        if is_ref(word) {
            add(format!("[{word}]"));
        } else if let Some(name) = locator_name(word) {
            add(name);
        }
    }
    for note in &error.notes {
        refs(&note.text).into_iter().for_each(&mut add);
    }
    needles
}

/// `step`'s words, quotes taken off (a shell's rules, roughly: enough to
/// find a locator).
fn words(step: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in step.chars() {
        match (quote, c) {
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

fn is_ref(word: &str) -> bool {
    word.len() >= 2 && word.starts_with('e') && word[1..].chars().all(|c| c.is_ascii_digit())
}

/// The text a locator looks for: `text=Buy` → `Buy`,
/// `role=button[name="Buy now"]` → `Buy now`, `label=Email` → `Email`.
fn locator_name(word: &str) -> Option<String> {
    let word = word.split(">>").next().unwrap_or(word).trim();
    if let Some(at) = word.find("name=") {
        let rest = word[at + 5..].trim_start_matches(['"', '\'']);
        let end = rest.find(['"', '\'', ']']).unwrap_or(rest.len());
        let name = &rest[..end];
        return (!name.is_empty()).then(|| name.to_owned());
    }
    for prefix in [
        "text=",
        "label=",
        "placeholder=",
        "testid=",
        "alt=",
        "title=",
    ] {
        if let Some(name) = word.strip_prefix(prefix) {
            let name = name.trim_matches(['"', '\'', '/']);
            return (!name.is_empty()).then(|| name.to_owned());
        }
    }
    None
}

/// The snapshot lines around the first needle found (the lines of each
/// match, with context, for up to 3 matches), or its top.
fn excerpt<'a>(snapshot: &'a str, needles: &[String]) -> (Vec<&'a str>, Option<String>) {
    let lines: Vec<&str> = snapshot.lines().collect();
    for needle in needles {
        let lower = needle.to_lowercase();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.to_lowercase().contains(&lower))
            .map(|(i, _)| i)
            .take(3)
            .collect();
        if hits.is_empty() {
            continue;
        }
        let mut shown: Vec<usize> = Vec::new();
        for hit in hits {
            let from = hit.saturating_sub(AROUND);
            let to = (hit + AROUND + 1).min(lines.len());
            for i in from..to {
                if !shown.contains(&i) {
                    shown.push(i);
                }
            }
        }
        shown.sort_unstable();
        let mut out = Vec::new();
        // The page line first, so the excerpt says which page it is.
        if shown.first() != Some(&0) {
            out.push(lines[0]);
            out.push("…");
        }
        let mut last = None;
        for i in shown {
            if last.is_some_and(|l: usize| i > l + 1) {
                out.push("…");
            }
            out.push(lines[i]);
            last = Some(i);
        }
        if last.is_some_and(|l| l + 1 < lines.len()) {
            out.push("…");
        }
        return (out, Some(needle.clone()));
    }
    let mut out: Vec<&str> = lines.iter().take(TOP).copied().collect();
    if lines.len() > TOP {
        out.push("…");
    }
    (out, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ErrorInfo, ErrorKind};

    fn step(n: usize, ok: bool, command: &str) -> StepRecord {
        StepRecord {
            n,
            start_ms: 0,
            duration_ms: 12,
            at: format!("a.cdp:{n}:3"),
            command: command.to_owned(),
            hook: None,
            via: Vec::new(),
            ok,
            output: String::new(),
            warnings: Vec::new(),
            error: None,
            screenshot: None,
            masked: 0,
            healed: None,
        }
    }

    #[test]
    fn steps_split_into_words() {
        assert_eq!(
            words(r#"click 'role=button[name="Buy now"]' --force"#),
            ["click", r#"role=button[name="Buy now"]"#, "--force"]
        );
        assert_eq!(
            words(r#"fill label=Email "a b""#),
            ["fill", "label=Email", "a b"]
        );
    }

    #[test]
    fn locators_give_the_names_they_look_for() {
        assert_eq!(locator_name("text=Buy"), Some("Buy".into()));
        assert_eq!(
            locator_name(r#"role=button[name="Buy now"]"#),
            Some("Buy now".into())
        );
        assert_eq!(locator_name("label=Email >> nth=1"), Some("Email".into()));
        assert_eq!(locator_name("#submit"), None);
    }

    #[test]
    fn the_excerpt_is_around_the_target() {
        let mut snapshot = String::from("page t1 \"Shop\" http://x/\n");
        for i in 0..40 {
            let _ = writeln!(snapshot, "  - listitem \"item {i}\" [e{}]", i + 2);
        }
        snapshot.push_str("  - button \"Buy now\" [e50]\n");
        let (lines, found) = excerpt(&snapshot, &["Buy now".to_owned()]);
        assert_eq!(found.as_deref(), Some("Buy now"));
        assert_eq!(lines[0], "page t1 \"Shop\" http://x/");
        assert_eq!(lines[1], "…");
        assert!(lines.last().unwrap().contains("Buy now"));
        assert_eq!(lines.len(), 2 + AROUND + 1);
        // No target: the top of the page.
        let (lines, found) = excerpt(&snapshot, &["Nowhere".to_owned()]);
        assert_eq!(found, None);
        assert_eq!(lines.len(), TOP + 1);
    }

    #[test]
    fn a_report_names_the_step_the_error_and_the_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(artifacts::STEPS), "").unwrap();
        let failure = TestFailure {
            at: "a.cdp:6:3".into(),
            step: "click 'role=button[name=\"Buy now\"]'".into(),
            hook: None,
            via: Vec::new(),
            error: ErrorInfo::new(
                ErrorKind::Obscured,
                "element e50 is covered by another element",
            )
            .note("covering", "div \"We use cookies\" [e88]")
            .hint("chrome-cdp click e89"),
            report: None,
            likely: None,
        };
        let steps: Vec<StepRecord> = (1..=5)
            .map(|n| step(n, n < 5, &format!("step {n}")))
            .collect();
        let capture = Capture {
            screenshot: Err("a dialog is open".into()),
            masked: 0,
            snapshot: Ok(
                "page t1 \"Shop\" http://x/\n  - button \"Buy now\" [e50]\n  - dialog [e88]".into(),
            ),
        };
        let logs = Logs {
            errors: vec!["t1 boom".into()],
            bad_requests: Vec::new(),
        };
        let subject = Subject {
            title: "buys",
            place: "a.cdp:4",
            attempt: None,
            rerun: "chrome-cdp test a.cdp:4".into(),
        };
        let md = render(
            &subject,
            &failure,
            &steps,
            Some(&capture),
            &logs,
            dir.path(),
        );
        assert!(md.starts_with("# Failed: buys\n\n`a.cdp:4`\n"), "{md}");
        assert!(
            md.contains("`a.cdp:6:3`:\n\n    click 'role=button[name=\"Buy now\"]'\n"),
            "{md}"
        );
        assert!(md.contains("error: element e50 is covered by another element\n  covering: div \"We use cookies\" [e88]\n  hint: chrome-cdp click e89\n```"), "{md}");
        assert!(
            md.contains(
                "5 steps ran; the 3 before the failing one.\n\n```\n✓ a.cdp:2:3 step 2 (12ms)\n"
            ),
            "{md}"
        );
        assert!(md.contains("✗ a.cdp:5:3 step 5 (12ms)\n```"), "{md}");
        assert!(md.contains("around `[e50]`"), "{md}");
        assert!(md.contains("No screenshot: a dialog is open"), "{md}");
        assert!(md.contains("## Console errors\n\n- t1 boom\n"), "{md}");
        assert!(
            md.contains("## Failed and unmatched requests\n\nNone.\n"),
            "{md}"
        );
        assert!(
            md.contains(&format!(
                "- {}: every step",
                dir.path().join("steps.jsonl").display()
            )),
            "{md}"
        );
        assert!(!md.contains("failure.png"), "{md}");
        assert!(
            md.ends_with("Run it again: `chrome-cdp test a.cdp:4`\n"),
            "{md}"
        );
    }

    #[test]
    fn a_report_says_which_screenshots_hide_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let failure = TestFailure {
            at: "a.cdp:3:3".into(),
            step: "click text=Go".into(),
            hook: None,
            via: Vec::new(),
            error: ErrorInfo::new(ErrorKind::NotFound, "nothing matches text=Go"),
            report: None,
            likely: None,
        };
        let mut steps: Vec<StepRecord> = (1..=3).map(|n| step(n, n < 3, "x")).collect();
        steps[1].masked = 1;
        steps[2].masked = 1;
        let capture = Capture {
            screenshot: Ok(dir.path().join(artifacts::SCREENSHOT)),
            masked: 1,
            snapshot: Err("gone".into()),
        };
        let subject = Subject {
            title: "t",
            place: "a.cdp:1",
            attempt: None,
            rerun: "chrome-cdp test a.cdp:1".into(),
        };
        let md = render(
            &subject,
            &failure,
            &steps,
            Some(&capture),
            &Logs::default(),
            dir.path(),
        );
        assert!(
            md.contains("\nPainted over because they hold a `${secret:…}` value: 1 field in failure.png, and in 2 `--trace` screenshots.\n"),
            "{md}"
        );
        let md = render(
            &subject,
            &failure,
            &steps[..1],
            None,
            &Logs::default(),
            dir.path(),
        );
        assert!(!md.contains("Painted over"), "{md}");
    }
}

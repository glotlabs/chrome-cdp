//! `expect-snapshot` and `expect-screenshot` (plan 09, milestone 9.2):
//! the page against a baseline file.
//!
//! `expect-snapshot` compares the `snapshot -i` lines (refs left out, so a
//! baseline holds across sessions) with a text file and prints a unified
//! diff when they differ; `expect-screenshot` compares a screenshot with a
//! PNG, pixel by pixel ([`pixels`](super::pixels)), and saves the actual
//! image and a diff image when they differ. Both read the page again until
//! it matches or the timeout passes. `--update` stores the page instead,
//! once it is steady (two reads in a row alike); so does a first run with
//! no baseline, unless the CLI says not to (in CI). A snapshot baseline
//! may be a template ([`super::template`]): open values and `...` lines.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use protocol::schema::{BaselineResult, ExpectScreenshotData, ExpectSnapshotData};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};
use tokio::time::Instant;

use super::pixels::{self, Image};
use super::template::Template;
use crate::context::Scoped as _;
use crate::locator::{LocateOptions, Locator};
use crate::observe::save;
use crate::request::{
    ExpectScreenshotRequest, ExpectSnapshotRequest, ImageFormat, ScreenshotRequest, ShotScale,
    millis,
};
use crate::snapshot::scope_locator;
use crate::tabs::{Page, tab_name};
use crate::{Engine, Running};

/// A snapshot baseline: its text, and the template in it if it has one.
struct SnapshotBaseline {
    text: String,
    template: Option<Template>,
}

impl SnapshotBaseline {
    fn matches(&self, actual: &str) -> bool {
        match &self.template {
            Some(template) => template.matches(actual),
            None => self.text == actual,
        }
    }

    /// The page's text as the baseline would have it: see
    /// [`Template::project`].
    fn project(&self, actual: &str) -> String {
        match &self.template {
            Some(template) => template.project(actual),
            None => actual.to_owned(),
        }
    }
}

/// How long to wait between reads of the page.
const RETRY_DELAY: Duration = Duration::from_millis(100);

/// The least time one read of the page gets, even when it starts just
/// before the deadline.
const READ_TIME: Duration = Duration::from_secs(2);

/// What the reads of the page came to.
enum Outcome<T> {
    /// It matched the baseline.
    Matched(T),
    /// Two reads in a row were alike (storing), or the time ran out on
    /// the last one.
    Read { last: T, steady: bool },
}

/// How a baseline command went, for the output.
struct Reads {
    count: u64,
    elapsed: Duration,
    timeout: Duration,
}

impl Reads {
    fn retries(&self) -> u64 {
        self.count.saturating_sub(1)
    }

    /// `after 5s, 12 retries`, for a failure.
    fn after(&self) -> String {
        let retries = self.retries();
        format!(
            "after {}, {retries} {}",
            format::duration(self.timeout),
            if retries == 1 { "retry" } else { "retries" }
        )
    }
}

/// Checks `expect-snapshot`'s arguments without a browser.
pub fn check_expect_snapshot(req: &ExpectSnapshotRequest) -> Result<(), ErrorInfo> {
    check_name(&req.name, Cmd::new("expect-snapshot").arg("login-form"))?;
    scope_locator(req.scope.as_deref(), &snapshot_options(req)).map(drop)
}

/// Checks `expect-screenshot`'s arguments without a browser.
pub fn check_expect_screenshot(req: &ExpectScreenshotRequest) -> Result<(), ErrorInfo> {
    check_name(&req.name, Cmd::new("expect-screenshot").arg("login-form"))?;
    if !(0.0..=1.0).contains(&req.threshold) {
        return Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("--threshold goes from 0 to 1, got {}", req.threshold),
        )
        .hint(Hint::run(
            Cmd::new("expect-screenshot")
                .arg(&req.name)
                .flag("threshold", "0.1"),
        )));
    }
    crate::observe::check_screenshot(&shot_request(req)).map(drop)
}

/// A baseline's name: words of letters, digits, `-`, `_` and `.`,
/// separated by `/` for folders. `example` is the command with a good
/// one.
pub(crate) fn check_name(name: &str, example: Cmd) -> Result<(), ErrorInfo> {
    let word_ok = |w: &str| {
        !w.is_empty()
            && !w.starts_with('.')
            && w.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if name.split('/').all(word_ok) {
        return Ok(());
    }
    Err(ErrorInfo::new(
        ErrorKind::BadArgs,
        format!(
            "{name:?} cannot name a baseline: use letters, digits, -, _ and . (not first), \
             and / for folders"
        ),
    )
    .hint(Hint::run(example)))
}

fn snapshot_options(req: &ExpectSnapshotRequest) -> LocateOptions {
    LocateOptions {
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
    }
}

/// The `screenshot` that `expect-screenshot` takes: a PNG of CSS pixels,
/// never scaled down.
fn shot_request(req: &ExpectScreenshotRequest) -> ScreenshotRequest {
    ScreenshotRequest {
        tab: req.tab.clone(),
        locator: req.locator.clone(),
        full: req.full,
        container: req.container.clone(),
        format: ImageFormat::Png,
        quality: None,
        path: None,
        scale: ShotScale::Css,
        max_size: 0,
        omit_background: false,
        mask: req.mask.clone(),
        annotate: false,
        strict_refs: req.strict_refs,
        testid_attribute: req.testid_attribute.clone(),
        timeout_ms: req.timeout_ms,
    }
}

/// `expect-snapshot` as typed, to repeat in hints.
fn snapshot_command(req: &ExpectSnapshotRequest) -> Cmd {
    let mut cmd = Cmd::new("expect-snapshot").arg(&req.name);
    if let Some(scope) = &req.scope {
        cmd = cmd.flag("scope", scope);
    }
    if req.full {
        cmd = cmd.switch("full");
    }
    if let Some(dir) = &req.dir {
        cmd = cmd.flag("dir", dir);
    }
    if let Some(tab) = &req.tab {
        cmd = cmd.flag("tab", tab);
    }
    cmd
}

/// `expect-screenshot` as typed, to repeat in hints.
fn screenshot_command(req: &ExpectScreenshotRequest) -> Cmd {
    let mut cmd = Cmd::new("expect-screenshot").arg(&req.name);
    if let Some(locator) = &req.locator {
        cmd = cmd.arg(locator);
    }
    match (&req.container, req.full) {
        (Some(container), _) => cmd = cmd.flag("full", container),
        (None, true) => cmd = cmd.switch("full"),
        (None, false) => {}
    }
    for mask in &req.mask {
        cmd = cmd.flag("mask", mask);
    }
    if let Some(dir) = &req.dir {
        cmd = cmd.flag("dir", dir);
    }
    if let Some(tab) = &req.tab {
        cmd = cmd.flag("tab", tab);
    }
    cmd
}

/// The file at `path`, or `None` if there is none.
pub(crate) fn read_file(path: &Path) -> Result<Option<Vec<u8>>, ErrorInfo> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ErrorInfo::new(
            ErrorKind::BadArgs,
            format!("cannot read the baseline {}: {e}", path.display()),
        )),
    }
}

/// Replaces the file at `path` (its directories made first) in one step,
/// so a reader never sees half of it.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ErrorInfo> {
    let failed = |e: &io::Error| save::failed(path, e);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| save::failed(dir, &e))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    fs::write(&part, bytes).map_err(|e| failed(&e))?;
    fs::rename(&part, path).map_err(|e| {
        let _ = fs::remove_file(&part);
        failed(&e)
    })
}

/// A baseline's text as compared: line ends made `\n`, no trailing
/// blank lines or spaces at line ends.
fn normalize(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end_matches('\n')
        .to_owned()
}

/// A unified diff from `old` to `new`, and how many lines it adds and
/// removes.
fn unified_diff(old: &str, new: &str, from: &str, to: &str) -> (String, usize, usize) {
    let old = format!("{old}\n");
    let new = format!("{new}\n");
    let diff = TextDiff::from_lines(&old, &new);
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header(from, to)
        .to_string();
    (text, added, removed)
}

/// `1 line added, 2 removed`.
fn changes(added: usize, removed: usize) -> String {
    let lines = |n: usize| if n == 1 { "line" } else { "lines" };
    match (added, removed) {
        (a, 0) => format!("{a} {} added", lines(a)),
        (0, r) => format!("{r} {} removed", lines(r)),
        (a, r) => format!("{a} {} added, {r} removed", lines(a)),
    }
}

/// Cuts `diff` at a line boundary to about `max_chars` (0 = no limit),
/// saying how many lines were left out.
fn cut_diff(diff: &str, max_chars: usize) -> String {
    if max_chars == 0 || diff.chars().count() <= max_chars {
        return diff.to_owned();
    }
    let mut kept = String::new();
    let mut lines = diff.lines();
    for line in lines.by_ref() {
        if kept.chars().count() + line.chars().count() + 1 > max_chars {
            let left = 1 + lines.count();
            let _ = write!(
                kept,
                "… {} more diff {}; --max-chars 0 for all of it",
                format::thousands(left),
                if left == 1 { "line" } else { "lines" }
            );
            return kept;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

fn count_lines(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        text.lines().count() as u64
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Engine {
    /// `expect-snapshot <name>`: the active tab's (or `--tab`'s) snapshot
    /// against the baseline file.
    pub async fn expect_snapshot(&self, req: ExpectSnapshotRequest) -> Result<Output, ErrorInfo> {
        check_expect_snapshot(&req)?;
        let options = snapshot_options(&req);
        let scope = scope_locator(req.scope.as_deref(), &options)?;
        let baseline = match read_file(&req.path)? {
            Some(bytes) => {
                let text = normalize(&String::from_utf8_lossy(&bytes));
                let template = Template::parse(&text).map_err(|why| {
                    ErrorInfo::new(
                        ErrorKind::BadArgs,
                        format!("the baseline {} is not a valid template: {why}", req.path.display()),
                    )
                    .hint(Hint::text(
                        "escape the regex's special characters with \\, or quote the value: button \"Save\"",
                    ))
                })?;
                Some(SnapshotBaseline { text, template })
            }
            None => None,
        };
        if baseline.is_none() && !req.update && !req.create {
            let what = format!("expect-snapshot {}", req.name);
            return Err(self.no_baseline(&what, &req.path, snapshot_command(&req)));
        }
        let store = req.update || baseline.is_none();
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let start = Instant::now();
        let (outcome, count) = {
            let _tab = self.tabs.lock(&page.target_id).await;
            let reads = self.read_snapshots(
                &running,
                &page,
                &req,
                scope.as_ref(),
                &options,
                (!store).then_some(baseline.as_ref()).flatten(),
                mark,
                start + timeout,
            );
            Box::pin(self.unless_blocked(&running, &page, mark, reads)).await?
        };
        let reads = Reads {
            count,
            elapsed: start.elapsed(),
            timeout,
        };
        let path = req.path.display().to_string();
        let (text, steady) = match outcome {
            Outcome::Matched(text) => (text, true),
            Outcome::Read { last, steady } if store => (last, steady),
            Outcome::Read { last, .. } => {
                // Not storing: there is a baseline.
                let empty = SnapshotBaseline {
                    text: String::new(),
                    template: None,
                };
                let baseline = baseline.as_ref().unwrap_or(&empty);
                return Err(self.snapshot_differs(&page, &req, baseline, &last, &reads));
            }
        };
        let lines = count_lines(&text);
        let mut data = ExpectSnapshotData {
            tab: tab_name(page.id),
            name: req.name.clone(),
            baseline: path.clone(),
            result: BaselineResult::Passed,
            snapshot: text.clone(),
            lines,
            diff: None,
            elapsed_ms: ms(reads.elapsed),
            retries: reads.retries(),
        };
        let shown = match &baseline {
            _ if !store => format!(
                "passed: expect-snapshot {}: {lines} lines as in the baseline ({})",
                req.name,
                format::duration(reads.elapsed)
            ),
            // A template keeps its lines that still match.
            Some(old) if old.project(&text) == old.text => {
                data.result = BaselineResult::Unchanged;
                format!("baseline {path} is up to date ({lines} lines)")
            }
            Some(old) => {
                let new = old.project(&text);
                write_file(&req.path, format!("{new}\n").as_bytes())?;
                let (diff, added, removed) = unified_diff(&old.text, &new, "baseline", "page");
                data.result = BaselineResult::Updated;
                let shown = format!(
                    "updated baseline {path} ({}):\n{}",
                    changes(added, removed),
                    diff.trim_end()
                );
                data.diff = Some(diff);
                shown
            }
            None => {
                write_file(&req.path, format!("{text}\n").as_bytes())?;
                data.result = BaselineResult::Created;
                format!("created baseline {path} ({lines} lines):\n{text}")
            }
        };
        let more = "--scope <locator> or --max-chars 0 for all of it";
        let mut output = Output::typed(&data, shown)
            .quiet(path)
            .limit(req.max_chars, more);
        if !steady {
            output = output.warning(format!(
                "the page kept changing for {}; stored the last read",
                format::duration(reads.elapsed)
            ));
        }
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// Reads the page until it matches `baseline`, or (with none, when
    /// storing) until two reads in a row are alike, or `deadline`.
    /// Returns what it came to and how many reads it took.
    #[allow(clippy::too_many_arguments)]
    async fn read_snapshots(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectSnapshotRequest,
        scope: Option<&Locator>,
        options: &LocateOptions,
        baseline: Option<&SnapshotBaseline>,
        mark: u64,
        deadline: Instant,
    ) -> Result<(Outcome<String>, u64), ErrorInfo> {
        let retry = |r: &str| {
            let mut again = req.clone();
            again.scope = Some(r.to_owned());
            snapshot_command(&again).scoped(&self.session)
        };
        let mut last: Option<String> = None;
        let mut count = 0;
        loop {
            let left = deadline
                .saturating_duration_since(Instant::now())
                .max(READ_TIME);
            let text = self
                .baseline_text(running, page, scope, options, req.full, &retry, mark, left)
                .await;
            let text = match text {
                Ok(text) => normalize(&text),
                // The scope's element may come back (a re-render).
                Err(e) if e.kind == ErrorKind::NotFound && Instant::now() < deadline => {
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
                Err(e) => return Err(e),
            };
            count += 1;
            match baseline {
                Some(baseline) if baseline.matches(&text) => {
                    return Ok((Outcome::Matched(text), count));
                }
                None if last.as_deref() == Some(text.as_str()) => {
                    return Ok((
                        Outcome::Read {
                            last: text,
                            steady: true,
                        },
                        count,
                    ));
                }
                _ => {}
            }
            if Instant::now() + RETRY_DELAY >= deadline {
                return Ok((
                    Outcome::Read {
                        last: text,
                        steady: false,
                    },
                    count,
                ));
            }
            last = Some(text);
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }

    /// The failure when there is no baseline and it is not to be made.
    fn no_baseline(&self, what: &str, path: &Path, command: Cmd) -> ErrorInfo {
        ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!("{what} failed: there is no baseline yet"),
        )
        .note("baseline", path.display().to_string())
        .note(
            "note",
            "CI is set, so a missing baseline fails instead of being stored",
        )
        .hint(
            Hint::run(command.switch("update").scoped(&self.session))
                .why("stores the page as the baseline; commit the file"),
        )
        .details(json!({ "baseline": path.display().to_string() }))
    }

    /// `ExpectFailed` with the diff from the baseline to the page (in the
    /// baseline's terms, for a template).
    fn snapshot_differs(
        &self,
        page: &Page,
        req: &ExpectSnapshotRequest,
        baseline: &SnapshotBaseline,
        actual: &str,
        reads: &Reads,
    ) -> ErrorInfo {
        let projected = baseline.project(actual);
        let (diff, added, removed) = unified_diff(&baseline.text, &projected, "baseline", "page");
        let hint = match self.unread_errors(page.id) {
            Some((unread, cmd)) => Hint::run(cmd).why(format!(
                "{unread} console {} not shown yet; the page may have failed",
                if unread == 1 { "error" } else { "errors" }
            )),
            None => Hint::run(snapshot_command(req).switch("update").scoped(&self.session))
                .why("if the page is right and the baseline is out of date"),
        };
        ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!(
                "expect-snapshot {} failed: the page differs from the baseline",
                req.name
            ),
        )
        .note("baseline", req.path.display().to_string())
        .note(
            "actual",
            format!(
                "{} lines, {} ({})",
                count_lines(actual),
                changes(added, removed),
                reads.after()
            ),
        )
        .block("diff", cut_diff(&diff, req.max_chars))
        .hint(hint)
        .details(json!({
            "baseline": req.path.display().to_string(),
            "diff": diff,
            "added": added,
            "removed": removed,
            "retries": reads.retries(),
            "timeout_ms": req.timeout_ms,
        }))
    }

    /// `expect-screenshot <name>`: a screenshot of the active tab (or
    /// `--tab`) against the baseline PNG.
    pub async fn expect_screenshot(
        &self,
        req: ExpectScreenshotRequest,
    ) -> Result<Output, ErrorInfo> {
        check_expect_screenshot(&req)?;
        let baseline = match read_file(&req.path)? {
            Some(bytes) => Some(Image::decode(&bytes).map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::BadArgs,
                    format!("the baseline {} is not a PNG: {e}", req.path.display()),
                )
                .hint(
                    Hint::run(
                        screenshot_command(&req)
                            .switch("update")
                            .scoped(&self.session),
                    )
                    .why("stores the page as the baseline again"),
                )
            })?),
            None => None,
        };
        if baseline.is_none() && !req.update && !req.create {
            let what = format!("expect-screenshot {}", req.name);
            return Err(self.no_baseline(&what, &req.path, screenshot_command(&req)));
        }
        let store = req.update || baseline.is_none();
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        self.check_not_blocked(&running, &page)?;
        let mark = running.tabs.mark();
        let start = Instant::now();
        let (outcome, count, warnings) = {
            let _tab = self.tabs.lock(&page.target_id).await;
            let reads = self.take_screenshots(
                &running,
                &page,
                &req,
                (!store).then_some(baseline.as_ref()).flatten(),
                mark,
                start + timeout,
            );
            Box::pin(self.unless_blocked(&running, &page, mark, reads)).await?
        };
        let reads = Reads {
            count,
            elapsed: start.elapsed(),
            timeout,
        };
        let path = req.path.display().to_string();
        let ((image, bytes), steady) = match outcome {
            Outcome::Matched(shot) => (shot, true),
            Outcome::Read { last, steady } if store => (last, steady),
            Outcome::Read { last, .. } => {
                let baseline = baseline.unwrap_or_else(|| last.0.clone());
                return Err(self.screenshot_differs(&page, &req, &baseline, &last, &reads));
            }
        };
        let changed = baseline
            .as_ref()
            .and_then(|old| pixels::count_changed(old, &image, req.threshold));
        let (width, height) = (image.width, image.height);
        let mut data = ExpectScreenshotData {
            tab: tab_name(page.id),
            name: req.name.clone(),
            baseline: path.clone(),
            result: BaselineResult::Passed,
            width,
            height,
            diff_pixels: changed.unwrap_or(0),
            elapsed_ms: ms(reads.elapsed),
            retries: reads.retries(),
        };
        let text = match (&baseline, changed) {
            _ if !store => format!(
                "passed: expect-screenshot {}: {width}x{height} as in the baseline{} ({})",
                req.name,
                match changed {
                    Some(0) | None => String::new(),
                    Some(n) => format!(
                        " but for {} {}",
                        format::thousands(to_usize(n)),
                        if n == 1 { "pixel" } else { "pixels" }
                    ),
                },
                format::duration(reads.elapsed),
            ),
            (Some(_), Some(0)) => {
                data.result = BaselineResult::Unchanged;
                format!("baseline {path} is up to date ({width}x{height})")
            }
            (Some(old), changed) => {
                write_file(&req.path, &bytes)?;
                data.result = BaselineResult::Updated;
                format!(
                    "updated baseline {path} ({width}x{height}; {})",
                    match changed {
                        Some(n) => format!("{} pixels changed", format::thousands(to_usize(n))),
                        None => format!("was {}x{}", old.width, old.height),
                    }
                )
            }
            (None, _) => {
                write_file(&req.path, &bytes)?;
                data.result = BaselineResult::Created;
                format!("created baseline {path} ({width}x{height})")
            }
        };
        let mut output = Output::typed(&data, text).quiet(path);
        for warning in warnings {
            output = output.warning(warning);
        }
        if !steady {
            output = output.warning(format!(
                "the page kept changing for {}; stored the last screenshot",
                format::duration(reads.elapsed)
            ));
        }
        Ok(self.report_events(&running, mark, output, None).await)
    }

    /// Takes screenshots until one matches `baseline`, or (with none,
    /// when storing) until two in a row are alike, or `deadline`. Returns
    /// what it came to (the image and its PNG), how many it took and the
    /// last one's warnings.
    async fn take_screenshots(
        &self,
        running: &Running,
        page: &Page,
        req: &ExpectScreenshotRequest,
        baseline: Option<&Image>,
        mark: u64,
        deadline: Instant,
    ) -> Result<(Outcome<(Image, Vec<u8>)>, u64, Vec<String>), ErrorInfo> {
        let shot = shot_request(req);
        let mut last: Option<Image> = None;
        let mut count = 0;
        loop {
            let (bytes, warnings) = self
                .screenshot_bytes(
                    running,
                    page,
                    &shot,
                    mark,
                    deadline.max(Instant::now() + READ_TIME),
                )
                .await?;
            let image = Image::decode(&bytes).map_err(|e| {
                ErrorInfo::new(
                    ErrorKind::ProtocolError,
                    format!("Chrome sent a screenshot that is not a PNG: {e}"),
                )
            })?;
            count += 1;
            match baseline {
                Some(baseline)
                    if pixels::count_changed(baseline, &image, req.threshold)
                        .is_some_and(|n| n <= req.max_diff_pixels) =>
                {
                    return Ok((Outcome::Matched((image, bytes)), count, warnings));
                }
                None if last.as_ref() == Some(&image) => {
                    let last = (image, bytes);
                    return Ok((Outcome::Read { last, steady: true }, count, warnings));
                }
                _ => {}
            }
            if Instant::now() + RETRY_DELAY >= deadline {
                let last = (image, bytes);
                return Ok((
                    Outcome::Read {
                        last,
                        steady: false,
                    },
                    count,
                    warnings,
                ));
            }
            last = Some(image);
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }

    /// `ExpectFailed` with the actual screenshot and a diff image saved.
    fn screenshot_differs(
        &self,
        page: &Page,
        req: &ExpectScreenshotRequest,
        baseline: &Image,
        (actual, bytes): &(Image, Vec<u8>),
        reads: &Reads,
    ) -> ErrorInfo {
        let shots = self.output_dir().join("shots");
        let saved = save::save(bytes, None, &shots, "png");
        let (width, height) = (actual.width, actual.height);
        let changed = pixels::count_changed(baseline, actual, req.threshold);
        let (message, diff) = match changed {
            Some(n) => {
                let pixels = u64::from(width) * u64::from(height);
                #[allow(clippy::cast_precision_loss)]
                let share = n as f64 * 100.0 / pixels.max(1) as f64;
                let diff = pixels::diff_image(baseline, actual, req.threshold)
                    .encode()
                    .ok()
                    .and_then(|png| save::save(&png, None, &shots, "png").ok());
                (
                    format!(
                        "{} {} ({share:.2}%) differ from the baseline",
                        format::thousands(to_usize(n)),
                        if n == 1 { "pixel" } else { "pixels" },
                    ),
                    diff,
                )
            }
            None => (
                format!(
                    "the page is {width}x{height}, the baseline {}x{}",
                    baseline.width, baseline.height
                ),
                None,
            ),
        };
        let mut info = ErrorInfo::new(
            ErrorKind::ExpectFailed,
            format!("expect-screenshot {} failed: {message}", req.name),
        )
        .note("baseline", req.path.display().to_string());
        let mut details = json!({
            "baseline": req.path.display().to_string(),
            "width": width,
            "height": height,
            "baseline_width": baseline.width,
            "baseline_height": baseline.height,
            "diff_pixels": changed,
            "threshold": req.threshold,
            "max_diff_pixels": req.max_diff_pixels,
            "retries": reads.retries(),
            "timeout_ms": req.timeout_ms,
        });
        match &saved {
            Ok(path) => {
                info = info.note(
                    "actual",
                    format!("{} ({width}x{height}, {})", path.display(), reads.after()),
                );
                details["actual"] = Value::from(path.display().to_string());
            }
            Err(e) => {
                info = info.note("actual", format!("not saved: {}", e.message));
            }
        }
        if let Some(diff) = &diff {
            info = info.note("diff", format!("{} (changes in red)", diff.display()));
            details["diff"] = Value::from(diff.display().to_string());
        }
        let hint = match self.unread_errors(page.id) {
            Some((unread, cmd)) => Hint::run(cmd).why(format!(
                "{unread} console {} not shown yet; the page may have failed",
                if unread == 1 { "error" } else { "errors" }
            )),
            None => Hint::run(
                screenshot_command(req)
                    .switch("update")
                    .scoped(&self.session),
            )
            .why("if the page is right and the baseline is out of date"),
        };
        info.hint(hint).details(details)
    }
}

fn to_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::{changes, check_name, cut_diff, normalize, unified_diff};

    #[test]
    fn names_are_words_and_folders() {
        for ok in ["login", "login-form", "checkout/step_2", "v1.2"] {
            assert!(
                check_name(ok, protocol::Cmd::new("expect-snapshot").arg("login-form")).is_ok(),
                "{ok}"
            );
        }
        for bad in ["", "a b", "../x", "a//b", "/abs", ".hidden", "a/"] {
            let err = check_name(bad, protocol::Cmd::new("expect-snapshot").arg("login-form"))
                .unwrap_err();
            assert_eq!(
                err.hint.as_deref(),
                Some("chrome-cdp expect-snapshot login-form"),
                "{bad}"
            );
        }
    }

    #[test]
    fn baselines_compare_without_line_end_noise() {
        assert_eq!(normalize("- a  \r\n- b\n\n"), "- a\n- b");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn diffs_are_unified_and_counted() {
        let (diff, added, removed) = unified_diff(
            "- heading \"Sign in\"\n- button \"Go\"",
            "- heading \"Sign in\"\n- button \"Continue\"\n- link \"Help\"",
            "baseline",
            "page",
        );
        assert_eq!((added, removed), (2, 1));
        assert_eq!(
            diff,
            "--- baseline\n+++ page\n@@ -1,2 +1,3 @@\n \
             - heading \"Sign in\"\n\
             -- button \"Go\"\n\
             +- button \"Continue\"\n\
             +- link \"Help\"\n"
        );
        assert_eq!(changes(2, 1), "2 lines added, 1 removed");
        assert_eq!(changes(1, 0), "1 line added");
        assert_eq!(changes(0, 3), "3 lines removed");
    }

    #[test]
    fn long_diffs_are_cut_at_a_line() {
        let diff: String = (0..50)
            .flat_map(|i| [format!("+line {i}"), "\n".into()])
            .collect();
        let cut = cut_diff(&diff, 100);
        assert!(cut.starts_with("+line 0\n"));
        assert!(
            cut.ends_with("… 38 more diff lines; --max-chars 0 for all of it"),
            "{cut}"
        );
        assert_eq!(cut_diff(&diff, 0), diff);
    }
}

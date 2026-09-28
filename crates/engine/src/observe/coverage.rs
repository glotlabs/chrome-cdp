//! `coverage start` and `coverage stop` (plan 06, milestone 6.5): how
//! much of the page's JavaScript ran and of its CSS matched, as DevTools'
//! Coverage panel measures it.
//!
//! JavaScript comes from V8's precise block coverage (`Profiler`), CSS
//! from rule usage tracking (`CSS`, which needs `DOM` on); both are on
//! for the tab's session only between start and stop. A stylesheet's size
//! comes with `CSS.styleSheetAdded`, which `CSS.enable` sends for the
//! sheets that exist and later ones. The output sums up the unused bytes
//! per file, inline scripts and styles summed per document; scripts with
//! no URL (chrome-cdp's own, `eval`) are left out. Only the tab's own
//! session is measured: a cross-site iframe runs in its own.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cdp::Session;
use cdp::domains::{css as cdp_css, dom as cdp_dom, profiler};
use protocol::schema::{CoverageFile, CoverageStartData, CoverageStopData, CoverageTotal};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use tokio::task::AbortHandle;

use crate::Engine;
use crate::errors::from_cdp;
use crate::request::{CoverageStartRequest, CoverageStopRequest, millis};
use crate::tabs::tab_name;

/// How many stylesheets a recording keeps the size of.
const SHEETS: usize = 5000;

/// A running `coverage start`.
#[derive(Debug)]
pub(crate) struct CoverageRecording {
    tab: u32,
    session: Session,
    started: Instant,
    /// Stylesheets by id: their URL, whether inline, and length.
    sheets: Arc<Mutex<HashMap<String, Sheet>>>,
    watcher: AbortHandle,
}

#[derive(Debug, Clone)]
struct Sheet {
    url: String,
    inline: bool,
    length: u64,
}

impl Drop for CoverageRecording {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

impl Engine {
    /// `coverage start`: turns on JS and CSS coverage in the tab.
    pub async fn coverage_start(&self, req: CoverageStartRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        if let Some(tab) = lock(&running.coverage).as_ref().map(|c| c.tab) {
            let data = CoverageStartData {
                tab: tab_name(tab),
                already: true,
            };
            let text = format!(
                "still measuring the coverage of {}; {} shows it",
                tab_name(tab),
                Cmd::new("coverage stop").session(&self.session)
            );
            return Ok(Output::typed(&data, text).quiet(tab_name(tab)));
        }
        let session = page.session.with_timeout(timeout);
        // Before `CSS.enable`, which reports the sheets there are.
        let sheets: Arc<Mutex<HashMap<String, Sheet>>> = Arc::default();
        let mut events = session.listen::<cdp_css::StyleSheetAdded>();
        let collect = sheets.clone();
        let watcher = tokio::spawn(async move {
            while let Some(added) = events.next().await {
                let header = added.header;
                let mut sheets = lock(&collect);
                if sheets.len() < SHEETS {
                    sheets.insert(
                        header.style_sheet_id,
                        Sheet {
                            url: header.source_url,
                            inline: header.is_inline,
                            length: whole(header.length),
                        },
                    );
                }
            }
        })
        .abort_handle();
        let recording = CoverageRecording {
            tab: page.id,
            session: page.session.clone(),
            started: Instant::now(),
            sheets,
            watcher,
        };
        let start = profiler::StartPreciseCoverageParams {
            call_count: Some(false),
            detailed: Some(true),
            allow_triggered_updates: None,
        };
        // Sent at once, awaited in order.
        let calls = (
            session.call(profiler::EnableParams {}),
            session.call(start),
            session.call(cdp_dom::EnableParams::default()),
            session.call(cdp_css::EnableParams {}),
            session.call(cdp_css::StartRuleUsageTrackingParams {}),
        );
        let started = async {
            calls.0.await?;
            calls.1.await?;
            calls.2.await?;
            calls.3.await?;
            calls.4.await?;
            Ok::<_, cdp::CdpError>(())
        }
        .await;
        if let Err(e) = started {
            coverage_off(&session).await;
            return Err(from_cdp(&e, &self.session));
        }
        *lock(&running.coverage) = Some(recording);
        let tab = tab_name(page.id);
        let text = format!(
            "measuring the JavaScript and CSS coverage of {tab}; act, then {} shows what was \
             not used",
            Cmd::new("coverage stop").session(&self.session)
        );
        let data = CoverageStartData {
            tab: tab.clone(),
            already: false,
        };
        Ok(Output::typed(&data, text).quiet(tab))
    }

    /// `coverage stop`: what ran and matched since `coverage start`.
    pub async fn coverage_stop(&self, req: CoverageStopRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let Some(recording) = lock(&running.coverage).take() else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("session {} is not measuring coverage", self.session),
            )
            .hint(
                Hint::run(Cmd::new("coverage start").session(&self.session))
                    .why("then act, then coverage stop"),
            ));
        };
        let timeout = millis(req.timeout_ms);
        let session = recording.session.with_timeout(timeout);
        let scripts = session.call(profiler::TakePreciseCoverageParams {});
        let rules = session.call(cdp_css::StopRuleUsageTrackingParams {});
        let (scripts, rules) = (scripts.await, rules.await);
        coverage_off(&session).await;
        let scripts = scripts.map_err(|e| from_cdp(&e, &self.session))?.result;
        let rules = rules.map_err(|e| from_cdp(&e, &self.session))?.rule_usage;
        let sheets = lock(&recording.sheets).clone();
        let documents: Vec<String> = running
            .tabs
            .frames(recording.tab)
            .into_iter()
            .map(|f| f.url)
            .collect();
        let mut files = js_files(&scripts, &documents);
        files.extend(css_files(&rules, &sheets));
        let total = |kind: &str| {
            let of_kind = files.iter().filter(|f| f.kind == kind);
            CoverageTotal {
                files: of_kind.clone().count(),
                total: of_kind.clone().map(|f| f.total).sum(),
                unused: of_kind.map(|f| f.unused).sum(),
            }
        };
        let (js, css) = (total("js"), total("css"));
        files.sort_by(|a, b| b.unused.cmp(&a.unused).then_with(|| a.url.cmp(&b.url)));
        let skipped = match req.limit {
            0 => 0,
            limit => files.len().saturating_sub(limit),
        };
        files.truncate(files.len() - skipped);
        let tab = tab_name(recording.tab);
        let mut text = String::new();
        for (name, t) in [("JS", &js), ("CSS", &css)] {
            let _ = writeln!(
                text,
                "{name}: {} {}, {}, {} unused",
                t.files,
                if t.files == 1 { "file" } else { "files" },
                format::size(t.total),
                share(t.unused, t.total),
            );
        }
        if files.is_empty() {
            text.push_str("no scripts or stylesheets with a URL ran");
        }
        let rows: Vec<Vec<String>> = files
            .iter()
            .map(|f| {
                vec![
                    f.kind.clone(),
                    format::size(f.total),
                    format!("{} unused", share(f.unused, f.total)),
                    if f.inline {
                        format!("{} (inline)", f.url)
                    } else {
                        f.url.clone()
                    },
                ]
            })
            .collect();
        text.push_str(&format::table(&rows));
        if skipped > 0 {
            let _ = write!(
                text,
                "\n… {skipped} more {}; --limit 0 lists them all",
                if skipped == 1 { "file" } else { "files" }
            );
        }
        let quiet = files
            .iter()
            .map(|f| format!("{}\t{}\t{}", f.unused, f.total, f.url))
            .collect::<Vec<_>>()
            .join("\n");
        let data = CoverageStopData {
            tab,
            js,
            css,
            files,
            skipped,
            duration_ms: u64::try_from(recording.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        Ok(Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it, or --limit N"))
    }
}

/// Turns the domains `coverage start` turned on off again.
async fn coverage_off(session: &Session) {
    let calls = (
        session.call(profiler::StopPreciseCoverageParams {}),
        session.call(profiler::DisableParams {}),
        session.call(cdp_css::DisableParams {}),
        session.call(cdp_dom::DisableParams {}),
    );
    let _ = calls.0.await;
    let _ = calls.1.await;
    let _ = calls.2.await;
    let _ = calls.3.await;
}

/// `40%` of `total`, or `0%` of nothing.
fn share(part: u64, total: u64) -> String {
    let percent = (part * 100).checked_div(total).unwrap_or(0);
    format!("{} ({percent}%)", format::size(part))
}

/// Each script with a URL, inline ones summed per document.
fn js_files(scripts: &[profiler::ScriptCoverage], documents: &[String]) -> Vec<CoverageFile> {
    let mut files: Vec<CoverageFile> = Vec::new();
    for script in scripts {
        if !has_url(&script.url) {
            continue;
        }
        let ranges: Vec<(u64, u64, bool)> = script
            .functions
            .iter()
            .flat_map(|f| &f.ranges)
            .map(|r| (offset(r.start_offset), offset(r.end_offset), r.count > 0))
            .collect();
        let (total, used) = used(&ranges);
        let inline = documents.contains(&script.url);
        add(&mut files, "js", &script.url, inline, total, total - used);
    }
    files
}

/// Each stylesheet with a URL, inline ones summed per document.
fn css_files(rules: &[cdp_css::RuleUsage], sheets: &HashMap<String, Sheet>) -> Vec<CoverageFile> {
    let mut used: HashMap<&str, Vec<(u64, u64)>> = HashMap::new();
    for rule in rules.iter().filter(|r| r.used) {
        used.entry(rule.style_sheet_id.as_str())
            .or_default()
            .push((whole(rule.start_offset), whole(rule.end_offset)));
    }
    let mut ids: Vec<&String> = sheets.keys().collect();
    ids.sort();
    let mut files = Vec::new();
    for id in ids {
        let sheet = &sheets[id];
        if !has_url(&sheet.url) {
            continue;
        }
        let used = used
            .get(id.as_str())
            .map_or(0, |ranges| union(ranges))
            .min(sheet.length);
        add(
            &mut files,
            "css",
            &sheet.url,
            sheet.inline,
            sheet.length,
            sheet.length - used,
        );
    }
    files
}

fn add(
    files: &mut Vec<CoverageFile>,
    kind: &str,
    url: &str,
    inline: bool,
    total: u64,
    unused: u64,
) {
    if let Some(file) = files
        .iter_mut()
        .find(|f| f.kind == kind && f.url == url && f.inline == inline)
    {
        file.total += total;
        file.unused += unused;
    } else {
        files.push(CoverageFile {
            kind: kind.to_owned(),
            url: url.to_owned(),
            inline,
            total,
            unused,
        });
    }
}

/// Whether a script or sheet came from somewhere (not chrome-cdp's own
/// scripts, `eval` or constructed sheets).
fn has_url(url: &str) -> bool {
    ["http:", "https:", "file:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

/// The size of a script and how much of it ran, from V8's block coverage
/// ranges `(start, end, ran)`. Ranges nest; each offset counts as the
/// innermost range around it says. The outermost (the script's own)
/// covers all of it.
fn used(ranges: &[(u64, u64, bool)]) -> (u64, u64) {
    let total = ranges.iter().map(|r| r.1).max().unwrap_or(0);
    // Boundaries: ends before starts at one offset, outer starts first.
    let mut events: Vec<(u64, u8, u64, usize)> = Vec::with_capacity(ranges.len() * 2);
    for (i, &(start, end, _)) in ranges.iter().enumerate() {
        if end > start {
            events.push((start, 1, u64::MAX - end, i));
            events.push((end, 0, start, i));
        }
    }
    events.sort_unstable();
    let mut stack: Vec<usize> = Vec::new();
    let mut used = 0;
    let mut at = 0;
    for (offset, kind, _, i) in events {
        if let Some(&top) = stack.last()
            && ranges[top].2
        {
            used += offset - at;
        }
        at = offset;
        if kind == 1 {
            stack.push(i);
        } else if let Some(pos) = stack.iter().rposition(|&j| j == i) {
            stack.remove(pos);
        }
    }
    (total, used)
}

/// The length the ranges `(start, end)` cover together.
fn union(ranges: &[(u64, u64)]) -> u64 {
    let mut ranges = ranges.to_vec();
    ranges.sort_unstable();
    let mut covered = 0;
    let mut reach = 0;
    for (start, end) in ranges {
        let start = start.max(reach);
        if end > start {
            covered += end - start;
            reach = end;
        }
    }
    covered
}

fn offset(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

// Offsets and lengths are whole numbers well within u64.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole(n: f64) -> u64 {
    n.max(0.0).round() as u64
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{union, used};

    #[test]
    fn the_innermost_range_decides() {
        // A 100-char script whose function at 10..60 ran, except its
        // branch at 20..30; the function at 70..90 never ran.
        let ranges = [
            (0, 100, true),
            (10, 60, true),
            (20, 30, false),
            (70, 90, false),
        ];
        assert_eq!(used(&ranges), (100, 100 - 10 - 20));
        // Adjacent ranges, and one as wide as the script.
        let ranges = [
            (0, 50, false),
            (0, 50, true),
            (10, 20, false),
            (20, 30, false),
        ];
        assert_eq!(used(&ranges), (50, 30));
        assert_eq!(used(&[]), (0, 0));
    }

    #[test]
    fn used_rules_are_counted_once() {
        assert_eq!(union(&[(0, 10), (5, 15), (20, 25)]), 20);
        assert_eq!(union(&[(20, 25), (0, 10)]), 15);
    }
}

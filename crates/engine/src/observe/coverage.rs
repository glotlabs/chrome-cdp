//! `coverage start` and `coverage stop` (plan 06, milestone 6.5): how
//! much of the page's JavaScript ran and of its CSS matched, as DevTools'
//! Coverage panel measures it.
//!
//! JavaScript comes from V8's precise block coverage (`Profiler`), CSS
//! from rule usage tracking (`CSS`, which needs `DOM` on); both are on
//! between start and stop in every session of the tab: its page's, its
//! out-of-process iframes' and its workers' (no CSS there), those that
//! attach meanwhile too (plan 13.4, see `tabs::coverage`). A stylesheet's
//! size comes with `CSS.styleSheetAdded`, which `CSS.enable` sends for the
//! sheets that exist and later ones. The output sums up the unused bytes
//! per file and process, inline scripts and styles summed per document,
//! and names the iframe or worker a file ran in outside the tab's own
//! process; scripts with no URL (chrome-cdp's own, `eval`) are left out.

use std::collections::HashMap;
use std::fmt::Write as _;

use cdp::domains::{css as cdp_css, profiler};
use protocol::schema::{
    CoverageFile, CoverageStartData, CoverageStopData, CoverageTotal, FrameInfo, Worker,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};

use crate::Engine;
use crate::context::Scoped as _;
use crate::errors::from_cdp;
use crate::frames::iframe_label;
use crate::request::{CoverageStartRequest, CoverageStopRequest, millis};
use crate::tabs::coverage::Sheet;
use crate::tabs::tab_name;

/// Where a file ran, outside the tab's own process.
#[derive(Debug, Clone, Default, PartialEq)]
struct Place {
    /// An out-of-process iframe's frame id.
    frame: Option<String>,
    worker: Option<Worker>,
}

impl Engine {
    /// `coverage start`: turns on JS and CSS coverage in the tab.
    pub async fn coverage_start(&self, req: CoverageStartRequest) -> Result<Output, ErrorInfo> {
        let running = self.running()?;
        let timeout = millis(req.timeout_ms);
        let page = self.page(&running, req.tab.as_deref(), timeout).await?;
        if let Some(tab) = running.tabs.coverage_tab() {
            let data = CoverageStartData {
                tab: tab_name(tab),
                already: true,
            };
            let text = format!(
                "still measuring the coverage of {}; {} shows it",
                tab_name(tab),
                Cmd::new("coverage stop").scoped(&self.session)
            );
            return Ok(Output::typed(&data, text).quiet(tab_name(tab)));
        }
        if let Err(e) = running.tabs.coverage_start(page.id, timeout).await {
            let _ = running.tabs.coverage_stop(timeout).await;
            return Err(from_cdp(&e, &self.session));
        }
        let tab = tab_name(page.id);
        let text = format!(
            "measuring the JavaScript and CSS coverage of {tab}; act, then {} shows what was \
             not used",
            Cmd::new("coverage stop").scoped(&self.session)
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
        let timeout = millis(req.timeout_ms);
        let Some(stopped) = running.tabs.coverage_stop(timeout).await else {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("session {} is not measuring coverage", self.session),
            )
            .hint(
                Hint::run(Cmd::new("coverage start").scoped(&self.session))
                    .why("then act, then coverage stop"),
            ));
        };
        let stopped = stopped.map_err(|e| from_cdp(&e, &self.session))?;
        let frames = running.tabs.frames(stopped.tab);
        let documents: Vec<String> = frames.iter().map(|f| f.url.clone()).collect();
        let mut files = Vec::new();
        for measured in &stopped.measured {
            let place = Place {
                frame: measured.frame.clone(),
                worker: measured.worker.clone(),
            };
            js_files(&mut files, &measured.scripts, &documents, &place);
            css_files(&mut files, &measured.rules, &measured.sheets, &place);
        }
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
        let tab = tab_name(stopped.tab);
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
                let mut url = f.url.clone();
                if f.inline {
                    url.push_str(" (inline)");
                }
                if let Some(label) = place_label(f, &frames) {
                    let _ = write!(url, " {label}");
                }
                vec![
                    f.kind.clone(),
                    format::size(f.total),
                    format!("{} unused", share(f.unused, f.total)),
                    url,
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
            duration_ms: u64::try_from(stopped.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        Ok(Output::typed(&data, text.trim_end().to_owned())
            .quiet(quiet)
            .limit(req.max_chars, "--max-chars 0 for all of it, or --limit N"))
    }
}

/// `[iframe "pay"]` or `[dedicated worker w.js]`: where a file ran outside
/// the tab's own process.
fn place_label(file: &CoverageFile, frames: &[FrameInfo]) -> Option<String> {
    if let Some(worker) = &file.worker {
        return Some(worker.label());
    }
    let id = file.frame.as_deref()?;
    Some(match frames.iter().find(|f| f.id == id) {
        Some(frame) => format!("[{}]", iframe_label(frame)),
        None => "[iframe, gone]".to_owned(),
    })
}

/// `40%` of `total`, or `0%` of nothing.
fn share(part: u64, total: u64) -> String {
    let percent = (part * 100).checked_div(total).unwrap_or(0);
    format!("{} ({percent}%)", format::size(part))
}

/// Adds each script with a URL to `files`, inline ones summed per
/// document.
fn js_files(
    files: &mut Vec<CoverageFile>,
    scripts: &[profiler::ScriptCoverage],
    documents: &[String],
    place: &Place,
) {
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
        add(files, place, "js", &script.url, inline, total, total - used);
    }
}

/// Adds each stylesheet with a URL to `files`, inline ones summed per
/// document.
fn css_files(
    files: &mut Vec<CoverageFile>,
    rules: &[cdp_css::RuleUsage],
    sheets: &HashMap<String, Sheet>,
    place: &Place,
) {
    let mut used: HashMap<&str, Vec<(u64, u64)>> = HashMap::new();
    for rule in rules.iter().filter(|r| r.used) {
        used.entry(rule.style_sheet_id.as_str())
            .or_default()
            .push((whole(rule.start_offset), whole(rule.end_offset)));
    }
    let mut ids: Vec<&String> = sheets.keys().collect();
    ids.sort();
    for id in ids {
        let sheet = &sheets[id];
        if !has_url(&sheet.url) {
            continue;
        }
        let length = whole(sheet.length);
        let used = used
            .get(id.as_str())
            .map_or(0, |ranges| union(ranges))
            .min(length);
        add(
            files,
            place,
            "css",
            &sheet.url,
            sheet.inline,
            length,
            length - used,
        );
    }
}

/// Adds a file's bytes to `files`: to the entry of the same file, place
/// and kind if there is one.
fn add(
    files: &mut Vec<CoverageFile>,
    place: &Place,
    kind: &str,
    url: &str,
    inline: bool,
    total: u64,
    unused: u64,
) {
    if let Some(file) = files.iter_mut().find(|f| {
        f.kind == kind
            && f.url == url
            && f.inline == inline
            && f.frame == place.frame
            && f.worker == place.worker
    }) {
        file.total += total;
        file.unused += unused;
    } else {
        files.push(CoverageFile {
            kind: kind.to_owned(),
            url: url.to_owned(),
            inline,
            total,
            unused,
            frame: place.frame.clone(),
            worker: place.worker.clone(),
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

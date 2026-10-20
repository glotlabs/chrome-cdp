//! A load run's reports (plan 23.4): `report.md`, in plan 11's report
//! style, for an agent or a pull request, and `report.html`, one page that
//! opens offline, with charts over time (users, arrivals or iterations,
//! request p95 and error rate) and the tables of `summary.json`.

use std::fmt::Write as _;
use std::time::Duration;

use protocol::schema::{LoadData, LoadMetric};

use super::metrics::Second;

/// `report.md`'s and `report.html`'s names in the run's folder.
pub(super) const MARKDOWN: &str = "report.md";
pub(super) const HTML: &str = "report.html";

/// How many endpoints and steps the markdown lists, the busiest first.
const LISTED: usize = 50;

/// `184 ms`, `1.25 s`, or a score.
fn value(metric: &LoadMetric, v: f64) -> String {
    if metric.unit.is_empty() {
        format!("{v:.3}")
    } else {
        super::status::ms_text(v)
    }
}

fn percent(share: f64) -> String {
    super::status::percent(share * 100.0)
}

/// A markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn took(data: &LoadData) -> String {
    protocol::format::duration(Duration::from_millis(data.duration_ms))
}

/// The run in a few lines, for both reports.
fn facts(data: &LoadData) -> Vec<String> {
    let mut facts = Vec::new();
    match &data.profile {
        Some(profile) => facts.push(format!("Profile {profile}, {} model", data.model)),
        None => facts.push(format!("{} model, without a profile", data.model)),
    }
    facts.push(format!("Backend {}", data.backend));
    facts.push(format!(
        "{} iterations in {}, {} failed",
        data.iterations,
        took(data),
        data.failed
    ));
    if let Some(max) = data.max_users {
        facts.push(format!(
            "{} arrivals, {} dropped at max_users ({max}); at most {} users at once",
            data.arrivals, data.dropped, data.users
        ));
    } else {
        facts.push(format!("{} users at most", data.users));
    }
    facts.push(format!(
        "{} {} ({}), up to {} users each",
        data.browsers_started,
        if data.browsers_started == 1 {
            "Chrome"
        } else {
            "Chromes"
        },
        data.browser,
        data.users_per_browser
    ));
    if let Some(stopped) = &data.stopped {
        facts.push(format!("Stopped early: {}", stopped.message));
    }
    if !data.thresholds.is_empty() {
        let passed = data.thresholds.iter().filter(|t| t.passed).count();
        facts.push(format!(
            "Thresholds: {passed} of {} passed",
            data.thresholds.len()
        ));
    }
    facts
}

/// What a threshold measured: `412 ms`, `0.4%`, `no value`.
fn measured(t: &protocol::schema::LoadThreshold) -> String {
    match t.value {
        Some(v) if t.unit == "ms" => super::status::ms_text(v),
        Some(v) => format!("{v}{}", t.unit),
        None => "no value".to_owned(),
    }
}

/// The text of `report.md`.
pub(super) fn markdown(data: &LoadData) -> String {
    let mut md = format!("# Load run {}\n\n", data.run);
    for fact in facts(data) {
        let _ = writeln!(md, "- {fact}");
    }
    if !data.thresholds.is_empty() {
        md.push_str("\n## Thresholds\n\n| | Threshold | Measured | |\n|---|---|---|---|\n");
        for t in &data.thresholds {
            let _ = writeln!(
                md,
                "| {} | `{} {}` | {} | {}{} |",
                if t.passed { "✓" } else { "✗" },
                cell(&t.name),
                cell(&t.condition),
                measured(t),
                if t.aborted { "stopped the run. " } else { "" },
                cell(t.note.as_deref().unwrap_or_default())
            );
        }
    }
    md.push_str("\n## Requests\n\n");
    match &data.requests {
        None => md.push_str("None.\n"),
        Some(all) => {
            let _ = writeln!(
                md,
                "{} requests ({}/s), {} errors; p50 {}, p95 {}, p99 {}.\n",
                all.count,
                all.rate,
                percent(all.error_rate),
                value(all, all.p50),
                value(all, all.p95),
                value(all, all.p99)
            );
            md.push_str(
                "| Endpoint | Count | /s | Errors | p50 | p90 | p95 | p99 | Max |\n\
                 |---|--:|--:|--:|--:|--:|--:|--:|--:|\n",
            );
            for e in data.endpoints.iter().take(LISTED) {
                let _ = writeln!(
                    md,
                    "| `{}` | {}{} | {} | {} | {} | {} | {} | {} | {} |",
                    cell(&e.name),
                    e.count,
                    if e.cached > 0 {
                        format!(" ({} cached)", e.cached)
                    } else {
                        String::new()
                    },
                    e.rate,
                    percent(e.error_rate),
                    value(e, e.p50),
                    value(e, e.p90),
                    value(e, e.p95),
                    value(e, e.p99),
                    value(e, e.max)
                );
            }
            if data.endpoints.len() > LISTED {
                let _ = writeln!(
                    md,
                    "\n{} more endpoints are in summary.json.",
                    data.endpoints.len() - LISTED
                );
            }
        }
    }
    md.push_str("\n## Journeys\n\n| Journey | Iterations | Failed | p50 | p90 | p95 | p99 | Max |\n|---|--:|--:|--:|--:|--:|--:|--:|\n");
    for j in &data.journeys {
        let t = j.iteration_ms;
        let ms = super::status::ms_text;
        let _ = writeln!(
            md,
            "| {} (`{}`) | {} | {} | {} | {} | {} | {} | {} |",
            cell(&j.name),
            cell(&j.place),
            j.iterations,
            j.failed,
            ms(t.p50),
            ms(t.p90),
            ms(t.p95),
            ms(t.p99),
            ms(t.max)
        );
    }
    if data.max_users.is_some() {
        md.push_str("\nAn iteration's time counts from when its user arrived.\n");
    }
    if !data.step_times.is_empty() {
        md.push_str("\n## Steps\n\n| Step | Command | Count | Errors | p50 | p95 | p99 | Max |\n|---|---|--:|--:|--:|--:|--:|--:|\n");
        for s in data.step_times.iter().take(LISTED) {
            let _ = writeln!(
                md,
                "| `{}` | `{}` | {} | {} | {} | {} | {} | {} |",
                cell(s.name.trim_start_matches("step ")),
                cell(s.label.as_deref().unwrap_or_default()),
                s.count,
                s.errors,
                value(s, s.p50),
                value(s, s.p95),
                value(s, s.p99),
                value(s, s.max)
            );
        }
    }
    if !data.vitals.is_empty() {
        md.push_str("\n## What users saw\n\nPer page view; Web Vitals are rated at p75.\n\n| | Page views | p50 | p75 | p95 | Max |\n|---|--:|--:|--:|--:|--:|\n");
        for v in &data.vitals {
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} |",
                v.name.to_uppercase(),
                v.count,
                value(v, v.p50),
                value(v, v.p75),
                value(v, v.p95),
                value(v, v.max)
            );
        }
    }
    md.push_str(&errors_markdown(data));
    if let Some(compare) = &data.compare {
        md.push_str(&super::compare::markdown(compare));
    }
    md.push_str("\n## Files\n\n- summary.json: these numbers, as `--json` prints them\n- metrics.jsonl: each second of each group\n- report.html: this report, with charts over time\n");
    if let Some(dir) = &data.failures_dir {
        let _ = writeln!(
            md,
            "- {dir}: the page, console and requests of {} failed iterations",
            data.failures_kept
        );
    }
    md
}

fn errors_markdown(data: &LoadData) -> String {
    let mut md = String::from("\n## Errors\n\n");
    if data.errors.is_empty() && data.request_errors.is_empty() && data.exceptions_total == 0 {
        md.push_str("None.\n");
        return md;
    }
    md.push_str("| Kind | Count |\n|---|--:|\n");
    for (kind, n) in data.errors.iter().chain(&data.request_errors) {
        let _ = writeln!(md, "| {} | {n} |", cell(kind));
    }
    if data.exceptions_total > 0 {
        let _ = writeln!(md, "| uncaught exceptions | {} |", data.exceptions_total);
    }
    if !data.exceptions.is_empty() {
        md.push_str("\nUncaught exceptions:\n\n");
        for e in &data.exceptions {
            let _ = writeln!(md, "- {} × `{}`", e.count, cell(&e.message));
        }
    }
    md
}

/// HTML text.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const STYLE: &str = "\
.viz-root{color-scheme:light;--surface-1:#fcfcfb;--text-primary:#0b0b0b;--text-secondary:#52514e;\
--muted:#8a8984;--grid:#e6e5e0;--series-1:#2a78d6;--good:#1a7f37;--bad:#cf222e}\
@media (prefers-color-scheme:dark){.viz-root{color-scheme:dark;--surface-1:#1a1a19;\
--text-primary:#f0efec;--text-secondary:#c3c2b7;--muted:#8f8e86;--grid:#33332f;--series-1:#3987e5;\
--good:#3fb950;--bad:#f85149}}\
body{margin:0}.viz-root{background:var(--surface-1);color:var(--text-primary);\
font:14px/1.45 system-ui,sans-serif;padding:2em;min-height:100vh;box-sizing:border-box}\
h1{font-size:1.4em}h2{font-size:1.1em;margin-top:2em}ul.facts{padding-left:1.2em;color:var(--text-secondary)}\
.charts{display:grid;grid-template-columns:repeat(auto-fit,minmax(340px,1fr));gap:1.5em}\
figure{margin:0}figcaption{font-weight:600;margin-bottom:.3em}\
svg text{fill:var(--text-secondary);font-size:11px}svg .grid{stroke:var(--grid);stroke-width:1}\
svg .line{fill:none;stroke:var(--series-1);stroke-width:2;stroke-linejoin:round}\
svg .alone{fill:var(--series-1)}svg .hair{stroke:var(--muted);stroke-width:1;visibility:hidden}\
svg .dot{fill:var(--series-1);stroke:var(--surface-1);stroke-width:2;visibility:hidden}\
.tip{position:absolute;pointer-events:none;background:var(--surface-1);border:1px solid var(--grid);\
padding:.2em .5em;border-radius:4px;font-size:12px;visibility:hidden;white-space:nowrap}\
.tip b{color:var(--text-primary)}figure{position:relative}\
table{border-collapse:collapse;margin:.5em 0}td,th{border-bottom:1px solid var(--grid);\
padding:.25em .7em;text-align:left;vertical-align:top}th{color:var(--text-secondary);font-weight:600}\
td.n,th.n{text-align:right;font-variant-numeric:tabular-nums}.ok{color:var(--good)}.fail{color:var(--bad)}\
code{font-size:.95em}.up{color:var(--bad)}.down{color:var(--good)}";

/// Moves each chart's hairline, dot and tooltip to the second under the
/// pointer.
const SCRIPT: &str = r"
for (const fig of document.querySelectorAll('figure[data-points]')) {
  const pts = JSON.parse(fig.dataset.points);
  const svg = fig.querySelector('svg'), tip = fig.querySelector('.tip');
  const hair = svg.querySelector('.hair'), dot = svg.querySelector('.dot');
  const show = (on) => { for (const el of [hair, dot, tip]) el.style.visibility = on ? 'visible' : 'hidden'; };
  svg.addEventListener('pointerleave', () => show(false));
  svg.addEventListener('pointermove', (ev) => {
    const box = svg.getBoundingClientRect();
    const x = (ev.clientX - box.left) * svg.viewBox.baseVal.width / box.width;
    let best = pts[0];
    for (const p of pts) if (Math.abs(p[0] - x) < Math.abs(best[0] - x)) best = p;
    hair.setAttribute('x1', best[0]); hair.setAttribute('x2', best[0]);
    dot.setAttribute('cx', best[0]); dot.setAttribute('cy', best[1]);
    tip.replaceChildren();
    const value = document.createElement('b'); value.textContent = best[2];
    tip.append(value, document.createTextNode(' at ' + best[3]));
    tip.style.left = (best[0] * box.width / svg.viewBox.baseVal.width + 8) + 'px';
    tip.style.top = '1.6em';
    show(true);
  });
}
";

/// Chart size, in SVG units.
const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 180.0;
const LEFT: f64 = 52.0;
const BOTTOM: f64 = 22.0;
const TOP: f64 = 8.0;
const RIGHT: f64 = 10.0;

/// A line chart of one series over the run's seconds: `points` are
/// (second, value); `shown` formats a value; a `whole` series counts, and
/// its axis steps by whole numbers.
fn chart(
    title: &str,
    points: &[(u64, Option<f64>)],
    shown: &dyn Fn(f64) -> String,
    whole: bool,
) -> String {
    let known: Vec<(u64, f64)> = points
        .iter()
        .filter_map(|(t, v)| v.map(|v| (*t, v)))
        .collect();
    let mut svg = String::new();
    let _ = write!(svg, "<figure");
    let last = points.last().map_or(1, |p| p.0).max(1);
    #[allow(clippy::cast_precision_loss)]
    let x = |t: u64| LEFT + (WIDTH - LEFT - RIGHT) * t as f64 / last as f64;
    // Four steps of a round size.
    let step = nice(known.iter().map(|p| p.1).fold(0.0, f64::max) / 4.0);
    let top = 4.0 * if whole { step.max(1.0) } else { step };
    let y = |v: f64| HEIGHT - BOTTOM - (HEIGHT - BOTTOM - TOP) * v / top;
    let data: Vec<String> = known
        .iter()
        .map(|(t, v)| {
            format!(
                "[{:.1},{:.1},{},{}]",
                x(*t),
                y(*v),
                serde_json::to_string(&shown(*v)).unwrap_or_default(),
                serde_json::to_string(&clock(*t)).unwrap_or_default()
            )
        })
        .collect();
    let _ = write!(
        svg,
        " data-points=\"{}\"><figcaption>{}</figcaption>",
        escape(&format!("[{}]", data.join(","))),
        escape(title)
    );
    let _ = write!(
        svg,
        "<svg viewBox=\"0 0 {WIDTH} {HEIGHT}\" role=\"img\" aria-label=\"{} over the run\">",
        escape(title)
    );
    for i in 0..=4 {
        let v = top * f64::from(i) / 4.0;
        let _ = write!(
            svg,
            "<line class=grid x1=\"{LEFT}\" x2=\"{:.1}\" y1=\"{:.1}\" y2=\"{:.1}\"/>\
             <text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
            WIDTH - RIGHT,
            y(v),
            y(v),
            LEFT - 6.0,
            y(v) + 4.0,
            escape(&shown(v))
        );
    }
    for t in [0, last / 2, last] {
        let _ = write!(
            svg,
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{}</text>",
            x(t),
            HEIGHT - 6.0,
            clock(t)
        );
    }
    // A gap where a second had no value; a second alone between gaps is
    // a dot.
    let mut path = String::new();
    let mut alone = Vec::new();
    for (i, (t, v)) in points.iter().enumerate() {
        let Some(v) = v else { continue };
        let before = i > 0 && points[i - 1].1.is_some();
        let after = points.get(i + 1).is_some_and(|p| p.1.is_some());
        if !before && !after {
            alone.push((x(*t), y(*v)));
        }
        let _ = write!(
            path,
            "{}{:.1},{:.1}",
            if before { "L" } else { "M" },
            x(*t),
            y(*v)
        );
    }
    if known.is_empty() {
        let _ = write!(
            svg,
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">none</text>",
            f64::midpoint(LEFT, WIDTH),
            HEIGHT / 2.0
        );
    } else {
        let _ = write!(svg, "<path class=line d=\"{path}\"/>");
        for (cx, cy) in alone {
            let _ = write!(
                svg,
                "<circle class=alone cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"2.5\"/>"
            );
        }
    }
    let _ = write!(
        svg,
        "<line class=hair y1=\"{TOP}\" y2=\"{:.1}\"/><circle class=dot r=\"4\"/></svg>\
         <div class=tip></div></figure>",
        HEIGHT - BOTTOM
    );
    svg
}

/// A round number at least `max`: 1, 2, 2.5 or 5 times a power of ten.
fn nice(max: f64) -> f64 {
    if max <= 0.0 {
        return 0.25;
    }
    let magnitude = 10f64.powf(max.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|v| *v >= max)
        .unwrap_or(max)
}

/// `1:05`, `1:02:05`.
fn clock(t: u64) -> String {
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

/// A number with at most one decimal.
fn short(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

/// The text of `report.html`.
pub(super) fn html(data: &LoadData, timeline: &[Second]) -> String {
    let mut page = String::from(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width\">",
    );
    let _ = write!(
        page,
        "<title>chrome-cdp load {}</title><style>{STYLE}</style></head>\
         <body><div class=viz-root>\n<h1>Load run {}</h1>\n<ul class=facts>",
        escape(&data.run),
        escape(&data.run)
    );
    for fact in facts(data) {
        let _ = write!(page, "<li>{}</li>", escape(&fact));
    }
    page.push_str("</ul>\n");
    if !data.thresholds.is_empty() {
        page.push_str("<h2>Thresholds</h2><table><tr><th></th><th>Threshold</th><th class=n>Measured</th><th></th></tr>");
        for t in &data.thresholds {
            let _ = write!(
                page,
                "<tr><td class={}>{}</td><td><code>{} {}</code></td><td class=n>{}</td><td>{}{}</td></tr>",
                if t.passed { "ok" } else { "fail" },
                if t.passed { "✓ passed" } else { "✗ failed" },
                escape(&t.name),
                escape(&t.condition),
                escape(&measured(t)),
                if t.aborted { "stopped the run. " } else { "" },
                escape(t.note.as_deref().unwrap_or_default())
            );
        }
        page.push_str("</table>\n");
    }
    page.push_str("<h2>Over time</h2><div class=charts>");
    let series = |f: &dyn Fn(&Second) -> Option<f64>| -> Vec<(u64, Option<f64>)> {
        std::iter::once((0, None))
            .chain(timeline.iter().map(|s| (s.t, f(s))))
            .collect()
    };
    #[allow(clippy::cast_precision_loss)]
    let users = series(&|s| Some(f64::from(s.users)));
    page.push_str(&chart("Users running", &users, &short, true));
    #[allow(clippy::cast_precision_loss)]
    if data.max_users.is_some() {
        let arrivals = series(&|s| Some(s.arrivals as f64));
        page.push_str(&chart("Arrivals a second", &arrivals, &short, true));
    } else {
        let iterations = series(&|s| Some(s.iterations as f64));
        page.push_str(&chart(
            "Iterations ended a second",
            &iterations,
            &short,
            true,
        ));
    }
    let p95 = series(&|s| s.p95);
    page.push_str(&chart(
        "Request p95",
        &p95,
        &|v| super::status::ms_text(v),
        false,
    ));
    #[allow(clippy::cast_precision_loss)]
    let errors = series(&|s| (s.requests > 0).then(|| s.errors as f64 / s.requests as f64 * 100.0));
    page.push_str(&chart(
        "Request errors",
        &errors,
        &|v| super::status::percent(v),
        false,
    ));
    page.push_str("</div>\n");
    page.push_str(&tables(data));
    page.push_str(&seconds_table(timeline));
    let _ = writeln!(page, "<script>{SCRIPT}</script></div></body></html>");
    page
}

/// A group's numbers as a table row: count, /s, errors, percentiles.
fn row(name: &str, extra: &str, m: &LoadMetric) -> String {
    format!(
        "<tr><td><code>{}</code>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td>\
         <td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
        escape(name),
        extra,
        m.count,
        m.rate,
        percent(m.error_rate),
        escape(&value(m, m.p50)),
        escape(&value(m, m.p75)),
        escape(&value(m, m.p95)),
        escape(&value(m, m.p99)),
        escape(&value(m, m.max))
    )
}

const HEAD: &str = "<tr><th></th><th class=n>Count</th><th class=n>/s</th><th class=n>Errors</th>\
<th class=n>p50</th><th class=n>p75</th><th class=n>p95</th><th class=n>p99</th><th class=n>Max</th></tr>";

/// The tables of `summary.json`.
fn tables(data: &LoadData) -> String {
    let mut html = String::new();
    if let Some(all) = &data.requests {
        let _ = write!(html, "<h2>Requests</h2><table>{HEAD}");
        html.push_str(&row("every request", "", all));
        for e in &data.endpoints {
            let cached = if e.cached > 0 {
                format!(" <small>({} cached)</small>", e.cached)
            } else {
                String::new()
            };
            html.push_str(&row(&e.name, &cached, e));
        }
        html.push_str("</table>\n");
    }
    html.push_str("<h2>Journeys</h2><table><tr><th></th><th class=n>Iterations</th><th class=n>Failed</th><th class=n>p50</th><th class=n>p90</th><th class=n>p95</th><th class=n>p99</th><th class=n>Max</th></tr>");
    for j in &data.journeys {
        let t = j.iteration_ms;
        let ms = super::status::ms_text;
        let _ = write!(
            html,
            "<tr><td>{} <small><code>{}</code></small></td><td class=n>{}</td><td class=n>{}</td>\
             <td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
            escape(&j.name),
            escape(&j.place),
            j.iterations,
            j.failed,
            ms(t.p50),
            ms(t.p90),
            ms(t.p95),
            ms(t.p99),
            ms(t.max)
        );
    }
    html.push_str("</table>\n");
    if !data.step_times.is_empty() {
        let _ = write!(html, "<h2>Steps</h2><table>{HEAD}");
        for s in &data.step_times {
            let label = format!(
                "<br><small><code>{}</code></small>",
                escape(s.label.as_deref().unwrap_or_default())
            );
            html.push_str(&row(s.name.trim_start_matches("step "), &label, s));
        }
        html.push_str("</table>\n");
    }
    if !data.vitals.is_empty() {
        let _ = write!(
            html,
            "<h2>What users saw</h2><p>Per page view; Web Vitals are rated at p75.</p><table>{HEAD}"
        );
        for v in &data.vitals {
            html.push_str(&row(&v.name.to_uppercase(), "", v));
        }
        html.push_str("</table>\n");
    }
    let errors: Vec<(&String, &u64)> = data.errors.iter().chain(&data.request_errors).collect();
    if !errors.is_empty() || data.exceptions_total > 0 {
        html.push_str("<h2>Errors</h2><table><tr><th>Kind</th><th class=n>Count</th></tr>");
        for (kind, n) in errors {
            let _ = write!(
                html,
                "<tr><td>{}</td><td class=n>{n}</td></tr>",
                escape(kind)
            );
        }
        for e in &data.exceptions {
            let _ = write!(
                html,
                "<tr><td>uncaught: <code>{}</code></td><td class=n>{}</td></tr>",
                escape(&e.message),
                e.count
            );
        }
        html.push_str("</table>\n");
    }
    if let Some(compare) = &data.compare {
        html.push_str(&super::compare::html(compare));
    }
    html
}

/// Each second's numbers, the charts' table view.
fn seconds_table(timeline: &[Second]) -> String {
    let mut html = String::from(
        "<details><summary>Each second</summary><table><tr><th class=n>At</th><th class=n>Users</th>\
         <th class=n>Arrivals</th><th class=n>Dropped</th><th class=n>Iterations</th><th class=n>Failed</th>\
         <th class=n>Requests</th><th class=n>Errors</th><th class=n>Request p95</th></tr>",
    );
    for s in timeline {
        let _ = write!(
            html,
            "<tr><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td>\
             <td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
            clock(s.t),
            s.users,
            s.arrivals,
            s.dropped,
            s.iterations,
            s.failed,
            s.requests,
            s.errors,
            s.p95.map(super::status::ms_text).unwrap_or_default()
        );
    }
    html.push_str("</table></details>\n");
    html
}

#[cfg(test)]
mod tests;

//! The micro suite: chrome-cdp's per-command costs.
//!
//! Every number is a whole CLI invocation, as an agent pays it: process
//! start, the socket round trip, the daemon's work.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fixtures::Fixtures;
use serde::{Deserialize, Serialize};

use crate::report::{self, Host};
use crate::sandbox::Sandbox;
use crate::stats::{Better, Metric};

/// The snapshot corpus pages timed, by size (plan 04's corpus).
pub const SNAPSHOT_PAGES: [&str; 4] = ["simple", "forms", "big-app", "huge"];

/// Locators timed on `snapshot/huge.html` (about 50,000 nodes): each finds
/// one element near the end of the page.
pub const LOCATORS: [(&str, &str); 4] = [
    ("css", r#"css=section[aria-label="Section 480"] h2"#),
    ("text", "text=Item 480.7"),
    ("role", r#"role=heading[name="Section 480"]"#),
    ("xpath", r#"xpath=//section[@aria-label="Section 480"]/h2"#),
];

/// Route rules added for the overhead measurement: 19 that match nothing
/// the page loads, then one that sends every image on with a header, so
/// each request is paused, checked against all 20 and continued.
pub const ROUTE_RULES: usize = 20;
/// Requests `bench/micro/requests.html` makes after its document.
pub const PAGE_REQUESTS: usize = 50;

/// How many samples each measurement takes.
#[derive(Debug, Clone, Copy)]
pub struct Samples {
    pub cold: usize,
    pub warm: usize,
    pub each: usize,
}

impl Default for Samples {
    fn default() -> Self {
        Self {
            cold: 5,
            warm: 50,
            each: 10,
        }
    }
}

/// A plan's budget for a metric, checked in the report (plans 02, 07, 08
/// and the overview's definition of done).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub metric: String,
    /// `p50 < 1500 ms`
    pub budget: String,
    pub met: bool,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicroReport {
    pub suite: String,
    pub created: u64,
    pub host: Host,
    pub chrome: String,
    /// Metrics by tool: `chrome-cdp`.
    pub tools: BTreeMap<String, Vec<Metric>>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub struct Options<'a> {
    pub bin: &'a Path,
    pub chrome: &'a Path,
    pub samples: Samples,
}

pub fn run(opts: &Options<'_>) -> Result<MicroReport, String> {
    let site = Fixtures::start(0, 0).map_err(|e| format!("start the fixture server: {e}"))?;
    crate::note("micro: chrome-cdp");
    let ours = chrome_cdp(opts, &site)?;
    let tools = BTreeMap::from([("chrome-cdp".to_owned(), ours)]);
    let targets = targets(&tools["chrome-cdp"]);
    Ok(MicroReport {
        suite: "micro".to_owned(),
        created: report::now(),
        host: Host::this(),
        chrome: chrome_version(opts.chrome),
        tools,
        targets,
        notes: Vec::new(),
    })
}

pub fn chrome_version(chrome: &Path) -> String {
    Command::new(chrome)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Times `f` `n` times.
fn time_n(n: usize, mut f: impl FnMut() -> Result<(), String>) -> Result<Vec<Duration>, String> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let start = Instant::now();
        f()?;
        out.push(start.elapsed());
    }
    Ok(out)
}

fn chrome_cdp(opts: &Options<'_>, site: &Fixtures) -> Result<Vec<Metric>, String> {
    let s = opts.samples;
    let sandbox = Sandbox::new(opts.bin, opts.chrome)?;
    let mut metrics = Vec::new();

    // Cold start: no daemon, no Chrome.
    let cold = time_n(s.cold, || {
        sandbox.ok(["close", "--all"])?;
        sandbox.ok(["open", "about:blank"]).map(drop)
    })?;
    metrics.push(Metric::millis("cold_open", &cold).with_note("open about:blank, no daemon"));

    // `open` on a running session is a no-op: the round trip alone.
    let warm = time_n(s.warm, || sandbox.ok(["open", "-q"]).map(drop))?;
    metrics
        .push(Metric::millis("warm_round_trip", &warm).with_note("open -q on a running session"));

    for page in SNAPSHOT_PAGES {
        sandbox.ok(["goto", &site.url(&format!("/snapshot/{page}.html"))])?;
        let took = time_n(s.each, || sandbox.ok(["snapshot"]).map(drop))?;
        metrics.push(Metric::millis(&format!("snapshot/{page}"), &took));
    }

    sandbox.ok(["goto", &site.url("/snapshot/huge.html")])?;
    for (kind, locator) in LOCATORS {
        let took = time_n(s.each, || sandbox.ok(["text", locator, "-q"]).map(drop))?;
        metrics
            .push(Metric::millis(&format!("locator/{kind}"), &took).with_note("on snapshot/huge"));
    }

    sandbox.ok(["goto", &site.url("/bench/micro/click.html")])?;
    let took = time_n(s.each * 2, || sandbox.ok(["click", "css=#inc"]).map(drop))?;
    metrics.push(Metric::millis("click_report", &took).with_note("click with its action report"));

    let requests = site.url("/bench/micro/requests.html");
    let plain = time_n(s.each, || sandbox.ok(["goto", &requests]).map(drop))?;
    for i in 1..ROUTE_RULES {
        sandbox.ok([
            "route",
            &format!("**/no-such-path-{i}/**"),
            "--status",
            "404",
        ])?;
    }
    sandbox.ok([
        "route",
        "**/bench/micro/asset.svg*",
        "--set-header",
        "x-bench: 1",
    ])?;
    let routed = time_n(s.each, || sandbox.ok(["goto", &requests]).map(drop))?;
    sandbox.ok(["unroute", "--all"])?;
    let plain = Metric::millis("route/load_plain", &plain);
    let routed = Metric::millis("route/load_20_rules", &routed);
    #[allow(clippy::cast_precision_loss)]
    let per_request = (routed.p50 - plain.p50) / (PAGE_REQUESTS + 1) as f64;
    metrics.push(plain.with_note("goto a page of 50 images"));
    metrics.push(routed.with_note("the same with 20 route rules"));
    metrics.push(
        Metric::value(
            "route/overhead_per_request",
            "ms",
            Better::Lower,
            per_request.max(0.0),
        )
        .with_note("(p50 with rules − p50 without) / 51 requests"),
    );

    let (separate, batched) = batch_vs_single(&sandbox, s.each)?;
    let speedup = separate.p50 / batched.p50.max(f64::MIN_POSITIVE);
    metrics.push(separate.with_note("10 steps as 10 invocations"));
    metrics.push(batched.with_note("the same 10 steps in one batch"));
    metrics.push(Metric::value("batch/speedup", "x", Better::Higher, speedup));
    Ok(metrics)
}

/// The steps plan 08's criterion names: 10 cheap ones, so the per-call
/// cost dominates.
fn batch_steps() -> Vec<[&'static str; 3]> {
    (0..10)
        .map(|i| {
            if i % 2 == 0 {
                ["text", "css=h1", "-q"]
            } else {
                ["fill", "label=Name", "Ada"]
            }
        })
        .collect()
}

fn batch_vs_single(sandbox: &Sandbox, n: usize) -> Result<(Metric, Metric), String> {
    sandbox.ok(["goto", "data:text/html,<h1>Hi</h1><input aria-label=Name>"])?;
    let steps = batch_steps();
    let separate = time_n(n, || {
        for step in &steps {
            sandbox.ok(step)?;
        }
        Ok(())
    })?;
    let script: String = steps.iter().map(|s| s.join(" ") + "\n").collect();
    let batched = time_n(n, || {
        let mut child = sandbox
            .cli()
            .arg("batch")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("run batch: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(script.as_bytes())
                .map_err(|e| format!("write the batch: {e}"))?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "batch failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    })?;
    Ok((
        Metric::millis("batch/separate_10", &separate),
        Metric::millis("batch/one_10", &batched),
    ))
}

fn targets(metrics: &[Metric]) -> Vec<Target> {
    let find = |name: &str| metrics.iter().find(|m| m.name == name);
    let mut out = Vec::new();
    let mut add = |metric: &str, budget: &str, source: &str, met: Option<bool>| {
        if let Some(met) = met {
            out.push(Target {
                metric: metric.to_owned(),
                budget: budget.to_owned(),
                met,
                source: source.to_owned(),
            });
        }
    };
    add(
        "cold_open",
        "p50 < 1500 ms",
        "overview §7, plan 02",
        find("cold_open").map(|m| m.p50 < 1500.0),
    );
    add(
        "warm_round_trip",
        "p50 < 30 ms",
        "overview §7, plan 02",
        find("warm_round_trip").map(|m| m.p50 < 30.0),
    );
    add(
        "batch/speedup",
        "≥ 3x",
        "plan 08",
        find("batch/speedup").map(|m| m.p50 >= 3.0),
    );
    add(
        "route/overhead_per_request",
        "< 1 ms",
        "plan 07 (its p95 is checked by the engine's `overhead_with_20_rules`)",
        find("route/overhead_per_request").map(|m| m.p50 < 1.0),
    );
    out
}

/// The micro section of docs/benchmarks.md.
pub fn markdown(r: &MicroReport) -> String {
    let mut md = format!(
        "Last run: {} on {} ({} CPUs), {}.\n\n",
        report::date(r.created),
        r.host.key(),
        r.host.cpus,
        r.chrome
    );
    let empty = Vec::new();
    let ours = r.tools.get("chrome-cdp").unwrap_or(&empty);
    md.push_str("| Metric | p50 | p95 | Note |\n|---|---:|---:|---|\n");
    for m in ours {
        let p95 = if m.samples > 1 {
            format!("{} {}", report::fmt_num(m.p95), m.unit)
        } else {
            "–".to_owned()
        };
        let _ = writeln!(
            md,
            "| `{}` | {} {} | {p95} | {} |",
            m.name,
            report::fmt_num(m.p50),
            m.unit,
            m.note.as_deref().unwrap_or("")
        );
    }
    if !r.targets.is_empty() {
        md.push_str("\n| Budget | Metric | Met | Source |\n|---|---|---|---|\n");
        for t in &r.targets {
            let _ = writeln!(
                md,
                "| {} | `{}` | {} | {} |",
                t.budget,
                t.metric,
                if t.met { "✅" } else { "❌" },
                t.source
            );
        }
    }
    for note in &r.notes {
        let _ = write!(md, "\n{note}\n");
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_the_plans_budgets() {
        let metrics = [
            Metric::value("cold_open", "ms", Better::Lower, 900.0),
            Metric::value("warm_round_trip", "ms", Better::Lower, 31.0),
            Metric::value("batch/speedup", "x", Better::Higher, 3.0),
        ];
        let t = targets(&metrics);
        let met: Vec<(&str, bool)> = t.iter().map(|t| (t.metric.as_str(), t.met)).collect();
        assert_eq!(
            met,
            [
                ("cold_open", true),
                ("warm_round_trip", false),
                ("batch/speedup", true)
            ]
        );
    }

    #[test]
    fn the_markdown_has_a_row_per_metric() {
        let r = MicroReport {
            suite: "micro".into(),
            created: 0,
            host: Host {
                os: "linux".into(),
                arch: "x86_64".into(),
                cpus: 4,
            },
            chrome: "Chrome 154".into(),
            tools: BTreeMap::from([(
                "chrome-cdp".into(),
                vec![Metric::value("click_report", "ms", Better::Lower, 12.0)],
            )]),
            targets: vec![],
            notes: vec![],
        };
        let md = markdown(&r);
        assert!(md.contains("| `click_report` | 12 ms | – |  |"), "{md}");
        assert!(md.starts_with("Last run: 1970-01-01 on linux-x86_64 (4 CPUs), Chrome 154."));
    }

    #[test]
    fn the_benchmark_pages_exist() {
        let site = fixtures::site_dir();
        for page in SNAPSHOT_PAGES {
            assert!(
                site.join(format!("snapshot/{page}.html")).is_file(),
                "{page}"
            );
        }
        for page in ["click.html", "requests.html", "asset.svg"] {
            assert!(site.join("bench/micro").join(page).is_file(), "{page}");
        }
        let huge = std::fs::read_to_string(site.join("snapshot/huge.html")).unwrap();
        assert!(
            huge.contains("s <= 500"),
            "huge.html no longer has 500 sections"
        );
    }
}

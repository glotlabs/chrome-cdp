#![allow(clippy::cast_precision_loss)]

use protocol::schema::LoadData;
use serde_json::json;

use super::super::metrics::Second;
use super::{html, markdown};

/// A run of the shop with thresholds, a comparison and errors.
fn data() -> LoadData {
    let metric = |name: &str, label: Option<&str>, count: u64, p50: f64, unit: &str| {
        json!({
            "name": name, "label": label, "count": count, "rate": 2.5, "errors": 1,
            "error_rate": 1.0 / count as f64, "unit": unit,
            "p50": p50, "p75": p50 * 1.2, "p90": p50 * 1.5, "p95": p50 * 2.0,
            "p99": p50 * 3.0, "max": p50 * 4.0, "mean": p50 * 1.1,
        })
    };
    let base = json!({
        "run": "20261020-120000-ab12", "dir": "e2e/.results/load/20261020-120000-ab12",
        "backend": "staging", "profile": "e2e/load/shop.toml", "model": "open",
        "journeys": [{
            "place": "e2e/shop.cdp:1", "name": "browse | buy", "weight": 1,
            "iterations": 40, "failed": 1,
            "iteration_ms": {"p50": 1500.0, "p90": 2000.0, "p95": 2100.0, "p99": 2500.0, "max": 3000.0},
        }],
        "users": 12, "max_users": 100, "arrivals": 41, "dropped": 1,
        "browsers": 2, "browsers_started": 2, "browser": "chrome-headless-shell",
        "users_per_browser": 10, "duration_ms": 16000, "iterations": 40, "failed": 1,
        "steps": {"ok": 200, "failed": 1, "timeout": 0, "error": 0},
        "errors": {"ExpectFailed": 1}, "browser_crashes": 0, "failures_kept": 1,
        "failures_dir": "e2e/.results/load/20261020-120000-ab12/failures",
    });
    let more = json!({
        "requests": metric("requests", None, 400, 12.0, "ms"),
        "endpoints": [
            metric("GET /api/products/:id", None, 120, 30.0, "ms"),
            metric("static js", None, 80, 4.0, "ms"),
        ],
        "step_times": [metric("step e2e/shop.cdp:3", Some("goto /"), 40, 210.0, "ms")],
        "vitals": [metric("lcp", None, 80, 400.0, "ms"), metric("cls", None, 80, 0.01, "")],
        "request_errors": {"http_5xx": 1},
        "exceptions": [{"message": "TypeError: x is undefined", "count": 2}],
        "exceptions_total": 2,
        "thresholds": [
            {"name": "p95(GET /api/products/:id)", "condition": "< 300ms", "passed": true,
             "value": 60.0, "limit": 300.0, "unit": "ms"},
            {"name": "p75(inp)", "condition": "< 200ms", "passed": false, "limit": 200.0,
             "unit": "ms", "note": "inp had no value to measure"},
        ],
        "compare": {
            "run": "20261019-090000-cd34", "summary": "x/summary.json", "marked": 1,
            "changes": [
                {"name": "p95(GET /api/products/:id)", "before": 40.0, "after": 60.0,
                 "unit": "ms", "change": 0.5, "marked": true},
                {"name": "rate(requests)", "before": 25.0, "after": 25.5, "unit": "/s",
                 "change": 0.02, "marked": false},
            ],
        },
        "capacity": {
            "cpu_limit": 90, "memory_limit": 10, "lag_limit_ms": 50.0, "on_saturation": "warn",
            "machine": true, "saturated_s": 6, "seconds": 16,
            "saturations": [{"from_s": 9, "to_s": 14, "limit": "cpu", "value": 97.0}],
            "peak_users": 12, "held_users": 8, "held_per_browser": 4,
            "cpu_p50": 60.0, "cpu_p95": 96.0, "cpu_max": 97.0, "load_max": 14.2,
            "memory_free_min": 41.0, "lag_p99_ms": 3.2, "lag_max_ms": 61.0,
            "page_lag_p99_ms": 48.0, "chrome_cpu_max": 13.4, "chrome_rss_max": 3_221_225_472_u64,
            "own_cpu_max": 0.21, "own_rss_max": 62_914_560,
            "hint": "lower the rate to about 1.6/s, or run the profile on a bigger machine",
        },
    });
    let mut all = base;
    for (k, v) in more.as_object().unwrap() {
        all[k] = v.clone();
    }
    serde_json::from_value(all).unwrap()
}

#[test]
fn report_md_is_golden() {
    let text = markdown(&data());
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/load/report/golden.md");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &text).unwrap();
    }
    let want = std::fs::read_to_string(&golden).unwrap_or_default();
    assert_eq!(text, want, "UPDATE_GOLDEN=1 rewrites {}", golden.display());
}

#[test]
fn report_html_stands_alone() {
    let timeline: Vec<Second> = (1..=16)
        .map(|t| Second {
            t,
            users: 12,
            arrivals: 3,
            requests: 25,
            errors: u64::from(t == 5),
            p95: (t != 7).then_some(40.0 + t as f64),
            cpu: Some(if (9..=14).contains(&t) { 97.0 } else { 60.0 }),
            saturated: (9..=14).contains(&t),
            ..Second::default()
        })
        .collect();
    let page = html(&data(), &timeline);
    // Nothing to fetch: no external script, style, image or font.
    for needle in ["src=\"http", "href=\"http", "url(http", "@import", "<link"] {
        assert!(!page.contains(needle), "{needle}");
    }
    assert!(page.contains("<h1>Load run 20261020-120000-ab12</h1>"));
    for title in [
        "Users running",
        "Arrivals a second",
        "Request p95",
        "Request errors",
        "Client CPU (the machine's)",
    ] {
        assert!(
            page.contains(&format!("<figcaption>{title}</figcaption>")),
            "{title}"
        );
    }
    // A second without requests is a gap in the line.
    let p95 = page.split("<figcaption>Request p95").nth(1).unwrap();
    assert_eq!(
        p95.split("</figure>").next().unwrap().matches('M').count(),
        2
    );
    assert!(page.contains("<code>GET /api/products/:id</code>"));
    assert!(page.contains("browse | buy"));
    assert!(page.contains("TypeError: x is undefined"));
    assert!(page.contains("Compared with 20261019-090000-cd34"));
    assert!(page.contains("✗ failed"));
    assert!(page.contains("<summary>Each second</summary>"));
    // The saturated seconds are shaded in each chart, once.
    assert_eq!(page.matches("<rect class=sat").count(), 5);
    assert!(page.contains("<h2>Load generator</h2>"));
    assert!(page.contains("Saturated 0:08 to 0:14, cpu 97%"));
}

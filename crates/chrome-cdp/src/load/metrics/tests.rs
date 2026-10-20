use std::time::Duration;

use engine::{LoadRequest, PageVitals};
use protocol::schema::Timing;

use super::group::{Grouper, MOST_GROUPS, OTHER};
use super::{Gauges, METRICS, Mark, Metrics};
use crate::load::threshold::{Stat, Target};

fn metrics(dir: Option<&std::path::Path>) -> Metrics {
    Metrics::new(Grouper::new(&[], true, Some("http://shop.test")), dir).unwrap()
}

fn request(method: &str, path: &str, status: Option<u16>, ms: f64) -> LoadRequest {
    LoadRequest {
        method: method.into(),
        url: format!("http://shop.test{path}"),
        kind: "fetch".into(),
        status,
        cache: None,
        error: status
            .is_none()
            .then(|| "net::ERR_CONNECTION_RESET".to_owned()),
        duration_ms: Some(ms),
        timing: Some(Timing {
            blocked_ms: 0,
            dns_ms: 2,
            connect_ms: 5,
            tls_ms: 3,
            send_ms: 0,
            wait_ms: 40,
            receive_ms: Some(1),
        }),
    }
}

#[test]
fn requests_group_by_endpoint_with_their_errors() {
    let m = metrics(None);
    for i in 0..98 {
        m.request(&request(
            "GET",
            &format!("/api/products/{i}"),
            Some(200),
            100.0,
        ));
    }
    m.request(&request("GET", "/api/products/7", Some(503), 300.0));
    m.request(&request("GET", "/api/products/8", None, 5.0));
    let mut cached = request("GET", "/api/cart", Some(200), 1.0);
    cached.cache = Some("memory".into());
    m.request(&cached);
    let s = m.summary(Duration::from_secs(10));
    let all = s.requests.unwrap();
    assert_eq!((all.count, all.errors), (101, 2));
    let products = &s.endpoints[0];
    assert_eq!(products.name, "GET /api/products/:id");
    assert_eq!((products.count, products.errors), (100, 2));
    assert!((products.error_rate - 0.02).abs() < 1e-9);
    assert!((products.rate - 10.0).abs() < 1e-9);
    assert!((products.p50 - 100.0).abs() <= 1.0, "{}", products.p50);
    assert!((products.max - 300.0).abs() <= 3.0);
    assert_eq!(products.error_kinds["503"], 1);
    assert_eq!(products.error_kinds["net::ERR_CONNECTION_RESET"], 1);
    let phases = products.phases.unwrap();
    assert!((phases.ttfb_ms - 40.0).abs() < f64::EPSILON);
    assert!((phases.connect_ms - 2.0).abs() < f64::EPSILON);
    // A cached answer counts, but not in the times.
    let cart = &s.endpoints[1];
    assert_eq!(
        (cart.name.as_str(), cart.count, cart.cached),
        ("GET /api/cart", 1, 1)
    );
    assert!(cart.p50.abs() < f64::EPSILON);
    assert_eq!(s.request_errors["http_5xx"], 1);
    assert_eq!(s.request_errors["request_failed"], 1);
}

#[test]
fn past_the_most_groups_the_rest_is_other_with_the_likely_pattern() {
    let m = metrics(None);
    for i in 0..MOST_GROUPS + 30 {
        m.request(&request(
            "GET",
            &format!("/api/items/item-{i}"),
            Some(200),
            1.0,
        ));
    }
    let s = m.summary(Duration::from_secs(1));
    assert_eq!(s.endpoints.len(), MOST_GROUPS + 1);
    let other = s.endpoints.iter().find(|e| e.name == OTHER).unwrap();
    assert_eq!(other.count, 30);
    assert_eq!(s.overflow, Some(("GET /api/items/:name".to_owned(), 30)));
}

#[test]
fn steps_journeys_vitals_and_exceptions() {
    let m = metrics(None);
    m.step("step e2e/shop.cdp:3", "goto /", 120.0, None);
    m.step("step e2e/shop.cdp:3", "goto /", 80.0, Some("Timeout"));
    m.iteration("e2e/shop.cdp:1", "browse", 1500.0, false);
    m.vitals(&PageVitals {
        url: "http://shop.test/".into(),
        ttfb: Some(20.0),
        lcp: Some(400.0),
        cls: Some(0.12),
        ..PageVitals::default()
    });
    m.exception("TypeError: x");
    m.exception("TypeError: x");
    let s = m.summary(Duration::from_secs(2));
    let step = &s.steps[0];
    assert_eq!(step.name, "step e2e/shop.cdp:3");
    assert_eq!(step.label.as_deref(), Some("goto /"));
    assert_eq!((step.count, step.errors), (2, 1));
    assert_eq!(step.error_kinds["Timeout"], 1);
    assert_eq!(s.journeys[0].label.as_deref(), Some("browse"));
    assert_eq!(s.iterations.count, 1);
    let names: Vec<&str> = s.vitals.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["ttfb", "lcp", "cls"]);
    let cls = &s.vitals[2];
    assert_eq!(cls.unit, "");
    assert!((cls.p75 - 0.12).abs() < 0.002, "{}", cls.p75);
    assert_eq!(s.exceptions[0].count, 2);
    assert_eq!(s.exceptions_total, 2);
}

#[test]
fn each_second_is_a_line_per_group_and_a_run_line() {
    let dir = tempfile::tempdir().unwrap();
    let m = metrics(Some(dir.path()));
    m.request(&request("POST", "/api/cart", Some(200), 200.0));
    m.request(&request("POST", "/api/cart", Some(500), 210.0));
    m.iteration("e2e/shop.cdp:1", "browse", 900.0, true);
    let first = m.tick(
        1,
        Gauges {
            users: 3,
            arrivals: 5,
            dropped: 1,
        },
        Mark::Clean,
        Some(42.0),
    );
    assert_eq!(
        (first.requests, first.errors, first.iterations, first.failed),
        (2, 1, 1, 1)
    );
    assert_eq!((first.arrivals, first.dropped), (5, 1));
    let second = m.tick(
        2,
        Gauges {
            users: 2,
            arrivals: 7,
            dropped: 1,
        },
        Mark::Clean,
        None,
    );
    assert_eq!(
        (second.requests, second.arrivals, second.dropped),
        (0, 2, 0)
    );
    assert!(second.p95.is_none());
    let recent = m.recent();
    assert_eq!(
        (
            recent.seconds,
            recent.requests,
            recent.errors,
            recent.arrivals
        ),
        (2, 2, 1, 7)
    );
    assert!(recent.p95.is_some());
    assert_eq!(m.timeline().len(), 2);
    let text = std::fs::read_to_string(dir.path().join(METRICS)).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert_eq!(lines[0]["group"], "run");
    assert_eq!(lines[0]["users"], 3);
    assert_eq!(lines[1]["group"], "POST /api/cart");
    assert_eq!(lines[1]["kind"], "request");
    assert_eq!(lines[1]["count"], 2);
    assert_eq!(lines[2]["kind"], "journey");
    assert_eq!(lines[3]["t"], 2);
    // The run's totals keep what the seconds had.
    assert_eq!(m.summary(Duration::from_secs(2)).endpoints[0].count, 2);
}

#[test]
fn thresholds_leave_the_saturated_seconds_out() {
    let dir = tempfile::tempdir().unwrap();
    let m = metrics(Some(dir.path()));
    let gauges = |arrivals| Gauges {
        users: 1,
        arrivals,
        dropped: 0,
    };
    let p95 = |m: &Metrics, seconds: f64| {
        m.value(&Target::Requests, Stat::Quantile(0.95), seconds, gauges(0))
            .flatten()
            .unwrap()
    };
    // Seconds 1 and 2 fast; 3 to 7 slow and saturated (from 3, known at
    // 7); 8 fast again, and 9 past a limit for a second only.
    let marks = [
        Mark::Clean,
        Mark::Clean,
        Mark::Over,
        Mark::Over,
        Mark::Over,
        Mark::Over,
        Mark::Began(3),
        Mark::Clean,
        Mark::Over,
    ];
    for (i, mark) in marks.iter().enumerate() {
        let t = u64::try_from(i).unwrap() + 1;
        let ms = if (3..=7).contains(&t) { 900.0 } else { 100.0 };
        for _ in 0..10 {
            m.request(&request("GET", "/api/cart", Some(200), ms));
        }
        m.exception("TypeError: x");
        m.tick(t, gauges(t * 2), *mark, None);
        if t == 6 {
            // Not saturated yet: the slow seconds count.
            assert!(p95(&m, 6.0) > 800.0);
        }
    }
    assert_eq!(m.left_out(), 5);
    assert!((p95(&m, 9.0) - 100.0).abs() <= 1.0, "{}", p95(&m, 9.0));
    let count = m
        .value(&Target::Requests, Stat::Count, 9.0, gauges(18))
        .flatten();
    assert_eq!(count, Some(40.0));
    let rate = m
        .value(&Target::Arrivals, Stat::Rate, 9.0, gauges(18))
        .flatten()
        .unwrap();
    assert!((rate - 2.0).abs() < 1e-9, "{rate}");
    let exceptions = m.value(&Target::Exceptions, Stat::Count, 9.0, gauges(18));
    assert_eq!(exceptions.flatten(), Some(4.0));
    let endpoint = m
        .value(
            &Target::Endpoint("GET /api/cart".into()),
            Stat::Max,
            9.0,
            gauges(18),
        )
        .flatten()
        .unwrap();
    assert!((endpoint - 100.0).abs() <= 1.0, "{endpoint}");
    // The summary keeps every second; the timeline marks the saturated.
    assert_eq!(
        m.summary(Duration::from_secs(9)).requests.unwrap().count,
        90
    );
    let saturated: Vec<u64> = m
        .timeline()
        .iter()
        .filter(|s| s.saturated)
        .map(|s| s.t)
        .collect();
    assert_eq!(saturated, [3, 4, 5, 6, 7]);
}

//! The live numbers in Prometheus' text format (plan 23.4), which `load
//! --metrics-listen` serves while the run lasts: the users, arrivals and
//! exceptions, and per endpoint, journey, step and Web Vital the counts and
//! a summary of the times (p50, p90, p95 and p99 over the run so far).

use std::fmt::Write as _;

use super::hist::Unit;
use super::{Gauges, Inner, Kind, Metrics, Series};

/// The quantiles each summary has.
const QUANTILES: [f64; 4] = [0.5, 0.9, 0.95, 0.99];

impl Metrics {
    /// The numbers now, with the run's gauges.
    pub(crate) fn prometheus(&self, gauges: Gauges) -> String {
        let inner = self.inner();
        let mut out = String::new();
        let metric = |out: &mut String, name: &str, kind: &str, help: &str| {
            let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        };
        metric(
            &mut out,
            "chrome_cdp_load_users",
            "gauge",
            "Virtual users running now.",
        );
        let _ = writeln!(out, "chrome_cdp_load_users {}", gauges.users);
        metric(
            &mut out,
            "chrome_cdp_load_arrivals_total",
            "counter",
            "Users that arrived (an open model's).",
        );
        let _ = writeln!(out, "chrome_cdp_load_arrivals_total {}", gauges.arrivals);
        metric(
            &mut out,
            "chrome_cdp_load_dropped_total",
            "counter",
            "Arrivals dropped at max_users.",
        );
        let _ = writeln!(out, "chrome_cdp_load_dropped_total {}", gauges.dropped);
        metric(
            &mut out,
            "chrome_cdp_load_exceptions_total",
            "counter",
            "Uncaught exceptions in the users' pages.",
        );
        let _ = writeln!(
            out,
            "chrome_cdp_load_exceptions_total {}",
            inner.exceptions_total
        );
        let blocks = [
            (
                Kind::Request,
                "request",
                "endpoint",
                "Requests, by endpoint",
            ),
            (
                Kind::Journey,
                "iteration",
                "journey",
                "Iterations, by journey, from when they were due",
            ),
            (Kind::Step, "step", "step", "Steps, by line"),
        ];
        for (kind, name, label, help) in blocks {
            counts(&mut out, &inner, kind, name, label, help);
        }
        vitals(&mut out, &inner);
        out
    }
}

/// A label's value, escaped.
fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn series(inner: &Inner, kind: Kind) -> impl Iterator<Item = &Series> {
    inner.series.iter().filter(move |s| s.kind == kind)
}

/// `<name>s_total`, `<name>_errors_total` and `<name>_duration_seconds`
/// of every group of `kind`, labelled `label`.
fn counts(out: &mut String, inner: &Inner, kind: Kind, name: &str, label_name: &str, help: &str) {
    let total = format!("chrome_cdp_load_{name}s_total");
    let errors = format!("chrome_cdp_load_{name}_errors_total");
    let seconds = format!("chrome_cdp_load_{name}_duration_seconds");
    let _ = writeln!(out, "# HELP {total} {help}.\n# TYPE {total} counter");
    for s in series(inner, kind) {
        let _ = writeln!(
            out,
            "{total}{{{label_name}=\"{}\"}} {}",
            label(&s.name),
            s.total.count
        );
    }
    let _ = writeln!(
        out,
        "# HELP {errors} {help}: those that failed (requests: or got a status of 400 or more).\n\
         # TYPE {errors} counter"
    );
    for s in series(inner, kind) {
        let _ = writeln!(
            out,
            "{errors}{{{label_name}=\"{}\"}} {}",
            label(&s.name),
            s.total.errors
        );
    }
    let _ = writeln!(
        out,
        "# HELP {seconds} {help}: how long they took.\n# TYPE {seconds} summary"
    );
    for s in series(inner, kind) {
        summary(
            out,
            &seconds,
            &format!("{label_name}=\"{}\"", label(&s.name)),
            s,
            1000.0,
        );
    }
}

/// The Web Vitals: times in seconds, CLS as a score.
fn vitals(out: &mut String, inner: &Inner) {
    let seconds = "chrome_cdp_load_vital_seconds";
    let _ = writeln!(
        out,
        "# HELP {seconds} The page views' TTFB, FCP, LCP, load event and INP.\n# TYPE {seconds} summary"
    );
    for s in series(inner, Kind::Vital).filter(|s| s.unit() == Unit::Ms) {
        summary(
            out,
            seconds,
            &format!("vital=\"{}\"", label(&s.name)),
            s,
            1000.0,
        );
    }
    let cls = "chrome_cdp_load_cls";
    let _ = writeln!(
        out,
        "# HELP {cls} The page views' cumulative layout shift.\n# TYPE {cls} summary"
    );
    for s in series(inner, Kind::Vital).filter(|s| s.unit() == Unit::Score) {
        summary(out, cls, "", s, 1.0);
    }
}

/// A summary's quantiles, sum and count, its values divided by `scale`.
fn summary(out: &mut String, name: &str, labels: &str, s: &Series, scale: f64) {
    let hist = &s.total.hist;
    let sep = if labels.is_empty() { "" } else { "," };
    for q in QUANTILES {
        let _ = writeln!(
            out,
            "{name}{{{labels}{sep}quantile=\"{q}\"}} {}",
            hist.quantile(q) / scale
        );
    }
    #[allow(clippy::cast_precision_loss)]
    let sum = hist.mean() * hist.count() as f64 / scale;
    let braces = if labels.is_empty() {
        String::new()
    } else {
        format!("{{{labels}}}")
    };
    let _ = writeln!(out, "{name}_sum{braces} {sum}");
    let _ = writeln!(out, "{name}_count{braces} {}", hist.count());
}

#[cfg(test)]
mod tests {
    use engine::{LoadRequest, PageVitals};

    use super::super::group::Grouper;
    use super::super::{Gauges, Metrics};

    #[test]
    fn the_text_format() {
        let m = Metrics::new(Grouper::new(&[], true, Some("http://shop.test")), None).unwrap();
        m.request(&LoadRequest {
            method: "GET".into(),
            url: "http://shop.test/api/products/1".into(),
            kind: "fetch".into(),
            status: Some(500),
            cache: None,
            error: None,
            duration_ms: Some(200.0),
            timing: None,
        });
        m.step("step e2e/a\"b.cdp:3", "goto /", 50.0, None);
        m.vitals(&PageVitals {
            url: "http://shop.test/".into(),
            lcp: Some(1500.0),
            cls: Some(0.1),
            ..PageVitals::default()
        });
        let text = m.prometheus(Gauges {
            users: 3,
            arrivals: 10,
            dropped: 1,
        });
        for line in [
            "# TYPE chrome_cdp_load_users gauge",
            "chrome_cdp_load_users 3",
            "chrome_cdp_load_dropped_total 1",
            "chrome_cdp_load_requests_total{endpoint=\"GET /api/products/:id\"} 1",
            "chrome_cdp_load_request_errors_total{endpoint=\"GET /api/products/:id\"} 1",
            "chrome_cdp_load_request_duration_seconds_count{endpoint=\"GET /api/products/:id\"} 1",
            "chrome_cdp_load_steps_total{step=\"step e2e/a\\\"b.cdp:3\"} 1",
            "chrome_cdp_load_cls_count 1",
        ] {
            assert!(text.lines().any(|l| l == line), "no {line} in\n{text}");
        }
        // Times in seconds, within the histograms' 1%.
        for (key, want) in [
            (
                "chrome_cdp_load_request_duration_seconds{endpoint=\"GET /api/products/:id\",quantile=\"0.95\"}",
                0.2,
            ),
            (
                "chrome_cdp_load_vital_seconds{vital=\"lcp\",quantile=\"0.5\"}",
                1.5,
            ),
            ("chrome_cdp_load_cls{quantile=\"0.5\"}", 0.1),
        ] {
            let line = text.lines().find(|l| l.starts_with(key)).unwrap();
            let got: f64 = line[key.len()..].trim().parse().unwrap();
            assert!((got - want).abs() <= want * 0.01, "{line}");
        }
    }
}

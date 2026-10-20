#![allow(clippy::float_cmp)]

use super::{Known, Op, Stat, Target, Unit, Written, holds, parse};

fn read(key: &str, value: &str) -> Result<Written, String> {
    let value: toml::Value = toml::from_str::<toml::Table>(&format!("v = {value}"))
        .unwrap()
        .remove("v")
        .unwrap();
    parse(key, &value)
}

#[test]
fn the_plans_thresholds_parse() {
    let t = read("p95(GET /api/products/:id)", "\"< 300ms\"").unwrap();
    assert_eq!(t.stat, Stat::Quantile(0.95));
    assert_eq!(t.group, "GET /api/products/:id");
    assert_eq!(
        (t.op, t.limit, t.unit, t.abort),
        (Op::Lt, 300.0, Unit::Ms, false)
    );
    let t = read("p99(step checkout.cdp:14)", "\"< 2s\"").unwrap();
    assert_eq!(
        (t.group.as_str(), t.limit),
        ("step checkout.cdp:14", 2000.0)
    );
    let t = read("error_rate", "\"< 1%\"").unwrap();
    assert_eq!((t.stat, t.group.as_str()), (Stat::ErrorRate, "requests"));
    assert!((t.limit - 0.01).abs() < 1e-12);
    assert_eq!(t.unit, Unit::Percent);
    let t = read("p75(lcp)", "\"< 2.5s\"").unwrap();
    assert_eq!(t.limit, 2500.0);
    let t = read("rate(dropped)", "{ limit = \"== 0\", abort = true }").unwrap();
    assert_eq!(
        (t.stat, t.op, t.limit, t.abort),
        (Stat::Rate, Op::Eq, 0.0, true)
    );
    assert_eq!(t.condition, "== 0");
    let t = read("p99.9(requests)", "\">= 1m\"").unwrap();
    assert!(matches!(t.stat, Stat::Quantile(q) if (q - 0.999).abs() < 1e-12));
    assert_eq!((t.op, t.limit), (Op::Ge, 60_000.0));
    let t = read("p75(cls)", "\"<= 0.1\"").unwrap();
    assert_eq!((t.unit, t.limit), (Unit::Number, 0.1));
    let t = read("rate(arrivals)", "\"> 60/m\"").unwrap();
    assert_eq!((t.unit, t.limit), (Unit::PerSecond, 1.0));
    let t = read("error_rate(iterations)", "\"< 0.05\"").unwrap();
    assert_eq!(t.unit, Unit::Percent);
}

#[test]
fn mistakes_say_what_to_write() {
    let e = |key: &str, value: &str| read(key, value).unwrap_err();
    assert!(e("p95", "\"< 1s\"").contains("not a statistic of a group"));
    assert!(e("p95(GET /x", "\"< 1s\"").contains("close the group"));
    assert!(e("p100(steps)", "\"< 1s\"").contains("not a percentile"));
    assert!(e("median(steps)", "\"< 1s\"").contains("not a statistic"));
    assert!(e("p95(steps)", "\"300ms\"").contains("start with <"));
    assert!(e("p95(steps)", "\"< 300\"").contains("say the unit"));
    assert!(e("p95(steps)", "\"< fast\"").contains("not a number"));
    assert!(e("p95(steps)", "\"< 3h\"").contains("no unit"));
    assert!(e("p75(cls)", "\"< 1ms\"").contains("cls is a score"));
    assert!(e("p95(dropped)", "\"< 1ms\"").contains("count and a rate"));
    assert!(e("error_rate(lcp)", "\"< 1%\"").contains("no errors"));
    assert!(e("count(steps)", "\"< 1%\"").contains("plain number"));
    assert!(e("p95(steps)", "3").contains("a threshold's value is its limit"));
    assert!(e("p95(steps)", "{ limit = \"< 1s\", stop = true }").contains("`stop`"));
    assert!(e("p95(steps)", "{ abort = true }").contains("say the limit"));
}

#[test]
fn limits_hold_or_not() {
    let t = |cond: &str| read("p95(steps)", &format!("\"{cond}\"")).unwrap();
    assert!(holds(&t("< 300ms"), 299.9));
    assert!(!holds(&t("< 300ms"), 300.0));
    assert!(holds(&t("<= 300ms"), 300.0));
    assert!(holds(&t("> 1s"), 1000.1));
    assert!(holds(&t(">= 1s"), 1000.0));
    let zero = read("count(dropped)", "\"== 0\"").unwrap();
    assert!(holds(&zero, 0.0));
    assert!(!holds(&zero, 1.0));
    let not = read("count(dropped)", "\"!= 0\"").unwrap();
    assert!(holds(&not, 2.0));
}

fn known() -> Known {
    Known {
        steps: vec![
            "step e2e/scenarios/shop.cdp:4".to_owned(),
            "step e2e/scenarios/checkout.cdp:14".to_owned(),
            "step e2e/other/checkout.cdp:20".to_owned(),
        ],
        journeys: vec![
            (
                "e2e/scenarios/shop.cdp:2".to_owned(),
                "browse a category".to_owned(),
            ),
            (
                "e2e/scenarios/checkout.cdp:3".to_owned(),
                "checkout".to_owned(),
            ),
        ],
    }
}

#[test]
fn groups_resolve_before_the_run() {
    let k = known();
    assert_eq!(k.resolve("requests"), Ok(Target::Requests));
    assert_eq!(k.resolve("lcp"), Ok(Target::Vital("lcp".into())));
    assert_eq!(k.resolve("dropped"), Ok(Target::Dropped));
    assert_eq!(
        k.resolve("step checkout.cdp:14"),
        Ok(Target::Step("step e2e/scenarios/checkout.cdp:14".into()))
    );
    assert_eq!(
        k.resolve("step e2e/scenarios/shop.cdp:4"),
        Ok(Target::Step("step e2e/scenarios/shop.cdp:4".into()))
    );
    assert_eq!(
        k.resolve("journey checkout"),
        Ok(Target::Journey("e2e/scenarios/checkout.cdp:3".into()))
    );
    assert_eq!(
        k.resolve("journey e2e/scenarios/shop.cdp:2"),
        Ok(Target::Journey("e2e/scenarios/shop.cdp:2".into()))
    );
    assert_eq!(
        k.resolve("GET /api/products/:id"),
        Ok(Target::Endpoint("GET /api/products/:id".into()))
    );
    assert_eq!(
        k.resolve("static js"),
        Ok(Target::Endpoint("static js".into()))
    );
}

#[test]
fn an_unknown_group_has_the_closest_names() {
    let k = known();
    let e = k.resolve("step checkout.cdp:15").unwrap_err();
    assert!(
        e.starts_with("no group step checkout.cdp:15; the closest: step "),
        "{e}"
    );
    let e = k.resolve("journey check out").unwrap_err();
    assert_eq!(
        e,
        "no group journey check out; the closest: journey checkout"
    );
    let e = k.resolve("lpc").unwrap_err();
    assert!(e.contains("the closest: lcp"), "{e}");
    let e = k.resolve("step shop.cdp").unwrap_err();
    assert!(e.starts_with("no group step shop.cdp"), "{e}");
}

#[test]
fn a_failing_threshold_is_said_once_and_aborts_after_30_seconds() {
    use super::{Change, Threshold, Watch};
    let threshold = |key: &str, value: &str| Threshold {
        written: read(key, value).unwrap(),
        target: Target::Requests,
    };
    let thresholds = [
        threshold("p95(requests)", "{ limit = \"< 300ms\", abort = true }"),
        threshold("error_rate", "\"< 1%\""),
    ];
    let watch = Watch::default();
    assert!(watch.check(&thresholds, 1, &[None, Some(0.0)]).is_empty());
    assert_eq!(
        watch.check(&thresholds, 2, &[Some(400.0), Some(0.5)]),
        [Change::Failing(0, 400.0), Change::Failing(1, 0.5)]
    );
    for t in 3..32 {
        assert!(
            watch
                .check(&thresholds, t, &[Some(400.0), Some(0.5)])
                .is_empty(),
            "{t}"
        );
    }
    assert_eq!(
        watch.check(&thresholds, 32, &[Some(400.0), Some(0.5)]),
        [Change::Abort(0)]
    );
    assert!(watch.aborted(0));
    assert!(!watch.aborted(1));
    // Passing again, then failing again: said again.
    assert!(
        watch
            .check(&thresholds, 33, &[Some(400.0), Some(0.0)])
            .is_empty()
    );
    assert_eq!(
        watch.check(&thresholds, 34, &[Some(400.0), Some(0.5)]),
        [Change::Failing(1, 0.5)]
    );
}

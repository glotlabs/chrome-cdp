use protocol::{ErrorInfo, ErrorKind};
use serde_json::json;

use super::*;

const AT: &str = "flaky.cdp:7:3";

/// A step `command` at `at` that ran from `start` for `took` ms.
fn step(at: &str, command: &str, start: u64, took: u64, error: Option<ErrorInfo>) -> StepRecord {
    let mut step = StepRecord::new(
        at.to_owned(),
        command.to_owned(),
        None,
        Vec::new(),
        Err(&ErrorInfo::new(ErrorKind::BadArgs, "")),
    );
    step.ok = error.is_none();
    step.error = error;
    step.start_ms = start;
    step.duration_ms = took;
    step
}

fn request(method: &str, path: &str, kind: &str, sent: i64, answered: Option<i64>) -> Timed {
    Timed {
        method: method.to_owned(),
        url: format!("http://127.0.0.1:4000{path}"),
        kind: kind.to_owned(),
        sent_ms: sent,
        answered_ms: answered,
        unmatched: false,
    }
}

fn expect_failed(actual: &str) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::ExpectFailed, "expect failed: #total text \"$5\"")
        .details(json!({ "actual": actual }))
}

/// A run of `goto`, `click "Pay"`, then `expect #total text $5`, which
/// fails with `error` (or passes), with `requests`.
fn run(number: usize, error: Option<ErrorInfo>, requests: Vec<Timed>) -> RunRecord {
    let mut setup = step("flaky.cdp:4", "context new", 0, 5, None);
    setup.hook = Some("setup".into());
    let failure = error.clone().map(|error| TestFailure {
        at: AT.into(),
        step: "expect #total text $5".into(),
        hook: None,
        via: Vec::new(),
        error,
        report: None,
        likely: None,
    });
    RunRecord {
        label: format!("run {number}"),
        number,
        dir: PathBuf::from(format!("/r/run-{number}")),
        failure,
        steps: vec![
            setup,
            step("flaky.cdp:5:3", "goto /flaky.html", 10, 90, None),
            step("flaky.cdp:6:3", "click \"Pay\"", 100, 50, None),
            step(AT, "expect #total text $5", 150, 1000, error),
        ],
        requests,
    }
}

fn page(price: Option<i64>) -> Vec<Timed> {
    vec![
        request("GET", "/flaky.html", "document", 12, Some(30)),
        request("GET", "/api/price", "fetch", 60, price),
    ]
}

#[test]
fn a_request_open_when_a_step_acted_is_unsettled() {
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(expect_failed("$?")), page(Some(1400))),
        run(3, None, page(Some(80))),
        run(4, Some(expect_failed("$?")), page(None)),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(flake.cause, "unsettled", "{flake:#?}");
    assert_eq!((flake.runs, flake.failed_runs), (4, 2));
    assert_eq!(flake.at.as_deref(), Some(AT));
    assert!((flake.confidence - 0.9).abs() < 1e-9, "{flake:#?}");
    assert_eq!(
        flake.evidence,
        [
            "`GET /api/price` was still open when `click \"Pay\"` (flaky.cdp:6:3) started, in all 2 failing runs; in some it was never answered",
            "in all 2 passing runs it was answered before the step started (20ms before, at the closest)",
            "`goto /flaky.html` (flaky.cdp:5:3) sent it",
        ]
    );
    assert_eq!(
        flake.hint.as_deref(),
        Some(
            "add `wait --response '**/api/price'` after flaky.cdp:5:3 (which sends it), or an `expect` of what its answer shows before flaky.cdp:6:3"
        )
    );
}

#[test]
fn answers_in_another_order_are_a_race() {
    let two = |stock: i64, offer: i64| {
        vec![
            request("GET", "/api/stock", "fetch", 40, Some(stock)),
            request("GET", "/api/offer", "xhr", 41, Some(offer)),
        ]
    };
    let runs = vec![
        run(1, None, two(60, 90)),
        run(2, Some(expect_failed("Out of stock")), two(95, 70)),
        run(3, None, two(65, 80)),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(flake.cause, "race", "{flake:#?}");
    assert_eq!(
        flake.evidence,
        [
            "`GET /api/offer` was answered before `GET /api/stock`, in the one failing run",
            "in all 2 passing runs it was answered after it",
        ]
    );
    assert!(flake.others.is_empty(), "{flake:#?}");
}

#[test]
fn an_animating_element_is_a_moving_target() {
    let moving = ErrorInfo::new(
        ErrorKind::Timeout,
        "role=button[name=\"Pay\"] did not stop moving: it moved 40px in 100ms",
    )
    .details(json!({ "check": "stable", "locator": "role=button[name=\"Pay\"]" }));
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(moving), page(Some(70))),
    ];
    let setting = Setting {
        animations: true,
        ..Setting::default()
    };
    let flake = diagnose(&runs, &setting).unwrap();
    assert_eq!(flake.cause, "moving-target");
    assert_eq!(
        flake.evidence,
        [
            "in the one failing run the step could not act: role=button[name=\"Pay\"] did not stop moving: it moved 40px in 100ms"
        ]
    );
    assert!(flake.hint.unwrap().contains("disable_animations = true"));

    let covered = ErrorInfo::new(ErrorKind::Obscured, "#pay is covered by another element")
        .details(json!({ "check": "hit", "covering": { "role": "dialog", "name": "Cookies", "tag": "div" } }));
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(covered), page(Some(70))),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(flake.cause, "moving-target");
    assert_eq!(
        flake.evidence[1],
        "covered by role=dialog[name=\"Cookies\"]"
    );
    assert_eq!(
        flake.hint.as_deref(),
        Some(
            "wait until the covering element is gone before the step: `expect 'role=dialog[name=\"Cookies\"]' hidden`"
        )
    );
}

#[test]
fn a_request_only_failing_runs_send_is_different_requests() {
    let mut extra = page(Some(70));
    extra.push(request("GET", "/api/promo/17", "fetch", 80, Some(90)));
    let mut other = extra.clone();
    other[2].url = "http://127.0.0.1:4000/api/promo/18".into();
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(expect_failed("Promo!")), extra),
        run(3, Some(expect_failed("Promo!")), other),
        run(4, None, page(Some(75))),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(flake.cause, "different-requests", "{flake:#?}");
    assert_eq!(
        flake.evidence,
        ["`GET /api/promo/:id` was sent in all 2 failing runs, and in none of the 2 passing runs"]
    );

    // A request no mock answered, in failing runs only.
    let mut unmatched = page(Some(70));
    unmatched[1].unmatched = true;
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(expect_failed("$?")), unmatched),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(flake.cause, "different-requests", "{flake:#?}");
    assert_eq!(
        flake.evidence,
        ["no mock answered `GET /api/price` in the one failing run"]
    );
}

#[test]
fn another_value_each_failing_run_with_determinism_off_is_time() {
    let runs = vec![
        run(1, None, page(Some(70))),
        run(
            2,
            Some(expect_failed("Updated 10:31:02.123")),
            page(Some(70)),
        ),
        run(
            3,
            Some(expect_failed("Updated 10:31:05.876")),
            page(Some(70)),
        ),
    ];
    // Determinism on: the values do not point at it.
    let on = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(on.cause, "unknown", "{on:#?}");
    let setting = Setting {
        unfixed: vec!["freeze_time"],
        ..Setting::default()
    };
    let flake = diagnose(&runs, &setting).unwrap();
    assert_eq!(flake.cause, "time-or-randomness");
    assert_eq!(
        flake.evidence,
        [
            "the failing step saw another value in each failing run: \"Updated 10:31:02.123\", \"Updated 10:31:05.876\"",
            "freeze_time is off in [e2e.determinism]",
        ]
    );
    // One value in every failing run is not time.
    let same = vec![
        run(1, None, page(Some(70))),
        run(2, Some(expect_failed("$?")), page(Some(70))),
        run(3, Some(expect_failed("$?")), page(Some(70))),
    ];
    assert_eq!(diagnose(&same, &setting).unwrap().cause, "unknown");
}

#[test]
fn failing_from_one_run_on_against_a_real_backend_is_shared_state() {
    let runs: Vec<RunRecord> = (1..=5)
        .map(|n| {
            run(
                n,
                (n >= 3).then(|| expect_failed("2 items")),
                page(Some(70)),
            )
        })
        .collect();
    assert_eq!(
        diagnose(&runs, &Setting::default()).unwrap().cause,
        "unknown"
    );
    let real = Setting {
        real: true,
        ..Setting::default()
    };
    let flake = diagnose(&runs, &real).unwrap();
    assert_eq!(flake.cause, "shared-state");
    assert!((flake.confidence - 0.5).abs() < 1e-9);
    assert!(flake.evidence[0].starts_with("every run before run 3 passed"));
    // A pass after a failure is not left-behind data.
    let mut mixed = runs;
    mixed[3].failure = None;
    assert_eq!(diagnose(&mixed, &real).unwrap().cause, "unknown");
}

#[test]
fn unknown_names_the_first_step_whose_result_differs() {
    let mut failed = run(2, Some(expect_failed("$0")), page(Some(70)));
    failed.steps[2].output = "clicked button \"Pay\" (12ms); total $0".into();
    let mut passed = run(1, None, page(Some(70)));
    passed.steps[2].output = "clicked button \"Pay\" (31ms); total $5".into();
    let flake = diagnose(&[passed, failed], &Setting::default()).unwrap();
    assert_eq!(flake.cause, "unknown");
    assert!(flake.confidence.abs() < 1e-9);
    assert_eq!(
        flake.evidence,
        [
            "the first step whose result differs is `click \"Pay\"` (flaky.cdp:6:3): run 1 (passed) gave \"clicked button \"Pay\" (…); total $5\", run 2 (failed) gave \"clicked button \"Pay\" (…); total $0\""
        ]
    );
    assert_eq!(
        flake.hint.as_deref(),
        Some("compare /r/run-1/steps.jsonl with /r/run-2/steps.jsonl")
    );
}

#[test]
fn causes_rank_by_the_failing_runs_they_explain() {
    // Run 2's price came late; runs 3 and 4 also sent an extra request,
    // which explains more.
    let mut extra = page(Some(70));
    extra.push(request("GET", "/api/promo", "fetch", 80, Some(90)));
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(expect_failed("$?")), page(Some(1400))),
        run(3, Some(expect_failed("$?")), extra.clone()),
        run(4, Some(expect_failed("$?")), extra),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    assert_eq!(flake.cause, "different-requests", "{flake:#?}");
    assert!((flake.confidence - 0.57).abs() < 1e-9, "{flake:#?}");
    let others: Vec<&str> = flake.others.iter().map(|o| o.cause.as_str()).collect();
    assert_eq!(others, ["unsettled"]);
}

#[test]
fn runs_that_agree_are_not_flaky() {
    let passed = vec![run(1, None, page(Some(70))), run(2, None, page(Some(70)))];
    assert!(diagnose(&passed, &Setting::default()).is_none());
    let failed = vec![run(1, Some(expect_failed("$?")), page(Some(70)))];
    assert!(diagnose(&failed, &Setting::default()).is_none());
}

#[test]
fn urls_are_templated_and_timings_normalized() {
    assert_eq!(
        template("http://h:1/api/orders/123?id=550e8400-e29b-41d4-a716-446655440000&x=a#top"),
        "http://h:1/api/orders/:id?id=:id&x=a"
    );
    assert_eq!(
        response_pattern("GET http://h:1/api/orders/:id?x=1"),
        "**/api/orders/*"
    );
    assert_eq!(
        normalized("goto /a (865ms) in 1.2s; 3 items"),
        "goto /a (…) in …; 3 items"
    );
    assert_eq!(normalized("v2s and 10s"), "v2s and …");
}

#[test]
fn the_text_and_section_name_the_cause() {
    let runs = vec![
        run(1, None, page(Some(70))),
        run(2, Some(expect_failed("$?")), page(None)),
    ];
    let flake = diagnose(&runs, &Setting::default()).unwrap();
    let text = text(&flake);
    assert!(
        text.starts_with(
            "    flaky: 1 of 2 runs failed at flaky.cdp:7:3 (`expect #total text $5`)\n    likely cause: acted before the page settled (confidence 0.90)\n      `GET /api/price` was still open"
        ),
        "{text}"
    );
    assert!(text.contains("\n      fix: add `wait --response"), "{text}");
    let md = section(&flake, &runs);
    assert!(
        md.starts_with("\n## Flaky\n\n1 of 2 runs failed at"),
        "{md}"
    );
    assert!(md.contains("Failed: run 2. Passed: run 1.\n"), "{md}");
    assert!(md.contains("compare /r/run-2 with /r/run-1"), "{md}");
}

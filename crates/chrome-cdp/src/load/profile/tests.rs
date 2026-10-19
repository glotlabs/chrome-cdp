#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::time::Duration;

use super::*;

fn ok(text: &str) -> Profile {
    parse(text, "e2e/load/shop.toml", Path::new("/p")).unwrap_or_else(|e| panic!("{e:?}"))
}

/// The error's first problem and the others, as `file:line:col: message`.
fn problems(text: &str) -> Vec<String> {
    let e = parse(text, "e2e/load/shop.toml", Path::new("/p")).unwrap_err();
    let mut all = vec![e.message.clone()];
    all.extend(
        e.notes
            .iter()
            .filter(|n| n.label == "also")
            .map(|n| n.text.clone()),
    );
    all
}

const JOURNEY: &str = "[[journeys]]\ntest = \"e2e/scenarios/shop.cdp:search\"\n";

#[test]
fn the_plans_example_parses() {
    let p = ok(r#"
backend = "staging"

[[journeys]]
test = "e2e/scenarios/shop.cdp:browse a category"
weight = 70
[[journeys]]
test = "e2e/scenarios/shop.cdp:12"
weight = 25
[[journeys]]
test = "e2e/scenarios/checkout.cdp:checkout as a signed-in user"
weight = 5
data = "e2e/load/users.csv"
data_order = "random"
data_exhausted = "stop"

[arrivals]
stages = [
  { rate = "0..20/s", duration = "2m" },
  { rate = "20/s",    duration = "10m" },
  { rate = "20..0/s", duration = "1m" },
]
max_users = 400

think = "3s..9s"
"#);
    assert_eq!(p.backend.as_deref(), Some("staging"));
    assert_eq!(p.journeys.len(), 3);
    let j = &p.journeys[0];
    assert_eq!(j.file, Path::new("/p/e2e/scenarios/shop.cdp"));
    assert_eq!(j.pick, TestPick::Name("browse a category".into()));
    assert_eq!(j.weight, 70);
    assert_eq!(j.at.to_string(), "e2e/load/shop.toml:5:8");
    assert_eq!(p.journeys[1].pick, TestPick::Line(12));
    let data = p.journeys[2].data.as_ref().unwrap();
    assert_eq!(data.path, Path::new("/p/e2e/load/users.csv"));
    assert_eq!(data.order, DataOrder::Random);
    assert_eq!(data.exhausted, DataExhausted::Stop);
    let Model::Open(arrivals) = &p.model else {
        panic!("{:?}", p.model)
    };
    assert_eq!(arrivals.max_users, 400);
    assert_eq!(
        arrivals.stages[0],
        RateStage {
            from: 0.0,
            to: 20.0,
            duration: Duration::from_secs(120)
        }
    );
    assert_eq!(p.model.duration(), Duration::from_mins(13));
    // `think` after [arrivals] belongs to it in TOML; it is the profile's.
    assert_eq!(p.think.to_string(), "3s..9s");
    assert_eq!(p.on_failure, OnFailure::Continue);
}

#[test]
fn a_closed_model_with_a_count() {
    let p = ok(&format!(
        "think = \"exp(4s)\"\nthink_between_steps = true\non_failure = \"stop-run\"\n\
         max_failures = 20\n{JOURNEY}[users]\ncount = 50\nramp_up = \"1m\"\nduration = \"5m\"\n"
    ));
    let Model::Closed(users) = &p.model else {
        panic!()
    };
    assert_eq!(
        users.stages,
        [
            UserStage {
                from: 0,
                to: 50,
                duration: Duration::from_secs(60)
            },
            UserStage {
                from: 50,
                to: 50,
                duration: Duration::from_secs(300)
            }
        ]
    );
    assert_eq!(users.peak(), 50);
    assert!(p.think_between_steps);
    assert_eq!(p.on_failure, OnFailure::StopRun);
    assert_eq!(p.max_failures, 20);
    assert_eq!(p.journeys[0].weight, 1);
    // Stages, and iterations without a duration.
    let p = ok(&format!(
        "{JOURNEY}[users]\nstages = [{{ users = \"0..10\", duration = \"30s\" }}, {{ users = 10, duration = \"1m\" }}]\n"
    ));
    let Model::Closed(users) = &p.model else {
        panic!()
    };
    assert_eq!(users.stages[1].from, 10);
    let p = ok(&format!("{JOURNEY}[users]\ncount = 5\niterations = 3\n"));
    let Model::Closed(users) = &p.model else {
        panic!()
    };
    assert_eq!(users.iterations, Some(3));
    assert_eq!(users.stages[0].duration, UNTIL_DONE);
}

#[test]
fn rates_parse() {
    assert_eq!(parse_rate("20/s"), Ok((20.0, 20.0)));
    assert_eq!(parse_rate("0..20/s"), Ok((0.0, 20.0)));
    assert_eq!(parse_rate("60/m..2/s"), Ok((1.0, 2.0)));
    assert_eq!(parse_rate("0.5/s"), Ok((0.5, 0.5)));
    assert_eq!(parse_rate("3600/h"), Ok((1.0, 1.0)));
    assert!(parse_rate("20").unwrap_err().contains("say per what"));
    assert!(parse_rate("20/d").unwrap_err().contains("not /s, /m or /h"));
    assert!(parse_rate("-1/s").is_err());
}

#[test]
fn every_problem_at_once() {
    let all = problems(
        "[[journeys]]\ntest = \"e2e/shop.cdp\"\n[[journeys]]\ntest = \"e2e/shop.cdp:a\"\n\
         weight = 0\n[[journeys]]\ntest = \"e2e/shop.cdp:b\"\nweight = -3\n\
         [arrivals]\nstages = [{ rate = \"fast\", duration = \"1m\" }]\n\
         [users]\ncount = 3\n",
    );
    assert_eq!(
        all,
        [
            "e2e/load/shop.toml:2:8: test = \"e2e/shop.cdp\": name a test of a scenario file, \
             FILE:NAME or FILE:LINE",
            "e2e/load/shop.toml:5:10: weight = 0: a whole number above 0",
            "e2e/load/shop.toml:8:10: weight = -3: a whole number above 0",
            "e2e/load/shop.toml:11:1: a profile has [arrivals] (open: users arrive at a rate) \
             or [users] (closed: a number of users loop), not both",
        ]
    );
}

#[test]
fn a_model_is_needed_and_keys_are_known() {
    let all = problems(JOURNEY);
    assert_eq!(
        all,
        ["e2e/load/shop.toml:1:1: the profile has no load model: add [arrivals] or [users]"]
    );
    let all = problems(&format!("{JOURNEY}[arrivals]\nrate = \"5/s\"\n"));
    assert!(all[0].contains("rate needs a duration"), "{all:?}");
    let all = problems(&format!(
        "{JOURNEY}[arrivals]\nrate = \"0/s\"\nduration = \"1m\"\n"
    ));
    assert!(all[0].contains("no user ever arrives"), "{all:?}");
    let all = problems(&format!("{JOURNEY}[users]\ncount = 5\n"));
    assert!(
        all[0].contains("needs a duration (or iterations)"),
        "{all:?}"
    );
    let all = problems(&format!(
        "max_failures = 5\n{JOURNEY}[users]\ncount = 5\nduration = \"1m\"\n"
    ));
    assert!(all[0].contains("on_failure = \"stop-run\""), "{all:?}");
    let e = parse(
        &format!("{JOURNEY}[users]\ncount = 5\nduration = \"1m\"\nspeed = 3\n"),
        "p.toml",
        Path::new("/p"),
    )
    .unwrap_err();
    assert!(
        e.message.starts_with("p.toml:6:1: unknown field `speed`"),
        "{}",
        e.message
    );
    let all = problems("[users]\ncount = 5\nduration = \"1m\"\n");
    assert!(all[0].contains("the profile has no journeys"), "{all:?}");
    let all = problems(
        "[[journeys]]\ntest = \"e2e/a.cdp:x\"\ndata_order = \"random\"\n\
         [users]\ncount = 5\nduration = \"1m\"\n",
    );
    assert!(all[0].contains("which it has not"), "{all:?}");
}

#[test]
fn profiles_are_found_by_name_or_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(DIR)).unwrap();
    std::fs::write(root.join(DIR).join("shop.toml"), "").unwrap();
    assert_eq!(
        find("shop", root, root),
        Some(root.join("e2e/load/shop.toml"))
    );
    assert_eq!(find("other", root, root), None);
    assert_eq!(find("e2e/shop.cdp", root, root), None);
    assert_eq!(find("e2e/shop.cdp:3", root, root), None);
    assert_eq!(find("x/p.toml", root, root), Some(root.join("x/p.toml")));
    assert_eq!(names(root), ["shop"]);
}

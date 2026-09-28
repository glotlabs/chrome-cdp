use std::path::Path;
use std::time::Duration;

use protocol::script::Resolve;
use regex::Regex;

use super::select::{
    Filter, Target, files_matching, files_of, glob_matches, parse_shard, parse_target, select,
    shard,
};
use super::{Scenario, Settings, load};
use crate::config::Viewport;

fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn load_text(text: &str) -> Result<Scenario, Vec<String>> {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "s.cdp", text);
    load(&path, "s.cdp").map_err(|errors| {
        errors
            .iter()
            .map(|e| format!("{}: {}", e.span, e.message))
            .collect()
    })
}

const CHECKOUT: &str = r#"# e2e/scenarios/checkout.cdp
@mocks base checkout             # layered onto default_mocks
@tags smoke checkout

@before
  goto /

@test "lists products"
  expect 'role=listitem' count 3
  expect text=Keyboard visible

@test "retries when the API is busy"
  @retries 2
  click 'role=button[name="Buy"]'
  expect role=alert text "Please try again"

@test "shows an error page when the API is down"
  @mocks errors/api-down         # extra layer for this test only
  @viewport 390x844
  reload

@test "empty cart" @skip "flaky upstream component, see #12"
  click text=Cart
"#;

#[test]
fn a_scenario_has_settings_hooks_and_tests() {
    let s = load_text(CHECKOUT).unwrap();
    assert_eq!(s.settings.mocks, ["base", "checkout"]);
    assert_eq!(s.settings.tags, ["smoke", "checkout"]);
    assert_eq!(s.before.len(), 1);
    assert_eq!(s.before[0].line.text, "goto /");
    let names: Vec<&str> = s.tests.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "lists products",
            "retries when the API is busy",
            "shows an error page when the API is down",
            "empty cart"
        ]
    );
    assert_eq!(s.tests[0].steps.len(), 2);
    assert_eq!(s.tests[0].span.line, 8);
    // A test reaches to the line before the next one.
    assert_eq!(s.tests[0].end_line, 11);
    assert_eq!(s.tests[1].settings.retries, Some(2));
    let third = s.settings_of(2);
    assert_eq!(third.mocks, ["base", "checkout", "errors/api-down"]);
    assert_eq!(
        third.viewport,
        Some(Viewport {
            width: 390,
            height: 844
        })
    );
    assert_eq!(
        s.settings_of(3).skip.as_deref(),
        Some("flaky upstream component, see #12")
    );
    assert_eq!(s.tests[3].end_line, u32::MAX);
}

#[test]
fn settings_layer_a_test_on_its_file() {
    let file = Settings {
        mocks: vec!["base".into(), "a".into()],
        tags: vec!["smoke".into()],
        timeout: Some(Duration::from_secs(5)),
        device: Some("iPhone 15".into()),
        ..Settings::default()
    };
    let test = Settings {
        mocks: vec!["base".into()],
        tags: vec!["smoke".into(), "slow".into()],
        viewport: Some(Viewport {
            width: 10,
            height: 20,
        }),
        only: true,
        ..Settings::default()
    };
    let both = file.with(&test);
    // Named again, a set moves to where it is named last.
    assert_eq!(both.mocks, ["a", "base"]);
    assert_eq!(both.tags, ["smoke", "slow"]);
    assert_eq!(both.timeout, Some(Duration::from_secs(5)));
    assert!(both.only);
    // A viewport replaces the file's device.
    assert_eq!(both.device, None);
    assert!(both.viewport.is_some());
}

#[test]
fn directives_are_checked() {
    let cases: &[(&str, &str)] = &[
        (
            "goto /\n@test \"x\"\n  reload\n",
            "s.cdp:1:1: a command outside a block",
        ),
        ("@test\n  reload\n", "s.cdp:1:1: @test needs a name"),
        ("@test \"x\"\n", "s.cdp:1:1: test \"x\" has no steps"),
        (
            "@tset \"x\"\n  reload\n",
            "s.cdp:1:1: unknown directive @tset",
        ),
        (
            "@test \"x\" @retries\n  reload\n",
            "s.cdp:1:11: @retries takes a number",
        ),
        (
            "@timeout soon\n@test \"x\"\n  reload\n",
            "s.cdp:1:1: @timeout takes one duration",
        ),
        (
            "@viewport big\n@test \"x\"\n  reload\n",
            "s.cdp:1:1: @viewport takes WIDTHxHEIGHT",
        ),
        (
            "@test \"x\"\n  @before\n    reload\n",
            "s.cdp:2:3: @before inside a @test",
        ),
        (
            "@before\n  @mocks a\n@test \"x\"\n  reload\n",
            "s.cdp:2:3: @mocks inside @before",
        ),
        (
            "@before\n  goto /\n@before\n  reload\n@test \"x\"\n  reload\n",
            "s.cdp:3:1: a second @before block",
        ),
        (
            "@include steps.cdp\n@test \"x\"\n  reload\n",
            "s.cdp:1:1: @include outside a block",
        ),
        (
            "@mocks\n  goto /\n@test \"x\"\n  reload\n",
            "s.cdp:1:1: @mocks needs a mock set",
        ),
        ("# nothing\n", "s.cdp:1:1: the scenario has no @test"),
        (
            "@test \"a\" \"b\"\n  reload\n",
            "s.cdp:1:11: a test name is one word",
        ),
    ];
    for (text, expected) in cases {
        let errors = load_text(text).unwrap_err();
        assert!(
            errors.iter().any(|e| e == expected),
            "{text:?}: {errors:?}, expected {expected}"
        );
    }
    // A known name close to the typo is suggested.
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "s.cdp", "@tset \"x\"\n  reload\n");
    let errors = load(&path, "s.cdp").unwrap_err();
    assert_eq!(errors[0].hint.as_deref(), Some("did you mean @test?"));
    // A skipped test may have no steps yet.
    assert!(load_text("@test \"later\" @skip\n").is_ok());
}

#[test]
fn includes_inline_step_files_with_parameters() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "steps/login.cdp",
        "# log in\ngoto /login\nfill label=Email ${email}\n@include submit.cdp label=${email}\n",
    );
    write(dir.path(), "steps/submit.cdp", "click text=${label}\n");
    let path = write(
        dir.path(),
        "scenarios/a.cdp",
        "@test \"x\"\n  @include ../steps/login.cdp email=a@example.com\n  expect text=Hi visible\n",
    );
    let s = load(&path, "a.cdp").unwrap();
    let steps = &s.tests[0].steps;
    let texts: Vec<&str> = steps.iter().map(|s| s.line.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "goto /login",
            "fill label=Email ${email}",
            "click text=${label}",
            "expect text=Hi visible"
        ]
    );
    let expanded = steps[1].line.words[2]
        .expand(steps[1].vars.as_ref())
        .unwrap();
    assert_eq!(expanded, "a@example.com");
    let expanded = steps[2].line.words[1]
        .expand(steps[2].vars.as_ref())
        .unwrap();
    assert_eq!(expanded, "text=a@example.com");
    assert_eq!(steps[2].via.len(), 2);
    assert_eq!(steps[3].via.len(), 0);

    // A missing parameter says where to pass it.
    let missing = steps[1]
        .vars
        .resolve(&protocol::script::Var::Name("password".into()))
        .unwrap_err();
    assert!(missing.hint.unwrap().contains("a.cdp:2:3"));

    // Cycles and missing files.
    write(dir.path(), "steps/loop.cdp", "@include loop.cdp\n");
    let path = write(
        dir.path(),
        "scenarios/b.cdp",
        "@test \"x\"\n  @include ../steps/loop.cdp\n",
    );
    let errors = load(&path, "b.cdp").unwrap_err();
    assert!(
        errors[0].message.starts_with("@include cycle: "),
        "{errors:?}"
    );
    let path = write(
        dir.path(),
        "scenarios/c.cdp",
        "@test \"x\"\n  @include nope.cdp\n",
    );
    let errors = load(&path, "c.cdp").unwrap_err();
    assert!(errors[0].message.starts_with("cannot read "), "{errors:?}");
    write(dir.path(), "steps/bad.cdp", "@test \"inner\"\n  reload\n");
    let path = write(
        dir.path(),
        "scenarios/d.cdp",
        "@test \"x\"\n  @include ../steps/bad.cdp\n",
    );
    let errors = load(&path, "d.cdp").unwrap_err();
    assert!(
        errors.iter().any(|e| e.message == "@test in a step file"),
        "{errors:?}"
    );
}

#[test]
fn globs_match_paths() {
    assert!(glob_matches(
        "e2e/scenarios/**/*.cdp",
        "e2e/scenarios/a.cdp"
    ));
    assert!(glob_matches(
        "e2e/scenarios/**/*.cdp",
        "e2e/scenarios/x/y/a.cdp"
    ));
    assert!(!glob_matches("e2e/scenarios/**/*.cdp", "e2e/steps/a.cdp"));
    assert!(glob_matches("./tests/*.cdp", "tests/a.cdp"));
    assert!(!glob_matches("tests/*.cdp", "tests/x/a.cdp"));
    assert!(glob_matches("t?sts/**", "tests/x/a.cdp"));
}

#[test]
fn targets_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = write(dir.path(), "e2e/scenarios/a.cdp", "");
    let b = write(dir.path(), "e2e/scenarios/sub/b.cdp", "");
    write(dir.path(), "e2e/scenarios/notes.txt", "");
    write(dir.path(), "e2e/steps/login.cdp", "");
    write(dir.path(), "node_modules/x/c.cdp", "");
    assert_eq!(
        files_matching(dir.path(), &["e2e/scenarios/**/*.cdp".into()]),
        [a.clone(), b.clone()]
    );
    assert_eq!(
        parse_target("e2e/scenarios/a.cdp:12", dir.path()),
        Target {
            path: a.clone(),
            line: Some(12)
        }
    );
    let files = files_of(&[
        parse_target("e2e/scenarios/a.cdp:12", dir.path()),
        parse_target("e2e/scenarios/sub", dir.path()),
    ])
    .unwrap();
    assert_eq!(files.get(&a), Some(&vec![12]));
    assert_eq!(files.get(&b), Some(&vec![]));
    // Named whole too, a file runs whole.
    let files = files_of(&[
        parse_target("e2e/scenarios/a.cdp:12", dir.path()),
        parse_target("e2e/scenarios/a.cdp", dir.path()),
    ])
    .unwrap();
    assert_eq!(files.get(&a), Some(&vec![]));
    assert!(files_of(&[parse_target("nope.cdp", dir.path())]).is_err());
}

#[test]
fn selection_by_line_grep_tags_and_only() {
    let s = load_text(CHECKOUT).unwrap();
    let all = select(std::slice::from_ref(&s), &[vec![]], &Filter::default()).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[3].skipped(), Some("flaky upstream component, see #12"));
    assert_eq!(all[0].title, "s.cdp > lists products");

    let at = select(std::slice::from_ref(&s), &[vec![14]], &Filter::default()).unwrap();
    assert_eq!(at.len(), 1);
    assert_eq!(at[0].test, 1);
    let err = select(std::slice::from_ref(&s), &[vec![5]], &Filter::default()).unwrap_err();
    assert_eq!(err.message, "s.cdp:5 is not in a test");

    let grep = Filter {
        grep: Some(Regex::new("API").unwrap()),
        ..Filter::default()
    };
    assert_eq!(
        select(std::slice::from_ref(&s), &[vec![]], &grep)
            .unwrap()
            .len(),
        2
    );
    let mut tags = Filter::default();
    tags.set_tags("smoke,!checkout");
    assert!(
        select(std::slice::from_ref(&s), &[vec![]], &tags)
            .unwrap()
            .is_empty()
    );
    let mut tags = Filter::default();
    tags.set_tags("nope, smoke");
    assert_eq!(
        select(std::slice::from_ref(&s), &[vec![]], &tags)
            .unwrap()
            .len(),
        4
    );

    let only = load_text("@test \"a\"\n  reload\n@test \"b\" @only\n  reload\n").unwrap();
    let picked = select(&[only], &[vec![]], &Filter::default()).unwrap();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].test, 1);
}

#[test]
fn shards_cover_every_test_once() {
    for len in 0..12 {
        for total in 1..6 {
            let mut seen = Vec::new();
            for index in 1..=total {
                let part = shard((0..len).collect::<Vec<_>>(), index, total);
                assert!(part.len() <= len / total + 1);
                seen.extend(part);
            }
            assert_eq!(seen, (0..len).collect::<Vec<_>>(), "{len} in {total}");
        }
    }
    assert_eq!(parse_shard("2/4").unwrap(), (2, 4));
    for bad in ["0/4", "5/4", "1/0", "x", "1-2"] {
        assert!(parse_shard(bad).is_err(), "{bad}");
    }
}

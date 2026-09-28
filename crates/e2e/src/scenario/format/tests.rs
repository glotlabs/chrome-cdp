use std::path::{Path, PathBuf};

use proptest::prelude::*;
use protocol::script;

use super::{Plain, format, shape};

fn fmt(src: &str) -> String {
    match format("t.cdp", src, &Plain) {
        Ok(text) => text,
        Err(errors) => panic!("{errors:?}"),
    }
}

/// Formats `src` and checks the properties every output has: it parses
/// to the same tree, and formatting it again changes nothing.
fn check(src: &str) -> String {
    let out = fmt(src);
    let before = script::parse("t.cdp", src);
    let after = script::parse("t.cdp", &out);
    assert!(after.errors.is_empty(), "{out}\n{:?}", after.errors);
    assert_eq!(
        shape(&before.script, &Plain),
        shape(&after.script, &Plain),
        "{out}"
    );
    assert_eq!(fmt(&out), out, "not idempotent:\n{out}");
    out
}

const MESSY: &str = r#"# e2e/scenarios/checkout.cdp
@tags smoke   checkout
@mocks base checkout             # layered onto default_mocks
@before
  goto /
@test "lists products"
  expect "role=listitem" count 3
      # indented deeper
  expect text=Mechanical\ keyboard visible
# about the next test


@test 'retries when the API is busy'   @tags slow @skip "flaky upstream"
  click 'role=button[name="Buy"]'   # the main button
  expect role=alert text "Please try again" # a toast


  expect request POST /api/orders --json-body='{"qty":1}' \
        --count 2
  fill label=Email "${var:email}"
  @include ../steps/login.cdp email="a@example.com"
@test "empty"
  @skip
"#;

const TIDY: &str = r#"# e2e/scenarios/checkout.cdp
@mocks base checkout  # layered onto default_mocks
@tags smoke checkout

@before
  goto /

@test "lists products"
  expect 'role=listitem' count 3
  # indented deeper
  expect text='Mechanical keyboard' visible

# about the next test

@test "retries when the API is busy" @skip "flaky upstream" @tags slow
  click 'role=button[name="Buy"]'            # the main button
  expect role=alert text 'Please try again'  # a toast

  expect request POST /api/orders --json-body='{"qty":1}' \
      --count 2
  fill label=Email "${var:email}"
  @include ../steps/login.cdp email="a@example.com"

@test "empty"
  @skip
"#;

#[test]
fn formats_a_messy_scenario() {
    assert_eq!(check(MESSY), TIDY);
}

#[test]
fn keeps_every_comment() {
    let out = check(MESSY);
    for comment in [
        "# e2e/scenarios",
        "# layered onto",
        "# about the next",
        "# the main button",
        "# a toast",
    ] {
        assert!(out.contains(comment), "{comment} lost:\n{out}");
    }
}

#[test]
fn quotes_command_words_single_unless_they_need_double() {
    assert_eq!(check("goto \"/a b\"\n"), "goto '/a b'\n");
    assert_eq!(check("fill e1 \"it's\"\n"), "fill e1 \"it's\"\n");
    assert_eq!(check("fill e1 'a'\"${x}\"\n"), "fill e1 \"a${x}\"\n");
    assert_eq!(check("fill e1 \"a\\nb\"\n"), "fill e1 \"a\\nb\"\n");
    // A literal ${ stays literal, in single quotes.
    assert_eq!(check("fill e1 \"$${x}\"\n"), "fill e1 '${x}'\n");
    // Unquoted words are left as they are.
    assert_eq!(check("click   '#id'   text=Go\n"), "click '#id' text=Go\n");
    assert_eq!(check("click   #id   text=Go\n"), "click  #id   text=Go\n");
    assert_eq!(check("eval \"\\d+\"\n"), "eval '\\d+'\n");
}

#[test]
fn quotes_directive_arguments_double() {
    assert_eq!(check("@test 'a'\n  goto /\n"), "@test \"a\"\n  goto /\n");
    assert_eq!(
        check("@test 'say \"hi\"'\n  goto /\n"),
        "@test \"say \\\"hi\\\"\"\n  goto /\n"
    );
}

#[test]
fn orders_settings_but_keeps_those_that_replace_each_other() {
    let src = "@device \"iPhone 15\"\n@viewport 390x844\n@tags a\n@mocks x\n@mocks y\n\n@test \"t\"\n  goto /\n";
    assert_eq!(
        check(src),
        "@mocks x\n@mocks y\n@tags a\n@device \"iPhone 15\"\n@viewport 390x844\n\n@test \"t\"\n  goto /\n"
    );
    // A comment or blank line ends a run: nothing moves across it.
    let src = "@tags a\n# why\n@mocks x\n\n@test \"t\"\n  goto /\n";
    assert_eq!(check(src), src);
}

#[test]
fn keeps_syntax_first() {
    let src = "# header\n@syntax 1   # the version\n@test \"t\"\n  goto /\n";
    assert_eq!(
        check(src),
        "# header\n@syntax 1  # the version\n\n@test \"t\"\n  goto /\n"
    );
}

#[test]
fn scripts_keep_their_blank_lines() {
    let src = "goto example.com\n\n\n\nclick text=More\ntext h1\n";
    assert_eq!(check(src), "goto example.com\n\nclick text=More\ntext h1\n");
}

#[test]
fn fails_on_syntax_errors() {
    let errors = format("t.cdp", "click 'open\n", &Plain).unwrap_err();
    assert!(errors[0].message.contains("closing quote"), "{errors:?}");
}

#[test]
fn empty_files_stay_empty() {
    assert_eq!(check(""), "");
    assert_eq!(check("\n\n"), "");
    assert_eq!(check("# only\n"), "# only\n");
}

/// Every `.cdp` file of the repository's fixtures and docs: formatting
/// keeps its tree and is idempotent.
#[test]
fn every_fixture_formats_and_keeps_its_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["testdata", "docs", "examples"] {
        collect(&root.join(dir), &mut files);
    }
    assert!(files.len() > 5, "{files:?}");
    for file in files {
        let src = std::fs::read_to_string(&file).unwrap();
        if !script::parse("t.cdp", &src).errors.is_empty() {
            continue; // fixtures of syntax errors
        }
        let out = check(&src);
        assert!(
            !out.is_empty() || src.trim().is_empty(),
            "{}",
            file.display()
        );
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "cdp") {
            out.push(path);
        }
    }
}

/// A random line: indentation, then words from a pool of the forms the
/// grammar has, maybe a comment and a continuation.
fn line() -> impl Strategy<Value = String> {
    let word = prop::sample::select(vec![
        "click",
        "goto",
        "@test",
        "@tags",
        "@mocks",
        "@skip",
        "@include",
        "@before",
        "a",
        "e12",
        "'q w'",
        "\"d q\"",
        "\"${x}\"",
        "${env:HOME}",
        "'it'\"'\"'s'",
        "--flag='v w'",
        "k=\"v\"",
        "\"\\d\"",
        "\"a\\\\\"",
        "$${y}",
        "\"$${z}\"",
        "#id",
        "\"\"",
        "''",
        "x\\ y",
        "\"\\\"\"",
        "@viewport",
        "@device",
        "1x2",
        "@only",
    ]);
    (
        prop::sample::select(vec![0usize, 0, 2, 2, 4, 1]),
        prop::collection::vec(word, 1..6),
        prop::option::of(prop::sample::select(vec!["# c", "#x", "#  wide  "])),
        prop::bool::weighted(0.1),
    )
        .prop_map(|(indent, words, comment, continued)| {
            let mut line = " ".repeat(indent);
            for (i, w) in words.iter().enumerate() {
                if i > 0 {
                    line.push_str(if continued && i == 1 {
                        " \\\n    "
                    } else {
                        " "
                    });
                }
                line.push_str(w);
            }
            if let Some(c) = comment {
                line.push_str("   ");
                line.push_str(c);
            }
            line
        })
}

/// A random block: a top-level line, then lines indented 2 below it.
fn block() -> impl Strategy<Value = String> {
    let head = prop::sample::select(vec![
        "@test \"t\"",
        "@test 'u' @skip @tags a",
        "@before",
        "@mocks a",
        "@tags b",
        "goto /",
        "# c",
        "@viewport 1x2",
        "@device d",
        "@only",
    ]);
    let body = (line(), prop::bool::ANY).prop_map(|(line, blank)| {
        let line = format!("  {}", line.trim_start());
        if blank { format!("\n{line}") } else { line }
    });
    (head, prop::collection::vec(body, 0..4), prop::bool::ANY).prop_map(|(head, body, blank)| {
        let mut text = if blank {
            "\n".to_owned()
        } else {
            String::new()
        };
        text.push_str(head);
        for line in body {
            text.push('\n');
            text.push_str(&line);
        }
        text
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn idempotent_and_keeps_the_tree_of_blocks(blocks in prop::collection::vec(block(), 0..8)) {
        let src = blocks.join("\n") + "\n";
        if script::parse("t.cdp", &src).errors.is_empty() {
            check(&src);
        }
    }

    /// `fmt(fmt(x)) == fmt(x)`, and the tree is kept, for every script
    /// that parses.
    #[test]
    fn idempotent_and_keeps_the_tree(lines in prop::collection::vec(
        prop_oneof![line(), Just(String::new())], 0..12)
    ) {
        let src = lines.join("\n") + "\n";
        if script::parse("t.cdp", &src).errors.is_empty() {
            check(&src);
        }
    }
}

#[test]
fn every_setting_has_a_place_in_the_order() {
    for setting in crate::scenario::SETTINGS {
        assert!(
            super::ORDER.iter().any(|group| group.contains(setting)),
            "@{setting} is not in ORDER"
        );
    }
}

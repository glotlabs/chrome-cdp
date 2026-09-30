//! One test per rule of `docs/scenario-syntax.md`, in its order, plus a
//! property test that the parser never panics and keeps spans inside the
//! input (the `fuzz/` target checks the same with libFuzzer).

use proptest::prelude::*;

use super::{Item, Line, Parsed, Part, Resolve, Script, Unresolved, Var, Word, parse};

fn ok(src: &str) -> Script {
    let Parsed { script, errors } = parse("t.cdp", src);
    assert!(errors.is_empty(), "{src:?}: {errors:#?}");
    script
}

/// The messages of the errors, each prefixed with `line:col`.
fn errors(src: &str) -> Vec<String> {
    parse("t.cdp", src)
        .errors
        .into_iter()
        .map(|e| format!("{}:{} {}", e.span.line, e.span.column, e.message))
        .collect()
}

fn lines(script: &Script) -> Vec<&Line> {
    script.commands().collect()
}

/// Each command line's words, with variables shown as `${…}`.
fn words(src: &str) -> Vec<Vec<String>> {
    lines(&ok(src))
        .iter()
        .map(|l| l.words.iter().map(show).collect())
        .collect()
}

fn show(w: &Word) -> String {
    w.parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.clone(),
            Part::Var(v) => v.var.to_string(),
        })
        .collect()
}

// Lines and indentation.

#[test]
fn one_command_per_line() {
    assert_eq!(
        words("goto /login\nfill label=Email ada@example.com\n\nclick e6"),
        [
            vec!["goto", "/login"],
            vec!["fill", "label=Email", "ada@example.com"],
            vec!["click", "e6"],
        ]
    );
}

#[test]
fn crlf_line_ends_and_a_missing_last_newline_are_fine() {
    assert_eq!(
        words("goto a\r\nclick b\r\n"),
        [vec!["goto", "a"], vec!["click", "b"]]
    );
    assert_eq!(words("click b"), [vec!["click", "b"]]);
    assert!(ok("").items.is_empty());
    assert!(ok("\n  \n\t\n").items.is_empty());
}

#[test]
fn spaces_and_tabs_separate_words() {
    assert_eq!(
        words("click   e6\t--force"),
        [vec!["click", "e6", "--force"]]
    );
}

#[test]
fn a_directive_body_is_indented_two_spaces() {
    let script = ok("@test \"logs in\"\n  goto /login\n  click e1\ngoto /");
    let [Item::Directive(d), Item::Command(after)] = script.items.as_slice() else {
        panic!("{:#?}", script.items)
    };
    assert_eq!(d.name, "test");
    assert_eq!(d.args.len(), 1);
    assert_eq!(show(&d.args[0]), "logs in");
    assert_eq!(d.body.len(), 2);
    assert_eq!(after.text, "goto /");
}

#[test]
fn directive_bodies_nest() {
    let script = ok("@test a\n  @mocks errors\n    reload\n  click e1\n");
    let Item::Directive(test) = &script.items[0] else {
        panic!()
    };
    let [Item::Directive(mocks), Item::Command(click)] = test.body.as_slice() else {
        panic!("{:#?}", test.body)
    };
    assert_eq!(mocks.body.len(), 1);
    assert_eq!(click.text, "click e1");
}

#[test]
fn indentation_other_than_a_body_is_an_error() {
    assert_eq!(
        errors("goto a\n  click b\n  click c\nclick d"),
        ["2:1 unexpected indentation: 2 spaces where 0 were expected"]
    );
    assert_eq!(
        errors("@test a\n    click b"),
        ["2:3 unexpected indentation: 4 spaces where 2 were expected"]
    );
    assert_eq!(
        errors(" click b"),
        ["1:1 unexpected indentation: 1 spaces where 0 were expected"]
    );
}

#[test]
fn tabs_may_not_indent() {
    assert_eq!(errors("\tclick b"), ["1:1 a tab in the indentation"]);
    assert_eq!(
        errors("@test a\n  \tclick b"),
        ["2:3 a tab in the indentation"]
    );
}

// Comments.

#[test]
fn a_hash_at_the_start_of_a_line_is_a_comment() {
    let script = ok("# logs in\n  # indented\nclick e1");
    let [Item::Comment(a), Item::Comment(b), Item::Command(_)] = script.items.as_slice() else {
        panic!("{:#?}", script.items)
    };
    assert_eq!(a.text, " logs in");
    assert_eq!(b.text, " indented");
    assert_eq!((b.span.line, b.span.column), (2, 3));
}

#[test]
fn a_hash_after_whitespace_starts_a_comment() {
    let script = ok("click e1 # the button\n");
    let line = lines(&script)[0];
    assert_eq!(line.words.len(), 2);
    assert_eq!(line.comment.as_ref().unwrap().text, " the button");
}

#[test]
fn a_hash_inside_a_word_or_quotes_is_text() {
    assert_eq!(
        words("click css=#go '#id' \"a # b\" x#y"),
        [vec!["click", "css=#go", "#id", "a # b", "x#y"]]
    );
    // A bare `#submit` is a comment, as in a shell.
    assert_eq!(words("click #submit"), [vec!["click"]]);
}

#[test]
fn comments_may_sit_inside_a_body_at_any_depth() {
    let script = ok("@test a\n  # step one\n  click e1\n# between\n@test b\n  click e2");
    let [Item::Directive(a), Item::Comment(_), Item::Directive(b)] = script.items.as_slice() else {
        panic!("{:#?}", script.items)
    };
    assert_eq!(a.body.len(), 2);
    assert_eq!(b.body.len(), 1);
}

// Quoting.

#[test]
fn single_quotes_are_literal() {
    assert_eq!(
        words(r#"fill e1 'a "b" \n ${x} $${y}'"#),
        [vec!["fill", "e1", r#"a "b" \n ${x} $${y}"#]]
    );
}

#[test]
fn double_quotes_take_three_escapes_and_variables() {
    assert_eq!(
        words(r#"fill e1 "say \"hi\"\\ \n ${who}""#),
        [vec!["fill", "e1", "say \"hi\"\\ \n ${who}"]]
    );
    let script = ok(r#"fill e1 "a ${who}""#);
    let word = &lines(&script)[0].words[2];
    assert!(matches!(word.parts[1], Part::Var(_)));
}

#[test]
fn other_backslashes_in_double_quotes_stay() {
    assert_eq!(
        words(r#"wait --url "/\d+/""#),
        [vec!["wait", "--url", r"/\d+/"]]
    );
}

#[test]
fn a_backslash_outside_quotes_escapes_one_character() {
    assert_eq!(
        words(r"click \#submit a\ b \'x"),
        [vec!["click", "#submit", "a b", "'x"]]
    );
}

#[test]
fn quoted_and_bare_parts_join_into_one_word() {
    assert_eq!(
        words(r#"click role=button[name="Sign in"]x'y'"#),
        [vec!["click", "role=button[name=Sign in]xy"]]
    );
    let script = ok("fill e1 '' plain");
    let line = lines(&script)[0];
    assert_eq!(show(&line.words[2]), "");
    assert!(line.words[2].quoted);
    assert!(!line.words[3].quoted);
}

#[test]
fn quotes_must_close_on_the_same_line() {
    assert_eq!(
        errors("fill e1 'abc\nclick e2"),
        ["1:9 a single quote without its closing quote"]
    );
    assert_eq!(
        errors("fill e1 \"abc\\\"\n"),
        ["1:9 a double quote without its closing quote"]
    );
}

#[test]
fn no_other_shell_features() {
    // Globs, pipes, `;` and `$(…)` are plain text.
    assert_eq!(
        words("eval 'x' | *.png ; $(date) $HOME ~"),
        [vec![
            "eval", "x", "|", "*.png", ";", "$(date)", "$HOME", "~"
        ]]
    );
}

// Line continuation.

#[test]
fn a_trailing_space_backslash_continues_the_line() {
    let script = ok("fill label=Email \\\n    ada@example.com # who\nclick e1");
    let all = lines(&script);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].text, "fill label=Email ada@example.com");
    assert_eq!(all[0].words[2].span.line, 2);
    assert_eq!(all[0].comment.as_ref().unwrap().text, " who");
}

#[test]
fn a_continuation_needs_a_space_and_a_next_line() {
    assert_eq!(
        errors("click e1\\\nx"),
        ["1:9 a backslash at the end of a word"]
    );
    assert_eq!(
        errors("click e1 \\"),
        ["1:10 a line continuation at the end of the file"]
    );
    assert_eq!(
        errors("click e1 \\\n\nclick e2"),
        ["1:10 a line continuation followed by a blank line"]
    );
}

#[test]
fn a_comment_does_not_continue() {
    assert_eq!(
        words("click e1 # not continued \\\nclick e2"),
        [vec!["click", "e1"], vec!["click", "e2"]]
    );
}

// Variables.

#[test]
fn variable_kinds() {
    let vars = |src: &str| -> Vec<Var> {
        lines(&ok(src))[0]
            .words
            .iter()
            .flat_map(|w| w.vars().map(|v| v.var.clone()))
            .collect()
    };
    assert_eq!(
        vars("x ${email} ${env:HOME} ${var:product} ${secret:pw} ${test.id} ${run.id}"),
        [
            Var::Name("email".into()),
            Var::Env("HOME".into()),
            Var::Var("product".into()),
            Var::Secret("pw".into()),
            Var::Path(vec!["test".into(), "id".into()]),
            Var::Path(vec!["run".into(), "id".into()]),
        ]
    );
    assert_eq!(
        vars(
            "x ${data:state/user.json#/email} ${data:all.json} ${saved.order.id} ${saved.o.items.0.sku}"
        ),
        [
            Var::Data {
                file: "state/user.json".into(),
                pointer: "/email".into()
            },
            Var::Data {
                file: "all.json".into(),
                pointer: String::new()
            },
            Var::Path(vec!["saved".into(), "order".into(), "id".into()]),
            Var::Path(vec![
                "saved".into(),
                "o".into(),
                "items".into(),
                "0".into(),
                "sku".into()
            ]),
        ]
    );
}

#[test]
fn variables_have_spans_and_join_text() {
    let script = ok("fill e1 e2e+${test.id}@example.com");
    let word = &lines(&script)[0].words[2];
    assert_eq!(word.parts.len(), 3);
    let var = word.vars().next().unwrap();
    assert_eq!((var.span.line, var.span.column), (1, 13));
    assert_eq!(var.span.end - var.span.start, "${test.id}".len());
}

#[test]
fn dollar_dollar_brace_is_a_literal() {
    assert_eq!(
        words(r#"eval '1' $${x} "$${y}" a$b $"#),
        [vec!["eval", "1", "${x}", "${y}", "a$b", "$"]]
    );
}

#[test]
fn bad_variables_are_errors() {
    assert_eq!(errors("x ${}"), ["1:3 an empty variable ${}"]);
    assert_eq!(errors("x ${a"), ["1:3 a ${ without its closing }"]);
    assert_eq!(
        errors("x ${foo:bar}"),
        ["1:3 unknown variable kind foo: in ${foo:bar}"]
    );
    assert_eq!(errors("x ${env:A-B}"), ["1:3 bad variable ${env:A-B}"]);
    assert_eq!(
        errors("x ${data:f#ptr}"),
        ["1:3 bad variable ${data:f#ptr}"]
    );
    assert_eq!(errors("x ${a..b}"), ["1:3 bad variable name ${a..b}"]);
    assert_eq!(errors("x ${1a}"), ["1:3 bad variable name ${1a}"]);
    assert_eq!(errors("x ${0.a}"), ["1:3 bad variable name ${0.a}"]);
    assert_eq!(errors("x ${a.-1}"), ["1:3 bad variable name ${a.-1}"]);
}

#[test]
fn expansion_fills_in_variables_without_splitting_words() {
    struct Vars;
    impl Resolve for Vars {
        fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
            match var {
                Var::Name(n) if n == "who" => Ok("Ada Lovelace".into()),
                other => Err(Unresolved::new(format!("no {other}")).hint("pass it")),
            }
        }
    }
    let script = ok("fill e1 \"hi ${who}!\" ${nope}");
    let line = lines(&script)[0];
    assert_eq!(line.words[2].expand(&Vars).unwrap(), "hi Ada Lovelace!");
    let err = line.words[3].expand(&Vars).unwrap_err();
    assert_eq!(err.to_string(), "t.cdp:1:22: no ${nope}");
    assert_eq!(err.hint.as_deref(), Some("pass it"));
    assert_eq!(line.words[1].literal().as_deref(), Some("e1"));
    assert_eq!(line.words[2].literal(), None);
}

// Directives.

#[test]
fn directives_take_words_and_a_comment() {
    let script = ok("@include ../steps/login.cdp email=${env:EMAIL} # log in\n");
    let Item::Directive(d) = &script.items[0] else {
        panic!()
    };
    assert_eq!(d.name, "include");
    assert_eq!(d.args.len(), 2);
    assert_eq!(d.comment.as_ref().unwrap().text, " log in");
    assert!(d.body.is_empty());
    // `@` later in a line, or quoted, is an ordinary word.
    assert_eq!(
        words("fill e1 @ada '@x'"),
        [vec!["fill", "e1", "@ada", "@x"]]
    );
}

#[test]
fn directive_names_are_lowercase() {
    assert_eq!(errors("@Test a"), ["1:1 bad directive name @Test"]);
    assert_eq!(errors("@ x"), ["1:1 bad directive name @"]);
}

#[test]
fn syntax_version() {
    assert_eq!(ok("@syntax 1\nclick e1").syntax, 1);
    let script = ok("# header\n@syntax 1\nclick e1");
    assert!(!script.items.iter().any(|i| matches!(i, Item::Directive(_))));
    assert_eq!(ok("click e1").syntax, super::SYNTAX_VERSION);
    assert_eq!(
        errors("@syntax 2"),
        ["1:9 syntax version 2 is newer than this chrome-cdp reads (1)"]
    );
    assert_eq!(errors("@syntax one"), ["1:9 bad syntax version one"]);
    assert_eq!(errors("@syntax"), ["1:1 @syntax takes one version number"]);
    assert_eq!(
        errors("click e1\n@syntax 1"),
        ["2:1 @syntax must be the first line of the file"]
    );
    assert_eq!(errors("@syntax 1\n  click e1"), ["2:3 @syntax has no body"]);
}

// Spans.

#[test]
fn spans_name_the_file_line_and_character_column() {
    let script = ok("goto /\n# ü\nfill 'ü' ada");
    let line = lines(&script)[1];
    assert_eq!(line.span.to_string(), "t.cdp:3:1");
    assert_eq!(line.words[2].span.to_string(), "t.cdp:3:10");
    assert_eq!(line.words[1].raw, "'ü'");
}

#[test]
fn errors_on_several_lines_are_all_reported() {
    assert_eq!(
        errors("fill 'a\nclick e1\nfill \"b\n@Bad"),
        [
            "1:6 a single quote without its closing quote",
            "3:6 a double quote without its closing quote",
            "4:1 bad directive name @Bad",
        ]
    );
    let parsed = parse("t.cdp", "fill 'a\nclick e1");
    assert_eq!(parsed.script.commands().count(), 1);
}

/// Every span lies within the input, on char boundaries, and its line and
/// column match its start.
fn check_spans(src: &str) {
    let Parsed { script, errors } = parse("f", src);
    let mut spans = Vec::new();
    collect(&script.items, &mut spans);
    spans.extend(errors.iter().map(|e| e.span.clone()));
    for span in spans {
        assert!(span.start <= span.end && span.end <= src.len(), "{span:?}");
        assert!(src.is_char_boundary(span.start) && src.is_char_boundary(span.end));
        let before = &src[..span.start];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        assert_eq!((span.line as usize, span.column as usize), (line, column));
    }
}

fn collect(items: &[Item], out: &mut Vec<super::Span>) {
    for item in items {
        match item {
            Item::Command(l) => {
                out.push(l.span.clone());
                out.extend(l.comment.iter().map(|c| c.span.clone()));
                for w in &l.words {
                    out.push(w.span.clone());
                    out.extend(w.vars().map(|v| v.span.clone()));
                }
            }
            Item::Directive(d) => {
                out.push(d.span.clone());
                out.extend(d.comment.iter().map(|c| c.span.clone()));
                for w in &d.args {
                    out.push(w.span.clone());
                    out.extend(w.vars().map(|v| v.span.clone()));
                }
                collect(&d.body, out);
            }
            Item::Comment(c) => out.push(c.span.clone()),
        }
    }
}

proptest! {
    #[test]
    fn never_panics_and_spans_stay_inside(src in "[ -~\t\n\r'\"\\\\$#@{}üé]{0,200}") {
        check_spans(&src);
    }

    #[test]
    fn any_bytes_as_text(src in any::<String>()) {
        check_spans(&src);
    }
}

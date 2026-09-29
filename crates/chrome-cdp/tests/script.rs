//! `batch` and `run` (plan 08, milestone 8.2) end to end: scripts are
//! checked before any step runs, steps print as they end, a failure stops
//! the script unless `--continue`, and `--json` prints a line per step.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::process::{Command, Output, Stdio};

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout};
use fixtures::Fixtures;
use serde_json::Value;

/// Runs `cmd` with `input` on stdin.
fn with_stdin(cmd: &mut Command, input: &str) -> Output {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    eprintln!(
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn batch(sandbox: &Sandbox, args: &[&str], input: &str) -> Output {
    with_stdin(sandbox.cli().arg("batch").args(args), input)
}

/// Each line of stdout as JSON.
fn json_lines(out: &Output) -> Vec<Value> {
    stdout(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_bad_script_fails_before_anything_runs() {
    let sandbox = Sandbox::new();
    let out = batch(
        &sandbox,
        &[],
        "open\n# a typo, a bad quote and a stray indent\nclik e6\nfill e1 'ada\n  goto /\n",
    );
    assert_eq!(code(&out), 2);
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(
        err.starts_with("error: <stdin>:3:1: unrecognized subcommand 'clik'\n"),
        "{err}"
    );
    assert!(
        err.contains("  tip: a similar subcommand exists: 'click'\n"),
        "{err}"
    );
    assert!(
        err.contains("  also: <stdin>:4:9: a single quote without its closing quote\n"),
        "{err}"
    );
    assert!(
        err.contains(
            "  also: <stdin>:5:1: unexpected indentation: 2 spaces where 0 were expected\n"
        ),
        "{err}"
    );
    // `open` did not run: no daemon started.
    assert!(sandbox.daemons().is_empty());

    // One error envelope with every problem in `details`.
    let out = batch(&sandbox, &["--json"], "clik e6\nfill e1 'ada\n");
    assert_eq!(code(&out), 2);
    let v = json_out(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["kind"], "BadArgs");
    let problems = v["error"]["details"]["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 2);
    assert_eq!(problems[1]["at"], "<stdin>:2:9");
}

#[test]
fn lines_are_checked_against_the_commands() {
    let sandbox = Sandbox::new();
    let check = |input: &str| {
        let out = batch(&sandbox, &[], input);
        assert_eq!(code(&out), 2);
        stderr(&out)
    };
    let err = check("click e1 --forse\n");
    assert!(
        err.starts_with("error: <stdin>:1:1: unexpected argument '--forse' found\n"),
        "{err}"
    );
    // The hint is the line with the flag clap suggests.
    assert!(err.contains("hint: chrome-cdp click e1 --force\n"), "{err}");

    // A bare #id is a comment, as in a shell; the error says so.
    let err = check("click #submit\n");
    assert!(
        err.starts_with(
            "error: <stdin>:1:1: the following required arguments were not provided: <LOCATOR> \
             (#submit starts a comment; quote it to pass it: '#submit')\n"
        ),
        "{err}"
    );

    // Locators are parsed too.
    let err = check("text h1\n");
    assert!(
        err.starts_with("error: <stdin>:1:1: `h1` is not a locator\n"),
        "{err}"
    );
    let err = check("drag e1 'role=button[name=\"x\"'\n");
    assert!(err.starts_with("error: <stdin>:1:1: "), "{err}");

    let err = check("batch\n");
    assert!(
        err.starts_with("error: <stdin>:1:1: batch, run and test cannot run inside a script\n"),
        "{err}"
    );
    let err = check("@test login\n  goto /\n");
    assert!(
        err.starts_with("error: <stdin>:1:1: unknown directive @test\n"),
        "{err}"
    );
    let err = check("@synax 1\ngoto /\n");
    assert!(
        err.starts_with(
            "error: <stdin>:1:1: unknown directive @synax\n  hint: did you mean @syntax?\n"
        ),
        "{err}"
    );
    let err = check("goto / --json\n");
    assert!(
        err.starts_with("error: <stdin>:1:1: --json on a line of a text script\n"),
        "{err}"
    );
    let err = check("# nothing\n\n");
    assert!(
        err.starts_with("error: <stdin>:1:1: the script has no commands\n"),
        "{err}"
    );
    let err = check("@syntax 2\ngoto /\n");
    assert!(
        err.starts_with(
            "error: <stdin>:1:9: syntax version 2 is newer than this chrome-cdp reads (1)\n"
        ),
        "{err}"
    );
}

#[test]
fn variables_must_have_values_before_the_script_starts() {
    let sandbox = Sandbox::new();
    let out = batch(&sandbox, &[], "fill label=Email ${email}\n");
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(
        err.starts_with("error: <stdin>:1:18: ${email} is not set\n"),
        "{err}"
    );
    assert!(
        err.contains("hint: chrome-cdp batch --var 'email=…'"),
        "{err}"
    );

    let out = with_stdin(
        sandbox
            .cli()
            .args(["batch"])
            .env_remove("CHROME_CDP_TEST_UNSET"),
        "goto ${env:CHROME_CDP_TEST_UNSET}\n",
    );
    assert!(
        stderr(&out).starts_with(
            "error: <stdin>:1:6: ${env:CHROME_CDP_TEST_UNSET}: the environment variable CHROME_CDP_TEST_UNSET is not set\n"
        ),
        "{}",
        stderr(&out)
    );
    let out = batch(&sandbox, &[], "goto /${test.id}\n");
    assert!(
        stderr(&out)
            .starts_with("error: <stdin>:1:7: ${test.id} has no value in batch and run scripts\n"),
        "{}",
        stderr(&out)
    );

    let out = with_stdin(sandbox.cli().args(["run", "missing.cdp"]), "");
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).starts_with("error: cannot read missing.cdp: "),
        "{}",
        stderr(&out)
    );

    let out = with_stdin(sandbox.cli().args(["batch", "--var", "1x=2"]), "");
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("`1x` is not a variable name"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn batch_runs_a_login_flow_in_one_call() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let script = format!(
        "# Sign in.
open {url}
fill label=Email ${{email}}
fill label=Password \"${{env:APP_PASSWORD}}\"   # from the environment
click 'role=button[name=\"Sign in\"]' \\
  --no-wait-after
text '#welcome'
",
        url = site.url("/script/login.html")
    );
    let out = with_stdin(
        sandbox
            .cli()
            .args(["batch", "--var", "email=ada@example.com"])
            .env("APP_PASSWORD", "s3cret"),
        &script,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    let headers: Vec<&str> = text.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(
        headers,
        [
            format!("[1/5] open {} ✓", site.url("/script/login.html")).as_str(),
            "[2/5] fill label=Email ${email} ✓",
            "[3/5] fill label=Password \"${env:APP_PASSWORD}\" ✓",
            "[4/5] click 'role=button[name=\"Sign in\"]' --no-wait-after ✓",
            "[5/5] text '#welcome' ✓",
        ]
    );
    assert!(
        text.ends_with("[5/5] text '#welcome' ✓\nWelcome, ada@example.com\n"),
        "{text}"
    );

    // --json: one line per step, with its place and its command's envelope.
    let out = batch(&sandbox, &["--json"], "text css=h1\ncount 'role=textbox'\n");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["step"], 1);
    assert_eq!(lines[0]["total"], 2);
    assert_eq!(lines[0]["at"], "<stdin>:1:1");
    assert_eq!(lines[0]["command"], "text css=h1");
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[1]["at"], "<stdin>:2:1");
    assert_eq!(lines[1]["data"]["count"], 2);

    // -q: only each step's value.
    let out = batch(&sandbox, &["-q"], "text css=h1\ncount 'role=textbox'\n");
    assert_eq!(stdout(&out), "Sign in\n2\n");

    // A step's `-` cannot read stdin: it holds the script.
    let out = batch(&sandbox, &[], "fill label=Email -\n");
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("cannot read TEXT from stdin: it holds the batch script"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_failed_step_stops_the_script_unless_continue() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", &site.url("/script/login.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let script = "text css=h1\nclick '#nope' --timeout 300ms\ntext css=h1\n";
    let out = batch(&sandbox, &[], script);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "[1/3] text css=h1 ✓\nSign in\n[2/3] click '#nope' --timeout 300ms ✗\n"
    );
    let err = stderr(&out);
    assert!(
        err.starts_with("error: <stdin>:2:1: no element matches #nope\n"),
        "{err}"
    );
    assert!(
        err.ends_with("stopped at step 2 of 3; 1 step did not run (--continue runs them anyway)\n"),
        "{err}"
    );

    let out = batch(&sandbox, &["--continue"], script);
    assert_eq!(code(&out), 1);
    assert!(
        stdout(&out).ends_with("[3/3] text css=h1 ✓\nSign in\n"),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).ends_with("1 of 3 steps failed: 2\n"),
        "{}",
        stderr(&out)
    );

    let out = batch(&sandbox, &["--json"], script);
    assert_eq!(code(&out), 1);
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["ok"], false);
    assert_eq!(lines[1]["error"]["kind"], "NotFound");
    assert_eq!(lines[1]["step"], 2);
}

#[test]
fn run_reads_a_file_with_vars_and_data() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let dir = sandbox.dir.path().join("e2e");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("users.json"),
        r#"[{"email": "grace@example.com", "password": "hopper"}]"#,
    )
    .unwrap();
    let file = dir.join("login.cdp");
    std::fs::write(
        &file,
        "@syntax 1\n\
         open ${site}/script/login.html\n\
         fill label=Email ${data:users.json#/0/email}\n\
         fill label=Password ${data:users.json#/0/password}\n\
         press Enter --in label=Password\n\
         wait --text 'Welcome, grace@example.com'\n",
    )
    .unwrap();
    let file = file.to_str().unwrap();
    let out = sandbox.run(&["run", file, "--var", &format!("site={}", site.url(""))]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("[5/5] wait --text 'Welcome, grace@example.com' ✓\n"),
        "{}",
        stdout(&out)
    );

    // Errors name the file.
    std::fs::write(
        dir.join("bad.cdp"),
        "goto /\nfill e1 ${data:users.json#/9}\n",
    )
    .unwrap();
    let bad = dir.join("bad.cdp");
    let out = sandbox.run(&["run", bad.to_str().unwrap()]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).starts_with(&format!(
            "error: {}:2:9: ${{data:users.json#/9}}: {} has nothing at /9\n",
            bad.display(),
            dir.join("users.json").display()
        )),
        "{}",
        stderr(&out)
    );

    // Lines inherit the global flags of the call.
    let out = sandbox.run(&[
        "run",
        file,
        "--session",
        "other",
        "--var",
        &format!("site={}", site.url("")),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["status", "--session", "other", "-q"]);
    assert_eq!(stdout(&out), "running\n");
}

#[test]
fn a_converted_playwright_test_passes_with_chrome_cdp_test() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let spec = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/scripts/login.spec.ts");
    let root = sandbox.dir.path().join("app");
    std::fs::create_dir_all(&root).unwrap();
    // `goto /script/login.html` resolves against the project's base_url.
    std::fs::write(
        root.join("chrome-cdp.toml"),
        format!("[e2e]\nbase_url = \"{}\"\n", site.a()),
    )
    .unwrap();
    let scenario = root.join("login.cdp");
    let out = sandbox.run(&[
        "convert",
        spec.to_str().unwrap(),
        "-o",
        scenario.to_str().unwrap(),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains(": 2 tests, 13 steps, 2 TODOs → "),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).starts_with("warning: 2 TODO lines"),
        "{}",
        stderr(&out)
    );

    // Each test runs in a context of its own, the @before steps first.
    let out = common::run(
        sandbox
            .cli()
            .current_dir(&root)
            .args(["test", "login.cdp"])
            .env("APP_PASSWORD", "s3cret"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("✓ login.cdp:11 sign in > welcomes the user ("),
        "{text}"
    );
    assert!(
        text.contains("✓ login.cdp:19 sign in > asks for the password ("),
        "{text}"
    );
    assert!(text.contains("\n2 tests: 2 passed ("), "{text}");
}

//! End-to-end tests of the `chrome-cdp` binary: flags, output modes, exit
//! codes, and `open` and `cdp` against a real Chrome.
//!
//! The real-Chrome tests need `CHROME_PATH`. Without it they pass as no-ops,
//! unless `CHROME_CDP_REQUIRE_CHROME=1` (set in CI), in which case they fail.

// clippy.toml's test allowances cover `#[test]` fns, not their helpers.
#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{Sandbox, chrome_path, cli, code, fake_chrome, json_out, run, stderr, stdout};
use serde_json::{Value, json};

#[test]
fn version_names_crate_git_and_protocol() {
    let out = run(cli().arg("--version"));
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    let expected_prefix = format!("chrome-cdp {} (git ", env!("CARGO_PKG_VERSION"));
    assert!(text.starts_with(&expected_prefix), "{text}");
    assert!(
        text.trim_end().ends_with(&format!(
            ", CDP protocol r{})",
            cdp::domains::PROTOCOL_REVISION
        )),
        "{text}"
    );
    assert!(stderr(&out).is_empty());

    let out = run(cli().args(["-V", "--json"]));
    let v = json_out(&out);
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["data"]["version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(
        v["data"]["protocol_revision"],
        json!(cdp::domains::PROTOCOL_REVISION)
    );
    let sha = v["data"]["git_sha"].as_str().unwrap();
    assert!(!sha.is_empty());

    let out = run(cli().args(["-V", "-q"]));
    assert_eq!(stdout(&out), format!("{}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_exits_zero_and_hides_the_daemon() {
    let out = run(cli().arg("--help"));
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for flag in ["--json", "--quiet", "--session", "--timeout", "--verbose"] {
        assert!(text.contains(flag), "{flag} missing from help");
    }
    assert!(!text.contains("__daemon"));
}

#[test]
fn no_command_is_a_usage_error() {
    let out = run(&mut cli());
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    assert_eq!(
        stderr(&out),
        "error: no command given\n  hint: chrome-cdp --help\n"
    );
}

#[test]
fn usage_errors_exit_2_and_honour_json() {
    let out = run(cli().arg("--bogus"));
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    assert!(stderr(&out).starts_with("error: unexpected argument '--bogus'"));

    let out = run(cli().args(["--bogus", "--json"]));
    assert_eq!(code(&out), 2);
    let v = json_out(&out);
    assert_eq!(v["ok"], json!(false));
    assert_eq!(v["error"]["kind"], json!("BadArgs"));
    assert_eq!(
        v["error"]["message"],
        json!("unexpected argument '--bogus' found")
    );

    let out = run(cli().args(["--timeout", "5000", "-V"]));
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("needs a unit"), "{}", stderr(&out));
}

#[test]
fn typos_hint_the_corrected_command() {
    let hint = |args: &[&str]| {
        let out = run(cli().args(args));
        assert_eq!(code(&out), 2);
        stderr(&out)
            .lines()
            .find_map(|l| l.strip_prefix("  hint: "))
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        hint(&["clik", "text=Sign in"]),
        "chrome-cdp click 'text=Sign in'"
    );
    assert_eq!(
        hint(&["click", "--jsn", "e1"]),
        "chrome-cdp click --json e1"
    );
    // clap offers check and schema; schema is closer.
    assert_eq!(hint(&["scheme", "version"]), "chrome-cdp schema version");
    // Nothing close: the help.
    assert_eq!(hint(&["frobnicate"]), "chrome-cdp --help");
    // Names that are values, not subcommands, get suggestions too.
    assert_eq!(
        hint(&["schema", "tab", "clos"]),
        "chrome-cdp schema tab close"
    );
    assert_eq!(hint(&["help", "waitng"]), "chrome-cdp help waiting");
}

#[test]
fn skill_installs_into_the_project_or_the_user_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let deep = repo.join("web").join("src");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::create_dir(repo.join(".git")).unwrap();
    let home = tmp.path().join("home");
    let skill = run(cli().arg("skill"));
    assert_eq!(code(&skill), 0);
    assert!(stdout(&skill).starts_with("---\nname: chrome-cdp\n"));

    let install = |args: &[&str]| {
        run(cli()
            .args(["skill", "--install"])
            .args(args)
            .current_dir(&deep)
            .env("HOME", &home)
            .env_remove("CLAUDE_CONFIG_DIR"))
    };
    let project = repo.join(".claude/skills/chrome-cdp/SKILL.md");
    let out = install(&[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("installed {}\n", project.display()));
    assert_eq!(std::fs::read_to_string(&project).unwrap(), stdout(&skill));

    // Again: nothing to do. An old copy is replaced.
    let out = install(&["-q"]);
    assert_eq!(stdout(&out), format!("{}\n", project.display()));
    let out = install(&["--json"]);
    assert_eq!(json_out(&out)["data"]["write"], json!("unchanged"));
    std::fs::write(&project, "old").unwrap();
    let out = install(&["--project"]);
    assert_eq!(stdout(&out), format!("updated {}\n", project.display()));
    assert_eq!(std::fs::read_to_string(&project).unwrap(), stdout(&skill));

    let out = install(&["--user"]);
    let user = home.join(".claude/skills/chrome-cdp/SKILL.md");
    assert_eq!(stdout(&out), format!("installed {}\n", user.display()));
    assert!(user.is_file());
}

#[test]
fn bash_completes_commands_flags_and_values() {
    let out = run(cli().args(["completions", "bash"]));
    assert_eq!(code(&out), 0);
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("chrome-cdp.bash");
    std::fs::write(&script, stdout(&out)).unwrap();
    // COMPREPLY for the words typed so far, the last one being completed.
    let complete = |words: &[&str]| {
        let quoted: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();
        let current = words.last().unwrap();
        let previous = words[words.len() - 2];
        let program = format!(
            "source '{}'; COMP_WORDS=({}); COMP_CWORD={}; \
             _chrome__cdp chrome-cdp '{current}' '{previous}'; \
             printf '%s\n' \"${{COMPREPLY[@]}}\"",
            script.display(),
            quoted.join(" "),
            words.len() - 1
        );
        let out = Command::new("bash")
            .args(["-c", &program])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(complete(&["chrome-cdp", "scre"]), "screenshot\n");
    assert_eq!(complete(&["chrome-cdp", "click", "--js"]), "--json\n");
    assert_eq!(complete(&["chrome-cdp", "tab", "cl"]), "close\n");
    assert_eq!(complete(&["chrome-cdp", "__d"]).trim(), "");
}

#[test]
fn global_flags_have_env_vars() {
    let out = run(cli().arg("-V").env("CHROME_CDP_JSON", "1"));
    assert_eq!(json_out(&out)["ok"], json!(true));
    let out = run(cli().arg("-V").env("CHROME_CDP_QUIET", "true"));
    assert_eq!(stdout(&out), format!("{}\n", env!("CARGO_PKG_VERSION")));
    let out = run(cli().arg("-V").env("CHROME_CDP_TIMEOUT", "soon"));
    assert_eq!(code(&out), 2);
    // A usage error found through the env var is still JSON.
    let out = run(cli().arg("--bogus").env("CHROME_CDP_JSON", "1"));
    assert_eq!(json_out(&out)["error"]["kind"], json!("BadArgs"));
}

#[test]
fn config_files_set_defaults_below_env_and_flags() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(&dir, "exec sleep 30");
    let project = dir.path().join("app/src");
    std::fs::create_dir_all(&project).unwrap();
    let user = dir.path().join("xdg/chrome-cdp");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("config.toml"), "timeout = \"200ms\"\n").unwrap();
    let timeout_message = |cmd: &mut Command| {
        let out = run(cmd
            .current_dir(&project)
            .env("XDG_CONFIG_HOME", dir.path().join("xdg")));
        assert_eq!(code(&out), 3);
        json_out(&out)["error"]["message"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let sandbox = Sandbox::with_chrome(&chrome);
    let call = || {
        let mut cmd = sandbox.cli();
        cmd.args(["open", "--json"]);
        cmd
    };

    // The user config applies…
    assert_eq!(
        timeout_message(&mut call()),
        "Chrome started but did not answer within 200ms"
    );
    // …the nearest project config (found walking up) beats it…
    std::fs::write(
        dir.path().join("app/.chrome-cdp.toml"),
        "timeout = \"300ms\"\n",
    )
    .unwrap();
    assert!(timeout_message(&mut call()).ends_with("within 300ms"));
    // …the env var beats both, and the flag beats the env var.
    assert!(timeout_message(call().env("CHROME_CDP_TIMEOUT", "400ms")).ends_with("within 400ms"));
    assert!(
        timeout_message(
            call()
                .env("CHROME_CDP_TIMEOUT", "400ms")
                .args(["--timeout", "250ms"])
        )
        .ends_with("within 250ms")
    );
}

#[test]
fn bad_config_is_a_usage_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(".chrome-cdp.toml");
    std::fs::write(&file, "sesion = \"shop\"\n").unwrap();
    let out = run(cli().arg("-V").current_dir(dir.path()));
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).is_empty());
    assert_eq!(
        stderr(&out),
        format!(
            "error: config {f}: unknown field `sesion`, expected one of `session`, `timeout`, \
             `base_url`, `testid_attribute`, `snap_after_action`, `e2e`\n  hint: fix or remove the key in {f}; known keys: session, \
             timeout, base_url, testid_attribute, snap_after_action, and the [e2e] section\n",
            f = file.display()
        )
    );
}

#[test]
fn cdp_checks_arguments_before_starting_a_daemon() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["cdp", "getVersion", "--json"]);
    assert_eq!(code(&out), 2);
    assert_eq!(json_out(&out)["error"]["kind"], json!("BadArgs"));

    let out = sandbox.run(&["cdp", "Runtime.evaluate", "{nope", "--json"]);
    assert_eq!(code(&out), 2);
    let err = &json_out(&out)["error"];
    assert!(
        err["message"]
            .as_str()
            .unwrap()
            .starts_with("params are not valid JSON")
    );
    assert!(
        err["hint"]
            .as_str()
            .unwrap()
            .contains(r#"'{"expression":"1+1"}'"#)
    );
    let out = sandbox.run(&["open", "--session", "a/b"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).starts_with("error: `a/b` is not a session name\n"),
        "{}",
        stderr(&out)
    );
    let out = sandbox.run(&["connect", "chrome"]);
    assert_eq!(code(&out), 2);
    assert!(!sandbox.runtime_dir().join("chrome-cdp").exists());
}

#[test]
fn a_missing_chrome_leaves_no_daemon_behind() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["open", "--chrome", "/nonexistent/chrome"]);
    assert_eq!(code(&out), 4);
    assert_eq!(
        stderr(&out),
        "error: no Chrome at /nonexistent/chrome\n  \
         hint: pass --chrome with the path of a Chrome or Chromium binary\n"
    );
    assert!(sandbox.daemons().is_empty());

    // `cdp` starts a browser for a new session, found the same way.
    let out = run(sandbox
        .cli()
        .args(["cdp", "Browser.getVersion"])
        .env("CHROME_CDP_CHROME", "/nonexistent/chrome"));
    assert_eq!(code(&out), 4);
    assert_eq!(
        stderr(&out),
        "error: no Chrome at /nonexistent/chrome (from CHROME_CDP_CHROME)\n  \
         hint: point CHROME_CDP_CHROME at a Chrome or Chromium binary, or unset it\n"
    );
    assert!(sandbox.daemons().is_empty());
    let out = sandbox.run(&["status", "-q"]);
    assert_eq!(stdout(&out), "stopped\n");
}

/// `open` in `sandbox` with no `--chrome`, an empty install cache and an
/// empty `PATH`, so only the fixed system locations remain.
fn discovery_cli(sandbox: &Sandbox) -> Command {
    let dir = sandbox.dir.path();
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let mut cmd = sandbox.cli();
    cmd.args(["open"])
        .env("CHROME_CDP_CACHE", dir.join("cache"))
        .env("PATH", &empty);
    cmd
}

#[test]
fn open_says_where_it_looked_for_chrome() {
    let system = ["/opt/google/chrome/chrome", "/snap/bin/chromium"];
    if !cfg!(target_os = "linux") || system.iter().any(|p| Path::new(p).exists()) {
        eprintln!("skipping: this machine has a system Chrome");
        return;
    }
    let sandbox = Sandbox::new();
    let out = run(discovery_cli(&sandbox).arg("--json"));
    assert_eq!(code(&out), 4);
    let err = &json_out(&out)["error"];
    assert_eq!(err["kind"], json!("ChromeNotFound"));
    assert_eq!(err["message"], json!("no Chrome or Chromium found"));
    let cache = sandbox.dir.path().join("cache/browsers");
    assert_eq!(
        err["details"]["searched"],
        json!([
            cache.display().to_string(),
            "PATH (google-chrome-stable, google-chrome, chromium, chromium-browser)",
            "/opt/google/chrome/chrome",
            "/snap/bin/chromium",
        ])
    );
    assert_eq!(
        err["hint"],
        json!("chrome-cdp install, or pass --chrome /path/to/chrome, or set CHROME_CDP_CHROME")
    );
}

#[test]
fn open_notes_a_too_old_chrome_when_launch_fails() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = dir.path().join("old-chrome");
    std::fs::write(
        &chrome,
        "#!/bin/sh\n[ \"$1\" = --version ] && { echo 'Chromium 120.0.6099.1'; exit 0; }\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&chrome, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 4);
    assert!(
        stderr(&out).contains(&format!(
            "\n  warning: Chromium 120.0.6099.1 at {} is older than Chrome 150, the oldest \
             chrome-cdp supports; some commands may fail\n",
            chrome.display()
        )),
        "{}",
        stderr(&out)
    );
    assert!(sandbox.daemons().is_empty());
}

#[test]
fn open_finds_chrome_without_being_told() {
    let Some(chrome) = chrome_path() else { return };
    // An install cache holding (a link to) the test Chrome, as
    // `chrome-cdp install` would lay it out.
    let sandbox = Sandbox::new();
    let id = "stable-154.0.0.0-linux64";
    let install = sandbox.dir.path().join("cache/browsers").join(id);
    std::fs::create_dir_all(install.join("chrome-linux64")).unwrap();
    std::os::unix::fs::symlink(&chrome, install.join("chrome-linux64/chrome")).unwrap();
    std::fs::write(install.join("INSTALLATION_COMPLETE"), "").unwrap();

    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        let out = run(discovery_cli(&sandbox).arg("--json"));
        assert_eq!(code(&out), 0);
        let browser = &json_out(&out)["data"]["browser"];
        assert_eq!(
            browser["source"],
            json!(format!("installed by chrome-cdp install ({id})"))
        );
        assert!(sandbox.run(&["close"]).status.success());
    }

    // The env var beats the cache.
    let out = run(discovery_cli(&sandbox)
        .env("CHROME_CDP_CHROME", &chrome)
        .arg("--json"));
    assert_eq!(code(&out), 0);
    assert_eq!(
        json_out(&out)["data"]["browser"]["source"],
        json!("given by CHROME_CDP_CHROME")
    );
}

#[test]
fn open_reports_why_chrome_failed_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(
        &dir,
        "echo '[1:1:0927/101010.1:FATAL:zygote_host_impl_linux.cc(128)] No usable sandbox! \
         If you are running on Ubuntu 23.10+ ...' >&2\nexit 1",
    );
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open"]);
    assert_eq!(code(&out), 4);
    let err = stderr(&out);
    assert!(
        err.starts_with(
            "error: Chrome exited during startup: No usable sandbox! If you are running on Ubuntu"
        ),
        "{err}"
    );
    assert!(
        err.contains("hint: the kernel blocks Chrome's sandbox"),
        "{err}"
    );

    let out = sandbox.run(&["open", "--json"]);
    let v = json_out(&out);
    assert_eq!(v["error"]["kind"], json!("LaunchFailed"));
    let lines = v["error"]["details"]["chrome_stderr"].as_array().unwrap();
    assert_eq!(lines.len(), 1);
    // Chrome's stderr is in the session's chrome.log too.
    let out = sandbox.run(&["status", "--logs"]);
    assert!(
        stdout(&out).contains("No usable sandbox!"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn open_times_out_on_a_silent_chrome() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = fake_chrome(&dir, "exec sleep 30");
    let sandbox = Sandbox::with_chrome(&chrome);
    let started = std::time::Instant::now();
    let out = sandbox.run(&["open", "--timeout", "300ms", "--json"]);
    assert_eq!(code(&out), 3);
    let v = json_out(&out);
    assert_eq!(v["error"]["kind"], json!("Timeout"));
    assert_eq!(
        v["error"]["message"],
        json!("Chrome started but did not answer within 300ms")
    );
    // The hung child is killed, not waited for.
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    sandbox.assert_all_gone();
}

#[test]
fn cdp_browser_get_version() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    // A new session gets a browser, and says so.
    let out = sandbox.run(&["cdp", "Browser.getVersion"]);
    assert_eq!(code(&out), 0);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(v["product"].as_str().unwrap().contains("Chrome"), "{v}");
    assert_eq!(v["protocolVersion"], json!("1.3"));
    assert!(
        stderr(&out).starts_with("warning: session default had no browser; started Chrome/"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run(&["cdp", "Browser.getVersion", "--json"]);
    let v = json_out(&out);
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["data"]["protocolVersion"], json!("1.3"));
    assert!(v.get("warnings").is_none(), "{v}");
}

#[test]
fn cdp_page_evaluate_from_stdin() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let mut child = sandbox
        .cli()
        .args(["cdp", "--target", "page", "Runtime.evaluate", "-", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"expression":"6*7","returnByValue":true}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "{\"result\":{\"description\":\"42\",\"type\":\"number\",\"value\":42}}\n"
    );
}

#[test]
fn cdp_page_state_lasts_between_calls() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let out = sandbox.run(&["open", "data:text/html,<title>kept</title>"]);
    assert_eq!(code(&out), 0);
    let eval = |expr: &str| {
        let params = json!({"expression": expr, "returnByValue": true}).to_string();
        let out = sandbox.run(&[
            "cdp",
            "--target",
            "page",
            "Runtime.evaluate",
            &params,
            "--json",
        ]);
        assert_eq!(code(&out), 0);
        json_out(&out)["data"]["result"]["value"].clone()
    };
    assert_eq!(eval("document.title"), json!("kept"));
    eval("window.counter = 41");
    assert_eq!(eval("window.counter + 1"), json!(42));
}

#[test]
fn cdp_protocol_errors_and_timeouts() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    assert_eq!(code(&sandbox.run(&["open", "--session", "s2"])), 0);
    // A page method sent to the browser; the hint keeps the session.
    let out = sandbox.run(&[
        "cdp",
        "Runtime.evaluate",
        r#"{"expression":"1"}"#,
        "--session",
        "s2",
    ]);
    assert_eq!(code(&out), 4);
    assert_eq!(
        stderr(&out),
        "error: Runtime.evaluate: 'Runtime.evaluate' wasn't found\n  \
         hint: if Runtime.evaluate is a page method, send it to a page: \
         chrome-cdp cdp --session s2 --target page Runtime.evaluate …\n"
    );

    // Chrome answers, but only after the timeout.
    let out = sandbox.run(&[
        "cdp",
        "--session",
        "s2",
        "--target",
        "page",
        "Runtime.evaluate",
        r#"{"expression":"new Promise(() => {})","awaitPromise":true}"#,
        "--timeout",
        "500ms",
        "--json",
    ]);
    assert_eq!(code(&out), 3);
    let err = &json_out(&out)["error"];
    assert_eq!(err["kind"], json!("Timeout"));
    assert_eq!(
        err["hint"],
        json!("retry with a longer timeout, e.g. --timeout 1s")
    );
}

#[test]
fn cdp_output_is_bounded() {
    let Some(chrome) = chrome_path() else { return };
    let sandbox = Sandbox::with_chrome(&chrome);
    let long = r#"{"expression":"'x'.repeat(100000)","returnByValue":true}"#;
    let out = sandbox.run(&["cdp", "--target", "page", "Runtime.evaluate", long]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    let last = text.lines().last().unwrap();
    assert!(
        last.starts_with("… truncated ") && last.ends_with("use --max-chars 0 for all of it"),
        "{last}"
    );
    assert!(text.chars().count() < 20_200);

    // --json stays valid JSON: the long string is shortened to fit.
    let out = sandbox.run(&[
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        long,
        "--json",
    ]);
    let v = json_out(&out);
    assert!(v["data"].to_string().chars().count() <= 20_000);
    let value = v["data"]["result"]["value"].as_str().unwrap();
    assert!(value.ends_with('…'));
    assert_eq!(v["data"]["result"]["type"], json!("string"));
    assert_eq!(
        v["truncated"],
        json!([{"path": "/result/value", "kind": "string", "len": 100_000,
                "kept": value.chars().count() - 1}])
    );
    assert!(
        v["warnings"][0]
            .as_str()
            .unwrap()
            .ends_with("chars; use --max-chars 0 for all of it")
    );

    // --max-chars 0 turns the bound off.
    let out = sandbox.run(&[
        "cdp",
        "--target",
        "page",
        "Runtime.evaluate",
        long,
        "--json",
        "--max-chars",
        "0",
    ]);
    let v = json_out(&out);
    assert_eq!(
        v["data"]["result"]["value"].as_str().unwrap().len(),
        100_000
    );
    assert!(v.get("truncated").is_none());
}

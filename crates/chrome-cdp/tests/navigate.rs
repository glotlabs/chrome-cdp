//! Navigation (plan 03, milestone 3.3) end to end: `goto`, `back`,
//! `forward` and `reload` through the daemon, `base_url` from the project
//! config, `open --ignore-https-errors`, and `--tab`.
//!
//! The real-Chrome tests need `CHROME_PATH` (see `cli.rs`).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::io::Write as _;
use std::process::Stdio;

use common::{Sandbox, chrome_path, code, json_out, run, stderr, stdout};
use fixtures::Fixtures;

#[test]
fn relative_urls_use_base_url_from_the_project_config() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let project = common::tempdir();
    std::fs::write(
        project.path().join(".chrome-cdp.toml"),
        format!("base_url = \"{}/nav/\"\n", site.b()),
    )
    .unwrap();
    let in_project = |args: &[&str]| run(sandbox.cli().args(args).current_dir(project.path()));

    let out = in_project(&["goto", "a.html"]);
    // Not relative: `a.html` is a host name, as typed.
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("cannot resolve the host a.html"));

    let out = in_project(&["goto", "./a.html", "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{}\n", site.url_b("/nav/a.html")));

    let out = in_project(&["tab", "new", "/nav/b.html", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let tab = &json_out(&out)["data"]["tab"];
    assert_eq!(tab["id"], "t2");
    assert_eq!(tab["url"], site.url_b("/nav/b.html"));

    // Outside the project, a relative URL resolves against the page.
    let out = sandbox.run(&["goto", "./a.html?from=b", "--tab", "t2", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["tab"], "t2");
    assert_eq!(data["url"], site.url_b("/nav/a.html?from=b"));
    assert_eq!(data["status"], 200);
    // Even on Chrome's error page: against the URL that failed.
    let refused = "http://127.0.0.1:47/app/";
    let out = sandbox.run(&["goto", refused, "--tab", "t2"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let out = sandbox.run(&["goto", "./login", "--tab", "t2", "--json"]);
    assert_eq!(code(&out), 1);
    assert_eq!(
        json_out(&out)["error"]["details"]["url"],
        "http://127.0.0.1:47/app/login"
    );
}

#[test]
fn history_and_reload_through_the_daemon() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    // `back` needs a running session; it does not start one.
    let out = sandbox.run(&["back"]);
    assert_eq!(code(&out), 4, "{}", stderr(&out));

    // `goto` starts one, and reads the URL from stdin with `-`.
    let mut child = sandbox
        .cli()
        .args(["goto", "-", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{}", site.url("/nav/a.html")).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{}\n", site.url("/nav/a.html")));
    assert!(
        stderr(&out).contains("had no browser; started"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run(&["goto", &site.url("/nav/b.html")]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = sandbox.run(&["back"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with(&format!("→ 200 {} \"Page A\" (", site.url("/nav/a.html"))),
        "{}",
        stdout(&out)
    );
    let out = sandbox.run(&["forward", "-q"]);
    assert_eq!(stdout(&out), format!("{}\n", site.url("/nav/b.html")));
    let out = sandbox.run(&["forward", "--json"]);
    assert_eq!(json_out(&out)["data"]["navigated"], false);
    let out = sandbox.run(&["reload", "--hard", "--wait", "domcontentloaded", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let data = &json_out(&out)["data"];
    assert_eq!(data["wait"], "domcontentloaded");
    assert_eq!(data["url"], site.url("/nav/b.html"));
}

#[test]
fn ignore_https_errors_is_a_session_option() {
    let Some(chrome) = chrome_path() else { return };
    let site = Fixtures::shared();
    let sandbox = Sandbox::with_chrome(&chrome);
    let url = format!("{}/nav/a.html", site.https());
    let out = sandbox.run(&["goto", &url, "--json"]);
    assert_eq!(code(&out), 1);
    let error = &json_out(&out)["error"];
    assert_eq!(error["kind"], "NavigationFailed");
    assert_eq!(error["details"]["reason"], "certificate");
    assert_eq!(
        error["hint"],
        "chrome-cdp open --ignore-https-errors   (for test servers; then goto again)"
    );

    // As the hint says; the running browser takes it without a restart.
    let out = sandbox.run(&["open", "--ignore-https-errors"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).contains("is already open"), "{}", stdout(&out));
    let out = sandbox.run(&["goto", &url, "-q"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{url}\n"));
}

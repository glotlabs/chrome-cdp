//! Web servers of a test run: start, reuse, readiness, early exits,
//! timeouts and teardown (plan 11, milestone 11.1).
//!
//! The server under test is this test binary itself: `serve_helper` (an
//! ignored test) serves HTTP when `E2E_SERVE_PORT` is set, so the tests
//! need no Python or netcat.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use e2e::config::WebServer;
use e2e::web_server::{Stopped, WebServers};
use protocol::ErrorKind;

/// Serves `HTTP <E2E_SERVE_STATUS>` (200 by default) on 127.0.0.1:<port>
/// after `E2E_SERVE_DELAY_MS`, and exits after `E2E_SERVE_EXIT_MS` if set.
#[test]
#[ignore = "run by the other tests as their web server"]
fn serve_helper() {
    let env = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let Some(port) = env("E2E_SERVE_PORT") else {
        return;
    };
    std::thread::sleep(Duration::from_millis(
        env("E2E_SERVE_DELAY_MS").unwrap_or(0),
    ));
    if let Some(ms) = env("E2E_SERVE_EXIT_MS") {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            println!("exiting on purpose");
            std::process::exit(7);
        });
    }
    let status = env("E2E_SERVE_STATUS").unwrap_or(200);
    let listener = TcpListener::bind(("127.0.0.1", u16::try_from(port).unwrap())).unwrap();
    println!("listening on {port}");
    serve(&listener, status);
}

fn serve(listener: &TcpListener, status: u64) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
            line.clear();
        }
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
        );
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A web server whose command runs the helper on `port`, after `before`
/// (shell commands) and with `env` for the helper.
fn helper(port: u16, before: &str, env: &[(&str, &str)]) -> WebServer {
    let exe = std::env::current_exe().unwrap();
    let mut vars: BTreeMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    vars.insert("E2E_SERVE_PORT".into(), port.to_string());
    WebServer {
        command: format!(
            "{before}exec '{}' serve_helper --exact --ignored --nocapture",
            exe.display()
        ),
        url: format!("http://127.0.0.1:{port}/"),
        reuse_existing: true,
        timeout: Duration::from_secs(20),
        env: vars,
        cwd: std::env::temp_dir(),
    }
}

/// The status line's code of a GET of `/` on `port`, or `None`.
fn get(port: u16) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    text.split(' ').nth(1)?.parse().ok()
}

/// Whether the group `pgid` is gone within 10s. It yields to the runtime,
/// whose task reaps the killed command.
async fn gone(pgid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !browser::launch::process_group_alive(pgid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[tokio::test]
async fn starts_waits_for_the_url_and_stops_the_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    // A child of the command that would outlive it without the group.
    let config = helper(port, "sleep 300 & ", &[("E2E_SERVE_DELAY_MS", "300")]);
    let mut servers = WebServers::start(&[config], dir.path()).await.unwrap();
    let [info] = servers.servers().try_into().unwrap();
    assert!(!info.reused);
    assert!(info.ready_in >= Duration::from_millis(300), "{info:?}");
    assert_eq!(get(port), Some(200));
    let log = info.log.unwrap();
    assert_eq!(log, dir.path().join("web-server.log"));
    let text = read(&log);
    assert!(text.starts_with("# chrome-cdp: sleep 300 & exec"), "{text}");
    assert!(text.contains("listening on"), "{text}");

    let pgid = info.pgid.unwrap();
    servers.stop().await;
    assert!(gone(pgid).await, "the group outlived stop");
    assert_eq!(get(port), None);
}

#[tokio::test]
async fn reuses_a_server_that_already_answers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || serve(&listener, 404));
    let dir = tempfile::tempdir().unwrap();
    let mut config = helper(port, "exit 1; ", &[]);
    let servers = WebServers::start(std::slice::from_ref(&config), dir.path())
        .await
        .unwrap();
    let [info] = servers.servers().try_into().unwrap();
    assert!(info.reused && info.log.is_none() && info.pgid.is_none());
    assert!(!dir.path().join("web-server.log").exists());

    config.reuse_existing = false;
    let e = WebServers::start(&[config], dir.path()).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::LaunchFailed);
    assert_eq!(
        e.message,
        format!("http://127.0.0.1:{port}/ already answers (HTTP 404), and reuse_existing is false")
    );
    assert!(e.hint.unwrap().contains("set reuse_existing = true"));
}

#[tokio::test]
async fn a_command_that_exits_early_fails_with_its_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = helper(free_port(), "", &[]);
    config.command = "echo 'Error: Cannot find module vite'; exit 3".into();
    let started = Instant::now();
    let e = WebServers::start(&[config], dir.path()).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(e.kind, ErrorKind::LaunchFailed);
    assert!(
        e.message.starts_with("web server `echo 'Error: Cannot find module vite'; exit 3` exited (status 3) before http://127.0.0.1:"),
        "{}",
        e.message
    );
    let text = e.to_text();
    assert!(text.contains("Error: Cannot find module vite"), "{text}");
    assert!(text.contains("web-server.log"), "{text}");
    assert!(
        e.hint
            .unwrap()
            .starts_with("run it by hand to see why: cd ")
    );
}

#[tokio::test]
async fn times_out_with_the_last_answer_and_kills_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut config = helper(port, "", &[("E2E_SERVE_STATUS", "503")]);
    config.timeout = Duration::from_millis(1500);
    let e = WebServers::start(&[config], dir.path()).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Timeout);
    assert!(e.message.ends_with("within 1.5s"), "{}", e.message);
    let last = e.notes.iter().find(|n| n.label == "last try").unwrap();
    assert_eq!(last.text, "HTTP 503");
    // The server was stopped with the failed start.
    let deadline = Instant::now() + Duration::from_secs(10);
    while get(port).is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(get(port), None);

    let mut silent = helper(free_port(), "", &[]);
    silent.command = "exec sleep 30".into();
    silent.timeout = Duration::from_millis(300);
    let e = WebServers::start(&[silent], dir.path()).await.unwrap_err();
    let last = e.notes.iter().find(|n| n.label == "last try").unwrap();
    assert_eq!(last.text, "connection refused");
}

#[tokio::test]
async fn several_servers_get_a_log_each_and_their_env() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (free_port(), free_port());
    let first = helper(
        a,
        "echo \"greeting=$GREETING browser=$BROWSER\"; ",
        &[("GREETING", "hi")],
    );
    let second = helper(b, "echo \"browser=$BROWSER\"; ", &[("BROWSER", "firefox")]);
    let mut servers = WebServers::start(&[first, second], dir.path())
        .await
        .unwrap();
    assert_eq!((get(a), get(b)), (Some(200), Some(200)));
    let logs: Vec<String> = servers
        .servers()
        .iter()
        .map(|s| read(s.log.as_deref().unwrap()))
        .collect();
    assert!(logs[0].contains("greeting=hi browser=none"), "{}", logs[0]);
    assert!(logs[1].contains("browser=firefox"), "{}", logs[1]);
    assert!(dir.path().join("web-server-2.log").is_file());
    servers.stop().await;
    assert_eq!((get(a), get(b)), (None, None));
}

#[tokio::test]
async fn a_server_that_exits_during_the_run_stops_the_work() {
    let dir = tempfile::tempdir().unwrap();
    let config = helper(free_port(), "", &[("E2E_SERVE_EXIT_MS", "500")]);
    let mut servers = WebServers::start(&[config], dir.path()).await.unwrap();
    let stopped = servers
        .run(std::future::pending::<()>(), std::future::pending())
        .await
        .unwrap_err();
    let Stopped::Exited(e) = stopped else {
        panic!("{stopped:?}")
    };
    assert!(
        e.message.ends_with("exited (status 7) while the tests ran"),
        "{}",
        e.message
    );
    assert!(e.to_text().contains("exiting on purpose"));
}

#[tokio::test]
async fn work_ends_the_run_and_an_interrupt_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut servers = WebServers::start(&[helper(port, "", &[])], dir.path())
        .await
        .unwrap();
    let done = servers
        .run(async { get(port) }, std::future::pending())
        .await
        .unwrap();
    assert_eq!(done, Some(200));
    let interrupted = servers
        .run(std::future::pending::<()>(), async {})
        .await
        .unwrap_err();
    assert!(matches!(interrupted, Stopped::Interrupted));
    servers.stop().await;
    // A stopped server's exit is not news.
    let quiet = tokio::time::timeout(Duration::from_millis(200), servers.exited()).await;
    assert!(quiet.is_err());
}

#[tokio::test]
async fn dropping_the_servers_kills_them() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let servers = WebServers::start(&[helper(port, "", &[])], dir.path())
        .await
        .unwrap();
    let pgid = servers.servers()[0].pgid.unwrap();
    drop(servers);
    assert!(gone(pgid).await);
}

/// Starts a web server on `E2E_HOLD_PORT`, prints its group, and waits
/// to be killed: a chrome-cdp that dies without stopping it.
#[test]
#[ignore = "run by a_killed_chrome_cdp_leaves_no_server"]
fn hold_helper() {
    let Ok(port) = std::env::var("E2E_HOLD_PORT") else {
        return;
    };
    let dir = std::env::var("E2E_HOLD_DIR").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let config = helper(port.parse().unwrap(), "", &[]);
        let servers = WebServers::start(&[config], Path::new(&dir)).await.unwrap();
        println!("pgid {}", servers.servers()[0].pgid.unwrap());
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn a_killed_chrome_cdp_leaves_no_server() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["hold_helper", "--exact", "--ignored", "--nocapture"])
        .env("E2E_HOLD_PORT", port.to_string())
        .env("E2E_HOLD_DIR", dir.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = BufReader::new(holder.stdout.take().unwrap());
    let pgid: u32 = out
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line.strip_prefix("pgid ").map(|p| p.parse().unwrap()))
        .expect("the holder printed the server's group");
    assert_eq!(get(port), Some(200));
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(gone(pgid).await, "the server outlived chrome-cdp");
}

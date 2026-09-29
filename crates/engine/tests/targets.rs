//! The target matrix (plan 13) against a real Chrome (`CHROME_PATH`;
//! skipped without it unless `CHROME_CDP_REQUIRE_CHROME` is set).
//!
//! `targets/matrix.html` has every kind of target: the main frame, a
//! same-site iframe, a cross-site one (an OOPIF), and a dedicated, a
//! shared and a service worker. [`ROWS`] is the table of features by
//! target kind: each cell is checked, not applicable (with why), possible
//! only in part in Chrome (with why; checked, a miss noted), or still to
//! do (with the milestone that does it). A feature that reaches a new
//! kind of target turns its `Todo` into `Check`; plan 13 is done when no
//! `Todo` is left.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    CdpRequest, ConsoleRequest, EmulateRequest, EmulationChange, Engine, EvalRequest, EvalWorld,
    ExpectRequest, Expectation, HttpCredentials, InitScriptAddRequest, MockLogRequest,
    MockOffRequest, MockUseRequest, NavigateRequest, Navigation, NetworkRequest, OpenRequest,
    RequestCheck, RouteAction, RouteBody, RouteRequest, RouteSpec, Since, Target, UnrouteRequest,
    WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{ConsoleData, EvalData, Level, MockLogData, NetworkData, WorkerKind};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;

/// The kinds of target: a column's title, and the matrix page's name for
/// its target.
const COLUMNS: [(&str, &str); 6] = [
    ("main frame", "main"),
    ("same-site iframe", "same"),
    ("OOPIF", "oopif"),
    ("dedicated worker", "dedicated"),
    ("shared worker", "shared"),
    ("service worker", "service"),
];

/// What a row says about one kind of target.
#[derive(Debug, Clone, Copy)]
enum Cell {
    /// The feature works there, and the row's probe checks it.
    Check,
    /// The feature means nothing there; why.
    Na(&'static str),
    /// It should work there but does not yet; the milestone that does it.
    Todo(&'static str),
    /// Chrome lets it work there only in part; why. The probe runs, and
    /// a miss is a note, not a failure.
    Partly(&'static str),
}

use Cell::{Check, Na, Partly, Todo};

/// How a row checks a target.
#[derive(Debug, Clone, Copy)]
enum Probe {
    /// It was resumed and answers a message.
    Answers,
    /// `eval --frame` runs in its document.
    Eval,
    /// Its `console.log` is in the tab's console.
    Console,
    /// Its uncaught exception is in the tab's errors.
    Errors,
    /// Its `fetch` is in the tab's network log.
    Network,
    /// A `route` answers its `fetch`.
    Route,
    /// A mock set (`mock use`) answers its `fetch`.
    Mock,
    /// `expect request` finds its `fetch`.
    ExpectRequest,
    /// Its request that no mock of a set's scope answers is unmatched
    /// (`mock log --unmatched`, as the test runner checks).
    Unmatched,
    /// A `[[websocket]]` mock answers its `WebSocket`.
    MockWebSocket,
    /// `open --http-credentials` answers the challenge of its `fetch`.
    HttpAuth,
    /// An `init-script --now` ran in it.
    InitScript,
    /// `ua` reaches its `navigator.userAgent`.
    UserAgent,
}

struct Row {
    feature: &'static str,
    probe: Probe,
    cells: [Cell; 6],
}

const NO_DOCUMENT: &str = "a worker has no document";

const SHARED_FETCH: &str = "Chrome 154 takes Fetch.enable in a shared worker only before it \
     starts, so its requests are intercepted only if that call wins a race with its start";

/// Features by target kind (see [`COLUMNS`]).
const ROWS: &[Row] = &[
    Row {
        feature: "resumed, answers",
        probe: Probe::Answers,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "eval --frame",
        probe: Probe::Eval,
        cells: [
            Check,
            Check,
            Check,
            Na(NO_DOCUMENT),
            Na(NO_DOCUMENT),
            Na(NO_DOCUMENT),
        ],
    },
    Row {
        feature: "console",
        probe: Probe::Console,
        cells: [Check, Check, Check, Check, Todo("13.3"), Todo("13.3")],
    },
    Row {
        feature: "errors",
        probe: Probe::Errors,
        cells: [Check, Check, Check, Check, Todo("13.3"), Todo("13.3")],
    },
    Row {
        feature: "network",
        probe: Probe::Network,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "route",
        probe: Probe::Route,
        cells: [Check, Check, Check, Check, Partly(SHARED_FETCH), Check],
    },
    Row {
        feature: "mock use (HTTP)",
        probe: Probe::Mock,
        cells: [Check, Check, Check, Check, Partly(SHARED_FETCH), Check],
    },
    Row {
        feature: "expect request",
        probe: Probe::ExpectRequest,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "unmatched check",
        probe: Probe::Unmatched,
        cells: [Check, Check, Check, Check, Partly(SHARED_FETCH), Check],
    },
    Row {
        feature: "mocked WebSocket",
        probe: Probe::MockWebSocket,
        cells: [
            Check,
            Check,
            Check,
            Todo("13.2"),
            Todo("13.2"),
            Todo("13.2"),
        ],
    },
    Row {
        feature: "init-script",
        probe: Probe::InitScript,
        cells: [
            Check,
            Check,
            Check,
            Todo("13.3"),
            Todo("13.3"),
            Todo("13.3"),
        ],
    },
    // Last: the credentials stay.
    Row {
        feature: "HTTP auth",
        probe: Probe::HttpAuth,
        cells: [Check, Check, Check, Check, Partly(SHARED_FETCH), Check],
    },
    Row {
        feature: "ua",
        probe: Probe::UserAgent,
        cells: [
            Check,
            Check,
            Check,
            Todo("13.3"),
            Todo("13.3"),
            Todo("13.3"),
        ],
    },
];

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

/// A session on the matrix page, once every target of it has answered.
async fn matrix(path: &Path, name: &str) -> Arc<Engine> {
    let site = Fixtures::shared();
    let engine = Engine::new(name, None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    let url = site.url("/targets/matrix.html");
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.clone(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
    let title = eventually("every target of the matrix to answer", async || {
        let title = eval(&engine, "document.title").await;
        (title == json!("Targets: ready")).then_some(title)
    })
    .await;
    assert_eq!(title, json!("Targets: ready"));
    engine
}

async fn eval(engine: &Engine, script: &str) -> Value {
    eval_in(engine, None, script).await
}

/// `script`'s value in frame `frame` (the main frame with `None`).
async fn eval_in(engine: &Engine, frame: Option<&str>, script: &str) -> Value {
    let out = engine
        .eval(EvalRequest {
            script: script.to_owned(),
            locator: None,
            frame: frame.map(str::to_owned),
            world: EvalWorld::Main,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    let data: EvalData = serde_json::from_value(out.data).unwrap();
    data.value
}

/// Has target `who` of the matrix run `op`; its answer.
async fn ask(engine: &Engine, who: &str, op: &str) -> Value {
    eval(engine, &format!("ask({who:?}, {op:?})")).await
}

/// Polls `check` until it returns a value, for up to 10 s.
async fn eventually<T>(what: &str, mut check: impl AsyncFnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Polls `check` for up to 5 s; whether it held.
async fn within(mut check: impl AsyncFnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn console(engine: &Engine, errors: bool) -> ConsoleData {
    let out = engine
        .console(ConsoleRequest {
            errors,
            tab: None,
            level: Level::Debug,
            since: Since::Seq { seq: 0 },
            grep: None,
            limit: 0,
            wait_ms: 0,
            sourcemaps: false,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("console: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

async fn network(engine: &Engine) -> NetworkData {
    let out = engine
        .network(NetworkRequest {
            tab: None,
            failed: false,
            types: Vec::new(),
            url: None,
            method: None,
            status: Vec::new(),
            since: Since::Seq { seq: 0 },
            limit: 0,
            wait_ms: 0,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("network: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

/// `f`'s value, run with the matrix's mock set in use.
async fn with_mocks<T>(engine: &Engine, f: impl AsyncFnOnce() -> T) -> T {
    let dir = fixtures::site_dir().join("targets/mocks");
    engine
        .mock_use(MockUseRequest {
            sets: vec![mock::Source {
                name: "matrix".into(),
                file: dir.join("matrix.toml"),
                base: dir.clone(),
            }],
            add: false,
            seed: 0,
            test: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("mock use: {}", e.to_text()));
    let value = f().await;
    engine
        .mock_off(MockOffRequest {
            sets: Vec::new(),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("mock off: {}", e.to_text()));
    value
}

/// The kind of worker target `who` of the matrix is, if it is one.
fn worker_kind(who: &str) -> Option<WorkerKind> {
    match who {
        "dedicated" => Some(WorkerKind::Dedicated),
        "shared" => Some(WorkerKind::Shared),
        "service" => Some(WorkerKind::Service),
        _ => None,
    }
}

/// Runs `probe` on target `who`; what went wrong, if anything.
async fn probe(engine: &Arc<Engine>, probe: Probe, who: &str) -> Result<(), String> {
    match probe {
        Probe::Answers => {
            let answer = ask(engine, who, "ping").await;
            (answer == json!("pong"))
                .then_some(())
                .ok_or_else(|| format!("answered {answer}"))
        }
        Probe::Eval => {
            let frame = (who != "main").then_some(who);
            let at = eval_in(engine, frame, "location.pathname + location.search").await;
            let want = match frame {
                None => "/targets/matrix.html".to_owned(),
                Some(_) => format!("/targets/frame.html?who={who}"),
            };
            at.as_str()
                .is_some_and(|at| at.starts_with(&want))
                .then_some(())
                .ok_or_else(|| format!("ran in {at}"))
        }
        Probe::Console => {
            ask(engine, who, "log").await;
            let text = format!("log from {who}");
            let seen = within(async || {
                let data = console(engine, false).await;
                data.entries.iter().any(|e| e.text == text)
            })
            .await;
            seen.then_some(())
                .ok_or_else(|| format!("no `{text}` in the console"))
        }
        Probe::Errors => {
            ask(engine, who, "throw").await;
            let text = format!("thrown in {who}");
            let seen = within(async || {
                let data = console(engine, true).await;
                data.entries.iter().any(|e| e.text.contains(&text))
            })
            .await;
            seen.then_some(())
                .ok_or_else(|| format!("no `{text}` in the errors"))
        }
        Probe::Network => {
            let answer = ask(engine, who, "fetch").await;
            if answer != json!(who) {
                return Err(format!("fetch answered {answer}"));
            }
            let url = format!("/targets/api?from={who}");
            let mut entry = None;
            within(async || {
                let data = network(engine).await;
                entry = data.requests.into_iter().find(|r| r.url.ends_with(&url));
                entry.is_some()
            })
            .await;
            let entry = entry.ok_or_else(|| format!("no {url} in the network log"))?;
            // A worker's request is marked with it.
            let kind = entry.worker.map(|w| w.kind);
            (kind == worker_kind(who))
                .then_some(())
                .ok_or_else(|| format!("{url} is marked {kind:?}"))
        }
        Probe::Route => {
            engine
                .route(RouteRequest {
                    rules: vec![RouteSpec {
                        pattern: "/targets/api?from=".into(),
                        label: None,
                        methods: Vec::new(),
                        resource_types: Vec::new(),
                        times: None,
                        delay_ms: 0,
                        context: None,
                        action: RouteAction::Fulfill {
                            status: 200,
                            headers: Vec::new(),
                            body: RouteBody::Text {
                                text: r#"{"from":"routed"}"#.into(),
                            },
                        },
                    }],
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .unwrap_or_else(|e| panic!("route: {}", e.to_text()));
            let answer = ask(engine, who, "fetch").await;
            engine
                .unroute(UnrouteRequest {
                    ids: Vec::new(),
                    all: true,
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .unwrap_or_else(|e| panic!("unroute: {}", e.to_text()));
            (answer == json!("routed"))
                .then_some(())
                .ok_or_else(|| format!("fetch answered {answer}"))
        }
        Probe::Mock => {
            let answer = with_mocks(engine, async || ask(engine, who, "fetch").await).await;
            (answer == json!(format!("mocked {who}")))
                .then_some(())
                .ok_or_else(|| format!("fetch answered {answer}"))
        }
        Probe::ExpectRequest => {
            ask(engine, who, "fetch").await;
            let check = RequestCheck {
                method: "GET".into(),
                target: "/targets/api".into(),
                query: vec![("from".into(), who.into())],
                headers: Vec::new(),
                json: None,
                count: None,
                min: None,
                max: None,
                since: None,
                none: false,
            };
            Box::pin(engine.expect(ExpectRequest {
                expectation: Expectation::Request(check),
                not: false,
                tab: None,
                strict_refs: false,
                testid_attribute: None,
                timeout_ms: 5_000,
                max_chars: 0,
            }))
            .await
            .map(drop)
            .map_err(|e| e.to_text())
        }
        Probe::Unmatched => {
            let answer = with_mocks(engine, async || {
                let answer = ask(engine, who, "unmatched").await;
                let log = engine
                    .mock_log(&MockLogRequest {
                        unmatched: true,
                        all: false,
                        limit: 0,
                        max_chars: 0,
                    })
                    .unwrap();
                let log: MockLogData = serde_json::from_value(log.data).unwrap();
                let url = format!("/targets/unmatched?from={who}");
                let seen = log.log.iter().any(|r| r.url.ends_with(&url));
                (answer, seen)
            })
            .await;
            match answer {
                (_, true) => Ok(()),
                (answer, false) => Err(format!("not unmatched (answered {answer})")),
            }
        }
        Probe::MockWebSocket => {
            let answer = with_mocks(engine, async || ask(engine, who, "ws").await).await;
            (answer == json!("pong"))
                .then_some(())
                .ok_or_else(|| format!("the socket answered {answer}"))
        }
        Probe::HttpAuth => {
            engine
                .open(OpenRequest {
                    http_credentials: Some(HttpCredentials {
                        username: fixtures::AUTH_USER.into(),
                        password: fixtures::AUTH_PASSWORD.into(),
                        origin: None,
                    }),
                    timeout_ms: TIMEOUT_MS,
                    ..OpenRequest::default()
                })
                .await
                .unwrap_or_else(|e| panic!("open --http-credentials: {}", e.to_text()));
            let answer = ask(engine, who, "auth").await;
            (answer == json!(200))
                .then_some(())
                .ok_or_else(|| format!("fetch answered {answer}"))
        }
        Probe::InitScript => {
            engine
                .init_script_add(InitScriptAddRequest {
                    source: "self.matrixInit = 'ran';".to_owned(),
                    file: None,
                    world: EvalWorld::Main,
                    now: true,
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .unwrap_or_else(|e| panic!("init-script: {}", e.to_text()));
            let answer = ask(engine, who, "init").await;
            (answer == json!("ran"))
                .then_some(())
                .ok_or_else(|| format!("init answered {answer}"))
        }
        Probe::UserAgent => {
            const AGENT: &str = "chrome-cdp-matrix/1.0";
            engine
                .emulate(EmulateRequest {
                    change: EmulationChange::UserAgent {
                        user_agent: Some(AGENT.to_owned()),
                    },
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .unwrap_or_else(|e| panic!("ua: {}", e.to_text()));
            let report = ask(engine, who, "report").await;
            (report["userAgent"] == json!(AGENT))
                .then_some(())
                .ok_or_else(|| format!("userAgent {}", report["userAgent"]))
        }
    }
}

/// Every row's checked cells hold. The table goes to stderr, with why
/// cells are n/a and how many are still to do.
#[tokio::test]
async fn every_feature_in_every_target() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-table").await;
    let mut failed = Vec::new();
    let mut todo = 0;
    let mut reasons: Vec<String> = Vec::new();
    let mut table = format!("{:<18}", "");
    for (title, _) in COLUMNS {
        write!(table, " {title:<17}").unwrap();
    }
    for row in ROWS {
        write!(table, "\n{:<18}", row.feature).unwrap();
        for (cell, (title, who)) in row.cells.iter().zip(COLUMNS) {
            let text = match cell {
                Check => match probe(&engine, row.probe, who).await {
                    Ok(()) => "ok".to_owned(),
                    Err(e) => {
                        failed.push(format!("{} in the {title}: {e}", row.feature));
                        "FAILED".to_owned()
                    }
                },
                Na(why) => {
                    let reason = format!("n/a in {}: {why}", row.feature);
                    if !reasons.contains(&reason) {
                        reasons.push(reason);
                    }
                    "n/a".to_owned()
                }
                Partly(why) => {
                    let reason = format!("partly in the {title}: {why}");
                    if !reasons.contains(&reason) {
                        reasons.push(reason);
                    }
                    match probe(&engine, row.probe, who).await {
                        Ok(()) => "ok (partly)".to_owned(),
                        Err(e) => {
                            reasons.push(format!("missed {} in the {title}: {e}", row.feature));
                            "missed (partly)".to_owned()
                        }
                    }
                }
                Todo(milestone) => {
                    todo += 1;
                    format!("todo ({milestone})")
                }
            };
            write!(table, " {text:<17}").unwrap();
        }
    }
    eprintln!("{table}\n{}\n{todo} cells to do", reasons.join("\n"));
    engine.close().await;
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The matrix page loads with every target attached (Chrome says so)
/// and resumed (each answered): the OOPIF, both dedicated workers, both
/// shared workers and the service worker. None can hang the page.
#[tokio::test]
async fn every_target_attaches_and_resumes() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-attach").await;
    for who in ["oopif-dedicated", "oopif-shared"] {
        assert_eq!(ask(&engine, who, "ping").await, json!("pong"), "{who}");
    }
    let out = engine
        .cdp(CdpRequest {
            method: Some("Target.getTargets".into()),
            params: serde_json::Map::new(),
            target: Target::Browser,
            tab: None,
            frame: None,
            listen: Vec::new(),
            count: 0,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let infos = out.data["targetInfos"].as_array().unwrap();
    let mut kinds: Vec<(String, String)> = infos
        .iter()
        .filter(|t| t["url"].as_str().unwrap_or_default().contains("/targets/"))
        .map(|t| {
            assert_eq!(t["attached"], json!(true), "{t}");
            let url = t["url"].as_str().unwrap();
            let file = url.rsplit('/').next().unwrap();
            (t["type"].as_str().unwrap().to_owned(), file.to_owned())
        })
        .collect();
    kinds.sort();
    let want = [
        ("iframe", "frame.html?who=oopif&workers=1"),
        ("page", "matrix.html"),
        ("service_worker", "sw.js"),
        ("shared_worker", "shared.js?who=oopif-shared"),
        ("shared_worker", "shared.js?who=shared"),
        ("worker", "dedicated.js?who=dedicated"),
        ("worker", "dedicated.js?who=oopif-dedicated"),
    ];
    let want: Vec<(String, String)> = want
        .iter()
        .map(|(k, f)| ((*k).to_owned(), (*f).to_owned()))
        .collect();
    assert_eq!(kinds, want);
    engine.close().await;
}

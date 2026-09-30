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
use engine::{A11yLevel, Read, ReadRequest, WaitFor, WaitRequest};
use engine::{
    CdpRequest, ConsoleRequest, CoverageStartRequest, CoverageStopRequest, EmulateRequest,
    EmulationChange, Engine, EvalRequest, EvalWorld, ExpectRequest, Expectation, HttpCredentials,
    InitScriptAddRequest, InitScriptRemoveRequest, MockLogRequest, MockOffRequest, MockUseRequest,
    NavigateRequest, Navigation, NetworkRequest, OpenRequest, RequestCheck, RouteAction, RouteBody,
    RouteRequest, RouteSpec, Since, TabNewRequest, Target, UnrouteRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{
    ConsoleData, CoverageStopData, EvalData, Level, MockLogData, NetworkData, WorkerKind,
};
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
    #[allow(dead_code, reason = "no cell is left to do; a new row may have some")]
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
    /// A mock's `sse` answer streams to its `EventSource`.
    MockEventSource,
    /// A `[[websocket]]` mock answers its binary message with one (an
    /// `ArrayBuffer`, and a `Blob`).
    MockWebSocketBinary,
    /// `open --http-credentials` answers the challenge of its `fetch`.
    HttpAuth,
    /// An `init-script --now` ran in it.
    InitScript,
    /// `ua` reaches its `navigator.userAgent` and its requests.
    UserAgent,
    /// `locale` reaches its `navigator.language` and its requests'
    /// `Accept-Language`.
    Locale,
    /// `tz` reaches its time zone.
    Timezone,
    /// `time freeze` stops its `Date.now()`.
    TimeFreeze,
    /// `time advance` moves its frozen `Date.now()` on.
    TimeAdvance,
    /// `time pause` stops its timers, and `time run-for` fires them when
    /// due.
    TimeRunFor,
    /// The test runner's determinism script seeds its `Math.random`.
    Determinism,
    /// `offline` fails its `fetch`.
    Offline,
    /// `cdp --listen --all-sessions` hears its `fetch`, named after it.
    Listen,
    /// `coverage` counts the script its `fetch` ran, marked with where it
    /// ran.
    Coverage,
    /// `text` without a locator shows its document's text.
    Text,
    /// `wait --text` without `--in` sees text that appears in it.
    WaitText,
    /// `expect --a11y` finds a violation in it, marked with its frame.
    A11y,
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
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "errors",
        probe: Probe::Errors,
        cells: [Check, Check, Check, Check, Check, Check],
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
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "mocked EventSource",
        probe: Probe::MockEventSource,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "binary WebSocket",
        probe: Probe::MockWebSocketBinary,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "init-script",
        probe: Probe::InitScript,
        cells: [Check, Check, Check, Check, Check, Check],
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
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "locale",
        probe: Probe::Locale,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "tz",
        probe: Probe::Timezone,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "time freeze",
        probe: Probe::TimeFreeze,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "time advance",
        probe: Probe::TimeAdvance,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "time run-for",
        probe: Probe::TimeRunFor,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "offline",
        probe: Probe::Offline,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "determinism",
        probe: Probe::Determinism,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "cdp --all-sessions",
        probe: Probe::Listen,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "coverage",
        probe: Probe::Coverage,
        cells: [Check, Check, Check, Check, Check, Check],
    },
    Row {
        feature: "text",
        probe: Probe::Text,
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
        feature: "wait --text",
        probe: Probe::WaitText,
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
        feature: "expect --a11y",
        probe: Probe::A11y,
        cells: [
            Check,
            Check,
            Check,
            Na(NO_DOCUMENT),
            Na(NO_DOCUMENT),
            Na(NO_DOCUMENT),
        ],
    },
];

/// The test runner's determinism script (plan 11.6).
const DETERMINISM: &str = include_str!("../../chrome-cdp/src/test/determinism.js");

/// When the clock rows freeze it: 2030-01-01T00:00:00Z.
const FROZEN_MS: i64 = 1_893_456_000_000;

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
            locals: false,
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
            let data = console(engine, false).await;
            let Some(entry) = data.entries.iter().find(|e| e.text == text) else {
                return Err(format!("no `{text}` in the console"));
            };
            // A worker's entry is marked with it.
            (seen && entry.worker_kind == worker_kind(who))
                .then_some(())
                .ok_or_else(|| format!("`{text}` is marked {:?}", entry.worker_kind))
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
        Probe::MockEventSource => {
            let answer = with_mocks(engine, async || ask(engine, who, "sse").await).await;
            (answer == json!("streamed"))
                .then_some(())
                .ok_or_else(|| format!("the EventSource got {answer}"))
        }
        Probe::MockWebSocketBinary => {
            let answers = with_mocks(engine, async || {
                (
                    ask(engine, who, "ws-binary").await,
                    ask(engine, who, "ws-blob").await,
                )
            })
            .await;
            (answers == (json!("4,5,6"), json!("4,5,6")))
                .then_some(())
                .ok_or_else(|| format!("the socket answered {answers:?}"))
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
            emulate(
                engine,
                EmulationChange::UserAgent {
                    user_agent: Some(AGENT.to_owned()),
                },
            )
            .await;
            let report = ask(engine, who, "report").await;
            let headers = ask(engine, who, "headers").await;
            (report["userAgent"] == json!(AGENT) && headers["userAgent"] == json!(AGENT))
                .then_some(())
                .ok_or_else(|| {
                    format!(
                        "userAgent {}, its requests' {}",
                        report["userAgent"], headers["userAgent"]
                    )
                })
        }
        Probe::Locale => {
            emulate(
                engine,
                EmulationChange::Locale {
                    locale: Some("de-DE".to_owned()),
                },
            )
            .await;
            let report = ask(engine, who, "report").await;
            let headers = ask(engine, who, "headers").await;
            (report["language"] == json!("de-DE") && headers["language"] == json!("de-DE"))
                .then_some(())
                .ok_or_else(|| {
                    format!(
                        "language {}, its requests' {}",
                        report["language"], headers["language"]
                    )
                })
        }
        Probe::Timezone => {
            emulate(
                engine,
                EmulationChange::Timezone {
                    timezone: Some("Asia/Tokyo".to_owned()),
                },
            )
            .await;
            let report = ask(engine, who, "report").await;
            (report["timeZone"] == json!("Asia/Tokyo"))
                .then_some(())
                .ok_or_else(|| format!("time zone {}", report["timeZone"]))
        }
        Probe::TimeFreeze => {
            emulate(
                engine,
                EmulationChange::TimeFreeze {
                    epoch_ms: FROZEN_MS,
                },
            )
            .await;
            let report = ask(engine, who, "report").await;
            (report["now"] == json!(FROZEN_MS))
                .then_some(())
                .ok_or_else(|| format!("Date.now() {}", report["now"]))
        }
        Probe::TimeAdvance => {
            emulate(
                engine,
                EmulationChange::TimeFreeze {
                    epoch_ms: FROZEN_MS,
                },
            )
            .await;
            emulate(engine, EmulationChange::TimeAdvance { ms: 60_000 }).await;
            let report = ask(engine, who, "report").await;
            (report["now"] == json!(FROZEN_MS + 60_000))
                .then_some(())
                .ok_or_else(|| format!("Date.now() {}", report["now"]))
        }
        Probe::TimeRunFor => {
            emulate(engine, EmulationChange::TimePause { epoch_ms: None }).await;
            let set = ask(engine, who, "timer-start").await;
            emulate(engine, EmulationChange::TimeRunFor { ms: 299_999 }).await;
            let early = ask(engine, who, "timer-fired").await;
            emulate(engine, EmulationChange::TimeRunFor { ms: 1 }).await;
            let fired = ask(engine, who, "timer-fired").await;
            emulate(engine, EmulationChange::TimeResume).await;
            (set == json!("set") && early == json!(false) && fired == json!(true))
                .then_some(())
                .ok_or_else(|| format!("timer {set}, early {early}, then {fired}"))
        }
        Probe::Listen => {
            // The page asks the target to fetch, as the listened call.
            let expression = format!("ask({who:?}, 'fetch')");
            let out = engine
                .cdp(CdpRequest {
                    method: Some("Runtime.evaluate".into()),
                    params: json!({ "expression": expression, "awaitPromise": true })
                        .as_object()
                        .unwrap()
                        .clone(),
                    target: Target::Page,
                    tab: None,
                    frame: None,
                    listen: vec!["Network.requestWillBeSent".into()],
                    count: 0,
                    all_sessions: true,
                    timeout_ms: 2_000,
                    max_chars: 0,
                })
                .await
                .map_err(|e| e.to_text())?;
            let data: protocol::schema::CdpListenData = serde_json::from_value(out.data).unwrap();
            let asked = format!("/targets/api?from={who}");
            let heard: Vec<&str> = data
                .events
                .iter()
                .filter(|e| {
                    e.params["request"]["url"]
                        .as_str()
                        .is_some_and(|u| u.ends_with(&asked))
                })
                .filter_map(|e| e.from.as_deref())
                .collect();
            // A same-site iframe is in the page's process and session.
            let want = match who {
                "main" | "same" => "main".to_owned(),
                "oopif" => "iframe \"oopif\"".to_owned(),
                worker => format!("{worker} worker"),
            };
            heard
                .iter()
                .any(|from| from.starts_with(&want))
                .then_some(())
                .ok_or_else(|| format!("heard its request from {heard:?}, not {want}"))
        }
        Probe::Coverage => {
            engine
                .coverage_start(CoverageStartRequest {
                    tab: None,
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .map_err(|e| e.to_text())?;
            ask(engine, who, "fetch").await;
            let out = engine
                .coverage_stop(CoverageStopRequest {
                    limit: 0,
                    timeout_ms: TIMEOUT_MS,
                    max_chars: 0,
                })
                .await
                .map_err(|e| e.to_text())?;
            let data: CoverageStopData = serde_json::from_value(out.data).unwrap();
            // Its `targetOps` ran in the ops.js of its own process.
            let site = Fixtures::shared();
            let origin = if who == "oopif" { site.b() } else { site.a() };
            let ops = format!("{origin}/targets/ops.js");
            let file = data.files.iter().find(|f| {
                f.kind == "js"
                    && f.url == ops
                    && match worker_kind(who) {
                        // The service worker's URL has no `who`.
                        Some(kind) => f.worker.as_ref().is_some_and(|w| {
                            w.kind == kind
                                && (kind == WorkerKind::Service
                                    || w.url.contains(&format!("who={who}")))
                        }),
                        None => f.worker.is_none() && f.frame.is_some() == (who == "oopif"),
                    }
            });
            match file {
                Some(file) if file.unused < file.total => Ok(()),
                Some(file) => Err(format!("{ops} did not run: {file:?}")),
                None => Err(format!("no {ops} of it in {:?}", data.files)),
            }
        }
        Probe::Text => {
            let frame = (who != "main").then_some(who);
            let marker = format!("Text shown in {who}");
            eval_in(
                engine,
                frame,
                &format!("document.body.append(Object.assign(document.createElement('p'), {{ textContent: {marker:?} }})); 1"),
            )
            .await;
            let out = Box::pin(engine.read(ReadRequest {
                read: Read::Text,
                locator: None,
                tab: None,
                strict_refs: false,
                testid_attribute: None,
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            }))
            .await
            .map_err(|e| e.to_text())?;
            out.text
                .contains(&marker)
                .then_some(())
                .ok_or_else(|| format!("no `{marker}` in {}", out.text))
        }
        Probe::WaitText => {
            let frame = (who != "main").then_some(who);
            let marker = format!("Text that came later in {who}");
            eval_in(
                engine,
                frame,
                &format!("setTimeout(() => document.body.append(Object.assign(document.createElement('p'), {{ textContent: {marker:?} }})), 300); 1"),
            )
            .await;
            Box::pin(engine.wait(WaitRequest {
                condition: WaitFor::Text {
                    text: marker,
                    within: None,
                    gone: false,
                },
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
        Probe::A11y => {
            let frame = (who != "main").then_some(who);
            let a11y = async || {
                let err = Box::pin(engine.expect(ExpectRequest {
                    expectation: Expectation::A11y {
                        scope: None,
                        level: A11yLevel::A,
                    },
                    not: false,
                    tab: None,
                    strict_refs: false,
                    testid_attribute: None,
                    timeout_ms: 5_000,
                    max_chars: 0,
                }))
                .await
                .err();
                // The unnamed buttons, and whether each is in an iframe.
                let details = err.and_then(|e| e.details).unwrap_or_default();
                details["violations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|v| v["rule"] == "button-name")
                    .flat_map(|v| v["elements"].as_array().cloned().unwrap_or_default())
                    .map(|e| e["frame"].is_string())
                    .collect::<Vec<bool>>()
            };
            let before = a11y().await;
            eval_in(
                engine,
                frame,
                "document.body.append(Object.assign(document.createElement('button'), { id: 'a11y-probe' })); 1",
            )
            .await;
            let after = a11y().await;
            eval_in(
                engine,
                frame,
                "document.getElementById('a11y-probe').remove(); 1",
            )
            .await;
            let framed = |found: &[bool]| found.iter().filter(|f| **f == frame.is_some()).count();
            (framed(&after) == framed(&before) + 1)
                .then_some(())
                .ok_or_else(|| {
                    format!("unnamed buttons (in an iframe?) {before:?}, then {after:?}")
                })
        }
        Probe::Offline => {
            emulate(engine, EmulationChange::Offline { offline: true }).await;
            let answer = ask(engine, who, "fetch").await;
            emulate(engine, EmulationChange::Offline { offline: false }).await;
            answer
                .as_str()
                .is_some_and(|a| a.starts_with("failed"))
                .then_some(())
                .ok_or_else(|| format!("fetch answered {answer}"))
        }
        Probe::Determinism => {
            // Seeded, each target starts the same sequence: its first
            // value is the one a fresh copy gives.
            let seeded = format!("{}(1, 0, false);", DETERMINISM.trim_end());
            let want = eval(
                engine,
                &format!("(() => {{ {seeded} return Math.random(); }})()"),
            )
            .await;
            engine
                .init_script_add(InitScriptAddRequest {
                    source: seeded,
                    file: None,
                    world: EvalWorld::Main,
                    now: true,
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .unwrap_or_else(|e| panic!("init-script: {}", e.to_text()));
            let report = ask(engine, who, "report").await;
            engine
                .init_script_remove(InitScriptRemoveRequest {
                    ids: Vec::new(),
                    all: true,
                    timeout_ms: TIMEOUT_MS,
                })
                .await
                .unwrap_or_else(|e| panic!("init-script remove: {}", e.to_text()));
            (report["random"] == want)
                .then_some(())
                .ok_or_else(|| format!("Math.random() {}, not {want}", report["random"]))
        }
    }
}

/// `emulate`'s warnings.
async fn emulate(engine: &Engine, change: EmulationChange) -> Vec<String> {
    engine
        .emulate(EmulateRequest {
            change,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("emulate: {}", e.to_text()))
        .warnings
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

/// `tz` and `locale` check that every worker took them (all do in Chrome
/// 154: nothing to say), and a first `time freeze` says the workers there
/// are got the clock late.
#[tokio::test]
async fn emulate_says_what_workers_missed() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-emulate").await;
    let tz = EmulationChange::Timezone {
        timezone: Some("Asia/Tokyo".to_owned()),
    };
    assert_eq!(emulate(&engine, tz).await, Vec::<String>::new());
    let locale = EmulationChange::Locale {
        locale: Some("fr-FR".to_owned()),
    };
    assert_eq!(emulate(&engine, locale).await, Vec::<String>::new());
    let freeze = EmulationChange::TimeFreeze {
        epoch_ms: FROZEN_MS,
    };
    let warnings = emulate(&engine, freeze.clone()).await;
    // The dedicated, shared and service worker, and the OOPIF's
    // dedicated and shared worker.
    assert_eq!(
        warnings,
        [
            "5 workers already running got the frozen clock late: timers set there before keep real time"
        ]
    );
    // Frozen already: nothing new.
    assert_eq!(emulate(&engine, freeze).await, Vec::<String>::new());
    // A worker that starts now has the frozen clock from its start.
    let now = eval(
        &engine,
        "new Promise((resolve) => { \
           const w = new Worker('/targets/dedicated.js?who=late'); \
           w.onmessage = (e) => { if (e.data.ready) w.postMessage({ id: 1, op: 'report' }); \
             else resolve(e.data.result.now); }; })",
    )
    .await;
    assert_eq!(now, json!(FROZEN_MS));
    engine.close().await;
}

/// A dedicated or shared worker started while mock sets are in use has
/// the mock shim before its own script runs: a socket it opens first
/// thing is mocked.
#[tokio::test]
async fn a_new_worker_has_the_mock_shim_first() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-shim").await;
    let (dedicated, shared) = with_mocks(&engine, async || {
        let dedicated = eval(
            &engine,
            "new Promise((resolve) => { \
               const w = new Worker('/targets/ws-first.js'); \
               w.onmessage = (e) => resolve(e.data); })",
        )
        .await;
        let shared = eval(
            &engine,
            "new Promise((resolve) => { \
               const w = new SharedWorker('/targets/ws-first.js'); \
               w.port.onmessage = (e) => resolve(e.data); })",
        )
        .await;
        (dedicated, shared)
    })
    .await;
    assert_eq!(dedicated, json!("hello"));
    assert_eq!(shared, json!("hello"));
    engine.close().await;
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
            all_sessions: false,
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

/// A shared worker's error is in the console of each tab of its origin,
/// and `expect --no-errors --all-tabs` counts it once.
#[tokio::test]
async fn a_shared_workers_error_counts_once_across_tabs() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-errors-once").await;
    let site = Fixtures::shared();
    engine
        .tab_new(TabNewRequest {
            url: Some(site.url("/targets/matrix.html")),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("tab new: {}", e.to_text()));
    eventually("the second tab's targets to answer", async || {
        let title = eval(&engine, "document.title").await;
        (title == json!("Targets: ready")).then_some(())
    })
    .await;
    ask(&engine, "shared", "throw").await;
    let in_both = eventually("the error in both tabs", async || {
        let mut tabs = 0;
        for tab in [1, 2] {
            let out = engine
                .console(ConsoleRequest {
                    errors: true,
                    tab: Some(format!("t{tab}")),
                    level: Level::Debug,
                    since: Since::Seq { seq: 0 },
                    grep: None,
                    limit: 0,
                    wait_ms: 0,
                    sourcemaps: false,
                    locals: false,
                    timeout_ms: TIMEOUT_MS,
                    max_chars: 0,
                })
                .await
                .unwrap();
            let data: ConsoleData = serde_json::from_value(out.data).unwrap();
            if data.entries.iter().any(|e| {
                e.text.contains("thrown in shared") && e.worker_kind == Some(WorkerKind::Shared)
            }) {
                tabs += 1;
            }
        }
        (tabs == 2).then_some(tabs)
    })
    .await;
    assert_eq!(in_both, 2);
    let err = Box::pin(engine.expect(ExpectRequest {
        expectation: Expectation::NoErrors {
            since: None,
            all_tabs: true,
        },
        not: false,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: 5_000,
        max_chars: 0,
    }))
    .await
    .unwrap_err();
    let text = err.to_text();
    assert!(text.contains("  actual: 1 error\n"), "{text}");
    assert!(
        text.contains("thrown in shared") && text.contains("[shared worker shared.js]"),
        "{text}"
    );
    engine.close().await;
}

/// `cdp --listen --all-sessions` hears a worker that starts while it
/// listens from its first request on: the script it imports.
#[tokio::test]
async fn all_sessions_hears_a_worker_that_starts_meanwhile() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-listen-new").await;
    let expression = "new Promise((resolve) => { \
        const w = new Worker('/targets/dedicated.js?who=late'); \
        w.onmessage = (e) => resolve(e.data); })";
    let out = engine
        .cdp(CdpRequest {
            method: Some("Runtime.evaluate".into()),
            params: json!({ "expression": expression, "awaitPromise": true })
                .as_object()
                .unwrap()
                .clone(),
            target: Target::Page,
            tab: None,
            frame: None,
            listen: vec!["Network.requestWillBeSent".into()],
            count: 0,
            all_sessions: true,
            timeout_ms: 2_000,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("cdp: {}", e.to_text()));
    let data: protocol::schema::CdpListenData = serde_json::from_value(out.data).unwrap();
    let heard: Vec<(String, &str)> = data
        .events
        .iter()
        .map(|e| {
            let url = e.params["request"]["url"].as_str().unwrap_or_default();
            let file = url.rsplit('/').next().unwrap_or_default().to_owned();
            (file, e.from.as_deref().unwrap_or_default())
        })
        .collect();
    assert!(
        heard.contains(&("ops.js".to_owned(), "dedicated worker dedicated.js")),
        "{heard:?}"
    );
    engine.close().await;
}

/// `coverage` measures a cross-site iframe that attaches while it runs,
/// from its first script on. It shares the process (and V8 isolate) of the
/// other iframe of its site, which ran code before it joined: each script
/// is counted once, an inline one under its own iframe.
#[tokio::test]
async fn coverage_measures_an_iframe_that_attaches_meanwhile() {
    let Some(path) = chrome_path() else { return };
    let engine = matrix(&path, "engine-targets-coverage-new").await;
    let site = Fixtures::shared();
    engine
        .coverage_start(CoverageStartRequest {
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("coverage start: {}", e.to_text()));
    // Before the late iframe joins its process.
    ask(&engine, "oopif", "fetch").await;
    let src = site.url_b("/targets/frame.html?who=late");
    eval(
        &engine,
        &format!(
            "new Promise((resolve) => {{ \
               addEventListener('message', (e) => {{ if (e.data.ready === 'late') resolve(); }}); \
               const f = document.createElement('iframe'); \
               f.name = 'late'; f.src = {src:?}; document.body.append(f); }})"
        ),
    )
    .await;
    let out = engine
        .coverage_stop(CoverageStopRequest {
            limit: 0,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("coverage stop: {}", e.to_text()));
    let data: CoverageStopData = serde_json::from_value(out.data).unwrap();
    let frames = engine.frames(None, Duration::from_secs(5)).await.unwrap();
    let id = |name: &str| frames.iter().find(|f| f.name == name).unwrap().id.clone();
    let files: Vec<(&str, Option<&str>)> = data
        .files
        .iter()
        .filter(|f| f.url.starts_with(site.b()) && f.worker.is_none())
        .map(|f| (f.url.rsplit('/').next().unwrap(), f.frame.as_deref()))
        .collect();
    let (late, oopif) = (id("late"), id("oopif"));
    let mut want = vec![
        ("frame.html?who=late", Some(late.as_str())),
        ("frame.html?who=oopif&workers=1", Some(oopif.as_str())),
        ("ops.js", Some(oopif.as_str())),
    ];
    let mut files = files;
    files.sort_unstable();
    want.sort_unstable();
    assert_eq!(files, want, "{}", out.text);
    engine.close().await;
}

//! `eval` and `cdp --listen` against a real Chrome (`CHROME_PATH`; skipped
//! without it unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture
//! site (plan 06, milestone 6.4).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    CdpRequest, Engine, EvalRequest, EvalWorld, NavigateRequest, Navigation, OpenRequest,
    SnapshotRequest, Target, WaitUntil,
};
use fixtures::Fixtures;
use protocol::schema::{CdpListenData, EvalData, SnapshotData, SnapshotNode};
use protocol::{ErrorInfo, ErrorKind, Output};
use serde_json::{Value, json};

const TIMEOUT_MS: u64 = 30_000;

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

async fn start(path: &Path, name: &str) -> Arc<Engine> {
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
    engine
}

async fn goto(engine: &Engine, url: &str) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

fn request(script: &str) -> EvalRequest {
    EvalRequest {
        script: script.to_owned(),
        locator: None,
        frame: None,
        world: EvalWorld::Main,
        tab: None,
        strict_refs: false,
        testid_attribute: None,
        sourcemaps: false,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn eval(engine: &Engine, req: EvalRequest) -> Result<(Output, EvalData), ErrorInfo> {
    let script = req.script.clone();
    let output = engine.eval(req).await?;
    let data = serde_json::from_value(output.data.clone())
        .unwrap_or_else(|e| panic!("{script}: {e}: {}", output.data));
    Ok((output, data))
}

/// The text form and type of `script`'s value.
async fn value(engine: &Engine, script: &str) -> (String, String) {
    let (output, data) = eval(engine, request(script))
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    (output.text, data.kind)
}

async fn failure(engine: &Engine, req: EvalRequest) -> ErrorInfo {
    let script = req.script.clone();
    match engine.eval(req).await {
        Ok(output) => panic!("{script}: no error: {}", output.text),
        Err(info) => info,
    }
}

/// Values of every kind: printed as the page wrote them, typed in
/// `--json`.
#[tokio::test]
async fn values_print_as_json_or_as_they_are() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-eval-values").await;
    goto(&engine, &site.url("/eval/index.html")).await;

    let cases = [
        ("document.title", "Eval", "string"),
        ("6 * 7", "42", "number"),
        ("undefined", "undefined", "undefined"),
        ("null", "null", "null"),
        ("NaN", "NaN", "number"),
        ("2n ** 70n", "1180591620717411303424n", "bigint"),
        // Keys keep their order; a lone object literal is an object.
        (
            "{b: 1, a: [true, 'x']}",
            "{\n  \"b\": 1,\n  \"a\": [\n    true,\n    \"x\"\n  ]\n}",
            "object",
        ),
        (
            "document.querySelector('#save')",
            "button#save.primary.wide [e1]",
            "node",
        ),
        (
            "[...document.querySelectorAll('.item')].map((li) => li.textContent)",
            "[\n  \"one\",\n  \"two\"\n]",
            "array",
        ),
        // Statements: the last value is the result.
        ("const n = 2; n * 3", "6", "number"),
        // Promises are awaited, functions called, `await` works.
        ("Promise.resolve('later')", "later", "string"),
        ("() => document.title.length", "4", "number"),
        (
            "await new Promise((r) => setTimeout(() => r('waited'), 20))",
            "waited",
            "string",
        ),
        (
            "const o = {}; o.me = o; o",
            "{\n  \"me\": \"[Circular]\"\n}",
            "object",
        ),
        ("new Map([['k', 1]])", "{\n  \"k\": 1\n}", "object"),
        ("new Date(0)", "1970-01-01T00:00:00.000Z", "object"),
        ("Symbol('s')", "Symbol(s)", "symbol"),
        // userGesture: the page sees a user activation.
        ("navigator.userActivation.isActive", "true", "boolean"),
    ];
    for (script, text, kind) in cases {
        let (shown, shown_kind) = value(&engine, script).await;
        assert_eq!(
            (shown.as_str(), shown_kind.as_str()),
            (text, kind),
            "{script}"
        );
    }

    // `--json` has the value itself; `-q` is compact.
    let (output, data) = eval(&engine, request("({a: 1, b: 'x'})")).await.unwrap();
    assert_eq!(data.value, json!({"a": 1, "b": "x"}));
    assert_eq!(output.quiet.as_deref(), Some(r#"{"a":1,"b":"x"}"#));
    assert_eq!(data.world, "main");

    // The script's declarations do not stay in the page.
    value(&engine, "let leftover = 1; leftover").await;
    assert_eq!(value(&engine, "typeof leftover").await.0, "undefined");

    // Long values are bounded.
    let output = engine
        .eval(EvalRequest {
            max_chars: 100,
            ..request("'x'.repeat(5000)")
        })
        .await
        .unwrap();
    assert!(output.limit.is_some());
    engine.close().await;
}

fn snapshot_ref(nodes: &[SnapshotNode], role: &str, name: &str) -> Option<String> {
    nodes.iter().find_map(|n| {
        if n.role == role && n.name == name {
            n.reference.clone()
        } else {
            snapshot_ref(&n.children, role, name)
        }
    })
}

/// `--in` binds the element as `el` (in its own frame), `--frame` runs
/// in a frame, `--world isolated` away from the page's globals.
#[tokio::test]
async fn elements_frames_and_worlds() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-eval-in").await;
    goto(&engine, &site.url("/eval/index.html")).await;

    let (_, data) = eval(
        &engine,
        EvalRequest {
            locator: Some("#name".into()),
            ..request("el.value")
        },
    )
    .await
    .unwrap();
    assert_eq!(data.value, json!("Ada"));
    assert_eq!(data.locator.as_deref(), Some("#name"));
    let (output, _) = eval(
        &engine,
        EvalRequest {
            locator: Some("#save".into()),
            ..request("(el) => el.className")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "primary wide");

    // A ref from a snapshot.
    let snapshot = engine
        .snapshot(SnapshotRequest {
            tab: None,
            interactive: true,
            scope: None,
            depth: None,
            diff: false,
            all_refs: false,
            all: false,
            text_limit: 200,
            no_text: false,
            engine: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let snapshot: SnapshotData = serde_json::from_value(snapshot.data).unwrap();
    let save = snapshot_ref(&snapshot.nodes, "button", "Save").unwrap();
    let (output, _) = eval(
        &engine,
        EvalRequest {
            locator: Some(save),
            ..request("el.id")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "save");

    // An element in a frame: the script runs in that frame.
    let (output, _) = eval(
        &engine,
        EvalRequest {
            locator: Some("frame=inner >> #in-frame".into()),
            ..request("`${el.textContent} ${location.pathname}`")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "inside the frame /eval/frame.html");
    let (output, data) = eval(
        &engine,
        EvalRequest {
            frame: Some("inner".into()),
            ..request("document.title")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "Inner");
    assert_eq!(data.frame.as_deref(), Some("inner"));

    // Not one element.
    let ambiguous = failure(
        &engine,
        EvalRequest {
            locator: Some(".item".into()),
            ..request("el.textContent")
        },
    )
    .await;
    assert_eq!(
        ambiguous.kind,
        ErrorKind::Ambiguous,
        "{}",
        ambiguous.to_text()
    );
    assert!(
        ambiguous
            .hint
            .as_deref()
            .is_some_and(|h| h.contains("chrome-cdp eval --in e")),
        "{}",
        ambiguous.to_text()
    );
    let missing = failure(
        &engine,
        EvalRequest {
            locator: Some("#nope".into()),
            // It waits for the element that long.
            timeout_ms: 300,
            ..request("el")
        },
    )
    .await;
    assert_eq!(missing.kind, ErrorKind::NotFound);
    assert!(
        missing.to_text().contains("waited: 300ms"),
        "{}",
        missing.to_text()
    );
    let both = failure(
        &engine,
        EvalRequest {
            locator: Some("#name".into()),
            frame: Some("inner".into()),
            ..request("el")
        },
    )
    .await;
    assert_eq!(both.kind, ErrorKind::BadArgs);

    // Worlds: the page's globals are in the main world only.
    assert_eq!(value(&engine, "typeof explode").await.0, "function");
    let (output, data) = eval(
        &engine,
        EvalRequest {
            world: EvalWorld::Isolated,
            ..request("`${typeof explode} ${document.title}`")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "undefined Eval");
    assert_eq!(data.world, "isolated");
    engine.close().await;
}

/// Elements in the value get refs from the tab's registry (the first 50),
/// which act like a snapshot's: `--in` takes them, and one whose element
/// was re-rendered falls back to its role and name.
#[tokio::test]
async fn returned_elements_get_refs() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-eval-refs").await;
    goto(&engine, &site.url("/eval/index.html")).await;

    let (output, data) = eval(
        &engine,
        request("[...document.querySelectorAll('.item'), document.querySelector('.item')]"),
    )
    .await
    .unwrap();
    assert_eq!(
        output.text,
        "[\n  \"li.item [e1]\",\n  \"li.item [e2]\",\n  \"li.item [e1]\"\n]"
    );
    assert_eq!(data.refs, ["e1", "e2"]);
    assert_eq!(data.refs_cut, 0);
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
    assert_eq!(data.value[1], json!("li.item [e2]"));

    // A ref names the element for any command, as a snapshot's does.
    let (output, _) = eval(
        &engine,
        EvalRequest {
            locator: Some("e2".into()),
            ..request("el.textContent")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "two");
    let (output, data) = eval(&engine, request("document.querySelector('#save')"))
        .await
        .unwrap();
    assert_eq!(output.text, "button#save.primary.wide [e3]");
    assert_eq!(
        output.quiet.as_deref(),
        Some("button#save.primary.wide [e3]")
    );
    assert_eq!(data.kind, "node");
    let snapshot = engine
        .snapshot(SnapshotRequest {
            tab: None,
            interactive: true,
            scope: None,
            depth: None,
            diff: false,
            all_refs: false,
            all: false,
            text_limit: 200,
            no_text: false,
            engine: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            max_chars: 0,
        })
        .await
        .unwrap();
    let snapshot: SnapshotData = serde_json::from_value(snapshot.data).unwrap();
    assert_eq!(
        snapshot_ref(&snapshot.nodes, "button", "Save").as_deref(),
        Some("e3")
    );

    // Nodes outside the document, and text, get none.
    let (output, data) = eval(
        &engine,
        request("[document.createElement('div'), document.querySelector('h1').firstChild]"),
    )
    .await
    .unwrap();
    assert_eq!(output.text, "[\n  \"div\",\n  \"#text \\\"Eval\\\"\"\n]");
    assert!(data.refs.is_empty());

    // In a frame, and in the isolated world.
    let (output, data) = eval(
        &engine,
        EvalRequest {
            frame: Some("inner".into()),
            ..request("document.querySelector('#in-frame')")
        },
    )
    .await
    .unwrap();
    let inner = data.refs[0].clone();
    assert_eq!(output.text, format!("p#in-frame [{inner}]"));
    let (output, _) = eval(
        &engine,
        EvalRequest {
            locator: Some(inner),
            ..request("`${el.textContent} ${location.pathname}`")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "inside the frame /eval/frame.html");
    let (output, data) = eval(
        &engine,
        EvalRequest {
            world: EvalWorld::Isolated,
            ..request("document.querySelector('#name')")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, format!("input#name [{}]", data.refs[0]));

    // At most 50 per call; the output says how many more there were.
    let script = "const box = document.createElement('div'); \
                  for (let i = 0; i < 60; i++) box.append(document.createElement('span')); \
                  document.body.append(box); [...box.children]";
    let (output, data) = eval(&engine, request(script)).await.unwrap();
    assert_eq!(data.refs.len(), 50);
    assert_eq!(data.refs_cut, 10);
    assert_eq!(data.value[49], json!(format!("span [{}]", data.refs[49])));
    assert_eq!(data.value[50], json!("span"));
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.starts_with("10 more elements have no ref")),
        "{:?}",
        output.warnings
    );

    // The ref keeps the role and name, for when the element is re-rendered.
    value(
        &engine,
        "const b = document.querySelector('#save'); b.replaceWith(b.cloneNode(true))",
    )
    .await;
    let (output, _) = eval(
        &engine,
        EvalRequest {
            locator: Some("e3".into()),
            ..request("el.id")
        },
    )
    .await
    .unwrap();
    assert_eq!(output.text, "save");
    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.contains("re-resolved by role+name")),
        "{:?}",
        output.warnings
    );
    engine.close().await;
}

/// Exceptions are `EvalError`s with the message, the stack (page frames
/// resolved with source maps on localhost, the script's own named `eval`
/// with its line numbers) and a hint when the mistake is a common one.
#[tokio::test]
async fn exceptions_show_a_mapped_stack() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-eval-errors").await;
    goto(&engine, &site.url("/eval/index.html")).await;

    let err = failure(&engine, request("const a = 1;\nexplode()")).await;
    assert_eq!(err.kind, ErrorKind::EvalError);
    assert_eq!(err.exit_code(), 1);
    assert!(
        err.message
            .starts_with("the script threw TypeError: Cannot read properties of null"),
        "{}",
        err.to_text()
    );
    let text = err.to_text();
    assert!(
        text.contains("  at: explode (/eval/src/app.ts:8:"),
        "{text}"
    );
    assert!(text.contains("  at: eval:2:1"), "{text}");
    let details = err.details.unwrap();
    assert_eq!(details["exception"], json!("TypeError"));
    let top = &details["stack"][0];
    assert!(top["url"].as_str().unwrap().ends_with("/eval/dist/app.js"));
    assert_eq!(top["line"], json!(4));
    assert_eq!(top["original"]["line"], json!(8));

    // Thrown from a function the script returned, and a rejection.
    let err = failure(&engine, request("() => { throw new RangeError('deep'); }")).await;
    assert_eq!(err.message, "the script threw RangeError: deep");
    let err = failure(&engine, request("await Promise.reject(new Error('no'))")).await;
    assert_eq!(err.message, "the script threw Error: no");
    let err = failure(&engine, request("throw 'bare'")).await;
    assert_eq!(err.message, "the script threw bare");

    // Syntax errors point into the script.
    let err = failure(&engine, request("1 +\nlet x = ;")).await;
    assert!(
        err.message
            .starts_with("the script does not parse: SyntaxError"),
        "{}",
        err.to_text()
    );
    assert!(err.to_text().contains("at: eval:2:"), "{}", err.to_text());
    assert!(
        err.hint.as_deref().is_some_and(|h| h.contains("eval -")),
        "{}",
        err.to_text()
    );
    // The `--in` element does not stay behind when the script does not
    // parse.
    let err = failure(
        &engine,
        EvalRequest {
            locator: Some("#name".into()),
            ..request("el.value +")
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::EvalError);
    assert_eq!(
        value(&engine, "Object.getOwnPropertySymbols(globalThis).length")
            .await
            .0,
        "0"
    );
    let err = failure(&engine, request("return 1")).await;
    assert!(
        err.hint
            .as_deref()
            .is_some_and(|h| h.contains("drop `return`")),
        "{}",
        err.to_text()
    );
    let err = failure(&engine, request("el.value")).await;
    assert!(
        err.hint.as_deref().is_some_and(|h| h.contains("--in e12")),
        "{}",
        err.to_text()
    );

    // A promise that never settles times out.
    let err = failure(
        &engine,
        EvalRequest {
            timeout_ms: 500,
            ..request("await new Promise(() => {})")
        },
    )
    .await;
    assert_eq!(err.kind, ErrorKind::Timeout, "{}", err.to_text());
    assert_eq!(err.message, "the script did not finish within 500ms");

    // Nothing to run.
    let err = failure(&engine, request("  ")).await;
    assert_eq!(err.kind, ErrorKind::BadArgs);

    // The page still works after all that.
    assert_eq!(value(&engine, "document.title").await.0, "Eval");
    engine.close().await;
}

fn cdp_request(method: Option<&str>, params: Value, listen: &[&str], count: usize) -> CdpRequest {
    CdpRequest {
        all_sessions: false,
        method: method.map(str::to_owned),
        params: serde_json::from_value(params).unwrap(),
        target: Target::Page,
        tab: None,
        frame: None,
        listen: listen.iter().map(|s| (*s).to_owned()).collect(),
        count,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

/// `cdp --listen`: the events of the call's session that follow it.
#[tokio::test]
async fn cdp_listens_for_events() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "engine-eval-listen").await;
    goto(&engine, &site.url("/eval/index.html")).await;

    let output = engine
        .cdp(cdp_request(
            Some("Page.reload"),
            json!({}),
            &["Page.loadEventFired"],
            1,
        ))
        .await
        .unwrap();
    let data: CdpListenData = serde_json::from_value(output.data.clone()).unwrap();
    assert_eq!(data.result, Some(serde_json::Map::new()));
    assert_eq!(data.events.len(), 1);
    assert_eq!(data.events[0].method, "Page.loadEventFired");
    assert!(
        output
            .text
            .starts_with("{}\nPage.loadEventFired {\"timestamp\":"),
        "{}",
        output.text
    );

    // `Domain.*`, several events, and no method: listen only, while
    // another command makes them happen.
    let listen = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .cdp(CdpRequest {
                    timeout_ms: 5_000,
                    ..cdp_request(None, json!({}), &["Runtime.*"], 2)
                })
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    value(&engine, "console.log('one'); console.log('two')").await;
    let output = listen.await.unwrap().unwrap();
    let data: CdpListenData = serde_json::from_value(output.data).unwrap();
    assert_eq!(data.result, None);
    let methods: Vec<&str> = data.events.iter().map(|e| e.method.as_str()).collect();
    assert_eq!(
        methods,
        ["Runtime.consoleAPICalled", "Runtime.consoleAPICalled"]
    );
    assert_eq!(data.events[1].params["args"][0]["value"], json!("two"));

    // `--count 0` keeps listening until the timeout; none is fine then.
    let output = engine
        .cdp(CdpRequest {
            timeout_ms: 300,
            ..cdp_request(None, json!({}), &["Animation.animationCreated"], 0)
        })
        .await
        .unwrap();
    assert_eq!(output.data["events"], json!([]));
    assert_eq!(
        output.warnings,
        ["no Animation.animationCreated event within 300ms"]
    );
    // With a count, none is a timeout that keeps the call's result.
    let err = engine
        .cdp(CdpRequest {
            timeout_ms: 300,
            ..cdp_request(
                Some("Runtime.evaluate"),
                json!({"expression": "1"}),
                &["Animation.animationCreated"],
                1,
            )
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
    assert_eq!(err.details.unwrap()["result"]["result"]["value"], json!(1));

    // Bad event names are usage errors.
    let err = engine
        .cdp(cdp_request(None, json!({}), &["nope"], 1))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);
    engine.close().await;
}

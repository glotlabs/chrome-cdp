//! Recordings against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site: `har`
//! (plan 06, milestone 6.5).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use browser::Host;
use engine::{
    Action, ActionRequest, Button, Engine, HarContent, HarStartRequest, HarStopRequest,
    NavigateRequest, Navigation, OpenRequest, TabNewRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::ErrorKind;
use protocol::schema::{HarStartData, HarStopData};
use serde_json::Value;

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

async fn click(engine: &Engine, locator: &str) {
    engine
        .act(ActionRequest {
            action: Action::Click {
                locator: locator.to_owned(),
                button: Button::Left,
                count: 1,
                modifiers: Vec::new(),
                position: None,
                force: false,
                dblclick: false,
            },
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            no_wait_after: false,
            snap: false,
            max_chars: 20_000,
        })
        .await
        .unwrap_or_else(|e| panic!("click {locator}: {}", e.to_text()));
}

async fn har_start(engine: &Engine, content: HarContent) -> HarStartData {
    let out = engine
        .har_start(HarStartRequest {
            tab: None,
            content,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("har start: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

async fn har_stop(engine: &Engine, path: &Path) -> (HarStopData, Value, Vec<String>) {
    let out = engine
        .har_stop(HarStopRequest {
            path: Some(path.to_owned()),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("har stop: {}", e.to_text()));
    let data: HarStopData = serde_json::from_value(out.data.clone()).unwrap();
    let har: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    (data, har, out.warnings)
}

/// Checks `har` against the HAR 1.2 spec's required fields and types
/// (the rules of the `har-schema` JSON Schema).
fn validate_har(har: &Value) {
    let date =
        regex::Regex::new(r"^\d{4}-?\d\d-?\d\dT?\d\d:?\d\d:?\d\d(\.\d+)?(Z|[+-]\d\d:?\d\d)$")
            .unwrap();
    let string = |v: &Value, at: &str| assert!(v.is_string(), "{at} is not a string: {v}");
    let number = |v: &Value, at: &str| assert!(v.is_number(), "{at} is not a number: {v}");
    let name_values = |v: &Value, at: &str| {
        let list = v
            .as_array()
            .unwrap_or_else(|| panic!("{at} is not an array"));
        for item in list {
            string(&item["name"], &format!("{at}[].name"));
            string(&item["value"], &format!("{at}[].value"));
        }
    };
    let log = &har["log"];
    assert_eq!(log["version"], "1.2");
    string(&log["creator"]["name"], "creator.name");
    string(&log["creator"]["version"], "creator.version");
    for page in log["pages"].as_array().unwrap() {
        assert!(date.is_match(page["startedDateTime"].as_str().unwrap()));
        string(&page["id"], "page.id");
        string(&page["title"], "page.title");
        assert!(page["pageTimings"].is_object());
    }
    for entry in log["entries"].as_array().unwrap() {
        let at = entry["request"]["url"].as_str().unwrap_or("?");
        assert!(
            date.is_match(entry["startedDateTime"].as_str().unwrap()),
            "{at}: startedDateTime"
        );
        assert!(entry["time"].as_f64().unwrap() >= 0.0, "{at}: time");
        let request = &entry["request"];
        for field in ["method", "url", "httpVersion"] {
            string(&request[field], &format!("{at}: request.{field}"));
        }
        name_values(&request["cookies"], "request.cookies");
        name_values(&request["headers"], "request.headers");
        name_values(&request["queryString"], "request.queryString");
        if let Some(post) = request.get("postData") {
            string(&post["mimeType"], "postData.mimeType");
            string(&post["text"], "postData.text");
        }
        number(&request["headersSize"], "request.headersSize");
        number(&request["bodySize"], "request.bodySize");
        let response = &entry["response"];
        number(&response["status"], "response.status");
        for field in ["statusText", "httpVersion", "redirectURL"] {
            string(&response[field], &format!("{at}: response.{field}"));
        }
        name_values(&response["cookies"], "response.cookies");
        name_values(&response["headers"], "response.headers");
        number(&response["content"]["size"], "content.size");
        string(&response["content"]["mimeType"], "content.mimeType");
        number(&response["headersSize"], "response.headersSize");
        number(&response["bodySize"], "response.bodySize");
        assert!(entry["cache"].is_object());
        let timings = &entry["timings"];
        for phase in ["send", "wait", "receive"] {
            assert!(timings[phase].as_f64().unwrap() >= 0.0, "{at}: {phase}");
        }
        for phase in ["blocked", "dns", "connect", "ssl"] {
            assert!(timings[phase].as_f64().unwrap() >= -1.0, "{at}: {phase}");
        }
    }
}

fn entry<'a>(har: &'a Value, url_end: &str) -> &'a Value {
    har["log"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["request"]["url"].as_str().unwrap().ends_with(url_end))
        .unwrap_or_else(|| panic!("no entry for {url_end}: {har}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn har_records_every_tab_from_the_start() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&path, "record-har").await;
    goto(&engine, &site.url("/har/index.html")).await;

    // Stopping without a start says how to start.
    let err = engine
        .har_stop(HarStopRequest {
            path: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadArgs);
    assert!(err.to_text().contains("har start"), "{}", err.to_text());

    let started = har_start(&engine, HarContent::Embed).await;
    assert!(!started.already);
    assert!(har_start(&engine, HarContent::Embed).await.already);
    click(&engine, "#post").await;
    engine
        .tab_new(TabNewRequest {
            url: Some(site.url("/har/index.html")),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();

    let file = dir.path().join("out/run.har");
    let (data, har, warnings) = har_stop(&engine, &file).await;
    validate_har(&har);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(data.pages, ["t1", "t2"]);
    assert_eq!(data.path, file.display().to_string());
    assert_eq!(data.missing_bodies, 0);
    let entries = har["log"]["entries"].as_array().unwrap();
    // t1's own load came before the start: only its POST is in.
    let t1: Vec<&str> = entries
        .iter()
        .filter(|e| e["pageref"] == "t1")
        .map(|e| e["request"]["url"].as_str().unwrap())
        .collect();
    assert_eq!(t1.len(), 1, "{t1:?}");
    let post = entry(&har, "/items?n=2");
    assert_eq!(post["request"]["method"], "POST");
    assert_eq!(post["request"]["postData"]["text"], r#"{"a":1}"#);
    assert_eq!(post["request"]["postData"]["mimeType"], "application/json");
    assert_eq!(post["request"]["queryString"][0]["name"], "n");
    assert_eq!(post["response"]["status"], 200);
    assert_eq!(post["_resourceType"], "fetch");
    let items: Value =
        serde_json::from_str(post["response"]["content"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 2);
    // t2 is in from its first request; the image comes in base64.
    let document = entries
        .iter()
        .find(|e| e["pageref"] == "t2" && e["_resourceType"] == "document")
        .unwrap();
    assert!(
        document["response"]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("<title>HAR</title>")
    );
    let png = entry(&har, "/har/dot.png");
    assert_eq!(png["pageref"], "t2");
    assert_eq!(png["response"]["content"]["encoding"], "base64");
    assert_eq!(png["response"]["content"]["mimeType"], "image/png");
    assert!(png["timings"]["wait"].as_f64().unwrap() >= 0.0);
    assert_eq!(data.entries, entries.len());
    assert_eq!(data.bodies, entries.len());

    // The recording ended; `--content omit` keeps bodies out.
    assert!(!har_start(&engine, HarContent::Omit).await.already);
    click(&engine, "#post").await;
    let (data, har, _) = har_stop(&engine, &dir.path().join("omit.har")).await;
    validate_har(&har);
    assert_eq!((data.entries, data.bodies), (1, 0));
    let post = entry(&har, "/items?n=2");
    assert!(post["response"]["content"].get("text").is_none());
    assert!(post["response"]["content"]["size"].as_i64().unwrap() > 0);
    engine.close().await;
}

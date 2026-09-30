//! Emulation (`emulate`, `viewport`, `geo`, `tz`, `locale`, `ua`, `media`,
//! `offline`, `throttle`, `cpu-throttle`, `time`, `permissions`; plan 07,
//! milestone 7.2) against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site's
//! `emulate/` pages. Every setting is checked from page JS.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    Action, ActionRequest, EmulateRequest, EmulationChange, Engine, EvalRequest, EvalWorld,
    NavigateRequest, Navigation, OpenRequest, PermissionsOp, PermissionsRequest, TabSelectRequest,
    TabsRequest, WaitUntil,
};
use fixtures::Fixtures;
use protocol::ErrorInfo;
use protocol::schema::{
    EmulateData, EvalData, Geolocation, Media, PermissionsData, TabsData, Throttle, Viewport,
};
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

async fn open(engine: &Arc<Engine>, path: &Path, chrome_args: Vec<String>) {
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            chrome_args,
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
}

async fn start(path: &Path, name: &str) -> Arc<Engine> {
    let engine = Engine::new(name, None, None);
    open(&engine, path, Vec::new()).await;
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

fn eval_request(script: &str) -> EvalRequest {
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

async fn eval_with(engine: &Engine, req: EvalRequest) -> Value {
    let script = req.script.clone();
    let out = engine
        .eval(req)
        .await
        .unwrap_or_else(|e| panic!("{script}: {}", e.to_text()));
    let data: EvalData = serde_json::from_value(out.data).unwrap();
    data.value
}

/// `script`'s value in the active tab.
async fn eval(engine: &Engine, script: &str) -> Value {
    eval_with(engine, eval_request(script)).await
}

async fn try_emulate(engine: &Engine, change: EmulationChange) -> Result<EmulateData, ErrorInfo> {
    let out = engine
        .emulate(EmulateRequest {
            change,
            timeout_ms: TIMEOUT_MS,
        })
        .await?;
    Ok(serde_json::from_value(out.data).unwrap())
}

async fn emulate(engine: &Engine, change: EmulationChange) -> EmulateData {
    let what = format!("{change:?}");
    try_emulate(engine, change)
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()))
}

async fn permissions(
    engine: &Engine,
    op: PermissionsOp,
    names: &[&str],
    origin: Option<&str>,
) -> PermissionsData {
    let out = engine
        .permissions(PermissionsRequest {
            op,
            names: names.iter().map(|&n| n.to_owned()).collect(),
            origin: origin.map(str::to_owned),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("permissions {op:?} {names:?}: {}", e.to_text()));
    serde_json::from_value(out.data).unwrap()
}

fn device(name: &str) -> EmulationChange {
    EmulationChange::Device {
        name: name.to_owned(),
    }
}

fn viewport(width: u32, height: u32, dpr: f64) -> EmulationChange {
    EmulationChange::Viewport {
        viewport: Some(Viewport {
            width,
            height,
            dpr,
            mobile: false,
            touch: false,
            screen_width: None,
            screen_height: None,
        }),
    }
}

/// What `emulate/first.html`'s first script saw, of the tab `tab` (the
/// active one if `None`) and the frame `frame`.
async fn first(engine: &Engine, tab: Option<&str>, frame: Option<&str>) -> Value {
    let mut req = eval_request("window.first");
    req.tab = tab.map(str::to_owned);
    req.frame = frame.map(str::to_owned);
    let start = Instant::now();
    loop {
        let value = eval_with(engine, req.clone()).await;
        if !value.is_null() {
            return value;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no window.first");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Clicks `locator` with the engine's pipeline.
async fn act(engine: &Engine, action: Action) {
    let what = format!("{action:?}");
    engine
        .act(ActionRequest {
            action,
            tab: None,
            strict_refs: false,
            testid_attribute: None,
            timeout_ms: TIMEOUT_MS,
            no_wait_after: false,
            snap: false,
            max_chars: 0,
        })
        .await
        .unwrap_or_else(|e| panic!("{what}: {}", e.to_text()));
}

fn click(locator: &str) -> Action {
    Action::Click {
        locator: locator.to_owned(),
        button: engine::Button::Left,
        count: 1,
        modifiers: Vec::new(),
        position: None,
        force: false,
        dblclick: false,
    }
}

async fn log(engine: &Engine) -> String {
    eval(engine, "document.getElementById('log').textContent")
        .await
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn headless_pages_get_a_fixed_default_viewport() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "emulate-default").await;
    assert_eq!(
        eval(&engine, "[innerWidth, innerHeight, devicePixelRatio]").await,
        json!([1280, 800, 1])
    );
    let data = emulate(&engine, viewport(800, 600, 2.0)).await;
    assert_eq!(data.default_viewport.unwrap().width, 1280);
    assert_eq!(
        eval(&engine, "[innerWidth, innerHeight, devicePixelRatio]").await,
        json!([800, 600, 2])
    );
    emulate(&engine, EmulationChange::Viewport { viewport: None }).await;
    assert_eq!(
        eval(&engine, "[innerWidth, innerHeight, devicePixelRatio]").await,
        json!([1280, 800, 1])
    );
    engine.close().await;

    // A --window-size of the user's decides instead.
    let engine = Engine::new("emulate-window-size", None, None);
    open(&engine, &path, vec!["--window-size=1000,700".into()]).await;
    let width = eval(&engine, "innerWidth").await;
    assert_ne!(width, json!(1280));
    let data = emulate(&engine, EmulationChange::Show).await;
    assert!(data.default_viewport.is_none());
    engine.close().await;
}

#[tokio::test]
async fn each_setting_is_seen_by_page_js() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "emulate-each").await;
    goto(&engine, &site.url("/emulate/permissions.html")).await;

    // A device: viewport, pixel ratio, touch, user agent.
    let data = emulate(&engine, device("iphone 15")).await;
    assert_eq!(data.emulation.device.as_deref(), Some("iPhone 15"));
    goto(&engine, &site.url("/emulate/first.html")).await;
    let seen = eval(
        &engine,
        "[innerWidth, devicePixelRatio, navigator.maxTouchPoints > 0, 'ontouchstart' in window, navigator.userAgent.includes('iPhone'), screen.width, screen.height]",
    )
    .await;
    assert_eq!(seen, json!([393, 3, true, true, true, 393, 852]));

    let seen = |script: &'static str| {
        let engine = engine.clone();
        async move { eval(&engine, script).await }
    };
    emulate(
        &engine,
        EmulationChange::Locale {
            locale: Some("de-DE".into()),
        },
    )
    .await;
    assert_eq!(
        seen("[navigator.language, new Intl.NumberFormat().format(1234.5)]").await,
        json!(["de-DE", "1.234,5"])
    );
    let accept =
        seen("fetch('/echo/lang').then((r) => r.json()).then((j) => j.headers['accept-language'])")
            .await;
    assert!(accept.as_str().unwrap().starts_with("de-DE"), "{accept}");

    emulate(
        &engine,
        EmulationChange::Timezone {
            timezone: Some("Asia/Tokyo".into()),
        },
    )
    .await;
    assert_eq!(
        seen("[Intl.DateTimeFormat().resolvedOptions().timeZone, new Date(0).getHours()]").await,
        json!(["Asia/Tokyo", 9])
    );

    emulate(
        &engine,
        EmulationChange::Media {
            media: Some(Media {
                r#type: Some("print".into()),
                color_scheme: Some("dark".into()),
                reduced_motion: Some("reduce".into()),
                forced_colors: Some("active".into()),
            }),
        },
    )
    .await;
    assert_eq!(
        seen("['print', '(prefers-color-scheme: dark)', '(prefers-reduced-motion: reduce)', '(forced-colors: active)'].map((q) => matchMedia(q).matches)").await,
        json!([true, true, true, true])
    );
    // One feature changes, the others stay.
    emulate(
        &engine,
        EmulationChange::Media {
            media: Some(Media {
                color_scheme: Some("light".into()),
                ..Media::default()
            }),
        },
    )
    .await;
    assert_eq!(
        seen("['print', '(prefers-color-scheme: dark)', '(prefers-reduced-motion: reduce)'].map((q) => matchMedia(q).matches)").await,
        json!([true, false, true])
    );
    emulate(&engine, EmulationChange::Media { media: None }).await;
    assert_eq!(seen("matchMedia('print').matches").await, json!(false));

    emulate(
        &engine,
        EmulationChange::UserAgent {
            user_agent: Some("TestBot/1.0".into()),
        },
    )
    .await;
    assert_eq!(seen("navigator.userAgent").await, json!("TestBot/1.0"));
    let agent =
        seen("fetch('/echo/ua').then((r) => r.json()).then((j) => j.headers['user-agent'])").await;
    assert_eq!(agent, json!("TestBot/1.0"));

    emulate(&engine, EmulationChange::Offline { offline: true }).await;
    assert_eq!(seen("navigator.onLine").await, json!(false));
    assert_eq!(
        seen("fetch('/echo/offline').then(() => 'loaded', () => 'failed')").await,
        json!("failed")
    );
    emulate(&engine, EmulationChange::Offline { offline: false }).await;
    assert_eq!(seen("navigator.onLine").await, json!(true));

    // Latency holds each response back (a correctness check with a wide
    // margin, not a measurement).
    emulate(
        &engine,
        EmulationChange::Throttle {
            throttle: Some(Throttle {
                preset: None,
                latency_ms: 600.0,
                download_kbps: None,
                upload_kbps: None,
            }),
        },
    )
    .await;
    let took = seen("(async () => { const t = performance.now(); await fetch('/echo/slow?' + Math.random()); return performance.now() - t; })()").await;
    assert!(took.as_f64().unwrap() >= 500.0, "{took}");
    emulate(&engine, EmulationChange::Throttle { throttle: None }).await;
    let data = emulate(&engine, EmulationChange::CpuThrottle { rate: Some(4.0) }).await;
    assert_eq!(data.emulation.cpu_throttle, Some(4.0));
    emulate(&engine, EmulationChange::CpuThrottle { rate: None }).await;

    // The position, with the permission granted on the way.
    goto(&engine, &site.url("/emulate/permissions.html")).await;
    emulate(
        &engine,
        EmulationChange::Geolocation {
            geolocation: Some(Geolocation {
                latitude: 59.9139,
                longitude: 10.7522,
                accuracy: 25.0,
            }),
        },
    )
    .await;
    assert_eq!(seen("state('geolocation')").await, json!("granted"));
    assert_eq!(seen("position()").await, json!([59.9139, 10.7522, 25]));

    // A frozen clock, moved on, then real again.
    emulate(
        &engine,
        EmulationChange::TimeFreeze {
            epoch_ms: 1_769_851_800_000,
        },
    )
    .await;
    assert_eq!(
        seen("[Date.now(), new Date().toISOString(), new Date(5).getTime(), new Date() instanceof Date, typeof Date()]").await,
        json!([1_769_851_800_000_i64, "2026-01-31T09:30:00.000Z", 5, true, "string"])
    );
    // Timers set while frozen: moving on runs the ones due by then, each
    // once and in the order they are due, and performance.now() moves too.
    seen(
        "window.fired = []; setTimeout(() => fired.push('60s'), 60_000); \
          setTimeout((a) => fired.push(a), 30_000, '30s'); \
          clearTimeout(setTimeout(() => fired.push('cleared'), 1_000)); \
          setTimeout(() => fired.push('2m'), 120_000); \
          window.every = setInterval(() => fired.push('every 20s'), 20_000); \
          window.started = performance.now(); 0",
    )
    .await;
    emulate(&engine, EmulationChange::TimeAdvance { ms: 90_000 }).await;
    assert_eq!(
        seen("new Date().toISOString()").await,
        json!("2026-01-31T09:31:30.000Z")
    );
    assert_eq!(
        seen("new Promise((r) => setTimeout(r, 100)).then(() => [fired, performance.now() - started >= 90_000])").await,
        json!([["every 20s", "30s", "60s"], true])
    );
    seen("clearInterval(every); 0").await;
    // A new document gets the moved time.
    goto(&engine, &site.url("/emulate/first.html")).await;
    assert_eq!(
        first(&engine, None, None).await["now"],
        json!(1_769_851_890_000_i64)
    );
    emulate(&engine, EmulationChange::TimeReset).await;
    assert!(seen("Date.now()").await.as_f64().unwrap() > 1_780_000_000_000.0);
    assert_eq!(seen("Date.now === Date.now").await, json!(true));

    // Reset puts everything back.
    let data = emulate(&engine, EmulationChange::Reset).await;
    assert_eq!(data.emulation, protocol::schema::Emulation::default());
    goto(&engine, &site.url("/emulate/first.html")).await;
    let seen = first(&engine, None, None).await;
    assert_eq!(seen["width"], json!(1280));
    assert_eq!(seen["language"], json!("en-US"));
    assert_eq!(seen["timeZone"], json!("UTC"));
    assert_eq!(seen["touch"], json!(0));
    assert!(
        seen["userAgent"]
            .as_str()
            .unwrap()
            .contains("HeadlessChrome")
    );
    engine.close().await;
}

#[tokio::test]
async fn unknown_time_zones_and_locales_are_refused() {
    let Some(path) = chrome_path() else { return };
    let engine = start(&path, "emulate-refused").await;
    for twice in [false, true] {
        if twice {
            // Chrome keeps an old zone silently once one is set.
            emulate(
                &engine,
                EmulationChange::Timezone {
                    timezone: Some("Europe/Oslo".into()),
                },
            )
            .await;
        }
        let err = try_emulate(
            &engine,
            EmulationChange::Timezone {
                timezone: Some("Mars/Base".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.exit_code(), 2, "{}", err.to_text());
        assert!(err.message.contains("Mars/Base"), "{}", err.message);
    }
    let data = emulate(&engine, EmulationChange::Show).await;
    assert_eq!(data.emulation.timezone.as_deref(), Some("Europe/Oslo"));
    let err = try_emulate(
        &engine,
        EmulationChange::Locale {
            locale: Some("xx_bogus!".into()),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{}", err.to_text());
    let err = try_emulate(&engine, EmulationChange::TimeAdvance { ms: 1 })
        .await
        .unwrap_err();
    assert!(err.to_text().contains("time freeze"), "{}", err.to_text());
    engine.close().await;
}

/// Settings apply before a page's first script runs: set before the
/// browser starts, in a popup opened later, and in an out-of-process
/// iframe.
#[tokio::test]
async fn settings_apply_to_first_scripts_popups_and_frames() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = Engine::new("emulate-first", None, None);
    // Emulation belongs to the session: it waits for its browser.
    emulate(&engine, device("Pixel 7")).await;
    for change in [
        EmulationChange::Locale {
            locale: Some("fr-FR".into()),
        },
        EmulationChange::Timezone {
            timezone: Some("America/New_York".into()),
        },
        EmulationChange::Media {
            media: Some(Media {
                color_scheme: Some("dark".into()),
                reduced_motion: Some("reduce".into()),
                forced_colors: Some("active".into()),
                ..Media::default()
            }),
        },
        EmulationChange::TimeFreeze {
            epoch_ms: 1_700_000_000_000,
        },
        EmulationChange::Geolocation {
            geolocation: Some(Geolocation {
                latitude: 48.8566,
                longitude: 2.3522,
                accuracy: 10.0,
            }),
        },
        EmulationChange::Throttle {
            throttle: Some(Throttle {
                preset: None,
                latency_ms: 300.0,
                download_kbps: None,
                upload_kbps: None,
            }),
        },
    ] {
        emulate(&engine, change).await;
    }
    open(&engine, &path, Vec::new()).await;
    let want = |width: u64| {
        json!({
            "width": width,
            "dpr": 2.625,
            "touch": 5,
            "language": "fr-FR",
            "timeZone": "America/New_York",
            "dark": true,
            "reducedMotion": true,
            "forcedColors": true,
            "onLine": true,
            "now": 1_700_000_000_000_i64,
        })
    };
    let check = |seen: Value, width: Option<u64>, what: &str| {
        let mut seen = seen;
        let seen = seen.as_object_mut().unwrap();
        let agent = seen.remove("userAgent").unwrap();
        assert!(
            agent.as_str().unwrap().contains("Pixel 7"),
            "{what}: {agent}"
        );
        let mut want = want(width.unwrap_or(0));
        if width.is_none() {
            for key in ["width", "dpr"] {
                seen.remove(key);
                want.as_object_mut().unwrap().remove(key);
            }
        }
        assert_eq!(Value::Object(seen.clone()), want, "{what}");
    };
    // The position and the throttled fetch the first script asked for.
    let later = |tab: Option<String>, frame: Option<&'static str>, what: &'static str| {
        let engine = engine.clone();
        async move {
            let mut req = eval_request("window.later");
            req.tab = tab;
            req.frame = frame.map(str::to_owned);
            let start = Instant::now();
            let later = loop {
                let value = eval_with(&engine, req.clone()).await;
                if !value["fetchMs"].is_null() && !value["position"].is_null() {
                    break value;
                }
                assert!(start.elapsed() < Duration::from_secs(10), "{what}: {value}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            assert_eq!(later["position"], json!([48.8566, 2.3522]), "{what}");
            // A correctness check with a wide margin, not a measurement.
            let took = later["fetchMs"].as_f64();
            assert!(took.is_some_and(|ms| ms >= 250.0), "{what}: {later}");
        }
    };

    goto(&engine, &site.url("/emulate/first.html")).await;
    check(first(&engine, None, None).await, Some(412), "the first tab");
    later(None, None, "the first tab").await;

    // An iframe of the other origin (its own process and session) gets
    // the session's settings too. Chrome sets device metrics only on
    // top-level targets: the frame keeps a pixel ratio of 1, and its
    // first script runs before it has a size. (Checked before the popup,
    // which hides this tab: Chrome holds back a hidden page's geolocation
    // requests.)
    goto(&engine, &site.url("/emulate/frame.html")).await;
    check(
        first(&engine, None, Some("child")).await,
        None,
        "the cross-origin iframe",
    );
    later(None, Some("child"), "the cross-origin iframe").await;
    let mut size = eval_request("innerWidth");
    size.frame = Some("child".into());
    assert_eq!(eval_with(&engine, size).await, json!(400));

    // A popup's first script.
    eval(&engine, "window.open('/emulate/first.html'); true").await;
    let start = Instant::now();
    let popup = loop {
        let out = engine
            .tabs(TabsRequest {
                timeout_ms: TIMEOUT_MS,
                max_chars: 0,
            })
            .await
            .unwrap();
        let tabs: TabsData = serde_json::from_value(out.data).unwrap();
        if let Some(tab) = tabs
            .tabs
            .iter()
            .find(|t| t.id != "t1" && t.url.ends_with("/emulate/first.html"))
        {
            break tab.id.clone();
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "{:#?}",
            tabs.tabs
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    check(
        first(&engine, Some(&popup), None).await,
        Some(412),
        "the popup",
    );
    // The popup opens behind its opener, and Chrome holds back a hidden
    // page's geolocation requests: bring it to the front.
    engine
        .tab_select(TabSelectRequest {
            tab: popup.clone(),
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
    later(Some(popup), None, "the popup").await;
    engine.close().await;
}

#[tokio::test]
async fn permissions_are_granted_denied_and_reset() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "emulate-permissions").await;
    goto(&engine, &site.url("/emulate/permissions.html")).await;
    let state = |name: &'static str| {
        let engine = engine.clone();
        async move { eval(&engine, &format!("state('{name}')")).await }
    };
    assert_eq!(state("notifications").await, json!("prompt"));

    let data = permissions(
        &engine,
        PermissionsOp::Grant,
        &["notifications", "camera"],
        None,
    )
    .await;
    assert_eq!(data.permissions.len(), 2);
    assert_eq!(state("notifications").await, json!("granted"));
    assert_eq!(state("camera").await, json!("granted"));

    permissions(
        &engine,
        PermissionsOp::Deny,
        &["geolocation"],
        Some(site.a()),
    )
    .await;
    assert_eq!(state("geolocation").await, json!("denied"));
    // Another origin is not affected.
    let mut other =
        eval_request("navigator.permissions.query({ name: 'geolocation' }).then((s) => s.state)");
    goto(&engine, &site.url_b("/emulate/permissions.html")).await;
    other.frame = None;
    assert_eq!(eval_with(&engine, other).await, json!("prompt"));
    goto(&engine, &site.url("/emulate/permissions.html")).await;

    let data = permissions(&engine, PermissionsOp::Reset, &["notifications"], None).await;
    assert_eq!(data.permissions.len(), 2);
    assert_eq!(state("notifications").await, json!("prompt"));
    assert_eq!(state("camera").await, json!("granted"));

    let data = permissions(&engine, PermissionsOp::Reset, &[], None).await;
    assert!(data.permissions.is_empty());
    assert_eq!(state("camera").await, json!("prompt"));
    assert_eq!(state("geolocation").await, json!("prompt"));

    // They are the session's: a restarted browser gets them.
    permissions(&engine, PermissionsOp::Grant, &["notifications"], None).await;
    engine.close().await;
    open(&engine, &path, Vec::new()).await;
    goto(&engine, &site.url("/emulate/permissions.html")).await;
    assert_eq!(state("notifications").await, json!("granted"));
    let data = permissions(&engine, PermissionsOp::List, &[], None).await;
    assert_eq!(data.permissions.len(), 1);
    engine.close().await;
}

/// Clicks and taps land under a mobile device's pixel ratio and page
/// scale, and at a pixel ratio of 2.
#[tokio::test]
async fn actions_land_under_emulation() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path, "emulate-actions").await;
    for change in [
        viewport(800, 600, 2.0),
        // No meta viewport: the page is laid out 980 wide and zoomed out.
        device("Pixel 7"),
        device("iPad Mini landscape"),
    ] {
        let what = format!("{change:?}");
        emulate(&engine, change).await;
        goto(&engine, &site.url("/actions/scroll.html")).await;
        for id in ["near", "far", "deep", "right"] {
            let before = log(&engine).await;
            act(&engine, click(&format!("#{id}"))).await;
            let after = log(&engine).await;
            assert_eq!(
                after.strip_prefix(&before).map(str::trim),
                Some(id),
                "{what}: a click on #{id} logged {after:?}"
            );
        }
    }
    // Input reaches an out-of-process iframe and the wheel scrolls in all
    // of a viewport larger than headless Chrome's window.
    emulate(&engine, viewport(1600, 1100, 1.0)).await;
    goto(&engine, &site.url("/actions/frames.html")).await;
    act(&engine, click("frame=other >> #inner-far")).await;
    let mut in_frame = eval_request("document.getElementById('log').textContent");
    in_frame.frame = Some("other".into());
    assert_eq!(eval_with(&engine, in_frame).await, json!("inner-far"));

    // A device has touch, so `tap` works.
    emulate(&engine, device("iPhone 15")).await;
    goto(&engine, &site.url("/actions/touch.html")).await;
    act(
        &engine,
        Action::Tap {
            locator: "#tap".into(),
            modifiers: Vec::new(),
            position: None,
            force: false,
        },
    )
    .await;
    let logged = log(&engine).await;
    assert!(
        logged.contains("touchstart:tap") && logged.contains("click:tap"),
        "{logged}"
    );
    engine.close().await;
}

/// `open --device` emulates the device before the page loads.
#[tokio::test]
async fn open_emulates_a_device_before_loading() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = Engine::new("emulate-open", None, None);
    let chrome = browser::discover(Some(&path), &Host::current()).unwrap();
    let request = OpenRequest {
        chrome: Some(chrome),
        device: Some("pixel 7".into()),
        url: Some(site.url("/emulate/first.html")),
        timeout_ms: TIMEOUT_MS,
        ..OpenRequest::default()
    };
    let bad = OpenRequest {
        device: Some("Nokia 3310".into()),
        ..request.clone()
    };
    let err = engine.open(bad).await.unwrap_err();
    assert_eq!(err.exit_code(), 2, "{}", err.to_text());
    assert_eq!(engine.state(), protocol::schema::SessionState::Idle);

    let opened = engine.open(request).await.unwrap();
    assert_eq!(opened.data.device.as_deref(), Some("Pixel 7"));
    let seen = first(&engine, None, None).await;
    assert_eq!(
        (seen["width"].clone(), seen["dpr"].clone()),
        (json!(412), json!(2.625))
    );
    engine.close().await;
}

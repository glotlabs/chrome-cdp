//! Emulation in page, iframe and worker sessions (plan 07, milestone 7.2;
//! workers since plan 13.3; the settings are [`crate::emulate`]'s).
//!
//! A new session gets every setting that is on during its setup, while it
//! is paused ([`setup_steps`]); a change is sent to every session there is
//! ([`apply`]), only for what changed. The viewport is a page's (an
//! out-of-process iframe follows its page's); the rest is sent to iframe
//! sessions too, since each renderer has its own. The frozen clock is an
//! init script, and each session remembers its id to replace it.
//!
//! A worker gets what means something there (see "Decisions log", plan
//! 13.3): the user agent (its requests', and `navigator.userAgent`
//! through [`agent_script`], which Chrome leaves alone), the locale, the
//! time zone, offline and throttling (not in a dedicated worker, whose
//! requests are its frame's), and the frozen clock, run with
//! `Runtime.evaluate` (before its own script in a new worker, late in a
//! running one).

use std::sync::Arc;
use std::time::Duration;

use cdp::Session;
use cdp::domains::emulation::{
    ClearDeviceMetricsOverrideParams, ClearGeolocationOverrideParams, MediaFeature,
    ScreenOrientation, ScreenOrientationType, SetCpuThrottlingRateParams,
    SetDeviceMetricsOverrideParams, SetEmulatedMediaParams, SetGeolocationOverrideParams,
    SetLocaleOverrideParams, SetTimezoneOverrideParams, SetTouchEmulationEnabledParams,
    SetUserAgentOverrideParams,
};
use cdp::domains::{browser, network, page, runtime};
use futures_util::future::join_all;
use protocol::schema::{Emulation, Viewport};

use super::features::Feature;
use super::targets::TargetKind;
use super::{Shared, Step, lock, step};
use crate::emulate::{Aspects, agent_script, clock_script};

/// The setup steps of a new page, iframe or worker session (of `kind`).
pub(super) fn setup_steps(
    shared: &Arc<Shared>,
    session: &Session,
    kind: TargetKind,
    steps: &mut Vec<(&str, Step)>,
) {
    let scope = session.id().and_then(|id| {
        lock(&shared.state)
            .targets
            .get(id)
            .and_then(|t| t.scope.clone())
    });
    let state = shared.settings.emulation.get_in(scope.as_deref());
    let default = shared.default_viewport.as_ref();
    let aspects = Aspects::set(&state, default.is_some());
    steps.extend(calls(&state, default, aspects, kind, session));
    if let Some(time) = &state.time
        && kind.worker().is_some()
    {
        steps.push((
            "the frozen clock",
            evaluate(session, clock_script(Some(time.epoch_ms))),
        ));
    } else if let Some(time) = &state.time {
        let shared = shared.clone();
        let add = add_clock(session, Some(time.epoch_ms));
        let session_id = session.id().unwrap_or_default().to_owned();
        steps.push((
            "the frozen clock",
            Box::pin(async move {
                let identifier = add.await?;
                if let Some(target) = lock(&shared.state).targets.get_mut(&session_id) {
                    target.clock = Some(identifier);
                }
                Ok(())
            }),
        ));
    }
}

/// Sends what changed (`aspects`) to every target of `scope`
/// [`Feature::Emulation`] applies to. Returns the errors Chrome gave (an unknown time zone, say).
pub(super) async fn apply(
    shared: &Shared,
    aspects: Aspects,
    scope: Option<&str>,
    timeout: Duration,
) -> Vec<String> {
    let state = shared.settings.emulation.get_in(scope);
    let default = shared.default_viewport.as_ref();
    let targets: Vec<(String, Session, TargetKind, Option<String>)> = lock(&shared.state)
        .applies(Feature::Emulation)
        .filter(|(_, t)| t.scope.as_deref() == scope)
        .map(|(id, t)| {
            (
                id.clone(),
                t.session.with_timeout(timeout),
                t.kind,
                t.clock.clone(),
            )
        })
        .collect();
    let mut calls_sent: Vec<Step> = Vec::new();
    for (_, session, kind, _) in &targets {
        calls_sent.extend(
            calls(&state, default, aspects, *kind, session)
                .into_iter()
                .map(|(_, call)| call),
        );
    }
    let mut errors: Vec<String> = join_all(calls_sent)
        .await
        .into_iter()
        .filter_map(Result::err)
        .map(|e| e.to_string())
        .collect();
    if aspects.clock {
        let at = state.time.as_ref().map(|t| t.epoch_ms);
        let clocks = targets
            .into_iter()
            .map(|(id, session, kind, old)| async move {
                if kind.worker().is_some() {
                    // A worker has no documents to come: the script sets the
                    // time, or takes the clock out, where it runs.
                    let set = evaluate(&session, clock_script(at)).await;
                    return (id, set.map(|()| None));
                }
                (id, replace_clock(&session, old, at).await)
            });
        for (id, result) in join_all(clocks).await {
            match result {
                Ok(identifier) => {
                    if let Some(target) = lock(&shared.state).targets.get_mut(&id) {
                        target.clock = identifier;
                    }
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
    }
    for error in &errors {
        // Usually a target that closed meanwhile.
        tracing::debug!("cannot change a target's emulation: {error}");
    }
    errors
}

/// Takes the old clock script out and adds one for `at` (updating the
/// documents there are), or with `None` takes the clock out of them.
async fn replace_clock(
    session: &Session,
    old: Option<String>,
    at: Option<i64>,
) -> cdp::Result<Option<String>> {
    if let Some(identifier) = old {
        session
            .call(page::RemoveScriptToEvaluateOnNewDocumentParams::new(
                identifier,
            ))
            .await?;
    }
    let identifier = add_clock(session, at).await?;
    if at.is_some() {
        return Ok(Some(identifier));
    }
    // The script that took the clock out of the documents has done its
    // work.
    session
        .call(page::RemoveScriptToEvaluateOnNewDocumentParams::new(
            identifier,
        ))
        .await?;
    Ok(None)
}

/// Adds the clock script for `at`, running it in the documents there are
/// too; returns its id.
fn add_clock(
    session: &Session,
    at: Option<i64>,
) -> impl Future<Output = cdp::Result<String>> + Send + 'static {
    let mut params = page::AddScriptToEvaluateOnNewDocumentParams::new(clock_script(at));
    params.run_immediately = Some(true);
    let call = session.call(params);
    async move { Ok(call.await?.identifier) }
}

/// Runs `source` in `session` (a worker's), for its effect.
fn evaluate(session: &Session, source: String) -> Step {
    let mut params = runtime::EvaluateParams::new(source);
    params.silent = Some(true);
    step(session.call(params))
}

/// The calls that bring one session (of `kind`) in line with `state` for
/// `aspects` (the clock aside). Each sends the setting whether on or off.
fn calls(
    state: &Emulation,
    default: Option<&Viewport>,
    aspects: Aspects,
    kind: TargetKind,
    session: &Session,
) -> Vec<(&'static str, Step)> {
    let main = kind == TargetKind::Page;
    let worker = kind.worker().is_some();
    let mut calls: Vec<(&'static str, Step)> = Vec::new();
    if aspects.viewport && !worker {
        let viewport = state.viewport.as_ref().or(default);
        if main
            && default.is_some()
            && let Some(viewport) = viewport
        {
            calls.push(("growing the window", grow_window(session, viewport)));
        }
        if main {
            match viewport {
                Some(viewport) => calls.push((
                    "Emulation.setDeviceMetricsOverride",
                    step(session.call(metrics(viewport))),
                )),
                None => calls.push((
                    "Emulation.clearDeviceMetricsOverride",
                    step(session.call(ClearDeviceMetricsOverrideParams::default())),
                )),
            }
        }
        let touch = viewport.is_some_and(|v| v.touch);
        let mut params = SetTouchEmulationEnabledParams::new(touch);
        if touch {
            params.max_touch_points = Some(5);
        }
        calls.push((
            "Emulation.setTouchEmulationEnabled",
            step(session.call(params)),
        ));
    }
    if aspects.agent {
        // An empty user agent with no language takes the override out.
        let mut params =
            SetUserAgentOverrideParams::new(state.user_agent.clone().unwrap_or_default());
        params.accept_language.clone_from(&state.locale);
        calls.push(("Emulation.setUserAgentOverride", step(session.call(params))));
        if worker {
            let agent = agent_script(state.user_agent.as_deref());
            calls.push(("navigator.userAgent", evaluate(session, agent)));
        }
    }
    if aspects.locale {
        let params = SetLocaleOverrideParams {
            locale: state.locale.clone(),
        };
        let call = session.call(params);
        calls.push((
            "Emulation.setLocaleOverride",
            Box::pin(async move {
                match call.await {
                    // The locale is the renderer's: a worker that runs in
                    // its page's renderer has it from the page's session
                    // (`emulate` checks that it took).
                    Err(e) if worker && e.to_string().contains("Another locale override") => Ok(()),
                    result => result.map(drop),
                }
            }),
        ));
    }
    if aspects.timezone {
        let params = SetTimezoneOverrideParams::new(state.timezone.clone().unwrap_or_default());
        calls.push(("Emulation.setTimezoneOverride", step(session.call(params))));
    }
    if aspects.geolocation && !worker {
        match &state.geolocation {
            Some(geo) => {
                let params = SetGeolocationOverrideParams {
                    latitude: Some(geo.latitude),
                    longitude: Some(geo.longitude),
                    accuracy: Some(geo.accuracy),
                    ..SetGeolocationOverrideParams::default()
                };
                calls.push((
                    "Emulation.setGeolocationOverride",
                    step(session.call(params)),
                ));
            }
            None => calls.push((
                "Emulation.clearGeolocationOverride",
                step(session.call(ClearGeolocationOverrideParams::default())),
            )),
        }
    }
    if aspects.media && !worker {
        let media = &state.media;
        let feature = |name: &str, value: &Option<String>| MediaFeature {
            name: name.to_owned(),
            value: value.clone().unwrap_or_default(),
        };
        let params = SetEmulatedMediaParams {
            media: Some(media.r#type.clone().unwrap_or_default()),
            features: Some(vec![
                feature("prefers-color-scheme", &media.color_scheme),
                feature("prefers-reduced-motion", &media.reduced_motion),
                feature("forced-colors", &media.forced_colors),
            ]),
        };
        calls.push(("Emulation.setEmulatedMedia", step(session.call(params))));
    }
    if aspects.network && kind != TargetKind::DedicatedWorker {
        let throttle = state.throttle.as_ref();
        let bytes = |kbps: Option<f64>| kbps.map_or(-1.0, |k| k * 1000.0 / 8.0);
        let params = network::EmulateNetworkConditionsParams::new(
            state.offline,
            throttle.map_or(0.0, |t| t.latency_ms),
            bytes(throttle.and_then(|t| t.download_kbps)),
            bytes(throttle.and_then(|t| t.upload_kbps)),
        );
        calls.push((
            "Network.emulateNetworkConditions",
            step(session.call(params)),
        ));
    }
    if aspects.cpu && !worker {
        let params = SetCpuThrottlingRateParams::new(state.cpu_throttle.unwrap_or(1.0));
        calls.push(("Emulation.setCPUThrottlingRate", step(session.call(params))));
    }
    calls
}

/// Room around the viewport in a headless window: its made-up toolbar
/// and borders take 20x143 pixels in Chrome 154.
const WINDOW_MARGIN: (i64, i64) = (100, 250);

/// Makes a headless page's window at least as large as `viewport`.
/// Chrome sends input to out-of-process iframes and scrolls with the
/// wheel only within the window, not within the emulated viewport.
fn grow_window(session: &Session, viewport: &Viewport) -> Step {
    let get = session.call(browser::GetWindowForTargetParams { target_id: None });
    let session = session.clone();
    let want = (
        i64::from(viewport.width) + WINDOW_MARGIN.0,
        i64::from(viewport.height) + WINDOW_MARGIN.1,
    );
    Box::pin(async move {
        let window = get.await?;
        let (width, height) = (
            window.bounds.width.unwrap_or(0),
            window.bounds.height.unwrap_or(0),
        );
        if width >= want.0 && height >= want.1 {
            return Ok(());
        }
        let bounds = browser::Bounds {
            left: None,
            top: None,
            width: Some(width.max(want.0)),
            height: Some(height.max(want.1)),
            window_state: None,
        };
        session
            .call(browser::SetWindowBoundsParams::new(
                window.window_id,
                bounds,
            ))
            .await
            .map(drop)
    })
}

/// The viewport the page session `session` emulates: its context's, else
/// the default one.
fn page_viewport(shared: &Shared, session: &Session) -> Option<Viewport> {
    let scope = session.id().and_then(|id| {
        lock(&shared.state)
            .targets
            .get(id)
            .and_then(|t| t.scope.clone())
    });
    shared
        .settings
        .emulation
        .get_in(scope.as_deref())
        .viewport
        .or_else(|| shared.default_viewport.clone())
}

/// Makes the page's viewport `width` x `height` CSS pixels for a while
/// (`screenshot --full` of a page that scrolls inside a container), as
/// the viewport it emulates otherwise is (at `dpr` if it emulates none),
/// screen and orientation included.
pub(super) async fn stretch_viewport(
    shared: &Shared,
    session: &Session,
    (width, height): (u32, u32),
    dpr: f64,
) -> cdp::Result<()> {
    let mut params = match page_viewport(shared, session) {
        Some(viewport) => metrics(&viewport),
        None => SetDeviceMetricsOverrideParams::new(0, 0, dpr, false),
    };
    params.width = i64::from(width);
    params.height = i64::from(height);
    session.call(params).await.map(drop)
}

/// Gives the page back the viewport it emulates, or none, after
/// [`stretch_viewport`].
pub(super) async fn restore_viewport(shared: &Shared, session: &Session) -> cdp::Result<()> {
    match page_viewport(shared, session) {
        Some(viewport) => session.call(metrics(&viewport)).await.map(drop),
        None => session
            .call(ClearDeviceMetricsOverrideParams::default())
            .await
            .map(drop),
    }
}

/// `Emulation.setDeviceMetricsOverride` for `viewport`; a mobile screen
/// is turned the way its viewport is.
fn metrics(viewport: &Viewport) -> SetDeviceMetricsOverrideParams {
    let mut params = SetDeviceMetricsOverrideParams::new(
        i64::from(viewport.width),
        i64::from(viewport.height),
        viewport.dpr,
        viewport.mobile,
    );
    params.screen_width = Some(i64::from(viewport.screen_width.unwrap_or(viewport.width)));
    params.screen_height = Some(i64::from(viewport.screen_height.unwrap_or(viewport.height)));
    if viewport.mobile {
        let landscape = viewport.width > viewport.height;
        params.screen_orientation = Some(ScreenOrientation {
            r#type: if landscape {
                ScreenOrientationType::LandscapePrimary
            } else {
                ScreenOrientationType::PortraitPrimary
            },
            angle: if landscape { 90 } else { 0 },
        });
    }
    params
}

/// Gives a new browser the session's permissions (its contexts are new:
/// they have none).
pub(super) async fn apply_permissions(shared: &Shared, timeout: Duration) {
    let browser = shared.conn.browser().with_timeout(timeout);
    for permission in shared.settings.emulation.permissions(None) {
        let setting = match permission.setting.as_str() {
            "denied" => browser::PermissionSetting::Denied,
            _ => browser::PermissionSetting::Granted,
        };
        let params = permission_params(&permission.name, setting, permission.origin.clone(), None);
        if let Err(e) = browser.call(params).await {
            tracing::warn!("cannot set the {} permission: {e}", permission.name);
        }
    }
}

/// `Browser.setPermission` for a name `permissions` takes (`midi-sysex`
/// is `midi` with sysex), in context `scope` (Chrome's id of a context of
/// ours; the default context with `None`).
pub(crate) fn permission_params(
    name: &str,
    setting: browser::PermissionSetting,
    origin: Option<String>,
    scope: Option<&str>,
) -> browser::SetPermissionParams {
    let sysex = name == "midi-sysex";
    let mut params = browser::SetPermissionParams::new(
        browser::PermissionDescriptor {
            name: if sysex { "midi" } else { name }.to_owned(),
            sysex: sysex.then_some(true),
            user_visible_only: None,
            allow_without_sanitization: None,
            allow_without_gesture: None,
            pan_tilt_zoom: None,
        },
        setting,
    );
    params.origin = origin;
    params.browser_context_id = scope.map(str::to_owned);
    params
}

//! Emulation in page and iframe sessions (plan 07, milestone 7.2; the
//! settings are [`crate::emulate`]'s).
//!
//! A new session gets every setting that is on during its setup, while it
//! is paused ([`setup_steps`]); a change is sent to every session there is
//! ([`apply`]), only for what changed. The viewport is a page's (an
//! out-of-process iframe follows its page's); the rest is sent to iframe
//! sessions too, since each renderer has its own. The frozen clock is an
//! init script, and each session remembers its id to replace it.

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
use cdp::domains::{browser, network, page};
use futures_util::future::join_all;
use protocol::schema::{Emulation, Viewport};

use super::{Shared, Step, lock, step};
use crate::emulate::{Aspects, clock_script};

/// The setup steps of a new page (`main`) or iframe session.
pub(super) fn setup_steps(
    shared: &Arc<Shared>,
    session: &Session,
    main: bool,
    steps: &mut Vec<(&str, Step)>,
) {
    let scope = session.id().and_then(|id| {
        lock(&shared.state)
            .hosts
            .get(id)
            .and_then(|h| h.scope.clone())
    });
    let state = shared.settings.emulation.get_in(scope.as_deref());
    let default = shared.default_viewport.as_ref();
    let aspects = Aspects::set(&state, default.is_some());
    steps.extend(calls(&state, default, aspects, main, session));
    if let Some(time) = &state.time {
        let shared = shared.clone();
        let add = add_clock(session, Some(time.epoch_ms));
        let session_id = session.id().unwrap_or_default().to_owned();
        steps.push((
            "the frozen clock",
            Box::pin(async move {
                let identifier = add.await?;
                if let Some(host) = lock(&shared.state).hosts.get_mut(&session_id) {
                    host.clock = Some(identifier);
                }
                Ok(())
            }),
        ));
    }
}

/// Sends what changed (`aspects`) to every page and iframe session of
/// `scope`. Returns the errors Chrome gave (an unknown time zone, say).
pub(super) async fn apply(
    shared: &Shared,
    aspects: Aspects,
    scope: Option<&str>,
    timeout: Duration,
) -> Vec<String> {
    let state = shared.settings.emulation.get_in(scope);
    let default = shared.default_viewport.as_ref();
    let hosts: Vec<(String, Session, bool, Option<String>)> = lock(&shared.state)
        .hosts
        .iter()
        .filter(|(_, h)| h.scope.as_deref() == scope)
        .map(|(id, h)| {
            (
                id.clone(),
                h.session.with_timeout(timeout),
                h.main,
                h.clock.clone(),
            )
        })
        .collect();
    let mut calls_sent: Vec<Step> = Vec::new();
    for (_, session, main, _) in &hosts {
        calls_sent.extend(
            calls(&state, default, aspects, *main, session)
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
        let clocks = hosts.into_iter().map(|(id, session, _, old)| async move {
            (id, replace_clock(&session, old, at).await)
        });
        for (id, result) in join_all(clocks).await {
            match result {
                Ok(identifier) => {
                    if let Some(host) = lock(&shared.state).hosts.get_mut(&id) {
                        host.clock = identifier;
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

/// The calls that bring one session in line with `state` for `aspects`
/// (the clock aside). Each sends the setting whether on or off.
fn calls(
    state: &Emulation,
    default: Option<&Viewport>,
    aspects: Aspects,
    main: bool,
    session: &Session,
) -> Vec<(&'static str, Step)> {
    let mut calls: Vec<(&'static str, Step)> = Vec::new();
    if aspects.viewport {
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
    }
    if aspects.locale {
        let params = SetLocaleOverrideParams {
            locale: state.locale.clone(),
        };
        calls.push(("Emulation.setLocaleOverride", step(session.call(params))));
    }
    if aspects.timezone {
        let params = SetTimezoneOverrideParams::new(state.timezone.clone().unwrap_or_default());
        calls.push(("Emulation.setTimezoneOverride", step(session.call(params))));
    }
    if aspects.geolocation {
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
    if aspects.media {
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
    if aspects.network {
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
    if aspects.cpu {
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

/// Gives a new browser the session's permissions.
pub(super) async fn apply_permissions(shared: &Shared, timeout: Duration) {
    let browser = shared.conn.browser().with_timeout(timeout);
    for permission in shared.settings.emulation.permissions() {
        let setting = match permission.setting.as_str() {
            "denied" => browser::PermissionSetting::Denied,
            _ => browser::PermissionSetting::Granted,
        };
        let params = permission_params(&permission.name, setting, permission.origin.clone());
        if let Err(e) = browser.call(params).await {
            tracing::warn!("cannot set the {} permission: {e}", permission.name);
        }
    }
}

/// `Browser.setPermission` for a name `permissions` takes (`midi-sysex`
/// is `midi` with sysex).
pub(crate) fn permission_params(
    name: &str,
    setting: browser::PermissionSetting,
    origin: Option<String>,
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
    params
}

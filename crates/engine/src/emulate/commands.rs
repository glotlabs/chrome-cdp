//! `emulate`, `viewport`, `geo`, `tz`, `locale`, `ua`, `media`, `offline`,
//! `throttle`, `cpu-throttle`, `time` and `permissions` (plan 07,
//! milestone 7.2).
//!
//! The settings belong to the session, not to a tab: none of these
//! commands takes a tab lock. Without a browser they are only recorded,
//! and a browser started later gets them.

use cdp::domains::browser::{PermissionSetting, ResetPermissionsParams};
use std::fmt::Write as _;

use cdp::domains::runtime;
use protocol::schema::{
    DeviceInfo, EmulateData, Emulation, FrozenTime, Geolocation, PermissionInfo, PermissionsData,
    Throttle, Viewport,
};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output};

use super::{Aspects, DEVICES, check_permission, device_viewport, find_device};
use crate::errors::from_cdp;
use crate::observe::iso8601;
use crate::request::{EmulateRequest, EmulationChange, PermissionsOp, PermissionsRequest, millis};
use crate::tabs::emulation::permission_params;
use crate::{Engine, Running, State};

impl Engine {
    /// The running browser, if there is one.
    fn running_now(&self) -> Option<std::sync::Arc<Running>> {
        match &*self.lock_state() {
            State::Running(running) => Some(running.clone()),
            State::Crashed(_) | State::Empty => None,
        }
    }

    /// The viewport pages get without their own: the running browser's,
    /// else what a headless browser would give.
    fn default_viewport(&self) -> Option<Viewport> {
        match self.running_now() {
            Some(running) => running.tabs.default_viewport(),
            None => Some(super::default_viewport()),
        }
    }

    /// Changes what the session emulates (or shows it) and sends the
    /// change to every page and frame.
    pub async fn emulate(&self, req: EmulateRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let emulator = &self.settings.emulation;
        let scope = self.context_key()?;
        let scope = scope.as_deref();
        let _changing = emulator.changing.lock().await;
        let before = emulator.get_in(scope);
        let mut after = before.clone();
        change(&mut after, &req.change, &self.session)?;
        emulator.set_in(scope, after.clone());
        let geolocation = matches!(
            req.change,
            EmulationChange::Geolocation {
                geolocation: Some(_)
            }
        );
        if geolocation {
            emulator.record_permission("geolocation", "granted", None);
        }
        let aspects = Aspects::changed(&before, &after);
        if let Some(running) = self.running_now() {
            if let Some(check) = intl_check(&req.change)
                && !self.passes(&running, scope, &check, timeout).await
            {
                emulator.set_in(scope, before);
                return Err(self.refused(&req.change, "Invalid"));
            }
            if aspects.any() {
                let errors = running.tabs.apply_emulation(aspects, scope, timeout).await;
                if let Some(refused) = errors.iter().find(|e| e.contains("Invalid")) {
                    emulator.set_in(scope, before);
                    running.tabs.apply_emulation(aspects, scope, timeout).await;
                    return Err(self.refused(&req.change, refused));
                }
            }
            if geolocation {
                let params = permission_params("geolocation", PermissionSetting::Granted, None);
                running
                    .conn
                    .browser()
                    .with_timeout(timeout)
                    .call(params)
                    .await
                    .map_err(|e| from_cdp(&e, &self.session))?;
            }
        }
        let default_viewport = self.default_viewport();
        let text = describe_change(&req.change, &after, default_viewport.as_ref());
        let data = EmulateData {
            emulation: after,
            default_viewport,
            devices: None,
        };
        Ok(Output::typed(&data, text))
    }

    /// Whether `expression` is true in the active tab; `true` when it
    /// cannot tell.
    async fn passes(
        &self,
        running: &Running,
        scope: Option<&str>,
        expression: &str,
        timeout: std::time::Duration,
    ) -> bool {
        let page = match scope {
            Some(scope) => running.tabs.page_in(scope, None, timeout).await,
            None => running.tabs.page(None, timeout).await,
        };
        let Ok(page) = page else {
            return true;
        };
        let mut params = runtime::EvaluateParams::new(expression.to_owned());
        params.return_by_value = Some(true);
        match page.session.with_timeout(timeout).call(params).await {
            Ok(result) => result.result.value != Some(serde_json::Value::Bool(false)),
            Err(_) => true,
        }
    }

    /// The error for a setting Chrome refused (an unknown time zone or
    /// locale).
    fn refused(&self, change: &EmulationChange, error: &str) -> ErrorInfo {
        let reason = error
            .rsplit(": ")
            .next()
            .unwrap_or(error)
            .split(" (code")
            .next()
            .unwrap_or_default();
        let (what, value, example) = match change {
            EmulationChange::Timezone { timezone } => (
                "time zone",
                timezone.clone().unwrap_or_default(),
                Cmd::new("tz").arg("Europe/Berlin"),
            ),
            EmulationChange::Locale { locale } => (
                "locale",
                locale.clone().unwrap_or_default(),
                Cmd::new("locale").arg("de-DE"),
            ),
            _ => ("setting", String::new(), Cmd::new("emulate")),
        };
        let message = if reason == "Invalid" {
            format!("Chrome does not know the {what} `{value}`")
        } else {
            format!("Chrome does not know the {what} `{value}`: {reason}")
        };
        ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::run(example.session(&self.session)))
    }

    /// `permissions`: lists, grants, denies or resets permissions.
    pub async fn permissions(&self, req: PermissionsRequest) -> Result<Output, ErrorInfo> {
        let timeout = millis(req.timeout_ms);
        let emulator = &self.settings.emulation;
        let _changing = emulator.changing.lock().await;
        let names = req
            .names
            .iter()
            .map(|n| check_permission(n, req.op, &self.session))
            .collect::<Result<Vec<String>, _>>()?;
        let origin = req.origin.as_deref().map(|o| o.trim_end_matches('/'));
        let running = self.running_now();
        let browser = running
            .as_ref()
            .map(|r| r.conn.browser().with_timeout(timeout));
        let whom = origin.map_or_else(|| "every origin".to_owned(), str::to_owned);
        let text = match req.op {
            PermissionsOp::List => list_text(&emulator.permissions(), &self.session),
            PermissionsOp::Grant | PermissionsOp::Deny => {
                if names.is_empty() {
                    return Err(ErrorInfo::new(ErrorKind::BadArgs, "name a permission")
                        .note("permissions", super::PERMISSIONS.join(", "))
                        .hint(Hint::run(
                            Cmd::new("permissions")
                                .arg("grant")
                                .arg("geolocation")
                                .session(&self.session),
                        )));
                }
                let (setting, word, verb) = if req.op == PermissionsOp::Grant {
                    (PermissionSetting::Granted, "granted", "granted")
                } else {
                    (PermissionSetting::Denied, "denied", "denied")
                };
                for name in &names {
                    if let Some(browser) = &browser {
                        let params = permission_params(name, setting, origin.map(str::to_owned));
                        browser
                            .call(params)
                            .await
                            .map_err(|e| self.bad_permission(name, origin, &e))?;
                    }
                    emulator.record_permission(name, word, origin);
                }
                format!("{verb} {} for {whom}", names.join(", "))
            }
            PermissionsOp::Reset => {
                if names.is_empty() && origin.is_none() {
                    if let Some(browser) = &browser {
                        browser
                            .call(ResetPermissionsParams::default())
                            .await
                            .map_err(|e| from_cdp(&e, &self.session))?;
                    }
                    emulator.forget_permissions(|_| true);
                    "every permission is back to asking".to_owned()
                } else {
                    let gone = emulator.forget_permissions(|p| {
                        (names.is_empty() || names.contains(&p.name))
                            && (origin.is_none() || p.origin.as_deref() == origin)
                    });
                    let mut reset: Vec<(String, Option<String>)> = names
                        .iter()
                        .map(|n| (n.clone(), origin.map(str::to_owned)))
                        .collect();
                    for p in gone {
                        let key = (p.name, p.origin);
                        if !reset.contains(&key) {
                            reset.push(key);
                        }
                    }
                    if let Some(browser) = &browser {
                        for (name, origin) in &reset {
                            let params =
                                permission_params(name, PermissionSetting::Prompt, origin.clone());
                            browser
                                .call(params)
                                .await
                                .map_err(|e| self.bad_permission(name, origin.as_deref(), &e))?;
                        }
                    }
                    let mut shown: Vec<&str> = reset.iter().map(|(n, _)| n.as_str()).collect();
                    shown.dedup();
                    if shown.is_empty() {
                        format!("no permissions set for {whom}")
                    } else {
                        format!("{} back to asking for {whom}", shown.join(", "))
                    }
                }
            }
        };
        let data = PermissionsData {
            permissions: emulator.permissions(),
        };
        Ok(Output::typed(&data, text))
    }

    fn bad_permission(&self, name: &str, origin: Option<&str>, e: &cdp::CdpError) -> ErrorInfo {
        let message = e.to_string();
        if origin.is_some() && message.to_ascii_lowercase().contains("origin") {
            return ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{}` is not an origin", origin.unwrap_or_default()),
            )
            .hint(Hint::run(
                Cmd::new("permissions")
                    .arg("grant")
                    .arg(name)
                    .flag("origin", "https://example.com")
                    .session(&self.session),
            ));
        }
        from_cdp(e, &self.session)
    }
}

/// A check in the page for a time zone or locale: Chrome refuses a bad
/// one only while no other is set, and keeps the old one silently
/// otherwise.
fn intl_check(change: &EmulationChange) -> Option<String> {
    let quoted = |s: &str| serde_json::Value::String(s.to_owned()).to_string();
    match change {
        EmulationChange::Timezone {
            timezone: Some(timezone),
        } => Some(format!(
            "(() => {{ try {{ new Intl.DateTimeFormat('en-US', {{ timeZone: {} }}); return true; }} catch {{ return false; }} }})()",
            quoted(timezone)
        )),
        EmulationChange::Locale {
            locale: Some(locale),
        } => Some(format!(
            "(() => {{ try {{ return Intl.getCanonicalLocales({}).length === 1; }} catch {{ return false; }} }})()",
            quoted(&locale.replace('_', "-"))
        )),
        _ => None,
    }
}

/// Applies `change` to `state`.
fn change(state: &mut Emulation, change: &EmulationChange, session: &str) -> Result<(), ErrorInfo> {
    match change {
        EmulationChange::Show => {}
        EmulationChange::Reset => *state = Emulation::default(),
        EmulationChange::Device { name } => {
            let device = find_device(name, session)?;
            state.device = Some(device.name.to_owned());
            state.viewport = Some(device_viewport(device));
            state.user_agent = Some(device.user_agent.to_owned());
        }
        EmulationChange::Viewport { viewport } => {
            state.device = None;
            state.viewport.clone_from(viewport);
        }
        EmulationChange::Geolocation { geolocation } => state.geolocation.clone_from(geolocation),
        EmulationChange::Timezone { timezone } => state.timezone.clone_from(timezone),
        EmulationChange::Locale { locale } => state.locale.clone_from(locale),
        EmulationChange::UserAgent { user_agent } => {
            state.device = None;
            state.user_agent.clone_from(user_agent);
        }
        EmulationChange::Media { media } => match media {
            None => state.media = protocol::schema::Media::default(),
            Some(media) => {
                let current = &mut state.media;
                for (from, to) in [
                    (&media.r#type, &mut current.r#type),
                    (&media.color_scheme, &mut current.color_scheme),
                    (&media.reduced_motion, &mut current.reduced_motion),
                    (&media.forced_colors, &mut current.forced_colors),
                ] {
                    if from.is_some() {
                        to.clone_from(from);
                    }
                }
            }
        },
        EmulationChange::Offline { offline } => state.offline = *offline,
        EmulationChange::Throttle { throttle } => state.throttle.clone_from(throttle),
        EmulationChange::CpuThrottle { rate } => {
            state.cpu_throttle = rate.filter(|&r| r > 1.0);
        }
        EmulationChange::TimeFreeze { epoch_ms } => state.time = Some(frozen(*epoch_ms)),
        EmulationChange::TimeAdvance { ms } => {
            let Some(time) = &state.time else {
                return Err(ErrorInfo::new(
                    ErrorKind::BadArgs,
                    "the time is not frozen, so it cannot advance",
                )
                .hint(
                    Hint::run(Cmd::new("time").arg("freeze").session(session))
                        .why("freezes it at the current time"),
                ));
            };
            let ms = i64::try_from(*ms).unwrap_or(i64::MAX);
            state.time = Some(frozen(time.epoch_ms.saturating_add(ms)));
        }
        EmulationChange::TimeReset => state.time = None,
    }
    Ok(())
}

fn frozen(epoch_ms: i64) -> FrozenTime {
    FrozenTime {
        epoch_ms,
        iso: iso8601(u64::try_from(epoch_ms).unwrap_or(0)),
    }
}

/// The text of a change: the lines of the settings it changed.
fn describe_change(
    change: &EmulationChange,
    state: &Emulation,
    default: Option<&Viewport>,
) -> String {
    let lines = match change {
        EmulationChange::Show => {
            let lines = describe(state, default);
            if lines.len() > 1 {
                return lines.join("\n");
            }
            let mut text = "nothing emulated".to_owned();
            if default.is_some() {
                text.push_str("; ");
                text.push_str(&viewport_line(None, default));
            }
            return text;
        }
        EmulationChange::Reset => {
            let mut text = "emulation reset".to_owned();
            if default.is_some() {
                text.push_str("; ");
                text.push_str(&viewport_line(None, default));
            }
            return text;
        }
        EmulationChange::Device { .. } => vec![
            format!("device {}", state.device.clone().unwrap_or_default()),
            viewport_line(state.viewport.as_ref(), default),
            agent_line(state),
        ],
        EmulationChange::Viewport { .. } => vec![viewport_line(state.viewport.as_ref(), default)],
        EmulationChange::Geolocation { .. } => vec![geo_line(state)],
        EmulationChange::Timezone { .. } => vec![timezone_line(state)],
        EmulationChange::Locale { .. } => vec![locale_line(state)],
        EmulationChange::UserAgent { .. } => vec![agent_line(state)],
        EmulationChange::Media { .. } => vec![media_line(state)],
        EmulationChange::Offline { .. } => vec![offline_line(state)],
        EmulationChange::Throttle { .. } => vec![throttle_line(state)],
        EmulationChange::CpuThrottle { .. } => vec![cpu_line(state)],
        EmulationChange::TimeFreeze { .. }
        | EmulationChange::TimeAdvance { .. }
        | EmulationChange::TimeReset => vec![time_line(state)],
    };
    lines.join("\n")
}

/// A line per setting that is on; the viewport always.
fn describe(state: &Emulation, default: Option<&Viewport>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(device) = &state.device {
        lines.push(format!("device {device}"));
    }
    lines.push(viewport_line(state.viewport.as_ref(), default));
    if state.user_agent.is_some() {
        lines.push(agent_line(state));
    }
    if state.locale.is_some() {
        lines.push(locale_line(state));
    }
    if state.timezone.is_some() {
        lines.push(timezone_line(state));
    }
    if state.geolocation.is_some() {
        lines.push(geo_line(state));
    }
    if !state.media.is_empty() {
        lines.push(media_line(state));
    }
    if state.offline {
        lines.push(offline_line(state));
    }
    if state.throttle.is_some() {
        lines.push(throttle_line(state));
    }
    if state.cpu_throttle.is_some() {
        lines.push(cpu_line(state));
    }
    if state.time.is_some() {
        lines.push(time_line(state));
    }
    lines
}

/// `viewport 390x664 @3x, mobile, touch, screen 390x844`.
fn viewport_line(viewport: Option<&Viewport>, default: Option<&Viewport>) -> String {
    let Some(v) = viewport.or(default) else {
        return "viewport: the window's".to_owned();
    };
    let mut line = format!("viewport {}x{}", v.width, v.height);
    if (v.dpr - 1.0).abs() > f64::EPSILON {
        let _ = write!(line, " @{}x", v.dpr);
    }
    if v.mobile {
        line.push_str(", mobile");
    }
    if v.touch {
        line.push_str(", touch");
    }
    if let (Some(w), Some(h)) = (v.screen_width, v.screen_height) {
        let _ = write!(line, ", screen {w}x{h}");
    }
    if viewport.is_none() {
        line.push_str(" (the headless default)");
    }
    line
}

fn agent_line(state: &Emulation) -> String {
    match &state.user_agent {
        Some(ua) => format!("user agent {ua}"),
        None => "user agent: the browser's own".to_owned(),
    }
}

fn locale_line(state: &Emulation) -> String {
    match &state.locale {
        Some(locale) => format!("locale {locale}"),
        None => "locale: the browser's own".to_owned(),
    }
}

fn timezone_line(state: &Emulation) -> String {
    match &state.timezone {
        Some(tz) => format!("timezone {tz}"),
        None => "timezone: the browser's own".to_owned(),
    }
}

/// `geolocation 59.9139,10.7522 (accuracy 10m)`.
fn geo_line(state: &Emulation) -> String {
    match &state.geolocation {
        Some(Geolocation {
            latitude,
            longitude,
            accuracy,
        }) => format!("geolocation {latitude},{longitude} (accuracy {accuracy}m)"),
        None => "geolocation: not emulated".to_owned(),
    }
}

/// `media print, prefers-color-scheme dark`.
fn media_line(state: &Emulation) -> String {
    let media = &state.media;
    let mut parts = Vec::new();
    if let Some(kind) = &media.r#type {
        parts.push(kind.clone());
    }
    for (name, value) in [
        ("prefers-color-scheme", &media.color_scheme),
        ("prefers-reduced-motion", &media.reduced_motion),
        ("forced-colors", &media.forced_colors),
    ] {
        if let Some(value) = value {
            parts.push(format!("{name} {value}"));
        }
    }
    if parts.is_empty() {
        "media: the browser's own".to_owned()
    } else {
        format!("media {}", parts.join(", "))
    }
}

fn offline_line(state: &Emulation) -> String {
    if state.offline {
        "offline: requests fail and navigator.onLine is false".to_owned()
    } else {
        "online".to_owned()
    }
}

/// `throttle slow-3g: latency 2s, 400kbit/s down, 400kbit/s up`.
fn throttle_line(state: &Emulation) -> String {
    let Some(Throttle {
        preset,
        latency_ms,
        download_kbps,
        upload_kbps,
    }) = &state.throttle
    else {
        return "throttle: off".to_owned();
    };
    let rate =
        |kbps: &Option<f64>| kbps.map_or_else(|| "no limit".to_owned(), |k| format!("{k}kbit/s"));
    let latency = std::time::Duration::from_secs_f64(latency_ms.max(0.0) / 1000.0);
    format!(
        "throttle{}: latency {}, {} down, {} up",
        preset.as_ref().map(|p| format!(" {p}")).unwrap_or_default(),
        protocol::format::duration(latency),
        rate(download_kbps),
        rate(upload_kbps)
    )
}

fn cpu_line(state: &Emulation) -> String {
    match state.cpu_throttle {
        Some(rate) => format!("cpu-throttle: {rate}x slower"),
        None => "cpu-throttle: full speed".to_owned(),
    }
}

fn time_line(state: &Emulation) -> String {
    match &state.time {
        Some(time) => format!("time frozen at {}", time.iso),
        None => "time: real".to_owned(),
    }
}

/// `permissions`' list.
fn list_text(permissions: &[PermissionInfo], session: &str) -> String {
    if permissions.is_empty() {
        return format!(
            "no permissions set; {} sets one",
            Cmd::new("permissions")
                .arg("grant")
                .raw("<name>")
                .session(session)
        );
    }
    let rows: Vec<Vec<String>> = permissions
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                p.setting.clone(),
                p.origin
                    .clone()
                    .unwrap_or_else(|| "every origin".to_owned()),
            ]
        })
        .collect();
    protocol::format::table(&rows)
}

/// `emulate --list`: the device table.
pub fn device_list() -> Output {
    let devices: Vec<DeviceInfo> = DEVICES
        .iter()
        .map(|d| DeviceInfo {
            name: d.name.to_owned(),
            viewport: device_viewport(d),
            user_agent: d.user_agent.to_owned(),
        })
        .collect();
    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|d| {
            let v = &d.viewport;
            let mut kind = if v.mobile { "mobile" } else { "desktop" }.to_owned();
            if v.touch {
                kind.push_str(", touch");
            }
            vec![
                d.name.clone(),
                format!("{}x{}", v.width, v.height),
                format!("@{}x", v.dpr),
                kind,
            ]
        })
        .collect();
    let quiet: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
    let quiet = quiet.join("\n");
    let data = EmulateData {
        emulation: Emulation::default(),
        default_viewport: None,
        devices: Some(devices),
    };
    Output::typed(&data, protocol::format::table(&rows)).quiet(quiet)
}

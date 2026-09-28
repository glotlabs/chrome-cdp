//! Emulation (plan 07, milestone 7.2): devices, viewport, geolocation,
//! time zone, locale, user agent, CSS media, network and CPU throttling,
//! permissions, and a frozen clock.
//!
//! Like routes, every setting belongs to the session ([`Emulator`], in the
//! tab manager's settings, so it outlives a browser) and is sticky: a
//! change applies to every page and iframe session now, and each new one
//! (a tab, a popup, an out-of-process iframe) gets the settings while it
//! is paused, before its first script runs (see `tabs::emulation`).
//! Permissions are the browser's, so a new browser gets them when it
//! starts.
//!
//! In headless mode a page without a `viewport` gets [`default_viewport`]
//! (1280x800): the window of headless Chrome is an odd 780x437 otherwise.
//! Headed and connected browsers keep their window's size.
//!
//! `time freeze` does not use Chrome's virtual time: pausing it stops
//! timers and page loads, and letting it advance fires timers at once.
//! An init script replaces `Date` instead ([`clock_script`]); moving the
//! frozen time on also runs the page's timers due by then (plan 11.6).

pub(crate) mod commands;
mod devices;

use std::sync::{Mutex, MutexGuard, PoisonError};

use protocol::schema::{Emulation, PermissionInfo, Throttle, Viewport};
use protocol::suggest::{self, squash};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use crate::request::PermissionsOp;

pub use devices::{DEVICES, Device};

/// The viewport of a headless page without its own.
pub fn default_viewport() -> Viewport {
    Viewport {
        width: 1280,
        height: 800,
        dpr: 1.0,
        mobile: false,
        touch: false,
        screen_width: None,
        screen_height: None,
    }
}

/// A device's viewport.
pub fn device_viewport(device: &Device) -> Viewport {
    let differs = (device.screen_width, device.screen_height) != (device.width, device.height);
    Viewport {
        width: device.width,
        height: device.height,
        dpr: device.dpr,
        mobile: device.mobile,
        touch: device.touch,
        screen_width: differs.then_some(device.screen_width),
        screen_height: differs.then_some(device.screen_height),
    }
}

/// The device called `name` (case, spaces and punctuation do not count:
/// `iphone-13` is `iPhone 13`), or an error with the closest names.
pub fn find_device(name: &str, session: &str) -> Result<&'static Device, ErrorInfo> {
    let wanted = squash(name);
    if let Some(device) = DEVICES.iter().find(|d| squash(d.name) == wanted) {
        return Ok(device);
    }
    let words: Vec<String> = name.split_whitespace().map(squash).collect();
    let mut close: Vec<&str> = DEVICES
        .iter()
        .filter(|d| {
            let squashed = squash(d.name);
            !words.is_empty() && words.iter().all(|w| squashed.contains(w.as_str()))
        })
        .map(|d| d.name)
        .filter(|n| !n.ends_with(" landscape"))
        .take(3)
        .collect();
    if close.is_empty() {
        close = suggest::similar(name, DEVICES.iter().map(|d| d.name), 3);
    }
    let mut info = ErrorInfo::new(ErrorKind::BadArgs, format!("no device is called `{name}`"));
    if !close.is_empty() {
        info = info.note("did you mean", close.join(", "));
    }
    let hint = match close.first() {
        Some(first) => Hint::run(Cmd::new("emulate").arg(first).session(session)),
        None => {
            Hint::run(Cmd::new("emulate").switch("list").session(session)).why("lists the devices")
        }
    };
    Err(info.hint(hint))
}

/// `throttle`'s presets, as Chrome DevTools has them: latency in ms,
/// download and upload in kbit/s.
const THROTTLE_PRESETS: &[(&str, f64, f64, f64)] = &[
    ("slow-3g", 2000.0, 400.0, 400.0),
    ("fast-3g", 562.5, 1440.0, 675.0),
    ("4g", 165.0, 8100.0, 1350.0),
];

/// The names of `throttle`'s presets.
pub fn throttle_presets() -> impl Iterator<Item = &'static str> {
    THROTTLE_PRESETS.iter().map(|p| p.0)
}

/// The preset called `name` (`slow-3g`, `fast-3g`, `4g`).
pub fn throttle_preset(name: &str) -> Option<Throttle> {
    let name = name.to_ascii_lowercase();
    THROTTLE_PRESETS
        .iter()
        .find(|p| p.0 == name)
        .map(|&(preset, latency, down, up)| Throttle {
            preset: Some(preset.to_owned()),
            latency_ms: latency,
            download_kbps: Some(down),
            upload_kbps: Some(up),
        })
}

/// The permissions `permissions` takes, as the Permissions API names them.
pub const PERMISSIONS: &[&str] = &[
    "geolocation",
    "notifications",
    "clipboard-read",
    "clipboard-write",
    "camera",
    "microphone",
    "midi",
    "midi-sysex",
    "background-sync",
    "persistent-storage",
    "screen-wake-lock",
    "local-fonts",
    "storage-access",
    "idle-detection",
    "window-management",
    "accelerometer",
    "gyroscope",
    "magnetometer",
    "payment-handler",
];

/// Checks a permission name (any case) for `permissions <op>`.
pub fn check_permission(name: &str, op: PermissionsOp, session: &str) -> Result<String, ErrorInfo> {
    let lower = name.trim().to_ascii_lowercase();
    if PERMISSIONS.contains(&lower.as_str()) {
        return Ok(lower);
    }
    let closest = suggest::similar(&lower, PERMISSIONS.iter().copied(), 1)
        .into_iter()
        .next();
    let mut info = ErrorInfo::new(
        ErrorKind::BadArgs,
        format!("`{name}` is not a permission chrome-cdp knows"),
    )
    .note("permissions", PERMISSIONS.join(", "));
    if let Some(closest) = closest {
        info = info.hint(Hint::run(
            Cmd::new("permissions")
                .arg(op.name())
                .arg(closest)
                .session(session),
        ));
    }
    Err(info)
}

/// Parses `time freeze`'s time: ISO 8601 (`2026-01-31`,
/// `2026-01-31T09:30`, `2026-01-31T09:30:00.5+01:00`; UTC without an
/// offset) or ms since the Unix epoch. Returns ms since the epoch.
pub fn parse_time(text: &str) -> Result<i64, String> {
    let text = text.trim();
    let bad =
        || format!("`{text}` is not a time; use ISO 8601 (2026-01-31T09:30:00Z) or ms since 1970");
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse().map_err(|_| bad());
    }
    let (date, rest) = text.split_at(text.find(['T', 't', ' ']).unwrap_or(text.len()));
    let mut parts = date.splitn(3, '-');
    let mut field = |len: usize| -> Option<i64> {
        let part = parts.next()?;
        (part.len() == len && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day)) = (field(4), field(2), field(2)) else {
        return Err(bad());
    };
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return Err(bad());
    }
    let mut ms = days_from_civil(year, month, day) * 86_400_000;
    let rest = rest.get(1..).unwrap_or_default();
    if !rest.is_empty() {
        let offset_at = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
        let (clock, zone) = rest.split_at(offset_at);
        ms += clock_ms(clock).ok_or_else(bad)?;
        ms -= offset_ms(zone).ok_or_else(bad)?;
    }
    if ms < 0 {
        return Err(format!("`{text}` is before 1970"));
    }
    Ok(ms)
}

/// `09:30`, `09:30:15` or `09:30:15.250` in ms since midnight.
fn clock_ms(clock: &str) -> Option<i64> {
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let fields: Vec<&str> = hms.split(':').collect();
    if !(2..=3).contains(&fields.len())
        || fields
            .iter()
            .any(|f| f.len() != 2 || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let number = |f: &str| f.parse::<i64>().ok();
    let (h, m) = (number(fields[0])?, number(fields[1])?);
    let s = fields.get(2).map_or(Some(0), |f| number(f))?;
    if h > 23 || m > 59 || s > 59 {
        return None;
    }
    let mut ms = ((h * 60 + m) * 60 + s) * 1000;
    if !fraction.is_empty() {
        if !fraction.bytes().all(|b| b.is_ascii_digit()) || fields.len() != 3 {
            return None;
        }
        let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
        ms += number(&digits)?;
    }
    Some(ms)
}

/// `Z`, `+01:00`, `-0530` or nothing, in ms east of UTC.
fn offset_ms(zone: &str) -> Option<i64> {
    match zone {
        "" | "Z" | "z" => return Some(0),
        _ => {}
    }
    let sign = match zone.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = zone[1..].chars().filter(|&c| c != ':').collect();
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i64, i64) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    (h <= 23 && m <= 59).then_some(sign * (h * 60 + m) * 60_000)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The frozen clock's init script for time `at` (ms since the epoch), or
/// with `None` the script that takes it out of a document.
pub(crate) fn clock_script(at: Option<i64>) -> String {
    let at = at.map_or_else(|| "null".to_owned(), |ms| ms.to_string());
    format!("{}({at});", include_str!("clock.js").trim_end())
}

/// What a change touches, so only those calls are sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Aspects {
    /// The viewport and touch.
    pub(crate) viewport: bool,
    /// The user agent and `Accept-Language` (the locale's).
    pub(crate) agent: bool,
    pub(crate) locale: bool,
    pub(crate) timezone: bool,
    pub(crate) geolocation: bool,
    pub(crate) media: bool,
    /// Offline and throttling.
    pub(crate) network: bool,
    pub(crate) cpu: bool,
    pub(crate) clock: bool,
}

impl Aspects {
    /// What differs between two states.
    pub(crate) fn changed(before: &Emulation, after: &Emulation) -> Self {
        Self {
            viewport: before.viewport != after.viewport,
            agent: before.user_agent != after.user_agent || before.locale != after.locale,
            locale: before.locale != after.locale,
            timezone: before.timezone != after.timezone,
            geolocation: before.geolocation != after.geolocation,
            media: before.media != after.media,
            network: before.offline != after.offline || before.throttle != after.throttle,
            cpu: before.cpu_throttle != after.cpu_throttle,
            clock: before.time != after.time,
        }
    }

    /// What a new page or frame session needs: what is set, and the
    /// viewport if there is a default one.
    pub(crate) fn set(state: &Emulation, default_viewport: bool) -> Self {
        let mut aspects = Self::changed(&Emulation::default(), state);
        aspects.viewport |= default_viewport;
        aspects
    }

    pub(crate) fn any(self) -> bool {
        self != Self::default()
    }
}

/// The session's emulation settings and permissions.
#[derive(Debug, Default)]
pub(crate) struct Emulator {
    state: Mutex<Emulation>,
    /// Each browser context of ours has its own (plan 11.4), starting
    /// from nothing emulated; by scope (see `crate::context`).
    scoped: Mutex<std::collections::HashMap<String, Emulation>>,
    permissions: Mutex<Vec<PermissionInfo>>,
    /// Serialises the commands that change them.
    pub(crate) changing: tokio::sync::Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Emulator {
    pub(crate) fn get(&self) -> Emulation {
        lock(&self.state).clone()
    }

    pub(crate) fn set(&self, state: Emulation) {
        *lock(&self.state) = state;
    }

    /// What `scope` emulates: the session's with `None`.
    pub(crate) fn get_in(&self, scope: Option<&str>) -> Emulation {
        match scope {
            None => self.get(),
            Some(scope) => lock(&self.scoped).get(scope).cloned().unwrap_or_default(),
        }
    }

    pub(crate) fn set_in(&self, scope: Option<&str>, state: Emulation) {
        match scope {
            None => self.set(state),
            Some(scope) => {
                lock(&self.scoped).insert(scope.to_owned(), state);
            }
        }
    }

    /// Forgets what `scope`, a context that closed, emulated.
    pub(crate) fn drop_scope(&self, scope: &str) {
        lock(&self.scoped).remove(scope);
    }

    pub(crate) fn permissions(&self) -> Vec<PermissionInfo> {
        lock(&self.permissions).clone()
    }

    /// Records `name` as `setting` for `origin` (replacing what was set
    /// for the same name and origin).
    pub(crate) fn record_permission(&self, name: &str, setting: &str, origin: Option<&str>) {
        let mut permissions = lock(&self.permissions);
        permissions.retain(|p| !(p.name == name && p.origin.as_deref() == origin));
        permissions.push(PermissionInfo {
            name: name.to_owned(),
            setting: setting.to_owned(),
            origin: origin.map(str::to_owned),
        });
    }

    /// Forgets the permissions `forget` accepts; returns them.
    pub(crate) fn forget_permissions(
        &self,
        mut forget: impl FnMut(&PermissionInfo) -> bool,
    ) -> Vec<PermissionInfo> {
        let mut permissions = lock(&self.permissions);
        let (gone, kept) = permissions.drain(..).partition(|p| forget(p));
        *permissions = kept;
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_found_loosely() {
        assert_eq!(
            find_device("iPhone 13", "default").unwrap().name,
            "iPhone 13"
        );
        assert_eq!(
            find_device("iphone-13", "default").unwrap().name,
            "iPhone 13"
        );
        assert_eq!(
            find_device("pixel 7 LANDSCAPE", "default").unwrap().name,
            "Pixel 7 landscape"
        );
        let err = find_device("iPhone 99", "default").unwrap_err();
        assert_eq!(err.exit_code(), 2);
        let text = err.to_text();
        assert!(text.contains("did you mean"), "{text}");
        let err = find_device("Galaxy", "default").unwrap_err().to_text();
        assert!(err.contains("Galaxy S8"), "{err}");
        let err = find_device("xyzzy", "default").unwrap_err().to_text();
        assert!(err.contains("emulate --list"), "{err}");
    }

    #[test]
    fn the_device_table_is_sane() {
        assert!(DEVICES.len() >= 40);
        for device in DEVICES {
            assert!(device.width > 0 && device.height > 0, "{}", device.name);
            assert!(device.dpr >= 1.0, "{}", device.name);
            assert!(
                device.user_agent.starts_with("Mozilla/5.0"),
                "{}",
                device.name
            );
            let same = DEVICES
                .iter()
                .filter(|d| squash(d.name) == squash(device.name));
            assert_eq!(same.count(), 1, "{}", device.name);
        }
    }

    #[test]
    fn times_parse_as_utc_unless_offset() {
        assert_eq!(parse_time("1970-01-01"), Ok(0));
        assert_eq!(parse_time("1700000000000"), Ok(1_700_000_000_000));
        assert_eq!(parse_time("2026-01-31T09:30:00Z"), Ok(1_769_851_800_000));
        assert_eq!(parse_time("2026-01-31T09:30"), Ok(1_769_851_800_000));
        assert_eq!(
            parse_time("2026-01-31T10:30:00+01:00"),
            Ok(1_769_851_800_000)
        );
        assert_eq!(
            parse_time("2026-01-31T04:00:00-0530"),
            Ok(1_769_851_800_000)
        );
        assert_eq!(parse_time("2026-01-31T09:30:00.25Z"), Ok(1_769_851_800_250));
        assert_eq!(parse_time("2024-02-29 00:00"), Ok(1_709_164_800_000));
        for bad in [
            "",
            "soon",
            "2026-13-01",
            "2023-02-29",
            "2026-01-31T25:00",
            "2026-1-31",
            "2026-01-31T09:30+1",
        ] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn changes_touch_only_their_aspects() {
        let before = Emulation::default();
        let mut after = before.clone();
        after.locale = Some("de-DE".into());
        let aspects = Aspects::changed(&before, &after);
        assert!(aspects.locale && aspects.agent && !aspects.viewport && !aspects.clock);
        assert!(!Aspects::set(&before, false).any());
        assert!(Aspects::set(&before, true).viewport);
    }

    #[test]
    fn permissions_are_checked() {
        assert_eq!(
            check_permission("Geolocation", PermissionsOp::Grant, "default").unwrap(),
            "geolocation"
        );
        let err = check_permission("notification", PermissionsOp::Grant, "default")
            .unwrap_err()
            .to_text();
        assert!(err.contains("permissions grant notifications"), "{err}");
        let err = check_permission("camra", PermissionsOp::Deny, "default")
            .unwrap_err()
            .to_text();
        assert!(err.contains("permissions deny camera"), "{err}");
    }
}

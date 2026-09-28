//! The emulation commands' arguments, checked and turned into the
//! engine's [`EmulationChange`] (plan 07, milestone 7.2) before any daemon
//! starts.

use engine::EmulationChange;
use protocol::schema::{Geolocation, Media, Throttle, Viewport};
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint};

use crate::cli::{
    CpuThrottleArgs, EmulateArgs, GeoArgs, MediaArgs, NameArgs, OfflineArgs, OnOff, ThrottleArgs,
    TimeCommand, UaArgs, ViewportArgs,
};

/// Whether an argument asks to reset its setting.
fn is_reset(text: &str) -> bool {
    text.eq_ignore_ascii_case("reset")
}

fn bad(message: impl Into<String>, example: Cmd) -> ErrorInfo {
    ErrorInfo::new(ErrorKind::BadArgs, message).hint(Hint::run(example))
}

/// `emulate [DEVICE|reset]` (not `--list`, which needs no session).
pub(crate) fn emulate(args: &EmulateArgs, session: &str) -> Result<EmulationChange, ErrorInfo> {
    let name = args.device.join(" ");
    Ok(if name.is_empty() {
        EmulationChange::Show
    } else if is_reset(&name) {
        EmulationChange::Reset
    } else {
        let device = engine::find_device(&name, session)?;
        EmulationChange::Device {
            name: device.name.to_owned(),
        }
    })
}

/// `viewport WIDTHxHEIGHT [--dpr N] [--mobile] [--touch]` or `reset`.
pub(crate) fn viewport(args: &ViewportArgs) -> Result<EmulationChange, ErrorInfo> {
    let example = || Cmd::new("viewport").arg("1440x900");
    if is_reset(&args.size) {
        if args.dpr.is_some() || args.mobile || args.touch {
            return Err(bad(
                "`viewport reset` takes no --dpr, --mobile or --touch",
                Cmd::new("viewport").arg("reset"),
            ));
        }
        return Ok(EmulationChange::Viewport { viewport: None });
    }
    let size = args.size.to_ascii_lowercase().replace('×', "x");
    let parsed = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)));
    let Some((width, height)) =
        parsed.filter(|&(w, h)| (1..=10_000).contains(&w) && (1..=10_000).contains(&h))
    else {
        return Err(bad(
            format!(
                "`{}` is not a size; use WIDTHxHEIGHT, e.g. 1440x900",
                args.size
            ),
            example(),
        ));
    };
    let dpr = args.dpr.unwrap_or(1.0);
    if !(dpr.is_finite() && dpr > 0.0 && dpr <= 10.0) {
        return Err(bad(
            format!("--dpr {dpr} is out of range; use e.g. 1, 2 or 3"),
            Cmd::new("viewport").arg(&args.size).flag("dpr", "2"),
        ));
    }
    Ok(EmulationChange::Viewport {
        viewport: Some(Viewport {
            width,
            height,
            dpr,
            mobile: args.mobile,
            touch: args.touch,
            screen_width: None,
            screen_height: None,
        }),
    })
}

/// `geo LAT,LON [--accuracy M]` or `reset`.
pub(crate) fn geo(args: &GeoArgs) -> Result<EmulationChange, ErrorInfo> {
    let text = &args.position;
    if is_reset(text) {
        return Ok(EmulationChange::Geolocation { geolocation: None });
    }
    let numbers: Vec<Option<f64>> = text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f64>().ok().filter(|n| n.is_finite()))
        .collect();
    let example = || Cmd::new("geo").arg("59.9139,10.7522");
    let [Some(latitude), Some(longitude)] = numbers[..] else {
        return Err(bad(
            format!("`{text}` is not a position; use LATITUDE,LONGITUDE in degrees"),
            example(),
        ));
    };
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return Err(bad(
            format!(
                "{latitude},{longitude} is off the globe: latitude is -90 to 90, longitude -180 to 180"
            ),
            example(),
        ));
    }
    if !(args.accuracy.is_finite() && args.accuracy >= 0.0) {
        return Err(bad("--accuracy must be 0 or more metres", example()));
    }
    Ok(EmulationChange::Geolocation {
        geolocation: Some(Geolocation {
            latitude,
            longitude,
            accuracy: args.accuracy,
        }),
    })
}

/// `tz <IANA>` or `reset` (Chrome checks the name).
pub(crate) fn timezone(args: &NameArgs) -> EmulationChange {
    EmulationChange::Timezone {
        timezone: value(&args.name),
    }
}

/// `locale <BCP 47>` or `reset` (Chrome checks the tag).
pub(crate) fn locale(args: &NameArgs) -> EmulationChange {
    EmulationChange::Locale {
        locale: value(&args.name.replace('_', "-")),
    }
}

fn value(text: &str) -> Option<String> {
    let text = text.trim();
    (!is_reset(text)).then(|| text.to_owned())
}

/// `ua <USER-AGENT>` (`-`: stdin) or `reset`.
pub(crate) fn user_agent(args: &UaArgs) -> Result<EmulationChange, ErrorInfo> {
    let text = crate::input::text_arg(&args.user_agent, "USER-AGENT")?;
    let text = text.trim();
    if text.is_empty() {
        return Err(bad("the user agent is empty", Cmd::new("ua").arg("reset")));
    }
    Ok(EmulationChange::UserAgent {
        user_agent: value(text),
    })
}

/// `media [--type T] [--color-scheme S] [--reduced-motion R]
/// [--forced-colors F]` or `reset`.
pub(crate) fn media(args: &MediaArgs) -> Result<EmulationChange, ErrorInfo> {
    if args.reset.is_some() {
        return Ok(EmulationChange::Media { media: None });
    }
    let media = Media {
        r#type: args.r#type.clone(),
        color_scheme: args.color_scheme.clone(),
        reduced_motion: args.reduced_motion.clone(),
        forced_colors: args.forced_colors.clone(),
    };
    if media.is_empty() {
        return Err(bad(
            "media needs a feature to set, or reset",
            Cmd::new("media").flag("color-scheme", "dark"),
        ));
    }
    Ok(EmulationChange::Media { media: Some(media) })
}

pub(crate) fn offline(args: &OfflineArgs) -> EmulationChange {
    EmulationChange::Offline {
        offline: args.state == OnOff::On,
    }
}

/// `throttle PRESET|off`, with `--latency`, `--down` and `--up` changing
/// the preset's values or setting their own.
pub(crate) fn throttle(args: &ThrottleArgs) -> Result<EmulationChange, ErrorInfo> {
    let custom = args.latency.is_some() || args.down.is_some() || args.up.is_some();
    let presets: Vec<&str> = engine::throttle_presets().collect();
    let mut throttle = match args.preset.as_deref() {
        Some(off) if off.eq_ignore_ascii_case("off") => {
            if custom {
                return Err(bad(
                    "`throttle off` takes no --latency, --down or --up",
                    Cmd::new("throttle").arg("off"),
                ));
            }
            return Ok(EmulationChange::Throttle { throttle: None });
        }
        Some(name) => engine::throttle_preset(name).ok_or_else(|| {
            ErrorInfo::new(
                ErrorKind::BadArgs,
                format!("`{name}` is not a throttling preset"),
            )
            .note("presets", presets.join(", "))
            .hint(Hint::run(Cmd::new("throttle").arg("slow-3g")))
        })?,
        None if custom => Throttle {
            preset: None,
            latency_ms: 0.0,
            download_kbps: None,
            upload_kbps: None,
        },
        None => {
            return Err(ErrorInfo::new(
                ErrorKind::BadArgs,
                "throttle needs a preset, --latency, --down or --up",
            )
            .note("presets", presets.join(", "))
            .hint(Hint::run(Cmd::new("throttle").arg("slow-3g"))));
        }
    };
    if custom {
        throttle.preset = None;
    }
    if let Some(latency) = args.latency {
        throttle.latency_ms = latency.as_secs_f64() * 1000.0;
    }
    for (flag, value, slot) in [
        ("down", args.down, &mut throttle.download_kbps),
        ("up", args.up, &mut throttle.upload_kbps),
    ] {
        if let Some(kbps) = value {
            if !(kbps.is_finite() && kbps > 0.0) {
                return Err(bad(
                    format!("--{flag} must be more than 0 kbit/s"),
                    Cmd::new("throttle").flag(flag, "1000"),
                ));
            }
            *slot = Some(kbps);
        }
    }
    Ok(EmulationChange::Throttle {
        throttle: Some(throttle),
    })
}

/// `cpu-throttle RATE|off`.
pub(crate) fn cpu_throttle(args: &CpuThrottleArgs) -> Result<EmulationChange, ErrorInfo> {
    let text = args.rate.trim().trim_end_matches(['x', 'X']);
    if text.eq_ignore_ascii_case("off") || text.eq_ignore_ascii_case("reset") {
        return Ok(EmulationChange::CpuThrottle { rate: None });
    }
    match text.parse::<f64>() {
        Ok(rate) if rate.is_finite() && (1.0..=100.0).contains(&rate) => {
            Ok(EmulationChange::CpuThrottle {
                rate: (rate > 1.0).then_some(rate),
            })
        }
        _ => Err(bad(
            format!(
                "`{}` is not a slowdown; use 1 to 100 (4 = four times slower) or off",
                args.rate
            ),
            Cmd::new("cpu-throttle").arg("4"),
        )),
    }
}

/// `time freeze [TIME]`, `time advance DURATION`, `time reset`.
pub(crate) fn time(command: &TimeCommand) -> Result<EmulationChange, ErrorInfo> {
    Ok(match command {
        TimeCommand::Freeze(args) => {
            let epoch_ms = if let Some(text) = &args.time {
                protocol::format::parse_time(text).map_err(|message| {
                    bad(
                        message,
                        Cmd::new("time").arg("freeze").arg("2026-01-31T09:30:00Z"),
                    )
                })?
            } else {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                i64::try_from(now.as_millis()).unwrap_or(i64::MAX)
            };
            EmulationChange::TimeFreeze { epoch_ms }
        }
        TimeCommand::Advance(args) => EmulationChange::TimeAdvance { ms: args.by },
        TimeCommand::Reset => EmulationChange::TimeReset,
    })
}

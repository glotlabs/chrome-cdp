//! Units, tables and quoting shared by every text renderer (plan 08 §2):
//! durations in human units (`340ms`, `1.2s`), sizes as `12KB`, counts with
//! thousands separators, tables no wider than [`MAX_WIDTH`], shell-safe
//! arguments for hints, and text safe for a terminal ([`printable`]).

use std::borrow::Cow;
use std::time::Duration;

/// The widest a table row may be, in characters.
pub const MAX_WIDTH: usize = 120;

/// Aligns `rows` into columns two spaces apart. The last column is cut
/// (ending in `…`) so no line is wider than [`MAX_WIDTH`]; trailing spaces
/// are trimmed. Rows may have different lengths.
pub fn table<S: AsRef<str>>(rows: &[Vec<S>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; columns];
    for row in rows {
        // The last cell of each row is not padded, so it does not widen.
        for (i, cell) in row.iter().enumerate().take(row.len().saturating_sub(1)) {
            widths[i] = widths[i].max(cell.as_ref().chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            let cell = cell.as_ref();
            if i + 1 < row.len() {
                let pad = widths[i] - cell.chars().count();
                line.push_str(cell);
                line.extend(std::iter::repeat_n(' ', pad + 2));
            } else {
                let room = MAX_WIDTH.saturating_sub(line.chars().count());
                if cell.chars().count() > room {
                    line.extend(cell.chars().take(room.saturating_sub(1)));
                    line.push('…');
                } else {
                    line.push_str(cell);
                }
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.pop();
    out
}

/// Parses `500ms`, `30s`, `1.5s`, `2m`. A unit is required so `5000` is
/// never misread as seconds or milliseconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let value: f64 = num
        .parse()
        .map_err(|_| format!("`{s}` is not a duration; use e.g. 500ms, 30s or 2m"))?;
    let secs = match unit.trim() {
        "ms" => value / 1000.0,
        "s" => value,
        "m" => value * 60.0,
        "" => return Err(format!("`{s}` needs a unit; use e.g. {s}s or {s}ms")),
        other => return Err(format!("unknown unit `{other}`; use ms, s or m")),
    };
    let duration = Duration::try_from_secs_f64(secs).map_err(|e| format!("`{s}`: {e}"))?;
    if duration.is_zero() {
        return Err("the duration must be greater than zero".into());
    }
    Ok(duration)
}

/// Human units: `340ms`, `1.2s`, `30s`, `2m`, `2m30s`.
pub fn duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 10_000 && !ms.is_multiple_of(1000) {
        format!("{:.1}s", d.as_secs_f64())
    } else if ms < 60_000 {
        format!("{}s", d.as_secs())
    } else {
        let (m, s) = (d.as_secs() / 60, d.as_secs() % 60);
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m{s}s")
        }
    }
}

/// `5MB`, `200KB`, `1.5MB`, `2GB`, `1024`: a size in bytes (K, M and G
/// are 1024s).
pub fn parse_size(text: &str) -> Result<u64, String> {
    let upper = text.trim().to_ascii_uppercase();
    let (number, scale) = if let Some(n) = upper.strip_suffix("GB").or(upper.strip_suffix('G')) {
        (n, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(n) = upper.strip_suffix("MB").or(upper.strip_suffix('M')) {
        (n, 1024.0 * 1024.0)
    } else if let Some(n) = upper.strip_suffix("KB").or(upper.strip_suffix('K')) {
        (n, 1024.0)
    } else {
        (upper.strip_suffix('B').unwrap_or(&upper), 1.0)
    };
    let number: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("`{text}` is no size; write 5MB, 200KB or a number of bytes"))?;
    if !(number.is_finite() && number >= 0.0) {
        return Err(format!("`{text}` is no size"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok((number * scale).round() as u64)
}

/// Byte sizes in 1024 steps: `512B`, `1.5KB`, `12KB`, `3.4MB`, `2GB`. One
/// decimal below 10 units, none above.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    // Precision loss above 2^53 bytes does not matter for display.
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let unit = UNITS[unit];
    if value < 10.0 && (value.fract() * 10.0).round() % 10.0 != 0.0 {
        format!("{value:.1}{unit}")
    } else {
        format!("{value:.0}{unit}")
    }
}

/// `18203` → `18,203`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Quotes `arg` for a POSIX shell when it needs it, so a hint can be pasted
/// as is: `e12` stays `e12`, `Sign in` becomes `'Sign in'`, and `it's`
/// becomes `'it'\''s'`.
pub fn shell_quote(arg: &str) -> Cow<'_, str> {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+%^".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        Cow::Borrowed(arg)
    } else {
        Cow::Owned(format!("'{}'", arg.replace('\'', r"'\''")))
    }
}

/// Unix ms as ISO 8601 in UTC: `2026-09-28T10:04:05.123Z`.
pub fn iso8601(ms: u64) -> String {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60,
        ms % 1000
    )
}

/// Now as ISO 8601 in UTC (see [`iso8601`]).
pub fn iso8601_now() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    iso8601(ms)
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

/// `text` safe to print on a terminal: control characters but newline and
/// tab become their visible symbols (`ESC` is `␛`, a lone `CR` is `␍`,
/// `DEL` is `␡`, a C1 control `�`), and `CR LF` is `LF`. Text renders
/// hold what pages say (console messages, dialogs, titles), and an escape
/// sequence there could rewrite the terminal's screen, title or clipboard.
pub fn printable(text: &str) -> Cow<'_, str> {
    let unsafe_char = |c: char| c.is_control() && c != '\n' && c != '\t';
    if !text.contains(unsafe_char) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(match c {
            '\r' if chars.peek() == Some(&'\n') => continue,
            '\n' | '\t' => c,
            '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}'),
            '\u{7f}' => '\u{2421}',
            c if c.is_control() => '\u{fffd}',
            c => c,
        });
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        MAX_WIDTH, duration, parse_size, parse_time, printable, shell_quote, size, table, thousands,
    };

    #[test]
    fn tables_align_and_stay_narrow() {
        let rows = vec![
            vec!["e1", "button", "Sign in"],
            vec!["e12", "link", "Home"],
            vec!["e3", "", ""],
        ];
        assert_eq!(table(&rows), "e1   button  Sign in\ne12  link    Home\ne3");
        let long = "x".repeat(200);
        let rows = vec![vec!["e1", long.as_str()]];
        let out = table(&rows);
        assert_eq!(out.chars().count(), MAX_WIDTH);
        assert!(out.starts_with("e1  xxx") && out.ends_with("x…"));
        assert_eq!(table::<&str>(&[]), "");
    }

    #[test]
    fn durations_use_human_units() {
        assert_eq!(duration(Duration::from_millis(340)), "340ms");
        assert_eq!(duration(Duration::from_millis(1200)), "1.2s");
        assert_eq!(duration(Duration::from_secs(1)), "1s");
        assert_eq!(duration(Duration::from_secs(30)), "30s");
        assert_eq!(duration(Duration::from_secs(120)), "2m");
        assert_eq!(duration(Duration::from_secs(150)), "2m30s");
    }

    #[test]
    fn sizes_use_short_units() {
        assert_eq!(size(0), "0B");
        assert_eq!(size(1023), "1023B");
        assert_eq!(size(1024), "1KB");
        assert_eq!(size(1536), "1.5KB");
        assert_eq!(size(12 * 1024 + 300), "12KB");
        assert_eq!(size(3_565_158), "3.4MB");
        assert_eq!(parse_size("5MB"), Ok(5 * 1024 * 1024));
        assert_eq!(parse_size("1.5g"), Ok(3 * 512 * 1024 * 1024));
        assert_eq!(parse_size(" 200KB "), Ok(200 * 1024));
        assert_eq!(parse_size("1024"), Ok(1024));
        assert!(parse_size("lots").is_err());
        assert!(parse_size("-1MB").is_err());
        assert_eq!(size(2 * 1024 * 1024 * 1024), "2GB");
        assert_eq!(size(u64::MAX), "16777216TB");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(18_203), "18,203");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("e12"), "e12");
        assert_eq!(shell_quote("--timeout=2s"), "--timeout=2s");
        assert_eq!(shell_quote("https://x.test/a?b"), "'https://x.test/a?b'");
        assert_eq!(shell_quote("Sign in"), "'Sign in'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("{\"a\":1}"), "'{\"a\":1}'");
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
    fn printable_text_has_no_escapes() {
        assert!(matches!(
            printable("plain\ttext\n"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(
            printable("\x1b]52;c;aGk=\x07red\x1b[31m"),
            "\u{241b}]52;c;aGk=\u{2407}red\u{241b}[31m"
        );
        assert_eq!(printable("a\r\nb\rc"), "a\nb\u{240d}c");
        assert_eq!(
            printable("\u{7f}\u{9b}31m\0"),
            "\u{2421}\u{fffd}31m\u{2400}"
        );
    }
}

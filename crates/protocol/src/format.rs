//! Units and quoting shared by every text renderer (plan 08 §2): durations
//! in human units (`340ms`, `1.2s`), sizes as `12KB`, counts with thousands
//! separators, and shell-safe arguments for hints.

use std::borrow::Cow;
use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{duration, shell_quote, size, thousands};

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
}

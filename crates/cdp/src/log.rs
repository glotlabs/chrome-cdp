//! Helpers for wire-level tracing (`CHROME_CDP_LOG=cdp=trace`).

use std::fmt;

/// Frames longer than this are truncated in logs (screenshots are megabytes).
const MAX_LOGGED: usize = 1024;

/// Displays a wire frame, truncated to [`MAX_LOGGED`] bytes.
pub(crate) struct Truncated<'a>(pub(crate) &'a [u8]);

impl fmt::Display for Truncated<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        if bytes.len() <= MAX_LOGGED {
            return f.write_str(&String::from_utf8_lossy(bytes));
        }
        write!(
            f,
            "{}… (truncated, {} bytes total)",
            String::from_utf8_lossy(&bytes[..MAX_LOGGED]),
            bytes.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LOGGED, Truncated};

    #[test]
    fn short_frames_are_logged_whole() {
        assert_eq!(Truncated(b"{\"id\":1}").to_string(), "{\"id\":1}");
    }

    #[test]
    fn long_frames_are_truncated() {
        let frame = vec![b'a'; 5 * 1024 * 1024];
        let logged = Truncated(&frame).to_string();
        assert!(logged.starts_with(&"a".repeat(MAX_LOGGED)));
        assert!(logged.ends_with("… (truncated, 5242880 bytes total)"));
        assert!(logged.len() < MAX_LOGGED + 64);
    }
}

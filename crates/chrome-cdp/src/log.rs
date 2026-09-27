//! Logging to stderr (`-v`, `CHROME_CDP_LOG`).

use tracing_subscriber::EnvFilter;

use crate::output;

/// Installs the stderr logger. `CHROME_CDP_LOG` (an `EnvFilter` directive,
/// e.g. `cdp=trace` for every CDP frame) wins over `-v`.
pub(crate) fn init(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = match std::env::var("CHROME_CDP_LOG") {
        Ok(spec) if !spec.trim().is_empty() => EnvFilter::try_new(&spec).unwrap_or_else(|e| {
            output::stderr(&format!("warning: ignoring CHROME_CDP_LOG={spec:?}: {e}\n"));
            EnvFilter::new(default)
        }),
        _ => EnvFilter::new(default),
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

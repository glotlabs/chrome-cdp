//! Observability (plan 06): what the page logged and threw, for the
//! `console` and `errors` commands (milestone 6.1), its requests, for
//! `network` (milestone 6.2), what it looks like, for `screenshot` and
//! `pdf` (milestone 6.3), what a script says about it, for `eval`
//! (milestone 6.4), recordings and measurements (`har`, `trace`, `perf`,
//! `coverage`, milestone 6.5), and helpers for a human watching (`highlight`,
//! `inspect`, milestone 6.6).
//!
//! Each tab records into its buffers from the moment it exists (see
//! `tabs::watch_host`), so these commands work retroactively.

mod commands;
pub(crate) mod console;
mod coverage;
mod devtools;
mod eval;
pub(crate) mod format;
mod har;
pub(crate) mod hmr;
pub(crate) mod netlog;
mod network;
mod pdf;
mod perf;
pub(crate) mod save;
pub(crate) mod screenshot;
mod sourcemaps;
mod trace;

pub(crate) use commands::{entry_text, regex_arg, shown_source, shown_url};
pub use eval::check_eval;
pub(crate) use har::{HarRecording, iso8601};
pub use network::RESOURCE_TYPES;
pub(crate) use perf::OBSERVER as PERF_OBSERVER;
pub use screenshot::check_screenshot;
pub(crate) use sourcemaps::{Mapped, RESOLVE_LIMIT, SourceMaps, is_local};
pub(crate) use trace::TraceRecording;

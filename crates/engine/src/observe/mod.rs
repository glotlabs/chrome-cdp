//! Observability (plan 06): what the page logged and threw, for the
//! `console` and `errors` commands (milestone 6.1), its requests, for
//! `network` (milestone 6.2), what it looks like, for `screenshot` and
//! `pdf` (milestone 6.3), and what a script says about it, for `eval`
//! (milestone 6.4). Later milestones add traces here.
//!
//! Each tab records into its buffers from the moment it exists (see
//! `tabs::watch_host`), so these commands work retroactively.

mod commands;
pub(crate) mod console;
mod eval;
mod format;
mod har;
pub(crate) mod netlog;
mod network;
mod pdf;
mod perf;
mod save;
mod screenshot;
mod sourcemaps;
mod trace;

pub use eval::check_eval;
pub(crate) use har::HarRecording;
pub use network::RESOURCE_TYPES;
pub(crate) use perf::OBSERVER as PERF_OBSERVER;
pub use screenshot::check_screenshot;
pub(crate) use sourcemaps::SourceMaps;
pub(crate) use trace::TraceRecording;

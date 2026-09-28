//! Observability (plan 06): what the page logged and threw, for the
//! `console` and `errors` commands (milestone 6.1), its requests, for
//! `network` (milestone 6.2), and what it looks like, for `screenshot`
//! and `pdf` (milestone 6.3). Later milestones add `eval` and traces
//! here.
//!
//! Each tab records into its buffers from the moment it exists (see
//! `tabs::watch_host`), so these commands work retroactively.

mod commands;
pub(crate) mod console;
mod format;
pub(crate) mod netlog;
mod network;
mod pdf;
mod save;
mod screenshot;
mod sourcemaps;

pub use network::RESOURCE_TYPES;
pub use screenshot::check_screenshot;
pub(crate) use sourcemaps::SourceMaps;

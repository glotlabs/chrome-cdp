//! Observability (plan 06): what the page logged and threw, for the
//! `console` and `errors` commands (milestone 6.1). Later milestones add
//! the network log, screenshots, `eval` and traces here.
//!
//! Each tab records into its buffers from the moment it exists (see
//! `tabs::watch_host`), so these commands work retroactively.

mod commands;
pub(crate) mod console;
mod format;
mod sourcemaps;

pub(crate) use sourcemaps::SourceMaps;

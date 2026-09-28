//! E2E testing of a web project against a mocked or real backend (plan 11).
//!
//! - [`config`]: the `[e2e]` section of the project's `chrome-cdp.toml`,
//!   found by walking up from the current directory (milestone 11.1);
//! - [`web_server`]: starts the app's dev servers in their own process
//!   groups, waits for them to answer, and tears them down (11.1);
//! - [`init`]: `chrome-cdp init e2e`, which scaffolds the layout (11.1).
//!
//! The mock engine, the runner and the reports (11.2 onwards) build on
//! these.

pub mod config;

pub use config::{Config, find_project_file};

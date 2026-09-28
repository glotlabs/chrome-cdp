//! E2E testing of a web project against a mocked or real backend (plan 11).
//!
//! - [`backend`]: the backend a run uses, `mock` or a real one, and its
//!   `${var:…}` values (11.11);
//! - [`config`]: the `[e2e]` section of the project's `chrome-cdp.toml`,
//!   found by walking up from the current directory (milestone 11.1);
//! - [`web_server`]: starts the app's dev servers in their own process
//!   groups, waits for them to answer, and tears them down (11.1);
//! - [`init`]: `chrome-cdp init e2e`, which scaffolds the layout (11.1);
//! - [`scenario`]: scenario files and which of their tests a run has
//!   (11.4);
//! - [`mock_server`]: `mock serve`, the mock sets as a real HTTP server
//!   for server-side fetches (11.9).
//!
//! The mock engine (11.2) is the `mock` crate. The runner (11.4) is the
//! CLI's `test` command, since steps are CLI command lines.

pub mod backend;
pub mod config;
pub mod init;
pub mod mock_server;
pub mod scenario;
pub mod web_server;

pub use config::{Config, find_project_file};

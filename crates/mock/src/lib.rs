//! Mock files and the mock engine (plan 11, milestone 11.2).
//!
//! A mock file (TOML) is one named **mock set**: `[settings]` (scope,
//! unmatched policy, CORS, latency) and `[[mock]]` entries, each a `match`
//! on the request and a `respond`. [`MockSet::load`] reads and checks one,
//! with every problem at `file:line:col`; [`Mocks`] layers the sets in use
//! and decides what each request gets. It has no browser in it: the
//! daemon's router (`engine::route`) asks it about each paused request, the
//! CLI asks it for `mock explain` without a session, and plan 11.9's
//! `mock serve` can answer real HTTP requests with it.
//!
//! - [`file`]: the format as written, and its JSON Schema
//!   (`chrome-cdp schema mocks`);
//! - `load`: finding sets by name and compiling them;
//! - `matcher`: conditions on a request, with the reason one fails;
//! - `respond`: answers, data files, content types;
//! - `template`: `{{params.id}}` and friends (`minijinja`);
//! - `engine`: [`Mocks`];
//! - [`text`]: the text form of the `mock` commands' output.

mod engine;
pub mod file;
mod glob;
mod load;
mod matcher;
mod respond;
mod template;
pub mod text;

pub use engine::{Answer, Mocks, Outcome};
pub use file::{Cors, Unmatched, schema};
pub use load::{MockSet, Source, available};
pub use matcher::Request;
pub use respond::{
    MAX_FILE, NETWORK_ERRORS, Response, content_type, is_header_name, network_error,
};

#[cfg(test)]
mod tests;

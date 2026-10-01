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
//! - [`format`]: `chrome-cdp fmt` of mock files;
//! - `load`: finding sets by name and compiling them;
//! - `matcher`: conditions on a request, with the reason one fails;
//! - `multipart`: `multipart/form-data` bodies, for `match.form`;
//! - `closest`: how close a request comes to them (unmatched requests'
//!   closest mocks, and [`RequestFilter`] of `expect request`);
//! - `respond`: answers, data files, content types;
//! - `template`: `{{params.id}}` and friends (`minijinja`);
//! - [`check`] and [`openapi`]: `mock check`'s lints, and mocks against
//!   an OpenAPI spec;
//! - [`drift`]: real answers against the mocks' (`test --verify-mocks`);
//! - [`record`]: mock files from recorded traffic (`mock record`, `mock
//!   import`), `record_streams`: their WebSockets and EventSources, and
//!   `stable`: the body fields `--match-bodies` keys them by;
//! - `engine`: [`Mocks`];
//! - `ws`: `[[websocket]]` mocks (milestone 11.9), for the page's
//!   `WebSocket` stand-in;
//! - [`text`]: the text form of the `mock` commands' output;
//! - [`reference`]: the mock file reference page, from the schema.

pub mod check;
mod closest;
mod collection;
pub mod drift;

mod engine;
pub mod file;
pub mod format;
mod glob;
mod load;
mod matcher;
mod multipart;
pub mod openapi;
pub mod record;
mod record_streams;
pub mod reference;
mod respond;
mod stable;
mod template;
pub mod text;
mod ws;

pub use closest::{RequestFilter, Verdict};
pub use engine::{Answer, Mocks, Outcome, Seed, StateItem};
pub use file::{AfterSequence, Cors, Unmatched, schema};
pub use load::{Located, MockSet, Source, available};
pub use matcher::{Request, json_contains};
pub use respond::{
    MAX_FILE, NETWORK_ERRORS, Response, SseEvent, content_type, is_header_name, network_error,
    parse_sse,
};
pub use ws::{SocketGlob, WsFrame, WsKey, WsMessage, WsOutcome, WsStep};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_check;
#[cfg(test)]
mod tests_record;
#[cfg(test)]
mod tests_stateful;
#[cfg(test)]
mod tests_ws;

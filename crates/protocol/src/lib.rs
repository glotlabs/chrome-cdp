//! Shared types for the CLI <-> daemon protocol (line-delimited JSON-RPC 2.0)
//! and the output contract every command follows (plan 08 §2-§4).
//!
//! - [`ErrorKind`] and its exit code, and the error payload [`ErrorInfo`];
//! - [`Hint`] and [`Cmd`] to build the "next command" of an error;
//! - [`Output`], a command's result in every form, and [`render`], the one
//!   renderer for text, `-q` and the `--json` [`Envelope`];
//! - [`bound`]: how text and JSON output stay within a budget;
//! - [`format`]: durations, sizes and shell quoting for text output;
//! - [`schema`]: each command's `data` type and its JSON Schema;
//! - [`rpc`]: the line-delimited JSON-RPC 2.0 envelope between the CLI and
//!   the session daemon (plan 02);
//! - [`script`]: the `.cdp` format of `batch`, `run` and scenarios;
//! - [`suggest`]: "did you mean" for mistyped names.
//!
//! It has no I/O so both the CLI and the daemon can depend on it cheaply.

pub mod bound;
mod error;
pub mod format;
mod hint;
mod output;
pub mod rpc;
pub mod schema;
pub mod script;
mod style;
pub mod suggest;

pub use bound::{Limit, TruncatedKind, Truncation};
pub use error::{ErrorInfo, ErrorKind, Note};
pub use hint::{Cmd, DEFAULT_SESSION, Hint, PROGRAM};
pub use output::{Envelope, Mode, Output, Rendered, render};
pub use style::Style;

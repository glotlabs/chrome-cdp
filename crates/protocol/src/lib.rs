//! Shared types for the CLI <-> daemon protocol (line-delimited JSON-RPC 2.0).
//!
//! This crate owns the error kinds and their exit codes ([`ErrorKind`]), the
//! error payload every command reports ([`ErrorInfo`]), and the `--json`
//! output envelope ([`Envelope`]) from plan 08 §2-§3. The RPC request and
//! response types join them with the daemon (plan 02). It has no I/O so both
//! the CLI and the daemon can depend on it cheaply.

mod error;
mod output;

pub use error::{ErrorInfo, ErrorKind};
pub use output::Envelope;

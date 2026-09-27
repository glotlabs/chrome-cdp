//! Shared types for the CLI <-> daemon protocol (line-delimited JSON-RPC 2.0).
//!
//! This crate will own the RPC request/response types, the error kinds and
//! their exit codes, and the `--json` output schemas (plan 08 §2-§3). It has
//! no I/O so both the CLI and the daemon can depend on it cheaply.

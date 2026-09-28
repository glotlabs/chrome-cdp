//! The per-session daemon: socket server, session lifecycle, idle shutdown
//! and RPC dispatch into the engine (plan 02, milestone 2.4).
//!
//! - [`paths`]: where a session's socket, pid, `meta.json` and logs live.
//! - [`server`]: the daemon process (`chrome-cdp __daemon`).
//! - [`client`]: the CLI's side: finding, starting and replacing daemons,
//!   and calls.
//! - [`rpc`]: the method names and params; the JSON-RPC envelope is
//!   `protocol::rpc`.
//! - [`output`]: the session commands' text forms.
//!
//! Keep Unix-socket specifics in `transport/unix.rs` so the rest stays
//! portable (overview §8).

pub mod client;
mod history;
pub mod meta;
pub mod output;
pub mod paths;
pub mod rpc;
pub mod server;
mod transport;

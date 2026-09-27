//! The per-session daemon: socket server, session lifecycle, idle shutdown
//! and RPC dispatch into the engine (plan 02).
//!
//! Keep Unix-socket specifics in `transport/unix.rs` so the rest stays
//! portable (overview §8).

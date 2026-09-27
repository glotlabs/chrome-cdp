//! Generates the typed CDP domain modules in `crates/cdp` from the vendored
//! `browser_protocol.json` and `js_protocol.json` (plan 01, milestone 1.2).
//!
//! It is a library so `xtask gen-cdp` can call it and check for drift.

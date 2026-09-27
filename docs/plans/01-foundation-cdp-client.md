# Plan 01 — Foundation & CDP client

Status: TODO
Depends on: 00-overview (read first), 08 §2–§4 (output conventions)

## Goal

Set up the Cargo workspace, the tooling, and a correct, fast, well-tested CDP
client that every later plan builds on. At the end of this plan,
`chrome-cdp cdp Browser.getVersion` works against a Chrome the test harness
launches. The daemon does not exist yet; this plan uses a direct connection.

## Milestone 1.1 — Workspace & tooling

- Create a Cargo workspace with the crates from overview §4. Add
  `rust-toolchain.toml` (stable, pinned) and `.editorconfig`.
- Set the release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`,
  `panic = "abort"`. Record the binary size in the decisions log and keep it
  under 15 MB.
- Add lints to workspace `Cargo.toml`: `unsafe_code = "deny"` (allow it only in
  the fd-passing module), `clippy::pedantic` warn with a curated allow-list.
- Add `rustfmt.toml` and `deny.toml` (cargo-deny: licenses and advisories).
- Add `.gitignore` and `README.md` (a short stub with a pointer to docs).
- Add `xtask` with the subcommands `gen-cdp`, `fixtures`, `ci` (fmt, clippy,
  test), and `dist`.
- Add a GitHub Actions skeleton: `ubuntu-latest`, `ubuntu-24.04-arm`,
  `macos-latest` (arm64) and `macos-13` (x86_64). It runs
  `cargo fmt --check`, `clippy -D warnings`, and `cargo test`. Plan 10 grows it.

## Milestone 1.2 — Protocol type generation

- Vendor `browser_protocol.json` and `js_protocol.json` into
  `crates/cdp/protocol/`. Pin them to a Chromium revision and record it in
  `PROTOCOL_VERSION`.
- The `cdp-gen` crate generates, per domain, a module with:
  - one params struct and one returns struct per command, and an
    `impl Command for XParams { const METHOD: &str; type Returns; }`;
  - one struct per event, and an `impl Event { const NAME: &str }`;
  - types, where enums become Rust enums with a `#[serde(other)] Unknown`
    variant, so newer Chrome versions don't break deserialisation;
  - `Option<T>` + `skip_serializing_if` for optional fields;
    `#[serde(rename_all = "camelCase")]`.
- Generate only an allow-listed set of domains to keep compile time down:
  Browser, Target, Page, Runtime, DOM, DOMSnapshot, Accessibility, Input,
  Network, Fetch, Emulation, Log, Security, Storage, Tracing, IO,
  Performance, CSS (subset), Overlay (for headed highlight). Adding one is a
  one-line change.
- The generated code is committed. The CI checks `cargo xtask gen-cdp --check`
  to detect drift.

## Milestone 1.3 — Transport & connection

```rust
#[async_trait] trait Transport { async fn send(&self, msg: Bytes) -> Result<()>; async fn recv(&self) -> Result<Bytes>; }
```

- `PipeTransport`: Chrome reads fd 3 and writes fd 4. Messages are
  NUL-terminated JSON. Spawn it via `std::process::Command` + `pre_exec`
  (`dup2`) inside the single `unsafe` module, or use `command-fds`. It must
  work on both Linux and macOS, and a test must cover both.
- `WsTransport`: `tokio-tungstenite` with no message size limit. CDP
  screenshots can be larger than 100 MB.
- `Connection` has one writer task and one reader task.
  - Request ids come from an `AtomicU64`. Pending calls live in
    `DashMap<u64, oneshot::Sender<Result<Value>>>`.
  - Events are routed by `sessionId`: each flat session (`Target.attachToTarget`
    with `flatten: true`) has its own `broadcast` channel. Consumers subscribe
    by event name via a typed `session.listen::<Page::LoadEventFired>()` stream.
  - Protocol errors become `CdpError::Protocol { code, message, data }`.
    Transport closure fails all pending calls with `CdpError::Disconnected`
    (never hang).
  - Every call gets a per-call timeout (default 30 s, overridable), and cancel
    safety: dropping the future removes the pending entry.
- `Session` is a handle `{ conn, session_id: Option<String> }` with
  `call<C: Command>(params) -> C::Returns` and `call_raw(method, Value)`.
- Enable tracing at `CHROME_CDP_LOG=cdp=trace` to log every frame. Truncate
  large payloads such as screenshots.

## Milestone 1.4 — Bootstrap CLI

- Add a `chrome-cdp` binary using clap derive, and global flags defined by plan
  08: `--json`, `--session`, `--timeout`, `--quiet`, `-v`.
- Add a temporary, hidden `chrome-cdp __direct cdp <Method> [json] --chrome <path>`.
  It launches Chrome with the pipe, runs the call, prints the result, and
  exits. Plan 02 replaces this with the daemon-backed `chrome-cdp cdp`.
- Add `chrome-cdp --version`: prints the crate version, git sha, and protocol
  revision.

## Tests

- Unit tests: message framing (split and concatenated NUL frames), id
  correlation, error mapping, session routing, and timeout/cancel cleanup. Use
  an in-memory mock transport with scripted responses.
- Integration test (`#[ignore]` unless `CHROME_PATH` is set; always run in CI):
  launch real headless Chrome via pipe → `Browser.getVersion` → create target →
  attach flat → `Page.navigate` to a data URL → receive `Page.loadEventFired`.
- A fuzz or property test for the frame decoder (`proptest`).

## Acceptance criteria

- [ ] `cargo xtask ci` is green on all 4 CI runners.
- [ ] The pipe transport works on Linux and macOS. Killing Chrome makes all
      pending calls fail within 100 ms, without panics.
- [ ] 10,000 sequential `Runtime.evaluate("1+1")` calls finish in < 3 s locally
      (a latency baseline for plan 10).
- [ ] No `unwrap()`/`expect()` outside tests and `main` (enforced by a clippy
      lint).

## Out of scope

The daemon, Chrome discovery, and the install command are in plan 02.

## Decisions log

_(append here)_

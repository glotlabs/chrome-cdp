# Plan 01 — Foundation & CDP client

Status: DONE (1.1-1.4; the two open criteria need a green GitHub CI run, including macOS)
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
- [x] 10,000 sequential `Runtime.evaluate("1+1")` calls finish in < 3 s locally
      (a latency baseline for plan 10).
- [x] No `unwrap()`/`expect()` outside tests and `main` (enforced by a clippy
      lint).

## Out of scope

The daemon, Chrome discovery, and the install command are in plan 02.

## Decisions log

_(append here)_

- 1.1: Toolchain pinned to `1.98.1` (current stable) with `rust-version = "1.98"`; bump both together.
- 1.1: CI uses `macos-15-intel` instead of `macos-13` for macOS x86_64, because GitHub retired the `macos-13` runner image in late 2025.
- 1.1: Release binary size: 301KB (x86_64 Linux stub, no dependencies yet). Budget is 15 MB, enforced by `cargo xtask dist` and the CI `dist` job.
- 1.1: No `license` field in the workspace yet (no license has been chosen); `deny.toml` ignores unpublished crates in the license check.
- 1.1: No-`unwrap`/`expect` is enforced with `clippy::unwrap_used`/`expect_used` (warn, `-D warnings` in `xtask ci`); `clippy.toml` allows them in tests. `main` must opt out explicitly with `#[allow]`.
- 1.1: Library crates also warn on `print_stdout`/`print_stderr` so only the binary writes to stdout (plan 08 §2); the binary and xtask opt out.
- 1.1: `xtask` is std-only so `cargo xtask ci` starts fast. CI runs `cargo xtask ci` itself, so local and CI checks cannot diverge. `gen-cdp` and `fixtures` are placeholders that exit non-zero until milestone 1.2 and the fixture server exist.
- 1.2: Protocol pinned to Chromium r1687809 (devtools-protocol tag `v0.0.1687809`), the last roll at or below Chrome 154.0.8037.57's branch position 1689415. Compared with Chrome 154's live `/json/protocol`, the only difference is one `Audits` enum value.
- 1.2: The generator is a `cdp-gen` binary that `cargo xtask gen-cdp` runs via `cargo run`, so xtask stays std-only. It pipes its output through `rustfmt` with the workspace `rustfmt.toml`, so `cargo fmt --check` accepts the generated code.
- 1.2: The allow-list is `crates/cdp/protocol/domains.txt` (one line per domain). `Domain: a, b` generates a subset: CSS = enable, disable, getComputedStyleForNode, and rule-usage tracking (plan 06 coverage); Overlay = enable, disable, highlightNode, hideHighlight.
- 1.2: Types that listed domains take from unlisted domains (today Debugger and DOMDebugger) are generated as types-only modules instead of falling back to `serde_json::Value`.
- 1.2: Modules are snake_case (`cdp::domains::page::LoadEventFired`), not `Page::LoadEventFired` as the 1.3 sketch writes.
- 1.2: If an enum already has an `unknown` value, that variant carries `#[serde(other)]`, so there is no second `Unknown`. The catch-all is always the last variant, because serde requires it.
- 1.2: By-value recursive fields are boxed (`Option<Box<StackTrace>>`). An event whose name collides with a type gets an `Event` suffix (none do today). Params structs get `new(required...)`, or `Default` when every field is optional.
- 1.2: `integer` → `i64`, `number` → `f64`, an `object` with no properties → `serde_json::Map`. Named primitive types are `pub type` aliases, not newtypes, to keep the generated code small.
- 1.2: The generated code is 22 modules and 21.9k lines. The `cdp` crate builds in about 9 s (debug or release, 4 cores).
- 1.2: The drift check also runs as a `cdp-gen` unit test, so plain `cargo test` catches stale generated code as well as `cargo xtask ci`.
- 1.3: `Transport` is split into `TransportSender`/`TransportReceiver` halves (`fn split(self)`) using native async fns in traits (`impl Future + Send`), not `#[async_trait]` with `&self`. Each I/O task owns one half, so there is no locking or boxing.
- 1.3: The fd passing is our own `pre_exec` + `dup2` (`libc`) in `transport/pipe/fd.rs`, not `command-fds`. The child's pipe ends are first moved to fds >= 5 with `F_DUPFD_CLOEXEC`, so a `dup2` onto 3 or 4 can never clobber the other source. `PipeTransport::spawn` takes a `tokio::process::Command` whose flags the caller supplies; plan 02 owns the flag list.
- 1.3: Per-session event channels are created on first `listen`/`events` and removed on `Target.detachedFromTarget` or disconnect, so streams end instead of hanging. A listener more than `event_buffer` (1024) events behind skips the oldest with a warning. An event that fails to decode into its typed struct is skipped with a warning.
- 1.3: The per-call timeout override is `Session::with_timeout(d)`, which returns a handle copy, not an extra parameter on every call.
- 1.3: `CdpError` = `Protocol { method, code, message, data }`, `Disconnected { reason }`, `Timeout { method, after }`, `Serialize`, `Decode`. Mapping these to plan 08 error kinds and exit codes is left to the `protocol` crate.
- 1.3: Kill latency: on Linux, pending calls fail 6-55 ms after SIGKILL (Chrome 154, test run alone). Nearly all of that is the kernel tearing down Chrome before the pipe closes, and with parallel tests it passed 100 ms. So the Chrome test asserts < 500 ms, and a unit test asserts that the client itself reacts to a closed transport in < 100 ms.
- 1.3: 10,000 sequential `Runtime.evaluate("1+1")` calls over the pipe took 1.6 s in release (160 µs/call) on the dev machine. The benchmark is an `#[ignore]` test: `cargo test -p cdp --release --test chrome -- --ignored --nocapture`.
- 1.3: The Chrome tests are not `#[ignore]`. Without `CHROME_PATH` they pass as no-ops. CI sets `CHROME_PATH` and `CHROME_CDP_REQUIRE_CHROME=1` on runners with Google Chrome preinstalled, so there a missing Chrome fails the run. The Linux arm64 runner has no Google Chrome and covers the pipe only through a stand-in `sh` child. The stand-in child tests run on every Unix runner, macOS included.
- 1.3: `WsTransport` supports `ws://` only (no TLS feature). Add rustls in plan 02 if `connect` needs `wss://`.
- 1.3: The library only emits tracing events (targets `cdp::wire` and `cdp::connection`). The binary installs the `CHROME_CDP_LOG` subscriber (milestone 1.4); the Chrome tests already do.
- 1.4: Release binary size with clap, tokio and tempfile: 1.7 MB (x86_64 Linux).
- 1.4: `ErrorKind` (the plan 08 §3 table with exit codes), `ErrorInfo` (message, hint, details, stderr text form) and the `--json` `Envelope` live in `protocol` now, because every command needs them. The envelope omits empty `warnings`/`events`. The hint builder, golden tests and JSON Schemas stay in milestone 8.1.
- 1.4: Every command returns `Result<Success, ErrorInfo>`, and `output::render` prints it: text or `-q` on stdout, warnings and errors on stderr, or one envelope with `--json`. New commands add a `Command` variant and return a `Success` with their text and quiet forms.
- 1.4: `--version`/`-V` is our own flag (clap's is disabled), so `--version --json` and `-q` follow the output contract. Text: `chrome-cdp 0.1.0 (git <sha12>, CDP protocol r1687809)`. `build.rs` embeds the sha; `CHROME_CDP_GIT_SHA` at build time overrides it (tarball builds), else it is `unknown`.
- 1.4: `--timeout` requires a unit (`500ms`, `30s`, `2m`). A bare `5000` is a usage error, because agents coming from Playwright mean milliseconds and humans mean seconds.
- 1.4: `--json`, `--quiet`, `--session` and `--timeout` have `CHROME_CDP_*` env vars. `-v` has none; `CHROME_CDP_LOG` (an `EnvFilter` directive) is its env form and overrides it. `--json` and `--quiet` conflict.
- 1.4: Usage errors print clap's text on stderr and exit 2. With `--json` (flag or env var) they are a `BadArgs` envelope on stdout like any other error.
- 1.4: `__direct cdp` also takes `--target browser|page` (page = a new about:blank tab), which plan 02's `cdp` has too, so page methods are testable now. The Chrome comes from `--chrome` or `CHROME_CDP_CHROME` only; discovery is plan 02. It sends `Browser.getVersion` as a handshake, so a Chrome that dies at startup is `LaunchFailed` with the tail of its stderr (in `details.chrome_stderr`), and "No usable sandbox" gets an AppArmor hint.
- 1.4: `CdpError` maps to `ProtocolError` (exit 4; -32601/-32602 hint at the domain's docs), `Timeout` (exit 3; hint doubles `--timeout`), and `BrowserCrashed` for a disconnect (exit 4).
- 1.4: Text and `-q` output of `__direct cdp` is bounded by `--max-chars` (default 20,000; 0 = no limit). `--json` output is never truncated, because truncated JSON is invalid; plan 08 should decide how `--json` stays bounded.

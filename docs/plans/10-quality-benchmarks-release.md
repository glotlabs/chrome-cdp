# Plan 10 — Quality, benchmarks & release

Status: IN PROGRESS (10.1-10.2 done; the fixture site and CI matrix start in plan 01 and grow with every plan)
Depends on: all; the benchmark harness should exist by the end of plan 04

## Goal

Prove the claim "better than Playwright for agents" with numbers, keep it
true with CI gates, and ship a v1 that installs in one command on Linux and
macOS.

## Milestone 10.1 — Test infrastructure

- **Fixture site** (`testdata/site`): static pages plus small dynamic
  endpoints, served by a test-support crate (`axum`) on random ports with two
  hostnames (`127.0.0.1` and `localhost`) for cross-origin/OOPIF cases.
  Include small React, Vue and Svelte apps, prebuilt and committed so tests
  need no Node. The fixture shop is `examples/e2e-shop` from plan 11; its axum
  API is also the "real backend" for plan 11's `--backend local` tests.
- **E2E harness** (`tests/`): helpers that run the real `chrome-cdp` binary
  with a unique `--session` per test, assert on stdout/stderr/exit code, and
  guarantee cleanup with a `Drop` guard that runs `close`. Use
  `cargo nextest` for parallel, isolated runs with retries turned **off**,
  so flakiness is visible.
- **Flakiness gate:** a nightly job runs the full E2E suite 20× per runner.
  Any failure opens an issue automatically.
- **Leak checks:** after the suite, no chrome or daemon processes remain, and
  no socket files are left in the session dirs.
- **Soak test** (nightly): 1,000 random commands from a weighted command mix
  against fixtures. Assert no crash, bounded RSS, and p99 latency within
  budget.

## Milestone 10.2 — CI matrix

| Runner | Arch | Chrome source |
|---|---|---|
| ubuntu-latest | x86_64 | Chrome for Testing (stable), plus a system Chrome job |
| ubuntu-24.04-arm | arm64 | distro Chromium (no CfT on linux-arm64) |
| macos-latest | arm64 | CfT stable |
| macos-13 | x86_64 | CfT stable |

- Also run a weekly job against CfT **beta**. That catches protocol breakage
  before it reaches stable.
- Also run a job against the **minimum supported** Chrome version.
- Build checks: `cargo build --release` for `x86_64/aarch64-unknown-linux-musl`
  and `x86_64/aarch64-apple-darwin`. Track binary size and fail on +20%.

## Milestone 10.3 — Playwright parity matrix

Maintain `docs/playwright-parity.md`, a table of Playwright capability →
`chrome-cdp` equivalent → status (✅ / 🟡 partial / ❌ / ➖ not planned) →
agent-critical (yes/no). Rows at minimum:

- navigation;
- the locator types (role, text, label, placeholder, alt, title, testid,
  css, xpath, chaining, filters, nth);
- auto-wait/actionability;
- click/dblclick/hover/fill/type/press/check/select/upload/drag/tap/focus/scroll;
- dialogs, downloads, popups, frames, shadow DOM;
- screenshot/pdf/video (video: 🟡 via `Page.startScreencast` → frames → optional
  ffmpeg; decide whether it is in v1);
- console, page errors, network events, route/mock, HAR record/replay
  (replay: `route --from-har`);
- storageState, browser contexts, emulation (devices/geo/tz/locale/media/
  offline/permissions), clock;
- tracing, accessibility snapshot, `expect` matchers;
- test runner (`webServer`, projects, retries, sharding, reporters), `page.route`
  mocking in tests, HAR replay, running the same tests against different
  environments;
- codegen (➖).

The v1 gate: no ❌ in rows marked agent-critical.

## Milestone 10.4 — Benchmarks

`xtask bench` has three suites. Results go to `docs/benchmarks.md` with
methodology, and nightly regressions of more than 15% fail CI.

1. **Micro:**
   - cold start;
   - warm command round trip;
   - snapshot latency by page size (the corpus from plan 04);
   - click-to-report latency;
   - route overhead;
   - `batch` vs single calls.

   Each is compared with Playwright library equivalents (a `bench/playwright/`
   Node harness, run only in bench jobs).
2. **Token efficiency:** for each corpus page, count the tokens of the
   `chrome-cdp snapshot`, `snapshot -i` and Playwright MCP `browser_snapshot`
   outputs (tokenizer: count via the Anthropic token counting API if a key is
   available, otherwise an approximation; record which one was used).
3. **Agent task benchmark:** about 25 scripted tasks on the fixture sites and
   stable public sandboxes. Examples: log in, fill a multi-step form, find
   and fix a failing button on the local dev server, verify responsive layout
   on mobile, debug a console error, upload a file, handle a popup.
   - Include the E2E tasks from plan 11: write scenarios + mocks for a
     frontend, fix an app change that broke a scenario, and make a mocked suite
     also pass against the real backend.
   - Run Claude Code headless (`claude -p`) with (a) only `chrome-cdp skill` and
     (b) Playwright MCP.
   - Measure success (checked by a verifier), total tokens, turns, and
     wall time. Run N=5 per task and arm.
   - This is the headline number for v1 (overview §7). It also decides open
     defaults: `--snap` after actions, snapshot engine, and default text
     limits.

## Milestone 10.5 — Packaging & distribution

- `cargo-dist` (or goreleaser-rust) builds release artifacts for the 4 targets
  on tag push:
  - tarballs + SHA256 sums;
  - GitHub release notes generated from conventional commits;
  - **macOS:** sign and notarize the binary (Developer ID). Without
    notarization, Gatekeeper quarantines downloads. Document the `xattr`
    workaround until signing is set up.
- Install channels:
  - `curl -fsSL …/install.sh | sh` (detects OS/arch, verifies the checksum,
    installs to `~/.local/bin`);
  - a Homebrew tap (`brew install <org>/tap/chrome-cdp`);
  - `cargo install chrome-cdp` / `cargo binstall chrome-cdp`;
  - an npm wrapper (`npx chrome-cdp`) with per-platform optional-dependency
    packages, because agents live in Node repos.
- `chrome-cdp upgrade` (self-update from GitHub releases, verified checksum)
  and `chrome-cdp doctor` (checks the Chrome install, missing Linux libs,
  sandbox, socket dir permissions, macOS quarantine, and version skew; prints
  fixes).
- Versioning: semver. The CLI syntax and `--json` schemas are the public API.
  Keep a `CHANGELOG.md`.

## Milestone 10.6 — Documentation

- `docs/e2e/` (plan 11) is linked from the README as a headline feature.
- `README.md`: a 30-second pitch, install, a 6-command quickstart, and a
  comparison table vs Playwright CLI/MCP with the benchmark numbers.
- `docs/`: a command reference generated from clap (`xtask docs`), plus guides
  on locators & refs, waiting, debugging a local dev server, testing with
  `.cdp`, CI usage (GitHub Actions example), Docker usage (a published
  `ghcr.io/…/chrome-cdp` image with Chrome and fonts), `migrating-from-
  playwright.md`, security model, and troubleshooting (by error kind).
- `SKILL.md` (plan 08) lives in the repo and is tested by the agent benchmark.

## Milestone 10.7 — Security review

- Threat model doc: a local attacker cannot reach the browser (no TCP port,
  0600 socket, per-uid dirs); state files and profiles hold secrets; `eval`
  and the raw `cdp` command are full-power by design.
- `connect` to a remote `ws://` warns when it is not loopback.
- `cargo deny` + `cargo audit` in CI. Fuzz the RPC decoder and the `.cdp`
  parser (`cargo fuzz`, nightly job).

## v1.0 release checklist

- [ ] Overview §7 definition of done is fully ticked.
- [ ] Parity matrix: no ❌ in agent-critical rows.
- [ ] Benchmarks published; the agent benchmark beats Playwright MCP on success
      rate and tokens.
- [ ] Nightly flakiness job green for 7 consecutive days.
- [ ] Signed and notarized macOS binaries; brew, curl, cargo and npm installs
      verified on clean VMs (macOS arm64/x64 verified by the maintainer).
- [ ] Docs complete; `chrome-cdp doctor` passes on all CI runners.

## Decisions log

_(append here)_
- 10.1: The fixture server stays on `hyper` (`crates/fixtures`, from plan 01) rather than `axum`: it already serves both origins, HTTPS, WebSockets and the dynamic routes, and axum would add a framework for no new feature. The shop's backend is `fixtures::Shop` in the same crate.
- 10.1: The E2E harness lives in `crates/chrome-cdp/tests/common/`, not a top-level `tests/`: integration tests need `CARGO_BIN_EXE_chrome-cdp`, which Cargo only gives the binary's own crate. Each test's `Sandbox` has its own `XDG_RUNTIME_DIR` and `TMPDIR`, which isolates it more than a unique `--session`; `Sandbox::session()` adds a `Session` guard with a unique name, `ok`/`json`/`fails` assertions on exit code, stdout and stderr, and `close` on drop.
- 10.1: Leak checks happen twice. Dropping a `Sandbox` runs `close --all`, then fails the test if a daemon, a Chrome or a socket file of it is left (keeping its dir to look at); so a leak names the test that caused it. After the suite, `cargo xtask leaks` (replacing CI's `pgrep` step) checks the whole machine for chrome-cdp's processes and for socket files in every session dir and `target/tmp`. nextest's own `leak-timeout` fails a test whose children hold its output open.
- 10.1: `.config/nextest.toml`: retries 0 everywhere; a test hangs for at most 5 minutes (the golden test, which runs every command's cases in one test, 16), so a hang like `.looper`'s "navigate test hung once" fails the run. `cargo xtask ci` uses nextest when installed (plus `cargo test --doc`, which nextest does not run); CI installs it.
- 10.1: The flakiness gate is `cargo xtask e2e --repeat 20`: the whole workspace suite 20 times with the `ci` profile and `xtask leaks` after each run, all 20 even after a failure, with a summary in `target/nextest/e2e-summary.md`. The nightly `flaky` job runs it on the 4 CI runners; a failure opens an issue through `.github/actions/nightly-issue` (label `nightly`, one open issue per title, later failures comment on it). plan 11.10's `e2e-shop --repeat 20` job stays as it is.
- 10.1: Its first nextest runs found two flakes that `cargo test` had hidden: the golden test's `<n> retries` mask missed the singular, and the state test's `logIn()` eval raced its own navigation. Both are fixed.
- 10.1: The soak test (`random_command_mix_does_not_crash_or_grow` in `crates/chrome-cdp/tests/soak.rs`, ignored; `cargo xtask soak`, nightly on Linux and macOS) draws 1,000 commands from a seeded, weighted mix of 23 kinds over 13 fixture pages, acting on refs from its last `snapshot -i`. A crash is a signal, a new daemon pid, or exit 2 or 4; exits 1, 3 and 5 are allowed, with at most 2% timeouts. Budgets: daemon RSS under 150 MB (as plan 06's), p99 at most 2 s including page loads. The budgets are not tuned: deferred: benchmarks run after all plans. Short runs (150-200 commands, three seeds) only checked that the mix runs.
- 10.1: The framework apps (`testdata/site/apps/`) are one todo app written three times (React 18 UMD with `createElement`, Vue 3 with an in-DOM template, Svelte 5 compiled from `vendor/src/Todos.svelte`), so one test (`tests/apps.rs`) checks all three. The vendored builds moved from `actions/vendor/` to `/vendor/` for all pages to share; `vendor/README.md` says how to rebuild the Svelte bundles.
- 10.1: "Consolidate the fixture site": `testdata/site/index.html` now links every folder, and `crates/fixtures/tests/site.rs` fails when a folder is missing from it or a link is broken.
- 10.2: Linux arm64 runs Chrome for Testing stable like the other runners, not distro Chromium: CfT has published linux-arm64 builds since Chrome 153 (plan 02's decisions log). Ubuntu's Chromium is a snap, whose confinement blocks the suite's sandbox dirs, so it gets its own `chromium` job on the arm runner that launches it through the CLI (temporary and named profile).
- 10.2: `macos-15-intel` replaces `macos-13`, which GitHub has retired; it is the last Intel macOS image.
- 10.2: `cargo xtask chrome <stable|beta|min|VERSION|system> [--with-deps]` gets each job's Chrome with the debug build's `chrome-cdp install`, checks that it runs, and exports `CHROME_PATH` and `CHROME_CDP_REQUIRE_CHROME=1`, so every `ci` job (arm64 included) fails instead of skipping the Chrome tests. `min` reads `MIN_CHROME_MAJOR` from `crates/browser/src/version.rs`, so the job follows the constant. The nightly `flaky` and `soak` jobs use CfT stable through it too.
- 10.2: The `ci` matrix is ubuntu-latest (CfT stable, system Google Chrome, and the minimum Chrome), ubuntu-24.04-arm, macos-latest and macos-15-intel (CfT stable). The minimum-version job runs on every push, on Linux x86_64 only. Beta runs weekly (`weekly.yml`: `cargo xtask e2e` on Linux and macOS arm64); a failure opens an issue through `nightly-issue`.
- 10.2: The `build` job builds the four release targets, each on its own architecture so the smoke test (`--version`, `install --list`) runs the binary; the musl targets use `musl-tools` for ring's C. The size gate is a committed baseline per target in `xtask/binary-sizes.txt`: `cargo xtask dist --target T` fails over +20%, warns under −20% or with no baseline, and writes each size to the job summary; `--update-size` records a new baseline. Comparing with the base branch's build instead would double the build time. The macOS baselines could not be measured here (no macOS SDK); until they are recorded their jobs only warn.
- 10.2: The binary had outgrown plan 01's fixed 15MB budget (20.5MB for x86_64 Linux at 10.2; the `dist` job would have failed), so the +20% baseline replaces it. Baselines: x86_64 musl 20.6MB, aarch64 musl 16.2MB (cross-built here with clang and rust-lld).
- 10.2: The whole suite passed here against the minimum Chrome (CfT 150.0.7871.124 via `cargo xtask chrome min`: 956 tests, nextest `ci` profile), and the static x86_64 musl binary opened, snapshotted and closed CfT 154 with the sandbox on. None of the workflows has run on GitHub yet (the repo has no remote).

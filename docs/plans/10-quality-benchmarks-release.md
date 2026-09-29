# Plan 10 — Quality, benchmarks & release

Status: IN PROGRESS (10.1-10.7 done, the v1 release checklist is open; 10.4's micro suite and agent benchmark are built but not run: benchmarks run after all plans. 10.5's release workflow and 10.6's Docker image have not run yet: no tag, no signing secrets, no license. The fixture site and CI matrix start in plan 01 and grow with every plan)
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
- [x] Parity matrix: no ❌ in agent-critical rows.
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
- 10.3: `docs/playwright-parity.md` has one table per area with the plan's four columns, 128 rows. It is the short form of `docs/migrating-from-playwright.md` (plan 09.4), which keeps the per-call examples; the statuses were checked against the commands' `--help` and code, which made three of the guide's rows stale: `browser.newContext()` is `context new` (✅, was 🟡), HAR replay is `mock import FILE.har` + `mock use` (✅, was ❌; not `route --from-har`, since mock sets already replay requests), and `APIRequestContext` is `http` (✅, was 🟡). The guide is fixed too.
- 10.3: Agent-critical means an agent needs it routinely to open, operate or verify a page, or to write and run E2E tests, with no reasonable way around it. The three ❌ rows (`filter({ has, hasNot })`, `or()`/`and()`, paused timers) are not agent-critical, so the v1 gate holds; they stay `.looper` follow-ups.
- 10.3: Video is ➖, not in v1: an agent cannot watch a video, a failed test already leaves step screenshots (`test --trace`) and a `report.md`, and `Page.startScreencast` needs an ack per frame, so it would need its own recorder and an ffmpeg dependency for little agent value.
- 10.3: `crates/chrome-cdp/tests/parity.rs` keeps the matrix honest in CI: every row has a status and a yes/no, no agent-critical row is ❌, the summary line's counts match, and every command a row names (the first word of a code span in the chrome-cdp column) is in `help commands`. Playwright MCP's tools got no table of their own: each maps to a row here (`browser_generate_locator` to `history --as-scenario`).
- 10.4: The harness is a crate, `crates/bench` (it needs serde, TOML and the fixture server as a library), which `cargo xtask bench <micro|tokens|agent> [-- args]` runs after building the release `chrome-cdp` and `fixtures`; xtask stays std-only, as with `gen-cdp`. Each suite writes JSON to `target/bench/` and rewrites its own section of `docs/benchmarks.md` (between `<!-- bench:… -->` markers), which also holds the methodology and how to run each suite.
- 10.4: Micro times every chrome-cdp step as a whole CLI invocation (process start, socket round trip, daemon work), as an agent pays it. The Playwright side (`bench/playwright/micro.mjs`, `playwright-core` on the same Chrome) times calls inside one Node process, since the library has no CLI, except the cold start, which times a fresh `node` launching Chrome. Route overhead is the difference of two page loads' medians (50 images, 20 rules of which the last continues every image with a header) over 51 requests; the engine's `overhead_with_20_rules` keeps plan 07's p95 check. Added locator timings on the `huge` page (the `.looper` locator-benchmark note).
- 10.4: `tests/bench.rs`'s cold/warm and batch tests moved into the micro suite, which reports each plan budget (cold < 1.5 s, warm p50 < 30 ms, batch ≥ 3x, route < 1 ms/request) as met or not. The ignored timing tests that check budgets inside the engine (plan 01's 10,000 evaluations, plan 04's snapshot latency, plan 07's route p95, plan 11's runner overhead) stay tests; `xtask bench micro` runs them after the suite (`--no-tests` skips them).
- 10.4: The 15% gate is `--check` against a committed baseline in `bench/baselines/` (`--update-baseline` records one): per OS and architecture for micro (timings depend on the machine; without a baseline it warns, as `dist`'s size check does), per tokenizer for tokens (counts are exact). A nightly `bench` job runs both on Linux and macOS with `--no-tests` (shared runners are too noisy for absolute budgets) and files an issue on failure.
- 10.4: The token suite's tokenizer is the Anthropic token counting API (`claude-opus-5-5`, through `@anthropic-ai/sdk`) when `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` is set, else offline o200k_base as an approximation; the results name it. It ran here with o200k_base (no key): over the 13 corpus pages `snapshot` is 14,625 tokens and `snapshot -i` 11,593, against Playwright MCP's 476,225 (3%; 8% on `big-app`, 2% on `huge`, 44-68% on small pages); the same as plan 04's close-out, and the recorded baseline. The 5 Playwright refs with no chrome-cdp ref (1 on `hidden`, 4 on `modal`) are inert elements, as plan 04 found. The corpus is still plan 04's 13 pages; a larger real-world one is a `.looper` follow-up.
- 10.4: The agent benchmark has 26 tasks in `bench/agent/tasks/` (format in its README): 20 on fixture pages under `testdata/site/bench/`, a bug to fix on a local dev server, 3 on the-internet.herokuapp.com (`--offline` skips them), and plan 11's three E2E tasks on examples/e2e-shop. Success is decided by a verifier, never the agent's report: the fixture server keeps what a page posted (`POST /bench/record`, new), a `batch` script checks a fixed app, `chrome-cdp test` checks a suite, `unchanged` paths stop an agent from editing the tests, and only questions read the reply (its last `ANSWER:` line, ignoring case, spaces, commas, `$` and quotes).
- 10.4: Both arms run `claude -p --output-format json --setting-sources project --strict-mcp-config --dangerously-skip-permissions` with the same built-in tools (Bash, Read, Edit, Write, Glob, Grep, Skill) and a one-line preamble naming the browser tool; chrome-cdp's arm gets `chrome-cdp skill --install` and the binary on PATH, Playwright MCP's gets the MCP server (headless, isolated, the same Chrome) and a PATH without `chrome-cdp`. Tokens are summed over `modelUsage` (input, output, cache reads and writes); turns are `num_turns`; wall time is the whole process. A run past its task's limit is killed with its process group and fails.
- 10.4: Two of plan 11's E2E tasks (write scenarios and mocks; pass against the real backend) run only in the chrome-cdp arm: they are about chrome-cdp's runner and mock files, which Playwright MCP has no counterpart for. "Fix an app change that broke checkout" runs in both, since fixing the app needs only a browser.
- 10.4: `bench agent --check` runs no agent: each task's verifier must fail on the untouched task and pass after its `[solution]` (a batch script, file edits or the answer). All 26 pass it here. Writing the solutions found that `dialog accept` fails when the page opens its next dialog a moment after the previous one closed (a confirm then a prompt): a `.looper` follow-up.
- 10.4: Not run: the micro suite, the timing tests and the agent benchmark (deferred: benchmarks run after all plans). So the v1 checklist's "Benchmarks published; the agent benchmark beats Playwright MCP" stays open, as do the timing criteria of plans 02, 07, 08 and 11 and the overview's.
- 10.5: No cargo-dist: `.github/workflows/release.yml` builds each target with the existing `cargo xtask dist` (the +20% size gate and the smoke test, on each target's own architecture), and a std-only `cargo xtask release` makes the rest from `dist/`: `archive` (tarball + `.sha256`), `manifest` (`SHA256SUMS`, `manifest.json`), `notes`, `formula`, `npm` and `check`. The same steps, short of publishing, run in CI's `release packaging` job on every push, so a release tag does not meet a broken step; cargo-dist's generated workflow would have duplicated the build matrix and bypassed the size gate.
- 10.5: Archives are named `chrome-cdp-<target>.tar.gz`, without the version, so `releases/latest/download/…` always names the newest and `install.sh` needs no GitHub API call (60 requests an hour unauthenticated). Each release also has `manifest.json` (version, and each target's archive and SHA-256), which `upgrade` reads the same way; install.sh checks the archive's `.sha256`.
- 10.5: `cargo install chrome-cdp` needs every workspace crate on crates.io, and their short names (`cdp`, `browser`, `engine`, …) are taken there, so the published crates are now `chrome-cdp-<name>`, each keeping its short lib name (no code changes). They package and build standalone (`cargo package --workspace`, verified here). `bench`, `fixtures`, `cdp-gen` and `xtask` stay unpublished. `cargo binstall` takes the release archives (`[package.metadata.binstall]`).
- 10.5: No license is chosen yet (the `.looper` choose-license note: it is the owner's call), so publishing stays off in `[workspace.package]` (`publish = false`, one line to flip with the license): cargo-deny fails on publishable crates without a license, and crates.io refuses them. `release check` warns about both, and the crates.io job skips while publishing is off. docs/releasing.md lists what the first release needs.
- 10.5: Release notes are the CHANGELOG.md section of the version, then the commits since the previous tag grouped by their conventional-commit type (breaking, features, fixes, performance, changes, docs, tests, CI, maintenance). This repo's subjects are mostly `component: …`, so an unknown prefix goes under "Changes". The first release lists no commits (it would be the whole history).
- 10.5: macOS signing and notarization are optional steps (`.github/scripts/macos-sign.sh`, run when `APPLE_CERTIFICATE` is set; notarization also needs `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`). A bare Mach-O binary cannot be stapled, so Gatekeeper checks the notarization online. Until the secrets exist the binaries are unsigned, and docs/install.md documents `xattr -d com.apple.quarantine`; the install script removes the attribute itself, and `doctor` reports it.
- 10.5: Homebrew gets a formula (not a cask) in `glotlabs/homebrew-tap`, generated from the sums with `on_macos`/`on_linux` × `on_arm`/`on_intel` blocks and shell completions; pre-releases skip it. npm has a Node launcher package `chrome-cdp` and unscoped platform packages `chrome-cdp-<os>-<cpu>` (no npm organization needed) as optional dependencies with `os`/`cpu` set; the launcher passes exit codes through and exits 4 with a hint when the platform package is missing. crates.io, Homebrew and npm each run only when their token secret is set, and warn otherwise.
- 10.5: `upgrade` replaces only standalone installs: a binary under Homebrew's `Cellar`, a `node_modules` or cargo's `bin` is refused (BadArgs, exit 2) with that package manager's command (`--check` still reports, with that command). It downloads to memory, checks the SHA-256 against the manifest, unpacks next to the binary, runs the new one's `--version -q` and compares it with the manifest's version, then renames it over; any failure leaves the old binary and no staged file. Running sessions restart on the new build at their next command (the build id check from plan 02), which the output says. `CHROME_CDP_RELEASES_URL` points at a mirror; the tests serve releases from a local server.
- 10.5: `doctor` runs eight checks: the binary, the Chrome `open` would launch (a warning when older than `MIN_CHROME_MAJOR`), its libraries and Ubuntu's AppArmor userns rule (Linux), a real headless launch with a temporary profile (the only sure test of the sandbox; `--no-launch` skips it), the session dir's owner and 0700 mode, quarantine (macOS), and live sessions on another chrome-cdp version ("version skew"; the CDP protocol pin against Chrome's version is the `.looper` cdp-protocol-vs-installed-chrome note). A failed check fails the command with exit 4 (ChromeNotFound, LaunchFailed or DaemonUnavailable after the first failed check), every check in the notes and `details`. Every CI runner now runs it against its Chrome.
- 10.5: Verified here: `install.sh` (newest, pinned, bad checksum, missing release; shellcheck), the npm launcher (packed packages installed and run with `npx`), `upgrade` against a local server, `doctor` against Chrome for Testing 154 with the sandbox on, and `release archive`/`manifest`/`npm`/`notes`/`check` on the Linux musl builds. Not run: the release workflow itself (no tag or secrets; actionlint passes), signing and notarization (no macOS here), and installs from the real brew tap, npm and crates.io, so the v1 checklist's "installs verified on clean VMs" and the overview's one-command install stay open.
- 10.6: The command reference is `docs/commands/`, one page per command (its subcommands, options and examples) and an index by task with the global options, generated from the clap definitions by a test in the binary's help module (`help::reference`). `cargo xtask docs` runs that test with `UPDATE_DOCS=1`, as the golden tests do with `UPDATE_GOLDEN`: xtask stays std-only, and a plain `cargo test` fails when a command's help changed without the docs.
- 10.6: The README follows the plan (pitch, install, a 6-command quickstart, E2E as the headline feature, the comparison table); its long walkthrough of every command moved, section by section, to `docs/guide.md`, and its development notes to `docs/development.md`. `docs/README.md` indexes the docs.
- 10.6: The comparison table has the token suite's numbers from 10.4. Its agent-success and latency cells say "pending" (deferred: benchmarks run after all plans). Playwright CLI cells nobody has measured or checked are dashes rather than guesses.
- 10.6: The guides are `locators-and-refs.md`, `waiting.md`, `debugging-a-dev-server.md`, `testing.md` (`.cdp` files: batch, run, expect, test), `ci.md`, `docker.md`, `security.md` and `troubleshooting.md` (a section per error kind, by exit code); `migrating-from-playwright.md` already existed. `security.md` is the user-facing model; 10.7's threat model can extend it. `crates/chrome-cdp/tests/guides.rs` checks every relative link of the README and `docs/` (file and heading), that code blocks run real commands, and that every `ErrorKind` has its troubleshooting section.
- 10.6: The Docker image is `docker/Dockerfile`: Debian slim, the release's static musl binary, Chrome for Testing with `install --with-deps`, and the same font set as `docs/e2e/Dockerfile`, as a non-root user so Chrome keeps its sandbox where the container allows user namespaces (Docker's default seccomp profile does not, and chrome-cdp then falls back with its warning). The release workflow's `docker` job builds it for amd64 and arm64 and pushes `ghcr.io/glotlabs/chrome-cdp:<version>` and `:latest`. The amd64 image was built and run here (`doctor`, a session, `batch`, sandbox on with `seccomp=unconfined`); the arm64 build and the push have not run. `docs/e2e/Dockerfile` (with Node, built from a checkout) stays for E2E suites whose web server needs Node.
- 10.6: `SKILL.md` stays at `crates/chrome-cdp/src/help/SKILL.md`, where `include_str!` can reach it; the docs link it. The agent benchmark installs it with `skill --install`, so it tests what agents read, once it runs.
- 10.6: Writing the guides found two help examples that passed a bare `h1` as a locator (`expect h1 text …`, `batch`'s `text h1`), which fail with `BadLocator`; they use `css=h1` now. `init e2e`'s workflow installs chrome-cdp with `install.sh` instead of `cargo install`, which compiled the whole tree on every run.
- 10.7: The threat model is `docs/threat-model.md` (assets, who is trusted, each boundary's defences with the file that enforces them, what is powerful on purpose, what is left), linked from `security.md`, which stays the user-facing summary. The review checked every claim of both against the code, with read-only review passes over sockets and session dirs, files that hold secrets, page-to-daemon channels and page worlds, the mock and fixture servers, and install/upgrade; what it found is fixed in 10.7's commits or listed under "What is left" and in `.looper` notes.
- 10.7: Fixed by the review: a crafted HAR's method could name a path outside the mocks dir in `mock import` (the method is now cleaned and must be an HTTP token, and the writer refuses non-plain paths); the Chrome zip's symlink check could be beaten by a chain of links (nothing is written through a symlink now); page text could send terminal escape sequences (text output shows control characters as symbols, `protocol::format::printable`); HAR, trace and response-body files were 0644 (now 0600); downloads accepted plain-HTTP mirrors and HTTPS-to-HTTP redirects (`browser::download`: HTTPS, or HTTP only to loopback; install.sh uses curl `--proto =https --proto-redir =https`); `status`, `close` and `sessions` trusted session dirs under `/tmp` that another user could have made; an existing named profile dir kept a wide mode; `--chrome-arg=--remote-debugging-port` passed silently (it warns); the AppArmor profile took a cache path with a newline.
- 10.7: `--http-credentials` answered every origin's challenge (the `.looper` http-credentials-origin note). The default stays Playwright's (any origin), and `--http-credentials-origin` scopes it; a proxy's challenge gets the credentials only when its origin is the one named, and `history` shows `--http-credentials ***`.
- 10.7: `connect` warns rather than refuses a non-loopback endpoint (remote browsers are a real use, e.g. a container or VM); it checks both the endpoint typed and the WebSocket URL `/json/version` returns, since that may name another host.
- 10.7: `cargo deny` already ran in CI (since the workspace began); `cargo audit` joins it as its own job, and a nightly `audit` job reruns both advisory checks, since advisories arrive without a commit. Both are clean here (279 crates).
- 10.7: The "RPC decoder" is `protocol::rpc::decode_request`/`decode_response`/`decode_params`, now the daemon's and CLI's only way to read a line; its fuzz target also round-trips what it reads and renders any response in every mode. Its first minutes found that serde_json's default float parsing is off by one ULP for some numbers, so a number from `eval` could change between Chrome, the daemon and the CLI; the workspace enables serde_json's `float_roundtrip`. `cargo xtask fuzz [--time S]` runs every target from committed seeds (`fuzz/seeds/`) and a corpus the nightly `fuzz` job caches (5 minutes per target), and `xtask ci` checks the fuzz crate's format and clippy so the targets keep building (they had not built since the crates were renamed in 10.5). Both targets ran clean here (10 million inputs for the decoder).
- 10.7: Left as `.looper` notes: signed releases (the checksums come from the same host as the archives), the commands `chrome-cdp.toml` runs without showing them first, `mock serve`'s any-origin CORS and missing `Host` check, a download quota, the page-world `goto` title/status and `emulate` checks, and the mock shim binding's trust in the page's URL.
